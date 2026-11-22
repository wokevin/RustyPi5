"""Exercise the prepared QEMU disk: real E1000 DMA, local filtering and UDP.

Uses disposable firmware variables and snapshot disks, never formats the base image.
The loopback Ethernet peer is test-only; the guest kernel enforces the firewall.
"""

import argparse
import contextlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time

import cpu_boot_smoke

MAC = bytes.fromhex("525400123456")
PEER = bytes.fromhex("525400abcdef")
GUEST_IP = bytes([10, 0, 2, 15])
PEER_IP = bytes([10, 0, 2, 2])


def checksum(data):
    if len(data) % 2:
        data += b"\0"
    value = sum(struct.unpack("!" + "H" * (len(data) // 2), data))
    while value >> 16:
        value = (value & 65535) + (value >> 16)
    return (~value) & 65535


def repair(frame):
    frame[24:26] = b"\0\0"
    struct.pack_into("!H", frame, 24, checksum(bytes(frame[14:34])))
    frame[40:42] = b"\0\0"
    length = struct.unpack_from("!H", frame, 38)[0]
    pseudo = bytes(frame[26:34]) + b"\0\x11" + struct.pack("!H", length)
    struct.pack_into("!H", frame, 40, checksum(pseudo + bytes(frame[34:34 + length])) or 65535)


def response(request):
    length = struct.unpack_from("!H", request, 16)[0] + 14
    frame = bytearray(request[:length])
    frame[:12] = MAC + PEER
    frame[26:34] = PEER_IP + GUEST_IP
    struct.pack_into("!HH", frame, 34, 8081, 49152)
    repair(frame)
    return frame


def receive_exact(connection, count):
    result = bytearray()
    while len(result) < count:
        part = connection.recv(count - len(result))
        if not part:
            raise EOFError("QEMU socket closed")
        result.extend(part)
    return bytes(result)


class Gateway:
    def __init__(self, mode):
        self.mode = mode
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.listener.settimeout(.2)
        self.port = self.listener.getsockname()[1]
        self.connection = None
        self.stop = threading.Event()
        self.error = None
        self.disconnected = False
        self.frames = []
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def send(self, frame):
        self.connection.sendall(struct.pack("!I", len(frame)) + frame)

    def run(self):
        try:
            while not self.stop.is_set():
                try:
                    self.connection, _ = self.listener.accept()
                    break
                except socket.timeout:
                    continue
            if self.connection is None:
                return
            self.connection.settimeout(15)
            handled = False
            while not self.stop.is_set():
                length = struct.unpack("!I", receive_exact(self.connection, 4))[0]
                assert 14 <= length <= 2048, length
                frame = receive_exact(self.connection, length)
                self.frames.append(frame)
                if frame[12:14] == b"\x08\x06":
                    assert frame[28:32] == GUEST_IP and frame[38:42] == PEER_IP
                    reply = bytearray(60)
                    reply[:14] = MAC + PEER + b"\x08\x06"
                    reply[14:22] = b"\0\x01\x08\0\x06\x04\0\x02"
                    reply[22:42] = PEER + PEER_IP + MAC + GUEST_IP
                    self.send(reply)
                elif frame[12:14] == b"\x08\0":
                    if handled:
                        if self.mode == "disconnect":
                            # A retry follows a guest poll, allowing injected packets to drain
                            # before TCP EOF can discard QEMU's pending socket receive data.
                            self.connection.close()
                            self.connection = None
                            self.disconnected = True
                            return
                        continue
                    handled = True
                    assert frame[23] == 17 and frame[26:34] == GUEST_IP + PEER_IP
                    assert struct.unpack_from("!HH", frame, 34) == (49152, 8081)
                    assert frame[42:] == b"hello"
                    reply = response(frame)
                    for offset in (26, 34, 36, 42):
                        bad = reply.copy()
                        bad[offset] ^= 2
                        repair(bad)
                        self.send(bad)
                    bad = reply.copy()
                    bad[24] ^= 1
                    self.send(bad)
                    if self.mode == "reply":
                        time.sleep(.05)
                        self.send(reply)
        except (EOFError, OSError) as error:
            if not self.stop.is_set() and self.mode != "disconnect":
                self.error = error
        except Exception as error:
            self.error = error

    def close(self):
        self.stop.set()
        if self.connection is not None:
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
        self.listener.close()
        self.thread.join(timeout=3)
        assert not self.thread.is_alive(), "Gateway thread did not stop"


def capture_frames(path):
    data = path.read_bytes()
    assert data[:4] == b"\xd4\xc3\xb2\xa1", "Unexpected PCAP format"
    offset = 24
    frames = []
    while offset < len(data):
        assert offset + 16 <= len(data)
        length = struct.unpack_from("<I", data, offset + 8)[0]
        offset += 16
        assert offset + length <= len(data)
        frames.append(data[offset:offset + length])
        offset += length
    return frames


def run(qemu, project, mode):
    target = project / "target" / "network-boot-tests"
    target.mkdir(parents=True, exist_ok=True)
    output, errors = bytearray(), bytearray()
    gateway = None
    process = connection = monitor = None
    threads = []
    temporary = None
    try:
        temporary = tempfile.TemporaryDirectory(prefix=mode + "-", dir=target)
        gateway = Gateway(mode)
        # Cleanup is deferred until finally, after QEMU releases its firmware handle.
        with contextlib.nullcontext(temporary.name) as directory:
            work = Path(directory)
            firmware = qemu.parent / "share"
            variables = work / "vars.fd"
            shutil.copyfile(firmware / "edk2-i386-vars.fd", variables)
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            def escape(path):
                return str(path).replace(",", ",,")
            backend = f"socket,id=net0,connect=127.0.0.1:{gateway.port}"
            pcap = target / (mode + ".pcap")
            args = [str(qemu), "-machine", "pc", "-accel", "tcg", "-cpu", "max", "-smp", "2", "-m", "512",
                    "-display", "none", "-serial", "stdio", "-monitor", "none",
                    "-drive", f"if=pflash,format=raw,unit=0,readonly=on,file={escape(firmware / 'edk2-x86_64-code.fd')}",
                    "-drive", f"if=pflash,format=raw,unit=1,file={escape(variables)}",
                    "-drive", f"if=ide,index=0,format=raw,snapshot=on,file={escape(project / 'rustypi5.img')}",
                    "-netdev", backend, "-device", "e1000,netdev=net0,mac=52:54:00:12:34:56,addr=3",
                    "-object", f"filter-dump,id=capture0,netdev=net0,file={escape(pcap)}",
                    "-qmp", f"tcp:127.0.0.1:{port},server=on,wait=off", "-no-reboot", "-no-shutdown",
                    "-d", "guest_errors,cpu_reset", "-D", str(target / (mode + ".qemu.log"))]
            assert args.count("-netdev") == 1 and not any("hostfwd" in arg for arg in args)
            assert backend.startswith("socket,id=net0,connect=127.0.0.1:"), backend
            assert not any(arg.startswith(("user,", "tap,", "bridge,")) for arg in args)
            process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            def drain(stream, buffer):
                while byte := stream.read(1):
                    buffer.extend(byte)
            for stream, buffer in [(process.stdout, output), (process.stderr, errors)]:
                thread = threading.Thread(target=drain, args=(stream, buffer), daemon=True)
                thread.start()
                threads.append(thread)
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                try:
                    connection = socket.create_connection(("127.0.0.1", port), timeout=1)
                    break
                except OSError:
                    if process.poll() is not None:
                        raise AssertionError(errors.decode(errors="replace"))
                    time.sleep(.05)
            assert connection is not None, "QMP did not start"
            connection.settimeout(10)
            monitor = connection.makefile("rwb", buffering=0)
            json.loads(monitor.readline())
            def command(name, arguments=None):
                request = {"execute": name}
                if arguments is not None:
                    request["arguments"] = arguments
                monitor.write(json.dumps(request).encode() + b"\n")
                while True:
                    reply = json.loads(monitor.readline())
                    if "error" in reply:
                        raise AssertionError(reply)
                    if "return" in reply:
                        return reply["return"]
            command("qmp_capabilities")
            assert command("query-status")["running"]
            prompt = b"User A [ring 3]$ "
            def wait_for(needle, start=0):
                deadline = time.monotonic() + 60
                while needle not in output[start:] and time.monotonic() < deadline:
                    assert process.poll() is None, errors.decode(errors="replace")
                    if b"Kernel panic" in output or b"Networking stopped:" in output:
                        break
                    time.sleep(.025)
                assert needle in output[start:], output.decode(errors="replace") + errors.decode(errors="replace")
            wait_for(prompt)
            start = len(output)
            for byte in b"udp 10.0.2.2 8081 hello":
                before = len(output)
                process.stdin.write(bytes([byte]))
                process.stdin.flush()
                wait_for(bytes([byte]), before)
            process.stdin.write(b"\n")
            process.stdin.flush()
            expected = b"reply: hello" if mode == "reply" else b"udp: timed out"
            wait_for(expected, start)
            wait_for(prompt, start)
            assert b"kernel-only DMA; PIT-calibrated polling" in output
            assert b"supervisor-only uncached NX" in output
            dropped = int(re.search(rb"RX=\d+ dropped=(\d+)", output)[1])
            assert dropped >= 5, output.decode(errors="replace")
            # Inspect permissions and confirm the NIC remains active for later requests.
            time.sleep(.1)
            command("stop")
            registers = command("human-monitor-command", {"command-line": "info registers -a"})
            cr3 = int(re.search(r"CR3=([0-9a-fA-F]+)", registers)[1], 16)
            def peek(address):
                text = command("human-monitor-command", {"command-line": f"xp /1gx 0x{address:x}"})
                return int(re.search(r":\s*(0x[0-9a-fA-F]+)", text)[1], 16)
            def permissions(address):
                table = cr3
                user = writable = executable = True
                for shift in (39, 30, 21, 12):
                    entry = peek((table & 0x000FFFFFFFFFF000) + ((address >> shift) & 511) * 8)
                    assert entry & 1
                    user &= bool(entry & 4)
                    writable &= bool(entry & 2)
                    executable &= not bool(entry & (1 << 63))
                    if shift == 12 or (shift in (30, 21) and entry & 128):
                        return user, writable, executable, entry
                    table = entry
            pci = command("query-pci")[0]["devices"]
            nic = next(device for device in pci if device["slot"] == 3)
            bar = next(region for region in nic["regions"] if region["bar"] == 0)
            assert bar["size"] == 0x20000
            attributes = permissions(bar["address"])
            assert attributes[:3] == (False, True, False), attributes
            assert attributes[3] & 24 == 24, "NIC MMIO is not uncached"
            command("human-monitor-command", {"command-line": "o /l 0xcf8 0x80001804"})
            config = command("human-monitor-command", {"command-line": "i /w 0xcfc"})
            pci_command = int(re.search(r"=\s*(0x[0-9a-fA-F]+)", config)[1], 16)
            assert pci_command & 4, "Persistent NIC bus mastering is not enabled"
            assert peek(bar["address"] + 0x100) & 2, "Persistent NIC receive is not enabled"
            assert peek(bar["address"] + 0x400) & 2, "Persistent NIC transmit is not enabled"
            # APIC registers are CPU-local, not readable through debug physical-memory access.
            lapic = command("human-monitor-command", {"command-line": "info lapic"})
            timer = re.search(r"LVTT\s+0x([0-9a-fA-F]+)", lapic)
            assert timer is not None and not int(timer[1], 16) & (1 << 16), lapic
            symbols, _ = cpu_boot_smoke.layout(project)
            dma = [(address, size) for name, (address, size) in symbols.items()
                   if "network" in name and "3DMA" in name]
            assert len(dma) == 1, dma
            for address in (dma[0][0], dma[0][0] + dma[0][1] - 1):
                assert not permissions(address)[0], "DMA memory is user-accessible"
            network_info = command("human-monitor-command", {"command-line": "info network"})
            assert "type=socket" in network_info, network_info
            if gateway is not None:
                gateway.close()
                assert gateway.error is None, gateway.error
                if mode == "disconnect":
                    assert gateway.disconnected, "Peer did not perform the requested disconnect"
                gateway = None
            command("quit")
            process.wait(timeout=10)
            for thread in threads:
                thread.join(timeout=2)
            frames = capture_frames(pcap)
            assert any(frame[12:14] == b"\x08\x06" and frame[6:12] == MAC for frame in frames)
            assert any(frame[12:14] == b"\x08\0" and frame[6:12] == MAC for frame in frames)
            if mode == "reply":
                assert any(frame[12:14] == b"\x08\0" and frame[:6] == MAC for frame in frames)
            print(f"PASS {mode}: ring-3 UDP, filtering, MMIO/DMA isolation, persistent device and capture")
    finally:
        if process is not None:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            for thread in threads:
                thread.join(timeout=2)
            for stream in (process.stdin, process.stdout, process.stderr):
                stream.close()
        if monitor is not None:
            monitor.close()
        if connection is not None:
            connection.close()
        if gateway is not None:
            gateway.close()
        (target / (mode + ".serial.log")).write_bytes(output)
        (target / (mode + ".stderr.log")).write_bytes(errors)
        if temporary is not None:
            temporary.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--qemu", type=Path, default=Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "qemu" / "qemu-system-x86_64.exe")
    parser.add_argument("--modes", nargs="+", choices=["reply", "deny", "disconnect"], default=["reply", "deny", "disconnect"])
    args = parser.parse_args()
    project = Path(__file__).resolve().parent.parent
    image = project / "rustypi5.img"
    before = cpu_boot_smoke.digest(image)
    try:
        for mode in args.modes:
            run(args.qemu, project, mode)
    finally:
        assert cpu_boot_smoke.digest(image) == before, "Persistent disk changed"


if __name__ == "__main__":
    main()

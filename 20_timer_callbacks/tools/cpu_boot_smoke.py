"""Boot the prepared QEMU disk with 1/2/4 CPUs and verify dedicated kernel placement."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import threading
import time


def layout(project):
    elf = (project / "target" / "qemu-x86_64" / "kernel" / "x86_64-unknown-none" / "release" / "RustyPi5").read_bytes()
    section_offset = struct.unpack_from("<Q", elf, 40)[0]
    section_size, section_count = struct.unpack_from("<HH", elf, 58)
    sections = [struct.unpack_from("<IIQQQQIIQQ", elf, section_offset + index * section_size) for index in range(section_count)]
    symbols = {}
    for section in sections:
        if section[1] != 2:
            continue
        strings = sections[section[6]]
        names = elf[strings[4]:strings[4] + strings[5]]
        for offset in range(section[4], section[4] + section[5], section[9]):
            name, _, _, _, address, size = struct.unpack_from("<IBBHQQ", elf, offset)
            text = names[name:names.index(b"\0", name)].decode("ascii")
            symbols[text] = (address, size)
    reserved = []
    for token, size in [("11KERNEL_HEAP", 1024 * 1024), ("12RING0_STACKS", 64 * 32 * 1024),
                        ("21SECONDARY_CORE_STACKS", 63 * 128 * 1024)]:
        found = [(address, actual) for name, (address, actual) in symbols.items() if token in name and actual == size]
        assert len(found) == 1, (token, found)
        address, actual = found[0]
        assert address % 4096 == 0
        reserved.append((address, address + actual))
    program_offset = struct.unpack_from("<Q", elf, 32)[0]
    program_size, program_count = struct.unpack_from("<HH", elf, 54)
    loads = []
    for index in range(program_count):
        kind, flags, _, address, _, _, memory_size, _ = struct.unpack_from("<IIQQQQQQ", elf, program_offset + index * program_size)
        if kind == 1:
            loads.append((address, address + memory_size, flags))
    assert any(start <= 0x7000 and end >= 0x8000 for start, end, _ in loads), "Bootstrap scratch is not loader-reserved"
    for index, (start, end) in enumerate(reserved):
        assert any(first <= start and last >= end and flags & 2 for first, last, flags in loads), "Kernel reserve is not backed by a writable ELF load segment"
        assert end <= 0x10000000 or start >= 0x10200000
        for other_start, other_end in reserved[:index]:
            assert end <= other_start or start >= other_end
    return symbols, reserved


def run(qemu, project, count, image=None, script_fixture=False):
    symbols, reserved = layout(project)
    output_dir = project / "target" / "cpu-boot-tests"
    output_dir.mkdir(parents=True, exist_ok=True)
    prefix = output_dir / f"cpu-{count}"
    firmware = qemu.parent / "share"
    variables = prefix.with_suffix(".fd")
    shutil.copyfile(firmware / "edk2-i386-vars.fd", variables)
    if image is None:
        image = project / "rustypi5.img"
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    def escape(path):
        return str(path).replace(",", ",,")
    args = [str(qemu), "-machine", "pc", "-accel", "tcg", "-cpu", "max", "-smp", f"{count},sockets=1,cores={count},threads=1",
            "-m", "512", "-display", "none", "-net", "none", "-serial", "stdio",
            "-drive", f"if=pflash,format=raw,unit=0,readonly=on,file={escape(firmware / 'edk2-x86_64-code.fd')}",
            "-drive", f"if=pflash,format=raw,unit=1,file={escape(variables)}",
            "-drive", f"if=ide,index=0,format=raw,snapshot=on,file={escape(image)}",
            "-qmp", f"tcp:127.0.0.1:{port},server=on,wait=off", "-no-reboot", "-no-shutdown",
            "-d", "guest_errors,cpu_reset", "-D", str(prefix.with_suffix(".qemu.log"))]
    process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output, errors = bytearray(), bytearray()
    def drain(stream, target):
        while True:
            byte = stream.read(1)
            if not byte:
                return
            target.extend(byte)
    threads = []
    for stream, target in [(process.stdout, output), (process.stderr, errors)]:
        thread = threading.Thread(target=drain, args=(stream, target), daemon=True)
        thread.start()
        threads.append(thread)
    connection = monitor = None
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            try:
                connection = socket.create_connection(("127.0.0.1", port), timeout=1)
                break
            except OSError:
                if process.poll() is not None:
                    raise RuntimeError(errors.decode(errors="replace"))
        if connection is None:
            raise RuntimeError("QEMU monitor did not start")
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
                    raise RuntimeError(reply)
                if "return" in reply:
                    return reply["return"]
        command("qmp_capabilities")
        assert command("query-status")["running"]
        def wait_for(needle, start=0):
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if needle in output[start:]:
                    return bytes(output[start:]).decode(errors="replace")
                if process.poll() is not None:
                    break
                time.sleep(.025)
            registers = command("human-monitor-command", {"command-line": "info registers -a"})
            raise AssertionError(f"Missing {needle!r}\n{output.decode(errors='replace')}\n{registers}")
        if count == 1:
            wait_for(b"Dedicated kernel CPU requires at least two CPUs")
            assert b"User A [ring 3]$ " not in output
            print("PASS 1 CPU: explicit refusal; no user execution on kernel CPU")
        else:
            wait_for(b"User A [ring 3]$ ")
            boot = output.decode(errors="replace")
            assert f"CPU topology: {count} discovered, {count} online" in boot, boot
            assert "CPU 0: APIC 0, online=true, role=kernel" in boot, boot
            assert "ring 3 on CPU 1" in boot, boot
            assert "Hardware IRQ mask/restore verification PASS on CPU 0" in boot, boot
            assert "Kernel worker verification: 16 cross-core completions and wake IRQs PASS" in boot, boot
            probes = re.findall(r"Kernel worker: CPU (\d+) probe (\d+) processed on CPU 0; wake IRQs=(\d+)", boot)
            assert len(probes) == 16, probes
            assert [int(sequence) for _, sequence, _ in probes] == list(range(1, 17)), probes
            assert all(int(source) == 1 for source, _, _ in probes), probes
            assert all(int(interrupts) >= index for index, (_, _, interrupts) in enumerate(probes, 1)), probes
            for index in range(1, count):
                assert f"CPU {index}: APIC {index}, online=true, role=user-eligible" in boot, boot
            registers = command("human-monitor-command", {"command-line": "info registers -a"})
            prefix.with_suffix(".registers.log").write_text(registers, encoding="ascii")
            cpus = re.split(r"CPU#\d+", registers)[1:]
            assert len(cpus) == count
            assert "HLT=1" in cpus[0] and "CPL=0" in cpus[0], cpus[0]
            def register(cpu, name):
                return int(re.search(name + r"\s*=\s*([0-9a-fA-F]+)", cpu)[1], 16)
            assert register(cpus[0], "RFL") & (1 << 9), "Sleeping worker has interrupts disabled"
            assert not register(cpus[1], "RFL") & (1 << 9), "User CPU unexpectedly enabled hardware IRQs"
            tss = [int(re.search(r"TR\s*=0030 ([0-9a-fA-F]+)", cpu)[1], 16) for cpu in cpus]
            assert len(set(tss)) == count, "CPUs share a TSS"
            def peek(address):
                value = command("human-monitor-command", {"command-line": f"xp /1gx 0x{address:x}"})
                return int(re.search(r":\s*(0x[0-9a-fA-F]+)", value)[1], 16)
            stacks = [peek(address + 4) for address in tss]
            assert len(set(stacks)) == count, "CPUs share a supervisor entry stack"
            entry_start, entry_end = reserved[1]
            assert all(entry_start < top <= entry_end and top % 4096 == 0 for top in stacks)
            assert reserved[1][0] <= register(cpus[1], "RSP") < reserved[1][1], "Input syscall is not on its supervisor stack"
            assert register(cpus[0], "RSP") != register(cpus[1], "RSP")
            cr3 = register(cpus[0], "CR3")
            def permissions(address):
                table = cr3
                user = writable = True
                executable = True
                for shift in (39, 30, 21, 12):
                    entry = peek((table & 0x000ffffffffff000) + ((address >> shift) & 511) * 8)
                    if not entry & 1:
                        return None
                    user &= bool(entry & 4)
                    writable &= bool(entry & 2)
                    executable &= not bool(entry & (1 << 63))
                    if shift == 12 or (shift in (30, 21) and entry & 128):
                        if address == 0xFEE00000:
                            assert entry & 24 == 24, "APIC MMIO is not mapped uncached"
                        return user, writable, executable
                    table = entry
            assert not permissions(0xFEE00000)[0], "APIC MMIO is user-accessible"
            for start, end in reserved:
                assert not permissions(start)[0] and not permissions(end - 1)[0], "Kernel reserve is user-accessible"
            user_stack = symbols["__user_stack_start"][0]
            assert permissions(user_stack) == (True, True, False)
            assert permissions(user_stack - 4096) is None, "Missing lower user-stack guard"
            assert permissions(symbols["__user_stack_end"][0]) is None, "Missing upper user-stack guard"
            text = symbols["__user_text_start"][0]
            assert permissions(text) == (True, False, True)
            print(f"PASS {count} CPUs: distinct TSS/entry stacks, supervisor-only reserves, NX user stack with guards")
            prompt = b"User A [ring 3]$ "
            cases = [("pwd", "/\r\n"), ("ls", "etc/\r\nhome/\r\ntmp/\r\nREADME.txt\r\n"),
                     ("cat /system/private.txt", "filesystem: Path unavailable"),
                     ("cd /home/usera", None), ("pwd", "/home/usera\r\n"),
                     ("cat welcome.txt", "print(" if script_fixture else "Welcome, User A."), ("echo alive", "alive\r\n")]
            for text, expected in cases:
                start = len(output)
                for character in text + "\r":
                    key = {" ": "spc", "/": "slash", ".": "dot", "\r": "ret"}.get(character, character)
                    command("human-monitor-command", {"command-line": "sendkey " + key + " 10"})
                    time.sleep(.035)
                result = wait_for(prompt, start).split(prompt.decode())[0]
                assert text in result, result
                if expected:
                    assert expected in result, (text, result)
            def serial_line(text, expected_prompt):
                start = len(output)
                for byte in text.encode("ascii"):
                    echo_start = len(output)
                    process.stdin.write(bytes([byte]))
                    process.stdin.flush()
                    wait_for(bytes([byte]), echo_start)
                command("human-monitor-command", {"command-line": "sendkey ret 10"})
                return wait_for(expected_prompt, start)
            assert "PATH=/home/usera/programs" in serial_line("path", prompt)
            assert "PATH=/home/usera:/home/usera/programs" in serial_line("path /home/usera:/home/usera/programs", prompt)
            assert "script unavailable" in serial_line("run /system/private.txt", prompt)
            assert "script unavailable" in serial_line("run missing.rps", prompt)
            assert "use absolute directories" in serial_line("path /tmp::/home/usera", prompt)
            assert "PATH=/home/usera:/home/usera/programs" in serial_line("path", prompt)
            if script_fixture:
                assert "PATH=/system:/home/usera" in serial_line("path /system:/home/usera", prompt)
                for name in ("welcome.txt", "./welcome.txt", "/home/usera/welcome.txt"):
                    result = serial_line("run " + name, prompt)
                    assert "from disk\r\n" in result and "a" * 1100 + "\r\n" in result, result
                    assert "script: line" not in result, result
            serial_line("script", b"script> ")
            serial_line("let count = 0;", b"script> ")
            serial_line("while (count < 3) {", b"script> ")
            serial_line("print(count);", b"script> ")
            serial_line("count = count + 1;", b"script> ")
            serial_line("}", b"script> ")
            result = serial_line(".end", prompt)
            assert "0\r\n1\r\n2\r\n" in result, result
            serial_line("script", b"script> ")
            serial_line("print(1 / 0);", b"script> ")
            result = serial_line(".end", prompt)
            assert "script: line 1: arithmetic error" in result, result
            serial_line("script", b"script> ")
            serial_line('if (1) { print("yes"); } else { print("no"); }', b"script> ")
            result = serial_line(".end", prompt)
            assert "yes\r\n" in result, result
            serial_line("script", b"script> ")
            serial_line("while (1) { }", b"script> ")
            result = serial_line(".end", prompt)
            assert "execution or nesting limit exceeded" in result, result
            serial_line("script", b"script> ")
            serial_line("print(1);", b"script> ")
            result = serial_line(".cancel", prompt)
            assert "\r\n1\r\n" not in result, result
            serial_line("script", b"script> ")
            start = len(output)
            for key in ("p", "r", "i", "n", "t", "shift-9", "2", "shift-equal", "3", "shift-0", "semicolon", "ret"):
                command("human-monitor-command", {"command-line": "sendkey " + key + " 10"})
                time.sleep(.035)
            assert "print(2+3);" in wait_for(b"script> ", start)
            assert "\r\n5\r\n" in serial_line(".end", prompt)
            assert "alive" in serial_line("echo alive", prompt)
            final_registers = command("human-monitor-command", {"command-line": "info registers -a"})
            kernel_cpu = re.split(r"CPU#\d+", final_registers)[1]
            assert "HLT=1" in kernel_cpu and "CPL=0" in kernel_cpu, kernel_cpu
            assert register(kernel_cpu, "RFL") & (1 << 9), kernel_cpu
            print(f"PASS {count} CPUs: 16 hardware IPI wakeups, IRQ restore, CPU 0 kernel-only; seven PS/2 command checks")
            print(f"PASS {count} CPUs: interactive scripts, branches, error/loop limits, PATH and access checks")
        command("quit")
        process.wait(timeout=10)
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        for thread in threads:
            thread.join(timeout=2)
        if monitor is not None:
            monitor.close()
        if connection is not None:
            connection.close()
        prefix.with_suffix(".serial.log").write_bytes(output)
        prefix.with_suffix(".stderr.log").write_bytes(errors)
        variables.unlink()


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        while data := stream.read(1024 * 1024):
            result.update(data)
    return result.digest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--qemu", type=Path, default=Path(os.environ.get("ProgramFiles", "C:\\Program Files")) / "qemu" / "qemu-system-x86_64.exe")
    parser.add_argument("--cpus", type=int, nargs="+", default=[1, 2, 4])
    args = parser.parse_args()
    project = Path(__file__).resolve().parent.parent
    image = project / "rustypi5.img"
    before = digest(image)
    try:
        for count in args.cpus:
            run(args.qemu, project, count)
    finally:
        assert digest(image) == before, "QEMU changed the persistent base image"


if __name__ == "__main__":
    main()

"""Clean-build and exercise the real ring-3 UDP commands via the HostOnly launcher."""

import argparse
import json
from pathlib import Path
import subprocess
import threading
import time


def children(pid, name="qemu-system-x86_64.exe"):
    query = (f"@(Get-CimInstance Win32_Process -Filter 'ParentProcessId = {pid}' | "
             f"Where-Object {{ $_.Name -eq '{name}' }} | "
             "Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress")
    data = subprocess.check_output(["powershell.exe", "-NoProfile", "-Command", query])
    result = json.loads(data)
    return [result] if isinstance(result, int) else result


def run(project, mode, test_peer_failure):
    launcher = project / "run-qemu.ps1"
    command = ["powershell.exe", "-NoProfile", "-File", str(launcher),
               "-Headless", "-Smp", "2", "-NetworkMode", mode]
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT)
    output = bytearray()
    def drain():
        while byte := process.stdout.read(1):
            output.extend(byte)
    thread = threading.Thread(target=drain, daemon=True)
    thread.start()
    qemu = None
    peer = None
    try:
        def wait(needle, start=0, seconds=100):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if needle in output[start:]:
                    return
                if process.poll() is not None:
                    break
                time.sleep(.025)
            raise AssertionError(f"Missing {needle!r}:\n" + output.decode(errors="replace"))
        prompt = b"User A [ring 3]$ "
        wait(prompt)
        owned = children(process.pid)
        assert len(owned) == 1, owned
        qemu = owned[0]
        if mode == "HostOnly":
            peers = children(process.pid, "python.exe")
            assert len(peers) == 1, peers
            peer = peers[0]
        def send(text, expected):
            start = len(output)
            for char in text.encode("ascii"):
                before = len(output)
                process.stdin.write(bytes([char]))
                process.stdin.flush()
                wait(bytes([char]), before, 5)
            process.stdin.write(b"\n")
            process.stdin.flush()
            wait(prompt, start, 20)
            result = bytes(output[start:]).decode(errors="replace")
            assert expected in result, (text, result)
        if mode == "HostOnly":
            send("net status", "network: enabled")
            send("udp 10.0.2.2 8081 hello", "reply: hello")
            send("udp 10.0.2.2 8081 second message", "reply: second message")
            send("udp 10.0.2.2 8082 hello", "denied by local firewall")
            send("udp 10.0.2.3 8081 hello", "denied by local firewall")
            send("udp invalid 8081 hello", "usage:")
            send("udp 10.0.2.2 8081 " + "x" * 96, "reply: " + "x" * 96)
            send("udp 10.0.2.2 8081 " + "x" * 97, "usage:")
            send("udp 10.0.2.2 8081 again", "reply: again")
            if test_peer_failure:
                subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                                f"Stop-Process -Id {peer} -ErrorAction Stop"], check=True)
                send("udp 10.0.2.2 8081 offline", "udp: timed out")
                send("net status", "network: enabled")
        else:
            send("net status", "network: disabled")
            send("udp 10.0.2.2 8081 hello", "network: disabled")
        print(f"PASS {mode}: ring-3 status, permitted and denied requests, repeated exchanges")
    finally:
        if qemu is None and process.poll() is None:
            owned = children(process.pid)
            if len(owned) == 1:
                qemu = owned[0]
        if qemu is not None:
            subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                            f"if (Get-Process -Id {qemu} -ErrorAction SilentlyContinue) "
                            f"{{ Stop-Process -Id {qemu} -ErrorAction Stop }}"], check=True)
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=10)
        thread.join(timeout=3)
        process.stdin.close()
        process.stdout.close()
        if peer is not None:
            subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                            f"if (Get-Process -Id {peer} -ErrorAction SilentlyContinue) "
                            "{ throw 'Launcher-owned peer is still running' }"], check=True)
        target = project / "target" / "network-user-tests"
        target.mkdir(parents=True, exist_ok=True)
        (target / (mode + ".log")).write_bytes(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--modes", nargs="+", choices=["HostOnly", "Off"], default=["HostOnly", "Off"])
    parser.add_argument("--peer-failure", action="store_true", help="terminate the owned peer and verify timeout")
    args = parser.parse_args()
    project = Path(__file__).resolve().parent.parent
    for mode in args.modes:
        run(project, mode, args.peer_failure)


if __name__ == "__main__":
    main()

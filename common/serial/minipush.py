#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>
# Python port: Ruby dependency removed.

"""Minimal serial "chainboot" pusher. Requires: pip install pyserial"""

import sys
import struct
import time

import serial  # pyserial

BAUD_RATE = 921_600
CHUNK_SIZE = 512
REQUEST_TOKEN = b"\x03\x03\x03"


def wait_for_payload_request(ser: serial.Serial) -> None:
    print("[MP] \U0001F50C Please power the target now")

    count = 0
    while True:
        data = ser.read(1)
        if not data:
            continue

        if data == b"\x03":
            count += 1
            if count == 3:
                return
        else:
            count = 0
            sys.stdout.buffer.write(data)
            sys.stdout.flush()


def send_payload(ser: serial.Serial, payload: bytes) -> None:
    size = len(payload)
    ser.write(struct.pack("<L", size))

    ack = ser.read(2)
    if ack != b"OK":
        raise RuntimeError(f"Protocol error: expected 'OK', got {ack!r}")

    sent = 0
    while sent < size:
        chunk = payload[sent : sent + CHUNK_SIZE]
        written = ser.write(chunk)
        sent += written
        pct = int((sent / size) * 100)
        print(f"\r[MP] \u23E9 Pushing {sent // 1024} KiB {pct}%", end="", flush=True)
    print()


def run_terminal(ser: serial.Serial) -> None:
    print("[MP] Entering terminal mode (Ctrl+C to exit)")
    try:
        while True:
            data = ser.read(1)
            if data:
                sys.stdout.buffer.write(data)
                sys.stdout.flush()
    except KeyboardInterrupt:
        pass


def main() -> int:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <serial_device> <payload_path>")
        return 1

    device = sys.argv[1]
    payload_path = sys.argv[2]

    with open(payload_path, "rb") as f:
        payload = f.read()

    while True:
        try:
            with serial.Serial(device, BAUD_RATE, timeout=1) as ser:
                wait_for_payload_request(ser)
                send_payload(ser, payload)
                run_terminal(ser)
            break
        except (serial.SerialException, OSError) as e:
            print(f"\n[MP] \u26A1 Connection error: {e}")
            print("[MP] Remove power and USB serial. Reinsert serial first, then power.")
            while True:
                time.sleep(1)
                try:
                    with serial.Serial(device, BAUD_RATE, timeout=0.1):
                        break
                except (serial.SerialException, OSError):
                    continue

    print("\n[MP] Bye \U0001F44B")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
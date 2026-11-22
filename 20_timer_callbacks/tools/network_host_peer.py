"""One-guest loopback Ethernet peer for the RustyPi5 UDP echo test.

This process has no WAN connection and accepts only ARP and UDP echo frames for
10.0.2.2:8081. The guest kernel, not this peer, enforces the guest firewall.
"""

import socket
import struct
import sys

GUEST_MAC = bytes.fromhex("525400123456")
PEER_MAC = bytes.fromhex("525400abcdef")
GUEST_IP = bytes((10, 0, 2, 15))
PEER_IP = bytes((10, 0, 2, 2))


def checksum(data):
    if len(data) % 2:
        data += b"\0"
    value = sum(struct.unpack("!" + "H" * (len(data) // 2), data))
    while value >> 16:
        value = (value & 65535) + (value >> 16)
    return (~value) & 65535


def exact(connection, count):
    data = bytearray()
    while len(data) < count:
        part = connection.recv(count - len(data))
        if not part:
            if not data:
                return None
            raise EOFError("truncated QEMU Ethernet frame")
        data.extend(part)
    return data


def reply(frame):
    if len(frame) < 14 or frame[6:12] != GUEST_MAC:
        return None
    if frame[12:14] == b"\x08\x06":
        if (len(frame) < 42 or frame[:6] != b"\xff" * 6
                or frame[14:22] != b"\0\x01\x08\0\x06\x04\0\x01"
                or frame[22:32] != GUEST_MAC + GUEST_IP or frame[38:42] != PEER_IP):
            return None
        result = bytearray(60)
        result[:14] = GUEST_MAC + PEER_MAC + b"\x08\x06"
        result[14:22] = b"\0\x01\x08\0\x06\x04\0\x02"
        result[22:42] = PEER_MAC + PEER_IP + GUEST_MAC + GUEST_IP
        return result
    if frame[12:14] != b"\x08\0" or frame[:6] != PEER_MAC or len(frame) < 42:
        return None
    length = struct.unpack_from("!H", frame, 16)[0]
    if (frame[14] != 0x45 or length < 29 or length > len(frame) - 14
            or frame[23] != 17 or frame[26:34] != GUEST_IP + PEER_IP
            or struct.unpack_from("!H", frame, 20)[0] & 0xBFFF
            or checksum(frame[14:34]) != 0):
        return None
    udp_length = struct.unpack_from("!H", frame, 38)[0]
    pseudo = GUEST_IP + PEER_IP + b"\0\x11" + struct.pack("!H", udp_length)
    if (udp_length != length - 20 or udp_length > 104
            or frame[34:38] != struct.pack("!HH", 49152, 8081)
            or frame[40:42] == b"\0\0" or checksum(pseudo + frame[34:14 + length]) != 0):
        return None
    result = bytearray(frame[:14 + length])
    result[:12] = GUEST_MAC + PEER_MAC
    result[26:34] = PEER_IP + GUEST_IP
    result[34:38] = struct.pack("!HH", 8081, 49152)
    result[24:26] = b"\0\0"
    struct.pack_into("!H", result, 24, checksum(result[14:34]))
    result[40:42] = b"\0\0"
    pseudo = PEER_IP + GUEST_IP + b"\0\x11" + struct.pack("!H", udp_length)
    struct.pack_into("!H", result, 40, checksum(pseudo + result[34:]) or 65535)
    return result


def main():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        print(listener.getsockname()[1], flush=True)
        connection, _ = listener.accept()
        with connection:
            while True:
                header = exact(connection, 4)
                if header is None:
                    return
                length = struct.unpack("!I", header)[0]
                if length < 14 or length > 1514:
                    raise ValueError("invalid QEMU Ethernet frame length")
                frame = exact(connection, length)
                if frame is None:
                    raise EOFError("missing QEMU Ethernet frame")
                result = reply(frame)
                if result is not None:
                    connection.sendall(struct.pack("!I", len(result)) + result)


if __name__ == "__main__":
    try:
        main()
    except (OSError, EOFError, ValueError) as error:
        print(f"Host-only network peer failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)

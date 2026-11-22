"""Create a dedicated GPT/FAT32/RustyPi5FS disk, or update only its boot files."""

import argparse
import os
from pathlib import Path
import struct
import uuid
import zlib

SECTOR = 512
BLOCK = 4096
ESP_START = 2048
ESP_SECTORS = 131072
ROOT_START = ESP_START + ESP_SECTORS
ROOT_BLOCKS = 32768
ROOT_SECTORS = ROOT_BLOCKS * 8
DISK_SECTORS = ROOT_START + ROOT_SECTORS + 2048
ROOT_TYPE = uuid.UUID("57a01418-268e-4cb0-9e97-783acd856305").bytes_le
ESP_TYPE = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").bytes_le
MAGIC = b"RPI5FS01"
DATA_START = 11


def crc(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def write_at(stream, offset, data):
    stream.seek(offset)
    stream.write(data)


def partition(kind, start, count, label):
    return struct.pack("<16s16sQQQ72s", kind, uuid.uuid4().bytes_le,
                       start, start + count - 1, 0, label.encode("utf-16le"))


def gpt_header(current, backup, entries_lba, entries, disk_id):
    header = bytearray(SECTOR)
    struct.pack_into("<8sIIIIQQQQ16sQIII", header, 0, b"EFI PART", 0x10000,
                     92, 0, 0, current, backup, 34, DISK_SECTORS - 34,
                     disk_id, entries_lba, 128, 128, crc(entries))
    struct.pack_into("<I", header, 16, crc(header[:92]))
    return header


def format_root(stream):
    entries = [
        ("", 0, 1, 0, 0o755, b""),
        ("programs", 0, 1, 0, 0o750, b""),
        ("etc", 0, 1, 0, 0o755, b""),
        ("home", 0, 1, 0, 0o755, b""),
        ("usera", 3, 1, 1000, 0o700, b""),
        ("programs", 4, 1, 1000, 0o700, b""),
        ("tmp", 0, 1, 0, 0o1777, b""),
        ("system", 0, 1, 0, 0o700, b""),
        ("README.txt", 0, 2, 0, 0o644,
         b"RustyPi5 filesystem v1. Try ls, pwd, cd /home/usera, and cat welcome.txt.\n"),
        ("os-release", 2, 2, 0, 0o644, b"NAME=RustyPi5\nFILESYSTEM=RustyPi5FS\n"),
        ("welcome.txt", 4, 2, 1000, 0o600,
         b"Welcome, User A. This is your private home directory.\n"),
        ("private.txt", 7, 2, 0, 0o600, b"Kernel-owned system storage.\n"),
    ]
    base = ROOT_START * SECTOR
    superblock = bytearray(BLOCK)
    struct.pack_into("<8sIIQIIII", superblock, 0, MAGIC, 1, BLOCK,
                     ROOT_BLOCKS, len(entries), 3, DATA_START, 0)
    struct.pack_into("<I", superblock, 40, crc(superblock[:40]))
    write_at(stream, base, superblock)
    inode_bitmap = bytearray(BLOCK)
    block_bitmap = bytearray(BLOCK)
    for i in range(DATA_START):
        block_bitmap[i // 8] |= 1 << (i % 8)
    next_block = DATA_START
    for index, (name, parent, kind, owner, mode, content) in enumerate(entries):
        inode_bitmap[index // 8] |= 1 << (index % 8)
        blocks = (len(content) + BLOCK - 1) // BLOCK
        first = next_block if blocks else 0
        record = bytearray(128)
        encoded = name.encode("ascii")
        struct.pack_into("<IIIIIIQQ", record, 0, kind, owner, owner, mode,
                         parent, len(encoded), first, len(content))
        record[40:40 + len(encoded)] = encoded
        struct.pack_into("<I", record, 124, crc(record[:124]))
        write_at(stream, base + 3 * BLOCK + index * 128, record)
        if content:
            write_at(stream, base + first * BLOCK, content)
        for block in range(next_block, next_block + blocks):
            block_bitmap[block // 8] |= 1 << (block % 8)
        next_block += blocks
    write_at(stream, base + BLOCK, inode_bitmap)
    write_at(stream, base + 2 * BLOCK, block_bitmap)


def fat_entry(name, attr, cluster, size=0):
    record = bytearray(32)
    record[:11] = name
    record[11] = attr
    struct.pack_into("<H", record, 20, cluster >> 16)
    struct.pack_into("<HI", record, 26, cluster & 0xFFFF, size)
    return record


def check_image_path(path):
    path = Path(path)
    reserved = {"CON", "PRN", "AUX", "NUL"}
    reserved.update(f"{prefix}{number}" for prefix in ("COM", "LPT") for number in range(10))
    if (path.suffix.lower() != ".img" or ":" in path.name
            or str(path).startswith(("\\\\.\\", "\\\\?\\"))
            or path.name.split(".", 1)[0].upper() in reserved
            or path.is_symlink() or (path.exists() and not path.is_file())):
        raise ValueError("Only regular .img files are supported; device paths are forbidden")
    return path


def read_boot_files(shim, kernel):
    if not shim or not kernel:
        raise ValueError("Both a UEFI shim and kernel ELF are required")
    files = [(b"BOOTX64 EFI", Path(shim).read_bytes()),
             (b"KERNEL  ELF", Path(kernel).read_bytes())]
    required = 5 + sum(max(1, (len(data) + SECTOR - 1) // SECTOR) for _, data in files)
    if required > ESP_SECTORS - 32 - 2048 + 2:
        raise ValueError("Boot files exceed the EFI partition capacity")
    return files


def update_boot(stream, shim, kernel):
    validate_disk(stream)
    write_boot(stream, read_boot_files(shim, kernel))


def write_boot(stream, files):
    reserved, fat_sectors = 32, 1024
    data_sector = reserved + 2 * fat_sectors
    fat = bytearray(fat_sectors * SECTOR)
    for index, value in [(0, 0x0FFFFFF8), (1, 0xFFFFFFFF),
                         (2, 0x0FFFFFFF), (3, 0x0FFFFFFF), (4, 0x0FFFFFFF)]:
        struct.pack_into("<I", fat, index * 4, value)
    base = ESP_START * SECTOR
    boot = bytearray(SECTOR)
    boot[:3], boot[3:11] = b"\xEB\x58\x90", b"RUSTYPI5"
    struct.pack_into("<HBHBHHBHHHII", boot, 11, SECTOR, 1, reserved, 2,
                     0, 0, 0xF8, 0, 63, 255, ESP_START, ESP_SECTORS)
    struct.pack_into("<IHHIHH", boot, 36, fat_sectors, 0, 0, 2, 1, 6)
    boot[64], boot[66] = 0x80, 0x29
    struct.pack_into("<I", boot, 67, 0x52504935)
    boot[71:82], boot[82:90], boot[510:512] = b"RUSTYPI5ESP", b"FAT32   ", b"\x55\xAA"
    info = bytearray(SECTOR)
    struct.pack_into("<I", info, 0, 0x41615252)
    struct.pack_into("<III", info, 484, 0x61417272, 0xFFFFFFFF, 0xFFFFFFFF)
    struct.pack_into("<I", info, 508, 0xAA550000)
    for sector, data in [(0, boot), (6, boot), (1, info), (7, info)]:
        write_at(stream, base + sector * SECTOR, data)
    def cluster_offset(cluster):
        return base + (data_sector + cluster - 2) * SECTOR
    root = fat_entry(b"EFI        ", 0x10, 3)
    efi = fat_entry(b".          ", 0x10, 3) + fat_entry(b"..         ", 0x10, 0)
    efi += fat_entry(b"BOOT       ", 0x10, 4)
    directory = fat_entry(b".          ", 0x10, 4) + fat_entry(b"..         ", 0x10, 3)
    cluster = 5
    for name, data in files:
        count = max(1, (len(data) + SECTOR - 1) // SECTOR)
        directory += fat_entry(name, 0x20, cluster, len(data))
        for index in range(count):
            value = cluster + index + 1 if index + 1 < count else 0x0FFFFFFF
            struct.pack_into("<I", fat, (cluster + index) * 4, value)
        write_at(stream, cluster_offset(cluster), data.ljust(count * SECTOR, b"\0"))
        cluster += count
    for number, content in [(2, root), (3, efi), (4, directory)]:
        write_at(stream, cluster_offset(number), content.ljust(SECTOR, b"\0"))
    for number in range(2):
        write_at(stream, base + (reserved + number * fat_sectors) * SECTOR, fat)


def validate_disk(stream):
    stream.seek(0, os.SEEK_END)
    if stream.tell() != DISK_SECTORS * SECTOR:
        raise ValueError("Unsupported disk size; refusing to update")
    stream.seek(SECTOR)
    header = bytearray(stream.read(SECTOR))
    expected = struct.unpack_from("<I", header, 16)[0]
    struct.pack_into("<I", header, 16, 0)
    if (header[:8] != b"EFI PART" or struct.unpack_from("<II", header, 8) != (0x10000, 92)
            or crc(header[:92]) != expected
            or struct.unpack_from("<QQQQ", header, 24) != (1, DISK_SECTORS - 1, 34, DISK_SECTORS - 34)
            or struct.unpack_from("<QII", header, 72) != (2, 128, 128)):
        raise ValueError("Invalid GPT header; refusing to update")
    stream.seek(2 * SECTOR)
    entries = stream.read(16384)
    if crc(entries) != struct.unpack_from("<I", header, 88)[0]:
        raise ValueError("Invalid GPT partition checksum")
    for offset, kind, start, count in [(0, ESP_TYPE, ESP_START, ESP_SECTORS),
                                       (128, ROOT_TYPE, ROOT_START, ROOT_SECTORS)]:
        if entries[offset:offset + 16] != kind or struct.unpack_from("<QQ", entries, offset + 32) != (start, start + count - 1):
            raise ValueError("Unexpected partition layout; refusing to update")
    stream.seek(ROOT_START * SECTOR)
    superblock = stream.read(SECTOR)
    if (superblock[:8] != MAGIC or crc(superblock[:40]) != struct.unpack_from("<I", superblock, 40)[0]
            or struct.unpack_from("<IIQ", superblock, 8) != (1, BLOCK, ROOT_BLOCKS)
            or not 1 <= struct.unpack_from("<I", superblock, 24)[0] <= 256
            or struct.unpack_from("<III", superblock, 28) != (3, DATA_START, 0)):
        raise ValueError("Invalid RustyPi5 root; refusing to update")


def create_disk(path, shim, kernel):
    path = check_image_path(path)
    files = read_boot_files(shim, kernel)
    # Exclusive creation: never truncate an existing image, even on a failed build.
    with path.open("xb") as stream:
        stream.truncate(DISK_SECTORS * SECTOR)
        mbr = bytearray(SECTOR)
        mbr[446 + 4] = 0xEE
        struct.pack_into("<II", mbr, 454, 1, DISK_SECTORS - 1)
        mbr[510:] = b"\x55\xAA"
        write_at(stream, 0, mbr)
        entries = bytearray(16384)
        entries[:128] = partition(ESP_TYPE, ESP_START, ESP_SECTORS, "RustyPi5 EFI")
        entries[128:256] = partition(ROOT_TYPE, ROOT_START, ROOT_SECTORS, "RustyPi5 root")
        disk_id = uuid.uuid4().bytes_le
        write_at(stream, 2 * SECTOR, entries)
        write_at(stream, (DISK_SECTORS - 33) * SECTOR, entries)
        write_at(stream, SECTOR, gpt_header(1, DISK_SECTORS - 1, 2, entries, disk_id))
        write_at(stream, (DISK_SECTORS - 1) * SECTOR,
                 gpt_header(DISK_SECTORS - 1, 1, DISK_SECTORS - 33, entries, disk_id))
        format_root(stream)
        write_boot(stream, files)
        stream.flush()
        os.fsync(stream.fileno())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["create", "update-boot"])
    parser.add_argument("image", type=Path)
    parser.add_argument("--shim", required=True, type=Path)
    parser.add_argument("--kernel", required=True, type=Path)
    args = parser.parse_args()
    args.image = check_image_path(args.image)
    if args.operation == "create":
        create_disk(args.image, args.shim, args.kernel)
    else:
        with args.image.open("r+b") as stream:
            update_boot(stream, args.shim, args.kernel)
            stream.flush()
            os.fsync(stream.fileno())
    print(f"{args.operation}: {args.image}")


if __name__ == "__main__":
    main()

"""Exercise script loading against a disposable copy of the prepared QEMU disk."""

import argparse
import os
from pathlib import Path
import shutil
import struct
import tempfile
import zlib

import cpu_boot_smoke
import disk_image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--qemu", type=Path, default=Path(os.environ.get("ProgramFiles", "C:\\Program Files")) / "qemu" / "qemu-system-x86_64.exe")
    args = parser.parse_args()
    project = Path(__file__).resolve().parent.parent
    original = project / "rustypi5.img"
    before = cpu_boot_smoke.digest(original)
    target = project / "target"
    target.mkdir(parents=True, exist_ok=True)
    try:
        with tempfile.TemporaryDirectory(prefix="script-fixture-", dir=target) as temporary:
            image = Path(temporary) / "fixture.img"
            shutil.copyfile(original, image)
            with image.open("r+b") as stream:
                disk_image.format_root(stream)
                base = disk_image.ROOT_START * disk_image.SECTOR
                offset = base + 3 * disk_image.BLOCK + 10 * 128
                stream.seek(offset)
                inode = bytearray(stream.read(128))
                assert inode[40:51] == b"welcome.txt"
                data_block = struct.unpack_from("<Q", inode, 24)[0]
                source = b'print("from disk");\nprint("' + b'a' * 1100 + b'");\n'
                assert len(source) <= disk_image.BLOCK
                struct.pack_into("<Q", inode, 32, len(source))
                struct.pack_into("<I", inode, 124, zlib.crc32(inode[:124]) & 0xFFFFFFFF)
                stream.seek(offset)
                stream.write(inode)
                stream.seek(base + data_block * disk_image.BLOCK)
                stream.write(source)
            cpu_boot_smoke.run(args.qemu, project, 2, image=image, script_fixture=True)
        print("PASS disposable filesystem: PATH, relative and absolute script loading; original disk unchanged")
    finally:
        assert cpu_boot_smoke.digest(original) == before, "Persistent base disk changed"


if __name__ == "__main__":
    main()

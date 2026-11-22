"""Host-side format and non-destructive boot-update regression tests."""

import hashlib
from pathlib import Path
import struct
import tempfile
import unittest

import disk_image as disk


class DiskTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.shim = self.directory / "shim.efi"
        self.kernel = self.directory / "kernel.elf"
        self.shim.write_bytes(b"MZ" + bytes(range(256)) * 3)
        self.kernel.write_bytes(b"ELF" + b"kernel payload" * 100)
        self.image = self.directory / "test.img"
        disk.create_disk(self.image, self.shim, self.kernel)

    def read_at(self, offset, length):
        with self.image.open("rb") as stream:
            stream.seek(offset)
            return stream.read(length)

    def root_hash(self):
        digest = hashlib.sha256()
        with self.image.open("rb") as stream:
            stream.seek(disk.ROOT_START * disk.SECTOR)
            for _ in range(disk.ROOT_SECTORS // 2048):
                digest.update(stream.read(2048 * disk.SECTOR))
        return digest.digest()

    def test_partition_headers_and_backup(self):
        with self.image.open("rb") as stream:
            disk.validate_disk(stream)
        self.assertEqual(self.image.stat().st_size, disk.DISK_SECTORS * disk.SECTOR)
        mbr = self.read_at(0, 512)
        self.assertEqual(mbr[510:], b"\x55\xAA")
        self.assertEqual(mbr[450], 0xEE)
        primary = self.read_at(512, 512)
        backup = bytearray(self.read_at((disk.DISK_SECTORS - 1) * 512, 512))
        expected = struct.unpack_from("<I", backup, 16)[0]
        struct.pack_into("<I", backup, 16, 0)
        self.assertEqual(disk.crc(backup[:92]), expected)
        self.assertEqual(struct.unpack_from("<QQ", backup, 24), (disk.DISK_SECTORS - 1, 1))
        entries = self.read_at(1024, 16384)
        self.assertEqual(entries, self.read_at((disk.DISK_SECTORS - 33) * 512, 16384))
        self.assertEqual(disk.crc(entries), struct.unpack_from("<I", primary, 88)[0])

    def test_fat32_geometry_and_boot_payloads(self):
        base = disk.ESP_START * 512
        boot = self.read_at(base, 512)
        self.assertEqual(boot[510:], b"\x55\xAA")
        self.assertEqual(boot[71:82], b"RUSTYPI5ESP")
        self.assertEqual(boot[82:90], b"FAT32   ")
        self.assertEqual(self.read_at(base + 6 * 512, 512), boot)
        sectors_per_fat = struct.unpack_from("<I", boot, 36)[0]
        data_start = 32 + 2 * sectors_per_fat
        clusters = disk.ESP_SECTORS - data_start
        self.assertGreaterEqual(clusters, 65525)
        self.assertGreaterEqual(sectors_per_fat * 512 // 4, clusters + 2)
        def cluster_offset(cluster):
            return base + (data_start + cluster - 2) * 512
        boot_dir = self.read_at(cluster_offset(4), 512)
        fat = self.read_at(base + 32 * 512, sectors_per_fat * 512)
        self.assertEqual(fat, self.read_at(base + (32 + sectors_per_fat) * 512, len(fat)))
        for offset, name, source in [(64, b"BOOTX64 EFI", self.shim), (96, b"KERNEL  ELF", self.kernel)]:
            entry = boot_dir[offset:offset + 32]
            self.assertEqual(entry[:11], name)
            cluster = struct.unpack_from("<H", entry, 26)[0]
            size = struct.unpack_from("<I", entry, 28)[0]
            content = bytearray()
            visited = set()
            while cluster < 0x0FFFFFF8:
                self.assertNotIn(cluster, visited)
                visited.add(cluster)
                self.assertGreaterEqual(cluster, 5)
                self.assertLess(cluster, clusters + 2)
                content.extend(self.read_at(cluster_offset(cluster), 512))
                cluster = struct.unpack_from("<I", fat, cluster * 4)[0] & 0x0FFFFFFF
            self.assertEqual(content[:size], source.read_bytes())

    def test_existing_image_is_never_overwritten(self):
        before = self.root_hash()
        with self.assertRaises(FileExistsError):
            disk.create_disk(self.image, self.shim, self.kernel)
        self.assertEqual(self.root_hash(), before)

    def test_device_and_non_image_paths_are_rejected(self):
        for path in ["NUL.img", "COM1.img", "disk.txt", "disk.img:stream", "\\\\.\\PhysicalDrive0"]:
            with self.assertRaisesRegex(ValueError, "regular .img"):
                disk.check_image_path(path)
        with self.assertRaisesRegex(ValueError, "regular .img"):
            disk.check_image_path(self.directory)

    def test_missing_payload_does_not_create_an_image(self):
        image = self.directory / "missing.img"
        with self.assertRaises(FileNotFoundError):
            disk.create_disk(image, self.directory / "missing.efi", self.kernel)
        self.assertFalse(image.exists())

    def test_boot_update_preserves_every_root_byte(self):
        with self.image.open("r+b") as stream:
            disk.write_at(stream, (disk.ROOT_START + disk.ROOT_SECTORS - 1) * 512,
                          b"persistent sentinel")
        before = self.root_hash()
        self.kernel.write_bytes(b"new kernel" * 200)
        with self.image.open("r+b") as stream:
            disk.validate_disk(stream)
            disk.update_boot(stream, self.shim, self.kernel)
        self.assertEqual(self.root_hash(), before)
        self.test_fat32_geometry_and_boot_payloads()

    def test_corrupt_primary_gpt_is_rejected(self):
        with self.image.open("r+b") as stream:
            disk.write_at(stream, 512 + 56, b"broken")
        with self.image.open("rb") as stream:
            with self.assertRaisesRegex(ValueError, "GPT"):
                disk.validate_disk(stream)

    def test_corrupt_partition_array_is_rejected(self):
        with self.image.open("r+b") as stream:
            disk.write_at(stream, 1024 + 56, b"broken")
        with self.image.open("rb") as stream:
            with self.assertRaisesRegex(ValueError, "partition checksum"):
                disk.validate_disk(stream)

    def test_corrupt_superblock_is_rejected(self):
        with self.image.open("r+b") as stream:
            disk.write_at(stream, disk.ROOT_START * 512, b"broken")
        with self.image.open("rb") as stream:
            with self.assertRaisesRegex(ValueError, "root"):
                disk.validate_disk(stream)


if __name__ == "__main__":
    unittest.main()

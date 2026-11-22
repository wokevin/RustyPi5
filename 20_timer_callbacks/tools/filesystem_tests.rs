// SPDX-License-Identifier: MIT OR Apache-2.0

#[path = "../kernel/src/filesystem.rs"]
mod filesystem;

use filesystem::{Filesystem, Identity, SectorRead};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const ROOT: u64 = 133120;
const USER: Identity = Identity {
    uid: 1000,
    gid: 1000,
};
const OTHER: Identity = Identity {
    uid: 1001,
    gid: 1001,
};

struct Disk {
    file: File,
    sectors: u64,
    overlay: BTreeMap<u64, [u8; 512]>,
    fail_at: Option<u64>,
}

impl Disk {
    fn open() -> Self {
        let file =
            File::open(std::env::var_os("RUSTYPI5_TEST_IMAGE").expect("Set RUSTYPI5_TEST_IMAGE"))
                .unwrap();
        let sectors = file.metadata().unwrap().len() / 512;
        Self {
            file,
            sectors,
            overlay: BTreeMap::new(),
            fail_at: None,
        }
    }

    fn change(&mut self, lba: u64, change: impl FnOnce(&mut [u8; 512])) {
        let mut bytes = [0; 512];
        self.read_sector(lba, &mut bytes).unwrap();
        change(&mut bytes);
        self.overlay.insert(lba, bytes);
    }

    fn inode(&mut self, index: usize, change: impl FnOnce(&mut [u8])) {
        self.change(ROOT + 24 + index as u64 / 4, |sector| {
            let offset = index % 4 * 128;
            let record = &mut sector[offset..offset + 128];
            change(record);
            let crc = crc(&record[..124]);
            record[124..].copy_from_slice(&crc.to_le_bytes());
        });
    }
}

impl SectorRead for Disk {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read_sector(&mut self, lba: u64, output: &mut [u8; 512]) -> Result<(), &'static str> {
        if lba >= self.sectors {
            return Err("Out of bounds");
        }
        if self.fail_at == Some(lba) {
            return Err("Injected disk failure");
        }
        if let Some(bytes) = self.overlay.get(&lba) {
            *output = *bytes;
            return Ok(());
        }
        self.file
            .seek(SeekFrom::Start(lba * 512))
            .map_err(|_| "Seek failed")?;
        self.file.read_exact(output).map_err(|_| "Read failed")
    }
}

fn crc(bytes: &[u8]) -> u32 {
    let mut state = !0u32;
    for &byte in bytes {
        state ^= byte as u32;
        for _ in 0..8 {
            state = (state >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(state & 1));
        }
    }
    !state
}

#[test]
fn browsing_uses_real_metadata_and_contents() {
    let mut disk = Disk::open();
    let fs = Filesystem::mount(&mut disk).unwrap();
    let mut output = Vec::new();
    fs.list(&mut disk, 0, USER, |b| output.extend_from_slice(b))
        .unwrap();
    assert_eq!(output, b"etc/\nhome/\ntmp/\nREADME.txt\n");
    let home = fs.lookup(&mut disk, 0, b"/home/usera", USER).unwrap();
    output.clear();
    fs.pwd(&mut disk, home, USER, |b| output.extend_from_slice(b))
        .unwrap();
    assert_eq!(output, b"/home/usera\n");
    let welcome = fs
        .lookup(&mut disk, home, b"./programs/../welcome.txt", USER)
        .unwrap();
    output.clear();
    fs.cat(&mut disk, welcome, USER, |b| output.extend_from_slice(b))
        .unwrap();
    assert_eq!(
        output,
        b"Welcome, User A. This is your private home directory.\n"
    );
    assert_eq!(fs.lookup(&mut disk, home, b"../../..", USER).unwrap(), 0);
}

#[test]
fn hidden_paths_cannot_be_guessed_or_traversed() {
    let mut disk = Disk::open();
    let fs = Filesystem::mount(&mut disk).unwrap();
    for path in [
        b"/system".as_slice(),
        b"/system/private.txt",
        b"/programs",
        b"/system/../etc",
        b"/missing",
    ] {
        assert_eq!(
            fs.lookup(&mut disk, 0, path, USER).err(),
            Some("Path unavailable")
        );
    }
    assert_eq!(
        fs.lookup(&mut disk, 0, b"/home/usera/welcome.txt", OTHER)
            .err(),
        Some("Path unavailable")
    );
    let home = fs.lookup(&mut disk, 0, b"/home", OTHER).unwrap();
    let mut output = Vec::new();
    fs.list(&mut disk, home, OTHER, |b| output.extend_from_slice(b))
        .unwrap();
    assert!(output.is_empty());
    assert!(fs.lookup(&mut disk, 0, b"/etc/os-release/", USER).is_err());
    assert!(fs.lookup(&mut disk, 0, &[b'a'; 128], USER).is_err());
    assert!(fs.lookup(&mut disk, 0, b"/etc/\0bad", USER).is_err());
}

#[test]
fn path_limits_are_exact_and_long_relative_cwd_is_reported() {
    for second_length in [62, 63] {
        let mut disk = Disk::open();
        disk.inode(3, |node| {
            node[20..24].copy_from_slice(&63u32.to_le_bytes());
            node[40..103].fill(b'a');
        });
        disk.inode(4, |node| {
            node[20..24].copy_from_slice(&(second_length as u32).to_le_bytes());
            node[40..40 + second_length].fill(b'b');
        });
        let fs = Filesystem::mount(&mut disk).unwrap();
        let home = fs.lookup(&mut disk, 0, &[b'a'; 63], USER).unwrap();
        let cwd = fs
            .lookup(&mut disk, home, &vec![b'b'; second_length], USER)
            .unwrap();
        let mut expected = vec![b'/'];
        expected.extend_from_slice(&[b'a'; 63]);
        expected.push(b'/');
        expected.extend_from_slice(&vec![b'b'; second_length]);
        let mut output = Vec::new();
        let result = fs.pwd(&mut disk, cwd, USER, |b| output.extend_from_slice(b));
        if second_length == 62 {
            assert_eq!(expected.len(), 127);
            assert_eq!(fs.lookup(&mut disk, 0, &expected, USER).unwrap(), cwd);
            result.unwrap();
            expected.push(b'\n');
            assert_eq!(output, expected);
        } else {
            assert_eq!(expected.len(), 128);
            assert_eq!(
                fs.lookup(&mut disk, 0, &expected, USER).err(),
                Some("Invalid path (maximum 127 ASCII bytes)")
            );
            assert_eq!(result.err(), Some("Path exceeds 127 bytes"));
            assert!(output.is_empty());
        }
    }
}

#[test]
fn cat_streams_multiple_sectors_and_stops_at_exact_file_size() {
    let mut disk = Disk::open();
    disk.inode(8, |node| {
        node[32..40].copy_from_slice(&1025u64.to_le_bytes())
    });
    for (offset, value) in [(0, b'a'), (1, b'b'), (2, b'c')] {
        disk.change(ROOT + 11 * 8 + offset, |sector| sector.fill(value));
    }
    let fs = Filesystem::mount(&mut disk).unwrap();
    let inode = fs.lookup(&mut disk, 0, b"/README.txt", USER).unwrap();
    let mut output = Vec::new();
    fs.cat(&mut disk, inode, USER, |b| output.extend_from_slice(b))
        .unwrap();
    let mut expected = vec![b'a'; 512];
    expected.extend_from_slice(&[b'b'; 512]);
    expected.push(b'c');
    assert_eq!(output, expected);
}

#[test]
fn raw_file_read_enforces_capacity_permissions_and_disk_errors() {
    let mut disk = Disk::open();
    let fs = Filesystem::mount(&mut disk).unwrap();
    let inode = fs.lookup(&mut disk, 0, b"/home/usera/welcome.txt", USER).unwrap();
    let mut output = [0u8; 128];
    let length = fs.read_file(&mut disk, inode, USER, &mut output).unwrap();
    assert_eq!(&output[..length], b"Welcome, User A. This is your private home directory.\n");
    assert_eq!(fs.read_file(&mut disk, inode, OTHER, &mut output), Err("Path unavailable"));
    assert_eq!(fs.read_file(&mut disk, inode, USER, &mut output[..length - 1]), Err("File exceeds buffer capacity"));
    let root = fs.lookup(&mut disk, 0, b"/", USER).unwrap();
    assert_eq!(fs.read_file(&mut disk, root, USER, &mut output), Err("Path unavailable"));
    disk.fail_at = Some(ROOT + 24 + inode as u64 / 4);
    assert_eq!(fs.read_file(&mut disk, inode, USER, &mut output), Err("Injected disk failure"));
}

#[test]
fn owner_and_group_bits_do_not_fall_back_to_world_bits() {
    let mut disk = Disk::open();
    disk.inode(4, |node| {
        node[12..16].copy_from_slice(&0o007u32.to_le_bytes())
    });
    let fs = Filesystem::mount(&mut disk).unwrap();
    assert!(fs.lookup(&mut disk, 0, b"/home/usera", USER).is_err());
    assert!(fs
        .lookup(
            &mut disk,
            0,
            b"/home/usera",
            Identity {
                uid: 1001,
                gid: 1000
            }
        )
        .is_err());
    assert!(fs.lookup(&mut disk, 0, b"/home/usera", OTHER).is_ok());
}

#[test]
fn corrupt_headers_bitmaps_and_inode_checksums_are_rejected() {
    for lba in [1, 2, ROOT, ROOT + 8, ROOT + 16, ROOT + 24] {
        let mut disk = Disk::open();
        disk.change(lba, |sector| sector[0] ^= 1);
        assert!(
            Filesystem::mount(&mut disk).is_err(),
            "sector {} accepted",
            lba
        );
    }
}

#[test]
fn checksummed_but_invalid_extents_names_and_parents_are_rejected() {
    let changes: [(usize, usize, u64); 6] = [
        (8, 24, 32768),
        (8, 32, u64::MAX),
        (4, 16, 4),
        (4, 20, 64),
        (8, 24, 10),
        (9, 24, 11),
    ];
    for (inode, offset, value) in changes {
        let mut disk = Disk::open();
        disk.inode(inode, |node| {
            let length = if offset == 24 || offset == 32 { 8 } else { 4 };
            node[offset..offset + length].copy_from_slice(&value.to_le_bytes()[..length]);
        });
        assert!(
            Filesystem::mount(&mut disk).is_err(),
            "inode {} offset {} accepted",
            inode,
            offset
        );
    }
}

#[test]
fn duplicate_names_and_unknown_inode_types_are_rejected() {
    let mut disk = Disk::open();
    disk.inode(2, |node| {
        node[20..24].copy_from_slice(&8u32.to_le_bytes());
        node[40..48].copy_from_slice(b"programs");
    });
    assert!(Filesystem::mount(&mut disk).is_err());
    let mut disk = Disk::open();
    disk.inode(8, |node| node[..4].copy_from_slice(&3u32.to_le_bytes()));
    assert!(Filesystem::mount(&mut disk).is_err());
}

#[test]
fn unsupported_superblock_version_and_oversized_count_are_rejected() {
    for (offset, value) in [(8, 2u32), (24, 257u32)] {
        let mut disk = Disk::open();
        disk.change(ROOT, |sector| {
            sector[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let checksum = crc(&sector[..40]);
            sector[40..44].copy_from_slice(&checksum.to_le_bytes());
        });
        assert!(Filesystem::mount(&mut disk).is_err());
    }
}

#[test]
fn disk_failures_are_reported_not_treated_as_empty_files() {
    let mut disk = Disk::open();
    let fs = Filesystem::mount(&mut disk).unwrap();
    let inode = fs.lookup(&mut disk, 0, b"/README.txt", USER).unwrap();
    disk.fail_at = Some(ROOT + 11 * 8);
    assert_eq!(
        fs.cat(&mut disk, inode, USER, |_| {}).err(),
        Some("Injected disk failure")
    );
    disk.fail_at = Some(1);
    assert_eq!(
        Filesystem::mount(&mut disk).err(),
        Some("Injected disk failure")
    );
}

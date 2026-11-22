// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded, read-only RustyPi5FS v1 reader. No allocator or firmware services are used.

#[cfg(feature = "bsp_qemu_x86")]
mod ata;
#[cfg(feature = "bsp_qemu_x86")]
pub mod shell;

pub trait SectorRead {
    fn sectors(&self) -> u64;
    fn read_sector(&mut self, lba: u64, output: &mut [u8; 512]) -> Result<(), &'static str>;
}

const ROOT_TYPE: [u8; 16] = [
    0x18, 0x14, 0xa0, 0x57, 0x8e, 0x26, 0xb0, 0x4c, 0x9e, 0x97, 0x78, 0x3a, 0xcd, 0x85, 0x63, 0x05,
];
const UNAVAILABLE: &str = "Path unavailable";

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u32_at(bytes, offset) as u64 | ((u32_at(bytes, offset + 4) as u64) << 32)
}

fn crc_update(mut state: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        state ^= byte as u32;
        for _ in 0..8 {
            state = (state >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(state & 1)));
        }
    }
    state
}

#[derive(Clone, Copy)]
pub struct Identity {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Copy)]
pub struct Node {
    pub directory: bool,
    pub parent: u32,
    name: [u8; 64],
    name_length: usize,
    uid: u32,
    gid: u32,
    mode: u32,
    first: u64,
    size: u64,
}

impl Node {
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_length]
    }

    fn accessible(&self, user: Identity) -> bool {
        let shift = if self.uid == user.uid {
            6
        } else if self.gid == user.gid {
            3
        } else {
            0
        };
        let required = if self.directory { 5 } else { 4 };
        ((self.mode >> shift) & required) == required
    }
}

pub struct Filesystem {
    start: u64,
    blocks: u64,
    count: u32,
}

impl Filesystem {
    pub fn mount(disk: &mut impl SectorRead) -> Result<Self, &'static str> {
        let mut sector = [0u8; 512];
        disk.read_sector(1, &mut sector)?;
        if &sector[..8] != b"EFI PART"
            || u32_at(&sector, 8) != 0x10000
            || u32_at(&sector, 12) != 92
            || u64_at(&sector, 24) != 1
            || u64_at(&sector, 32) != disk.sectors().checked_sub(1).ok_or("Disk too small")?
            || u32_at(&sector, 80) != 128
            || u32_at(&sector, 84) != 128
        {
            return Err("Unsupported GPT layout");
        }
        let expected = u32_at(&sector, 16);
        sector[16..20].fill(0);
        if !crc_update(!0, &sector[..92]) != expected {
            return Err("GPT header checksum mismatch");
        }
        let table = u64_at(&sector, 72);
        let first_usable = u64_at(&sector, 40);
        let last_usable = u64_at(&sector, 48);
        if table < 2
            || table.checked_add(32).map_or(true, |end| end > first_usable)
            || last_usable >= disk.sectors()
            || first_usable > last_usable
        {
            return Err("Invalid GPT bounds");
        }
        let table_crc = u32_at(&sector, 88);
        let mut state = !0;
        let mut partition = None;
        for index in 0..32 {
            disk.read_sector(table + index, &mut sector)?;
            state = crc_update(state, &sector);
            for entry in sector.chunks_exact(128) {
                if entry[..16] == ROOT_TYPE {
                    if partition.is_some() {
                        return Err("Multiple RustyPi5 root partitions");
                    }
                    let start = u64_at(entry, 32);
                    let end = u64_at(entry, 40);
                    if start < first_usable || start > end || end > last_usable || start % 8 != 0 {
                        return Err("Invalid root partition bounds");
                    }
                    partition = Some((start, end));
                }
            }
        }
        if !state != table_crc {
            return Err("GPT partition checksum mismatch");
        }
        let (start, end) = partition.ok_or("RustyPi5 root partition not found")?;
        disk.read_sector(start, &mut sector)?;
        if &sector[..8] != b"RPI5FS01"
            || u32_at(&sector, 8) != 1
            || u32_at(&sector, 12) != 4096
            || u32_at(&sector, 28) != 3
            || u32_at(&sector, 32) != 11
            || u32_at(&sector, 36) != 0
            || !crc_update(!0, &sector[..40]) != u32_at(&sector, 40)
        {
            return Err("Unsupported or corrupt RustyPi5FS superblock");
        }
        let fs = Self {
            start,
            blocks: u64_at(&sector, 16),
            count: u32_at(&sector, 24),
        };
        if fs.blocks < 11
            || fs.blocks > 32768
            || fs.blocks * 8 > end - start + 1
            || fs.count == 0
            || fs.count > 256
        {
            return Err("Invalid filesystem bounds");
        }
        for block in 0..11 {
            fs.allocated(disk, 2, block)?;
        }
        let mut previous_end = 11;
        for index in 0..fs.count {
            fs.allocated(disk, 1, index as u64)?;
            let node = fs.node(disk, index)?;
            if index == 0 {
                if !node.directory || node.parent != 0 || node.name_length != 0 {
                    return Err("Invalid root inode");
                }
            } else {
                if node.parent >= index
                    || !fs.node(disk, node.parent)?.directory
                    || node.name_length == 0
                    || node.name() == b"."
                    || node.name() == b".."
                    || !node
                        .name()
                        .iter()
                        .all(|byte| (b'!'..=b'~').contains(byte) && *byte != b'/')
                {
                    return Err("Invalid directory entry");
                }
                for prior in 1..index {
                    let other = fs.node(disk, prior)?;
                    if node.parent == other.parent && node.name() == other.name() {
                        return Err("Duplicate directory entry");
                    }
                }
            }
            if node.directory || node.size == 0 {
                if node.first != 0 || node.size != 0 {
                    return Err("Invalid empty inode extent");
                }
            } else {
                let blocks = node.size.checked_add(4095).ok_or("File size overflow")? / 4096;
                let end = node
                    .first
                    .checked_add(blocks)
                    .ok_or("File extent overflow")?;
                if node.first < previous_end || end > fs.blocks {
                    return Err("Overlapping or out-of-range file extent");
                }
                for block in node.first..end {
                    fs.allocated(disk, 2, block)?;
                }
                previous_end = end;
            }
        }
        Ok(fs)
    }

    fn allocated(
        &self,
        disk: &mut impl SectorRead,
        bitmap: u64,
        bit: u64,
    ) -> Result<(), &'static str> {
        let mut sector = [0u8; 512];
        disk.read_sector(self.start + bitmap * 8 + bit / 4096, &mut sector)?;
        if sector[(bit % 4096 / 8) as usize] & (1 << (bit % 8)) == 0 {
            return Err("Referenced inode or block is unallocated");
        }
        Ok(())
    }

    pub fn node(&self, disk: &mut impl SectorRead, index: u32) -> Result<Node, &'static str> {
        if index >= self.count {
            return Err("Invalid inode number");
        }
        let mut sector = [0u8; 512];
        disk.read_sector(self.start + 24 + index as u64 / 4, &mut sector)?;
        let offset = index as usize % 4 * 128;
        let record = &sector[offset..offset + 128];
        let kind = u32_at(record, 0);
        let length = u32_at(record, 20) as usize;
        if !crc_update(!0, &record[..124]) != u32_at(record, 124)
            || !(kind == 1 || kind == 2)
            || length > 63
            || u32_at(record, 12) & !0o7777 != 0
        {
            return Err("Corrupt or unsupported inode");
        }
        let mut name = [0; 64];
        name.copy_from_slice(&record[40..104]);
        Ok(Node {
            directory: kind == 1,
            uid: u32_at(record, 4),
            gid: u32_at(record, 8),
            mode: u32_at(record, 12),
            parent: u32_at(record, 16),
            name,
            name_length: length,
            first: u64_at(record, 24),
            size: u64_at(record, 32),
        })
    }

    pub fn lookup(
        &self,
        disk: &mut impl SectorRead,
        cwd: u32,
        path: &[u8],
        user: Identity,
    ) -> Result<u32, &'static str> {
        if path.len() > 127 || !path.iter().all(|byte| (b' '..=b'~').contains(byte)) {
            return Err("Invalid path (maximum 127 ASCII bytes)");
        }
        let mut current = if path.first() == Some(&b'/') { 0 } else { cwd };
        for component in path
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
        {
            let directory = self.node(disk, current)?;
            if !directory.directory || !directory.accessible(user) {
                return Err(UNAVAILABLE);
            }
            if component == b"." {
                continue;
            }
            if component == b".." {
                current = directory.parent;
                continue;
            }
            let mut found = None;
            for index in 1..self.count {
                let node = self.node(disk, index)?;
                if node.parent == current && node.name() == component && node.accessible(user) {
                    found = Some(index);
                    break;
                }
            }
            current = found.ok_or(UNAVAILABLE)?;
        }
        let node = self.node(disk, current)?;
        if !node.accessible(user) || (path.last() == Some(&b'/') && !node.directory) {
            return Err(UNAVAILABLE);
        }
        Ok(current)
    }

    pub fn list(
        &self,
        disk: &mut impl SectorRead,
        directory: u32,
        user: Identity,
        mut emit: impl FnMut(&[u8]),
    ) -> Result<(), &'static str> {
        let parent = self.node(disk, directory)?;
        if !parent.directory || !parent.accessible(user) {
            return Err(UNAVAILABLE);
        }
        for index in 1..self.count {
            let node = self.node(disk, index)?;
            if node.parent == directory && node.accessible(user) {
                emit(node.name());
                emit(if node.directory { b"/\n" } else { b"\n" });
            }
        }
        Ok(())
    }

    pub fn read_file(
        &self,
        disk: &mut impl SectorRead,
        inode: u32,
        user: Identity,
        output: &mut [u8],
    ) -> Result<usize, &'static str> {
        let node = self.node(disk, inode)?;
        if node.directory || !node.accessible(user) {
            return Err(UNAVAILABLE);
        }
        if node.size > output.len() as u64 {
            return Err("File exceeds buffer capacity");
        }
        let mut sector = [0u8; 512];
        let mut used = 0;
        while used < node.size as usize {
            disk.read_sector(self.start + node.first * 8 + (used / 512) as u64, &mut sector)?;
            let length = core::cmp::min(512, node.size as usize - used);
            output[used..used + length].copy_from_slice(&sector[..length]);
            used += length;
        }
        Ok(used)
    }

    pub fn cat(
        &self,
        disk: &mut impl SectorRead,
        inode: u32,
        user: Identity,
        mut emit: impl FnMut(&[u8]),
    ) -> Result<(), &'static str> {
        let node = self.node(disk, inode)?;
        if node.directory || !node.accessible(user) {
            return Err(UNAVAILABLE);
        }
        let mut sector = [0u8; 512];
        let mut remaining = node.size;
        let mut lba = self.start + node.first * 8;
        while remaining != 0 {
            disk.read_sector(lba, &mut sector)?;
            let length = core::cmp::min(remaining, 512) as usize;
            emit(&sector[..length]);
            remaining -= length as u64;
            lba += 1;
        }
        Ok(())
    }

    pub fn pwd(
        &self,
        disk: &mut impl SectorRead,
        cwd: u32,
        user: Identity,
        mut emit: impl FnMut(&[u8]),
    ) -> Result<(), &'static str> {
        let mut path = [0u8; 128];
        let mut used = path.len();
        let mut current = cwd;
        while current != 0 {
            let node = self.node(disk, current)?;
            if !node.directory || !node.accessible(user) {
                return Err(UNAVAILABLE);
            }
            used = used
                .checked_sub(node.name_length + 1)
                .filter(|offset| *offset != 0)
                .ok_or("Path exceeds 127 bytes")?;
            path[used] = b'/';
            path[used + 1..used + 1 + node.name_length].copy_from_slice(node.name());
            current = node.parent;
        }
        if used == path.len() {
            used -= 1;
            path[used] = b'/';
        }
        emit(&path[used..]);
        emit(b"\n");
        Ok(())
    }
}

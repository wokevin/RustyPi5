// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded xAPIC topology and dedicated-kernel-CPU placement policy.

pub struct Topology<const N: usize> {
    ids: [u8; N],
    count: usize,
    boot: u8,
    boot_seen: bool,
}

impl<const N: usize> Topology<N> {
    pub fn new(boot: u8) -> Self {
        Self {
            ids: [0; N],
            count: 0,
            boot,
            boot_seen: false,
        }
    }

    pub fn add(&mut self, id: u8, enabled: bool) -> Result<(), &'static str> {
        if !enabled {
            return Ok(());
        }
        if self.ids[..self.count].contains(&id) {
            return Err("Duplicate enabled APIC ID");
        }
        if self.count == N {
            return Err("CPU topology exceeds supported capacity");
        }
        self.ids[self.count] = id;
        self.count += 1;
        if id == self.boot {
            self.boot_seen = true;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Self, &'static str> {
        if !self.boot_seen {
            return Err("Boot CPU absent from enabled firmware topology");
        }
        let index = self.ids[..self.count]
            .iter()
            .position(|id| *id == self.boot)
            .ok_or("Boot CPU topology mismatch")?;
        self.ids.swap(0, index);
        Ok(self)
    }

    pub fn ids(&self) -> &[u8] {
        &self.ids[..self.count]
    }
}

pub fn user_eligible(logical_id: usize, online: bool) -> bool {
    logical_id != 0 && online
}

pub fn select_user_cpu(online: impl Iterator<Item = (usize, bool)>) -> Result<usize, &'static str> {
    online.filter(|(id, ready)| user_eligible(*id, *ready)).map(|(id, _)| id).next()
        .ok_or("Dedicated kernel CPU requires at least one online secondary CPU; use -Smp 2 or greater")
}

pub fn parse_madt(
    bytes: &[u8],
    mut emit: impl FnMut(u8, bool) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if bytes.len() < 44 || &bytes[..4] != b"APIC" {
        return Err("Invalid MADT header");
    }
    let mut offset = 44;
    while offset < bytes.len() {
        if bytes.len() - offset < 2 {
            return Err("Truncated MADT entry header");
        }
        let kind = bytes[offset];
        let length = bytes[offset + 1] as usize;
        if length < 2 || length > bytes.len() - offset {
            return Err("Invalid MADT entry length");
        }
        let entry = &bytes[offset..offset + length];
        match kind {
            0 => {
                if length != 8 {
                    return Err("Invalid Local APIC entry length");
                }
                let flags = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
                emit(entry[3], flags & 1 != 0)?;
            }
            9 => {
                if length < 16 {
                    return Err("Invalid x2APIC entry length");
                }
                let flags = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
                if flags & 1 != 0 {
                    return Err("Enabled x2APIC topology is not supported yet");
                }
            }
            _ => {}
        }
        offset += length;
    }
    Ok(())
}

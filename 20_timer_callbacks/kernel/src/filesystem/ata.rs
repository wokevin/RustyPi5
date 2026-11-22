// SPDX-License-Identifier: MIT OR Apache-2.0

//! Primary-master legacy IDE PIO, matching the QEMU PC launcher. Read commands only.

use super::SectorRead;
use core::arch::asm;

pub struct Ata {
    sectors: u64,
}

unsafe fn out(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

unsafe fn input(port: u16) -> u8 {
    let value;
    asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

fn delay() {
    for _ in 0..4 {
        unsafe {
            input(0x3f6);
        }
    }
}

fn wait(data: bool) -> Result<(), &'static str> {
    for _ in 0..1_000_000 {
        let status = unsafe { input(0x1f7) };
        if status == 0 || status == 0xff {
            return Err("IDE primary-master disk is absent");
        }
        if status & 0x80 == 0 {
            if status & 0x21 != 0 {
                return Err("IDE command failed");
            }
            if !data || status & 8 != 0 {
                return Ok(());
            }
        }
        core::hint::spin_loop();
    }
    Err("IDE command timed out")
}

fn words(output: &mut [u8; 512]) {
    for pair in output.chunks_exact_mut(2) {
        let word: u16;
        unsafe {
            asm!("in ax, dx", in("dx") 0x1f0u16, out("ax") word, options(nomem, nostack, preserves_flags));
        }
        pair.copy_from_slice(&word.to_le_bytes());
    }
    delay();
}

impl Ata {
    pub fn open() -> Result<Self, &'static str> {
        unsafe {
            out(0x3f6, 2); // Polling only: disable IDE interrupts.
            out(0x1f6, 0xa0);
        }
        delay();
        wait(false)?;
        unsafe {
            out(0x1f2, 0);
            out(0x1f3, 0);
            out(0x1f4, 0);
            out(0x1f5, 0);
            out(0x1f7, 0xec);
        }
        delay();
        wait(true)?;
        if unsafe { input(0x1f4) } != 0 || unsafe { input(0x1f5) } != 0 {
            return Err("IDE primary master is not an ATA disk");
        }
        let mut identify = [0; 512];
        words(&mut identify);
        if identify[99] & 2 == 0 {
            return Err("ATA disk does not support LBA");
        }
        let sectors =
            u32::from_le_bytes([identify[120], identify[121], identify[122], identify[123]]) as u64;
        if sectors == 0 || sectors > 0x1000_0000 {
            return Err("Unsupported ATA LBA28 capacity");
        }
        Ok(Self { sectors })
    }
}

impl SectorRead for Ata {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read_sector(&mut self, lba: u64, output: &mut [u8; 512]) -> Result<(), &'static str> {
        if lba >= self.sectors {
            return Err("IDE read outside disk");
        }
        wait(false)?;
        unsafe {
            out(0x1f6, 0xe0 | ((lba >> 24) as u8 & 15));
        }
        delay();
        wait(false)?;
        unsafe {
            out(0x1f2, 1);
            out(0x1f3, lba as u8);
            out(0x1f4, (lba >> 8) as u8);
            out(0x1f5, (lba >> 16) as u8);
            out(0x1f7, 0x20);
        }
        delay();
        wait(true)?;
        words(output);
        wait(false)
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0

//! QEMU 82540EM (8086:100e), legacy descriptors, bounded polling, kernel-only DMA.

use core::{arch::asm, marker::PhantomData, ptr, sync::atomic::{fence, AtomicBool, Ordering}};
use super::stack::MAX_FRAME;

const COUNT: usize = 16;
const BUFFER: usize = 2048;
static INITIALIZED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy)]
struct Rx { address: u64, length: u16, checksum: u16, status: u8, errors: u8, special: u16 }
#[repr(C)]
#[derive(Clone, Copy)]
struct Tx { address: u64, length: u16, cso: u8, command: u8, status: u8, css: u8, special: u16 }
#[repr(C, align(4096))]
struct Dma {
    rx: [Rx; COUNT],
    tx: [Tx; COUNT],
    receive: [[u8; BUFFER]; COUNT],
    transmit: [[u8; BUFFER]; COUNT],
}
static mut DMA: Dma = Dma {
    rx: [Rx { address: 0, length: 0, checksum: 0, status: 0, errors: 0, special: 0 }; COUNT],
    tx: [Tx { address: 0, length: 0, cso: 0, command: 0, status: 1, css: 0, special: 0 }; COUNT],
    receive: [[0; BUFFER]; COUNT], transmit: [[0; BUFFER]; COUNT],
};

pub struct Device {
    base: usize,
    rx: usize,
    tx: usize,
    discard_chain: bool,
    pub mac: [u8; 6],
    pub bad_frames: u64,
    owner: PhantomData<*mut ()>,
}

unsafe fn pci_read(offset: u8) -> u32 {
    // The launcher fixes the single NIC at bus 0, slot 3, function 0.
    asm!("out dx, eax", in("dx") 0xCF8u16, in("eax") 0x8000_1800u32 | offset as u32,
        options(nomem, nostack, preserves_flags));
    let value;
    asm!("in eax, dx", in("dx") 0xCFCu16, out("eax") value,
        options(nomem, nostack, preserves_flags));
    value
}
unsafe fn pci_write(offset: u8, value: u32) {
    asm!("out dx, eax", in("dx") 0xCF8u16, in("eax") 0x8000_1800u32 | offset as u32,
        options(nomem, nostack, preserves_flags));
    asm!("out dx, eax", in("dx") 0xCFCu16, in("eax") value,
        options(nomem, nostack, preserves_flags));
}
unsafe fn pci_command(value: u32) { pci_write(4, value & 0xFFFF); }

/// Requires exclusive PCI config access on CPU 0 with IRQs masked.
pub unsafe fn present() -> bool { pci_read(0) != u32::MAX }

impl Device {
    /// Reserve the single NIC and its DMA storage. A failed initialization cannot be retried.
    ///
    /// # Safety
    /// Call once on CPU 0 with hardware IRQs masked, before user CPUs start. PCI config
    /// access must be exclusive. `map_mmio` must validate the BAR against RAM/user ranges
    /// and install supervisor-only, uncached identity mappings, retained for this device's
    /// lifetime. All kernel static DMA memory must be identity-mapped, supervisor-only,
    /// resident and cache-coherent. The PCI function must be QEMU's trusted 82540EM.
    pub unsafe fn init(map_mmio: fn(usize, usize) -> Result<(), &'static str>) -> Result<Self, &'static str> {
        if INITIALIZED.swap(true, Ordering::AcqRel) {
            return Err("E1000 initialization already attempted");
        }
        unsafe {
            let id = pci_read(0);
            if id != 0x100E_8086 { return Err("PCI 00:03.0 is missing or not the supported E1000"); }
            let bar = pci_read(0x10);
            let base = (bar & !0xF) as usize;
            if bar & 0xF != 0 || base == 0 || base % 0x20000 != 0 {
                return Err("E1000 needs an assigned, aligned 32-bit memory BAR");
            }
            let command = pci_read(4) & 0xFFFF;
            // Decode and bus mastering must be off while probing the BAR; restore its address
            // before validation, leaving DMA disabled on every initialization failure.
            pci_command((command | (1 << 10)) & !6);
            pci_write(0x10, u32::MAX);
            let mask = pci_read(0x10) & !0xF;
            pci_write(0x10, bar);
            if (!mask).wrapping_add(1) != 0x20000 {
                return Err("Unexpected E1000 BAR size");
            }
            map_mmio(base, 0x20000)?;
            pci_command((command | 2 | (1 << 10)) & !4);
            let mut device = Self { base, rx: 0, tx: 0, discard_chain: false, mac: [0; 6],
                bad_frames: 0, owner: PhantomData };
            device.write(0xD8, u32::MAX);
            device.write(0x100, 0);
            device.write(0x400, 0);
            device.write(0, device.read(0) | (1 << 26));
            let mut reset = false;
            for _ in 0..1_000_000 {
                if device.read(0) & (1 << 26) == 0 { reset = true; break; }
                core::hint::spin_loop();
            }
            if !reset { return Err("E1000 reset timed out; bus mastering remains disabled"); }
            device.write(0xD8, u32::MAX);
            let low = device.read(0x5400);
            let high = device.read(0x5404);
            device.mac[..4].copy_from_slice(&low.to_le_bytes());
            device.mac[4..].copy_from_slice(&(high as u16).to_le_bytes());
            if high & (1 << 31) == 0 || device.mac == [0; 6] || device.mac[0] & 1 != 0 {
                return Err("E1000 has no valid unicast MAC address");
            }
            let dma = ptr::addr_of_mut!(DMA);
            for i in 0..COUNT {
                (*dma).rx[i].address = ptr::addr_of!((*dma).receive[i]) as u64;
                (*dma).tx[i].address = ptr::addr_of!((*dma).transmit[i]) as u64;
            }
            for offset in (0x5200..0x5400).step_by(4) { device.write(offset, 0); }
            let rx = ptr::addr_of!((*dma).rx) as u64;
            let tx = ptr::addr_of!((*dma).tx) as u64;
            device.write(0x2800, rx as u32); device.write(0x2804, (rx >> 32) as u32);
            device.write(0x2808, (COUNT * 16) as u32); device.write(0x2810, 0);
            device.write(0x2818, (COUNT - 1) as u32);
            device.write(0x3800, tx as u32); device.write(0x3804, (tx >> 32) as u32);
            device.write(0x3808, (COUNT * 16) as u32); device.write(0x3810, 0); device.write(0x3818, 0);
            device.write(0x410, 10 | (8 << 10) | (6 << 20));
            fence(Ordering::SeqCst);
            pci_command(command | 6 | (1 << 10));
            device.write(0, device.read(0) | (1 << 6));
            device.write(0x400, (1 << 1) | (1 << 3) | (0x10 << 4) | (0x40 << 12));
            device.write(0x100, (1 << 1) | (1 << 15) | (1 << 26));
            Ok(device)
        }
    }

    pub fn link_up(&self) -> bool { unsafe { self.read(8) & 2 != 0 } }

    pub fn send(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.len() < 14 || frame.len() > MAX_FRAME { return Err("Invalid Ethernet TX length"); }
        unsafe {
            let dma = ptr::addr_of_mut!(DMA);
            let descriptor = ptr::addr_of_mut!((*dma).tx[self.tx]);
            if ptr::read_volatile(ptr::addr_of!((*descriptor).status)) & 1 == 0 {
                return Err("E1000 transmit ring busy");
            }
            fence(Ordering::Acquire);
            ptr::copy_nonoverlapping(frame.as_ptr(), ptr::addr_of_mut!((*dma).transmit[self.tx]) as *mut u8, frame.len());
            ptr::write_volatile(descriptor, Tx {
                address: ptr::addr_of!((*dma).transmit[self.tx]) as u64,
                length: frame.len() as u16, cso: 0, command: 0x0B, status: 0, css: 0, special: 0,
            });
            fence(Ordering::SeqCst);
            self.tx = (self.tx + 1) % COUNT;
            self.write(0x3818, self.tx as u32);
        }
        Ok(())
    }

    pub fn receive(&mut self, out: &mut [u8; MAX_FRAME]) -> Option<usize> {
        unsafe {
            let dma = ptr::addr_of_mut!(DMA);
            // Bound each poll, including malformed/non-EOP descriptor chains.
            for _ in 0..COUNT {
                let descriptor = ptr::addr_of_mut!((*dma).rx[self.rx]);
                let status = ptr::read_volatile(ptr::addr_of!((*descriptor).status));
                if status & 1 == 0 { return None; }
                fence(Ordering::Acquire);
                let length = ptr::read_volatile(ptr::addr_of!((*descriptor).length)) as usize;
                let errors = ptr::read_volatile(ptr::addr_of!((*descriptor).errors));
                let valid = !self.discard_chain && status & 2 != 0 && errors == 0
                    && length >= 14 && length <= MAX_FRAME;
                self.discard_chain = status & 2 == 0;
                if valid {
                    ptr::copy_nonoverlapping(ptr::addr_of!((*dma).receive[self.rx]) as *const u8, out.as_mut_ptr(), length);
                } else { self.bad_frames = self.bad_frames.saturating_add(1); }
                ptr::write_volatile(ptr::addr_of_mut!((*descriptor).status), 0);
                fence(Ordering::SeqCst);
                self.write(0x2818, self.rx as u32);
                self.rx = (self.rx + 1) % COUNT;
                if valid { return Some(length); }
            }
        }
        None
    }

    unsafe fn read(&self, offset: usize) -> u32 { ptr::read_volatile((self.base + offset) as *const u32) }
    unsafe fn write(&self, offset: usize, value: u32) {
        ptr::write_volatile((self.base + offset) as *mut u32, value);
        // Flush posted MMIO writes before dependent DMA operations.
        let _ = self.read(8);
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Retain the reserved static buffers even after shutdown; never reuse live DMA memory.
        unsafe {
            self.write(0xD8, u32::MAX);
            self.write(0x100, 0);
            self.write(0x400, 0);
            pci_command((pci_read(4) & 0xFFFF & !4) | (1 << 10));
            fence(Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_layout_matches_82540em() {
        assert_eq!(core::mem::size_of::<Rx>(), 16);
        assert_eq!(core::mem::size_of::<Tx>(), 16);
        assert_eq!(core::mem::align_of::<Dma>(), 4096);
        assert_eq!(COUNT * core::mem::size_of::<Rx>() % 128, 0);
        assert_eq!(COUNT * core::mem::size_of::<Tx>() % 128, 0);
        let dma = unsafe { ptr::addr_of!(DMA) as usize };
        let rx = unsafe { ptr::addr_of!((*ptr::addr_of!(DMA)).rx) as usize };
        let tx = unsafe { ptr::addr_of!((*ptr::addr_of!(DMA)).tx) as usize };
        assert_eq!(rx % 16, 0);
        assert_eq!(tx % 16, 0);
        assert_eq!(rx - dma, 0);
        assert_eq!(tx - dma, COUNT * 16);
        assert!(BUFFER >= MAX_FRAME);
    }
}

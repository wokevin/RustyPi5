// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Hyper-V x86_64 guest console.
//!
//! This was originally a polling driver for the legacy 16550-compatible UART on COM1
//! (I/O port `0x3F8`), using raw `out dx, al` / `in al, dx` instructions. That does **not** work
//! on this Gen 2 Hyper-V configuration: Hyper-V's hypervisor intercepts those I/O port
//! instructions and faults immediately with "the guest executed an intercepting instruction not
//! supported by Hyper-V instruction emulation" (Event ID 18570), rather than silently dropping
//! the write or no-opping the read. This is not a timing/race condition or a bad status-register
//! read — the very first `out`/`in` to this port range is fatal, regardless of retry count.
//!
//! Until a working output mechanism is implemented for this BSP (e.g. relaying through
//! `EFI_SERIAL_IO_PROTOCOL` while boot services are still active, like
//! `Shims/hyperv-shim/src/main.rs` does, or a synthetic/paravirtualized Hyper-V console), this
//! driver is a deliberate no-op stub: it implements the same `console::interface` as a real UART
//! so callers don't need `#[cfg]`-gating, but it does not touch any I/O port and therefore cannot
//! fault the guest.
//!
//! # TODO
//!
//! - Implement a real output path once one is confirmed not to fault on this Hyper-V
//!   configuration (candidates: a paravirtualized/synthetic console device, or framebuffer text
//!   output via the UEFI GOP handed off before `ExitBootServices()`).
//!
//! With the `qemu` feature, output is mirrored to QEMU's emulated 16550 COM1 and a
//! scrolling GOP framebuffer text console instead of the Hyper-V stub.

use crate::{console::interface, synchronization::IRQSafeNullLock};
use core::fmt;

#[cfg(feature = "qemu")]
unsafe fn read_port(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

#[cfg(feature = "qemu")]
unsafe fn write_port(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Inner, lock-protected UART state.
struct UartInner {
    chars_written: usize,
    #[cfg(feature = "qemu")]
    screen: Option<super::video::TextConsole>,
}

/// Representation of the UART.
pub struct Uart {
    inner: IRQSafeNullLock<UartInner>,
}

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

impl UartInner {
    const fn new() -> Self {
        Self {
            chars_written: 0,
            #[cfg(feature = "qemu")]
            screen: None,
        }
    }

    fn init(&mut self) -> Result<(), &'static str> {
        #[cfg(feature = "qemu")]
        {
            unsafe {
                write_port(0x3F9, 0x00);
                write_port(0x3FB, 0x80);
                write_port(0x3F8, 0x01);
                write_port(0x3F9, 0x00);
                write_port(0x3FB, 0x03);
                write_port(0x3FA, 0xC7);
                write_port(0x3FC, 0x0B);
            }
            self.screen = Some(super::video::TextConsole::new()?);
        }
        Ok(())
    }

    fn write_byte(&mut self, _byte: u8) {
        #[cfg(feature = "qemu")]
        {
            unsafe {
                while read_port(0x3FD) & 0x20 == 0 {
                    core::hint::spin_loop();
                }
                write_port(0x3F8, _byte);
            }
            if let Some(screen) = self.screen.as_mut() {
                screen.write_byte(_byte);
            }
        }
        self.chars_written += 1;
    }
}

impl fmt::Write for UartInner {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if c == '\n' {
                self.write_byte(b'\r');
            }
            let byte = if c.is_ascii() { c as u8 } else { b'?' };
            self.write_byte(byte);
        }

        Ok(())
    }
}

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

pub static UART: Uart = Uart::new();

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

impl Uart {
    /// Create an instance.
    pub const fn new() -> Self {
        Self {
            inner: IRQSafeNullLock::new(UartInner::new()),
        }
    }

    /// Bring up the UART and optional framebuffer. Must be called once before first use.
    /// A framebuffer error leaves the serial output path available.
    pub fn init(&self) -> Result<(), &'static str> {
        use crate::synchronization::interface::Mutex;

        self.inner.lock(|inner| inner.init())
    }
}

impl interface::Write for Uart {
    fn write_char(&self, c: char) {
        use crate::synchronization::interface::Mutex;

        self.inner.lock(|inner| {
            let byte = if c.is_ascii() { c as u8 } else { b'?' };
            inner.write_byte(byte);
        });
    }

    fn write_array(&self, a: &[char]) {
        use crate::synchronization::interface::Mutex;

        self.inner.lock(|inner| {
            for c in a {
                let byte = if c.is_ascii() { *c as u8 } else { b'?' };
                inner.write_byte(byte);
            }
        });
    }

    fn write_fmt(&self, args: fmt::Arguments) -> fmt::Result {
        use crate::synchronization::interface::Mutex;
        use core::fmt::Write;

        self.inner.lock(|inner| inner.write_fmt(args))
    }

    fn flush(&self) {
        #[cfg(feature = "qemu")]
        unsafe {
            while read_port(0x3FD) & 0x40 == 0 {
                core::hint::spin_loop();
            }
        }
    }
}

impl interface::Read for Uart {
    fn clear_rx(&self) {}
}

impl interface::Statistics for Uart {
    fn chars_written(&self) -> usize {
        use crate::synchronization::interface::Mutex;

        self.inner.lock(|inner| inner.chars_written)
    }
}

impl interface::All for Uart {}

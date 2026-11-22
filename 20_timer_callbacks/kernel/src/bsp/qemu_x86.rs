// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! QEMU PC x86-64 BSP, booted through UEFI.

pub use super::_x86::{
    acpi, apic, console, cpu, exception, gdt, idt, memory, trampoline, user, video,
};

pub fn board_name() -> &'static str {
    "QEMU PC x86_64 (UEFI)"
}

pub mod driver {
    pub fn init() -> Result<(), &'static str> {
        unsafe {
            super::idt::init(super::gdt::SEL_CODE64);
            super::gdt::init();
        }

        super::exception::asynchronous::register_x86_irq_manager();

        let console_result = super::console::UART.init();
        crate::console::register_console(&super::console::UART);

        if let Err(message) = console_result {
            crate::warn!("Framebuffer console unavailable: {}", message);
        }

        unsafe { super::cpu::discover_cores()?; }

        Ok(())
    }

    #[cfg(feature = "test_build")]
    pub fn qemu_bring_up_console() {
        let console_result = super::console::UART.init();
        crate::console::register_console(&super::console::UART);

        if let Err(message) = console_result {
            crate::warn!("Framebuffer console unavailable: {}", message);
        }
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Hyper-V x86_64 guest BSP (UEFI-booted, via the `Shims/hyperv-shim` loader).
//!
//! This BSP targets an x86_64 guest running under Hyper-V on an x86_64 host — confirmed viable,
//! unlike the earlier (removed) AArch64 attempt which cannot run on this host's CPU.
//!
//! # TODO
//!
//! Console output (COM1 UART) and CPU discovery (ACPI MADT + Local APIC) are wired up. Actually
//! releasing secondary cores still needs a 16-bit real-mode trampoline (see the `TODO` on
//! `cpu::HyperVSmpRelease::release_core`). Real paging and interrupts are added incrementally in
//! subsequent steps.

pub mod acpi;
pub mod apic;
pub mod console;
pub mod cpu;
pub mod exception;
pub mod gdt;
pub mod idt;
pub mod memory;
pub mod trampoline;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Board identification.
pub fn board_name() -> &'static str {
    "Hyper-V x86_64 Guest (UEFI)"
}

/// BSP driver subsystem.
pub mod driver {
    /// Initialize the BSP driver subsystem.
    ///
    /// Brings up the COM1 UART and registers it as the active console, then registers the
    /// bring-up-phase IRQ manager stub. This is the very first thing that must happen during
    /// boot, since without it there is no way to observe any subsequent output on real
    /// hardware/Hyper-V.
    ///
    /// Also parses the ACPI MADT to discover secondary cores. This is best-effort: if ACPI
    /// parsing fails (e.g. the shim couldn't find the RSDP), the board simply falls back to the
    /// single-entry boot-core-only table rather than failing boot entirely, since console output
    /// and driver init are considerably more important than SMP at this bring-up stage.
    pub fn init() -> Result<(), &'static str> {
        // Load a minimal, kernel-owned IDT *before* gdt::init()'s segment-register reload. UEFI's
        // own IDT can no longer be trusted post-ExitBootServices() (its handler code lives in
        // boot-services memory), so without this, the reload's control transfer (or any other
        // early fault) triple-faults the VM with zero diagnostics.
        unsafe { super::idt::init(super::gdt::SEL_CODE64) };

        unsafe { super::gdt::init() };

        super::exception::asynchronous::register_hyperv_irq_manager();

        super::console::UART.init();

        crate::console::register_console(&super::console::UART);

        if let Err(e) = unsafe { super::cpu::discover_cores() } {
            crate::warn!("ACPI CPU discovery failed, continuing single-core: {}", e);
        }

        Ok(())
    }

    /// Minimal console bring-up for unit tests, mirroring the other BSPs' test hook.
    #[cfg(feature = "test_build")]
    pub fn qemu_bring_up_console() {
        super::console::UART.init();

        crate::console::register_console(&super::console::UART);
    }
}

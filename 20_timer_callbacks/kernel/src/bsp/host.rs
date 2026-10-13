// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Host BSP (x86_64 stub, for algorithm testing only — not a real board).

pub mod console;
pub mod cpu;
pub mod exception;
pub mod memory;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Board identification.
pub fn board_name() -> &'static str {
    "Host (x86_64 algorithm test stub — no real hardware)"
}

/// BSP driver subsystem.
pub mod driver {
    /// Initialize the BSP driver subsystem.
    ///
    /// There are no real devices on host; this registers the host's minimal IRQ manager so that
    /// `register_and_enable_irq_handler()` calls made during `init_drivers_and_irqs()` succeed
    /// instead of hitting the `NullIRQManager` panic. It also switches the active console away
    /// from `BufferConsole`, since that console uses `InitStateLock` internally and would panic
    /// once used after the kernel init phase concludes.
    pub fn init() -> Result<(), &'static str> {
        super::exception::asynchronous::register_host_irq_manager();

        crate::console::register_console(&super::console::HOST_CONSOLE);

        Ok(())
    }

    /// Minimal console bring-up for unit tests.
    ///
    /// On host, the console is stdout and needs no MMIO setup, so this just registers it
    /// directly, mirroring the Raspberry Pi BSP's `qemu_bring_up_console()`.
    #[cfg(feature = "test_build")]
    pub fn qemu_bring_up_console() {
        crate::console::register_console(&super::console::HOST_CONSOLE);
    }
}

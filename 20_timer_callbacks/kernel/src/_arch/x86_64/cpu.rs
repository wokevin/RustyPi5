// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural processor code (x86_64).
//!
//! Two x86_64 targets share this file:
//! - `bsp_host`: a hosted (std-enabled) algorithm-test stub, not a real boot target.
//! - `bsp_hyperv_x86`: a bare-metal (no_std) target that actually boots under Hyper-V via the
//!   `Shims/hyperv-shim` UEFI loader.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::cpu::arch_cpu

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// No-op / spin hint.
#[inline(always)]
pub fn nop() {
    core::hint::spin_loop();
}

/// Pause execution on the core.
///
/// On the hosted (`bsp_host`) build, there is no hardware core to halt, so this parks the
/// calling thread indefinitely instead of busy-looping. On the bare-metal (`bsp_hyperv_x86`)
/// build, this instead issues `hlt` in a loop, since there is no OS thread to park.
#[inline(always)]
pub fn wait_forever() -> ! {
    #[cfg(feature = "bsp_host")]
    loop {
        std::thread::park();
    }

    #[cfg(any(feature = "bsp_hyperv_x86", feature = "bsp_qemu_x86"))]
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

//--------------------------------------------------------------------------------------------------
// Testing
//--------------------------------------------------------------------------------------------------

/// Make the host process exit with failure status.
///
/// `std::process::exit()` does not flush stdio buffers on its own. When stdout isn't a TTY (e.g.
/// piped through `cargo test`), it is fully block-buffered, so anything printed by the test
/// runner would otherwise be silently lost. Flush explicitly first.
#[cfg(feature = "test_build")]
pub fn qemu_exit_failure() -> ! {
    use std::io::Write;
    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();

    std::process::exit(1)
}

/// Make the host process exit with success status.
///
/// See the note on [`qemu_exit_failure`] regarding the explicit flush.
#[cfg(feature = "test_build")]
pub fn qemu_exit_success() -> ! {
    use std::io::Write;
    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();

    std::process::exit(0)
}

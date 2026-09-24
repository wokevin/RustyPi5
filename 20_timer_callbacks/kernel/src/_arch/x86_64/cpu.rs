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

    #[cfg(feature = "bsp_hyperv_x86")]
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
#[cfg(feature = "test_build")]
pub fn qemu_exit_failure() -> ! {
    std::process::exit(1)
}

/// Make the host process exit with success status.
#[cfg(feature = "test_build")]
pub fn qemu_exit_success() -> ! {
    std::process::exit(0)
}

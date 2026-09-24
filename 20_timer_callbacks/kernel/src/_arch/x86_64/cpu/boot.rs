// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! x86_64 kernel entry point.
//!
//! This is the target the `Shims/hyperv-shim` UEFI loader jumps to after calling
//! `ExitBootServices()`. Unlike the AArch64 path (`_arch/aarch64/cpu/boot.s`), there is no
//! separate assembly stage here yet — the shim is expected to have already set up a valid stack
//! before jumping here, since UEFI leaves us in long mode with paging already enabled by the
//! firmware (identity-mapped or otherwise, to be reconciled once our own paging is implemented).
//!
//! # TODO
//!
//! - No BSS zeroing yet (needed once the kernel has `.bss`-based statics, e.g. the allocator).
//! - No stack setup yet — currently assumes the shim leaves us on a usable stack.
//! - `kernel_init()` is called directly; secondary-core bring-up is not wired up for x86_64 yet.

/// The Rust entry point, called from the shim after `ExitBootServices()`.
///
/// # Safety
///
/// - Must only be called once, by the shim, on the boot core.
/// - No environment setup (stack, BSS, paging) is guaranteed correct yet; this is a bring-up
///   skeleton only.
#[no_mangle]
pub unsafe extern "sysv64" fn _start() -> ! {
	extern "Rust" {
		fn kernel_init() -> !;
	}

	kernel_init()
}

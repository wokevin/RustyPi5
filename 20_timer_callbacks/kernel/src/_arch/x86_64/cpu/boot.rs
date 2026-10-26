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

use core::arch::asm;

/// The Rust entry point, called from the shim after `ExitBootServices()`.
///
/// `rsdp_addr` is the physical address of the ACPI RSDP, as discovered by the shim while UEFI
/// boot services were still active (post-`ExitBootServices()`, walking the UEFI configuration
/// table is no longer possible). A value of `0` means the shim could not find it.
///
/// `framebuffer_info_addr` is the physical address of a `bsp::video::FramebufferInfo` populated
/// by the shim via GOP, also while boot services were still active. A `base` field of `0` inside
/// it means no GOP framebuffer was found/supported; the kernel must check that before treating
/// any other field in it as valid.
///
/// # Safety
///
/// - Must only be called once, by the shim, on the boot core.
/// - No environment setup (stack, BSS, paging) is guaranteed correct yet; this is a bring-up
///   skeleton only.
#[no_mangle]
pub unsafe extern "sysv64" fn _start(_rsdp_addr: usize, _framebuffer_info_addr: usize) -> ! {
	#[allow(unreachable_code)]
	{
		extern "Rust" {
			fn kernel_init() -> !;
		}

		crate::bsp::acpi::set_rsdp_addr(_rsdp_addr);
		crate::bsp::video::set_framebuffer_info_addr(_framebuffer_info_addr);

		kernel_init()
	}
}

/// Secondary-core entry point.
///
/// Unlike the AArch64 path (`_start_secondary` in `boot.s`), this needs no EL-check, BSS zeroing,
/// or MMU-enable step: by the time `bsp::hyperv_x86::trampoline`'s real-mode code reaches this
/// point, the AP is already running in 64-bit long mode, using the boot core's page tables and
/// GDT, with `RSP` already pointing at this core's reserved stack (all set up directly by the
/// trampoline — see its `lm_entry` — since there is no equivalent of AArch64's post-PSCI/spin-table
/// "MMU-disabled" intermediate state on x86_64's INIT-SIPI-SIPI mechanism once the trampoline has
/// done its job).
///
/// # Safety
///
/// - Must only ever be reached via `bsp::hyperv_x86::trampoline`'s real-mode code, immediately
///   after a secondary core has been released via INIT-SIPI-SIPI.
/// - `RSP` must already be valid (set by the trampoline) before this is reached.
#[no_mangle]
pub unsafe extern "sysv64" fn _start_secondary() -> ! {
	extern "Rust" {
		fn kernel_init_secondary() -> !;
	}

	kernel_init_secondary()
}

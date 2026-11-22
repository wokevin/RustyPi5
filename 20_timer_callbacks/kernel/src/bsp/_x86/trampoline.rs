// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! 16-bit real-mode AP (Application Processor) trampoline.
//!
//! Every x86_64 AP always starts executing in real mode at the physical address encoded by the
//! SIPI vector, regardless of how the BSP itself was booted (UEFI/long mode makes no difference
//! here — that's purely a BSP-side firmware convention). This module is what gets the AP from
//! that real-mode reset state into 64-bit long mode, running our own `_start_secondary` Rust
//! entry point.
//!
//! # Design
//!
//! - The trampoline blob (see `TRAMPOLINE_BLOB`, built from `global_asm!` below) is copied to a
//!   fixed physical address, [`TRAMPOLINE_PHYS_ADDR`], immediately before releasing a core. This
//!   address is 4 KiB-aligned, as required by the SIPI vector encoding
//!   (`vector = TRAMPOLINE_PHYS_ADDR / 0x1000`).
//! - A small scratch struct, [`ApBootInfo`], is written to a fixed physical address,
//!   [`BOOT_INFO_PHYS_ADDR`], just below the trampoline. The trampoline (in real mode, so it
//!   cannot take Rust-level arguments) reads this struct directly by its known physical address.
//! - The AP reuses the boot core's existing page tables (`CR3`) and GDT verbatim. This is valid
//!   only as long as physical memory stays identity-mapped (true throughout this bring-up phase —
//!   see `bsp::memory::mmu`'s no-op translation table) and only as long as the boot core's GDT
//!   remains resident at a fixed physical address for the AP's lifetime (true: `_arch::x86_64`
//!   never relocates or frees it).
//! - The A20 line is assumed already enabled. This holds for every UEFI/Hyper-V environment this
//!   BSP targets — UEFI firmware enables A20 before handing control to any OS loader, and it is
//!   never a real-mode BIOS environment where A20 could still be masked.
//!
//! The QEMU secondary entry replaces the bootstrap GDT with per-CPU GDT/TSS state before
//! publishing that CPU as online. Shared bootstrap scratch is reused only after acknowledgement.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Physical address the trampoline blob is copied to before every core release.
///
/// Must be 4 KiB-aligned (SIPI vector encoding requirement) and must not overlap
/// [`BOOT_INFO_PHYS_ADDR`] or any memory the firmware/BSP is still using at this point in boot.
/// `0x8000` is conventional for this purpose (used by Linux, most hobby OSes, etc.) and safely
/// below the `0x7c00`-and-up "commonly free" low-memory region on essentially all firmware,
/// including Hyper-V's synthetic UEFI.
pub const TRAMPOLINE_PHYS_ADDR: usize = 0x8000;

/// The SIPI vector corresponding to [`TRAMPOLINE_PHYS_ADDR`] (`addr / 0x1000`).
pub const TRAMPOLINE_SIPI_VECTOR: u8 = (TRAMPOLINE_PHYS_ADDR / 0x1000) as u8;

/// Physical address of the [`ApBootInfo`] scratch struct, read directly by the real-mode
/// trampoline. Placed just below the trampoline code itself.
pub const BOOT_INFO_PHYS_ADDR: usize = 0x7000;

/// Data handed from `release_core()` to the real-mode trampoline.
///
/// Field order and layout must exactly match what the assembly in `global_asm!` below expects —
/// any change here must be mirrored there.
#[repr(C)]
struct ApBootInfo {
	/// Physical address of the boot core's top-level page table (to be loaded into `CR3`).
	cr3: u64,

	/// Linear address of a 6-byte pseudo-descriptor (`limit: u16, base: u64`) describing the
	/// boot core's GDT, suitable for a real-mode `lgdt`.
	gdtr_addr: u64,

	/// Exclusive-end stack address for this AP (its initial `RSP` in long mode).
	stack_top: u64,

	/// The Rust entry point to jump to once in long mode (`_start_secondary`).
	entry_point: u64,

	/// Set to `1` by the trampoline immediately after it reads this struct, so `release_core()`
	/// can tell the AP has actually started executing (as opposed to SIPI having been silently
	/// ignored, which does happen on some hypervisors/hardware if timing is off).
	claimed: u32,
}

/// The trampoline machine code, assembled at build time and embedded as a byte blob.
///
/// # Safety
///
/// This blob is position-*dependent*: it assumes it will be copied verbatim to
/// [`TRAMPOLINE_PHYS_ADDR`] and executed from there in real mode (`CS:IP = vector:0000`). It also
/// hardcodes [`BOOT_INFO_PHYS_ADDR`] as an absolute address, so the two constants must be kept in
/// sync with the assembly below.
extern "C" {
    #[link_name = "__ap_trampoline_start"]
    static TRAMPOLINE_START: u8;
    #[link_name = "__ap_trampoline_end"]
    static TRAMPOLINE_END: u8;
}

// SAFETY / maintenance note: `BOOT_INFO_PHYS_ADDR` (0x7000) and `TRAMPOLINE_PHYS_ADDR` (0x8000)
// are baked into this assembly as immediates; if either constant above changes, these `mov`
// immediates must be updated to match.
global_asm!(
	r#"
.section .text.ap_trampoline, "ax"
.global __ap_trampoline_start
.global __ap_trampoline_end
.code16
__ap_trampoline_start:
	cli
	cld

	// Zero segments; we execute from CS:IP = (vector):0000, so use offsets relative to 0.
	xor ax, ax
	mov ds, ax
	mov es, ax
	mov ss, ax

	// Load the boot core's GDT (its pseudo-descriptor lives at a known physical/linear address;
	// real mode's segment:offset addressing with DS=0 means linear == offset here).
	mov eax, 0x7000 + 8      // &ApBootInfo.gdtr_addr
	mov ebx, [eax]
	.byte 0x66              // 32-bit GDT base, even above 16 MiB
	lgdt [ebx]

	// Enable protected mode.
	mov eax, cr0
	or al, 1
	mov cr0, eax

	// Flush the prefetch queue / instruction pipeline with a far jump into 32-bit protected mode.
	// Selector 0x08 is assumed to be a valid 32-bit flat code segment in the boot core's GDT (see
	// `_arch::x86_64::cpu::gdt`, which is expected to lay the GDT out with entry 1 == 32-bit code,
	// entry 2 == 64-bit code, matching the classic/Linux convention).
	ljmp 0x08, offset pm_entry

.code32
pm_entry:
	mov ax, 0x10             // flat data selector, conventionally right after the 32-bit code seg
	mov ds, ax
	mov es, ax
	mov ss, ax

	// Enable PAE (required before entering long mode).
	mov eax, cr4
	or eax, (1 << 5) | (1 << 9) | (1 << 10)
	mov cr4, eax

	// Load the boot core's page tables.
	mov eax, [0x7000]        // ApBootInfo.cr3 (low dword; we are still 32-bit here)
	mov cr3, eax

	// Enable long mode (set EFER.LME) via MSR 0xC0000080.
	mov ecx, 0xC0000080
	rdmsr
	or eax, 1 << 8
	wrmsr

	// Enable paging -> this actually activates long mode (in "compatibility submode" until the
	// next far jump loads a 64-bit code segment).
	mov eax, cr0
	and eax, ~((1 << 2) | (1 << 3) | (1 << 29) | (1 << 30))
	or eax, (1 << 31) | (1 << 1)
	mov cr0, eax

	// Far jump into genuine 64-bit long mode using the 64-bit code selector (0x18, entry 3).
	ljmp 0x18, offset lm_entry

.code64
lm_entry:
	mov ax, 0x10
	mov ds, ax
	mov es, ax
	mov ss, ax

	// Load our reserved stack.
	mov rax, 0x7000 + 16      // &ApBootInfo.stack_top
	mov rsp, [rax]

	// Capture the entry point before clobbering rax with the "claimed" address below.
	mov rax, 0x7000 + 24      // &ApBootInfo.entry_point
	mov rbx, [rax]

	// Mark ourselves claimed so `release_core()` can observe that SIPI was actually accepted.
	mov rax, 0x7000 + 32      // &ApBootInfo.claimed
	mov dword ptr [rax], 1

	xor rbp, rbp
	call rbx                // SysV entry requires RSP = 8 mod 16
	ud2

__ap_trampoline_end:
.code64
"#
);

/// Marker so `release_core()` can poll for successful AP startup.
static AP_CLAIMED_SEEN: AtomicU64 = AtomicU64::new(0);

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Install the trampoline and its boot-info scratch struct in low memory, then send
/// INIT-SIPI-SIPI to `target_apic_id` to actually start it running.
///
/// Returns once the SIPI sequence has been sent; does **not** wait for the AP to come online —
/// callers (see `cpu::smp::bring_up_all_cores()`) already poll `Cpu::is_online()` separately.
///
/// # Safety
///
/// - `stack_top` must point to the exclusive end of a stack region reserved solely for this core.
/// - `entry_point` must be a valid, never-returning function pointer expecting the System V AMD64
///   ABI, reachable directly (not through a trampoline of its own) once in long mode.
/// - Physical memory in `[BOOT_INFO_PHYS_ADDR, TRAMPOLINE_PHYS_ADDR + trampoline_len)` must
///   currently be free, identity-mapped, and not concurrently used by anything else (true during
///   this bring-up phase, before any general-purpose physical allocator exists).
/// - `target_apic_id` must identify a genuinely parked AP.
pub unsafe fn release_via_trampoline(
	target_apic_id: u8,
	entry_point: usize,
	stack_top: usize,
) -> Result<(), &'static str> {
	let (gdtr_raw, cr3) = super::gdt::current_gdtr_and_cr3();

	// The trampoline's `lgdt` needs a *pointer to* a pseudo-descriptor, not the descriptor's raw
	// bytes inline (real-mode code can't easily carry a 10-byte immediate). Stash the captured
	// descriptor right after the `ApBootInfo` struct and point `gdtr_addr` at it.
	let gdtr_scratch_addr = BOOT_INFO_PHYS_ADDR + core::mem::size_of::<ApBootInfoRepr>();
	core::ptr::copy_nonoverlapping(
		gdtr_raw.as_ptr(),
		gdtr_scratch_addr as *mut u8,
		gdtr_raw.len(),
	);

	// Copy the assembled trampoline blob to its fixed physical run address.
	let start = &TRAMPOLINE_START as *const u8;
	let end = &TRAMPOLINE_END as *const u8;
	let len = end as usize - start as usize;

	if len == 0 || len > 0x1000 {
		return Err("AP trampoline blob too large for its reserved low-memory window");
	}

	if start as usize != TRAMPOLINE_PHYS_ADDR {
		core::ptr::copy_nonoverlapping(start, TRAMPOLINE_PHYS_ADDR as *mut u8, len);
	}

	// Populate the boot-info scratch struct the trampoline will read.
	let info = BOOT_INFO_PHYS_ADDR as *mut ApBootInfoRepr;
	core::ptr::write_volatile(
		info,
		ApBootInfoRepr {
			cr3,
			gdtr_addr: gdtr_scratch_addr as u64,
			stack_top: stack_top as u64,
			entry_point: entry_point as u64,
			claimed: 0,
		},
	);

	super::apic::start_core(target_apic_id, TRAMPOLINE_SIPI_VECTOR)?;

	// Confirm all shared scratch has been consumed before a subsequent AP release may reuse it.
	// Rust readiness is checked separately through Cpu::is_online().
	for _ in 0..1_000_000 {
		let claimed = core::ptr::read_volatile(core::ptr::addr_of!((*info).claimed));
		if claimed != 0 {
			AP_CLAIMED_SEEN.fetch_add(1, Ordering::Relaxed);
			return Ok(());
		}
		core::hint::spin_loop();
	}

	Err("Secondary CPU did not claim bootstrap storage; refusing to reuse it")
}

/// Field-for-field identical to the private `ApBootInfo` doc struct above; kept as a separate,
/// non-`packed` type so field offsets used by both the assembly (as hardcoded immediates) and the
/// `core::mem::size_of` call above stay obviously in sync. Field offsets (all naturally
/// 8-byte-aligned on this `#[repr(C)]` layout): `cr3` = 0, `gdtr_addr` = 8, `stack_top` = 16,
/// `entry_point` = 24, `claimed` = 32. If any field is added/reordered, every hardcoded offset in
/// the `global_asm!` block above must be updated to match.
#[repr(C)]
struct ApBootInfoRepr {
	cr3: u64,
	gdtr_addr: u64,
	stack_top: u64,
	entry_point: u64,
	claimed: u32,
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Global Descriptor Table (GDT) for the Hyper-V x86_64 guest BSP.
//!
//! UEFI leaves the CPU running on a firmware-owned GDT that is not guaranteed to survive once
//! `ExitBootServices()` memory is reused, and whose selector layout is not something this kernel
//! controls. This module builds and loads a small, kernel-owned GDT with a fixed, known selector
//! layout, so that other code (notably `bsp::hyperv_x86::trampoline`, which hardcodes these
//! selector values into real-mode assembly) has something stable to depend on.
//!
//! # Selector layout
//!
//! | Selector | Index | Description               |
//! |----------|-------|----------------------------|
//! | `0x00`   | 0     | Null descriptor (required) |
//! | `0x08`   | 1     | 32-bit flat code segment   |
//! | `0x10`   | 2     | Flat data segment          |
//! | `0x18`   | 3     | 64-bit flat code segment   |
//!
//! This ordering and these exact selector values are load-bearing: `bsp::hyperv_x86::trampoline`
//! hardcodes `0x08`/`0x10`/`0x18` in its real/protected-mode assembly. If this layout ever
//! changes, that assembly must be updated to match.

use core::arch::asm;

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// A raw 8-byte GDT entry, in the classic x86 segment descriptor format.
#[repr(C)]
#[derive(Copy, Clone)]
struct GdtEntry(u64);

impl GdtEntry {
	/// The null descriptor. Required by the architecture to occupy index 0.
	const NULL: GdtEntry = GdtEntry(0);

	/// A 32-bit flat code segment: base `0`, limit `0xFFFFF` (4 KiB-granular, so 4 GiB), present,
	/// ring 0, executable/readable, non-conforming.
	const fn flat_code_32() -> Self {
		// Access byte: P=1, DPL=00, S=1, Type=1010 (code, readable, non-conforming) = 0x9A.
		// Flags: G=1 (4 KiB granularity), D/B=1 (32-bit) = 0xC.
		Self(Self::pack(0x9A, 0xC))
	}

	/// A flat data segment: base `0`, limit `0xFFFFF`, present, ring 0, writable.
	///
	/// Shared by both the 32-bit protected-mode stage and 64-bit long-mode stage of the AP
	/// trampoline (data segment attributes are ignored in 64-bit mode beyond the descriptor being
	/// present and marked as data, so one entry suffices for both).
	const fn flat_data() -> Self {
		// Access byte: P=1, DPL=00, S=1, Type=0010 (data, writable) = 0x92.
		// Flags: G=1, D/B=1 = 0xC.
		Self(Self::pack(0x92, 0xC))
	}

	/// A 64-bit flat code segment. In 64-bit mode, the base/limit fields are ignored by the CPU
	/// (a true flat model is architecturally enforced), but `L=1` (long mode) must be set and
	/// `D=0`.
	const fn flat_code_64() -> Self {
		// Access byte: P=1, DPL=00, S=1, Type=1010 (code, readable, non-conforming) = 0x9A.
		// Flags: G=1, L=1 (64-bit code), D/B=0 = 0xA.
		Self(Self::pack(0x9A, 0xA))
	}

	/// Pack an access byte and a 4-bit flags nibble into a full descriptor with base `0` and
	/// limit `0xFFFFF`, matching every entry this GDT needs.
	const fn pack(access: u8, flags: u8) -> u64 {
		let limit_low: u64 = 0xFFFF;
		let base_low: u64 = 0;
		let base_mid: u64 = 0;
		let base_high: u64 = 0;
		let limit_high: u64 = 0xF;

		limit_low
			| (base_low << 16)
			| (base_mid << 32)
			| ((access as u64) << 40)
			| (limit_high << 48)
			| ((flags as u64) << 52)
			| (base_high << 56)
	}
}

/// The kernel's GDT. `'static` and never mutated after `init()`, so every core (boot and
/// secondary) can safely share and reload it.
static GDT: [GdtEntry; 4] = [
	GdtEntry::NULL,        // 0x00
	GdtEntry::flat_code_32(), // 0x08
	GdtEntry::flat_data(),    // 0x10
	GdtEntry::flat_code_64(), // 0x18
];

/// A GDT pseudo-descriptor, in the exact byte layout the `lgdt`/`sgdt` instructions expect:
/// a 16-bit limit immediately followed by a 64-bit linear base address.
#[repr(C, packed)]
struct GdtPointer {
	limit: u16,
	base: u64,
}

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Selector for the 32-bit flat code segment (index 1).
pub const SEL_CODE32: u16 = 0x08;

/// Selector for the flat data segment (index 2).
pub const SEL_DATA: u16 = 0x10;

/// Selector for the 64-bit flat code segment (index 3).
pub const SEL_CODE64: u16 = 0x18;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Build and load the kernel's GDT on the current core, then reload every segment register to
/// reference it (required — `lgdt` alone does not retroactively fix up already-loaded segment
/// registers).
///
/// Must be called once on the boot core during early `kernel_init()`, before
/// `bsp::hyperv_x86::trampoline::release_via_trampoline()` captures `sgdt`/`cr3` state for
/// secondary cores (that capture assumes *this* GDT, not whatever UEFI left behind).
///
/// # Safety
///
/// - Must be called with interrupts disabled (true throughout early boot, before
///   `exception::asynchronous::local_irq_unmask()` runs).
/// - The `GDT` static must remain valid (`'static`) and at a fixed physical address for as long
///   as any core — boot or secondary — might still reference it, which is the entire remaining
///   lifetime of the kernel.
pub unsafe fn init() {
	let ptr = GdtPointer {
		limit: (core::mem::size_of_val(&GDT) - 1) as u16,
		base: GDT.as_ptr() as u64,
	};

	asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));

	// Reload every segment register. `CS` requires a far return (or far jump/call) rather than a
	// direct `mov`, since the CPU only re-validates/reloads `CS` as a side effect of a control
	// transfer that specifies a new selector.
	asm!(
		"push {sel}",
		"lea rax, [rip + 1f]",
		"push rax",
		"retfq",
		"1:",
		sel = in(reg) SEL_CODE64 as u64,
		out("rax") _,
		options(preserves_flags),
	);

	asm!(
		"mov ds, {sel:x}",
		"mov es, {sel:x}",
		"mov ss, {sel:x}",
		"mov fs, {sel:x}",
		"mov gs, {sel:x}",
		sel = in(reg) SEL_DATA,
		options(nostack, preserves_flags),
	);
}

/// Return the linear address of the currently-loaded GDT's pseudo-descriptor, and its physical
/// `CR3`, packaged for `bsp::hyperv_x86::trampoline` to hand off to a newly-released AP.
///
/// # Safety
///
/// - `init()` must have already run on this core.
pub unsafe fn current_gdtr_and_cr3() -> ([u8; 10], u64) {
	let mut gdtr_raw = [0u8; 10];
	asm!("sgdt [{}]", in(reg) gdtr_raw.as_mut_ptr(), options(nostack));

	let cr3: u64;
	asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));

	(gdtr_raw, cr3)
}

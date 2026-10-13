// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Minimal Interrupt Descriptor Table (IDT) for the Hyper-V x86_64 guest BSP.
//!
//! UEFI leaves the CPU running on a firmware-owned IDT whose handler code lives in
//! boot-services-owned memory. Once `ExitBootServices()` has been called, that memory is no
//! longer guaranteed to be valid or even mapped — so the very first CPU exception taken after
//! that point (even an entirely benign one, e.g. as a side effect of reloading segment registers
//! in `gdt::init()`) can jump into garbage and immediately triple-fault the VM. This module
//! installs a small, kernel-owned IDT with a distinct handler per CPU exception vector (0-31), so
//! a fault at least halts predictably on a known vector instead of jumping into firmware garbage.
//!
//! Must be loaded *before* `gdt::init()`'s segment-register reload, and before anything else that
//! could plausibly fault.
//!
//! # Diagnostics
//!
//! These handlers do **not** attempt to print the firing vector via raw COM1 port I/O. This Gen 2
//! Hyper-V configuration's hypervisor intercepts legacy `out dx, al` / `in al, dx` instructions
//! and faults the guest immediately ("the guest executed an intercepting instruction not
//! supported by Hyper-V instruction emulation", Event ID 18570) rather than silently dropping the
//! access. Attempting that from inside an exception handler is a fault-within-a-fault with no
//! handler for the second one, which cascades straight into a triple fault — masking the original
//! exception entirely instead of reporting it. Until a working output mechanism exists for this
//! BSP (see `console.rs`'s module docs), each handler instead parks the vector number in a
//! well-known static that can be inspected with an external debugger/hypervisor introspection
//! tool, then halts.

use core::arch::asm;
use core::sync::atomic::{AtomicU8, Ordering};

/// The exception vector of the most recent fault, for inspection via an external
/// debugger/hypervisor introspection tool (no in-guest print mechanism is available this early;
/// see the module-level docs). `0xFF` means "no exception vector 0-31 has fired yet".
pub static LAST_EXCEPTION_VECTOR: AtomicU8 = AtomicU8::new(0xFF);

/// Records `vector` in [`LAST_EXCEPTION_VECTOR`], then halts forever.
fn report_and_halt(vector: u8) {
	LAST_EXCEPTION_VECTOR.store(vector, Ordering::SeqCst);

	loop {
		unsafe { asm!("hlt", options(nomem, nostack)) };
	}
}

/// Minimal exception stack frame layout pushed by the CPU before entering an interrupt-gate
/// handler with no error code.
#[repr(C)]
struct ExceptionStackFrame {
	_ip: u64,
	_cs: u64,
	_flags: u64,
	_sp: u64,
	_ss: u64,
}

/// Generates a distinct `extern "x86-interrupt"` handler function for a given vector number, so
/// each one can report which vector actually fired instead of a single anonymous catch-all.
/// `$has_err` selects whether the CPU pushes an extra error-code word for this vector (per the
/// x86_64 architecture spec), since the stack frame layout differs and getting it wrong would
/// itself corrupt the stack.
macro_rules! exception_handler {
	($name:ident, $vector:expr, no_err) => {
		extern "x86-interrupt" fn $name(_stack_frame: ExceptionStackFrame) {
			report_and_halt($vector);
		}
	};
	($name:ident, $vector:expr, err) => {
		extern "x86-interrupt" fn $name(_stack_frame: ExceptionStackFrame, _error_code: u64) {
			report_and_halt($vector);
		}
	};
}

exception_handler!(vec00, 0, no_err);
exception_handler!(vec01, 1, no_err);
exception_handler!(vec02, 2, no_err);
exception_handler!(vec03, 3, no_err);
exception_handler!(vec04, 4, no_err);
exception_handler!(vec05, 5, no_err);
exception_handler!(vec06, 6, no_err);
exception_handler!(vec07, 7, no_err);
exception_handler!(vec08, 8, err);
exception_handler!(vec09, 9, no_err);
exception_handler!(vec10, 10, err);
exception_handler!(vec11, 11, err);
exception_handler!(vec12, 12, err);
exception_handler!(vec13, 13, err);
exception_handler!(vec14, 14, err);
exception_handler!(vec15, 15, no_err);
exception_handler!(vec16, 16, no_err);
exception_handler!(vec17, 17, err);
exception_handler!(vec18, 18, no_err);
exception_handler!(vec19, 19, no_err);
exception_handler!(vec20, 20, no_err);
exception_handler!(vec21, 21, err);
exception_handler!(vec22, 22, no_err);
exception_handler!(vec23, 23, no_err);
exception_handler!(vec24, 24, no_err);
exception_handler!(vec25, 25, no_err);
exception_handler!(vec26, 26, no_err);
exception_handler!(vec27, 27, no_err);
exception_handler!(vec28, 28, no_err);
exception_handler!(vec29, 29, err);
exception_handler!(vec30, 30, err);
exception_handler!(vec31, 31, no_err);

/// Catch-all for every vector 32-255 (IRQs and anything else): no distinct diagnostic needed yet,
/// since none of those are wired up or expected to fire this early.
extern "x86-interrupt" fn vec_other(_stack_frame: ExceptionStackFrame) {
	report_and_halt(255);
}

/// A raw 16-byte IDT gate descriptor (64-bit interrupt gate).
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct IdtEntry {
	offset_low: u16,
	selector: u16,
	ist: u8,
	type_attr: u8,
	offset_mid: u16,
	offset_high: u32,
	_reserved: u32,
}

impl IdtEntry {
	const MISSING: IdtEntry = IdtEntry {
		offset_low: 0,
		selector: 0,
		ist: 0,
		type_attr: 0,
		offset_mid: 0,
		offset_high: 0,
		_reserved: 0,
	};

	/// Builds a present, ring-0, 64-bit interrupt-gate descriptor pointing at `handler`.
	fn new(handler: usize, code_selector: u16) -> Self {
		Self {
			offset_low: (handler & 0xFFFF) as u16,
			selector: code_selector,
			ist: 0,
			// P=1, DPL=00, type=0xE (64-bit interrupt gate) -> 0x8E.
			type_attr: 0x8E,
			offset_mid: ((handler >> 16) & 0xFFFF) as u16,
			offset_high: ((handler >> 32) & 0xFFFF_FFFF) as u32,
			_reserved: 0,
		}
	}
}

/// 256-entry IDT. Vectors 0-31 get their own distinct handler; everything else falls back to
/// `vec_other`.
static mut IDT: [IdtEntry; 256] = [IdtEntry::MISSING; 256];

#[repr(C, packed)]
struct IdtPointer {
	limit: u16,
	base: u64,
}

/// Build and load a minimal, kernel-owned IDT on the current core, with vectors 0-31 each
/// pointing at their own distinct handler (see `exception_handler!` above) and everything else
/// pointing at `vec_other`.
///
/// # Safety
///
/// - Must be called with interrupts disabled.
/// - `code_selector` must be a valid, already-loaded 64-bit code segment selector (i.e. call this
///   after `gdt`'s descriptors exist, but — critically — before `gdt::init()` performs its
///   `CS`/data-segment reload, since that reload is itself a control transfer that can fault).
pub unsafe fn init(code_selector: u16) {
	let handlers: [usize; 32] = [
		vec00 as usize, vec01 as usize, vec02 as usize, vec03 as usize,
		vec04 as usize, vec05 as usize, vec06 as usize, vec07 as usize,
		vec08 as usize, vec09 as usize, vec10 as usize, vec11 as usize,
		vec12 as usize, vec13 as usize, vec14 as usize, vec15 as usize,
		vec16 as usize, vec17 as usize, vec18 as usize, vec19 as usize,
		vec20 as usize, vec21 as usize, vec22 as usize, vec23 as usize,
		vec24 as usize, vec25 as usize, vec26 as usize, vec27 as usize,
		vec28 as usize, vec29 as usize, vec30 as usize, vec31 as usize,
	];

	for (i, entry) in IDT.iter_mut().enumerate() {
		let handler = if i < 32 { handlers[i] } else { vec_other as usize };
		*entry = IdtEntry::new(handler, code_selector);
	}

	let ptr = IdtPointer {
		limit: (core::mem::size_of_val(&IDT) - 1) as u16,
		base: core::ptr::addr_of!(IDT) as u64,
	};

	asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Local APIC driver, used to send the INIT-SIPI-SIPI sequence to bring up secondary cores.
//!
//! This bring-up phase assumes the xAPIC (MMIO) interface at its architectural default physical
//! address, which is standard for Hyper-V Gen 2 guests and is not relocated by firmware in
//! practice. x2APIC (MSR-based) support can replace this later if needed.

use crate::time;
use core::time::Duration;

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Default physical (== identity-mapped virtual, during this bring-up phase) base address of the
/// xAPIC MMIO register block.
const APIC_BASE: usize = 0xFEE0_0000;

/// Interrupt Command Register, low dword. Writing this dword is what actually dispatches the IPI
/// described by the current contents of both ICR dwords.
const REG_ICR_LOW: usize = 0x300;

/// Interrupt Command Register, high dword. Bits `[31:24]` hold the destination APIC ID.
const REG_ICR_HIGH: usize = 0x310;

/// Delivery mode: INIT.
const DELIVERY_MODE_INIT: u32 = 0b101 << 8;

/// Delivery mode: Startup (SIPI).
const DELIVERY_MODE_STARTUP: u32 = 0b110 << 8;

/// Level: Assert (must be set for INIT; ignored for SIPI/Startup).
const LEVEL_ASSERT: u32 = 1 << 14;

/// Trigger mode: Edge (as opposed to Level).
const TRIGGER_EDGE: u32 = 0 << 15;

/// Read a 32-bit Local APIC register.
///
/// # Safety
///
/// - The Local APIC MMIO region must be mapped (identity-mapped during this bring-up phase).
#[inline(always)]
unsafe fn read_reg(offset: usize) -> u32 {
	core::ptr::read_volatile((APIC_BASE + offset) as *const u32)
}

/// Write a 32-bit Local APIC register.
///
/// # Safety
///
/// - The Local APIC MMIO region must be mapped (identity-mapped during this bring-up phase).
unsafe fn write_reg(offset: usize, value: u32) {
	core::ptr::write_volatile((APIC_BASE + offset) as *mut u32, value);
}

/// Busy-wait for the ICR's "Delivery Status" bit (12) to clear, indicating the previous IPI write
/// has been accepted by the APIC bus.
///
/// # Safety
///
/// - The Local APIC MMIO region must be mapped.
unsafe fn wait_for_icr_idle() {
	const DELIVERY_STATUS_PENDING: u32 = 1 << 12;

	while (read_reg(REG_ICR_LOW) & DELIVERY_STATUS_PENDING) != 0 {
		core::hint::spin_loop();
	}
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Send the standard INIT-SIPI-SIPI sequence to wake the AP identified by `target_apic_id`,
/// pointing it at the 16-bit real-mode trampoline located at `vector`.
///
/// `vector` is the trampoline's physical page number (i.e. `trampoline_phys_addr / 0x1000`), per
/// the SIPI vector encoding: the AP starts executing at physical address `vector * 0x1000` in real
/// mode with `CS = vector << 8`.
///
/// This follows the widely-used two-SIPI legacy sequence (INIT, 10ms delay, SIPI, 200us delay,
/// SIPI, 200us delay) recommended by the MP/MultiProcessor and Intel SDM Vol. 3A specifications
/// for maximum hardware/hypervisor compatibility.
///
/// # Safety
///
/// - `target_apic_id` must identify a genuinely parked AP; sending this to an already-running
///   core or an invalid ID is undefined behavior on real hardware.
/// - The 16-bit trampoline must already be installed at the physical address implied by `vector`,
///   fully set up to transition the AP through real mode -> protected mode -> long mode and then
///   jump into `_start_secondary` (see `TODO` in this crate's boot path — the trampoline itself is
///   not implemented yet).
/// - The Local APIC MMIO region must be mapped.
pub unsafe fn start_core(target_apic_id: u8, vector: u8) {
	let dest = (target_apic_id as u32) << 24;

	// 1. INIT (deasserted form isn't needed on modern APICs/hypervisors; assert-only is standard
	//    practice and what Hyper-V expects).
	write_reg(REG_ICR_HIGH, dest);
	write_reg(
		REG_ICR_LOW,
		DELIVERY_MODE_INIT | LEVEL_ASSERT | TRIGGER_EDGE,
	);
	wait_for_icr_idle();

	time::time_manager().spin_for(Duration::from_millis(10));

	// 2. First SIPI.
	write_reg(REG_ICR_HIGH, dest);
	write_reg(REG_ICR_LOW, DELIVERY_MODE_STARTUP | (vector as u32));
	wait_for_icr_idle();

	time::time_manager().spin_for(Duration::from_micros(200));

	// 3. Second SIPI (redundant on most modern hardware/hypervisors, but required by spec for
	//    older/pickier implementations).
	write_reg(REG_ICR_HIGH, dest);
	write_reg(REG_ICR_LOW, DELIVERY_MODE_STARTUP | (vector as u32));
	wait_for_icr_idle();

	time::time_manager().spin_for(Duration::from_micros(200));
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Local APIC startup and QEMU kernel-worker wakeup IPIs.
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
unsafe fn wait_for_icr_idle() -> Result<(), &'static str> {
	const DELIVERY_STATUS_PENDING: u32 = 1 << 12;

	for _ in 0..1_000_000 {
		if read_reg(REG_ICR_LOW) & DELIVERY_STATUS_PENDING == 0 { return Ok(()); }
		core::hint::spin_loop();
	}
	Err("Local APIC command delivery timed out")
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
pub unsafe fn start_core(target_apic_id: u8, vector: u8) -> Result<(), &'static str> {
	let dest = (target_apic_id as u32) << 24;
	wait_for_icr_idle()?;

	// 1. INIT (deasserted form isn't needed on modern APICs/hypervisors; assert-only is standard
	//    practice and what Hyper-V expects).
	write_reg(REG_ICR_HIGH, dest);
	write_reg(
		REG_ICR_LOW,
		DELIVERY_MODE_INIT | LEVEL_ASSERT | TRIGGER_EDGE,
	);
	wait_for_icr_idle()?;

	time::time_manager().spin_for(Duration::from_millis(10));

	// 2. First SIPI.
	write_reg(REG_ICR_HIGH, dest);
	write_reg(REG_ICR_LOW, DELIVERY_MODE_STARTUP | (vector as u32));
	wait_for_icr_idle()?;

	time::time_manager().spin_for(Duration::from_micros(200));

	// 3. Second SIPI (redundant on most modern hardware/hypervisors, but required by spec for
	//    older/pickier implementations).
	write_reg(REG_ICR_HIGH, dest);
	write_reg(REG_ICR_LOW, DELIVERY_MODE_STARTUP | (vector as u32));
	wait_for_icr_idle()?;

	time::time_manager().spin_for(Duration::from_micros(200));
	Ok(())
}

#[cfg(feature = "bsp_qemu_x86")]
pub const KERNEL_WAKE_VECTOR: u8 = 0xF0;
#[cfg(feature = "bsp_qemu_x86")]
pub const SPURIOUS_VECTOR: u8 = 0xFF;
#[cfg(feature = "bsp_qemu_x86")]
pub const NETWORK_TIMER_VECTOR: u8 = 0xF1;

#[cfg(feature = "bsp_qemu_x86")]
pub fn begin_timer_calibration() {
	unsafe {
		write_reg(0x320, (1 << 16) | NETWORK_TIMER_VECTOR as u32);
		write_reg(0x3E0, 3); // Divide by 16.
		write_reg(0x380, u32::MAX);
	}
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn finish_timer_calibration() -> u32 {
	unsafe {
		let elapsed = u32::MAX - read_reg(0x390);
		write_reg(0x380, 0);
		elapsed
	}
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn start_network_timer(ticks: u32) {
	assert!(ticks > 0);
	unsafe {
		write_reg(0x3E0, 3);
		write_reg(0x320, (1 << 17) | NETWORK_TIMER_VECTOR as u32);
		write_reg(0x380, ticks);
	}
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn stop_network_timer() {
	unsafe {
		write_reg(0x320, (1 << 16) | NETWORK_TIMER_VECTOR as u32);
		write_reg(0x380, 0);
	}
}

/// Initialize this CPU's APIC with only the fixed IPI path usable; CPU IRQs stay masked.
#[cfg(feature = "bsp_qemu_x86")]
pub fn init_local() -> Result<(), &'static str> {
	super::cpu::hardware_apic_id()?;
	if !crate::exception::asynchronous::is_local_irq_masked() {
		return Err("APIC initialization requires masked interrupts");
	}
	unsafe {
		let maximum_lvt = (read_reg(0x30) >> 16) & 0xFF;
		if maximum_lvt < 3 { return Err("Local APIC lacks required interrupt registers"); }
		for offset in [0x320, 0x350, 0x360, 0x370] { write_reg(offset, 1 << 16); }
		if maximum_lvt >= 4 { write_reg(0x340, 1 << 16); }
		if maximum_lvt >= 5 { write_reg(0x330, 1 << 16); }
		if maximum_lvt >= 6 { write_reg(0x2F0, 1 << 16); }
		write_reg(0x80, 0);
		write_reg(0xF0, (1 << 8) | SPURIOUS_VECTOR as u32);
	}
	Ok(())
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn init_kernel_worker() -> Result<(), &'static str> {
	if super::cpu::current_core_id().raw() != 0 {
		return Err("Kernel-worker APIC initialization requires CPU 0");
	}
	init_local()?;
	unsafe {
		core::arch::asm!("out dx, al", in("dx") 0x21u16, in("al") 0xFFu8,
			options(nomem, nostack, preserves_flags));
		core::arch::asm!("out dx, al", in("dx") 0xA1u16, in("al") 0xFFu8,
			options(nomem, nostack, preserves_flags));
	}
	Ok(())
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn notify_kernel(target: u8) -> Result<(), &'static str> {
	use crate::exception::asynchronous;
	super::cpu::hardware_apic_id()?;
	let saved = asynchronous::local_irq_mask_save();
	let result = unsafe {
		(|| {
			wait_for_icr_idle()?;
			write_reg(REG_ICR_HIGH, (target as u32) << 24);
			write_reg(REG_ICR_LOW, KERNEL_WAKE_VECTOR as u32);
			wait_for_icr_idle()
		})()
	};
	asynchronous::local_irq_restore(saved);
	result
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn end_of_interrupt() {
	unsafe { write_reg(0xB0, 0); }
}

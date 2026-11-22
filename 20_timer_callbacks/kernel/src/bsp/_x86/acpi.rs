// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Minimal ACPI table parsing (RSDP -> XSDT/RSDT -> MADT).
//!
//! Just enough to discover the Local APIC IDs of every CPU the firmware/hypervisor exposes, so
//! that real secondary-core bring-up (INIT-SIPI-SIPI) is possible. This is deliberately not a
//! general-purpose ACPI implementation — no AML, no other tables.

use core::sync::atomic::{AtomicUsize, Ordering};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Physical address of the RSDP, as handed to us by the shim. `0` means "unknown/not found".
static RSDP_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Root System Description Pointer (ACPI 2.0+, 36-byte form).
#[repr(C, packed)]
struct Rsdp {
	signature: [u8; 8],
	checksum: u8,
	oem_id: [u8; 6],
	revision: u8,
	rsdt_address: u32,
	length: u32,
	xsdt_address: u64,
	extended_checksum: u8,
	reserved: [u8; 3],
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Record the physical RSDP address supplied by the shim.
///
/// Must be called exactly once, before any other function in this module, from `_start()`.
pub fn set_rsdp_addr(addr: usize) {
	RSDP_ADDR.store(addr, Ordering::Relaxed);
}

/// Sum every byte in `[ptr, ptr+len)`; ACPI tables are valid iff this sum is `0`.
///
/// # Safety
///
/// - `[ptr, ptr+len)` must be readable memory.
unsafe fn checksum_ok(ptr: *const u8, len: usize) -> bool {
	let mut sum: u8 = 0;

	for i in 0..len {
		sum = sum.wrapping_add(*ptr.add(i));
	}

	sum == 0
}

/// Validate firmware tables, then pass each Local APIC ID and enabled state to `f`.
///
/// Returns `Err` if the RSDP was never set, is malformed, or no MADT could be found. Firmware
/// tables are trusted physical-memory structures read via direct pointer access, since paging is
/// still identity-mapped during this bring-up phase (see `bsp::memory::mmu`'s no-op translation
/// table).
///
/// # Safety
///
/// - Physical memory must currently be identity-mapped (true during this bring-up phase; will
///   need reconciling once real paging replaces the no-op translation table).
pub unsafe fn for_each_enabled_apic(
	f: impl FnMut(u8, bool) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
	let address = RSDP_ADDR.load(Ordering::Acquire);
	if address == 0 || address.checked_add(36).map_or(true, |end| end > 0x1_0000_0000) {
		return Err("ACPI RSDP outside identity-mapped memory");
	}
	let rsdp = core::ptr::read_unaligned(address as *const Rsdp);
	if &rsdp.signature != b"RSD PTR " || !checksum_ok(address as *const u8, 20) {
		return Err("Invalid RSDP signature or checksum");
	}
	let (root_address, width, signature) = if rsdp.revision >= 2 && rsdp.xsdt_address != 0 {
		let length = rsdp.length as usize;
		if !(36..=4096).contains(&length)
			|| address.checked_add(length).map_or(true, |end| end > 0x1_0000_0000)
			|| !checksum_ok(address as *const u8, length)
		{ return Err("Invalid extended RSDP"); }
		(rsdp.xsdt_address as usize, 8, b"XSDT")
	} else { (rsdp.rsdt_address as usize, 4, b"RSDT") };
	let root = table(root_address)?;
	if &root[..4] != signature || (root.len() - 36) % width != 0 {
		return Err("Invalid ACPI root table");
	}
	let mut madt = None;
	for entry in root[36..].chunks_exact(width) {
		let target = if width == 8 {
			core::ptr::read_unaligned(entry.as_ptr() as *const u64) as usize
		} else { core::ptr::read_unaligned(entry.as_ptr() as *const u32) as usize };
		let candidate = table(target)?;
		if &candidate[..4] == b"APIC" {
			if madt.is_some() { return Err("Multiple MADT tables"); }
			madt = Some(candidate);
		}
	}
	crate::cpu::topology::parse_madt(madt.ok_or("MADT not found")?, f)
}

// Firmware supplies trusted mapped pointers; lengths and checksums are still checked before parsing.
unsafe fn table(address: usize) -> Result<&'static [u8], &'static str> {
	if address == 0 || address.checked_add(36).map_or(true, |end| end > 0x1_0000_0000) {
		return Err("ACPI table outside identity-mapped memory");
	}
	let length = core::ptr::read_unaligned((address + 4) as *const u32) as usize;
	if !(36..=1024 * 1024).contains(&length)
		|| address.checked_add(length).map_or(true, |end| end > 0x1_0000_0000)
		|| !checksum_ok(address as *const u8, length)
	{ return Err("Invalid ACPI table length or checksum"); }
	Ok(core::slice::from_raw_parts(address as *const u8, length))
}

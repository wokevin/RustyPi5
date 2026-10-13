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

/// Common header present at the start of every ACPI system description table.
#[repr(C, packed)]
struct SdtHeader {
	signature: [u8; 4],
	length: u32,
	revision: u8,
	checksum: u8,
	oem_id: [u8; 6],
	oem_table_id: [u8; 8],
	oem_revision: u32,
	creator_id: u32,
	creator_revision: u32,
}

/// MADT (`APIC`) entry type 0: Processor Local APIC.
const MADT_ENTRY_LOCAL_APIC: u8 = 0;

/// A discovered CPU, as described by the MADT.
#[derive(Copy, Clone)]
pub struct MadtLocalApic {
	/// ACPI processor UID (not necessarily contiguous or APIC-ID-ordered).
	pub acpi_processor_id: u8,

	/// The value used as the `target_cpu`/destination in INIT-SIPI-SIPI.
	pub apic_id: u8,

	/// Whether the firmware reports this CPU as usable (enabled or online-capable).
	pub enabled: bool,
}

/// Maximum number of CPUs this bring-up phase supports discovering.
///
/// Arbitrary bound to avoid a heap dependency this early in boot; large enough for any
/// currently-realistic Hyper-V guest vCPU count.
const MAX_CPUS: usize = 64;

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

/// Parse the MADT and call `f` once for every Local APIC entry found.
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
pub unsafe fn for_each_local_apic(
	mut f: impl FnMut(MadtLocalApic),
) -> Result<(), &'static str> {
	let rsdp_addr = RSDP_ADDR.load(Ordering::Relaxed);
	if rsdp_addr == 0 {
		return Err("ACPI RSDP address was not provided by the shim");
	}

	let rsdp = &*(rsdp_addr as *const Rsdp);
	if &rsdp.signature != b"RSD PTR " {
		return Err("Invalid RSDP signature");
	}

	// Prefer the 64-bit XSDT (ACPI 2.0+); fall back to the 32-bit RSDT otherwise.
	let (sdt_addr, entry_size): (usize, usize) = if rsdp.revision >= 2 && rsdp.xsdt_address != 0 {
		(rsdp.xsdt_address as usize, 8)
	} else {
		(rsdp.rsdt_address as usize, 4)
	};

	let sdt_header = &*(sdt_addr as *const SdtHeader);
	if !checksum_ok(sdt_addr as *const u8, sdt_header.length as usize) {
		return Err("XSDT/RSDT checksum failed");
	}

	let entries_len = (sdt_header.length as usize) - core::mem::size_of::<SdtHeader>();
	let num_entries = entries_len / entry_size;
	let entries_ptr = (sdt_addr as *const u8).add(core::mem::size_of::<SdtHeader>());

	let mut madt_addr: Option<usize> = None;

	for i in 0..num_entries {
		let entry_addr = if entry_size == 8 {
			*(entries_ptr.add(i * 8) as *const u64) as usize
		} else {
			*(entries_ptr.add(i * 4) as *const u32) as usize
		};

		let header = &*(entry_addr as *const SdtHeader);
		if &header.signature == b"APIC" {
			madt_addr = Some(entry_addr);
			break;
		}
	}

	let madt_addr = madt_addr.ok_or("MADT (APIC) table not found")?;
	let madt_header = &*(madt_addr as *const SdtHeader);

	if !checksum_ok(madt_addr as *const u8, madt_header.length as usize) {
		return Err("MADT checksum failed");
	}

	// MADT-specific fixed fields immediately follow the common SDT header: local APIC address
	// (u32) and flags (u32), then a stream of variable-length entries.
	let madt_body = (madt_addr as *const u8).add(core::mem::size_of::<SdtHeader>());
	let variable_start = madt_body.add(8); // skip local_apic_address + flags
	let variable_len = (madt_header.length as usize) - core::mem::size_of::<SdtHeader>() - 8;

	let mut offset = 0usize;
	let mut count = 0usize;

	while offset + 2 <= variable_len && count < MAX_CPUS {
		let entry_type = *variable_start.add(offset);
		let entry_len = *variable_start.add(offset + 1) as usize;

		if entry_len < 2 {
			break; // malformed table; bail out rather than loop forever
		}

		if entry_type == MADT_ENTRY_LOCAL_APIC && entry_len >= 8 {
			let acpi_processor_id = *variable_start.add(offset + 2);
			let apic_id = *variable_start.add(offset + 3);
			let flags = *(variable_start.add(offset + 4) as *const u32);

			// Bit 0: Enabled. Bit 1: Online Capable (only meaningful if bit 0 is clear).
			let enabled = (flags & 0b01) != 0;
			let online_capable = (flags & 0b10) != 0;

			if enabled || online_capable {
				f(MadtLocalApic {
					acpi_processor_id,
					apic_id,
					enabled,
				});
				count += 1;
			}
		}

		offset += entry_len;
	}

	Ok(())
}

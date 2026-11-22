// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! BSP Memory Management Unit (Hyper-V x86_64 guest).
//!
//! UEFI firmware leaves paging enabled (identity-mapped) when we take over. This bring-up phase
//! uses dummy fixed regions and a no-op translation table, mirroring the host BSP, so that the
//! architecture-agnostic `crate::memory::mmu` code compiles and links. Real page-table management
//! (PML4/PDPT/PD/PT via CR3) will replace this once basic boot and console output are confirmed
//! working.

use crate::{
	memory::{
		mmu::{
			self as generic_mmu, translation_table::interface::TranslationTable, AttributeFields,
			MemoryRegion, PageAddress, TranslationGranule,
		},
		Address, Physical, Virtual,
	},
	synchronization::InitStateLock,
};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// A no-op translation table. Real page tables are not programmed yet during this bring-up phase.
pub struct HyperVTranslationTable;

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// The translation granule chosen for this BSP.
pub type KernelGranule = TranslationGranule<{ 4 * 1024 }>;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

static KERNEL_TABLES: InitStateLock<HyperVTranslationTable> =
	InitStateLock::new(HyperVTranslationTable);

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

impl TranslationTable for HyperVTranslationTable {
	fn init(&mut self) -> Result<(), &'static str> {
		Ok(())
	}

	unsafe fn map_at(
		&mut self,
		_virt_region: &MemoryRegion<Virtual>,
		_phys_region: &MemoryRegion<Physical>,
		_attr: &AttributeFields,
	) -> Result<(), &'static str> {
		Ok(())
	}

	fn try_virt_page_addr_to_phys_page_addr(
		&self,
		_virt_page_addr: PageAddress<Virtual>,
	) -> Result<PageAddress<Physical>, &'static str> {
		Err("Hyper-V x86_64 BSP does not implement real address translation yet")
	}

	fn try_page_attributes(
		&self,
		_virt_page_addr: PageAddress<Virtual>,
	) -> Result<AttributeFields, &'static str> {
		Err("Hyper-V x86_64 BSP does not implement real address translation yet")
	}

	fn try_virt_addr_to_phys_addr(
		&self,
		_virt_addr: Address<Virtual>,
	) -> Result<Address<Physical>, &'static str> {
		Err("Hyper-V x86_64 BSP does not implement real address translation yet")
	}
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// The code pages of the kernel binary.
///
/// No linker-defined layout is wired up yet; this is a dummy placeholder region.
pub fn virt_code_region() -> MemoryRegion<Virtual> {
	let start = Address::<Virtual>::new(0);
	let end_exclusive = Address::<Virtual>::new(KernelGranule::SIZE);

	MemoryRegion::new(
		PageAddress::from(start),
		PageAddress::from(end_exclusive),
	)
}

/// Linker/loader-reserved kernel-only heap, separate from user pages and all stacks.
pub fn virt_heap_region() -> MemoryRegion<Virtual> {
	let (first, end) = super::kernel_heap_bounds();
	let start = Address::<Virtual>::new(first);
	let end_exclusive = Address::<Virtual>::new(end);

	MemoryRegion::new(
		PageAddress::from(start),
		PageAddress::from(end_exclusive),
	)
}

/// The boot core stack pages.
///
/// Dummy placeholder region; the shim is expected to have already set up a valid stack.
pub fn virt_boot_core_stack_region() -> MemoryRegion<Virtual> {
	let start = Address::<Virtual>::new(KernelGranule::SIZE * 2);
	let end_exclusive = Address::<Virtual>::new(KernelGranule::SIZE * 3);

	MemoryRegion::new(
		PageAddress::from(start),
		PageAddress::from(end_exclusive),
	)
}

/// The MMIO remap pages.
///
/// Dummy placeholder region; no real MMIO remapping is wired up yet.
pub fn virt_mmio_remap_region() -> MemoryRegion<Virtual> {
	let start = Address::<Virtual>::new(KernelGranule::SIZE * 3);
	let end_exclusive = Address::<Virtual>::new(KernelGranule::SIZE * 4);

	MemoryRegion::new(
		PageAddress::from(start),
		PageAddress::from(end_exclusive),
	)
}

/// Return a reference to the (no-op) kernel translation table.
pub fn kernel_translation_tables() -> &'static InitStateLock<HyperVTranslationTable> {
	&KERNEL_TABLES
}

/// Add mapping records for the kernel binary.
///
/// No-op for now since there is no real translation table being populated.
pub fn kernel_add_mapping_records_for_precomputed() {
	let _ = generic_mmu::kernel_add_mapping_record;
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! BSP Memory Management Unit (host stub).
//!
//! Host/x86_64 builds are meant for algorithm testing only. There is no real hardware MMU being
//! programmed here; the OS already manages virtual memory for us. This module provides just enough
//! of the BSP MMU interface, backed by dummy fixed regions and a no-op translation table, so that
//! the architecture-agnostic `crate::memory::mmu` code continues to compile and link.

use crate::{
    memory::{
        mmu::{
            self as generic_mmu,
            translation_table::interface::TranslationTable,
            AttributeFields, MemoryRegion, PageAddress, TranslationGranule,
        },
        Address, Physical, Virtual,
    },
    synchronization::InitStateLock,
};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// A no-op translation table. Host builds never actually program hardware page tables.
pub struct HostTranslationTable;

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// The translation granule chosen for the host BSP.
pub type KernelGranule = TranslationGranule<{ 4 * 1024 }>;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

static KERNEL_TABLES: InitStateLock<HostTranslationTable> =
    InitStateLock::new(HostTranslationTable);

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

impl TranslationTable for HostTranslationTable {
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
        Err("Host BSP does not implement real address translation")
    }

    fn try_page_attributes(
        &self,
        _virt_page_addr: PageAddress<Virtual>,
    ) -> Result<AttributeFields, &'static str> {
        Err("Host BSP does not implement real address translation")
    }

    fn try_virt_addr_to_phys_addr(
        &self,
        _virt_addr: Address<Virtual>,
    ) -> Result<Address<Physical>, &'static str> {
        Err("Host BSP does not implement real address translation")
    }
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// The code pages of the kernel binary.
///
/// Host builds don't have a linker-defined layout, so this is a dummy placeholder region.
pub fn virt_code_region() -> MemoryRegion<Virtual> {
    let start = Address::<Virtual>::new(0);
    let end_exclusive = Address::<Virtual>::new(KernelGranule::SIZE);

    MemoryRegion::new(
        PageAddress::from(start),
        PageAddress::from(end_exclusive),
    )
}

/// The heap pages.
///
/// Host builds use the OS-managed heap (see `heap_alloc.rs`); this is a dummy placeholder region.
pub fn virt_heap_region() -> MemoryRegion<Virtual> {
    let start = Address::<Virtual>::new(KernelGranule::SIZE);
    let end_exclusive = Address::<Virtual>::new(KernelGranule::SIZE * 2);

    MemoryRegion::new(
        PageAddress::from(start),
        PageAddress::from(end_exclusive),
    )
}

/// The boot core stack pages.
///
/// Host builds run on the OS-managed stack; this is a dummy placeholder region.
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
/// Host builds have no MMIO to remap; this is a dummy placeholder region.
pub fn virt_mmio_remap_region() -> MemoryRegion<Virtual> {
    let start = Address::<Virtual>::new(KernelGranule::SIZE * 3);
    let end_exclusive = Address::<Virtual>::new(KernelGranule::SIZE * 4);

    MemoryRegion::new(
        PageAddress::from(start),
        PageAddress::from(end_exclusive),
    )
}

/// Return a reference to the (no-op) kernel translation table.
pub fn kernel_translation_tables() -> &'static InitStateLock<HostTranslationTable> {
    &KERNEL_TABLES
}

/// Add mapping records for the kernel binary.
///
/// No-op on the host BSP since there is no real translation table being populated.
pub fn kernel_add_mapping_records_for_precomputed() {
    let _ = generic_mmu::kernel_add_mapping_record;
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Memory Management Unit Driver.
//!
//! `bsp_host` builds run under an OS that already manages virtual memory, so there is no real
//! hardware MMU to program. `bsp_hyperv_x86` is bare-metal, but UEFI firmware already leaves
//! paging enabled (identity-mapped) when we take over — so for this bring-up phase, this remains
//! a no-op for both targets. Real page-table management (PML4/PDPT/PD/PT via CR3) will replace
//! this once basic boot is confirmed working.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::memory::mmu::arch_mmu

use crate::memory::{self, mmu::MMUEnableError, Address, Physical};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Memory Management Unit type.
struct MemoryManagementUnit;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

static MMU: MemoryManagementUnit = MemoryManagementUnit;

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

impl<const AS_SIZE: usize> memory::mmu::AddressSpace<AS_SIZE> {
    /// Checks for architectural restrictions.
    ///
    /// Neither x86_64 target emulates a real hardware address-space limit yet, so this is a
    /// no-op.
    pub const fn arch_address_space_size_sanity_checks() {}
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return a reference to the MMU instance.
pub fn mmu() -> &'static impl memory::mmu::interface::MMU {
    &MMU
}

//------------------------------------------------------------------------------
// OS Interface Code
//------------------------------------------------------------------------------

impl memory::mmu::interface::MMU for MemoryManagementUnit {
    unsafe fn enable_mmu_and_caching(
        &self,
        _phys_tables_base_addr: Address<Physical>,
    ) -> Result<(), MMUEnableError> {
        // No-op: the host OS (bsp_host) or UEFI firmware (bsp_hyperv_x86) already provides
        // active virtual memory/paging.
        Ok(())
    }

    #[inline(always)]
    fn is_enabled(&self) -> bool {
        // Report as always enabled for both targets.
        true
    }
}

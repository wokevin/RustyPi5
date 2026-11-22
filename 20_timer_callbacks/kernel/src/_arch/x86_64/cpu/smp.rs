// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural symmetric multiprocessing.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::cpu::smp::arch_smp

use crate::cpu::smp::CpuId;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return the executing core's logical discovery index, not its hardware APIC ID.
#[inline(always)]
pub fn core_id() -> CpuId {
    #[cfg(feature = "bsp_host")]
    { CpuId::new(0) }
    #[cfg(not(feature = "bsp_host"))]
    { crate::bsp::cpu::current_core_id() }
}

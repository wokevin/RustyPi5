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

/// Return the executing core's id.
///
/// Host/x86_64 builds are single-core for algorithm-testing purposes, so this always returns a
/// fixed id representing "core 0".
#[inline(always)]
pub fn core_id() -> CpuId {
    CpuId::new(0)
}

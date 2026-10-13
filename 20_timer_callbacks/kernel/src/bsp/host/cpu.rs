// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Host BSP processor code stub.

use crate::cpu::smp::{Cpu, CpuId, SmpReleaseError, SmpReleaseMethod};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Host builds are single-core (real threads are used instead of SMP), so this release method is
/// never expected to be invoked; it exists only to satisfy `bsp::cpu::smp_release_method()`.
struct HostSmpRelease;

impl SmpReleaseMethod for HostSmpRelease {
	unsafe fn release_core(
		&self,
		_id: CpuId,
		_entry_point: usize,
		_stack_top: usize,
	) -> Result<(), SmpReleaseError> {
		Err(SmpReleaseError::MechanismFailed(
			"Host builds are single-core; secondary core release is not supported",
		))
	}
}

static HOST_SMP_RELEASE: HostSmpRelease = HostSmpRelease;

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Host builds only ever have a single (boot) core.
pub static CORES: [Cpu; 1] = [Cpu::new(CpuId::new(0), true)];

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return the board's core table (boot core included).
pub fn cores() -> &'static [Cpu] {
    &CORES
}

/// Return this board's secondary-core release mechanism.
pub fn smp_release_method() -> &'static dyn SmpReleaseMethod {
    &HOST_SMP_RELEASE
}

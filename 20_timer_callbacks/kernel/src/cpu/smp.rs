// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Symmetric multiprocessing.

#[cfg(target_arch = "aarch64")]
#[path = "../_arch/aarch64/cpu/smp.rs"]
mod arch_smp;

#[cfg(target_arch = "x86_64")]
#[path = "../_arch/x86_64/cpu/smp.rs"]
mod arch_smp;

use crate::bsp;
use core::sync::atomic::{AtomicBool, Ordering};

//--------------------------------------------------------------------------------------------------
// Architectural Public Reexports
//--------------------------------------------------------------------------------------------------
pub use arch_smp::core_id;

#[cfg(target_arch = "aarch64")]
pub use arch_smp::{PsciConduit, PsciRelease, SpinTableRelease};

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Errors that can occur while releasing a secondary core.
#[derive(Debug)]
pub enum SmpReleaseError {
    /// The underlying release mechanism reported failure.
    MechanismFailed(&'static str),

    /// The requested core ID is out of range for this board.
    InvalidCoreId,
}

/// Identifies a physical core.
///
/// Wraps the raw, masked `MPIDR_EL1` affinity value (or an architecture-appropriate equivalent),
/// rather than a flat sequential index. This matters because PSCI's `CPU_ON` call expects the raw
/// affinity encoding as its `target_cpu` argument, and that encoding is not guaranteed to be a
/// simple `0..N` range on boards with multiple clusters/sockets.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct CpuId(u64);

impl CpuId {
    /// Create an instance from a raw, already-masked affinity value.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw affinity value.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Per-core state.
pub struct Cpu {
    id: CpuId,
    online: AtomicBool,
}

impl Cpu {
    /// Create an instance.
    ///
    /// `is_boot_core` marks the core that is already running at the time this array is
    /// initialized (it never goes through the release protocol).
    pub const fn new(id: CpuId, is_boot_core: bool) -> Self {
        Self {
            id,
            online: AtomicBool::new(is_boot_core),
        }
    }

    /// Return this core's id.
    pub const fn id(&self) -> CpuId {
        self.id
    }

    /// Return whether this core is known to be online.
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Acquire)
    }

    /// Mark this core as online.
    ///
    /// Called exactly once by each secondary core, immediately after it starts executing Rust
    /// code at its designated entry point.
    pub fn mark_online(&self) {
        self.online.store(true, Ordering::Release);
    }
}

/// A pluggable mechanism for releasing secondary cores out of their reset/parked state.
///
/// Different boards (and even different firmware versions of the same board) use different
/// conventions for this. Two are implemented architecturally (see
/// `crate::_arch::aarch64::cpu::smp`):
///
/// - PSCI `CPU_ON` (the modern, DT/ACPI-standard mechanism).
/// - Spin-tables (the classic Raspberry Pi firmware mechanism).
///
/// The BSP selects which one applies to a given board.
pub trait SmpReleaseMethod {
    /// Release the core identified by `id`, pointing it at `entry_point` with its stack pointer
    /// initialized to `stack_top`.
    ///
    /// # Safety
    ///
    /// - `entry_point` must be a valid function pointer that never returns.
    /// - `stack_top` must point to the exclusive end of a stack region reserved solely for this
    ///   core; it must not overlap any other core's stack or memory region.
    unsafe fn release_core(
        &self,
        id: CpuId,
        entry_point: usize,
        stack_top: usize,
    ) -> Result<(), SmpReleaseError>;
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return the full, board-defined array of cores (boot core included).
///
/// The BSP is responsible for populating this with the correct topology/affinity values and for
/// marking exactly one entry as the boot core.
pub fn cores() -> &'static [Cpu] {
    bsp::cpu::cores()
}

/// Bring up every non-boot core known to this board.
///
/// For each core not already marked online, invokes the board's release method, then polls that
/// core's `online` flag with a bounded timeout. Cores that don't respond in time are left offline
/// and do not stop the boot process — this is a discovery mechanism, not an assumption of a fixed
/// core count.
///
/// Returns the number of cores that came online (not including the boot core).
///
/// # Safety
///
/// - `entry_point` must be a valid function pointer that never returns, expecting to run on a
///   freshly released secondary core.
/// - `stack_top_for_core` must return a unique, valid, exclusive-end stack address for each core
///   id it is called with.
pub unsafe fn bring_up_all_cores(
    entry_point: usize,
    stack_top_for_core: impl Fn(CpuId) -> usize,
) -> usize {
    const POLL_ATTEMPTS: usize = 100_000;

    let method = bsp::cpu::smp_release_method();
    let mut brought_up = 0;

    for cpu in cores() {
        if cpu.is_online() {
            continue;
        }

        let stack_top = stack_top_for_core(cpu.id());

        if method.release_core(cpu.id(), entry_point, stack_top).is_err() {
            continue;
        }

        for _ in 0..POLL_ATTEMPTS {
            if cpu.is_online() {
                break;
            }

            core::hint::spin_loop();
        }

        if cpu.is_online() {
            brought_up += 1;
        }
    }

    brought_up
}


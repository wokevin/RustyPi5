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

use crate::cpu::smp::{CpuId, SmpReleaseError, SmpReleaseMethod};
use aarch64_cpu::{asm, registers::*};
use core::arch::asm;
use tock_registers::interfaces::Readable;

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// PSCI function ID for `CPU_ON` (SMC64 calling convention).
///
/// # Resources
///
/// - <https://developer.arm.com/documentation/den0022/latest/> (Arm Power State Coordination
///   Interface specification).
const PSCI_CPU_ON_64: u64 = 0xc400_0003;

/// Mask covering the `MPIDR_EL1` affinity fields (`Aff0`, `Aff1`, `Aff2`, `Aff3`), with the
/// reserved/non-affinity bits (`[63:40]`, `[31:24]`) cleared. This is the value PSCI's `CPU_ON`
/// expects as `target_cpu`, and what we use as our `CpuId`'s raw representation.
const MPIDR_AFFINITY_MASK: u64 = 0x00ff_00ff_ffff;

/// The conduit used to invoke PSCI functions.
///
/// Firmware advertises via the device tree's `psci` node `method` property whether `hvc` or `smc`
/// should be used. `Hvc` is the common case when a hypervisor/EL2 firmware layer is present (as on
/// Raspberry Pi 4/5 with the Videocore/ARM Trusted Firmware stack); `Smc` traps directly to EL3.
///
/// TODO_VERIFY: confirm the conduit advertised by the Pi 5 device tree (`bcm2712-rpi-5-b.dts`,
/// `psci { method = "..."; }`) before relying on this on real hardware.
#[derive(Copy, Clone)]
pub enum PsciConduit {
    /// Use the `hvc` instruction.
    Hvc,

    /// Use the `smc` instruction.
    Smc,
}

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Releases secondary cores via the PSCI `CPU_ON` call.
///
/// This is the modern, DT/ACPI-standard mechanism, implemented by an EL3 firmware/monitor (or a
/// hypervisor emulating one).
pub struct PsciRelease {
    conduit: PsciConduit,
}

/// Releases secondary cores via the classic Raspberry Pi firmware spin-table mechanism.
///
/// Each secondary core spins in ROM/firmware, polling a fixed physical "mailbox" address. Writing
/// the desired entry point address to that mailbox and issuing `sev` releases the core.
///
/// # Resources
///
/// - <https://github.com/raspberrypi/tools/blob/master/armstubs/armstub8.S> (legacy armstub spin
///   loop implementation).
pub struct SpinTableRelease {
    /// Kernel-virtual addresses of the per-core mailboxes, indexed by the core's `Aff0` value
    /// (core 0's boot core entry is unused/`None`, since it never needs to be released via
    /// spin-table).
    ///
    /// These must already be mapped into the kernel's translation tables — see
    /// `bsp::cpu::init_smp_mailboxes()`. Writing through raw, unmapped *physical* addresses after
    /// the MMU is enabled will cause a Data Abort.
    ///
    /// Spin-tables are a Raspberry Pi (single-cluster) mechanism, so only `Aff0` is meaningful
    /// here; `Aff1..3` are assumed to be 0 for all cores using this release method.
    mailboxes: &'static [Option<*mut u64>],
}

// Safety: the mailbox pointers refer to fixed physical MMIO/memory addresses that are valid for
// the lifetime of the program and are only ever written once per core.
unsafe impl Sync for SpinTableRelease {}

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

/// Issue a PSCI `CPU_ON` call using the SMC64 calling convention.
///
/// # Safety
///
/// - Caller must ensure `entry_point` and `context_id` are meaningful to the code that will run on
///   the released core.
#[inline(always)]
unsafe fn psci_cpu_on(conduit: PsciConduit, target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let function_id: u64 = PSCI_CPU_ON_64;
    let result: i64;

    match conduit {
        PsciConduit::Hvc => {
            asm!(
                "hvc #0",
                inout("x0") function_id => result,
                in("x1") target_cpu,
                in("x2") entry_point,
                in("x3") context_id,
                options(nomem, nostack)
            );
        }
        PsciConduit::Smc => {
            asm!(
                "smc #0",
                inout("x0") function_id => result,
                in("x1") target_cpu,
                in("x2") entry_point,
                in("x3") context_id,
                options(nomem, nostack)
            );
        }
    }

    result
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return the executing core's id, as the masked `MPIDR_EL1` affinity value.
#[inline(always)]
pub fn core_id() -> CpuId {
    CpuId::new(MPIDR_EL1.get() & MPIDR_AFFINITY_MASK)
}

impl PsciRelease {
    /// Create an instance.
    pub const fn new(conduit: PsciConduit) -> Self {
        Self { conduit }
    }
}

impl SmpReleaseMethod for PsciRelease {
    unsafe fn release_core(
        &self,
        id: CpuId,
        entry_point: usize,
        _stack_top: usize,
    ) -> Result<(), SmpReleaseError> {
        // Note: PSCI CPU_ON does not take a stack pointer argument. The entry point itself is
        // responsible for setting up its own stack as its very first action (see the secondary
        // core boot assembly), using its own core id to look up which stack region belongs to it.
        let result = psci_cpu_on(self.conduit, id.raw(), entry_point as u64, 0);

        if result != 0 {
            return Err(SmpReleaseError::MechanismFailed(
                "PSCI CPU_ON returned an error",
            ));
        }

        Ok(())
    }
}

impl SpinTableRelease {
    /// Create an instance.
    ///
    /// # Safety
    ///
    /// - `mailboxes` must contain valid, distinct physical addresses of the firmware-provided
    ///   spin-table mailboxes for this board, indexed by `Aff0`.
    pub const unsafe fn new(mailboxes: &'static [Option<*mut u64>]) -> Self {
        Self { mailboxes }
    }
}

impl SmpReleaseMethod for SpinTableRelease {
    unsafe fn release_core(
        &self,
        id: CpuId,
        entry_point: usize,
        _stack_top: usize,
    ) -> Result<(), SmpReleaseError> {
        // Spin-tables only understand Aff0 (single-cluster mechanism).
        let aff0 = id.raw() & 0b11;

        let mailbox = *self
            .mailboxes
            .get(aff0 as usize)
            .ok_or(SmpReleaseError::InvalidCoreId)?;

        let mailbox = mailbox.ok_or(SmpReleaseError::InvalidCoreId)?;

        core::ptr::write_volatile(mailbox, entry_point as u64);

        // Ensure the write is visible to the other core before waking it.
        asm::barrier::dsb(asm::barrier::SY);
        asm::sev();

        Ok(())
    }
}

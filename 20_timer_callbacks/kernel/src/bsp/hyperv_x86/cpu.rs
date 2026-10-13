// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Hyper-V x86_64 guest CPU bring-up.
//!
//! Secondary-core discovery (ACPI MADT) and release (INIT-SIPI-SIPI via a 16-bit real-mode
//! trampoline, see `super::trampoline`) are both fully wired up.

use crate::cpu::smp::{Cpu, CpuId, SmpReleaseError, SmpReleaseMethod};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Maximum number of cores this BSP is prepared to track.
///
/// Matches `super::acpi::MAX_CPUS`'s discovery bound; a fixed-size array is used instead of a
/// heap allocation because core discovery happens before the heap allocator is guaranteed to be
/// initialized.
///
/// Visible to `super::memory`, which derives its secondary-core stack reservation count from this
/// value to avoid the two constants silently drifting apart.
pub(super) const MAX_CORES: usize = 64;

/// Backing storage for the discovered core table. Every entry starts as core 0 / "boot core";
/// `discover_cores()` overwrites the live prefix with real data.
static mut CORE_STORAGE: [Cpu; MAX_CORES] = {
    // `Cpu::new` is `const fn`, so this repeat-array trick is valid; every slot is distinct
    // (non-`Copy`) storage, not aliased.
    const INIT: Cpu = Cpu::new(CpuId::new(0), false);
    let mut arr = [INIT; MAX_CORES];
    arr[0] = Cpu::new(CpuId::new(0), true); // core 0 is always the boot core
    arr
};

/// Number of valid entries at the front of `CORE_STORAGE`. Starts at `1` (boot core only) until
/// `discover_cores()` runs.
static NUM_CORES: AtomicUsize = AtomicUsize::new(1);

/// Local APIC ID of the boot core, as reported by ACPI. Needed to distinguish "self" from targets
/// during discovery. Defaults to `0`, the near-universal convention, until overwritten.
static BOOT_CORE_APIC_ID: AtomicU8 = AtomicU8::new(0);

/// Real secondary-core release, via the Local APIC's INIT-SIPI-SIPI sequence.
struct HyperVSmpRelease;

impl SmpReleaseMethod for HyperVSmpRelease {
    unsafe fn release_core(
        &self,
        id: CpuId,
        entry_point: usize,
        stack_top: usize,
    ) -> Result<(), SmpReleaseError> {
        let apic_id = apic_id_for(id).ok_or(SmpReleaseError::InvalidCoreId)?;

        super::trampoline::release_via_trampoline(apic_id, entry_point, stack_top)
            .map_err(SmpReleaseError::MechanismFailed)?;

        Ok(())
    }
}

static HYPERV_SMP_RELEASE: HyperVSmpRelease = HyperVSmpRelease;

/// Find the Local APIC ID associated with a discovered `CpuId`, if any.
///
/// `CpuId`'s raw value is the flat discovery index (0 = boot core), *not* the APIC ID itself,
/// mirroring how the Pi BSP's `CpuId` wraps a raw affinity value rather than an arbitrary index.
/// The APIC ID (the value INIT-SIPI-SIPI actually needs) is looked up separately here because
/// it's not necessarily equal to the flat index on real/virtualized hardware.
fn apic_id_for(id: CpuId) -> Option<u8> {
    let index = id.raw() as usize;
    if index >= NUM_CORES.load(Ordering::Acquire) {
        return None;
    }

    // SAFETY: `index` is within the initialized prefix per the check above, and this table is
    // only mutated during single-core discovery before any secondary core is released.
    unsafe { APIC_IDS[index] }.into()
}

/// Parallel array to `CORE_STORAGE`, holding each discovered core's real Local APIC ID (`Cpu`
/// itself only knows about the flat `CpuId` index, not the hardware APIC ID).
static mut APIC_IDS: [u8; MAX_CORES] = [0; MAX_CORES];

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return the board's core table (boot core included).
///
/// Before `discover_cores()` runs, this is a single-entry slice containing only the boot core.
pub fn cores() -> &'static [Cpu] {
    // SAFETY: `CORE_STORAGE`'s live prefix (`[0, NUM_CORES)`) is only written by
    // `discover_cores()`, which the BSP driver init path calls exactly once, on the boot core,
    // before any other core observes this slice.
    unsafe { &CORE_STORAGE[..NUM_CORES.load(Ordering::Acquire)] }
}

/// Alias kept for the `bsp::cpu::CORES`-shaped contract used elsewhere; prefer `cores()`.
///
/// # Note
///
/// Unlike the Raspberry Pi BSP, this can't be a `'static` fixed-size array constant, since the
/// real core count isn't known until ACPI is parsed at runtime. Callers that previously did
/// `bsp::cpu::CORES.iter()` should use `bsp::cpu::cores().iter()` instead.
pub fn core_count() -> usize {
    NUM_CORES.load(Ordering::Acquire)
}

/// Parse the ACPI MADT and populate the core table with every enabled/online-capable CPU it
/// reports.
///
/// Must be called exactly once, early during boot-core init, after `bsp::acpi::set_rsdp_addr()`
/// has run (i.e. after `_start()`) and before `cpu::smp::bring_up_all_cores()` is called.
///
/// # Safety
///
/// - Must only be called once, by the boot core, before any secondary core is released.
/// - Physical memory must currently be identity-mapped (true during this bring-up phase).
pub unsafe fn discover_cores() -> Result<(), &'static str> {
    let mut index = 0usize;

    super::acpi::for_each_local_apic(|local_apic| {
        if index >= MAX_CORES {
            return; // silently drop entries past our fixed-size bound
        }

        let is_boot_core = local_apic.apic_id == BOOT_CORE_APIC_ID.load(Ordering::Relaxed);

        CORE_STORAGE[index] = Cpu::new(CpuId::new(index as u64), is_boot_core);
        APIC_IDS[index] = local_apic.apic_id;
        index += 1;
    })?;

    if index == 0 {
        return Err("ACPI MADT reported zero usable Local APICs");
    }

    NUM_CORES.store(index, Ordering::Release);

    Ok(())
}

/// Return this board's secondary-core release mechanism.
pub fn smp_release_method() -> &'static dyn SmpReleaseMethod {
    &HYPERV_SMP_RELEASE
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! BSP Processor code.

#[cfg(all(
    target_arch = "aarch64",
    any(feature = "bsp_rpi5", all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), feature = "qemu"))
))]
use crate::cpu::smp::{PsciConduit, PsciRelease};

#[cfg(all(
    target_arch = "aarch64",
    any(feature = "bsp_rpi3", feature = "bsp_rpi4"),
    not(feature = "qemu")
))]
use crate::cpu::smp::SpinTableRelease;

use crate::cpu::smp::{Cpu, CpuId, SmpReleaseMethod};

#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
use crate::memory::{mmu::MMIODescriptor, Address, Physical};
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
use core::{mem::MaybeUninit, sync::atomic::{AtomicBool, Ordering}};

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Used by `arch` code to find the early boot core.
#[no_mangle]
#[link_section = ".text._start_arguments"]
pub static BOOT_CORE_ID: u64 = 0;

/// The maximum number of cores this board's static structures are sized for.
///
/// This is a compile-time ceiling used only for reserving stack memory in the linker script; it is
/// not an assumption about how many cores actually exist or respond at boot. All Raspberry Pi
/// models (3, 4, 5) to date have 4 cores in a single cluster, so a small ceiling above that is used
/// to leave headroom without wasting excessive memory.
pub const MAX_SUPPORTED_CORES: usize = 8;

/// The board's static core topology table.
///
/// Raspberry Pi boards (3, 4, 5) are single-cluster, so only `Aff0` varies; entries beyond the
/// board's actual core count are harmless — `bring_up_all_cores()` will simply fail to bring them
/// online and skip them, since they don't physically exist.
pub static CORES: [Cpu; MAX_SUPPORTED_CORES] = [
    Cpu::new(CpuId::new(0), true), // Boot core.
    Cpu::new(CpuId::new(1), false),
    Cpu::new(CpuId::new(2), false),
    Cpu::new(CpuId::new(3), false),
    Cpu::new(CpuId::new(4), false),
    Cpu::new(CpuId::new(5), false),
    Cpu::new(CpuId::new(6), false),
    Cpu::new(CpuId::new(7), false),
];

/// Return the board's core table (boot core included).
pub fn cores() -> &'static [Cpu] {
    &CORES
}

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Spin-table mailbox *physical* addresses, as defined by the classic Raspberry Pi armstub
/// firmware. Used by the Pi 3 and Pi 4 on real hardware (legacy, non-PSCI firmware). Indexed by
/// `Aff0`.
///
/// These must be mapped into the kernel's virtual address space before use — see
/// `init_smp_mailboxes()`. Writing through the raw physical addresses directly, once the MMU is
/// enabled, will fault.
///
/// Not used when building for QEMU (`qemu` feature): QEMU's `-kernel` direct-boot path always
/// installs a minimal PSCI firmware stub for secondary CPUs instead of emulating the real
/// firmware's spin-table polling loop, so writes to these mailbox addresses are never observed by
/// anything and no secondary core would ever wake up.
///
/// # Resources
///
/// - <https://github.com/raspberrypi/tools/blob/master/armstubs/armstub8.S>
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
const SPIN_TABLE_MAILBOXES_PHYS: [Option<Address<Physical>>; 4] = [
    None,                                    // Core 0 (boot core) is never released via spin-table.
    Some(Address::<Physical>::new(0xE0)),    // Core 1
    Some(Address::<Physical>::new(0xE8)),    // Core 2
    Some(Address::<Physical>::new(0xF0)),    // Core 3
];

/// Virtual (mapped) mailbox pointers, populated by `init_smp_mailboxes()` prior to any core
/// release attempt.
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
static mut SPIN_TABLE_MAILBOXES_VIRT: [Option<*mut u64>; 4] = [None, None, None, None];

#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
static SPIN_TABLE_RELEASE: MaybeUninit<SpinTableRelease> = MaybeUninit::uninit();

#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
static SPIN_TABLE_MAILBOXES_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// TODO_VERIFY: confirm the PSCI conduit (`hvc` vs `smc`) advertised by the Pi 5 device tree
/// (`bcm2712-rpi-5-b.dts`, `psci { method = "..."; }`) before relying on this on real hardware.
#[cfg(feature = "bsp_rpi5")]
static PSCI_RELEASE: PsciRelease = PsciRelease::new(PsciConduit::Hvc);

/// QEMU's `-kernel` direct-boot path always installs a minimal PSCI firmware stub for secondary
/// CPUs (regardless of machine model), reachable only via the `hvc` conduit (it is emulated as an
/// EL2 trap, not a full EL3 monitor) — so QEMU builds must use PSCI/Hvc even though real Pi 3/4
/// hardware uses the classic spin-table mechanism instead.
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), feature = "qemu"))]
static PSCI_RELEASE: PsciRelease = PsciRelease::new(PsciConduit::Hvc);

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Map each spin-table mailbox's physical address into the kernel's virtual address space.
///
/// Must be called exactly once, after `memory::mmu::kernel_init_mmio_va_allocator()` has run and
/// before any call to `smp_release_method()` / `bring_up_all_cores()`.
///
/// Not present on QEMU builds, since PSCI is used there instead.
///
/// # Safety
///
/// - Must only be called once, on the boot core, during single-core init.
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
pub unsafe fn init_smp_mailboxes() -> Result<(), &'static str> {
    use crate::memory::mmu::kernel_map_mmio;

    if SPIN_TABLE_MAILBOXES_INIT_DONE.load(Ordering::Relaxed) {
        return Err("SMP mailboxes already initialized");
    }

    for (i, phys) in SPIN_TABLE_MAILBOXES_PHYS.iter().enumerate() {
        if let Some(phys_addr) = phys {
            let mmio_descriptor = MMIODescriptor::new(*phys_addr, core::mem::size_of::<u64>());
            let virt_addr = kernel_map_mmio("SMP spin-table mailbox", &mmio_descriptor)?;

            SPIN_TABLE_MAILBOXES_VIRT[i] = Some(virt_addr.as_usize() as *mut u64);
        }
    }

    SPIN_TABLE_RELEASE
        .as_ptr()
        .cast_mut()
        .write(SpinTableRelease::new(&SPIN_TABLE_MAILBOXES_VIRT));

    SPIN_TABLE_MAILBOXES_INIT_DONE.store(true, Ordering::Release);

    Ok(())
}

/// Return this board's secondary-core release mechanism.
///
/// # Safety
///
/// - On non-QEMU Pi 3/4 builds, `init_smp_mailboxes()` must have been called first.
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), not(feature = "qemu")))]
pub unsafe fn smp_release_method() -> &'static dyn SmpReleaseMethod {
    &*SPIN_TABLE_RELEASE.as_ptr()
}

/// Return this board's secondary-core release mechanism (PSCI, used for QEMU Pi 3/4 builds).
#[cfg(all(any(feature = "bsp_rpi3", feature = "bsp_rpi4"), feature = "qemu"))]
pub fn smp_release_method() -> &'static dyn SmpReleaseMethod {
    &PSCI_RELEASE
}

/// Return this board's secondary-core release mechanism.
#[cfg(feature = "bsp_rpi5")]
pub fn smp_release_method() -> &'static dyn SmpReleaseMethod {
    &PSCI_RELEASE
}

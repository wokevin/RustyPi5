// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Hyper-V x86_64 guest memory map.

pub mod mmu;

use crate::memory::{Address, Virtual};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Size of each secondary core's reserved stack.
///
/// Matches the Raspberry Pi BSP's `SECONDARY_CORE_STACK_SIZE` convention.
const SECONDARY_CORE_STACK_SIZE: usize = 128 * 1024;

/// Maximum number of secondary cores this BSP reserves stack space for.
///
/// Derived from `super::cpu::MAX_CORES` (minus the boot core) so the two constants can never
/// silently drift apart.
const MAX_SECONDARY_CORES: usize = super::cpu::MAX_CORES - 1;

/// Statically-reserved stack storage for every possible secondary core.
///
/// No heap allocator or linker-script-reserved region exists yet for this bring-up phase (unlike
/// the Raspberry Pi BSP's `__secondary_core_stacks_start`/`_end_exclusive` linker symbols), so a
/// fixed-size `.bss` array is used instead. At `SECONDARY_CORE_STACK_SIZE` (128 KiB) per core and
/// `MAX_SECONDARY_CORES` (63) cores, this reserves 8 MiB of `.bss` — acceptable for a bring-up
/// phase, but should be replaced with on-demand physical allocation once a real physical frame
/// allocator exists.
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
struct CoreStack([u8; SECONDARY_CORE_STACK_SIZE]);
static mut SECONDARY_CORE_STACKS: [CoreStack; MAX_SECONDARY_CORES] =
    [CoreStack([0; SECONDARY_CORE_STACK_SIZE]); MAX_SECONDARY_CORES];

#[repr(C, align(4096))]
struct KernelHeap([u8; 1024 * 1024]);
static mut KERNEL_HEAP: KernelHeap = KernelHeap([0; 1024 * 1024]);

pub(super) fn kernel_heap_bounds() -> (usize, usize) {
    let start = unsafe { core::ptr::addr_of_mut!(KERNEL_HEAP) as usize };
    (start, start + core::mem::size_of::<KernelHeap>())
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return the exclusive-end stack address (i.e., the initial stack pointer value) for the given
/// secondary core.
///
/// `id`'s raw value is the flat discovery index assigned by `bsp::hyperv_x86::cpu::discover_cores`
/// (0 = boot core; never passed here — see that function's caller, `cpu::smp::bring_up_all_cores`,
/// which skips any core already marked online).
///
/// # Panics
///
/// - Panics if `id` is out of range for the reserved secondary-core stacks storage.
#[inline(always)]
pub fn secondary_core_stack_top_exclusive(id: crate::cpu::smp::CpuId) -> Address<Virtual> {
    let raw = id.raw();

    assert!(raw >= 1, "Core 0 uses the dedicated boot core stack");

    let index = (raw - 1) as usize;

    assert!(
        index < MAX_SECONDARY_CORES,
        "core id out of range for reserved secondary core stacks"
    );

    let stack = unsafe { core::ptr::addr_of_mut!(SECONDARY_CORE_STACKS[index]) };
    Address::<Virtual>::new(stack as usize + SECONDARY_CORE_STACK_SIZE)
}

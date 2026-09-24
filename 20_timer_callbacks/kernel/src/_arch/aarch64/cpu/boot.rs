// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural boot code.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::cpu::boot::arch_boot

use crate::{memory, memory::Address};
use aarch64_cpu::{asm, registers::*};
use core::{
    arch::global_asm,
    sync::atomic::{compiler_fence, Ordering},
};
use tock_registers::interfaces::Writeable;

// Assembly counterpart to this file.
global_asm!(
    include_str!("boot.s"),
    CONST_CURRENTEL_EL2 = const 0x8,
    CONST_CORE_ID_MASK = const 0b11,
    CONST_SECONDARY_CORE_STACK_SIZE = const 128 * 1024
);

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

/// Prepares the transition from EL2 to EL1.
///
/// # Safety
///
/// - The `bss` section is not initialized yet. The code must not use or reference it in any way.
/// - The HW state of EL1 must be prepared in a sound way.
#[inline(always)]
unsafe fn prepare_el2_to_el1_transition(
    virt_stack_end_exclusive_addr: u64,
    virt_entry_addr: u64,
) {
    // Enable timer counter registers for EL1.
    CNTHCTL_EL2.write(CNTHCTL_EL2::EL1PCEN::SET + CNTHCTL_EL2::EL1PCTEN::SET);

    // No offset for reading the counters.
    CNTVOFF_EL2.set(0);

    // Set EL1 execution state to AArch64.
    HCR_EL2.write(HCR_EL2::RW::EL1IsAarch64);

    // Set up a simulated exception return.
    //
    // First, fake a saved program status where all interrupts were masked and SP_EL1 was used as a
    // stack pointer.
    SPSR_EL2.write(
        SPSR_EL2::D::Masked
            + SPSR_EL2::A::Masked
            + SPSR_EL2::I::Masked
            + SPSR_EL2::F::Masked
            + SPSR_EL2::M::EL1h,
    );

    // Second, let the link register point to the desired entry point.
    ELR_EL2.set(virt_entry_addr);

    // Set up SP_EL1 (stack pointer), which will be used by EL1 once we "return" to it. Since there
    // are no plans to ever return to EL2, just re-use the same stack.
    SP_EL1.set(virt_stack_end_exclusive_addr);
}

/// Reset the backtrace by setting link register and frame pointer to zero.
///
/// # Safety
///
/// - This function must only be used immediately before entering EL1.
#[inline(always)]
unsafe fn prepare_backtrace_reset() {
    compiler_fence(Ordering::SeqCst);
    FP.set(0);
    LR.set(0);
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// The Rust entry of the `kernel` binary, for the boot core.
///
/// The function is called from the assembly `_start` function.
///
/// # Safety
///
/// - Exception return from EL2 must must continue execution in EL1 with `kernel_init()`.
#[no_mangle]
pub unsafe extern "C" fn _start_rust(
    phys_kernel_tables_base_addr: u64,
    virt_boot_core_stack_end_exclusive_addr: u64,
    virt_kernel_init_addr: u64,
) -> ! {
    // TEMP: raw UART sanity print, MMU is still off here, physical addressing is valid.
    let uart_dr = 0x3F20_1000 as *mut u8;
    for &b in b"START_RUST REACHED\r\n" {
        core::ptr::write_volatile(uart_dr, b);
    }

    prepare_el2_to_el1_transition(
        virt_boot_core_stack_end_exclusive_addr,
        virt_kernel_init_addr,
    );

    // Turn on the MMU for EL1.
    let addr = Address::new(phys_kernel_tables_base_addr as usize);
    memory::mmu::enable_mmu_and_caching(addr).unwrap();

    // TEMP: second sanity print, right after MMU enable, still physical addressing valid pre-eret.
    for &b in b"MMU ENABLED\r\n" {
        core::ptr::write_volatile(uart_dr, b);
    }

    prepare_backtrace_reset();
    asm::eret()
}

/// The Rust entry of the `kernel` binary, for secondary cores.
///
/// The function is called from the assembly `_start_secondary` function, once a secondary core has
/// been released via PSCI `CPU_ON` or the classic Raspberry Pi spin-table mechanism.
///
/// # Safety
///
/// - Exception return from EL2 must must continue execution in EL1 with `kernel_init_secondary()`.
/// - The kernel's translation tables must already be fully built and enabled-compatible, since this
///   core reuses the boot core's tables rather than building its own.
#[no_mangle]
pub unsafe extern "C" fn _start_rust_secondary(
    phys_kernel_tables_base_addr: u64,
    virt_stack_end_exclusive_addr: u64,
    virt_kernel_init_secondary_addr: u64,
) -> ! {
    prepare_el2_to_el1_transition(
        virt_stack_end_exclusive_addr,
        virt_kernel_init_secondary_addr,
    );

    // Turn on the MMU for EL1. The translation tables are already populated by the boot core; this
    // secondary core just needs to point its own TTBR1_EL1 at the same physical base and enable
    // its own SCTLR_EL1.M bit (this is per-core HW state, not shared).
    let addr = Address::new(phys_kernel_tables_base_addr as usize);
    memory::mmu::enable_mmu_and_caching(addr).unwrap();

    prepare_backtrace_reset();

    asm::eret()
}

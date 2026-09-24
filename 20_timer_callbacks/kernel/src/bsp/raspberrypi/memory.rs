// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! BSP Memory Management.
//!
//! The physical memory layout.
//!
//! The Raspberry's firmware copies the kernel binary to 0x8_0000. The preceding region will be used
//! as the boot core's stack.
//!
//! +---------------------------------------+
//! |                                       | boot_core_stack_start @ 0x0
//! |                                       |                                ^
//! | Boot-core Stack                       |                                | stack
//! |                                       |                                | growth
//! |                                       |                                | direction
//! +---------------------------------------+
//! |                                       | code_start @ 0x8_0000 == boot_core_stack_end_exclusive
//! | .text                                 |
//! | .rodata                               |
//! | .got                                  |
//! | .kernel_symbols                       |
//! |                                       |
//! +---------------------------------------+
//! |                                       | data_start == code_end_exclusive
//! | .data                                 |
//! | .bss                                  |
//! |                                       |
//! +---------------------------------------+
//! |                                       | heap_start == data_end_exclusive
//! | .heap                                 |
//! |                                       |
//! +---------------------------------------+
//! |                                       | heap_end_exclusive
//! |                                       |
//!
//!
//!
//!
//!
//! The virtual memory layout is as follows:
//!
//! +---------------------------------------+
//! |                                       | code_start @ __kernel_virt_start_addr
//! | .text                                 |
//! | .rodata                               |
//! | .got                                  |
//! | .kernel_symbols                       |
//! |                                       |
//! +---------------------------------------+
//! |                                       | data_start == code_end_exclusive
//! | .data                                 |
//! | .bss                                  |
//! |                                       |
//! +---------------------------------------+
//! |                                       | heap_start == data_end_exclusive
//! | .heap                                 |
//! |                                       |
//! +---------------------------------------+
//! |                                       |  mmio_remap_start == heap_end_exclusive
//! | VA region for MMIO remapping          |
//! |                                       |
//! +---------------------------------------+
//! |                                       |  mmio_remap_end_exclusive
//! | Unmapped guard page                   |
//! |                                       |
//! +---------------------------------------+
//! |                                       | boot_core_stack_start
//! |                                       |                                ^
//! | Boot-core Stack                       |                                | stack
//! |                                       |                                | growth
//! |                                       |                                | direction
//! +---------------------------------------+
//! |                                       | boot_core_stack_end_exclusive
//! |                                       |
pub mod mmu;

use crate::memory::{mmu::PageAddress, Address, Physical, Virtual};
use core::cell::UnsafeCell;

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

// Symbols from the linker script.
extern "Rust" {
    static __code_start: UnsafeCell<()>;
    static __code_end_exclusive: UnsafeCell<()>;

    static __data_start: UnsafeCell<()>;
    static __data_end_exclusive: UnsafeCell<()>;

    static __heap_start: UnsafeCell<()>;
    static __heap_end_exclusive: UnsafeCell<()>;

    static __secondary_core_stacks_start: UnsafeCell<()>;
    static __secondary_core_stacks_end_exclusive: UnsafeCell<()>;

    static __mmio_remap_start: UnsafeCell<()>;
    static __mmio_remap_end_exclusive: UnsafeCell<()>;

    static __boot_core_stack_start: UnsafeCell<()>;
    static __boot_core_stack_end_exclusive: UnsafeCell<()>;
}

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// The board's physical memory map.
#[rustfmt::skip]
pub(super) mod map {
    use super::*;

    /// Physical devices.
    #[cfg(feature = "bsp_rpi3")]
    pub mod mmio {
        use super::*;

        pub const PERIPHERAL_IC_START: Address<Physical> = Address::new(0x3F00_B200);
        pub const PERIPHERAL_IC_SIZE:  usize             =              0x24;

        pub const GPIO_START:          Address<Physical> = Address::new(0x3F20_0000);
        pub const GPIO_SIZE:           usize             =              0xA0;

        pub const PL011_UART_START:    Address<Physical> = Address::new(0x3F20_1000);
        pub const PL011_UART_SIZE:     usize             =              0x48;

        pub const LOCAL_IC_START:      Address<Physical> = Address::new(0x4000_0000);
        pub const LOCAL_IC_SIZE:       usize             =              0x100;

        /// DWC2 USB On-The-Go controller (BCM2835/2836/2837 "synopsys,dwc2").
        pub const USB_START:           Address<Physical> = Address::new(0x3F98_0000);
        pub const USB_SIZE:            usize             =              0x1_0000;

        pub const END:                 Address<Physical> = Address::new(0x4001_0000);
    }

    /// Physical devices.
    #[cfg(feature = "bsp_rpi4")]
    pub mod mmio {
        use super::*;

        pub const GPIO_START:       Address<Physical> = Address::new(0xFE20_0000);
        pub const GPIO_SIZE:        usize             =              0xA0;

        pub const PL011_UART_START: Address<Physical> = Address::new(0xFE20_1000);
        pub const PL011_UART_SIZE:  usize             =              0x48;

        pub const GICD_START:       Address<Physical> = Address::new(0xFF84_1000);
        pub const GICD_SIZE:        usize             =              0x824;

        pub const GICC_START:       Address<Physical> = Address::new(0xFF84_2000);
        pub const GICC_SIZE:        usize             =              0x14;

        pub const END:              Address<Physical> = Address::new(0xFF85_0000);
    }

    /// Physical devices.
    /// BCM2712 (Pi 5). RP1-hosted peripherals (GPIO/UART) are exposed at a fixed
    /// physical address because firmware pre-trains the PCIe link and locks the
    /// RP1 BAR before handing off to the kernel — no in-kernel PCIe driver needed.
    ///
    /// TODO_VERIFY all addresses against `bcm2712-rpi-5-b.dtsi` in the Linux
    /// kernel source tree before running on real hardware.
    #[cfg(feature = "bsp_rpi5")]
    pub mod mmio {
        use super::*;

        // RP1 southbridge base as fixed-mapped by firmware.
        // TODO_VERIFY: commonly cited as 0x1f_0000_0000 for the RP1 BAR window.
        // Reserved for future use once RP1-relative offsets are computed from this base
        // rather than hardcoded as absolute addresses below.
        #[allow(dead_code)]
        pub const RP1_BASE:            Address<Physical> = Address::new(0x1F_0000_0000);

        // RP1 UART0 offset within the RP1 BAR (RP1 datasheet: UART block offset).
        // TODO_VERIFY exact offset (RP1 peripherals datasheet, "UART" section).
        pub const PL011_UART_START:    Address<Physical> = Address::new(0x1F_0003_0000);
        pub const PL011_UART_SIZE:     usize             =              0x48;

        // RP1 GPIO offset within the RP1 BAR.
        // TODO_VERIFY exact offset (RP1 peripherals datasheet, "IO_BANK0" section).
        pub const GPIO_START:          Address<Physical> = Address::new(0x1F_000D_0000);
        pub const GPIO_SIZE:           usize             =              0xA0;

        // BCM2712 GIC-400, on-SoC (not RP1).
        // TODO_VERIFY against bcm2712.dtsi `gicv2` node.
        pub const GICD_START:          Address<Physical> = Address::new(0x10_7FFF_9000);
        pub const GICD_SIZE:           usize             =              0x1000;

        pub const GICC_START:          Address<Physical> = Address::new(0x10_7FFF_A000);
        pub const GICC_SIZE:           usize             =              0x1000;

        pub const END:                 Address<Physical> = Address::new(0x1F_0010_0000);
    }

    pub const END: Address<Physical> = mmio::END;
}

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

/// Start page address of the code segment.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn virt_code_start() -> PageAddress<Virtual> {
    PageAddress::from(unsafe { __code_start.get() as usize })
}

/// Size of the code segment.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn code_size() -> usize {
    unsafe { (__code_end_exclusive.get() as usize) - (__code_start.get() as usize) }
}

/// Start page address of the data segment.
#[inline(always)]
fn virt_data_start() -> PageAddress<Virtual> {
    PageAddress::from(unsafe { __data_start.get() as usize })
}

/// Size of the data segment.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn data_size() -> usize {
    unsafe { (__data_end_exclusive.get() as usize) - (__data_start.get() as usize) }
}

/// Start page address of the heap segment.
#[inline(always)]
fn virt_heap_start() -> PageAddress<Virtual> {
    PageAddress::from(unsafe { __heap_start.get() as usize })
}

/// Size of the heap segment.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn heap_size() -> usize {
    unsafe { (__heap_end_exclusive.get() as usize) - (__heap_start.get() as usize) }
}

/// Start page address of the secondary-core stacks region.
#[inline(always)]
fn virt_secondary_core_stacks_start() -> PageAddress<Virtual> {
    PageAddress::from(unsafe { __secondary_core_stacks_start.get() as usize })
}

/// Size of the secondary-core stacks region.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn secondary_core_stacks_size() -> usize {
    unsafe {
        (__secondary_core_stacks_end_exclusive.get() as usize)
            - (__secondary_core_stacks_start.get() as usize)
    }
}

/// Size of a single secondary core's reserved stack.
///
/// # Safety
///
/// - Must stay in sync with `SECONDARY_CORE_STACK_SIZE` in `kernel.ld`.
const SECONDARY_CORE_STACK_SIZE: usize = 128 * 1024;

/// Start page address of the MMIO remap reservation.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn virt_mmio_remap_start() -> PageAddress<Virtual> {
    PageAddress::from(unsafe { __mmio_remap_start.get() as usize })
}

/// Size of the MMIO remap reservation.
///
/// # Safety
///
/// - Value is provided by the linker script and must be trusted as-is.
#[inline(always)]
fn mmio_remap_size() -> usize {
    unsafe { (__mmio_remap_end_exclusive.get() as usize) - (__mmio_remap_start.get() as usize) }
}

/// Start page address of the boot core's stack.
#[inline(always)]
fn virt_boot_core_stack_start() -> PageAddress<Virtual> {
    PageAddress::from(unsafe { __boot_core_stack_start.get() as usize })
}

/// Size of the boot core's stack.
#[inline(always)]
fn boot_core_stack_size() -> usize {
    unsafe {
        (__boot_core_stack_end_exclusive.get() as usize) - (__boot_core_stack_start.get() as usize)
    }
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Exclusive end address of the physical address space.
#[inline(always)]
pub fn phys_addr_space_end_exclusive_addr() -> PageAddress<Physical> {
    PageAddress::from(map::END)
}

/// Return the exclusive-end stack address (i.e., the initial stack pointer value) for the given
/// secondary core.
///
/// `id` must not be the boot core's id. Core 0 (the boot core) uses a separate, dedicated stack
/// region and must not call this function.
///
/// # Panics
///
/// - Panics if `id` is out of range for the reserved secondary-core stacks region.
#[inline(always)]
pub fn secondary_core_stack_top_exclusive(id: crate::cpu::smp::CpuId) -> Address<Virtual> {
    // Raspberry Pi boards are single-cluster, so the raw affinity value is already a flat index
    // (Aff1..3 == 0). Boards with multiple clusters would need a different mapping here.
    let raw = id.raw();

    assert!(raw >= 1, "Core 0 uses the dedicated boot core stack");

    let index = (raw - 1) as usize;
    let offset = (index + 1) * SECONDARY_CORE_STACK_SIZE;

    assert!(
        offset <= secondary_core_stacks_size(),
        "core id out of range for reserved secondary core stacks"
    );

    virt_secondary_core_stacks_start().into_inner() + offset
}

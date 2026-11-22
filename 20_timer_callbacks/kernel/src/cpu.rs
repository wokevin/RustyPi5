// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2020-2023 Andre Richter <andre.o.richter@gmail.com>

//! Processor code.

#[cfg(target_arch = "aarch64")]
#[path = "_arch/aarch64/cpu.rs"]
mod arch_cpu;

#[cfg(target_arch = "x86_64")]
#[path = "_arch/x86_64/cpu.rs"]
mod arch_cpu;

mod boot;

pub mod smp;
pub mod topology;
#[cfg(feature = "bsp_qemu_x86")]
pub mod kernel_worker;
#[cfg(feature = "bsp_qemu_x86")]
pub mod work_queue;

//--------------------------------------------------------------------------------------------------
// Architectural Public Reexports
//--------------------------------------------------------------------------------------------------
pub use arch_cpu::{nop, wait_forever};

#[cfg(feature = "test_build")]
pub use arch_cpu::{qemu_exit_failure, qemu_exit_success};

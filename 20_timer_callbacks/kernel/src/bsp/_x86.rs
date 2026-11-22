// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Shared x86-64 UEFI bring-up code used by the QEMU and Hyper-V BSPs.

pub mod acpi;
pub mod apic;
pub mod console;
pub mod cpu;
pub mod exception;
pub mod gdt;
pub mod idt;
pub mod memory;
#[cfg(feature = "bsp_qemu_x86")]
mod network_shell;
#[cfg(feature = "bsp_qemu_x86")]
mod script;
#[cfg(feature = "bsp_qemu_x86")]
mod script_shell;
pub mod trampoline;
pub mod user;
pub mod video;

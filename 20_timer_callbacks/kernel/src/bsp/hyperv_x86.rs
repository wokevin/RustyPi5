// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Hyper-V x86_64 guest BSP (UEFI-booted, via the `Shims/hyperv-shim` loader).
//!
//! This BSP targets an x86_64 guest running under Hyper-V on an x86_64 host — confirmed viable,
//! unlike the earlier (removed) AArch64 attempt which cannot run on this host's CPU.
//!
//! # TODO
//!
//! This is currently an empty skeleton to validate the build/feature-flag wiring. Real
//! functionality (console, paging, interrupts, CPU bring-up) is added incrementally in
//! subsequent steps.

pub mod console;
pub mod cpu;
pub mod exception;
pub mod memory;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Board identification.
pub fn board_name() -> &'static str {
    "Hyper-V x86_64 Guest (UEFI)"
}

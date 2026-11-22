// SPDX-License-Identifier: MIT OR Apache-2.0

//! Standalone no_std compilation check; does not initialize devices or alter kernel wiring.
#![no_std]

#[path = "..\\kernel\\src\\network.rs"]
pub mod network;

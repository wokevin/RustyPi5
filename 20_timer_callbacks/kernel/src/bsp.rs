// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Conditional reexporting of Board Support Packages.

mod device_driver;

#[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4", feature = "bsp_rpi5"))]
mod raspberrypi;

#[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4", feature = "bsp_rpi5"))]
pub use raspberrypi::*;

#[cfg(feature = "bsp_host")]
mod host;

#[cfg(feature = "bsp_host")]
pub use host::*;

#[cfg(all(feature = "bsp_hyperv_x86", feature = "bsp_qemu_x86"))]
compile_error!("Select either the Hyper-V BSP or the QEMU BSP, not both.");

#[cfg(any(feature = "bsp_hyperv_x86", feature = "bsp_qemu_x86"))]
mod _x86;

#[cfg(feature = "bsp_hyperv_x86")]
mod hyperv_x86;

#[cfg(feature = "bsp_hyperv_x86")]
pub use hyperv_x86::*;

#[cfg(feature = "bsp_qemu_x86")]
mod qemu_x86;

#[cfg(feature = "bsp_qemu_x86")]
pub use qemu_x86::*;

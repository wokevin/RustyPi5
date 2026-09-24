// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Device driver.

#[cfg(any(feature = "bsp_rpi4", feature = "bsp_rpi5"))]
mod arm;
#[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4", feature = "bsp_rpi5"))]
mod bcm;
mod common;
#[cfg(feature = "bsp_rpi3")]
mod dwc2;

#[cfg(any(feature = "bsp_rpi4", feature = "bsp_rpi5"))]
pub use arm::*;
#[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4", feature = "bsp_rpi5"))]
pub use bcm::*;
#[cfg(feature = "bsp_rpi3")]
pub use dwc2::*;

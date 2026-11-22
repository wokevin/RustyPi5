// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural synchronous and asynchronous exception handling.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::exception::arch_exception

use crate::exception::PrivilegeLevel;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// The processing element's current privilege level.
///
/// Host/x86_64 builds always report kernel-level privilege, since there is no real exception-level
/// hardware being emulated for algorithm testing. The bare-metal Hyper-V target reports the same
/// for now, pending real CPL tracking via segment selectors.
pub fn current_privilege_level() -> (PrivilegeLevel, &'static str) {
    #[cfg(feature = "bsp_host")]
    return (PrivilegeLevel::Kernel, "Host");

    #[cfg(feature = "bsp_hyperv_x86")]
    return (PrivilegeLevel::Kernel, "Hyper-V (ring 0 assumed)");

    #[cfg(feature = "bsp_qemu_x86")]
    return (PrivilegeLevel::Kernel, "QEMU (ring 0 assumed)");
}

/// Init exception handling.
///
/// # Safety
///
/// - No-op on host/x86_64, since no real exception vector table is installed.
/// - No-op (for now) on the Hyper-V bare-metal target as well, pending a real IDT.
pub unsafe fn handling_init() {}

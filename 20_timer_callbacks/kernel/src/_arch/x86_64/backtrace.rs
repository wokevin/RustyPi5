// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2022-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural backtracing support.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::backtrace::arch_backtrace

use crate::backtrace::BacktraceItem;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Architectural implementation of the backtrace.
///
/// Host/x86_64 builds do not implement frame-pointer walking yet, so this always reports "no
/// valid stack frame found".
pub fn backtrace(f: impl FnOnce(Option<&mut dyn Iterator<Item = BacktraceItem>>)) {
	f(None)
}

//--------------------------------------------------------------------------------------------------
// Testing
//--------------------------------------------------------------------------------------------------

#[cfg(feature = "test_build")]
#[inline(always)]
/// Hack for corrupting the previous frame address in the current stack frame.
///
/// # Safety
///
/// - To be used only by testing code.
/// - No-op on host/x86_64 since backtrace() doesn't yet walk frames.
pub unsafe fn corrupt_previous_frame_addr() {}

#[cfg(feature = "test_build")]
#[inline(always)]
/// Hack for corrupting the link in the current stack frame.
///
/// # Safety
///
/// - To be used only by testing code.
/// - No-op on host/x86_64 since backtrace() doesn't yet walk frames.
pub unsafe fn corrupt_link() {}

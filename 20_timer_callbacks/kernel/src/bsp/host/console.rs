// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Host BSP console stub.

use crate::console::interface;
use core::fmt;

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// A minimal console that writes directly to the host's stdout.
///
/// Unlike `BufferConsole`, this does not use `InitStateLock`, so it can be written to after the
/// kernel init phase has concluded.
pub struct HostConsole;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

pub static HOST_CONSOLE: HostConsole = HostConsole;

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

impl interface::Write for HostConsole {
	fn write_char(&self, c: char) {
		print!("{}", c);
	}

	fn write_array(&self, a: &[char]) {
		for c in a {
			print!("{}", c);
		}
	}

	fn write_fmt(&self, args: fmt::Arguments) -> fmt::Result {
		use std::io::Write;
		std::print!("{}", args);
		std::io::stdout().flush().ok();

		Ok(())
	}

	fn flush(&self) {
		use std::io::Write;
		std::io::stdout().flush().ok();
	}
}

impl interface::Read for HostConsole {
	fn clear_rx(&self) {}
}

impl interface::Statistics for HostConsole {}
impl interface::All for HostConsole {}

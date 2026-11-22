// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Minimal linear-framebuffer video driver, backed by the GOP framebuffer the
//! `Shims/hyperv-shim` UEFI loader queries and hands off (physical address, stride, resolution)
//! before `ExitBootServices()`. This performs no mode negotiation of its own; it only plots
//! pixels into whatever mode the firmware/shim already selected.
//!
//! # Safety
//!
//! - This driver assumes the framebuffer's physical address is still identity-mapped and
//!   writable, exactly as it was under UEFI boot services. This holds today only because the
//!   kernel has not yet built its own page tables; once it does, the framebuffer region must be
//!   explicitly mapped (writable, no-execute) or every `put_pixel` call here becomes an
//!   instant page fault.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Mirrors `Shims/hyperv-shim`'s `FramebufferInfo` layout exactly -- field order, types, and
/// count must stay in sync with that definition, since the shim writes this struct and the
/// kernel only ever reads it back through a raw pointer with no further validation.
#[repr(C)]
struct FramebufferInfo {
	base: u64,
	width: u32,
	height: u32,
	pixels_per_scan_line: u32,
	bgr: bool,
	frame_buffer_size: usize,
}

/// Physical address of the shim-populated `FramebufferInfo`, set once by `_start` before
/// `kernel_init()` runs. `0` means "not yet set" (or the shim never populated one), distinct from
/// a `FramebufferInfo` with `base == 0` (shim ran, but GOP itself was unavailable) -- both cases
/// must be treated as "no video available" by callers.
static FRAMEBUFFER_INFO_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Records where the shim placed its `FramebufferInfo`. Must be called exactly once, early in
/// `_start`, before any other function in this module is used.
pub fn set_framebuffer_info_addr(addr: usize) {
	FRAMEBUFFER_INFO_ADDR.store(addr, Ordering::Relaxed);
}

/// A simple RGB color, independent of the framebuffer's actual on-the-wire pixel format (this
/// driver handles the RGB/BGR byte-order conversion internally in `put_pixel`).
#[derive(Clone, Copy)]
pub struct Color {
	pub r: u8,
	pub g: u8,
	pub b: u8,
}

impl Color {
	pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
	pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };
	pub const RED: Color = Color { r: 255, g: 0, b: 0 };
	pub const GREEN: Color = Color { r: 0, g: 255, b: 0 };
	pub const BLUE: Color = Color { r: 0, g: 0, b: 255 };
}

/// Returns the framebuffer's resolution as `(width, height)`, or `None` if no GOP framebuffer is
/// available (shim never populated one, or `base == 0`).
pub fn resolution() -> Option<(u32, u32)> {
	let info = framebuffer_info()?;
	Some((info.width, info.height))
}

/// Plots a single pixel at `(x, y)`, silently doing nothing if no framebuffer is available or if
/// `(x, y)` is out of bounds (rather than panicking/faulting on every out-of-range caller, which
/// would be an easy and otherwise-silent way to crash early boot code using this driver).
pub fn put_pixel(x: u32, y: u32, color: Color) {
	let Some(info) = framebuffer_info() else {
		return;
	};
	if x >= info.width || y >= info.height {
		return;
	}

	// Bound the computed offset against the GOP-reported `frame_buffer_size` (in addition to the
	// width/height check above): `pixels_per_scan_line` can make `offset` exceed the actual
	// mapped framebuffer even for an in-bounds `(x, y)` if the firmware's reported stride/size
	// don't agree, and an out-of-bounds write here is an instant, unrecoverable fault this early
	// in boot (no page fault handler / IDT trust yet).
	let offset = (y as usize) * (info.pixels_per_scan_line as usize) + (x as usize);
	if (offset + 1) * core::mem::size_of::<u32>() > info.frame_buffer_size {
		return;
	}

	// SAFETY: see this module's top-level doc comment -- relies on the framebuffer still being
	// identity-mapped and writable, true only before the kernel builds its own page tables.
	unsafe {
		let pixel_ptr = (info.base as *mut u32).add(offset);
		let word = if info.bgr {
			((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32)
		} else {
			((color.b as u32) << 16) | ((color.g as u32) << 8) | (color.r as u32)
		};
		pixel_ptr.write_volatile(word);
	}
}

/// Fills the entire visible framebuffer with a single color. Useful as a cheap, very early
/// "video output is alive" proof-of-life check, independent of any text/font rendering.
pub fn clear(color: Color) {
	let Some((width, height)) = resolution() else {
		return;
	};
	for y in 0..height {
		for x in 0..width {
			put_pixel(x, y, color);
		}
	}
}

/// Reject device mappings that overlap the active framebuffer, including its stride padding.
#[cfg(feature = "bsp_qemu_x86")]
pub(super) fn overlaps_framebuffer(start: usize, end: usize) -> bool {
	match framebuffer_info() {
		None => false,
		Some(info) => match (info.base as usize).checked_add(info.frame_buffer_size) {
			Some(frame_end) => start < frame_end && end > info.base as usize,
			None => true,
		},
	}
}

/// Reads back the shim-populated `FramebufferInfo`, returning `None` if it was never set or if
/// the shim recorded `base == 0` (no GOP framebuffer found/supported).
fn framebuffer_info() -> Option<FramebufferInfo> {
	let addr = FRAMEBUFFER_INFO_ADDR.load(Ordering::Relaxed);
	if addr == 0 {
		return None;
	}

	// SAFETY: addr was provided by the shim via _start and, per FramebufferInfo's doc comment,
	// must point at a valid, already-populated instance of the identically-laid-out struct the
	// shim wrote. This is a trusted handoff boundary, not something validated further here.
	let info = unsafe { core::ptr::read(addr as *const FramebufferInfo) };
	if info.base == 0 {
		return None;
	}
	Some(info)
}

#[cfg(feature = "qemu")]
pub(super) struct TextConsole {
	info: FramebufferInfo,
	column: u32,
	row: u32,
}

#[cfg(feature = "qemu")]
impl TextConsole {
	const CELL_WIDTH: u32 = 12;
	const CELL_HEIGHT: u32 = 16;

	pub(super) fn new() -> Result<Self, &'static str> {
		let info = framebuffer_info().ok_or("No GOP framebuffer; using serial only")?;
		let size = (info.pixels_per_scan_line as usize)
			.checked_mul(info.height as usize)
			.and_then(|pixels| pixels.checked_mul(4))
			.ok_or("Framebuffer dimensions overflow; using serial only")?;
		if info.width < Self::CELL_WIDTH || info.height < Self::CELL_HEIGHT
			|| info.pixels_per_scan_line < info.width || size > info.frame_buffer_size
			|| (info.base as usize).checked_add(size).is_none() || info.base % 4 != 0
		{
			return Err("Invalid GOP framebuffer geometry; using serial only");
		}
		let mut console = Self { info, column: 0, row: 0 };
		console.clear_rows(0, console.info.height);
		Ok(console)
	}

	pub(super) fn write_byte(&mut self, byte: u8) {
		match byte {
			b'\r' => self.column = 0,
			b'\n' => self.new_line(),
			b'\x08' => {
				if self.column != 0 {
					self.column -= 1;
				} else if self.row != 0 {
					self.row -= 1;
					self.column = self.info.width / Self::CELL_WIDTH - 1;
				}
			}
			_ => {
				if self.column == self.info.width / Self::CELL_WIDTH {
					self.new_line();
				}
				let glyph = glyph(byte);
				for y in 0..Self::CELL_HEIGHT {
					for x in 0..Self::CELL_WIDTH {
						let lit = x < 10 && y < 14
							&& glyph[(y / 2) as usize] & (1 << (4 - x / 2)) != 0;
						let color = if lit { 0x00DD_DDDD } else { 0 };
						self.pixel(self.column * Self::CELL_WIDTH + x,
							self.row * Self::CELL_HEIGHT + y, color);
					}
				}
				self.column += 1;
			}
		}
	}

	fn pixel(&self, x: u32, y: u32, color: u32) {
		let offset = y as usize * self.info.pixels_per_scan_line as usize + x as usize;
		// Geometry is validated once at construction; all callers stay within visible rows.
		unsafe { (self.info.base as *mut u32).add(offset).write_volatile(color) };
	}

	fn clear_rows(&mut self, start: u32, end: u32) {
		for y in start..end {
			for x in 0..self.info.width {
				self.pixel(x, y, 0);
			}
		}
	}

	fn new_line(&mut self) {
		self.column = 0;
		self.row += 1;
		let rows = self.info.height / Self::CELL_HEIGHT;
		if self.row < rows {
			return;
		}
		let visible_height = rows * Self::CELL_HEIGHT;
		let stride = self.info.pixels_per_scan_line as usize;
		let base = self.info.base as *mut u32;
		for y in Self::CELL_HEIGHT..visible_height {
			for x in 0..self.info.width {
				unsafe {
					let value = base.add(y as usize * stride + x as usize).read_volatile();
					base.add((y - Self::CELL_HEIGHT) as usize * stride + x as usize)
						.write_volatile(value);
				}
			}
		}
		self.row = rows - 1;
		self.clear_rows(self.row * Self::CELL_HEIGHT, visible_height);
	}
}

#[cfg(feature = "qemu")]
fn glyph(byte: u8) -> [u8; 7] {
	match byte {
		b' ' => [0; 7],
		b'!' => [4, 4, 4, 4, 4, 0, 4],
		b'"' => [10, 10, 10, 0, 0, 0, 0],
		b'#' => [10, 10, 31, 10, 31, 10, 10],
		b'$' => [4, 15, 20, 14, 5, 30, 4],
		b'%' => [24, 25, 2, 4, 8, 19, 3],
		b'&' => [12, 18, 20, 8, 21, 18, 13],
		b'\'' => [4, 4, 8, 0, 0, 0, 0],
		b'(' => [2, 4, 8, 8, 8, 4, 2],
		b')' => [8, 4, 2, 2, 2, 4, 8],
		b'*' => [0, 21, 14, 31, 14, 21, 0],
		b'+' => [0, 4, 4, 31, 4, 4, 0],
		b',' => [0, 0, 0, 0, 0, 4, 8],
		b'-' => [0, 0, 0, 31, 0, 0, 0],
		b'.' => [0, 0, 0, 0, 0, 0, 4],
		b'/' => [1, 2, 2, 4, 8, 8, 16],
		b'0' => [14, 17, 19, 21, 25, 17, 14],
		b'1' => [4, 12, 4, 4, 4, 4, 14],
		b'2' => [14, 17, 1, 2, 4, 8, 31],
		b'3' => [30, 1, 1, 14, 1, 1, 30],
		b'4' => [2, 6, 10, 18, 31, 2, 2],
		b'5' => [31, 16, 16, 30, 1, 1, 30],
		b'6' => [14, 16, 16, 30, 17, 17, 14],
		b'7' => [31, 1, 2, 4, 8, 8, 8],
		b'8' => [14, 17, 17, 14, 17, 17, 14],
		b'9' => [14, 17, 17, 15, 1, 1, 14],
		b':' => [0, 0, 4, 0, 4, 0, 0],
		b';' => [0, 0, 4, 0, 4, 4, 8],
		b'<' => [1, 2, 4, 8, 4, 2, 1],
		b'=' => [0, 0, 31, 0, 31, 0, 0],
		b'>' => [16, 8, 4, 2, 4, 8, 16],
		b'?' => [14, 17, 1, 2, 4, 0, 4],
		b'@' => [14, 17, 23, 21, 23, 16, 14],
		b'A' => [14, 17, 17, 31, 17, 17, 17],
		b'B' => [30, 17, 17, 30, 17, 17, 30],
		b'C' => [14, 17, 16, 16, 16, 17, 14],
		b'D' => [30, 17, 17, 17, 17, 17, 30],
		b'E' => [31, 16, 16, 30, 16, 16, 31],
		b'F' => [31, 16, 16, 30, 16, 16, 16],
		b'G' => [14, 17, 16, 23, 17, 17, 15],
		b'H' => [17, 17, 17, 31, 17, 17, 17],
		b'I' => [14, 4, 4, 4, 4, 4, 14],
		b'J' => [7, 2, 2, 2, 2, 18, 12],
		b'K' => [17, 18, 20, 24, 20, 18, 17],
		b'L' => [16, 16, 16, 16, 16, 16, 31],
		b'M' => [17, 27, 21, 21, 17, 17, 17],
		b'N' => [17, 25, 21, 19, 17, 17, 17],
		b'O' => [14, 17, 17, 17, 17, 17, 14],
		b'P' => [30, 17, 17, 30, 16, 16, 16],
		b'Q' => [14, 17, 17, 17, 21, 18, 13],
		b'R' => [30, 17, 17, 30, 20, 18, 17],
		b'S' => [15, 16, 16, 14, 1, 1, 30],
		b'T' => [31, 4, 4, 4, 4, 4, 4],
		b'U' => [17, 17, 17, 17, 17, 17, 14],
		b'V' => [17, 17, 17, 17, 17, 10, 4],
		b'W' => [17, 17, 17, 21, 21, 21, 10],
		b'X' => [17, 17, 10, 4, 10, 17, 17],
		b'Y' => [17, 17, 10, 4, 4, 4, 4],
		b'Z' => [31, 1, 2, 4, 8, 16, 31],
		b'[' => [14, 8, 8, 8, 8, 8, 14],
		b'\\' => [16, 8, 8, 4, 2, 2, 1],
		b']' => [14, 2, 2, 2, 2, 2, 14],
		b'^' => [4, 10, 17, 0, 0, 0, 0],
		b'_' => [0, 0, 0, 0, 0, 0, 31],
		b'`' => [8, 4, 2, 0, 0, 0, 0],
		b'a' => [0, 0, 14, 1, 15, 17, 15],
		b'b' => [16, 16, 30, 17, 17, 17, 30],
		b'c' => [0, 0, 14, 16, 16, 17, 14],
		b'd' => [1, 1, 15, 17, 17, 17, 15],
		b'e' => [0, 0, 14, 17, 31, 16, 14],
		b'f' => [6, 8, 8, 28, 8, 8, 8],
		b'g' => [0, 15, 17, 17, 15, 1, 14],
		b'h' => [16, 16, 30, 17, 17, 17, 17],
		b'i' => [4, 0, 12, 4, 4, 4, 14],
		b'j' => [2, 0, 6, 2, 2, 18, 12],
		b'k' => [16, 16, 18, 20, 24, 20, 18],
		b'l' => [12, 4, 4, 4, 4, 4, 14],
		b'm' => [0, 0, 26, 21, 21, 17, 17],
		b'n' => [0, 0, 30, 17, 17, 17, 17],
		b'o' => [0, 0, 14, 17, 17, 17, 14],
		b'p' => [0, 30, 17, 17, 30, 16, 16],
		b'q' => [0, 15, 17, 17, 15, 1, 1],
		b'r' => [0, 0, 22, 25, 16, 16, 16],
		b's' => [0, 0, 15, 16, 14, 1, 30],
		b't' => [8, 8, 28, 8, 8, 9, 6],
		b'u' => [0, 0, 17, 17, 17, 19, 13],
		b'v' => [0, 0, 17, 17, 17, 10, 4],
		b'w' => [0, 0, 17, 17, 21, 21, 10],
		b'x' => [0, 0, 17, 10, 4, 10, 17],
		b'y' => [0, 17, 17, 17, 15, 1, 14],
		b'z' => [0, 0, 31, 2, 4, 8, 31],
		b'{' => [2, 4, 4, 8, 4, 4, 2],
		b'|' => [4, 4, 4, 4, 4, 4, 4],
		b'}' => [8, 4, 4, 2, 4, 4, 8],
		b'~' => [0, 0, 8, 21, 2, 0, 0],
		_ => [14, 17, 1, 2, 4, 0, 4],
	}
}

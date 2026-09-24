// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural asynchronous exception handling.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::exception::asynchronous::arch_asynchronous

use core::sync::atomic::{AtomicBool, Ordering};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Tracks whether IRQs are currently masked on the (single) core.
///
/// `true` means masked/disabled. On `bsp_host`, this is a pure bookkeeping flag with no real
/// hardware behind it. On `bsp_hyperv_x86`, this is also a placeholder for now — real `cli`/`sti`
/// based masking is not yet implemented, pending IDT/APIC bring-up.
static IRQ_MASKED: AtomicBool = AtomicBool::new(false);

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Returns whether IRQs are masked on the executing core.
pub fn is_local_irq_masked() -> bool {
    IRQ_MASKED.load(Ordering::Relaxed)
}

/// Unmask IRQs on the executing core.
#[inline(always)]
pub fn local_irq_unmask() {
    IRQ_MASKED.store(false, Ordering::Relaxed);
}

/// Mask IRQs on the executing core.
#[inline(always)]
pub fn local_irq_mask() {
    IRQ_MASKED.store(true, Ordering::Relaxed);
}

/// Mask IRQs on the executing core and return the previously saved interrupt mask bits.
#[inline(always)]
pub fn local_irq_mask_save() -> u64 {
    let saved = IRQ_MASKED.load(Ordering::Relaxed) as u64;
    local_irq_mask();

    saved
}

/// Restore the interrupt mask bits using the callee's argument.
///
/// # Invariant
///
/// - No sanity checks on the input.
#[inline(always)]
pub fn local_irq_restore(saved: u64) {
    IRQ_MASKED.store(saved != 0, Ordering::Relaxed);
}

/// Print the exception status.
pub fn print_state() {
    use crate::info;

    let to_mask_str = |x| -> _ { if x { "Masked" } else { "Unmasked" } };

    info!("      IRQ:    {}", to_mask_str(IRQ_MASKED.load(Ordering::Relaxed)));
}

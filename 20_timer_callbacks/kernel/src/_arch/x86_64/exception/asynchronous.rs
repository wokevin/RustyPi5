// SPDX-License-Identifier: MIT OR Apache-2.0

//! Architectural interrupt masking.

#[cfg(feature = "bsp_qemu_x86")]
use core::arch::asm;
#[cfg(not(feature = "bsp_qemu_x86"))]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(feature = "bsp_qemu_x86"))]
static IRQ_MASKED: AtomicBool = AtomicBool::new(false);

#[inline(always)]
pub fn is_local_irq_masked() -> bool {
    #[cfg(feature = "bsp_qemu_x86")]
    {
        let flags: u64;
        unsafe {
            asm!(
                "pushfq",
                "pop {}",
                out(reg) flags,
                options(preserves_flags),
            );
        }
        flags & (1 << 9) == 0
    }

    #[cfg(not(feature = "bsp_qemu_x86"))]
    {
        IRQ_MASKED.load(Ordering::Relaxed)
    }
}

#[inline(always)]
pub fn local_irq_unmask() {
    #[cfg(feature = "bsp_qemu_x86")]
    unsafe {
        asm!("sti", options(nostack));
    }

    #[cfg(not(feature = "bsp_qemu_x86"))]
    IRQ_MASKED.store(false, Ordering::Relaxed);
}

#[inline(always)]
pub fn local_irq_mask() {
    #[cfg(feature = "bsp_qemu_x86")]
    unsafe {
        asm!("cli", options(nostack));
    }

    #[cfg(not(feature = "bsp_qemu_x86"))]
    IRQ_MASKED.store(true, Ordering::Relaxed);
}

#[inline(always)]
pub fn local_irq_mask_save() -> u64 {
    #[cfg(feature = "bsp_qemu_x86")]
    {
        let flags: u64;
        unsafe {
            asm!(
                "pushfq",
                "pop {}",
                "cli",
                out(reg) flags,
            );
        }
        u64::from(flags & (1 << 9) == 0)
    }

    #[cfg(not(feature = "bsp_qemu_x86"))]
    {
        let saved = IRQ_MASKED.load(Ordering::Relaxed) as u64;
        local_irq_mask();
        saved
    }
}

#[inline(always)]
pub fn local_irq_restore(saved: u64) {
    if saved == 0 {
        local_irq_unmask();
    } else {
        local_irq_mask();
    }
}

pub fn print_state() {
    crate::info!(
        "      IRQ:    {}",
        if is_local_irq_masked() {
            "Masked"
        } else {
            "Unmasked"
        }
    );
}

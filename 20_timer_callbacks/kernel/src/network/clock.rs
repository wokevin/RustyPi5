// SPDX-License-Identifier: MIT OR Apache-2.0

//! CPU-0 TSC clock and APIC calibration against the PC PIT's 1.193182 MHz channel 2.
//! PIT channel 0 and the speaker enable bit are left untouched.

use core::arch::asm;

pub struct Clock { origin: u64, cycles_per_ms: u64 }

unsafe fn input(port: u16) -> u8 {
    let value;
    asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}
unsafe fn output(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}
fn counter() -> u64 {
    unsafe {
        core::arch::x86_64::_mm_lfence();
        core::arch::x86_64::_rdtsc()
    }
}

impl Clock {
    /// Requires exclusive PIT channel 2 and local APIC ownership, on CPU 0 with IRQs masked.
    pub fn calibrate() -> Result<(Self, u32), &'static str> {
        if crate::bsp::cpu::current_core_id().raw() != 0
            || !crate::exception::asynchronous::is_local_irq_masked()
        { return Err("Network clock calibration requires masked CPU 0"); }
        if unsafe { core::arch::x86_64::__cpuid(1).edx } & (1 << 4) == 0 {
            return Err("Network clock requires a TSC");
        }
        const PIT_COUNT: u16 = 59659; // Approximately 50 ms, independent of CPU frequency.
        unsafe {
            let saved = input(0x61);
            output(0x61, saved & !3);
            output(0x43, 0xB0);
            output(0x42, PIT_COUNT as u8);
            output(0x42, (PIT_COUNT >> 8) as u8);
            crate::bsp::apic::begin_timer_calibration();
            let start = counter();
            output(0x61, (saved & !2) | 1);
            let mut completed = false;
            // Iteration bound only guards broken hardware; elapsed time comes from the PIT.
            for _ in 0..50_000_000 {
                if input(0x61) & 0x20 != 0 { completed = true; break; }
                core::hint::spin_loop();
            }
            let end = counter();
            let apic_ticks = crate::bsp::apic::finish_timer_calibration();
            output(0x61, saved);
            if !completed { return Err("PIT calibration timed out"); }
            let elapsed = end.checked_sub(start).ok_or("TSC moved backwards during calibration")?;
            let cycles_per_ms = ((elapsed as u128 * 1_193_182) / (PIT_COUNT as u128 * 1000)) as u64;
            let poll_ticks = ((apic_ticks as u64 * 1_193_182) / (PIT_COUNT as u64 * 100)) as u32;
            if cycles_per_ms == 0 || poll_ticks == 0 || apic_ticks == u32::MAX {
                return Err("Invalid TSC/APIC calibration");
            }
            Ok((Self { origin: end, cycles_per_ms }, poll_ticks))
        }
    }

    pub fn milliseconds(&self) -> Result<u64, &'static str> {
        counter().checked_sub(self.origin)
            .map(|cycles| cycles / self.cycles_per_ms)
            .ok_or("Network TSC moved backwards")
    }
    pub fn nonce(&self) -> u64 { self.origin }
}

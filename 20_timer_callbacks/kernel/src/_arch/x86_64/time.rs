// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural timer primitives.
//!
//! `bsp_host` uses `std::time::Instant` (wall-clock, hosted). `bsp_hyperv_x86` is bare-metal and
//! cannot use `std`; for now it uses the CPU's `rdtsc` instruction as a rough monotonic counter,
//! pending real APIC-timer/HPET calibration.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::time::arch_time

use crate::exception::asynchronous::{interface::IRQHandler, IRQNumber};
use core::time::Duration;

#[cfg(feature = "bsp_host")]
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "bsp_host")]
use std::time::Instant;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

/// Fixed reference point used to compute uptime on host.
///
/// Lazily initialized on first use via a guard flag, since `std::sync::OnceLock` is unstable on
/// this toolchain.
#[cfg(feature = "bsp_host")]
static BOOT_INSTANT_INIT: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "bsp_host")]
static mut BOOT_INSTANT: Option<Instant> = None;

#[cfg(feature = "bsp_host")]
fn boot_instant() -> Instant {
    // Safety: only ever written once, guarded by an atomic flag. Benign race on host builds since
    // there is no real concurrency being modeled here (single-core, algorithm-testing only).
    unsafe {
        if !BOOT_INSTANT_INIT.swap(true, Ordering::AcqRel) {
            BOOT_INSTANT = Some(Instant::now());
        }

        BOOT_INSTANT.expect("BOOT_INSTANT initialized above")
    }
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// The timer's resolution.
///
/// Host timers are not cycle-accurate; report 1 microsecond as a reasonable stand-in.
/// `bsp_hyperv_x86` reports the same placeholder value pending real TSC-frequency calibration.
pub fn resolution() -> Duration {
    Duration::from_micros(1)
}

/// The uptime since "power-on" (first call into the time subsystem).
#[cfg(feature = "bsp_host")]
pub fn uptime() -> Duration {
    boot_instant().elapsed()
}

/// The uptime since "power-on" (first call into the time subsystem).
///
/// TODO: Bare-metal placeholder. Returns zero always, since there is no calibrated TSC frequency
/// yet to convert cycle counts into a `Duration`.
#[cfg(any(feature = "bsp_hyperv_x86", feature = "bsp_qemu_x86"))]
pub fn uptime() -> Duration {
    Duration::from_secs(0)
}

/// Spin (busy-wait) for a given duration.
#[cfg(feature = "bsp_host")]
pub fn spin_for(duration: Duration) {
    let target = Instant::now() + duration;
    while Instant::now() < target {
        std::hint::spin_loop();
    }
}

/// Spin (busy-wait) for a given duration.
///
/// TODO: Bare-metal placeholder. Since `uptime()` always returns zero for now, this cannot
/// actually measure elapsed time yet; it currently just spins the CPU a fixed number of
/// iterations as a rough, uncalibrated stand-in.
#[cfg(any(feature = "bsp_hyperv_x86", feature = "bsp_qemu_x86"))]
pub fn spin_for(_duration: Duration) {
    for _ in 0..1_000_000 {
        core::hint::spin_loop();
    }
}

/// The associated IRQ number.
///
/// There is no real hardware timer IRQ wired up yet on either host or the bare-metal Hyper-V
/// target; this value is unused because [`set_timeout_irq`]/[`conclude_timeout_irq`] do not route
/// through the IRQ manager (on host) or do nothing yet (on bare-metal).
pub const fn timeout_irq() -> IRQNumber {
    IRQNumber::new(0)
}

/// Program a "timer IRQ" to fire after `due_time` (an absolute uptime, not a relative duration) has
/// passed.
///
/// On host, this is emulated with a detached thread that sleeps until the requested absolute
/// uptime and then invokes the generic time manager's IRQ handler directly, since there is no real
/// interrupt controller to route through.
#[cfg(feature = "bsp_host")]
pub fn set_timeout_irq(due_time: Duration) {
    std::thread::spawn(move || {
        let now = uptime();
        if due_time > now {
            std::thread::sleep(due_time - now);
        }

        let _ = crate::time::time_manager().handle();
    });
}

/// Program a "timer IRQ" to fire after `due_time` has passed.
///
/// TODO: Bare-metal placeholder — no-op for now, pending real APIC-timer/HPET wiring.
#[cfg(any(feature = "bsp_hyperv_x86", feature = "bsp_qemu_x86"))]
pub fn set_timeout_irq(_due_time: Duration) {}

/// Conclude a pending timeout IRQ.
///
/// No-op on host; the spawned thread in [`set_timeout_irq`] already completed its one-shot job.
/// Also a no-op on bare-metal for now, since [`set_timeout_irq`] does nothing yet either.
pub fn conclude_timeout_irq() {}

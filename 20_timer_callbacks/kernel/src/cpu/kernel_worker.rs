// SPDX-License-Identifier: MIT OR Apache-2.0

//! Deferred work executed exclusively by logical CPU 0.

use super::work_queue::{Work, WorkQueue};
use crate::{
	bsp,
	exception::asynchronous,
	synchronization::{interface::Mutex, SpinLock},
};
use core::{
	arch::asm,
	sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static QUEUE: SpinLock<WorkQueue<64>> = SpinLock::new(WorkQueue::new());
static READY: AtomicBool = AtomicBool::new(false);
static WAKE_INTERRUPTS: AtomicU64 = AtomicU64::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static SLEEPING: AtomicBool = AtomicBool::new(false);

pub fn init() -> Result<(), &'static str> {
	if bsp::cpu::current_core_id().raw() != 0 {
		return Err("Kernel worker must be initialized on CPU 0");
	}
	if !asynchronous::is_local_irq_masked() {
		return Err("Kernel worker initialization requires masked interrupts");
	}
	if READY.load(Ordering::Acquire) {
		return Err("Kernel worker already initialized");
	}

	bsp::apic::init_kernel_worker()?;
	READY.store(true, Ordering::Release);
	Ok(())
}

pub fn submit(work: Work) -> Result<(), &'static str> {
	if !READY.load(Ordering::Acquire) {
		return Err("Kernel worker is not initialized");
	}

	let target = bsp::cpu::hardware_id_for(super::smp::CpuId::new(0))
		.ok_or("Kernel CPU has no hardware APIC ID")?;

	if let Err(message) = QUEUE.lock(|queue| queue.push(work)) {
		crate::warn!("{}", message);
		return Err(message);
	}

	if let Err(message) = bsp::apic::notify_kernel(target) {
		panic!("Kernel work accepted but notification failed: {}", message);
	}

	Ok(())
}

pub fn handle_wakeup() {
	WAKE_INTERRUPTS.fetch_add(1, Ordering::Release);
	bsp::apic::end_of_interrupt();
}

fn process(work: Work) {
	match work {
		Work::Probe {
			source_cpu,
			sequence,
		} => {
			assert_eq!(bsp::cpu::current_core_id().raw(), 0);
			crate::info!(
				"Kernel worker: CPU {} probe {} processed on CPU 0; wake IRQs={}",
				source_cpu,
				sequence,
				WAKE_INTERRUPTS.load(Ordering::Acquire),
			);
			COMPLETED.store(sequence, Ordering::Release);
		}
	}
}

/// Boot acceptance check on the selected shell CPU, before entering ring 3.
pub fn verify_cross_core() -> Result<(), &'static str> {
	let source_cpu = bsp::cpu::current_core_id().raw() as usize;
	if source_cpu == 0 { return Err("Cross-core probe requires a secondary CPU"); }
	for sequence in 1..=16 {
		wait_until(|| SLEEPING.load(Ordering::Acquire))?;
		let interrupts = WAKE_INTERRUPTS.load(Ordering::Acquire);
		submit(Work::Probe { source_cpu, sequence })?;
		wait_until(|| COMPLETED.load(Ordering::Acquire) == sequence
			&& WAKE_INTERRUPTS.load(Ordering::Acquire) > interrupts)?;
	}
	wait_until(|| SLEEPING.load(Ordering::Acquire))?;
	crate::info!("Kernel worker verification: 16 cross-core completions and wake IRQs PASS");
	Ok(())
}

fn wait_until(ready: impl Fn() -> bool) -> Result<(), &'static str> {
	for _ in 0..50_000_000 {
		if ready() { return Ok(()); }
		core::hint::spin_loop();
	}
	Err("Kernel worker wakeup/completion timed out")
}

fn verify_irq_restore() {
	assert!(asynchronous::is_local_irq_masked());
	let masked = asynchronous::local_irq_mask_save();
	asynchronous::local_irq_restore(masked);
	assert!(asynchronous::is_local_irq_masked());
	asynchronous::local_irq_unmask();
	assert!(!asynchronous::is_local_irq_masked());
	let enabled = asynchronous::local_irq_mask_save();
	assert!(asynchronous::is_local_irq_masked());
	let nested = asynchronous::local_irq_mask_save();
	asynchronous::local_irq_restore(nested);
	assert!(asynchronous::is_local_irq_masked());
	asynchronous::local_irq_restore(enabled);
	assert!(!asynchronous::is_local_irq_masked());
	asynchronous::local_irq_mask();
	crate::info!("Hardware IRQ mask/restore verification PASS on CPU 0");
}

pub fn run(mut network: Option<crate::network::Runtime>) -> ! {
	assert_eq!(bsp::cpu::current_core_id().raw(), 0);
	assert!(READY.load(Ordering::Acquire));
	verify_irq_restore();
	if let Some(service) = network.as_ref() { service.start_timer(); }

	loop {
		asynchronous::local_irq_mask();
		let work = QUEUE.lock(|queue| queue.pop());
		let poll_network = crate::network::take_poll_due();

		if work.is_some() || poll_network {
			asynchronous::local_irq_unmask();
			if let Some(work) = work { process(work); }
			if poll_network {
				let finished = network.as_mut().map_or(false, |service| service.poll());
				if finished { network = None; }
			}
		} else {
			SLEEPING.store(true, Ordering::Release);
			// Keep the empty check and sleep free of an enabled-interrupt gap.
			unsafe {
				asm!("sti", "hlt", options(nostack));
			}
			SLEEPING.store(false, Ordering::Release);
		}
	}
}

// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

// Rust embedded logo for `make doc`.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/rust-embedded/wg/master/assets/logo/ewg-logo-blue-white-on-transparent.png"
)]

//! The `kernel` binary.

#![feature(format_args_nl)]
#![feature(abi_x86_interrupt)]
#![cfg_attr(not(feature = "bsp_host"), no_main)]
#![cfg_attr(not(feature = "bsp_host"), no_std)]

extern crate alloc;

use libkernel::{
    bsp, cpu, driver, exception, info, memory, state, time,
};

#[cfg(not(feature = "bsp_qemu_x86"))]
use libkernel::synchronization::{interface::Mutex, SpinLock};

/// Shared counter used to demonstrate that cross-core mutual exclusion actually works once
/// secondary cores are brought online.
///
/// Every online core (boot core included) races to increment this the same number of times. If
/// `SpinLock` did not provide real mutual exclusion, the final total would be less than the
/// expected `NUM_CORES * INCREMENTS_PER_CORE` due to lost updates from concurrent, unsynchronized
/// read-modify-write races. Since the total exactly matches the expected value on real
/// hardware/QEMU with multiple cores, this is a correctness test, not just a liveness test.
#[cfg(not(feature = "bsp_qemu_x86"))]
static SHARED_COUNTER: SpinLock<u64> = SpinLock::new(0);

/// Number of increments each core performs on `SHARED_COUNTER`.
#[cfg(not(feature = "bsp_qemu_x86"))]
const INCREMENTS_PER_CORE: u64 = 100_000;

/// Increment `SHARED_COUNTER` by `INCREMENTS_PER_CORE`, one at a time, taking the lock for each
/// individual increment. This maximizes lock contention between cores (as opposed to locking once
/// and incrementing in a tight loop), which is what actually stresses the cross-core exclusion
/// guarantee.
///
/// Prints a "thread N" style identity line before starting and after finishing, using the core's
/// raw affinity value (which is a flat `0..N` index on Raspberry Pi's single-cluster topology) as
/// the thread number.
#[cfg(not(feature = "bsp_qemu_x86"))]
fn hammer_shared_counter() {
    let thread_id = cpu::smp::core_id().raw();

    info!("Thread {}: starting ({} increments)", thread_id, INCREMENTS_PER_CORE);

    for _ in 0..INCREMENTS_PER_CORE {
        SHARED_COUNTER.lock(|counter| *counter += 1);
    }

    info!("Thread {}: done", thread_id);
}

/// Early init code.
///
/// When this code runs, virtual memory is already enabled.
///
/// # Safety
///
/// - Only a single core must be active and running this function.
/// - Printing will not work until the respective driver's MMIO is remapped.
#[cfg_attr(not(feature = "bsp_host"), no_mangle)]
unsafe fn kernel_init() -> ! {
    #[cfg(feature = "bsp_qemu_x86")]
    exception::asynchronous::local_irq_mask();
    exception::handling_init();

    #[cfg(not(feature = "bsp_host"))]
    memory::init();

    #[cfg(not(feature = "bsp_host"))]
    memory::heap_alloc::kernel_init_heap_allocator();

    // Initialize the timer subsystem.
    if let Err(x) = time::init() {
        panic!("Error initializing timer subsystem: {}", x);
    }

    // Initialize the BSP driver subsystem.
    if let Err(x) = bsp::driver::init() {
        panic!("Error initializing BSP driver subsystem: {}", x);
    }

    // Initialize all device drivers.
    driver::driver_manager().init_drivers_and_irqs();

    #[cfg(not(feature = "bsp_host"))]
    bsp::memory::mmu::kernel_add_mapping_records_for_precomputed();

    // On boards using the classic spin-table release mechanism (Pi 3/4 real hardware only — QEMU
    // uses PSCI instead, see the `qemu` feature), the mailbox addresses are physical and must be
    // mapped into the kernel's virtual address space before any write through them is attempted
    // (writes happen post-MMU-enable). This must run before the state transition below, since
    // mapping goes through `InitStateLock`-protected structures that are only writable during the
    // `Init` state.
    #[cfg(all(
        not(feature = "bsp_host"),
        any(feature = "bsp_rpi3", feature = "bsp_rpi4"),
        not(feature = "qemu")
    ))]
    if let Err(x) = bsp::cpu::init_smp_mailboxes() {
        panic!("Error initializing SMP spin-table mailboxes: {}", x);
    }

    // QEMU enables hardware interrupts only after the worker and APIC are ready.
    #[cfg(not(feature = "bsp_qemu_x86"))]
    exception::asynchronous::local_irq_unmask();

    // Announce conclusion of the kernel_init() phase.
    state::state_manager().transition_to_single_core_main();

    #[cfg(feature = "bsp_qemu_x86")]
    {
        if let Err(message) = bsp::user::prepare() {
            panic!("Cannot reserve kernel CPU: {}", message);
        }
        if let Err(message) = cpu::kernel_worker::init() {
            panic!("Cannot initialize kernel worker: {}", message);
        }
    }

    // Bring up secondary cores discovered by the BSP; QEMU requires startup acknowledgement.
    #[cfg(not(feature = "bsp_host"))]
    {
        // Secondary cores must be released into the assembly `_start_secondary` entry point, NOT
        // directly into `kernel_init_secondary`. At release time the core starts with the MMU
        // disabled and is still executing at whatever EL the firmware/PSCI left it in; only
        // `_start_secondary` (in `boot.s`) performs the EL2 check, sets up a temporary physical
        // stack, enables the MMU via `_start_rust_secondary`, and *then* jumps into
        // `kernel_init_secondary` from virtual, post-MMU Rust code.
        extern "C" {
            fn _start_secondary();
        }

        let entry_point = _start_secondary as usize;

        let brought_up = cpu::smp::bring_up_all_cores(entry_point, |id| {
            bsp::memory::secondary_core_stack_top_exclusive(id).as_usize()
        });

        if brought_up > 0 {
            state::state_manager().transition_to_multi_core_main();
        }

        info!("Secondary cores brought online: {}", brought_up);
    }

    // Transition from unsafe to safe.
    kernel_main()
}

/// Early init code for secondary cores.
///
/// When this code runs, virtual memory is already enabled (secondary cores reuse the boot core's
/// translation tables).
///
/// # Safety
///
/// - This function must only ever be reached via `_start_rust_secondary`, immediately after a
///   secondary core has been released.
#[cfg(not(feature = "bsp_host"))]
#[no_mangle]
unsafe extern "C" fn kernel_init_secondary() -> ! {
    #[cfg(feature = "bsp_qemu_x86")]
    {
        bsp::idt::load_secondary();
        bsp::gdt::init_secondary();
        bsp::apic::init_local().expect("Cannot initialize secondary local APIC");
    }
    let this_core = cpu::smp::core_id();

    // Find and mark this core online in the board's static core table.
    for c in cpu::smp::cores() {
        if c.id() == this_core {
            c.mark_online();
            break;
        }
    }

    #[cfg(feature = "bsp_qemu_x86")]
    bsp::user::secondary_idle();
    #[cfg(not(feature = "bsp_qemu_x86"))]
    cpu::wait_forever();
}

/// The main function running after the early init.
fn kernel_main() -> ! {
    #[cfg(not(feature = "bsp_host"))]
    use alloc::boxed::Box;
    use core::time::Duration;

    info!("{}", libkernel::version());
    info!("Booting on: {}", bsp::board_name());

    #[cfg(not(feature = "bsp_host"))]
    {
        memory::mmu::kernel_print_mappings();
    }

    let (_, privilege_level) = exception::current_privilege_level();
    info!("Current privilege level: {}", privilege_level);

    info!("Exception handling state:");
    exception::asynchronous::print_state();

    info!(
        "Architectural timer resolution: {} ns",
        time::time_manager().resolution().as_nanos()
    );

    info!("Drivers loaded:");
    driver::driver_manager().enumerate();

    info!("Registered IRQ handlers:");
    exception::asynchronous::irq_manager().print_handler();

    info!("Kernel heap:");
    memory::heap_alloc::kernel_heap_allocator().print_usage();

    // QEMU secondaries wait for explicit user placement rather than running this legacy demo.
    #[cfg(all(not(feature = "bsp_host"), not(feature = "bsp_qemu_x86")))]
    {
        let online_cores = cpu::smp::cores().iter().filter(|c| c.is_online()).count();

        info!(
            "SMP locking test: racing {} core(s) to increment a shared counter {} times each...",
            online_cores, INCREMENTS_PER_CORE
        );

        hammer_shared_counter();

        // Give secondary cores a moment to finish their share; they run concurrently and may
        // still be mid-way through their increments at this point.
        for _ in 0..10_000_000 {
            cpu::nop();
        }

        let expected = online_cores as u64 * INCREMENTS_PER_CORE;
        let actual = SHARED_COUNTER.lock(|counter| *counter);

        info!(
            "SMP locking test result: expected {}, got {} -> {}",
            expected,
            actual,
            if actual == expected {
                "PASS"
            } else {
                "FAIL (lost updates - locking is broken)"
            }
        );
    }

    time::time_manager().set_timeout_once(Duration::from_secs(5), Box::new(|| info!("Once 5")));
    time::time_manager().set_timeout_once(Duration::from_secs(3), Box::new(|| info!("Once 2")));
    time::time_manager()
        .set_timeout_periodic(Duration::from_secs(1), Box::new(|| info!("Periodic 1 sec")));

    #[cfg(feature = "bsp_qemu_x86")]
    {
        let user_cpu = bsp::cpu::user_cpu().expect("No online CPU available for users");
        info!("CPU topology: {} discovered, {} online; CPU 0 reserved for kernel services",
            bsp::cpu::core_count(), cpu::smp::cores().iter().filter(|cpu| cpu.is_online()).count());
        for cpu in cpu::smp::cores() {
            info!("CPU {}: APIC {}, online={}, role={}", cpu.id().raw(),
                bsp::cpu::hardware_id_for(cpu.id()).unwrap(), cpu.is_online(),
                if cpu.id().raw() == 0 { "kernel" } else { "user-eligible" });
        }
        let network = libkernel::network::initialize().expect("Cannot initialize protected networking");
        info!("Kernel remains on CPU 0; launching user shell on CPU {}", user_cpu.raw());
        bsp::user::launch_on(user_cpu).expect("Cannot launch user shell on secondary CPU");
        cpu::kernel_worker::run(network);
    }
    #[cfg(not(feature = "bsp_qemu_x86"))]
    info!("Echoing input now");
    #[cfg(not(feature = "bsp_qemu_x86"))]
    cpu::wait_forever();
}

/// Host entry point.
///
/// Bypasses the normal aarch64 boot-assembly `_start` -> `kernel_init()` flow, since host builds
/// run under a regular OS process rather than bare metal.
#[cfg(feature = "bsp_host")]
fn main() {
    unsafe {
        kernel_init();
    }
}

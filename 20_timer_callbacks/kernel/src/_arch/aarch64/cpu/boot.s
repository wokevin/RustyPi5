// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>

//--------------------------------------------------------------------------------------------------
// Definitions
//--------------------------------------------------------------------------------------------------

// Load the address of a symbol into a register, PC-relative.
//
// The symbol must lie within +/- 4 GiB of the Program Counter.
//
// # Resources
//
// - https://sourceware.org/binutils/docs-2.36/as/AArch64_002dRelocations.html
.macro ADR_REL register, symbol
	adrp	\register, \symbol
	add	\register, \register, #:lo12:\symbol
.endm

// Load the address of a symbol into a register, absolute.
//
// # Resources
//
// - https://sourceware.org/binutils/docs-2.36/as/AArch64_002dRelocations.html
.macro ADR_ABS register, symbol
	movz	\register, #:abs_g3:\symbol
	movk	\register, #:abs_g2_nc:\symbol
	movk	\register, #:abs_g1_nc:\symbol
	movk	\register, #:abs_g0_nc:\symbol
.endm

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------
.section .text._start

//------------------------------------------------------------------------------
// fn _start()
//------------------------------------------------------------------------------
_start:
	// Only proceed if the core executes in EL2. Park it otherwise.
	mrs	x0, CurrentEL
	cmp	x0, {CONST_CURRENTEL_EL2}
	b.ne	.L_parking_loop

	// Only proceed on the boot core. Secondary cores take a different path.
	mrs	x1, MPIDR_EL1
	and	x1, x1, {CONST_CORE_ID_MASK}
	ldr	x2, BOOT_CORE_ID      // provided by bsp/__board_name__/cpu.rs
	cmp	x1, x2
	b.ne	_start_secondary

	// If execution reaches here, it is the boot core.

	// Initialize DRAM.
	ADR_REL	x0, __bss_start
	ADR_REL x1, __bss_end_exclusive

.L_bss_init_loop:
	cmp	x0, x1
	b.eq	.L_prepare_rust
	stp	xzr, xzr, [x0], #16
	b	.L_bss_init_loop

	// Prepare the jump to Rust code.
.L_prepare_rust:
	// Load the base address of the kernel's translation tables.
	ldr	x0, PHYS_KERNEL_TABLES_BASE_ADDR // provided by bsp/__board_name__/memory/mmu.rs

	// Load the _absolute_ addresses of the following symbols. Since the kernel is linked at
	// the top of the 64 bit address space, these are effectively virtual addresses.
	ADR_ABS	x1, __boot_core_stack_end_exclusive
	ADR_ABS	x2, kernel_init

	// Load the PC-relative address of the stack and set the stack pointer.
	//
	// Since _start() is the first function that runs after the firmware has loaded the kernel
	// into memory, retrieving this symbol PC-relative returns the "physical" address.
	//
	// Setting the stack pointer to this value ensures that anything that still runs in EL2,
	// until the kernel returns to EL1 with the MMU enabled, works as well. After the return to
	// EL1, the virtual address of the stack retrieved above will be used.
	ADR_REL	x3, __boot_core_stack_end_exclusive
	mov	sp, x3

	// Read the CPU's timer counter frequency and store it in ARCH_TIMER_COUNTER_FREQUENCY.
	// Abort if the frequency read back as 0.
	ADR_REL	x4, ARCH_TIMER_COUNTER_FREQUENCY // provided by aarch64/time.rs
	mrs	x5, CNTFRQ_EL0
	cmp	x5, xzr
	b.eq	.L_parking_loop
	str	w5, [x4]

	// Jump to Rust code. x0, x1 and x2 hold the function arguments provided to _start_rust().
	b	_start_rust

	// Infinitely wait for events (aka "park the core").
.L_parking_loop:
	wfe
	b	.L_parking_loop

.size	_start, . - _start
.type	_start, function
.global	_start

//------------------------------------------------------------------------------
// fn _start_secondary()
//
// Entry point for secondary cores, released via PSCI CPU_ON or the classic Raspberry Pi
// spin-table mechanism (see `cpu::smp`). At this point:
//
// - The MMU is disabled.
// - x1 already holds this core's masked MPIDR_EL1 affinity value (Aff0), computed above before
//   the branch to this label.
//------------------------------------------------------------------------------
_start_secondary:
	// Only proceed if the core executes in EL2. Park it otherwise.
	mrs	x0, CurrentEL
	cmp	x0, {CONST_CURRENTEL_EL2}
	b.ne	.L_parking_loop

	// Compute this core's physical stack top:
	//
	//   __secondary_core_stacks_start + (Aff0 * SECONDARY_CORE_STACK_SIZE)
	//
	// Aff0 for a secondary core is always >= 1 (core 0 is the boot core and never arrives here).
	ADR_REL	x2, __secondary_core_stacks_start
	mov	x3, {CONST_SECONDARY_CORE_STACK_SIZE}
	mul	x4, x1, x3
	add	x2, x2, x4
	add	x2, x2, x3          // Exclusive end of *this* core's slice.
	mov	sp, x2

	// Load the base address of the kernel's translation tables (shared with the boot core).
	ldr	x0, PHYS_KERNEL_TABLES_BASE_ADDR

	// Load the _absolute_ (virtual) addresses for the post-MMU stack and Rust entry point.
	ADR_ABS	x5, __secondary_core_stacks_start
	mul	x6, x1, x3
	add	x5, x5, x6
	add	x5, x5, x3          // Virtual, exclusive-end stack address for this core.
	ADR_ABS	x6, kernel_init_secondary

	mov	x1, x5
	mov	x2, x6

	b	_start_rust_secondary

.size	_start_secondary, . - _start_secondary
.type	_start_secondary, function
.global	_start_secondary

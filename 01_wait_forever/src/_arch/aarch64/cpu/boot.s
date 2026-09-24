// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------
.section .text._start

//------------------------------------------------------------------------------
// fn _start()
//------------------------------------------------------------------------------
_start:
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
// - Execution arrives here directly from firmware/ROM (spin-table mailbox jump or PSCI CPU_ON),
//   NOT via a fall-through from `_start`. No registers are guaranteed to hold anything
//   meaningful yet, so this core's masked MPIDR_EL1 affinity value (Aff0) must be computed here.
//------------------------------------------------------------------------------
_start_secondary:
	// Only proceed if the core executes in EL2. Park it otherwise.
	mrs	x0, CurrentEL
	cmp	x0, {CONST_CURRENTEL_EL2}
	b.ne	.L_parking_loop

	// Compute this core's masked MPIDR_EL1 affinity value (Aff0).
	mrs	x1, MPIDR_EL1
	and	x1, x1, {CONST_CORE_ID_MASK}

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

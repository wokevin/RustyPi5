# Dedicated kernel CPU and reserved memory

## Current QEMU x86 behavior

`run-qemu.ps1` now defaults to **four virtual CPUs**. It still cleans both build
targets before building and never deletes or reformats `rustypi5.img`.

At boot the kernel:

1. Checks firmware ACPI checksums and table/entry lengths.
2. Reads the executing CPU's actual xAPIC identity and validates the supported
   APIC mode/mapping. It does not assume hardware ID zero.
3. Builds a bounded table from **enabled** MADT processor entries, rejecting
   duplicates, overflow, a missing boot CPU, and unsupported enabled x2APIC entries.
   Disabled/hotplug-only processors are not treated as currently available.
4. Places the boot CPU at logical index 0 regardless of firmware entry order.
5. Starts secondary CPUs with separate bootstrap stacks. Each loads its own
   GDT/TSS and the kernel IDT before publishing an online acknowledgement.
6. Selects an online secondary CPU for the ring-3 shell. User entry explicitly
   rejects logical CPU 0 and CPUs not acknowledged online.

Logical CPU numbers are kernel indices, not APIC IDs. Boot logs show both.
Discovered and online counts are distinct. Startup errors are explicit; QEMU
startup stops on an unacknowledged secondary rather than reusing shared bootstrap
scratch with a potentially late CPU still reading it.

The boot CPU stays in kernel mode and runs a dedicated interrupt-driven service
loop after the user handoff. The one shell runs on the first online secondary.
Additional secondaries remain idle and eligible for future user tasks; their
assignment loop still polls. There is **no general scheduler, Rust async executor,
USB/HID mouse driver, or general device interrupt routing yet**.

### Kernel worker and hardware wakeups

On QEMU, IRQ masking now reads and changes the executing CPU's hardware IF flag.
Initialization keeps IRQs disabled until the IDT and local APIC are ready. The
local APIC is supervisor-only and mapped uncached. Legacy PIC interrupts and
unrelated local APIC sources are masked; this is a dedicated fixed-IPI path, not
a general IRQ-controller implementation. Vector `0xF0` acknowledges the wakeup
and increments an atomic counter; spurious vector `0xFF` returns without EOI.
Neither handler prints, allocates, nor acquires the work-queue lock.

A supervisor-owned, allocation-free FIFO holds at most 64 typed work items.
Submission publishes work under a cross-core spinlock before sending an IPI to
the discovered boot APIC ID. Full queues reject explicitly and count rejections;
a notification failure after acceptance is fatal rather than pretending work
was rejected or safely delivered. CPU 0 drains the queue with interrupts enabled.
It disables interrupts for the empty check and uses one `sti; hlt` sequence to
avoid a lost-wakeup window. Console and heap access use cross-core spinlocks on
QEMU; the existing timer queue is not connected to this worker.

Before entering ring 3, CPU 1 verifies 16 sequential work completions, each
submitted after CPU 0 publishes its idle transition, and requires the hardware
wake counter to advance for every submission. Boot also checks masked, enabled,
and nested IRQ save/restore behavior on CPU 0. Current work is a diagnostic probe,
not USB processing or a task executor. This establishes the wake/defer mechanism
that later device work and useful async scheduling can build on.

A single-CPU boot is deliberately refused with a diagnostic instead of silently
running users on the kernel CPU:

```powershell
.\run-qemu.ps1 -Smp 1
```

Use the default or `-Smp 2`/`-Smp 4` for a working shell. These are QEMU virtual
CPUs scheduled by Windows, not exclusive ownership of host physical cores.
QEMU tests cover one, two, and four CPUs. Physical x86 hardware/SMT topology is
not validated: a logical processor reservation must not be described as reserving
both SMT siblings of a physical core.

## Separate kernel memory

Stacks are separate memory regions, not a way to make memory unlimited:

| Resource | Current reservation |
|---|---|
| Boot CPU kernel stack | 128 KiB allocated by the UEFI shim |
| Secondary bootstrap/kernel stack | 128 KiB per supported secondary, page-aligned |
| Supervisor syscall-entry stack | 32 KiB per supported CPU, page-aligned, own TSS |
| Kernel heap | 1 MiB page-aligned static backing, replacing the dummy address range |
| Demo user stack | 16 KiB, user-readable/writable, non-executable |
| User stack guards | One unmapped 4 KiB page on each side |

The x86 capacity remains 64 CPUs, so static reservations include stacks for
currently absent CPUs. This costs about 8 MiB for secondary stacks plus 2 MiB for
supervisor-entry stacks. It is intentionally bounded; a future physical frame
allocator can reserve on demand without taking memory from users.

The image loader reserves kernel static storage as part of ELF load segments;
these regions are not borrowed from the user region. The AP bootstrap scratch
page at physical `0x7000` is now also represented in the ELF memory layout so the
loader reserves it before exiting firmware boot services. The trampoline lives
at `0x8000` as before.

Kernel heap, stacks, and descriptor tables remain supervisor-only in the shared
page tables. User text is read-only/executable; the user stack is writable/NX.
The page tables are prepared once by the boot CPU before secondary startup, not
rebuilt concurrently by user CPUs. Each CPU enables its own NX/write-protection
state before entering the user image.

There is currently **no user memory-allocation syscall**, so users cannot consume
this kernel heap through a user allocator. Future user allocation must use a
separate pool/budget and enforce limits before granting memory. This change does
not yet implement user quotas, an emergency reserve, reclamation, or out-of-memory
recovery. Kernel code can still exhaust its own fixed heap. Kernel stacks are
reserved and separate but do not yet have unmapped guards or dedicated double-fault
stacks; this is not a claim of comprehensive memory-exhaustion/overflow safety.

Syscalls and faults still run privileged code locally on the user CPU, using its
own supervisor entry stack. The reserved CPU is for independent kernel services,
not a claim that no kernel instruction ever executes elsewhere. Current filesystem
and console syscalls are still synchronous on the calling CPU.

## Hardware boundaries

This milestone is validated on **QEMU x86**, not physical Pi 5. The Pi BSP still
uses a fixed candidate CPU list and an unverified firmware release convention.
Real Pi 5 work requires passing/parsing its device tree, verifying its CPU release
method, and wiring its interrupt controller and RP1/USB platform support. It is
not enabled or claimed by this change. Hyper-V uses shared x86 code but has not
been runtime-tested here.

Only QEMU's reserved CPU enables hardware interrupts for the worker. The shell
CPU still enters ring 3 with IF clear, and its synchronous input syscall polls
PS/2 and serial input. The generic x86 IRQ manager and timer interrupt support
remain stubs. Other BSP IRQ-mask behavior is unchanged. Device interrupt routing,
DMA ownership, USB enumeration and an async executor must be implemented and
validated separately; this milestone does not make those paths operational.

## Validation

From `20_timer_callbacks`, clean-build and prepare the disk first:

```powershell
.\run-qemu.ps1 -PrepareOnly
python .\tools\cpu_boot_smoke.py
```

The boot test starts its own headless QEMU instances, checks monitor responsiveness,
confirms topology/CPU placement, checks live per-CPU TSS and entry-stack addresses,
walks guest page tables for kernel/user permissions, user-stack guards and uncached
APIC access, requires 16 cross-core hardware wakeups, checks that CPU 0 sleeps
privileged with IF enabled, types filesystem commands through the emulated PS/2
keyboard, then shuts down each VM.
It checks that the persistent disk image is unchanged. Logs are retained under
`target\cpu-boot-tests`; temporary firmware variable copies are removed.

Topology parser/policy tests compile the same source without the kernel harness:

```powershell
New-Item -ItemType Directory -Force .\target\topology-tests | Out-Null
Remove-Item .\target\topology-tests\topology-tests.exe -ErrorAction SilentlyContinue
Remove-Item .\target\topology-tests\topology-tests.pdb -ErrorAction SilentlyContinue
rustc --edition=2021 --test .\tools\topology_tests.rs -o .\target\topology-tests\topology-tests.exe
.\target\topology-tests\topology-tests.exe
```

Tests cover nonzero boot IDs, sparse/unordered IDs, disabled/hotplug-only entries,
duplicates, capacity overflow, malformed entries, unsupported x2APIC, offline CPUs,
and refusal to place users on the dedicated kernel CPU.

Queue tests compile the kernel's queue source directly:

```powershell
New-Item -ItemType Directory -Force .\target\interrupt-tests | Out-Null
Remove-Item .\target\interrupt-tests\interrupt-tests.exe -ErrorAction SilentlyContinue
Remove-Item .\target\interrupt-tests\interrupt-tests.pdb -ErrorAction SilentlyContinue
rustc --edition=2021 --test .\tools\interrupt_tests.rs -o .\target\interrupt-tests\interrupt-tests.exe
.\target\interrupt-tests\interrupt-tests.exe
```

These cover empty/FIFO behavior, repeated wraparound, full-queue rejection without
losing accepted work, rejection accounting, and zero-capacity safety. Host tests
do not execute privileged interrupt instructions; the live QEMU checks do.

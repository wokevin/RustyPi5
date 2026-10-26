/* Linker script for the Hyper-V x86_64 BSP.
 *
 * The 16-bit real-mode AP trampoline (`bsp::hyperv_x86::trampoline`) is position-*dependent*:
 * its assembly's internal label/offset relocations are only valid if the code is linked (and
 * ultimately executed, after `release_core()` copies it there at runtime) at physical 0x8000
 * (TRAMPOLINE_PHYS_ADDR). Everything else in the kernel links at 0x800000 (8 MiB).
 *
 * NOTE: this previously used 0x200000 (2 MiB), then 0x400000 (4 MiB), both of which turned out to
 * be persistently occupied by EfiBootServicesData per the UEFI memory map on this Hyper-V/OVMF
 * configuration (confirmed via a shim-side diagnostic that reports the memory type covering each
 * PT_LOAD segment's fixed address before attempting AllocateAddress) -- not something freed by
 * relocating the kernel's own ELF/staging buffers, but firmware-reserved for its own use (commonly
 * SMM/runtime-services-adjacent structures on some platforms). 0x800000 was chosen after the shim
 * was extended to dump the *entire* UEFI memory map's EfiConventionalMemory (free) regions once at
 * boot, before any allocations could consume/split one: it reported a confirmed-free region
 * spanning physical 0x72A000..0x43F6000 (~60 MiB), comfortably containing this page-aligned
 * address with room well beyond the kernel's current ~1.2 MiB image size. If this also turns out
 * to collide (e.g. on different firmware/host), re-run with that same diagnostic (the
 * "conventional (free) memory regions" dump in efi_main, plus report_memory_type_at per segment)
 * to find a confirmed-free region instead of guessing again.
 *
 * A prior attempt using `INSERT AFTER .text` with just the trampoline section failed: setting
 * an explicit low VMA via INSERT permanently moves the linker's location counter backward with
 * no way to reset it for subsequent default sections, causing a bogus (wrapped/negative) total
 * image size. This script instead fully replaces the default layout with two explicit memory
 * regions, so there is no ambiguity about section ordering or the location counter.
 */

/* QEMU's EDK2 firmware has a different low-memory layout. The launcher overrides
 * the kernel base without changing the default Hyper-V layout or the SIPI vector.
 */
MEMORY
{
	TRAMPOLINE (rwx) : ORIGIN = 0x8000, LENGTH = 4K
	RAM        (rwx) : ORIGIN = DEFINED(__kernel_load_addr) ? __kernel_load_addr : 0x800000, LENGTH = 32M
}

ENTRY(_start)

SECTIONS
{
	.text.ap_trampoline : {
		KEEP(*(.text.ap_trampoline))
	} > TRAMPOLINE

	.text : {
		*(.text .text.*)
	} > RAM

	.rodata : {
		*(.rodata .rodata.*)
	} > RAM

	.got : {
		*(.got .got.*)
	} > RAM

	.data : {
		*(.data .data.*)
	} > RAM

	.bss : {
		*(.bss .bss.*)
		*(COMMON)
	} > RAM

	.eh_frame : {
		KEEP(*(.eh_frame))
	} > RAM

	.comment 0 : { *(.comment) }
	.debug_abbrev 0 : { *(.debug_abbrev) }
	.debug_info 0 : { *(.debug_info) }
	.debug_aranges 0 : { *(.debug_aranges) }
	.debug_ranges 0 : { *(.debug_ranges) }
	.debug_str 0 : { *(.debug_str) }
	.debug_pubnames 0 : { *(.debug_pubnames) }
	.debug_pubtypes 0 : { *(.debug_pubtypes) }
	.debug_frame 0 : { *(.debug_frame) }
	.debug_line 0 : { *(.debug_line) }
	.debug_loc 0 : { *(.debug_loc) }

	/DISCARD/ : {
		*(.note.*)
		*(.comment)
	}
}

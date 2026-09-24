
#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>
# Python port: Ruby dependency removed.

"""ELF introspection helpers for the kernel binary."""

from elftools.elf.elffile import ELFFile
from elftools.elf.constants import SH_FLAGS

from generic import AttributeFields, MappingDescriptor, MemoryRegion, align_up

SECTION_FLAG_ALLOC = SH_FLAGS.SHF_ALLOC

PF_X = 0x1
PF_W = 0x2
PF_R = 0x4


class KernelELF:
    def __init__(self, kernel_elf_path: str):
        self._file = open(kernel_elf_path, "rb")
        self._elf = ELFFile(self._file)
        self._symtab = self._elf.get_section_by_name(".symtab")

    def machine(self) -> str:
        return self._elf.get_machine_arch()

    def symbol_value(self, symbol_name: str) -> int:
        symbols = self._symtab.get_symbol_by_name(symbol_name)
        if not symbols:
            raise KeyError(f"Symbol not found: {symbol_name}")
        return symbols[0]["st_value"]

    def _segment_containing_virt_addr(self, virt_addr: int):
        for segment in self._elf.iter_segments():
            header = segment.header
            if header["p_type"] != "PT_LOAD":
                continue
            if header["p_vaddr"] <= virt_addr < header["p_vaddr"] + header["p_memsz"]:
                return segment
        raise LookupError(f"No segment contains virtual address {hex(virt_addr)}")

    def virt_to_phys(self, virt_addr: int) -> int:
        segment = self._segment_containing_virt_addr(virt_addr)
        header = segment.header
        translation_offset = header["p_vaddr"] - header["p_paddr"]

        return virt_addr - translation_offset

    def virt_addr_to_file_offset(self, virt_addr: int) -> int:
        segment = self._segment_containing_virt_addr(virt_addr)
        header = segment.header

        return header["p_offset"] + (virt_addr - header["p_vaddr"])

    def _sections_in_segment(self, segment) -> str:
        header = segment.header
        head = header["p_vaddr"]
        tail = head + header["p_memsz"]

        names = []
        for section in self._elf.iter_sections():
            addr = section["sh_addr"]
            flags = section["sh_flags"]

            if head <= addr < tail and (flags & SECTION_FLAG_ALLOC) != 0:
                names.append(section.name)

        return " ".join(names)

    def _select_load_segments(self):
        return [seg for seg in self._elf.iter_segments() if seg.header["p_type"] == "PT_LOAD"]

    @staticmethod
    def _segment_get_acc_perms(segment) -> str:
        flags = segment.header["p_flags"]
        readable = bool(flags & PF_R)
        writable = bool(flags & PF_W)

        if readable and writable:
            return "ReadWrite"
        if readable:
            return "ReadOnly"
        return "Invalid"

    def generate_mapping_descriptors(self, bsp):
        descriptors = []

        for segment in self._select_load_segments():
            header = segment.header

            # Assume each segment is page aligned.
            size = align_up(header["p_memsz"], bsp.kernel_granule_size())
            virt_start_addr = header["p_vaddr"]
            phys_start_addr = header["p_paddr"]
            acc_perms = self._segment_get_acc_perms(segment)
            execute_never = (header["p_flags"] & PF_X) == 0
            section_names = self._sections_in_segment(segment)

            virt_region = MemoryRegion(virt_start_addr, size, bsp.kernel_granule_size())
            phys_region = MemoryRegion(phys_start_addr, size, bsp.kernel_granule_size())
            attributes = AttributeFields("CacheableDRAM", acc_perms, execute_never)

            descriptors.append(MappingDescriptor(section_names, virt_region, phys_region, attributes))

        MappingDescriptor.update_max_section_name_length(max(len(d.name) for d in descriptors))

        return descriptors

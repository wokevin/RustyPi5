#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>
# Python port: Ruby dependency removed.

"""Generic (architecture-agnostic) translation table tool helpers."""

import math


class Granule64KiB:
    SIZE = 64 * 1024
    SHIFT = int(math.log2(SIZE))


class Granule512MiB:
    SIZE = 512 * 1024 * 1024
    SHIFT = int(math.log2(SIZE))
    MASK = SIZE - 1


def is_power_of_two(value: int) -> bool:
    return value != 0 and (value & (value - 1)) == 0


def is_aligned(value: int, alignment: int) -> bool:
    assert is_power_of_two(alignment)
    return (value & (alignment - 1)) == 0


def align_up(value: int, alignment: int) -> int:
    assert is_power_of_two(alignment)
    return (value + alignment - 1) & ~(alignment - 1)


def to_hex_underscore(value: int, with_leading_zeros: bool = False) -> str:
    fmt = "{:016x}" if with_leading_zeros else "{:x}"
    text = fmt.format(value)

    # Group into 4-hex-digit chunks from the right, joined by '_'.
    reversed_text = text[::-1]
    chunks = [reversed_text[i : i + 4] for i in range(0, len(reversed_text), 4)]
    grouped = "_".join(chunks)[::-1]

    return "0x" + grouped


class MemoryRegion(list):
    """A list where each value is the start address of a page."""

    def __init__(self, start_addr: int, size: int, granule_size: int):
        assert is_aligned(start_addr, granule_size)
        assert size > 0
        assert (size % granule_size) == 0

        num_pages = size // granule_size
        super().__init__((i * granule_size) + start_addr for i in range(num_pages))


class AttributeFields:
    """Collection of memory attributes."""

    def __init__(self, mem_attributes: str, acc_perms: str, execute_never: bool):
        self.mem_attributes = mem_attributes
        self.acc_perms = acc_perms
        self.execute_never = execute_never

    def __str__(self) -> str:
        x = {"CacheableDRAM": "C"}.get(self.mem_attributes, "?")
        y = {"ReadWrite": "RW", "ReadOnly": "RO"}.get(self.acc_perms, "??")
        z = "XN" if self.execute_never else "X "

        return f"{x} {y} {z}"


class MappingDescriptor:
    """A container that describes a virt-to-phys region mapping."""

    _max_section_name_length = len("Sections")

    def __init__(self, name, virt_region, phys_region, attributes):
        self.name = name
        self.virt_region = virt_region
        self.phys_region = phys_region
        self.attributes = attributes

    @classmethod
    def update_max_section_name_length(cls, length: int) -> None:
        cls._max_section_name_length = max(cls._max_section_name_length, length)

    @staticmethod
    def _size_human_readable(size: int) -> str:
        if size >= (1024 * 1024):
            return f"{(size // (1024 * 1024)):>3} MiB"
        if size >= 1024:
            return f"{(size // 1024):>3} KiB"
        raise ValueError("Size too small to represent")

    def __str__(self) -> str:
        name = self.name.ljust(self._max_section_name_length)
        virt_start = to_hex_underscore(self.virt_region[0], with_leading_zeros=True)
        phys_start = to_hex_underscore(self.phys_region[0], with_leading_zeros=True)
        size = self._size_human_readable(len(self.virt_region) * 65_536)

        return f"{name} | {virt_start} | {phys_start} | {size} | {self.attributes}"

    @classmethod
    def print_divider(cls) -> None:
        print("             ", end="")
        print("-" * cls._max_section_name_length, end="")
        print("--------------------------------------------------------------------")

    @classmethod
    def print_header(cls) -> None:
        cls.print_divider()
        print("             ", end="")
        print("Sections".center(cls._max_section_name_length), end="")
        print("   ", end="")
        print("Virt Start Addr".center(21), end="")
        print("   ", end="")
        print("Phys Start Addr".center(21), end="")
        print("   ", end="")
        print("Size".center(7), end="")
        print("   ", end="")
        print("Attr".center(7))
        cls.print_divider()


def kernel_map_binary(kernel_elf, bsp, translation_tables) -> None:
    mapping_descriptors = kernel_elf.generate_mapping_descriptors(bsp)

    # generate_mapping_descriptors() updates the header being printed with this call, so it must
    # come afterwards.
    MappingDescriptor.print_header()

    for descriptor in mapping_descriptors:
        print("Generating".rjust(12), end="")
        print(" ", end="")
        print(descriptor)

        translation_tables.map_at(descriptor.virt_region, descriptor.phys_region, descriptor.attributes)

    MappingDescriptor.print_divider()


def kernel_patch_tables(kernel_elf_path: str, bsp, translation_tables) -> None:
    print("Patching".rjust(12), end="")
    print(" Kernel table struct at ELF file offset ", end="")
    print(to_hex_underscore(bsp.kernel_tables_offset_in_file()))

    _binwrite(kernel_elf_path, translation_tables.to_binary(), bsp.kernel_tables_offset_in_file())


def kernel_patch_base_addr(kernel_elf_path: str, bsp, translation_tables) -> None:
    print("Patching".rjust(12), end="")
    print(" Kernel tables physical base address start argument to value ", end="")
    print(to_hex_underscore(translation_tables.phys_tables_base_addr()), end="")
    print(" at ELF file offset ", end="")
    print(to_hex_underscore(bsp.phys_kernel_tables_base_addr_offset_in_file()))

    _binwrite(
        kernel_elf_path,
        translation_tables.phys_tables_base_addr_binary(),
        bsp.phys_kernel_tables_base_addr_offset_in_file(),
    )


def _binwrite(path: str, data: bytes, offset: int) -> None:
    with open(path, "r+b") as f:
        f.seek(offset)
        f.write(data)

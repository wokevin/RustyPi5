

#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>
# Python port: Ruby dependency removed.

"""ARMv8-A translation table construction, matching translation_table.rs."""

import struct

from generic import Granule64KiB, Granule512MiB


class BitField:
    def __init__(self):
        self._value = 0

    def to_int(self) -> int:
        return self._value

    def _set_field(self, offset: int, num_bits: int, bits: int) -> None:
        mask = (1 << num_bits) - 1
        if bits & ~mask:
            raise ValueError(f"Input out of range: 0x{bits:x}")

        self._value &= ~(mask << offset)
        self._value |= bits << offset


class Stage1TableDescriptor(BitField):
    class Type:
        BLOCK = 0
        TABLE = 1

    class Valid:
        FALSE = 0
        TRUE = 1

    def set_next_level_table_addr(self, addr: int) -> None:
        addr >>= Granule64KiB.SHIFT
        self._set_field(16, 32, addr)

    def set_type(self, value: int) -> None:
        self._set_field(1, 1, value)

    def set_valid(self, value: int) -> None:
        self._set_field(0, 1, value)


class Stage1PageDescriptor(BitField):
    class SH:
        INNER_SHAREABLE = 0b11

    class AP:
        RW_EL1 = 0b00
        RO_EL1 = 0b10

    class Type:
        RESERVED_INVALID = 0
        PAGE = 1

    class Valid:
        FALSE = 0
        TRUE = 1

    def set_uxn(self, value: int) -> None:
        self._set_field(54, 1, value)

    def set_pxn(self, value: int) -> None:
        self._set_field(53, 1, value)

    def set_output_addr(self, addr: int) -> None:
        addr >>= Granule64KiB.SHIFT
        self._set_field(16, 32, addr)

    def set_af(self, value: int) -> None:
        self._set_field(10, 1, value)

    def set_sh(self, value: int) -> None:
        self._set_field(8, 2, value)

    def set_ap(self, value: int) -> None:
        self._set_field(6, 2, value)

    def set_attr_indx(self, value: int) -> None:
        self._set_field(2, 3, value)

    def set_type(self, value: int) -> None:
        self._set_field(1, 1, value)

    def set_valid(self, value: int) -> None:
        self._set_field(0, 1, value)


class MAIR:
    NORMAL = 1


class TranslationTable:
    """Translation table representing the structure defined in translation_table.rs."""

    def __init__(self, bsp):
        self._bsp = bsp
        self._do_sanity_checks()

        num_lvl2_tables = bsp.kernel_virt_addr_space_size() >> Granule512MiB.SHIFT

        self._lvl3_phys_start_addr = bsp.phys_addr_of_kernel_tables()
        self._lvl3 = [
            [Stage1PageDescriptor() for _ in range(8192)] for _ in range(num_lvl2_tables)
        ]
        lvl3_size_in_bytes = num_lvl2_tables * 8192 * 8

        self._lvl2_phys_start_addr = self._lvl3_phys_start_addr + lvl3_size_in_bytes
        self._lvl2 = [Stage1TableDescriptor() for _ in range(num_lvl2_tables)]

        self._populate_lvl2_entries(num_lvl2_tables)

    def map_at(self, virt_region, phys_region, attributes) -> None:
        if not virt_region:
            return

        if len(virt_region) != len(phys_region):
            raise ValueError("virt_region and phys_region sizes differ")
        if phys_region[-1] > self._bsp.phys_addr_space_end_page():
            raise ValueError("phys_region exceeds physical address space")

        for virt_page, phys_page in zip(virt_region, phys_region):
            desc = self._page_descriptor_from(virt_page)
            self._set_lvl3_entry(desc, phys_page, attributes)

    def to_binary(self) -> bytes:
        values = []
        for table in self._lvl3:
            for desc in table:
                values.append(desc.to_int())
        for desc in self._lvl2:
            values.append(desc.to_int())

        return struct.pack(f"<{len(values)}Q", *values)

    def phys_tables_base_addr_binary(self) -> bytes:
        return struct.pack("<Q", self._lvl2_phys_start_addr)

    def phys_tables_base_addr(self) -> int:
        return self._lvl2_phys_start_addr

    def _do_sanity_checks(self) -> None:
        if self._bsp.kernel_granule_size() != Granule64KiB.SIZE:
            raise ValueError("Kernel granule size mismatch")
        if (self._bsp.kernel_virt_addr_space_size() % Granule512MiB.SIZE) != 0:
            raise ValueError("Kernel virtual address space size not 512 MiB aligned")

    def _populate_lvl2_entries(self, num_lvl2_tables: int) -> None:
        lvl3_table_size_in_bytes = 8192 * 8

        for i, descriptor in enumerate(self._lvl2):
            phys_addr = self._lvl3_phys_start_addr + (i * lvl3_table_size_in_bytes)
            descriptor.set_next_level_table_addr(phys_addr)
            descriptor.set_type(Stage1TableDescriptor.Type.TABLE)
            descriptor.set_valid(Stage1TableDescriptor.Valid.TRUE)

    def _lvl2_lvl3_index_from(self, addr: int):
        addr -= self._bsp.kernel_virt_start_addr()

        lvl2_index = addr >> Granule512MiB.SHIFT
        lvl3_index = (addr & Granule512MiB.MASK) >> Granule64KiB.SHIFT

        if lvl2_index >= len(self._lvl2):
            raise ValueError("Virtual address out of range for translation table")

        return lvl2_index, lvl3_index

    def _page_descriptor_from(self, virt_addr: int) -> Stage1PageDescriptor:
        lvl2_index, lvl3_index = self._lvl2_lvl3_index_from(virt_addr)

        return self._lvl3[lvl2_index][lvl3_index]

    def _set_attributes(self, desc: Stage1PageDescriptor, attributes) -> None:
        if attributes.mem_attributes == "CacheableDRAM":
            desc.set_sh(Stage1PageDescriptor.SH.INNER_SHAREABLE)
            desc.set_attr_indx(MAIR.NORMAL)
        else:
            raise ValueError("Invalid mem_attributes")

        if attributes.acc_perms == "ReadOnly":
            desc.set_ap(Stage1PageDescriptor.AP.RO_EL1)
        elif attributes.acc_perms == "ReadWrite":
            desc.set_ap(Stage1PageDescriptor.AP.RW_EL1)
        else:
            raise ValueError("Invalid acc_perms")

        desc.set_pxn(1 if attributes.execute_never else 0)
        desc.set_uxn(1)

    def _set_lvl3_entry(self, desc: Stage1PageDescriptor, output_addr: int, attributes) -> None:
        desc.set_output_addr(output_addr)
        desc.set_af(1)
        desc.set_type(Stage1PageDescriptor.Type.PAGE)
        desc.set_valid(Stage1PageDescriptor.Valid.TRUE)

        self._set_attributes(desc, attributes)

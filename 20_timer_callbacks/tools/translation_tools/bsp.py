

#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>
# Python port: Ruby dependency removed.

"""Board Support Package helpers for the translation table tool."""

import re

from generic import Granule64KiB

_MEMORY_RS_PATH = "kernel/src/bsp/raspberrypi/memory.rs"


class RaspberryPi:
    def __init__(self, bsp_type: str, kernel_elf):
        self._bsp_type = bsp_type
        self._kernel_elf = kernel_elf

        with open(_MEMORY_RS_PATH, encoding="utf-8") as f:
            self._memory_src = f.read().splitlines()

        self._kernel_virt_addr_space_size = kernel_elf.symbol_value("__kernel_virt_addr_space_size")
        self._kernel_virt_start_addr = kernel_elf.symbol_value("__kernel_virt_start_addr")

        self._virt_addr_of_kernel_tables = kernel_elf.symbol_value("KERNEL_TABLES")
        self._virt_addr_of_phys_kernel_tables_base_addr = kernel_elf.symbol_value(
            "PHYS_KERNEL_TABLES_BASE_ADDR"
        )

    def kernel_granule_size(self) -> int:
        return Granule64KiB.SIZE

    def kernel_virt_addr_space_size(self) -> int:
        return self._kernel_virt_addr_space_size

    def kernel_virt_start_addr(self) -> int:
        return self._kernel_virt_start_addr

    def phys_addr_of_kernel_tables(self) -> int:
        return self._kernel_elf.virt_to_phys(self._virt_addr_of_kernel_tables)

    def kernel_tables_offset_in_file(self) -> int:
        return self._kernel_elf.virt_addr_to_file_offset(self._virt_addr_of_kernel_tables)

    def phys_kernel_tables_base_addr_offset_in_file(self) -> int:
        return self._kernel_elf.virt_addr_to_file_offset(
            self._virt_addr_of_phys_kernel_tables_base_addr
        )

    def phys_addr_space_end_page(self) -> int:
        matches = [line for line in self._memory_src if "pub const END" in line]

        index = {"rpi3": 0, "rpi4": 1, "rpi5": 2}.get(self._bsp_type)
        if index is None:
            raise ValueError(f"Unsupported BSP type: {self._bsp_type}")

        line = matches[index]

        hex_match = re.search(r"0x[0-9A-Fa-f_]*", line)
        if hex_match is None:
            raise ValueError(f"Could not find hex literal in line: {line}")

        hex_digits = hex_match.group(0).replace("_", "").replace("0x", "")

        return int(hex_digits, 16)

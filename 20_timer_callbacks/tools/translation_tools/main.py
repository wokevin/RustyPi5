#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Copyright (c) 2021-2023 Andre Richter <andre.o.richter@gmail.com>
# Python port: Ruby dependency removed.

"""Entry point for the translation table tool. Requires: pip install pyelftools"""

import sys
import time

from arch import TranslationTable
from bsp import RaspberryPi
from generic import kernel_map_binary, kernel_patch_base_addr, kernel_patch_tables
from kernel_elf import KernelELF


def main() -> int:
    bsp_type = sys.argv[1]
    kernel_elf_path = sys.argv[2]

    start = time.time()

    kernel_elf = KernelELF(kernel_elf_path)

    if bsp_type not in ("rpi3", "rpi4", "rpi5"):
        raise ValueError(f"Unsupported BSP type: {bsp_type}")

    bsp = RaspberryPi(bsp_type, kernel_elf)

    if kernel_elf.machine() != "AArch64":
        raise ValueError(f"Unsupported machine type: {kernel_elf.machine()}")

    translation_tables = TranslationTable(bsp)

    kernel_map_binary(kernel_elf, bsp, translation_tables)
    kernel_patch_tables(kernel_elf_path, bsp, translation_tables)
    kernel_patch_base_addr(kernel_elf_path, bsp, translation_tables)

    elapsed = time.time() - start

    print("Finished".rjust(12), end="")
    print(f" in {elapsed:.2f}s")

    return 0


if __name__ == "__main__":
    sys.exit(main())

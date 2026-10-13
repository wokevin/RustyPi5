## SPDX-License-Identifier: MIT OR Apache-2.0
##
## Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

include ../common/format.mk
include ../common/operating_system.mk

##--------------------------------------------------------------------------------------------------
## Optional, user-provided configuration values
##--------------------------------------------------------------------------------------------------

# Default to the RPi3.
BSP ?= rpi3

# Default to a serial device name that is common in Linux.
DEV_SERIAL ?= /dev/ttyUSB0

# Optional debug prints.
ifdef DEBUG_PRINTS
    FEATURES = --features debug_prints
endif

# Optional: use PSCI CPU_ON instead of the classic spin-table release mechanism on Pi 3/4 builds.
# Needed for QEMU (raspi3b/raspi4b), whose emulated firmware implements PSCI, not the real
# hardware's ROM spin-table polling loop.
ifdef QEMU_PSCI
    FEATURES += --features qemu
endif

# Optional integration test name.
ifdef TEST
    TEST_ARG = --test $(TEST)
else
    TEST_ARG = --test '*'
endif

# Default to single core unless overridden.
QEMU_SMP ?= 1


##--------------------------------------------------------------------------------------------------
## BSP-specific configuration values
##--------------------------------------------------------------------------------------------------
QEMU_MISSING_STRING = "This board is not yet supported for QEMU."

ifeq ($(BSP),rpi3)
    TARGET            = aarch64-unknown-none-softfloat
    KERNEL_BIN        = kernel8.img
    QEMU_BINARY       = qemu-system-aarch64
    QEMU_MACHINE_TYPE = raspi3b
    QEMU_RELEASE_ARGS = -serial stdio -display none
    QEMU_TEST_ARGS    = $(QEMU_RELEASE_ARGS) -semihosting
    OBJDUMP_BINARY    = aarch64-none-elf-objdump
    NM_BINARY         = aarch64-none-elf-nm
    READELF_BINARY    = aarch64-none-elf-readelf
    OPENOCD_ARG       = -f /openocd/tcl/interface/ftdi/ Olimex-arm-usb-tiny-h.cfg -f /openocd/rpi3.cfg
    JTAG_BOOT_IMAGE   = ../X1_JTAG_boot/jtag_boot_rpi3.img
    LD_SCRIPT_PATH    = $(shell pwd)/kernel/src/bsp/raspberrypi
    RUSTC_MISC_ARGS   = -C target-cpu=cortex-a53 -C force-frame-pointers
else ifeq ($(BSP),rpi4)
    TARGET            = aarch64-unknown-none-softfloat
    KERNEL_BIN        = kernel8.img
    QEMU_BINARY       = qemu-system-aarch64
    QEMU_MACHINE_TYPE =
    QEMU_RELEASE_ARGS = -serial stdio -display none
    QEMU_TEST_ARGS    = $(QEMU_RELEASE_ARGS) -semihosting
    OBJDUMP_BINARY    = aarch64-none-elf-objdump
    NM_BINARY         = aarch64-none-elf-nm
    READELF_BINARY    = aarch64-none-elf-readelf
    OPENOCD_ARG       = -f /openocd/tcl/interface/ftdi/ Olimex-arm-usb-tiny-h.cfg -f /openocd/rpi4.cfg
    JTAG_BOOT_IMAGE   = ../X1_JTAG_boot/jtag_boot_rpi4.img
    LD_SCRIPT_PATH    = $(shell pwd)/kernel/src/bsp/raspberrypi
    RUSTC_MISC_ARGS   = -C target-cpu=cortex-a72 -C force-frame-pointers
else ifeq ($(BSP),rpi5)
    TARGET            = aarch64-unknown-none-softfloat
    KERNEL_BIN        = kernel_2712.img
    QEMU_BINARY       = $(QEMU_MISSING_STRING)
    QEMU_MACHINE_TYPE =
    QEMU_RELEASE_ARGS = -serial stdio -display none
    QEMU_TEST_ARGS    = $(QEMU_RELEASE_ARGS) -semihosting
    OBJDUMP_BINARY    = aarch64-none-elf-objdump
    NM_BINARY         = aarch64-none-elf-nm
    READELF_BINARY    = aarch64-none-elf-readelf
    OPENOCD_ARG       = -f /openocd/tcl/interface/ftdi/ Olimex-arm-usb-tiny-h.cfg -f /openocd/rpi5.cfg
    JTAG_BOOT_IMAGE   = ../X1_JTAG_boot/jtag_boot_rpi5.img
    LD_SCRIPT_PATH    = $(shell pwd)/kernel/src/bsp/raspberrypi
    RUSTC_MISC_ARGS   = -C target-cpu=cortex-a76 -C force-frame-pointers
endif

# Export for build.rs.
export LD_SCRIPT_PATH



##--------------------------------------------------------------------------------------------------
## Targets and Prerequisites
##--------------------------------------------------------------------------------------------------
KERNEL_MANIFEST      = kernel/Cargo.toml
KERNEL_LINKER_SCRIPT = kernel.ld
LAST_BUILD_CONFIG    = target/$(BSP)_$(DEBUG_PRINTS).build_config

KERNEL_ELF_RAW      = target/$(TARGET)/release/RustyPi5
# This parses cargo's dep-info file.
# https://doc.rust-lang.org/cargo/guide/build-cache.html#dep-info-files
KERNEL_ELF_RAW_DEPS = $(filter-out %: ,$(file < $(KERNEL_ELF_RAW).d)) $(KERNEL_MANIFEST) $(LAST_BUILD_CONFIG)

##------------------------------------------------------------------------------
## Translation tables: precompute the kernel translation tables and patch them
## into the kernel ELF (Python port of the legacy Ruby tool; no Ruby dependency)
##------------------------------------------------------------------------------
TT_TOOL_PATH = tools/translation_tools

KERNEL_ELF_TTABLES      = $(KERNEL_ELF_RAW)_ttables
KERNEL_ELF_TTABLES_DEPS = $(KERNEL_ELF_RAW) $(KERNEL_ELF_RAW_DEPS) \
    $(TT_TOOL_PATH)/main.py $(TT_TOOL_PATH)/generic.py \
    $(TT_TOOL_PATH)/kernel_elf.py $(TT_TOOL_PATH)/bsp.py $(TT_TOOL_PATH)/arch.py

KERNEL_ELF = $(KERNEL_ELF_TTABLES)



##--------------------------------------------------------------------------------------------------
## Command building blocks
##--------------------------------------------------------------------------------------------------
RUSTFLAGS = $(RUSTC_MISC_ARGS)                   \
    -C link-arg=--library-path=$(LD_SCRIPT_PATH) \
    -C link-arg=--script=$(KERNEL_LINKER_SCRIPT)

RUSTFLAGS_PEDANTIC = $(RUSTFLAGS) \
    -D warnings                   \
    -D missing_docs

FEATURES     += --features bsp_$(BSP)
COMPILER_ARGS = --target=$(TARGET) \
    $(FEATURES)                    \
    --release

# build-std can be skipped for helper commands that do not rely on correct stack frames and other
# custom compiler options. This results in a huge speedup.
RUSTC_CMD   = cargo rustc $(COMPILER_ARGS) -Z build-std=core,alloc --manifest-path $(KERNEL_MANIFEST)
DOC_CMD     = cargo doc $(COMPILER_ARGS)
CLIPPY_CMD  = cargo clippy $(COMPILER_ARGS)
TEST_CMD    = cargo test $(COMPILER_ARGS) -Z build-std=core,alloc --manifest-path $(KERNEL_MANIFEST)
OBJCOPY_CMD = rust-objcopy \
    --strip-all            \
    -O binary

EXEC_QEMU          = $(QEMU_BINARY) -M $(QEMU_MACHINE_TYPE) -smp $(QEMU_SMP)
EXEC_TEST_DISPATCH = python3 ../common/tests/dispatch.py
EXEC_MINIPUSH      = python3 ../common/serial/minipush.py
EXEC_TT_TOOL       = .venv/bin/python3 $(TT_TOOL_PATH)/main.py



##--------------------------------------------------------------------------------------------------
## Targets
##--------------------------------------------------------------------------------------------------
.PHONY: all doc qemu chainboot clippy clean readelf objdump nm check

all: $(KERNEL_BIN)

##------------------------------------------------------------------------------
## Save the configuration as a file, so make understands if it changed.
##------------------------------------------------------------------------------
$(LAST_BUILD_CONFIG):
	@rm -f target/*.build_config
	@mkdir -p target
	@touch $(LAST_BUILD_CONFIG)

##------------------------------------------------------------------------------
## Compile the kernel ELF
##------------------------------------------------------------------------------
$(KERNEL_ELF_RAW): $(KERNEL_ELF_RAW_DEPS)
	$(call color_header, "Compiling kernel ELF - $(BSP)")
	@RUSTFLAGS="$(RUSTFLAGS_PEDANTIC)" $(RUSTC_CMD)

##------------------------------------------------------------------------------
## Generate the stripped kernel binary
##------------------------------------------------------------------------------
$(KERNEL_BIN): $(KERNEL_ELF)
	$(call color_header, "Generating stripped binary")
	@$(OBJCOPY_CMD) $(KERNEL_ELF) $(KERNEL_BIN)
	$(call color_progress_prefix, "Name")
	@echo $(KERNEL_BIN)
	$(call color_progress_prefix, "Size")
	@$(call disk_usage_KiB, $(KERNEL_BIN))

##------------------------------------------------------------------------------
## Generate the documentation
##------------------------------------------------------------------------------
doc: clean
	$(call color_header, "Generating docs")
	@$(DOC_CMD) --document-private-items --open

##------------------------------------------------------------------------------
## Run the kernel in QEMU
##------------------------------------------------------------------------------
ifeq ($(QEMU_MACHINE_TYPE),) # QEMU is not supported for the board.

qemu:
	$(call color_header, "$(QEMU_MISSING_STRING)")

else # QEMU is supported.

qemu: $(KERNEL_BIN)
	$(call color_header, "Launching QEMU")
	@$(EXEC_QEMU) $(QEMU_RELEASE_ARGS) -kernel $(KERNEL_BIN)

endif

##------------------------------------------------------------------------------
## Push the kernel to the real HW target
##------------------------------------------------------------------------------
chainboot: $(KERNEL_BIN)
	@$(EXEC_MINIPUSH) $(DEV_SERIAL) $(KERNEL_BIN)

##------------------------------------------------------------------------------
## Run clippy
##------------------------------------------------------------------------------
clippy:
	@RUSTFLAGS="$(RUSTFLAGS_PEDANTIC)" $(CLIPPY_CMD)
	@RUSTFLAGS="$(RUSTFLAGS_PEDANTIC)" $(CLIPPY_CMD) --features test_build --tests \
                --manifest-path $(KERNEL_MANIFEST)

##------------------------------------------------------------------------------
## Clean
##------------------------------------------------------------------------------
clean:
	rm -rf target $(KERNEL_BIN)

##------------------------------------------------------------------------------
## Run readelf
##------------------------------------------------------------------------------
readelf: $(KERNEL_ELF)
	$(call color_header, "Launching readelf")
	@$(READELF_BINARY) --headers $(KERNEL_ELF)

##------------------------------------------------------------------------------
## Run objdump
##------------------------------------------------------------------------------
objdump: $(KERNEL_ELF)
	$(call color_header, "Launching objdump")
	@$(OBJDUMP_BINARY) --disassemble --demangle \
                --section .text   \
                --section .rodata \
                $(KERNEL_ELF) | rustfilt

##------------------------------------------------------------------------------
## Run nm
##------------------------------------------------------------------------------
nm: $(KERNEL_ELF)
	$(call color_header, "Launching nm")
	@$(NM_BINARY) --demangle --print-size $(KERNEL_ELF) | sort | rustfilt



##--------------------------------------------------------------------------------------------------
## Debugging targets
##--------------------------------------------------------------------------------------------------
.PHONY: jtagboot openocd gdb gdb-opt0

##------------------------------------------------------------------------------
## Push the JTAG boot image to the real HW target
##------------------------------------------------------------------------------
jtagboot:
	@$(EXEC_MINIPUSH) $(DEV_SERIAL) $(JTAG_BOOT_IMAGE)

##------------------------------------------------------------------------------
## Start OpenOCD session
##------------------------------------------------------------------------------
openocd:
	$(call color_header, "Launching OpenOCD")
	@openocd $(OPENOCD_ARG)

##------------------------------------------------------------------------------
## Start GDB session
##------------------------------------------------------------------------------
gdb-opt0: RUSTC_MISC_ARGS += -C opt-level=0
gdb gdb-opt0: $(KERNEL_ELF)
	$(call color_header, "Launching GDB")
	@gdb-multiarch -q $(KERNEL_ELF)



##--------------------------------------------------------------------------------------------------
## Testing targets
##--------------------------------------------------------------------------------------------------
.PHONY: test test_boot test_unit test_integration

test_unit test_integration: FEATURES += --features test_build

ifeq ($(QEMU_MACHINE_TYPE),) # QEMU is not supported for the board.

test_boot test_unit test_integration test:
	$(call color_header, "$(QEMU_MISSING_STRING)")

else # QEMU is supported.

##------------------------------------------------------------------------------
## Run boot test
##------------------------------------------------------------------------------
test_boot: $(KERNEL_BIN)
	$(call color_header, "Boot test - $(BSP)")
	@$(EXEC_TEST_DISPATCH) $(EXEC_QEMU) $(QEMU_RELEASE_ARGS) -kernel $(KERNEL_BIN)

##------------------------------------------------------------------------------
## Helpers for unit and integration test targets
##------------------------------------------------------------------------------
define KERNEL_TEST_RUNNER
#!/usr/bin/env bash

    # The cargo test runner seems to change into the crate under test's directory. Therefore, ensure
    # this script executes from the root.
    cd $(shell pwd)

    TEST_ELF=$$(echo $$1 | sed -e 's/.*target/target/g')
    TEST_BINARY=$$(echo $$1.img | sed -e 's/.*target/target/g')

    $(OBJCOPY_CMD) $$TEST_ELF $$TEST_BINARY
    $(EXEC_TEST_DISPATCH) $(EXEC_QEMU) $(QEMU_TEST_ARGS) -kernel $$TEST_BINARY
endef

export KERNEL_TEST_RUNNER

define test_prepare
    @mkdir -p target
    @echo "$$KERNEL_TEST_RUNNER" > target/kernel_test_runner.sh
    @chmod +x target/kernel_test_runner.sh
endef

##------------------------------------------------------------------------------
## Run unit test(s)
##------------------------------------------------------------------------------
test_unit:
	$(call color_header, "Compiling unit test(s) - $(BSP)")
	$(call test_prepare)
	@RUSTFLAGS="$(RUSTFLAGS_PEDANTIC)" $(TEST_CMD) --lib

##------------------------------------------------------------------------------
## Run integration test(s)
##------------------------------------------------------------------------------
test_integration:
	$(call color_header, "Compiling integration test(s) - $(BSP)")
	$(call test_prepare)
	@RUSTFLAGS="$(RUSTFLAGS_PEDANTIC)" $(TEST_CMD) $(TEST_ARG)

test: test_boot test_unit test_integration

endif

##------------------------------------------------------------------------------
## Precompute the kernel translation tables and patch them into the kernel ELF
##------------------------------------------------------------------------------
$(KERNEL_ELF_TTABLES): $(KERNEL_ELF_TTABLES_DEPS)
	$(call color_header, "Precomputing kernel translation tables and patching kernel ELF")
	@cp $(KERNEL_ELF_RAW) $(KERNEL_ELF_TTABLES)
	@$(EXEC_TT_TOOL) $(BSP) $(KERNEL_ELF_TTABLES)

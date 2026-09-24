// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! DWC2 USB On-The-Go controller driver (host-mode only).
//!
//! Used by the Raspberry Pi 3 (BCM2837) and earlier boards. The Pi 4/5 use an XHCI controller
//! instead — see a future `xhci` driver module.
//!
//! This initial pass brings up the AHB/OTG core into host mode and exposes root-port status
//! (device attach/detach, reset, speed). Channel-based transfer support (control/bulk/interrupt
//! transfers needed for USB core enumeration and class drivers) is added in a later step.
//!
//! # Resources
//!
//! - Synopsys DesignWare Cores USB 2.0 Hi-Speed On-The-Go (HS OTG) Controller databook.
//! - Linux kernel `drivers/usb/dwc2/core.h`, `hcd.c` (register layout reference).
//! - <https://github.com/raspberrypi/linux/blob/rpi-6.6.y/arch/arm/boot/dts/broadcom/bcm2835-common.dtsi>
//!   (`usb` node: `compatible = "brcm,bcm2835-usb", "snps,dwc2";`)
//!
//! # TODO_VERIFY
//!
//! Register offsets below are transcribed from the DWC2 databook's default "Global CSR Map" and
//! "Host Mode CSR Map" and have **not** been validated against real hardware or QEMU raspi3b
//! yet. Verify against the databook (or Linux `dwc2/hw.h`) before relying on this driver.

use crate::{
	bsp::device_driver::common::MMIODerefWrapper,
	driver,
	exception::{self as generic_exception, asynchronous::IRQNumber},
	memory::{Address, Virtual},
	synchronization,
	synchronization::IRQSafeNullLock,
	time,
};
use core::time::Duration;
use tock_registers::{
	interfaces::{ReadWriteable, Readable},
	register_bitfields, register_structs,
	registers::ReadWrite,
};

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

register_bitfields! {
	u32,

	/// Core USB Configuration Register (GUSBCFG).
	GUSBCFG [
		/// ULPI UTMI+ Select (0 = UTMI+, matches the Pi's internal PHY wiring).
		ULPI_UTMI_SEL OFFSET(4) NUMBITS(1) [],

		/// Force Host Mode.
		FORCEHOSTMODE OFFSET(29) NUMBITS(1) []
	],

	/// Core Reset Register (GRSTCTL).
	GRSTCTL [
		/// Core Soft Reset. Self-clears when the reset has completed.
		CSFTRST OFFSET(0) NUMBITS(1) [],

		/// AHB Master Idle. Must be 1 before issuing CSFTRST.
		AHBIDLE OFFSET(31) NUMBITS(1) []
	],

	/// Core Interrupt Register (GINTSTS). Write-1-to-clear where applicable.
	GINTSTS [
		/// Host Port Interrupt: set when HPRT0 status changes.
		PRTINT OFFSET(24) NUMBITS(1) [],

		/// Host Channels Interrupt.
		HCHINT OFFSET(25) NUMBITS(1) []
	],

	/// Core Interrupt Mask Register (GINTMSK).
	GINTMSK [
		PRTINTMSK OFFSET(24) NUMBITS(1) [],
		HCHINTMSK OFFSET(25) NUMBITS(1) []
	],

	/// Host Port Control and Status Register (HPRT).
	HPRT [
		/// Port Connect Status (1 = device attached). Read-only status bit.
		PRTCONNSTS OFFSET(0) NUMBITS(1) [],

		/// Port Connect Detected (write-1-to-clear).
		PRTCONNDET OFFSET(1) NUMBITS(1) [],

		/// Port Enable.
		PRTENA OFFSET(2) NUMBITS(1) [],

		/// Port Enable/Disable Change (write-1-to-clear).
		PRTENCHNG OFFSET(3) NUMBITS(1) [],

		/// Port Reset.
		PRTRST OFFSET(8) NUMBITS(1) [],

		/// Port Power.
		PRTPWR OFFSET(12) NUMBITS(1) [],

		/// Port Speed (0 = High, 1 = Full, 2 = Low).
		PRTSPD OFFSET(17) NUMBITS(2) [
			High = 0b00,
			Full = 0b01,
			Low  = 0b10
		]
	]
}

register_structs! {
	#[allow(non_snake_case)]
	RegisterBlock {
		(0x000 => _reserved1),
		(0x008 => GUSBCFG: ReadWrite<u32, GUSBCFG::Register>),
		(0x00C => GRSTCTL: ReadWrite<u32, GRSTCTL::Register>),
		(0x010 => GINTSTS: ReadWrite<u32, GINTSTS::Register>),
		(0x014 => GINTMSK: ReadWrite<u32, GINTMSK::Register>),
		(0x018 => _reserved2),
		(0x400 => _reserved3),
		(0x440 => HPRT: ReadWrite<u32, HPRT::Register>),
		(0x444 => @END),
	}
}

/// Abstraction for the associated MMIO registers.
type Registers = MMIODerefWrapper<RegisterBlock>;

struct Dwc2UsbInner {
	registers: Registers,
}

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// The current state of the (single) root hub port, as observed by the driver.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[allow(dead_code)]
pub enum PortStatus {
    /// No device attached.
    NotConnected,

    /// A device is attached but the port has not yet been reset/enabled.
    Connected,

    /// A device is attached, reset, and enabled — ready for enumeration.
    Enabled,
}

/// Representation of the DWC2 USB controller (host mode).
pub struct Dwc2Usb {
	inner: IRQSafeNullLock<Dwc2UsbInner>,
}

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

impl Dwc2UsbInner {
	/// Create an instance.
	///
	/// # Safety
	///
	/// - The user must ensure to provide a correct MMIO start address.
	pub const unsafe fn new(mmio_start_addr: Address<Virtual>) -> Self {
		Self {
			registers: Registers::new(mmio_start_addr),
		}
	}

	/// Issue a core soft reset and wait for it to complete.
	///
	/// Mirrors the sequence used by Linux's `dwc2_core_reset()`: wait for the AHB master to go
	/// idle, request the reset, then poll until the core clears the reset bit itself.
	fn core_soft_reset(&mut self) -> Result<(), &'static str> {
		const MAX_POLL_ATTEMPTS: usize = 100_000;

		for _ in 0..MAX_POLL_ATTEMPTS {
			if self.registers.GRSTCTL.is_set(GRSTCTL::AHBIDLE) {
				break;
			}
		}

		self.registers.GRSTCTL.modify(GRSTCTL::CSFTRST::SET);

		for _ in 0..MAX_POLL_ATTEMPTS {
			if !self.registers.GRSTCTL.is_set(GRSTCTL::CSFTRST) {
				return Ok(());
			}
		}

		Err("DWC2 core soft reset did not complete")
	}

	/// Force the controller into host mode and select the UTMI+ PHY interface.
	fn force_host_mode(&mut self) {
		self.registers
			.GUSBCFG
			.modify(GUSBCFG::FORCEHOSTMODE::SET + GUSBCFG::ULPI_UTMI_SEL::CLEAR);

		// The core needs time to switch modes after FORCEHOSTMODE is set.
		time::time_manager().spin_for(Duration::from_millis(25));
	}

	/// Power the root port and hold it in reset for the required minimum duration (USB 2.0 spec:
	/// >= 10 ms for root hub reset signaling).
	fn power_and_reset_port(&mut self) {
		self.registers.HPRT.modify(HPRT::PRTPWR::SET);
		time::time_manager().spin_for(Duration::from_millis(10));

		self.registers.HPRT.modify(HPRT::PRTRST::SET);
		time::time_manager().spin_for(Duration::from_millis(50));
		self.registers.HPRT.modify(HPRT::PRTRST::CLEAR);
	}

	/// Enable the port and host-channel interrupts at the core level.
	fn unmask_interrupts(&mut self) {
		self.registers
			.GINTMSK
			.modify(GINTMSK::PRTINTMSK::SET + GINTMSK::HCHINTMSK::SET);
	}

	/// Read the current root port status.
	#[allow(dead_code)]
	fn port_status(&self) -> PortStatus {
		if !self.registers.HPRT.is_set(HPRT::PRTCONNSTS) {
			return PortStatus::NotConnected;
		}

		if self.registers.HPRT.is_set(HPRT::PRTENA) {
			PortStatus::Enabled
		} else {
			PortStatus::Connected
		}
	}

	/// Handle a pending interrupt: acknowledge port status-change bits.
	///
	/// Channel-transfer-complete handling (`HCHINT`) will be added alongside the transfer-layer
	/// implementation.
	fn handle_irq(&mut self) {
		if self.registers.GINTSTS.is_set(GINTSTS::PRTINT) {
			if self.registers.HPRT.is_set(HPRT::PRTCONNDET) {
				self.registers.HPRT.modify(HPRT::PRTCONNDET::SET);
			}
			if self.registers.HPRT.is_set(HPRT::PRTENCHNG) {
				self.registers.HPRT.modify(HPRT::PRTENCHNG::SET);
			}
		}
	}
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

impl Dwc2Usb {
	pub const COMPATIBLE: &'static str = "DWC2 USB";

	/// Create an instance.
	///
	/// # Safety
	///
	/// - The user must ensure to provide a correct MMIO start address.
	pub const unsafe fn new(mmio_start_addr: Address<Virtual>) -> Self {
		Self {
			inner: IRQSafeNullLock::new(Dwc2UsbInner::new(mmio_start_addr)),
		}
	}

	/// Return the current root port status.
	#[allow(dead_code)]
	pub fn port_status(&self) -> PortStatus {
		self.inner.lock(|inner| inner.port_status())
	}
}

//------------------------------------------------------------------------------
// OS Interface Code
//------------------------------------------------------------------------------
use synchronization::interface::Mutex;

impl driver::interface::DeviceDriver for Dwc2Usb {
	type IRQNumberType = IRQNumber;

	fn compatible(&self) -> &'static str {
		Self::COMPATIBLE
	}

	unsafe fn init(&self) -> Result<(), &'static str> {
		self.inner.lock(|inner| {
			inner.core_soft_reset()?;
			inner.force_host_mode();
			inner.power_and_reset_port();
			inner.unmask_interrupts();

			Ok(())
		})
	}

	fn register_and_enable_irq_handler(
		&'static self,
		irq_number: &Self::IRQNumberType,
	) -> Result<(), &'static str> {
		use generic_exception::asynchronous::{irq_manager, IRQHandlerDescriptor};

		let descriptor = IRQHandlerDescriptor::new(*irq_number, Self::COMPATIBLE, self);

		irq_manager().register_handler(descriptor)?;
		irq_manager().enable(irq_number);

		Ok(())
	}
}

impl generic_exception::asynchronous::interface::IRQHandler for Dwc2Usb {
	fn handle(&self) -> Result<(), &'static str> {
		self.inner.lock(|inner| inner.handle_irq());
		Ok(())
	}
}

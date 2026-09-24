// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Host BSP asynchronous exception handling stub.

/// Asynchronous exception handling.
pub mod asynchronous {
    use crate::exception::asynchronous::{interface, IRQContext, IRQHandlerDescriptor};
    use crate::synchronization::{interface::ReadWriteEx, InitStateLock};
    use core::fmt;

    //--------------------------------------------------------------------------------------------------
    // Public Definitions
    //--------------------------------------------------------------------------------------------------

    /// Interrupt number stub.
    ///
    /// Host has no real interrupt controller; this exists only to satisfy the generic
    /// `exception::asynchronous::IRQNumber` type alias and the `IRQHandlerDescriptor` /
    /// `DeviceDriverDescriptor` generic parameters.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub struct IRQNumber(u32);

    impl IRQNumber {
        /// Create an instance.
        pub const fn new(n: u32) -> Self {
            Self(n)
        }
    }

    impl fmt::Display for IRQNumber {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    /// A minimal IRQ manager for host builds.
    ///
    /// Host has no real interrupt controller; drivers still call `register_handler()` and
    /// `enable()` during init, so this stores at most one registered handler and quietly accepts
    /// enable requests instead of panicking like `NullIRQManager`.
    pub struct HostIRQManager {
        handler: InitStateLock<Option<IRQHandlerDescriptor<IRQNumber>>>,
    }

    //--------------------------------------------------------------------------------------------------
    // Global instances
    //--------------------------------------------------------------------------------------------------

    static HOST_IRQ_MANAGER: HostIRQManager = HostIRQManager {
        handler: InitStateLock::new(None),
    };

    //--------------------------------------------------------------------------------------------------
    // Public Code
    //--------------------------------------------------------------------------------------------------

    impl interface::IRQManager for HostIRQManager {
        type IRQNumberType = IRQNumber;

        fn register_handler(
            &self,
            descriptor: IRQHandlerDescriptor<Self::IRQNumberType>,
        ) -> Result<(), &'static str> {
            self.handler.write(|slot| *slot = Some(descriptor));

            Ok(())
        }

        fn enable(&self, _irq_number: &Self::IRQNumberType) {
            // No-op: there is no real interrupt controller to program on host.
        }

        fn handle_pending_irqs<'irq_context>(&'irq_context self, _ic: &IRQContext<'irq_context>) {
            // No-op: host drivers invoke their handlers directly (see `_arch/x86_64/time.rs`)
            // rather than through a real interrupt vector.
        }
    }

    /// Register the host IRQ manager as the kernel's active IRQ manager.
    ///
    /// Must be called once during BSP driver init, mirroring what the Raspberry Pi BSP does for
    /// its real interrupt controller.
    pub fn register_host_irq_manager() {
        crate::exception::asynchronous::register_irq_manager(&HOST_IRQ_MANAGER);
    }

    /// The IRQ map (empty; host drivers, if any, register no hardware IRQs).
    pub mod irq_map {
        use super::IRQNumber;

        /// Placeholder timer IRQ number — unused on host, since the host timer stub in
        /// `_arch/x86_64/time.rs` bypasses the IRQ manager entirely and calls the timer's
        /// handler directly from a background thread.
        pub const ARM_NS_PHYSICAL_TIMER: IRQNumber = IRQNumber::new(0);
    }
}

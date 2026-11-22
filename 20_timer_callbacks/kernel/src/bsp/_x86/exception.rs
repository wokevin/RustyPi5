// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Hyper-V x86_64 guest exception handling.

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
    /// No real IDT/PIC/APIC is wired up yet; this exists only to satisfy the generic
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

    /// A minimal IRQ manager for x86_64 bring-up.
    ///
    /// No real interrupt controller (PIC/APIC) is programmed yet; drivers still call
    /// `register_handler()` and `enable()` during init, so this stores at most one registered
    /// handler and quietly accepts enable requests instead of panicking like `NullIRQManager`.
    pub struct X86IRQManager {
        handler: InitStateLock<Option<IRQHandlerDescriptor<IRQNumber>>>,
    }

    //--------------------------------------------------------------------------------------------------
    // Global instances
    //--------------------------------------------------------------------------------------------------

    static X86_IRQ_MANAGER: X86IRQManager = X86IRQManager {
        handler: InitStateLock::new(None),
    };

    //--------------------------------------------------------------------------------------------------
    // Public Code
    //--------------------------------------------------------------------------------------------------

    impl interface::IRQManager for X86IRQManager {
        type IRQNumberType = IRQNumber;

        fn register_handler(
            &self,
            descriptor: IRQHandlerDescriptor<Self::IRQNumberType>,
        ) -> Result<(), &'static str> {
            self.handler.write(|slot| *slot = Some(descriptor));

            Ok(())
        }

        fn enable(&self, _irq_number: &Self::IRQNumberType) {
            // No-op: no real interrupt controller programmed yet.
        }

        fn handle_pending_irqs<'irq_context>(&'irq_context self, _ic: &IRQContext<'irq_context>) {
            // No-op: no real IDT/PIC/APIC wired up yet.
        }
    }

    /// Register the x86 bring-up IRQ manager as the kernel's active IRQ manager.
    pub fn register_x86_irq_manager() {
        crate::exception::asynchronous::register_irq_manager(&X86_IRQ_MANAGER);
    }

    /// The IRQ map (empty for now; no real interrupt controller programmed yet).
    pub mod irq_map {
        use super::IRQNumber;

        /// Placeholder timer IRQ number, pending real PIT/APIC timer bring-up.
        pub const TIMER: IRQNumber = IRQNumber::new(0);
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0

//! Explicitly owned, polled UDP bring-up service. Not enabled by merely linking this module.
//! The kernel must provide supervisor-only MMIO mappings and a real monotonic millisecond
//! clock before calling this service. The current placeholder `uptime()` is not sufficient.
//! Policy is chosen at construction by trusted kernel code, never by a user syscall.

#[path = "network/stack.rs"]
pub mod stack;
#[cfg(target_arch = "x86_64")]
#[path = "network/e1000.rs"]
pub mod e1000;
#[cfg(feature = "bsp_qemu_x86")]
#[path = "network/clock.rs"]
mod clock;
#[cfg(feature = "bsp_qemu_x86")]
#[path = "network/user.rs"]
pub mod user;

use stack::{Event, Policy, Stack, MAX_FRAME};

pub trait Link {
    fn mac(&self) -> [u8; 6];
    fn link_up(&self) -> bool;
    fn send(&mut self, frame: &[u8]) -> Result<(), &'static str>;
    fn receive(&mut self, output: &mut [u8; MAX_FRAME]) -> Option<usize>;
}

#[cfg(target_arch = "x86_64")]
impl Link for e1000::Device {
    fn mac(&self) -> [u8; 6] { self.mac }
    fn link_up(&self) -> bool { self.link_up() }
    fn send(&mut self, frame: &[u8]) -> Result<(), &'static str> { self.send(frame) }
    fn receive(&mut self, output: &mut [u8; MAX_FRAME]) -> Option<usize> { self.receive(output) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status { Idle, Waiting, Reply, TimedOut, Failed }

/// One bounded exchange at a time; the owner can reset it for later requests.
/// Dropping the owner shuts down its NIC. IRQ handlers must only schedule polling.
pub struct Network<L: Link> {
    link: L,
    stack: Stack,
    status: Status,
    started: u64,
    last_time: u64,
    last_send: u64,
    input: [u8; MAX_FRAME],
    output: [u8; MAX_FRAME],
}

impl<L: Link> Network<L> {
    pub fn new(link: L, nonce: u64, policy: Policy) -> Self {
        let stack = Stack::new(link.mac(), nonce, policy);
        Self { link, stack, status: Status::Idle, started: 0, last_time: 0,
            last_send: 0, input: [0; MAX_FRAME], output: [0; MAX_FRAME] }
    }

    pub fn status(&self) -> Status { self.status }
    pub fn counters(&self) -> (u64, u64) { (self.stack.received, self.stack.dropped) }

    pub fn reset(&mut self, payload: &[u8]) -> Result<(), &'static str> {
        if self.status == Status::Waiting { return Err("Network request is busy"); }
        self.stack.cancel();
        self.stack.set_payload(payload)?;
        self.status = Status::Idle;
        Ok(())
    }

    pub fn start(&mut self, now_ms: u64) -> Result<(), &'static str> {
        if self.status != Status::Idle { return Err("Network probe already started"); }
        if !self.link.link_up() { return self.fail("Network link is down"); }
        self.started = now_ms;
        self.last_time = now_ms;
        self.last_send = now_ms;
        self.status = Status::Waiting;
        self.transmit_probe()
    }

    /// Process at most 16 frames per invocation so untrusted traffic cannot starve kernel work.
    /// Call repeatedly with elapsed monotonic time, including when no packet interrupts arrive.
    pub fn poll(&mut self, now_ms: u64) -> Result<Status, &'static str> {
        if self.status != Status::Waiting { return Ok(self.status); }
        if now_ms < self.last_time { return self.fail("Network clock moved backwards"); }
        self.last_time = now_ms;
        if now_ms - self.started >= 5000 {
            self.stack.cancel();
            self.status = Status::TimedOut;
            return Ok(self.status);
        }
        if !self.link.link_up() { return self.fail("Network link went down"); }
        for _ in 0..16 {
            let length = match self.link.receive(&mut self.input) {
                Some(length) if length <= MAX_FRAME => length,
                Some(_) => return self.fail("Network device returned an invalid frame length"),
                None => break,
            };
            match self.stack.receive(&self.input[..length], &mut self.output) {
                Event::Transmit(length) => {
                    if let Err(message) = self.link.send(&self.output[..length]) {
                        return self.fail(message);
                    }
                }
                Event::Reply => { self.status = Status::Reply; return Ok(self.status); }
                Event::Drop(_) | Event::None => {},
            }
        }
        if now_ms - self.last_send >= 1000 {
            self.transmit_probe()?;
            self.last_send = now_ms;
        }
        Ok(self.status)
    }

    /// Drain unsolicited frames while idle so stale traffic cannot fill the RX ring.
    pub fn drain_idle(&mut self) -> Result<(), &'static str> {
        if self.status == Status::Waiting { return Err("Network request is active"); }
        for _ in 0..16 {
            let length = match self.link.receive(&mut self.input) {
                Some(length) if length <= MAX_FRAME => length,
                Some(_) => return self.fail("Network device returned an invalid frame length"),
                None => break,
            };
            if let Event::Transmit(length) = self.stack.receive(&self.input[..length], &mut self.output) {
                if let Err(message) = self.link.send(&self.output[..length]) {
                    return self.fail(message);
                }
            }
        }
        Ok(())
    }

    fn transmit_probe(&mut self) -> Result<(), &'static str> {
        let length = match self.stack.probe(&mut self.output) {
            Ok(length) => length,
            Err(message) => return self.fail(message),
        };
        if let Err(message) = self.link.send(&self.output[..length]) {
            return self.fail(message);
        }
        Ok(())
    }

    fn fail<T>(&mut self, message: &'static str) -> Result<T, &'static str> {
        self.stack.cancel();
        self.status = Status::Failed;
        Err(message)
    }
}

#[cfg(feature = "bsp_qemu_x86")]
static POLL_DUE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "bsp_qemu_x86")]
pub fn handle_timer() {
    POLL_DUE.store(true, core::sync::atomic::Ordering::Release);
}

/// Check-and-clear while IRQs are masked, before the worker's atomic sleep sequence.
#[cfg(feature = "bsp_qemu_x86")]
pub fn take_poll_due() -> bool {
    POLL_DUE.swap(false, core::sync::atomic::Ordering::AcqRel)
}

#[cfg(feature = "bsp_qemu_x86")]
pub struct Runtime {
    network: Network<e1000::Device>,
    clock: clock::Clock,
    timer_ticks: u32,
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn initialize() -> Result<Option<Runtime>, &'static str> {
    if crate::bsp::cpu::current_core_id().raw() != 0
        || !crate::exception::asynchronous::is_local_irq_masked()
    { return Err("Network initialization requires masked CPU 0"); }
    if !unsafe { e1000::present() } {
        crate::info!("Networking disabled: no NIC attached");
        return Ok(None);
    }
    let (clock, timer_ticks) = clock::Clock::calibrate()?;
    let device = unsafe { e1000::Device::init(crate::bsp::user::map_device_mmio)? };
    crate::info!("E1000 ready: MAC {:02x?}; kernel-only DMA; PIT-calibrated polling", device.mac);
    let policy = Policy { udp: Some(stack::UdpRule {
        peer: stack::GATEWAY_IP, peer_port: stack::PEER_PORT, local_port: stack::LOCAL_PORT,
    }) };
    let network = Network::new(device, clock.nonce(), policy);
    crate::info!("Network policy: default deny; only 10.0.2.15:49152 -> 10.0.2.2:8081 UDP echo");
    Ok(Some(Runtime { network, clock, timer_ticks }))
}

#[cfg(feature = "bsp_qemu_x86")]
impl Runtime {
    pub fn start_timer(&self) {
        crate::bsp::apic::start_network_timer(self.timer_ticks);
        user::set_available(true);
        handle_timer();
    }

    /// Returns true only if a fatal service failure requires dropping the NIC.
    pub fn poll(&mut self) -> bool {
        let mut payload = [0; stack::MAX_PAYLOAD];
        if let Some(length) = user::take_request(&mut payload) {
            let now = match self.clock.milliseconds() {
                Ok(now) => now,
                Err(message) => {
                    crate::warn!("Networking stopped: {}", message);
                    user::set_available(false);
                    user::complete(user::FAILED);
                    return true;
                }
            };
            let start = self.network.reset(&payload[..length]).and_then(|()| self.network.start(now));
            if let Err(message) = start {
                crate::warn!("UDP request failed: {}", message);
                user::complete(user::FAILED);
                return false;
            }
        }
        if self.network.status() != Status::Waiting {
            if let Err(message) = self.network.drain_idle() {
                crate::warn!("Networking stopped: {}", message);
                user::set_available(false);
                user::complete(user::FAILED);
                return true;
            }
            return false;
        }
        let result = self.clock.milliseconds().and_then(|now| self.network.poll(now));
        match result {
            Ok(Status::Waiting) => false,
            Ok(Status::Reply) => {
                let (received, dropped) = self.network.counters();
                crate::info!("UDP request complete; RX={} dropped={}", received, dropped);
                user::complete(user::SUCCESS);
                false
            }
            Ok(Status::TimedOut) => {
                let (received, dropped) = self.network.counters();
                crate::warn!("UDP request timed out; RX={} dropped={}", received, dropped);
                user::complete(user::TIMEOUT);
                false
            }
            Ok(_) => { user::complete(user::FAILED); false }
            Err(message) => {
                crate::warn!("Networking stopped: {}", message);
                user::set_available(false);
                user::complete(user::FAILED);
                true
            }
        }
    }
}

#[cfg(feature = "bsp_qemu_x86")]
impl Drop for Runtime {
    fn drop(&mut self) {
        user::set_available(false);
        user::complete(user::FAILED);
        crate::bsp::apic::stop_network_timer();
        self.network.stack.cancel();
        // Network's device owner disables receive/transmit and PCI bus mastering on drop.
    }
}

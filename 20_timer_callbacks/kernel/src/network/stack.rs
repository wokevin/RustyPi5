// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded Ethernet/ARP/IPv4/UDP bring-up and a default-deny, kernel-owned policy.
//! No allocation, IP forwarding, fragments, IPv6, TCP, or user policy mutation.

pub const LOCAL_IP: [u8; 4] = [10, 0, 2, 15];
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
pub const LOCAL_PORT: u16 = 49152;
pub const PEER_PORT: u16 = 8081;
pub const MAX_FRAME: usize = 1514;
const PROBE_LABEL: &[u8] = b"RustyPi5 UDP test";
const PROBE_LEN: usize = PROBE_LABEL.len() + core::mem::size_of::<u64>();
pub const MAX_PAYLOAD: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { Ingress, Egress }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UdpRule {
    pub peer: [u8; 4],
    pub peer_port: u16,
    pub local_port: u16,
}

/// Absence of a rule denies traffic in both directions. ARP is handled separately,
/// restricted to the configured next hop and this interface's addresses.
pub struct Policy {
    pub udp: Option<UdpRule>,
}

impl Policy {
    pub const fn deny_all() -> Self { Self { udp: None } }

    pub fn allows(&self, direction: Direction, source: [u8; 4], destination: [u8; 4],
        source_port: u16, destination_port: u16) -> bool
    {
        match self.udp {
            None => false,
            Some(rule) => match direction {
                Direction::Ingress => source == rule.peer && destination == LOCAL_IP
                    && source_port == rule.peer_port && destination_port == rule.local_port,
                Direction::Egress => source == LOCAL_IP && destination == rule.peer
                    && source_port == rule.local_port && destination_port == rule.peer_port,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason { Malformed, Unsupported, Policy, Unsolicited }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event { None, Transmit(usize), Reply, Drop(DropReason) }

pub struct Stack {
    mac: [u8; 6],
    gateway: Option<[u8; 6]>,
    policy: Policy,
    payload: [u8; MAX_PAYLOAD],
    payload_len: usize,
    pending: bool,
    pub received: u64,
    pub dropped: u64,
}

impl Stack {
    pub fn new(mac: [u8; 6], nonce: u64, policy: Policy) -> Self {
        let mut payload = [0; MAX_PAYLOAD];
        payload[..PROBE_LABEL.len()].copy_from_slice(PROBE_LABEL);
        payload[PROBE_LABEL.len()..PROBE_LEN].copy_from_slice(&nonce.to_be_bytes());
        Self { mac, gateway: None, policy, payload, payload_len: PROBE_LEN, pending: false, received: 0, dropped: 0 }
    }

    pub fn set_payload(&mut self, payload: &[u8]) -> Result<(), &'static str> {
        if self.pending || payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return Err("Invalid or busy UDP request");
        }
        self.payload[..payload.len()].copy_from_slice(payload);
        self.payload_len = payload.len();
        Ok(())
    }

    /// Retry the same outstanding datagram; no new flow or listener is created.
    pub fn probe(&mut self, out: &mut [u8; MAX_FRAME]) -> Result<usize, &'static str> {
        if !self.policy.allows(Direction::Egress, LOCAL_IP, GATEWAY_IP, LOCAL_PORT, PEER_PORT) {
            return Err("Network probe denied by egress policy");
        }
        self.pending = true;
        match self.gateway {
            None => Ok(self.arp(out, [255; 6], 1)),
            Some(peer) => Ok(self.udp(out, peer)),
        }
    }

    /// Revoke the outstanding reply allowance on timeout or transmission failure.
    pub fn cancel(&mut self) { self.pending = false; }

    pub fn receive(&mut self, frame: &[u8], out: &mut [u8; MAX_FRAME]) -> Event {
        self.received = self.received.saturating_add(1);
        let event = self.parse(frame, out);
        if let Event::Drop(_) = event { self.dropped = self.dropped.saturating_add(1); }
        event
    }

    fn parse(&mut self, frame: &[u8], out: &mut [u8; MAX_FRAME]) -> Event {
        if frame.len() < 14 || frame.len() > MAX_FRAME { return Event::Drop(DropReason::Malformed); }
        if frame[..6] != self.mac && frame[..6] != [255; 6] {
            return Event::Drop(DropReason::Policy);
        }
        if frame[6] & 1 != 0 || frame[6..12] == [0; 6] {
            return Event::Drop(DropReason::Malformed);
        }
        match be16(frame, 12) {
            0x0806 => self.receive_arp(frame, out),
            0x0800 => self.receive_ip(frame),
            _ => Event::Drop(DropReason::Unsupported),
        }
    }

    fn receive_arp(&mut self, frame: &[u8], out: &mut [u8; MAX_FRAME]) -> Event {
        if frame.len() < 42 || frame[14..20] != [0, 1, 8, 0, 6, 4] {
            return Event::Drop(DropReason::Malformed);
        }
        if !self.policy.allows(Direction::Egress, LOCAL_IP, GATEWAY_IP, LOCAL_PORT, PEER_PORT)
            || frame[22..28] != frame[6..12] || frame[28..32] != GATEWAY_IP
            || frame[38..42] != LOCAL_IP
        { return Event::Drop(DropReason::Policy); }
        let mut peer = [0; 6];
        peer.copy_from_slice(&frame[6..12]);
        if self.gateway.map_or(false, |known| known != peer) {
            return Event::Drop(DropReason::Policy);
        }
        match be16(frame, 20) {
            1 => Event::Transmit(self.arp(out, peer, 2)),
            2 if self.pending && frame[..6] == self.mac && frame[32..38] == self.mac => {
                self.gateway = Some(peer);
                Event::Transmit(self.udp(out, peer))
            }
            _ => Event::Drop(DropReason::Unsolicited),
        }
    }

    fn receive_ip(&mut self, frame: &[u8]) -> Event {
        if frame.len() < 34 || frame[14] != 0x45 { return Event::Drop(DropReason::Malformed); }
        let ip = &frame[14..];
        let length = be16(ip, 2) as usize;
        if length < 28 || length > ip.len() || checksum(&ip[..20]) != 0
            || be16(ip, 6) & 0xBFFF != 0 || ip[8] == 0
        { return Event::Drop(DropReason::Malformed); }
        if ip[9] != 17 { return Event::Drop(DropReason::Unsupported); }
        let mut source = [0; 4]; source.copy_from_slice(&ip[12..16]);
        let mut destination = [0; 4]; destination.copy_from_slice(&ip[16..20]);
        let udp = &ip[20..length];
        if be16(udp, 4) as usize != udp.len() || udp_checksum(source, destination, udp) != 0
            || be16(udp, 6) == 0
        { return Event::Drop(DropReason::Malformed); }
        if frame[..6] != self.mac || !self.policy.allows(Direction::Ingress, source,
            destination, be16(udp, 0), be16(udp, 2))
        { return Event::Drop(DropReason::Policy); }
        if !self.pending || self.gateway.map_or(true, |mac| frame[6..12] != mac)
            || udp[8..] != self.payload[..self.payload_len]
        { return Event::Drop(DropReason::Unsolicited); }
        self.pending = false;
        Event::Reply
    }

    fn ethernet(&self, out: &mut [u8; MAX_FRAME], peer: [u8; 6], kind: u16) {
        out.fill(0);
        out[..6].copy_from_slice(&peer);
        out[6..12].copy_from_slice(&self.mac);
        put16(out, 12, kind);
    }

    fn arp(&self, out: &mut [u8; MAX_FRAME], peer: [u8; 6], operation: u16) -> usize {
        self.ethernet(out, peer, 0x0806);
        out[14..20].copy_from_slice(&[0, 1, 8, 0, 6, 4]);
        put16(out, 20, operation);
        out[22..28].copy_from_slice(&self.mac);
        out[28..32].copy_from_slice(&LOCAL_IP);
        if operation == 2 { out[32..38].copy_from_slice(&peer); }
        out[38..42].copy_from_slice(&GATEWAY_IP);
        60
    }

    fn udp(&self, out: &mut [u8; MAX_FRAME], peer: [u8; 6]) -> usize {
        self.ethernet(out, peer, 0x0800);
        let ip = &mut out[14..];
        ip[0] = 0x45;
        put16(ip, 2, (28 + self.payload_len) as u16);
        put16(ip, 6, 0x4000);
        ip[8] = 64;
        ip[9] = 17;
        ip[12..16].copy_from_slice(&LOCAL_IP);
        ip[16..20].copy_from_slice(&GATEWAY_IP);
        let header_checksum = checksum(&ip[..20]);
        put16(ip, 10, header_checksum);
        let udp = &mut ip[20..28 + self.payload_len];
        put16(udp, 0, LOCAL_PORT);
        put16(udp, 2, PEER_PORT);
        put16(udp, 4, (8 + self.payload_len) as u16);
        udp[8..].copy_from_slice(&self.payload[..self.payload_len]);
        let sum = udp_checksum(LOCAL_IP, GATEWAY_IP, udp);
        put16(udp, 6, if sum == 0 { 0xFFFF } else { sum });
        14 + 28 + self.payload_len
    }
}

fn be16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}
fn sum(bytes: &[u8]) -> u32 {
    bytes.chunks(2).map(|pair| ((pair[0] as u32) << 8) | pair.get(1).copied().unwrap_or(0) as u32).sum()
}
fn finish(mut value: u32) -> u16 {
    while value >> 16 != 0 { value = (value & 0xFFFF) + (value >> 16); }
    !(value as u16)
}
pub fn checksum(bytes: &[u8]) -> u16 { finish(sum(bytes)) }
fn udp_checksum(source: [u8; 4], destination: [u8; 4], udp: &[u8]) -> u16 {
    finish(sum(&source) + sum(&destination) + 17 + udp.len() as u32 + sum(udp))
}

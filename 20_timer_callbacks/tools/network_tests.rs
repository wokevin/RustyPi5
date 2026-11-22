// SPDX-License-Identifier: MIT OR Apache-2.0

#[path = "..\\kernel\\src\\network.rs"]
mod network;

use network::stack::*;
use network::{Link, Network, Status};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
const PEER_MAC: [u8; 6] = [0x52, 0x54, 0, 0xAB, 0xCD, 0xEF];

fn policy() -> Policy {
    Policy { udp: Some(UdpRule { peer: GATEWAY_IP, peer_port: PEER_PORT, local_port: LOCAL_PORT }) }
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}
// Independent checksum implementation used to construct peer traffic.
fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut total = 0u32;
    for (index, &byte) in bytes.iter().enumerate() {
        total += (byte as u32) << if index % 2 == 0 { 8 } else { 0 };
    }
    while total > 65535 { total = (total & 65535) + (total >> 16); }
    !(total as u16)
}
fn ip_checksum(frame: &mut [u8]) {
    put16(frame, 24, 0);
    let sum = internet_checksum(&frame[14..34]);
    put16(frame, 24, sum);
}
fn udp_checksum(frame: &mut [u8]) {
    put16(frame, 40, 0);
    let ip_length = u16::from_be_bytes([frame[16], frame[17]]) as usize;
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&frame[26..34]);
    pseudo.extend_from_slice(&[0, 17]);
    pseudo.extend_from_slice(&((ip_length - 20) as u16).to_be_bytes());
    pseudo.extend_from_slice(&frame[34..14 + ip_length]);
    let sum = internet_checksum(&pseudo);
    put16(frame, 40, if sum == 0 { 65535 } else { sum });
}
fn arp(operation: u16) -> Vec<u8> {
    let mut frame = vec![0; 60];
    frame[..6].copy_from_slice(&MAC);
    frame[6..12].copy_from_slice(&PEER_MAC);
    frame[12..20].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4]);
    put16(&mut frame, 20, operation);
    frame[22..28].copy_from_slice(&PEER_MAC);
    frame[28..32].copy_from_slice(&GATEWAY_IP);
    frame[32..38].copy_from_slice(&MAC);
    frame[38..42].copy_from_slice(&LOCAL_IP);
    frame
}
fn ready_stack() -> (Stack, Vec<u8>) {
    let mut stack = Stack::new(MAC, 0x0102_0304_0506_0708, policy());
    let mut out = [0; MAX_FRAME];
    assert_eq!(stack.probe(&mut out), Ok(60));
    let length = match stack.receive(&arp(2), &mut out) {
        Event::Transmit(length) => length,
        other => panic!("ARP failed: {:?}", other),
    };
    (stack, out[..length].to_vec())
}
fn echo(request: &[u8]) -> Vec<u8> {
    let mut reply = request.to_vec();
    reply[..6].copy_from_slice(&MAC);
    reply[6..12].copy_from_slice(&PEER_MAC);
    reply[26..30].copy_from_slice(&GATEWAY_IP);
    reply[30..34].copy_from_slice(&LOCAL_IP);
    put16(&mut reply, 34, PEER_PORT);
    put16(&mut reply, 36, LOCAL_PORT);
    ip_checksum(&mut reply);
    udp_checksum(&mut reply);
    reply
}
fn receive(stack: &mut Stack, frame: &[u8]) -> Event {
    stack.receive(frame, &mut [0; MAX_FRAME])
}

#[test]
fn default_deny_blocks_udp_and_arp_without_mutating_output() {
    let mut stack = Stack::new(MAC, 1, Policy::deny_all());
    let mut out = [0xA5; MAX_FRAME];
    assert!(stack.probe(&mut out).is_err());
    assert_eq!(stack.receive(&arp(1), &mut out), Event::Drop(DropReason::Policy));
    assert_eq!(stack.receive(&arp(2), &mut out), Event::Drop(DropReason::Policy));
    assert_eq!(out, [0xA5; MAX_FRAME]);
}

#[test]
fn policy_checks_every_address_and_port_in_both_directions() {
    for direction in [Direction::Ingress, Direction::Egress] {
        let (source, destination, source_port, destination_port) = match direction {
            Direction::Ingress => (GATEWAY_IP, LOCAL_IP, PEER_PORT, LOCAL_PORT),
            Direction::Egress => (LOCAL_IP, GATEWAY_IP, LOCAL_PORT, PEER_PORT),
        };
        let rule = policy();
        assert!(rule.allows(direction, source, destination, source_port, destination_port));
        assert!(!rule.allows(direction, [192, 0, 2, 1], destination, source_port, destination_port));
        assert!(!rule.allows(direction, source, [192, 0, 2, 1], source_port, destination_port));
        assert!(!rule.allows(direction, source, destination, source_port + 1, destination_port));
        assert!(!rule.allows(direction, source, destination, source_port, destination_port + 1));
        assert!(!Policy::deny_all().allows(direction, source, destination, source_port, destination_port));
    }
}

#[test]
fn arp_resolution_udp_checksums_and_matching_reply() {
    let (mut stack, request) = ready_stack();
    assert_eq!(request.len(), 67);
    assert_eq!(&request[..6], &PEER_MAC);
    assert_eq!(&request[42..59], b"RustyPi5 UDP test");
    assert_eq!(&request[59..], &0x0102_0304_0506_0708u64.to_be_bytes());
    assert_eq!(internet_checksum(&request[14..34]), 0);
    let mut repaired = request.clone();
    udp_checksum(&mut repaired);
    assert_eq!(request, repaired);
    assert_eq!(receive(&mut stack, &echo(&request)), Event::Reply);
    assert_eq!(receive(&mut stack, &echo(&request)), Event::Drop(DropReason::Unsolicited));
}

#[test]
fn arp_requests_answer_only_for_configured_gateway() {
    let mut stack = Stack::new(MAC, 1, policy());
    let mut out = [0; MAX_FRAME];
    let mut request = arp(1);
    request[..6].fill(255);
    request[32..38].fill(0);
    assert_eq!(stack.receive(&request, &mut out), Event::Transmit(60));
    assert_eq!(&out[..6], &PEER_MAC);
    assert_eq!(&out[20..22], &[0, 2]);
    assert_eq!(&out[28..32], &LOCAL_IP);
    assert_eq!(&out[38..42], &GATEWAY_IP);
    request[28] ^= 1;
    assert_eq!(receive(&mut stack, &request), Event::Drop(DropReason::Policy));
}

#[test]
fn arp_rejects_unsolicited_mismatched_and_changed_peer() {
    let mut stack = Stack::new(MAC, 1, policy());
    assert_eq!(receive(&mut stack, &arp(2)), Event::Drop(DropReason::Unsolicited));
    let (mut stack, _) = ready_stack();
    for offset in [6, 22, 28, 32, 38] {
        let mut frame = arp(2);
        frame[offset] ^= 2;
        assert!(matches!(receive(&mut stack, &frame), Event::Drop(_)));
    }
    let mut changed = arp(1);
    changed[6] ^= 2;
    changed[22] ^= 2;
    assert_eq!(receive(&mut stack, &changed), Event::Drop(DropReason::Policy));
    let mut malformed = arp(2);
    malformed[18] = 5;
    assert_eq!(receive(&mut stack, &malformed), Event::Drop(DropReason::Malformed));
}

#[test]
fn all_truncated_replies_are_rejected_without_panics() {
    let (mut stack, request) = ready_stack();
    let reply = echo(&request);
    for length in 0..reply.len() {
        assert!(matches!(receive(&mut stack, &reply[..length]), Event::Drop(_)), "length {}", length);
    }
    assert_eq!(receive(&mut stack, &reply), Event::Reply);
    assert_eq!(stack.dropped, reply.len() as u64);
}

#[test]
fn rejects_fragments_options_zero_ttl_and_invalid_lengths() {
    for (offset, value) in [(14, 0x46), (16, 0xFF), (17, 27), (20, 0x20),
        (20, 0x80), (21, 1), (22, 0), (38, 0xFF), (39, 7)]
    {
        let (mut stack, request) = ready_stack();
        let mut reply = echo(&request);
        reply[offset] = value;
        ip_checksum(&mut reply);
        assert_eq!(receive(&mut stack, &reply), Event::Drop(DropReason::Malformed), "offset {}", offset);
    }
}

#[test]
fn rejects_bad_checksums_zero_udp_checksum_and_unsupported_protocols() {
    for offset in [24, 40, 50] {
        let (mut stack, request) = ready_stack();
        let mut reply = echo(&request);
        reply[offset] ^= 1;
        assert_eq!(receive(&mut stack, &reply), Event::Drop(DropReason::Malformed));
    }
    let (mut stack, request) = ready_stack();
    let mut reply = echo(&request);
    put16(&mut reply, 40, 0);
    assert_eq!(receive(&mut stack, &reply), Event::Drop(DropReason::Malformed));
    reply[23] = 6;
    ip_checksum(&mut reply);
    assert_eq!(receive(&mut stack, &reply), Event::Drop(DropReason::Unsupported));
    reply[12..14].copy_from_slice(&[0x86, 0xDD]);
    assert_eq!(receive(&mut stack, &reply), Event::Drop(DropReason::Unsupported));
}

#[test]
fn valid_checksums_do_not_bypass_flow_nonce_or_mac_checks() {
    for offset in [0, 6, 26, 30, 34, 36, 58] {
        let (mut stack, request) = ready_stack();
        let mut reply = echo(&request);
        reply[offset] ^= 2;
        ip_checksum(&mut reply);
        udp_checksum(&mut reply);
        assert!(matches!(receive(&mut stack, &reply), Event::Drop(_)), "offset {}", offset);
        assert_eq!(receive(&mut stack, &echo(&request)), Event::Reply);
    }
}

#[test]
fn cancellation_revokes_reply_and_arp_resolution_allowances() {
    let (mut stack, request) = ready_stack();
    stack.cancel();
    assert_eq!(receive(&mut stack, &echo(&request)), Event::Drop(DropReason::Unsolicited));
    assert_eq!(receive(&mut stack, &arp(2)), Event::Drop(DropReason::Unsolicited));
}

#[test]
fn checksum_matches_rfc1071_and_odd_byte_padding() {
    assert_eq!(checksum(&[0, 1, 0xF2, 3, 0xF4, 0xF5, 0xF6, 0xF7]), 0x220D);
    assert_eq!(checksum(&[1, 2, 3]), internet_checksum(&[1, 2, 3]));
}

#[test]
fn deterministic_malformed_corpus_is_bounded_and_never_transmits() {
    let mut stack = Stack::new(MAC, 1, Policy::deny_all());
    let mut seed = 0x1234_5678u32;
    let mut frame = [0; MAX_FRAME + 1];
    let mut out = [0xA5; MAX_FRAME];
    for iteration in 0..10000 {
        for byte in frame.iter_mut() {
            seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
            *byte = seed as u8;
        }
        let length = iteration % frame.len();
        assert!(matches!(stack.receive(&frame[..length], &mut out), Event::Drop(_)));
    }
    assert_eq!(out, [0xA5; MAX_FRAME]);
    assert_eq!(stack.dropped, 10000);
}

#[derive(Default)]
struct FakeLink {
    sent: Vec<Vec<u8>>,
    incoming: VecDeque<Vec<u8>>,
    send_error: bool,
    down: bool,
    oversized: bool,
    polls: usize,
}
impl Link for Rc<RefCell<FakeLink>> {
    fn mac(&self) -> [u8; 6] { MAC }
    fn link_up(&self) -> bool { !self.borrow().down }
    fn send(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        let mut link = self.borrow_mut();
        if link.send_error { return Err("test TX failure"); }
        link.sent.push(frame.to_vec());
        Ok(())
    }
    fn receive(&mut self, out: &mut [u8; MAX_FRAME]) -> Option<usize> {
        let mut link = self.borrow_mut();
        link.polls += 1;
        if link.oversized { return Some(MAX_FRAME + 1); }
        let frame = link.incoming.pop_front()?;
        out[..frame.len()].copy_from_slice(&frame);
        Some(frame.len())
    }
}
fn service() -> (Network<Rc<RefCell<FakeLink>>>, Rc<RefCell<FakeLink>>) {
    let link = Rc::new(RefCell::new(FakeLink::default()));
    (Network::new(link.clone(), 42, policy()), link)
}

#[test]
fn service_round_trip_and_replay_are_one_shot() {
    let (mut service, link) = service();
    assert_eq!(service.status(), Status::Idle);
    service.start(10).unwrap();
    assert!(service.start(10).is_err());
    link.borrow_mut().incoming.push_back(arp(2));
    assert_eq!(service.poll(20), Ok(Status::Waiting));
    let request = link.borrow().sent.last().unwrap().clone();
    link.borrow_mut().incoming.push_back(echo(&request));
    assert_eq!(service.poll(30), Ok(Status::Reply));
    assert_eq!(service.counters(), (2, 0));
    assert_eq!(service.poll(10000), Ok(Status::Reply));
    assert_eq!(link.borrow().sent.len(), 2);
}

#[test]
fn service_retries_then_times_out_at_exact_deadline() {
    let (mut service, link) = service();
    service.start(100).unwrap();
    assert_eq!(service.poll(1099), Ok(Status::Waiting));
    assert_eq!(link.borrow().sent.len(), 1);
    for time in [1100, 2100, 3100, 4100] { service.poll(time).unwrap(); }
    assert_eq!(link.borrow().sent.len(), 5);
    assert_eq!(service.poll(5099), Ok(Status::Waiting));
    assert_eq!(service.poll(5100), Ok(Status::TimedOut));
    assert_eq!(service.poll(6100), Ok(Status::TimedOut));
    assert_eq!(link.borrow().sent.len(), 5);
}

#[test]
fn service_limits_flood_processing_to_sixteen_frames() {
    let (mut service, link) = service();
    service.start(0).unwrap();
    for _ in 0..100 { link.borrow_mut().incoming.push_back(vec![0; 14]); }
    service.poll(1).unwrap();
    assert_eq!(link.borrow().polls, 16);
    assert_eq!(service.counters(), (16, 16));
}

#[test]
fn service_surfaces_policy_link_clock_and_device_errors() {
    let (mut service, link) = service();
    link.borrow_mut().send_error = true;
    assert_eq!(service.start(0), Err("test TX failure"));
    assert_eq!(service.status(), Status::Failed);
    let (mut service, link) = self::service();
    link.borrow_mut().down = true;
    assert_eq!(service.start(0), Err("Network link is down"));
    let (mut service, _) = self::service();
    service.start(10).unwrap();
    assert_eq!(service.poll(9), Err("Network clock moved backwards"));
    let (mut service, link) = self::service();
    service.start(0).unwrap();
    link.borrow_mut().oversized = true;
    assert_eq!(service.poll(1), Err("Network device returned an invalid frame length"));
    let link = Rc::new(RefCell::new(FakeLink::default()));
    let mut service = Network::new(link.clone(), 0, Policy::deny_all());
    assert_eq!(service.start(0), Err("Network probe denied by egress policy"));
    assert!(link.borrow().sent.is_empty());
}

#[test]
fn service_reuses_device_for_bounded_user_messages() {
    let (mut service, link) = service();
    assert!(service.reset(&[]).is_err());
    assert!(service.reset(&[b'x'; MAX_PAYLOAD + 1]).is_err());
    service.reset(b"hello").unwrap();
    service.start(1).unwrap();
    assert!(service.reset(b"busy").is_err());
    link.borrow_mut().incoming.push_back(arp(2));
    assert_eq!(service.poll(2), Ok(Status::Waiting));
    let request = link.borrow().sent.last().unwrap().clone();
    assert_eq!(&request[42..], b"hello");
    link.borrow_mut().incoming.push_back(echo(&request));
    assert_eq!(service.poll(3), Ok(Status::Reply));
    service.reset(&[b'z'; MAX_PAYLOAD]).unwrap();
    service.start(4).unwrap();
    let second = link.borrow().sent.last().unwrap().clone();
    assert_eq!(&second[42..], &[b'z'; MAX_PAYLOAD]);
    link.borrow_mut().incoming.push_back(echo(&request));
    link.borrow_mut().incoming.push_back(echo(&second));
    assert_eq!(service.poll(5), Ok(Status::Reply));
    assert_eq!(service.counters(), (4, 1));
}

#[test]
fn idle_service_drains_unsolicited_frames_with_a_strict_work_limit() {
    let (mut service, link) = service();
    for _ in 0..20 { link.borrow_mut().incoming.push_back(vec![0; 14]); }
    service.drain_idle().unwrap();
    assert_eq!(link.borrow().polls, 16);
    assert_eq!(service.counters(), (16, 16));
    service.drain_idle().unwrap();
    assert_eq!(service.counters(), (20, 20));
    assert!(link.borrow().sent.is_empty());
}

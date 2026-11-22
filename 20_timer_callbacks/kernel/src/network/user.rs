// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded request mailbox between the current ring-3 shell and CPU 0.

use crate::synchronization::{interface::Mutex, SpinLock};
use super::stack::{GATEWAY_IP, MAX_PAYLOAD, PEER_PORT};
use core::sync::atomic::{AtomicBool, Ordering};

pub const PENDING: usize = 0;
pub const SUCCESS: usize = 1;
pub const DISABLED: usize = 2;
pub const DENIED: usize = 3;
pub const INVALID: usize = 4;
pub const BUSY: usize = 5;
pub const TIMEOUT: usize = 6;
pub const FAILED: usize = 7;

const EMPTY: u8 = 0;
const QUEUED: u8 = 1;
const ACTIVE: u8 = 2;
const COMPLETE: u8 = 3;

struct Mailbox {
    state: u8,
    payload: [u8; MAX_PAYLOAD],
    length: usize,
    result: usize,
}
static MAILBOX: SpinLock<Mailbox> = SpinLock::new(Mailbox {
    state: EMPTY, payload: [0; MAX_PAYLOAD], length: 0, result: FAILED,
});
static AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn set_available(value: bool) { AVAILABLE.store(value, Ordering::Release); }
pub fn available() -> bool { AVAILABLE.load(Ordering::Acquire) }

/// The kernel copies and validates the whole request before submitting it.
pub fn submit(command: &[u8]) -> usize {
    let command = match core::str::from_utf8(command) { Ok(text) => text, Err(_) => return INVALID };
    let mut parts = command.splitn(3, ' ');
    let address = match parts.next() { Some(value) => value, None => return INVALID };
    let port = match parts.next() { Some(value) => value, None => return INVALID };
    let message = match parts.next() { Some(value) => value.as_bytes(), None => return INVALID };
    let mut parsed = [0u8; 4];
    let mut octets = address.split('.');
    for value in &mut parsed {
        *value = match octets.next().and_then(|part| part.parse::<u8>().ok()) {
            Some(number) => number, None => return INVALID,
        };
    }
    if octets.next().is_some() { return INVALID; }
    let port = match port.parse::<u16>() { Ok(value) if value != 0 => value, _ => return INVALID };
    if message.is_empty() || message.len() > MAX_PAYLOAD || message.iter().any(|byte| !(b' '..=b'~').contains(byte)) {
        return INVALID;
    }
    if parsed != GATEWAY_IP || port != PEER_PORT { return DENIED; }
    MAILBOX.lock(|mail| {
        if !available() { return DISABLED; }
        if mail.state != EMPTY { return BUSY; }
        mail.payload[..message.len()].copy_from_slice(message);
        mail.length = message.len();
        mail.state = QUEUED;
        PENDING
    })
}

pub fn take_request(output: &mut [u8; MAX_PAYLOAD]) -> Option<usize> {
    MAILBOX.lock(|mail| {
        if mail.state != QUEUED { return None; }
        output[..mail.length].copy_from_slice(&mail.payload[..mail.length]);
        mail.state = ACTIVE;
        Some(mail.length)
    })
}

pub fn complete(result: usize) {
    MAILBOX.lock(|mail| {
        if mail.state == ACTIVE || mail.state == QUEUED {
            mail.result = result;
            mail.state = COMPLETE;
        }
    });
}

pub fn result() -> usize {
    MAILBOX.lock(|mail| {
        if mail.state == COMPLETE {
            let result = mail.result;
            mail.state = EMPTY;
            result
        } else if mail.state == EMPTY { FAILED } else { PENDING }
    })
}

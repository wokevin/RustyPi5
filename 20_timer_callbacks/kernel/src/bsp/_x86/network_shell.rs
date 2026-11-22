// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ring-3 commands for the bounded kernel UDP echo interface.

use super::user;

macro_rules! text {
    ($name:ident, $value:expr) => {
        #[link_section = ".user_data"]
        static $name: [u8; $value.len()] = *$value;
    };
}
text!(HELP, b"net status | udp <IPv4> <port> <message>\r\n");
text!(UP, b"network: enabled; local firewall default deny; UDP 10.0.2.2:8081 allowed\r\n");
text!(DOWN, b"network: disabled\r\n");
text!(DENIED, b"udp: denied by local firewall\r\n");
text!(INVALID, b"udp: usage: udp <IPv4> <port> <message> (1-96 printable bytes)\r\n");
text!(BUSY, b"udp: another request is in progress\r\n");
text!(TIMEOUT, b"udp: timed out\r\n");
text!(FAILED, b"udp: network request failed\r\n");
text!(REPLY, b"reply: ");
text!(NEWLINE, b"\r\n");

#[link_section = ".user_text"]
unsafe fn emit(bytes: &'static [u8]) { user::user_print(bytes.as_ptr(), bytes.len()); }

#[link_section = ".user_text"]
pub unsafe fn help() { emit(&HELP); }

#[link_section = ".user_text"]
pub unsafe fn status() {
    if user::user_syscall(user::SYS_NET_STATUS, 0, 0) == 1 { emit(&UP); }
    else { emit(&DOWN); }
}

#[link_section = ".user_text"]
pub unsafe fn udp(input: *const u8, length: usize) {
    let mut result = user::user_syscall(user::SYS_NET_SEND, input as usize, length);
    if result == 0 {
        loop {
            result = user::user_syscall(user::SYS_NET_RESULT, 0, 0);
            if result != 0 { break; }
            core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
        }
    }
    match result {
        1 => {
            let mut index = 0;
            let mut spaces = 0;
            while index < length && spaces < 2 {
                if input.add(index).read() == b' ' { spaces += 1; }
                index += 1;
            }
            emit(&REPLY);
            user::user_print(input.add(index), length - index);
            emit(&NEWLINE);
        }
        2 => emit(&DOWN),
        3 => emit(&DENIED),
        4 => emit(&INVALID),
        5 => emit(&BUSY),
        6 => emit(&TIMEOUT),
        _ => emit(&FAILED),
    }
}

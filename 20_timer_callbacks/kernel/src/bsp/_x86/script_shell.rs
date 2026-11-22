// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ring-3 script commands and session-local script search path.

use super::{script, user};

const CAPACITY: usize = 4096;
const PATH_CAPACITY: usize = 127;

macro_rules! text {
    ($name:ident, $value:expr) => {
        #[link_section = ".user_data"]
        static $name: [u8; $value.len()] = *$value;
    };
}
text!(DEFAULT_PATH, b"/home/usera/programs");
text!(SCRIPT_PROMPT, b"script> ");
text!(SCRIPT_HELP, b"Enter statements; .end runs, .cancel discards.\r\n");
text!(TOO_LONG, b"script: input too long\r\n");
text!(PATH_ERROR, b"path: use absolute directories separated by colons (maximum 127 bytes)\r\n");
text!(PATH_PREFIX, b"PATH=");
text!(RUN_USAGE, b"Usage: run <script path or name>\r\n");
text!(NOT_FOUND, b"run: script unavailable\r\n");
text!(LOAD_ERROR, b"run: script could not be loaded; see kernel diagnostic\r\n");
text!(ERROR_PREFIX, b"script: line ");
text!(ERROR_SEPARATOR, b": ");
text!(ERR_SYNTAX, b"syntax error");
text!(ERR_NAME, b"unknown or duplicate variable");
text!(ERR_ARITHMETIC, b"arithmetic error");
text!(ERR_LIMIT, b"execution or nesting limit exceeded");
text!(ERR_LEXICAL, b"invalid character or string");
text!(NEWLINE, b"\r\n");

#[link_section = ".user_data"]
static mut SOURCE: [u8; CAPACITY] = [0; CAPACITY];
#[link_section = ".user_data"]
static mut PATH: [u8; PATH_CAPACITY] = [0; PATH_CAPACITY];
#[link_section = ".user_data"]
static mut PATH_LENGTH: usize = 0;

#[link_section = ".user_text"]
fn emit(pointer: *const u8, length: usize) {
    let mut offset = 0;
    while offset < length {
        let count = core::cmp::min(1024, length - offset);
        unsafe { user::user_print(pointer.add(offset), count); }
        offset += count;
    }
}

#[link_section = ".user_text"]
fn print_line_number(mut value: usize) {
    let mut buffer = [0u8; 20];
    let mut at = 20;
    loop {
        at -= 1;
        unsafe { *buffer.as_mut_ptr().add(at) = b'0' + (value % 10) as u8; }
        value /= 10;
        if value == 0 { break; }
    }
    emit(unsafe { buffer.as_ptr().add(at) }, 20 - at);
}

#[link_section = ".user_text"]
fn execute(length: usize) {
    let source = unsafe { core::ptr::addr_of!(SOURCE) as *const u8 };
    let result = unsafe { script::run(source, length, emit) };
    if let Err(error) = result {
        emit(ERROR_PREFIX.as_ptr(), ERROR_PREFIX.len());
        print_line_number(error.line);
        emit(ERROR_SEPARATOR.as_ptr(), ERROR_SEPARATOR.len());
        match error.code {
            script::SYNTAX => emit(ERR_SYNTAX.as_ptr(), ERR_SYNTAX.len()),
            script::NAME => emit(ERR_NAME.as_ptr(), ERR_NAME.len()),
            script::ARITHMETIC => emit(ERR_ARITHMETIC.as_ptr(), ERR_ARITHMETIC.len()),
            script::LIMIT => emit(ERR_LIMIT.as_ptr(), ERR_LIMIT.len()),
            _ => emit(ERR_LEXICAL.as_ptr(), ERR_LEXICAL.len()),
        }
        emit(NEWLINE.as_ptr(), NEWLINE.len());
    }
}

#[link_section = ".user_text"]
pub unsafe fn init() {
    let path = core::ptr::addr_of_mut!(PATH) as *mut u8;
    let mut index = 0;
    while index < DEFAULT_PATH.len() {
        path.add(index).write(DEFAULT_PATH.as_ptr().add(index).read());
        index += 1;
    }
    core::ptr::addr_of_mut!(PATH_LENGTH).write(index);
}

#[link_section = ".user_text"]
pub unsafe fn path(input: *const u8, length: usize) {
    let path = core::ptr::addr_of_mut!(PATH) as *mut u8;
    if length != 0 {
        let mut valid = length <= PATH_CAPACITY;
        let mut index = 0;
        let mut first = true;
        while index < length {
            let byte = input.add(index).read();
            if first && byte != b'/' { valid = false; }
            if byte == b':' { if first || index + 1 == length { valid = false; } first = true; }
            else { if byte <= b' ' || byte > b'~' { valid = false; } first = false; }
            index += 1;
        }
        if !valid { emit(PATH_ERROR.as_ptr(), PATH_ERROR.len()); return; }
        index = 0;
        while index < length { path.add(index).write(input.add(index).read()); index += 1; }
        core::ptr::addr_of_mut!(PATH_LENGTH).write(length);
    }
    emit(PATH_PREFIX.as_ptr(), PATH_PREFIX.len());
    emit(path, core::ptr::addr_of!(PATH_LENGTH).read());
    emit(NEWLINE.as_ptr(), NEWLINE.len());
}

#[link_section = ".user_text"]
pub unsafe fn interactive(line: *mut u8) {
    emit(SCRIPT_HELP.as_ptr(), SCRIPT_HELP.len());
    let source = core::ptr::addr_of_mut!(SOURCE) as *mut u8;
    let mut used = 0;
    loop {
        emit(SCRIPT_PROMPT.as_ptr(), SCRIPT_PROMPT.len());
        let length = user::user_syscall(user::SYS_READ, line as usize, 128);
        if length == 0 || length == usize::MAX || length >= 128 {
            emit(TOO_LONG.as_ptr(), TOO_LONG.len());
            return;
        }
        if length == 5 && line.read() == b'.' && line.add(1).read() == b'e'
            && line.add(2).read() == b'n' && line.add(3).read() == b'd' {
            execute(used);
            return;
        }
        if length == 8 && line.read() == b'.' && line.add(1).read() == b'c'
            && line.add(2).read() == b'a' && line.add(3).read() == b'n'
            && line.add(4).read() == b'c' && line.add(5).read() == b'e' && line.add(6).read() == b'l' { return; }
        if used + length > CAPACITY { emit(TOO_LONG.as_ptr(), TOO_LONG.len()); return; }
        let mut index = 0;
        while index < length { source.add(used + index).write(line.add(index).read()); index += 1; }
        used += length;
    }
}

#[link_section = ".user_text"]
unsafe fn load(path: *const u8, length: usize) -> usize {
    let request = [path as usize, length, core::ptr::addr_of_mut!(SOURCE) as usize, CAPACITY];
    user::user_syscall(user::SYS_SCRIPT_LOAD, request.as_ptr() as usize, core::mem::size_of_val(&request))
}

#[link_section = ".user_text"]
pub unsafe fn run(path: *const u8, length: usize) {
    if length == 0 || length > 127 { emit(RUN_USAGE.as_ptr(), RUN_USAGE.len()); return; }
    let mut index = 0;
    let mut has_slash = false;
    while index < length { if path.add(index).read() == b'/' { has_slash = true; } index += 1; }
    let mut result = usize::MAX - 1;
    if has_slash {
        result = load(path, length);
    } else {
        let dirs = core::ptr::addr_of!(PATH) as *const u8;
        let path_length = core::ptr::addr_of!(PATH_LENGTH).read();
        let mut start = 0;
        let mut candidate = [0u8; 127];
        while start < path_length {
            let mut end = start;
            while end < path_length && dirs.add(end).read() != b':' { end += 1; }
            let dir_length = end - start;
            let separator = if dirs.add(end - 1).read() == b'/' { 0 } else { 1 };
            if dir_length + separator + length > 127 {
                emit(PATH_ERROR.as_ptr(), PATH_ERROR.len());
                return;
            }
            index = 0;
            while index < dir_length { candidate.as_mut_ptr().add(index).write_volatile(dirs.add(start + index).read()); index += 1; }
            if separator != 0 { candidate.as_mut_ptr().add(index).write(b'/'); index += 1; }
            let mut offset = 0;
            while offset < length { candidate.as_mut_ptr().add(index + offset).write_volatile(path.add(offset).read()); offset += 1; }
            result = load(candidate.as_ptr(), index + length);
            if result != usize::MAX - 1 { break; }
            start = end + 1;
        }
    }
    if result == usize::MAX - 1 { emit(NOT_FOUND.as_ptr(), NOT_FOUND.len()); }
    else if result == usize::MAX { emit(LOAD_ERROR.as_ptr(), LOAD_ERROR.len()); }
    else if result <= CAPACITY { execute(result); }
    else { emit(LOAD_ERROR.as_ptr(), LOAD_ERROR.len()); }
}

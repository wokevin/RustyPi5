// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded C-style interpreter for the ring-3 demo shell.

const MAX_VARS: usize = 32;
const MAX_DEPTH: usize = 16;
const MAX_STEPS: u32 = 10000;
const IDENT: u8 = 128;
const NUMBER: u8 = 129;
const STRING: u8 = 130;
const EQ: u8 = 131;
const NE: u8 = 132;
const LE: u8 = 133;
const GE: u8 = 134;
const END: u8 = 135;

#[derive(Clone, Copy)]
struct Token { kind: u8, start: usize, end: usize }

#[derive(Clone, Copy)]
struct Variable { start: usize, end: usize, value: i64 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error { pub line: usize, pub code: u8 }

// Error codes are translated by the shell, so diagnostic strings stay in user memory.
pub const SYNTAX: u8 = 1;
pub const NAME: u8 = 2;
pub const ARITHMETIC: u8 = 3;
pub const LIMIT: u8 = 4;
pub const LEXICAL: u8 = 5;

struct Parser {
    source: *const u8,
    length: usize,
    position: usize,
    vars: [core::mem::MaybeUninit<Variable>; MAX_VARS],
    count: usize,
    steps: u32,
    depth: usize,
    emit: fn(*const u8, usize),
}

#[cfg_attr(not(test), link_section = ".user_data")]
static NEWLINE: [u8; 2] = *b"\r\n";

impl Parser {
    #[cfg_attr(not(test), link_section = ".user_text")]
    fn byte(&self, index: usize) -> u8 {
        if index < self.length { unsafe { *self.source.add(index) } } else { 0 }
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn error(&self, code: u8) -> Error {
        let mut line = 1;
        let mut index = 0;
        while index < self.position && index < self.length {
            if self.byte(index) == b'\n' { line += 1; }
            index += 1;
        }
        Error { line, code }
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn tick(&mut self) -> Result<(), Error> {
        if self.steps == 0 { return Err(self.error(LIMIT)); }
        self.steps -= 1;
        Ok(())
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn enter(&mut self) -> Result<(), Error> {
        if self.depth >= MAX_DEPTH { return Err(self.error(LIMIT)); }
        self.depth += 1;
        Ok(())
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn next(&mut self) -> Result<Token, Error> {
        self.tick()?;
        loop {
            while matches!(self.byte(self.position), b' ' | b'\t' | b'\r' | b'\n') {
                self.position += 1;
            }
            if self.byte(self.position) == b'/' && self.byte(self.position + 1) == b'/' {
                while self.position < self.length && self.byte(self.position) != b'\n' {
                    self.position += 1;
                }
            } else { break; }
        }
        let start = self.position;
        if start == self.length { return Ok(Token { kind: END, start, end: start }); }
        let c = self.byte(start);
        self.position += 1;
        let kind = if c.is_ascii_alphabetic() || c == b'_' {
            while self.byte(self.position).is_ascii_alphanumeric() || self.byte(self.position) == b'_' {
                self.position += 1;
            }
            IDENT
        } else if c.is_ascii_digit() {
            while self.byte(self.position).is_ascii_digit() { self.position += 1; }
            NUMBER
        } else if c == b'"' {
            while self.position < self.length && self.byte(self.position) != b'"' {
                let ch = self.byte(self.position);
                if !(b' '..=b'~').contains(&ch) || ch == b'\\' { return Err(self.error(LEXICAL)); }
                self.position += 1;
            }
            if self.position == self.length { return Err(self.error(LEXICAL)); }
            self.position += 1;
            STRING
        } else if c == b'=' && self.byte(self.position) == b'=' {
            self.position += 1; EQ
        } else if c == b'!' && self.byte(self.position) == b'=' {
            self.position += 1; NE
        } else if c == b'<' && self.byte(self.position) == b'=' {
            self.position += 1; LE
        } else if c == b'>' && self.byte(self.position) == b'=' {
            self.position += 1; GE
        } else if matches!(c, b'(' | b')' | b'{' | b'}' | b';' | b'=' | b'+' | b'-' | b'*' | b'/' | b'<' | b'>') {
            c
        } else { return Err(self.error(LEXICAL)); };
        Ok(Token { kind, start, end: self.position })
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn peek(&mut self) -> Result<Token, Error> {
        let position = self.position;
        let token = self.next()?;
        self.position = position;
        Ok(token)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn expect(&mut self, kind: u8) -> Result<Token, Error> {
        let token = self.next()?;
        if token.kind != kind { return Err(self.error(SYNTAX)); }
        Ok(token)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn keyword(&self, token: Token, a: u8, b: u8, c: u8, d: u8, e: u8) -> bool {
        let len = token.end - token.start;
        let expected = if e != 0 { 5 } else if d != 0 { 4 } else if c != 0 { 3 } else { 2 };
        token.kind == IDENT && len == expected && self.byte(token.start) == a
            && self.byte(token.start + 1) == b && (expected < 3 || self.byte(token.start + 2) == c)
            && (expected < 4 || self.byte(token.start + 3) == d)
            && (expected < 5 || self.byte(token.start + 4) == e)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn variable(&self, token: Token) -> Option<usize> {
        let mut index = 0;
        while index < self.count {
            let var = unsafe { (*self.vars.as_ptr().add(index)).assume_init() };
            if var.end - var.start == token.end - token.start {
                let mut offset = 0;
                while offset < token.end - token.start && self.byte(var.start + offset) == self.byte(token.start + offset) { offset += 1; }
                if offset == token.end - token.start { return Some(index); }
            }
            index += 1;
        }
        None
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn number(&mut self, token: Token) -> Result<i64, Error> {
        let mut value = 0i64;
        let mut index = token.start;
        while index < token.end {
            value = value.checked_mul(10).and_then(|v| v.checked_add((self.byte(index) - b'0') as i64))
                .ok_or_else(|| self.error(ARITHMETIC))?;
            index += 1;
        }
        Ok(value)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn atom(&mut self, active: bool) -> Result<i64, Error> {
        self.enter()?;
        let token = self.next()?;
        let value = match token.kind {
            NUMBER => self.number(token)?,
            IDENT => {
                if active {
                    let index = self.variable(token).ok_or_else(|| self.error(NAME))?;
                    unsafe { (*self.vars.as_ptr().add(index)).assume_init().value }
                } else { 0 }
            }
            b'(' => { let value = self.expression(active)?; self.expect(b')')?; value }
            b'-' => {
                let value = self.atom(active)?;
                if active { value.checked_neg().ok_or_else(|| self.error(ARITHMETIC))? } else { 0 }
            }
            _ => return Err(self.error(SYNTAX)),
        };
        self.depth -= 1;
        Ok(value)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn product(&mut self, active: bool) -> Result<i64, Error> {
        let mut left = self.atom(active)?;
        loop {
            let op = self.peek()?.kind;
            if op != b'*' && op != b'/' { break; }
            self.next()?;
            let right = self.atom(active)?;
            if active {
                left = if op == b'*' { left.checked_mul(right) } else { left.checked_div(right) }
                    .ok_or_else(|| self.error(ARITHMETIC))?;
            }
        }
        Ok(left)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn sum(&mut self, active: bool) -> Result<i64, Error> {
        let mut left = self.product(active)?;
        loop {
            let op = self.peek()?.kind;
            if op != b'+' && op != b'-' { break; }
            self.next()?;
            let right = self.product(active)?;
            if active {
                left = if op == b'+' { left.checked_add(right) } else { left.checked_sub(right) }
                    .ok_or_else(|| self.error(ARITHMETIC))?;
            }
        }
        Ok(left)
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn expression(&mut self, active: bool) -> Result<i64, Error> {
        let left = self.sum(active)?;
        let op = self.peek()?.kind;
        if !matches!(op, EQ | NE | LE | GE | b'<' | b'>') { return Ok(left); }
        self.next()?;
        let right = self.sum(active)?;
        Ok(if active && match op {
            EQ => left == right, NE => left != right, LE => left <= right,
            GE => left >= right, b'<' => left < right, _ => left > right,
        } { 1 } else { 0 })
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn print_number(&self, value: i64) {
        let mut bytes = [0u8; 21];
        let mut position = 21;
        let mut n = value.unsigned_abs();
        loop {
            position -= 1;
            unsafe { *bytes.as_mut_ptr().add(position) = b'0' + (n % 10) as u8; }
            n /= 10;
            if n == 0 { break; }
        }
        if value < 0 { position -= 1; unsafe { *bytes.as_mut_ptr().add(position) = b'-'; } }
        (self.emit)(unsafe { bytes.as_ptr().add(position) }, 21 - position);
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn block(&mut self, active: bool) -> Result<(), Error> {
        self.enter()?;
        self.expect(b'{')?;
        while self.peek()?.kind != b'}' {
            if self.peek()?.kind == END { return Err(self.error(SYNTAX)); }
            self.statement(active)?;
        }
        self.expect(b'}')?;
        self.depth -= 1;
        Ok(())
    }

    #[cfg_attr(not(test), link_section = ".user_text")]
    fn statement(&mut self, active: bool) -> Result<(), Error> {
        self.tick()?;
        let token = self.expect(IDENT)?;
        if self.keyword(token, b'l', b'e', b't', 0, 0) {
            let name = self.expect(IDENT)?;
            self.expect(b'=')?;
            let value = self.expression(active)?;
            self.expect(b';')?;
            if active {
                if self.variable(name).is_some() { return Err(self.error(NAME)); }
                if self.count == MAX_VARS { return Err(self.error(LIMIT)); }
                unsafe { (*self.vars.as_mut_ptr().add(self.count)).write(Variable { start: name.start, end: name.end, value }); }
                self.count += 1;
            }
        } else if self.keyword(token, b'p', b'r', b'i', b'n', b't') {
            self.expect(b'(')?;
            let next = self.peek()?;
            if next.kind == STRING {
                let text = self.next()?;
                self.expect(b')')?;
                self.expect(b';')?;
                if active { (self.emit)(unsafe { self.source.add(text.start + 1) }, text.end - text.start - 2); }
            } else {
                let value = self.expression(active)?;
                self.expect(b')')?;
                self.expect(b';')?;
                if active { self.print_number(value); }
            }
            if active { (self.emit)(NEWLINE.as_ptr(), 2); }
        } else if self.keyword(token, b'i', b'f', 0, 0, 0) {
            self.expect(b'(')?;
            let value = self.expression(active)?;
            self.expect(b')')?;
            self.block(active && value != 0)?;
            let next = self.peek()?;
            if self.keyword(next, b'e', b'l', b's', b'e', 0) {
                self.next()?;
                self.block(active && value == 0)?;
            }
        } else if self.keyword(token, b'w', b'h', b'i', b'l', b'e') {
            let start = self.position;
            loop {
                self.tick()?;
                self.position = start;
                self.expect(b'(')?;
                let value = self.expression(active)?;
                self.expect(b')')?;
                self.block(active && value != 0)?;
                if !active || value == 0 { break; }
            }
        } else {
            self.expect(b'=')?;
            let value = self.expression(active)?;
            self.expect(b';')?;
            if active {
                let index = self.variable(token).ok_or_else(|| self.error(NAME))?;
                unsafe { (*(*self.vars.as_mut_ptr().add(index)).as_mut_ptr()).value = value; }
            }
        }
        Ok(())
    }
}

/// The source must remain readable for `length` bytes until execution finishes.
#[cfg_attr(not(test), link_section = ".user_text")]
pub unsafe fn run(source: *const u8, length: usize, emit: fn(*const u8, usize)) -> Result<(), Error> {
    let mut parser = Parser {
        source, length, position: 0,
        vars: [core::mem::MaybeUninit::uninit(); MAX_VARS],
        count: 0, steps: MAX_STEPS, depth: 0, emit,
    };
    while parser.peek()?.kind != END { parser.statement(true)?; }
    Ok(())
}

// SPDX-License-Identifier: MIT OR Apache-2.0

//! Filesystem operations for the single demo shell; identity is kernel-owned, not syscall input.

use super::{ata::Ata, Filesystem, Identity};
use crate::{
    bsp,
    console::interface::Write,
    synchronization::{interface::Mutex, SpinLock},
};

pub const SYS_LS: u64 = 3;
pub const SYS_PWD: u64 = 4;
pub const SYS_CD: u64 = 5;
pub const SYS_CAT: u64 = 6;
const USER: Identity = Identity {
    uid: 1000,
    gid: 1000,
};

struct Session {
    disk: Ata,
    fs: Filesystem,
    cwd: u32,
}

static SESSION: SpinLock<Option<Session>> = SpinLock::new(None);

pub fn init() -> Result<(), &'static str> {
    let mut disk = Ata::open()?;
    let fs = Filesystem::mount(&mut disk)?;
    fs.lookup(&mut disk, 0, b"/", USER)?;
    SESSION.lock(|session| *session = Some(Session { disk, fs, cwd: 0 }));
    Ok(())
}

fn emit(bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            bsp::console::UART.write_char('\r');
        }
        // Files cannot inject terminal control sequences into the shell.
        bsp::console::UART.write_char(match byte {
            b'\n' | b'\t' | b' '..=b'~' => byte as char,
            _ => '.',
        });
    }
}

pub fn read_file(path: &[u8], output: &mut [u8]) -> Result<usize, &'static str> {
    SESSION.lock(|session| {
        let session = session.as_mut().ok_or("Filesystem not mounted")?;
        let Session { disk, fs, cwd } = session;
        let inode = fs.lookup(disk, *cwd, path, USER)?;
        fs.read_file(disk, inode, USER, output)
    })
}

pub fn execute(operation: u64, path: &[u8]) -> usize {
    let result = SESSION.lock(|session| {
        let session = session.as_mut().ok_or("Filesystem not mounted")?;
        let Session { disk, fs, cwd } = session;
        match operation {
            SYS_PWD => {
                if !path.is_empty() {
                    return Err("pwd takes no path");
                }
                fs.pwd(disk, *cwd, USER, emit)
            }
            SYS_LS => {
                let inode = fs.lookup(disk, *cwd, path, USER)?;
                if fs.node(disk, inode)?.directory {
                    fs.list(disk, inode, USER, emit)
                } else {
                    emit(fs.node(disk, inode)?.name());
                    emit(b"\n");
                    Ok(())
                }
            }
            SYS_CD => {
                let path = if path.is_empty() {
                    &b"/home/usera"[..]
                } else {
                    path
                };
                let inode = fs.lookup(disk, *cwd, path, USER)?;
                if !fs.node(disk, inode)?.directory {
                    return Err("Not a directory");
                }
                *cwd = inode;
                Ok(())
            }
            SYS_CAT => {
                if path.is_empty() {
                    return Err("Usage: cat <path>");
                }
                let inode = fs.lookup(disk, *cwd, path, USER)?;
                fs.cat(disk, inode, USER, emit)
            }
            _ => Err("Unknown filesystem operation"),
        }
    });
    match result {
        Ok(()) => 0,
        Err(message) => {
            emit(b"filesystem: ");
            emit(message.as_bytes());
            emit(b"\n");
            usize::MAX
        }
    }
}

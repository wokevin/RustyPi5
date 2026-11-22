// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2024 Kevin Wood <kevin.j.wood@gmail.com>

//! Ring-3 shell and its minimal syscall boundary for QEMU.

use core::arch::{asm, global_asm};
#[cfg(feature = "bsp_qemu_x86")]
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{bsp, console::interface::Write};

#[cfg(feature = "bsp_qemu_x86")]
static PREPARED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "bsp_qemu_x86")]
static SHELL_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(feature = "bsp_qemu_x86")]
static SHELL_STARTED: AtomicBool = AtomicBool::new(false);

const PAGE_SIZE: usize = 4096;
const USER_BASE: usize = 0x1000_0000;
const USER_REGION_SIZE: usize = 2 * 1024 * 1024;
const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const HUGE: u64 = 1 << 7;
const NO_EXECUTE: u64 = 1 << 63;
const SYS_WRITE: u64 = 1;
pub(super) const SYS_READ: u64 = 2;
const SYS_LS: u64 = 3;
const SYS_PWD: u64 = 4;
const SYS_CD: u64 = 5;
const SYS_CAT: u64 = 6;
#[cfg(feature = "bsp_qemu_x86")]
pub(super) const SYS_SCRIPT_LOAD: u64 = 7;
#[cfg(feature = "bsp_qemu_x86")]
pub(super) const SYS_NET_STATUS: u64 = 8;
#[cfg(feature = "bsp_qemu_x86")]
pub(super) const SYS_NET_SEND: u64 = 9;
#[cfg(feature = "bsp_qemu_x86")]
pub(super) const SYS_NET_RESULT: u64 = 10;

#[repr(align(4096))]
#[derive(Clone, Copy)]
struct PageTable([u64; 512]);

static mut PML4: PageTable = PageTable([0; 512]);
static mut PDPT: PageTable = PageTable([0; 512]);
static mut PAGE_DIRECTORIES: [PageTable; 4] = [PageTable([0; 512]); 4];
static mut USER_PAGE_TABLE: PageTable = PageTable([0; 512]);
#[cfg(feature = "bsp_qemu_x86")]
static mut DEVICE_PAGE_TABLE: PageTable = PageTable([0; 512]);
#[cfg(feature = "bsp_qemu_x86")]
static DEVICE_MAPPED: AtomicBool = AtomicBool::new(false);

extern "C" {
    static __user_text_start: u8;
    static __user_text_end: u8;
    static __user_rodata_start: u8;
    static __user_rodata_end: u8;
    static __user_data_start: u8;
    static __user_data_end: u8;
    static __user_stack_start: u8;
    static __user_stack_end: u8;
    fn user_syscall_entry();
}

#[link_section = ".user_data"]
static WELCOME: [u8; 49] = *b"User A shell (ring 3, unprivileged). Type help.\r\n";
#[link_section = ".user_data"]
static PROMPT: [u8; 17] = *b"User A [ring 3]$ ";
#[link_section = ".user_data"]
static HELP: [u8; 245 + b"pwd | ls [path] | cd [path] | cat <path> (read-only filesystem)\r\n".len()] = *b"Commands:\r\n\
  help       List commands\r\n\
  id         Show current demo identity\r\n\
  echo TEXT  Print text (no quoting or expansion)\r\n\
  clear      Clear an ANSI serial terminal (not the VM display yet)\r\n\
  exit       Stop this shell (no process cleanup yet)\r\n\
pwd | ls [path] | cd [path] | cat <path> (read-only filesystem)\r\n";
#[cfg(feature = "bsp_qemu_x86")]
#[link_section = ".user_data"]
static SCRIPT_HELP: [u8; b"script | run <file> | path [directories]\r\n".len()] = *b"script | run <file> | path [directories]\r\n";
#[link_section = ".user_data"]
static ID: [u8; 43] = *b"uid=1000(User A) euid=1000(User A) ring=3\r\n";
#[link_section = ".user_data"]
static UNKNOWN: [u8; 18] = *b"Unknown command.\r\n";
#[link_section = ".user_data"]
static GOODBYE: [u8; 16] = *b"Shell stopped.\r\n";
#[link_section = ".user_data"]
static CLEAR: [u8; 7] = *b"\x1b[2J\x1b[H";
#[link_section = ".user_data"]
static LINE_END: [u8; 2] = *b"\r\n";
#[link_section = ".user_data"]
static mut LINE: [u8; 128] = [0; 128];

#[cfg(target_arch = "x86_64")]
global_asm!(
    ".global user_syscall_entry",
    "user_syscall_entry:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov rdi, rsp",
    "call syscall_dispatch",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
);

#[cfg(feature = "bsp_qemu_x86")]
pub fn prepare() -> Result<(), &'static str> {
    if bsp::cpu::current_core_id().raw() != 0 { return Err("Only boot CPU may prepare user mappings"); }
    if PREPARED.load(Ordering::Acquire) { return Err("User image already prepared"); }
    if bsp::cpu::core_count() < 2 {
        return Err("Dedicated kernel CPU requires at least two CPUs; use -Smp 2 or greater");
    }
    crate::filesystem::shell::init()?;
    crate::info!("RustyPi5FS mounted read-only; access-based enumeration enabled.");
    install_user_mappings()?;
    unsafe { bsp::idt::install_user_syscall_gate(user_syscall_entry as usize); }
    PREPARED.store(true, Ordering::Release);
    Ok(())
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn launch_on(id: crate::cpu::smp::CpuId) -> Result<(), &'static str> {
    if bsp::cpu::current_core_id().raw() != 0 || !PREPARED.load(Ordering::Acquire) {
        return Err("Shell launch requires prepared boot CPU");
    }
    let index = id.raw() as usize;
    let cpu = bsp::cpu::cores().get(index).ok_or("Invalid shell CPU")?;
    if !crate::cpu::topology::user_eligible(index, cpu.is_online()) {
        return Err("Shell CPU must be an online secondary");
    }
    SHELL_CPU.compare_exchange(usize::MAX, index, Ordering::Release, Ordering::Relaxed)
        .map_err(|_| "Shell already assigned")?;
    for _ in 0..10_000_000 {
        if SHELL_STARTED.load(Ordering::Acquire) { return Ok(()); }
        core::hint::spin_loop();
    }
    Err("Shell CPU did not acknowledge launch")
}

#[cfg(feature = "bsp_qemu_x86")]
pub fn secondary_idle() -> ! {
    let id = bsp::cpu::current_core_id().raw() as usize;
    assert!(id != 0);
    loop {
        if SHELL_CPU.load(Ordering::Acquire) == id { start(); }
        core::hint::spin_loop();
    }
}

pub fn start() -> ! {
    #[cfg(feature = "bsp_qemu_x86")]
    {
        bsp::cpu::assert_user_cpu();
        assert!(PREPARED.load(Ordering::Acquire), "User image not prepared");
        unsafe { activate_user_mappings(); }
    }
    #[cfg(not(feature = "bsp_qemu_x86"))]
    {
        install_user_mappings().expect("Cannot enter user mode");
        unsafe { bsp::idt::install_user_syscall_gate(user_syscall_entry as usize); }
    }

    let (entry, stack) = unsafe {
        (
            user_shell_entry as usize,
            core::ptr::addr_of!(__user_stack_end) as usize,
        )
    };
    if entry < USER_BASE || entry >= USER_BASE + USER_REGION_SIZE
        || stack <= USER_BASE || stack > USER_BASE + USER_REGION_SIZE
    {
        panic!("User shell linker placement is outside its dedicated region");
    }

    crate::info!("Entering User A shell at ring 3 on CPU {}; kernel pages are supervisor-only.", crate::cpu::smp::core_id().raw());
    #[cfg(feature = "bsp_qemu_x86")]
    {
        SHELL_STARTED.store(true, Ordering::Release);
        crate::cpu::kernel_worker::verify_cross_core()
            .expect("Kernel worker cross-core verification failed");
    }
    unsafe {
        asm!(
            "push {data}",
            "push {stack}",
            "pushfq",
            "and qword ptr [rsp], -513",
            "push {code}",
            "push {entry}",
            "iretq",
            data = const crate::bsp::gdt::SEL_USER_DATA as u64,
            stack = in(reg) stack,
            code = const crate::bsp::gdt::SEL_USER_CODE64 as u64,
            entry = in(reg) entry,
            options(noreturn),
        );
    }
}

fn install_user_mappings() -> Result<(), &'static str> {
    let max_extended = unsafe { core::arch::x86_64::__cpuid(0x8000_0000).eax };
    if max_extended < 0x8000_0001
        || unsafe { core::arch::x86_64::__cpuid(0x8000_0001).edx } & (1 << 20) == 0
    {
        return Err("CPU does not support NX page protection");
    }

    unsafe {
        let pml4 = core::ptr::addr_of_mut!(PML4.0) as *mut u64;
        let pdpt = core::ptr::addr_of_mut!(PDPT.0) as *mut u64;
        let directories = core::ptr::addr_of_mut!(PAGE_DIRECTORIES) as *mut PageTable;
        let user_pt = core::ptr::addr_of_mut!(USER_PAGE_TABLE.0) as *mut u64;

        pml4.write((pdpt as u64) | PRESENT | WRITABLE | USER);
        for i in 0..4 {
            let directory = directories.add(i);
            pdpt.add(i).write((directory as u64) | PRESENT | WRITABLE | USER);
            let entries = directory as *mut u64;
            for j in 0..512 {
                let physical = (i * 1024 * 1024 * 1024 + j * 2 * 1024 * 1024) as u64;
                let cache_flags = if physical == 0xFEE0_0000 { (1 << 3) | (1 << 4) } else { 0 };
                entries.add(j).write(physical | PRESENT | WRITABLE | HUGE | cache_flags);
            }
        }

        let user_directory = directories as *mut u64;
        user_directory
            .add(USER_BASE / (2 * 1024 * 1024))
            .write((user_pt as u64) | PRESENT | WRITABLE | USER);

        map_user_range(user_pt, symbol_addr(core::ptr::addr_of!(__user_text_start)), symbol_addr(core::ptr::addr_of!(__user_text_end)), PRESENT | USER)?;
        map_user_range(user_pt, symbol_addr(core::ptr::addr_of!(__user_rodata_start)), symbol_addr(core::ptr::addr_of!(__user_rodata_end)), PRESENT | USER | NO_EXECUTE)?;
        map_user_range(user_pt, symbol_addr(core::ptr::addr_of!(__user_data_start)), symbol_addr(core::ptr::addr_of!(__user_data_end)), PRESENT | WRITABLE | USER | NO_EXECUTE)?;
        map_user_range(user_pt, symbol_addr(core::ptr::addr_of!(__user_stack_start)), symbol_addr(core::ptr::addr_of!(__user_stack_end)), PRESENT | WRITABLE | USER | NO_EXECUTE)?;

        activate_user_mappings();
    }

    Ok(())
}

/// Install one validated QEMU PCI aperture before any secondary can enter user mode.
#[cfg(feature = "bsp_qemu_x86")]
pub fn map_device_mmio(base: usize, length: usize) -> Result<(), &'static str> {
    if bsp::cpu::current_core_id().raw() != 0
        || !crate::exception::asynchronous::is_local_irq_masked()
        || !PREPARED.load(Ordering::Acquire)
        || SHELL_CPU.load(Ordering::Acquire) != usize::MAX
    { return Err("Device mapping requires prepared CPU 0 before user launch"); }
    let end = base.checked_add(length).ok_or("Device MMIO range overflow")?;
    // Fixed QEMU PC has 512 MiB RAM; OVMF assigns PCI BARs starting at 2 GiB.
    if length != 0x20000 || base % length != 0 || base < 0x8000_0000
        || end > 0xFEC0_0000 || super::video::overlaps_framebuffer(base, end)
    { return Err("Device MMIO overlaps RAM, framebuffer or reserved platform memory"); }
    if DEVICE_MAPPED.swap(true, Ordering::AcqRel) {
        return Err("Device MMIO mapping already reserved");
    }
    let huge_base = base & !0x1F_FFFF;
    unsafe {
        let table = core::ptr::addr_of_mut!(DEVICE_PAGE_TABLE.0) as *mut u64;
        for index in 0..512 {
            let address = huge_base + index * PAGE_SIZE;
            let cache = if address >= base && address < end { (1 << 3) | (1 << 4) } else { 0 };
            table.add(index).write(address as u64 | PRESENT | WRITABLE | NO_EXECUTE | cache);
        }
        let directories = core::ptr::addr_of_mut!(PAGE_DIRECTORIES) as *mut u64;
        directories.add(huge_base / 0x20_0000)
            .write(table as u64 | PRESENT | WRITABLE);
        // Flush this CPU's previous huge translation. APs load CR3 before user execution.
        asm!("mov cr3, {}", in(reg) core::ptr::addr_of!(PML4) as u64, options(nostack, preserves_flags));
    }
    crate::info!("Network MMIO: {:#x}..{:#x} supervisor-only uncached NX", base, end);
    Ok(())
}

unsafe fn activate_user_mappings() {
    let mut efer_low: u32;
    let mut efer_high: u32;
    asm!("rdmsr", in("ecx") 0xC000_0080u32, out("eax") efer_low, out("edx") efer_high, options(nostack, preserves_flags));
    efer_low |= 1 << 11;
    asm!("wrmsr", in("ecx") 0xC000_0080u32, in("eax") efer_low, in("edx") efer_high, options(nostack, preserves_flags));
    let mut cr0: u64;
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
    cr0 |= 1 << 16;
    asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
    asm!("mov cr3, {}", in(reg) core::ptr::addr_of!(PML4) as u64, options(nostack, preserves_flags));
}

unsafe fn map_user_range(table: *mut u64, start: usize, end: usize, flags: u64) -> Result<(), &'static str> {
    if start > end || start < USER_BASE || end > USER_BASE + USER_REGION_SIZE
        || start % PAGE_SIZE != 0 || end % PAGE_SIZE != 0
    {
        return Err("User section is unaligned or outside its reserved virtual region");
    }
    for address in (start..end).step_by(PAGE_SIZE) {
        let index = (address - USER_BASE) / PAGE_SIZE;
        table.add(index).write((address as u64) | flags);
    }
    Ok(())
}

unsafe fn symbol_addr(symbol: *const u8) -> usize {
    symbol as usize
}

#[no_mangle]
unsafe extern "C" fn syscall_dispatch(context: *mut u64) {
    let syscall = context.add(14).read();
    let first = context.add(9).read() as usize;
    let second = context.add(10).read() as usize;
    let result = match syscall {
        SYS_WRITE => user_write(first, second),
        SYS_READ => user_read(first, second),
        #[cfg(feature = "bsp_qemu_x86")]
        SYS_SCRIPT_LOAD => user_load_script(first, second),
        #[cfg(feature = "bsp_qemu_x86")]
        SYS_NET_STATUS => crate::network::user::available() as usize,
        #[cfg(feature = "bsp_qemu_x86")]
        SYS_NET_SEND => user_network_send(first, second),
        #[cfg(feature = "bsp_qemu_x86")]
        SYS_NET_RESULT => crate::network::user::result(),
        #[cfg(feature = "bsp_qemu_x86")]
        SYS_LS..=SYS_CAT => {
            if second > 127 || !user_range_readable(first, second) {
                crate::warn!("Rejected invalid filesystem syscall path buffer");
                usize::MAX
            } else {
                let mut path = [0u8; 127];
                core::ptr::copy_nonoverlapping(first as *const u8, path.as_mut_ptr(), second);
                crate::filesystem::shell::execute(syscall, &path[..second])
            }
        }
        _ => usize::MAX,
    };
    context.add(14).write(result as u64);
}

#[cfg(feature = "bsp_qemu_x86")]
fn user_network_send(address: usize, length: usize) -> usize {
    if length == 0 || length > 127 || !user_range_readable(address, length) {
        crate::warn!("Rejected invalid network syscall buffer");
        return crate::network::user::INVALID;
    }
    let mut command = [0u8; 127];
    unsafe { core::ptr::copy_nonoverlapping(address as *const u8, command.as_mut_ptr(), length); }
    crate::network::user::submit(&command[..length])
}

#[cfg(feature = "bsp_qemu_x86")]
fn user_load_script(request: usize, length: usize) -> usize {
    if length != 4 * core::mem::size_of::<usize>() || !user_range_readable(request, length) {
        crate::warn!("Rejected invalid script load request");
        return usize::MAX;
    }
    let fields = request as *const usize;
    let (path_address, path_length, output_address, capacity) = unsafe {
        (fields.read_unaligned(), fields.add(1).read_unaligned(), fields.add(2).read_unaligned(), fields.add(3).read_unaligned())
    };
    if path_length == 0 || path_length > 127 || capacity != 4096
        || !user_range_readable(path_address, path_length) || !user_range_writable(output_address, capacity)
    {
        crate::warn!("Rejected invalid script path or output buffer");
        return usize::MAX;
    }
    let mut path = [0u8; 127];
    let mut data = [0u8; 4096];
    unsafe { core::ptr::copy_nonoverlapping(path_address as *const u8, path.as_mut_ptr(), path_length); }
    match crate::filesystem::shell::read_file(&path[..path_length], &mut data) {
        Ok(size) => {
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), output_address as *mut u8, size); }
            size
        }
        Err("Path unavailable") => usize::MAX - 1,
        Err(message) => {
            crate::warn!("Script load failed: {}", message);
            usize::MAX
        }
    }
}

fn user_write(address: usize, length: usize) -> usize {
    if length > 1024 || !user_range_readable(address, length) {
        return usize::MAX;
    }
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    for byte in bytes {
        bsp::console::UART.write_char(*byte as char);
    }
    length
}

fn user_read(address: usize, length: usize) -> usize {
    if length == 0 || length > 1024 || !user_range_writable(address, length) {
        return usize::MAX;
    }

    let uart = &bsp::console::UART;
    let mut used = 0usize;

    loop {
        let byte = loop {
            if let Some(byte) = uart.try_read_byte() {
                break byte;
            }
            core::hint::spin_loop();
        };

        match byte {
            b'\r' | b'\n' => {
                unsafe { (address as *mut u8).add(used).write(b'\n') };
                uart.write_char('\r');
                uart.write_char('\n');
                return used + 1;
            }
            b'\x08' | b'\x7F' => {
                if used != 0 {
                    used -= 1;
                    uart.write_char('\x08');
                    uart.write_char(' ');
                    uart.write_char('\x08');
                }
            }
            b' '..=b'~' if used < length - 1 => {
                unsafe { (address as *mut u8).add(used).write(byte) };
                used += 1;
                uart.write_char(byte as char);
            }
            _ => {}
        }
    }
}

fn user_range_readable(address: usize, length: usize) -> bool {
    user_range_in(address, length, unsafe { symbol_addr(core::ptr::addr_of!(__user_rodata_start)) }, unsafe { symbol_addr(core::ptr::addr_of!(__user_rodata_end)) })
        || user_range_in(address, length, unsafe { symbol_addr(core::ptr::addr_of!(__user_data_start)) }, unsafe { symbol_addr(core::ptr::addr_of!(__user_data_end)) })
        || user_range_in(address, length, unsafe { symbol_addr(core::ptr::addr_of!(__user_stack_start)) }, unsafe { symbol_addr(core::ptr::addr_of!(__user_stack_end)) })
}

fn user_range_writable(address: usize, length: usize) -> bool {
    user_range_in(address, length, unsafe { symbol_addr(core::ptr::addr_of!(__user_data_start)) }, unsafe { symbol_addr(core::ptr::addr_of!(__user_data_end)) })
        || user_range_in(address, length, unsafe { symbol_addr(core::ptr::addr_of!(__user_stack_start)) }, unsafe { symbol_addr(core::ptr::addr_of!(__user_stack_end)) })
}

fn user_range_in(address: usize, length: usize, start: usize, end: usize) -> bool {
    address >= start && address.checked_add(length).map_or(false, |limit| limit <= end)
}

#[inline(always)]
#[link_section = ".user_text"]
pub(super) unsafe fn user_syscall(number: u64, first: usize, second: usize) -> usize {
    let result: u64;
    asm!(
        "int 0x80",
        inlateout("rax") number => result,
        in("rdi") first,
        in("rsi") second,
        clobber_abi("C"),
        options(preserves_flags),
    );
    result as usize
}

#[inline(always)]
#[link_section = ".user_text"]
pub(super) unsafe fn user_print(bytes: *const u8, length: usize) {
    user_syscall(SYS_WRITE, bytes as usize, length);
}

#[link_section = ".user_text"]
#[no_mangle]
extern "C" fn user_shell_entry() -> ! {
    unsafe {
        #[cfg(feature = "bsp_qemu_x86")]
        super::script_shell::init();
        user_print(core::ptr::addr_of!(WELCOME) as *const u8, WELCOME.len());
        loop {
            user_print(core::ptr::addr_of!(PROMPT) as *const u8, PROMPT.len());
            let line = core::ptr::addr_of_mut!(LINE) as *mut u8;
            let length = user_syscall(SYS_READ, line as usize, 128);
            if length == 0 || length > 128 || length == usize::MAX {
                continue;
            }
            let len = if (line.add(length - 1)).read() == b'\n' { length - 1 } else { length };
            let mut word = 0;
            while word < len && line.add(word).read() != b' ' { word += 1; }
            #[cfg(feature = "bsp_qemu_x86")]
            {
                let mut start = word;
                while start < len && line.add(start).read() == b' ' { start += 1; }
                let mut end = len;
                while end > start && line.add(end - 1).read() == b' ' { end -= 1; }
                if word == 3 && line.read() == b'u' && line.add(1).read() == b'd' && line.add(2).read() == b'p' {
                    super::network_shell::udp(line.add(start), end - start);
                    continue;
                }
                if word == 3 && line.read() == b'n' && line.add(1).read() == b'e' && line.add(2).read() == b't'
                    && end - start == 6 && line.add(start).read() == b's' && line.add(start + 1).read() == b't'
                    && line.add(start + 2).read() == b'a' && line.add(start + 3).read() == b't'
                    && line.add(start + 4).read() == b'u' && line.add(start + 5).read() == b's' {
                    super::network_shell::status();
                    continue;
                }
                if word == 6 && len == 6 && line.read() == b's' && line.add(1).read() == b'c'
                    && line.add(2).read() == b'r' && line.add(3).read() == b'i'
                    && line.add(4).read() == b'p' && line.add(5).read() == b't' {
                    super::script_shell::interactive(line);
                    continue;
                }
                if word == 3 && line.read() == b'r' && line.add(1).read() == b'u' && line.add(2).read() == b'n' {
                    super::script_shell::run(line.add(start), end - start);
                    continue;
                }
                if word == 4 && line.read() == b'p' && line.add(1).read() == b'a'
                    && line.add(2).read() == b't' && line.add(3).read() == b'h' {
                    super::script_shell::path(line.add(start), end - start);
                    continue;
                }
            }
            let operation = if word == 2 && line.read() == b'l' && line.add(1).read() == b's' {
                SYS_LS
            } else if word == 2 && line.read() == b'c' && line.add(1).read() == b'd' {
                SYS_CD
            } else if word == 3 && line.read() == b'p' && line.add(1).read() == b'w' && line.add(2).read() == b'd' {
                SYS_PWD
            } else if word == 3 && line.read() == b'c' && line.add(1).read() == b'a' && line.add(2).read() == b't' {
                SYS_CAT
            } else { 0 };
            if operation != 0 {
                let mut start = word;
                while start < len && line.add(start).read() == b' ' { start += 1; }
                let mut end = len;
                while end > start && line.add(end - 1).read() == b' ' { end -= 1; }
                user_syscall(operation, line.add(start) as usize, end - start);
                continue;
            }
            match len {
                4 if line.read() == b'h' && line.add(1).read() == b'e' && line.add(2).read() == b'l' && line.add(3).read() == b'p' => {
                    user_print(core::ptr::addr_of!(HELP) as *const u8, HELP.len());
                    #[cfg(feature = "bsp_qemu_x86")]
                    {
                        user_print(core::ptr::addr_of!(SCRIPT_HELP) as *const u8, SCRIPT_HELP.len());
                        super::network_shell::help();
                    }
                }
                2 if line.read() == b'i' && line.add(1).read() == b'd' => {
                    user_print(core::ptr::addr_of!(ID) as *const u8, ID.len());
                }
                5 if line.read() == b'c' && line.add(1).read() == b'l' && line.add(2).read() == b'e' && line.add(3).read() == b'a' && line.add(4).read() == b'r' => {
                    user_print(core::ptr::addr_of!(CLEAR) as *const u8, CLEAR.len());
                }
                4 if line.read() == b'e' && line.add(1).read() == b'x' && line.add(2).read() == b'i' && line.add(3).read() == b't' => {
                    user_print(core::ptr::addr_of!(GOODBYE) as *const u8, GOODBYE.len());
                    loop { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
                }
                4 if line.read() == b'e' && line.add(1).read() == b'c' && line.add(2).read() == b'h' && line.add(3).read() == b'o' => {
                    user_print(core::ptr::addr_of!(LINE_END) as *const u8, LINE_END.len());
                }
                _ if len >= 5 && line.read() == b'e' && line.add(1).read() == b'c' && line.add(2).read() == b'h' && line.add(3).read() == b'o' && line.add(4).read() == b' ' => {
                    user_print(line.add(5), len - 5);
                    user_print(core::ptr::addr_of!(LINE_END) as *const u8, LINE_END.len());
                }
                0 => {}
                _ => user_print(core::ptr::addr_of!(UNKNOWN) as *const u8, UNKNOWN.len()),
            }
        }
    }
}

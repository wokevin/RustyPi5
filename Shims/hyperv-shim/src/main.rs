#![no_std]
#![no_main]
#![feature(abi_efiapi)]

use core::arch::asm;
use core::ffi::c_void;
use core::mem::size_of;
use core::panic::PanicInfo;
use core::ptr;

/// EFI_SERIAL_IO_PROTOCOL GUID: `{BB25CF6F-F1D4-11D2-9A0C-0090273FC14D}`.
///
/// Gen 2 Hyper-V VMs do not route legacy `out dx, al` / `in al, dx` I/O-port instructions to the
/// virtual COM port the same way a real 16550 UART would be wired up on a Gen 1 (BIOS) VM; the
/// port is only reachable through this UEFI protocol during boot services. Using raw port I/O
/// here silently produces zero bytes on Gen 2, which is why earlier shim-side tracing appeared to
/// "work" (no crash) but the host-side named pipe reader never received anything.
const SERIAL_IO_PROTOCOL_GUID: Guid = Guid {
    data1: 0xbb25_cf6f,
    data2: 0xf1d4,
    data3: 0x11d2,
    data4: [0x9a, 0x0c, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d],
};

/// Minimal `EFI_SERIAL_IO_PROTOCOL` prefix, just enough to reach `Write`.
#[repr(C)]
struct SerialIoProtocol {
    _revision: u32,
    _reset: usize,
    _set_attributes: usize,
    _set_control_bits: usize,
    _get_control_bits: usize,
    write: unsafe extern "efiapi" fn(
        this: *mut SerialIoProtocol,
        buffer_size: *mut usize,
        buffer: *const c_void,
    ) -> usize,
    // remaining fields (Read, etc.) unused.
}

/// Cached handle to the serial protocol, set once by `shim_init_serial` early in `efi_main` so
/// later `shim_print` calls (including ones before the kernel handoff) don't need to re-locate it.
static mut SERIAL_IO: *mut SerialIoProtocol = ptr::null_mut();

/// Minimal `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`, just enough to reach `OutputString`. Unlike
/// `EFI_SERIAL_IO_PROTOCOL` (which may not be installed at all on Gen 2 Hyper-V's firmware during
/// boot services, causing `shim_print` to silently no-op), `ConOut` is guaranteed present by the
/// UEFI spec, so this gives us one channel we know must work if `efi_main` is actually reached.
#[repr(C)]
struct SimpleTextOutputProtocol {
    _reset: usize,
    output_string:
        unsafe extern "efiapi" fn(this: *mut SimpleTextOutputProtocol, string: *const u16) -> usize,
    // remaining fields unused.
}

/// Cached `ConOut` handle, set once by `shim_init_serial` alongside `SERIAL_IO`.
static mut CON_OUT: *mut SimpleTextOutputProtocol = ptr::null_mut();

/// Writes `msg` to both the serial protocol (if located) and `ConOut` (always attempted), so a
/// missing/failed serial protocol no longer means total silence. `ConOut` requires a
/// null-terminated UCS-2 string, so this converts ASCII `msg` on a small fixed-size stack buffer.
fn shim_print(msg: &str) {
    unsafe {
        if !SERIAL_IO.is_null() {
            let serial = &*SERIAL_IO;
            let mut write_bytes = |bytes: &[u8]| {
                let mut size = bytes.len();
                (serial.write)(SERIAL_IO, &mut size, bytes.as_ptr() as *const c_void);
            };
            write_bytes(msg.as_bytes());
            write_bytes(b"\r\n");
        }

        if !CON_OUT.is_null() {
            let mut buf = [0u16; 128];
            let mut i = 0;
            for byte in msg.bytes().take(buf.len() - 3) {
                buf[i] = byte as u16;
                i += 1;
            }
            buf[i] = b'\r' as u16;
            buf[i + 1] = b'\n' as u16;
            buf[i + 2] = 0;
            ((&*CON_OUT).output_string)(CON_OUT, buf.as_ptr());
        }
    }
}

/// Converts a byte slice to hex digits into `out`, returning the number of hex characters
/// written (always `bytes_written * 2`, no leading `0x`). Used by `shim_print_hex` /
/// `shim_print_label_hex` / `shim_print_symbolicated` to build a one-line
/// `<label><hex><suffix>` message without needing a heap/`core::fmt` formatter in this `no_std`
/// shim.
fn write_hex(mut value: u64, out: &mut [u8]) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut tmp = [0u8; 16];
    let mut n = 0;
    if value == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while value != 0 {
            tmp[n] = DIGITS[(value & 0xF) as usize];
            value >>= 4;
            n += 1;
        }
    }
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}

/// Prints `"{label}{value:#x}{suffix}"` in one line via `shim_print`.
fn shim_print_hex(label: &str, value: u64, suffix: &str) {
    let mut buf = [0u8; 256];
    let mut pos = 0;
    for b in label.bytes() {
        buf[pos] = b;
        pos += 1;
    }
    buf[pos] = b'0';
    buf[pos + 1] = b'x';
    pos += 2;
    let mut hex_buf = [0u8; 16];
    let n = write_hex(value, &mut hex_buf);
    buf[pos..pos + n].copy_from_slice(&hex_buf[..n]);
    pos += n;
    for b in suffix.bytes() {
        buf[pos] = b;
        pos += 1;
    }
    // SAFETY: buf only ever contains ASCII bytes written above.
    let s = unsafe { core::str::from_utf8_unchecked(&buf[..pos]) };
    shim_print(s);
}

/// Alias kept for call sites that label a raw (non-symbol) address; identical to `shim_print_hex`.
fn shim_print_label_hex(label: &str, value: u64, suffix: &str) {
    shim_print_hex(label, value, suffix);
}

/// Alias kept for call sites symbolicating a function/entry address; identical to
/// `shim_print_hex`, since this freestanding shim has no symbol table to resolve names from.
fn shim_print_symbolicated(label: &str, value: usize) {
    shim_print_hex(label, value as u64, "");
}

/// Locates `EFI_SERIAL_IO_PROTOCOL` (best-effort) and caches `ConOut` from the system table
/// (always present). Must be called once, early in `efi_main`, before any `shim_print` call.
///
/// # Safety
///
/// - `system_table` must point to a valid, currently-active `EFI_SYSTEM_TABLE`.
unsafe fn shim_init_serial(system_table: *const SystemTable) {
    let system_table_ref = &*system_table;
    CON_OUT = system_table_ref.con_out as *mut SimpleTextOutputProtocol;

    let bs = &*system_table_ref.boot_services;
    let mut interface: *mut c_void = ptr::null_mut();
    let status = (bs.locate_protocol)(&SERIAL_IO_PROTOCOL_GUID, ptr::null_mut(), &mut interface);
    if status == EFI_SUCCESS && !interface.is_null() {
        SERIAL_IO = interface as *mut SerialIoProtocol;
    }
}

/// ACPI 2.0 configuration table GUID: `{8868E871-E4F1-11D3-BC22-0080C73C8881}`.
const ACPI_20_TABLE_GUID: Guid = Guid {
    data1: 0x8868_e871,
    data2: 0xe4f1,
    data3: 0x11d3,
    data4: [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
};

/// EFI_SIMPLE_FILE_SYSTEM_PROTOCOL GUID: `{964E5B22-6459-11D2-8E39-00A0C969723B}`.
const SIMPLE_FILE_SYSTEM_PROTOCOL_GUID: Guid = Guid {
    data1: 0x964e_5b22,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// EFI_LOADED_IMAGE_PROTOCOL GUID: `{5B1B31A1-9562-11D2-8E3F-00A0C969723B}`.
const LOADED_IMAGE_PROTOCOL_GUID: Guid = Guid {
    data1: 0x5b1b_31a1,
    data2: 0x9562,
    data3: 0x11d2,
    data4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// EFI_GRAPHICS_OUTPUT_PROTOCOL GUID: `{9042A9DE-23DC-4A38-96FB-7ADED080516A}`.
///
/// Queried via `locate_protocol` (not `handle_protocol`): GOP is a boot-services-only protocol
/// typically installed on the firmware's own console output handle, not on the loaded image or
/// file system handles this shim already has, so a global search is the correct way to find it.
const GRAPHICS_OUTPUT_PROTOCOL_GUID: Guid = Guid {
    data1: 0x9042_a9de,
    data2: 0x23dc,
    data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};

const EFI_SUCCESS: usize = 0;
const EFI_FILE_MODE_READ: u64 = 0x0000_0000_0000_0001;
const EFI_ALLOCATE_ANY_PAGES: u32 = 0;
const EFI_ALLOCATE_ADDRESS: u32 = 2;
const EFI_LOADER_DATA: u32 = 2;

/// Kernel binary path relative to the ESP root, mirroring `BOOTX64.EFI`'s own `\EFI\BOOT` layout.
const KERNEL_PATH_UTF16: &[u16] = &[
    '\\' as u16, 'E' as u16, 'F' as u16, 'I' as u16, '\\' as u16, 'B' as u16, 'O' as u16,
    'O' as u16, 'T' as u16, '\\' as u16, 'K' as u16, 'E' as u16, 'R' as u16, 'N' as u16,
    'E' as u16, 'L' as u16, '.' as u16, 'E' as u16, 'L' as u16, 'F' as u16, 0,
];

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl PartialEq for Guid {
    fn eq(&self, other: &Self) -> bool {
        self.data1 == other.data1
            && self.data2 == other.data2
            && self.data3 == other.data3
            && self.data4 == other.data4
    }
}

#[repr(C)]
struct ConfigurationTable {
    vendor_guid: Guid,
    vendor_table: *const c_void,
}

/// Minimal prefix of `EFI_TABLE_HEADER`.
#[repr(C)]
struct TableHeader {
    _signature: u64,
    _revision: u32,
    _header_size: u32,
    _crc32: u32,
    _reserved: u32,
}

/// Minimal `EFI_SYSTEM_TABLE` prefix, just enough to reach `ConOut`, `BootServices`, and the
/// configuration table (needed to find the ACPI RSDP).
#[repr(C)]
struct SystemTable {
    _header: TableHeader,
    _firmware_vendor: usize,
    _firmware_revision: u32,
    _console_in_handle: usize,
    _con_in: usize,
    _console_out_handle: usize,
    con_out: *mut SimpleTextOutputProtocol,
    _standard_error_handle: usize,
    _std_err: usize,
    _runtime_services: usize,
    boot_services: *mut BootServices,
    number_of_table_entries: usize,
    configuration_table: *mut ConfigurationTable,
}

/// Minimal `EFI_BOOT_SERVICES` prefix, just enough to reach the handful of services we need.
///
/// # Safety
///
/// Field offsets must exactly match the UEFI spec's `EFI_BOOT_SERVICES` table layout; any drift
/// will call through the wrong function pointer.
#[repr(C)]
struct BootServices {
    _header: TableHeader,
    _raise_tpl: usize,
    _restore_tpl: usize,
    allocate_pages: unsafe extern "efiapi" fn(
        alloc_type: u32,
        memory_type: u32,
        pages: usize,
        memory: *mut u64,
    ) -> usize,
    _free_pages: usize,
    get_memory_map: unsafe extern "efiapi" fn(
        memory_map_size: *mut usize,
        memory_map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        descriptor_version: *mut u32,
    ) -> usize,
    allocate_pool: unsafe extern "efiapi" fn(
        pool_type: u32,
        size: usize,
        buffer: *mut *mut c_void,
    ) -> usize,
    _free_pool: usize,
    _create_event: usize,
    _set_timer: usize,
    _wait_for_event: usize,
    _signal_event: usize,
    _close_event: usize,
    _check_event: usize,
    _install_protocol_interface: usize,
    _reinstall_protocol_interface: usize,
    _uninstall_protocol_interface: usize,
    handle_protocol: unsafe extern "efiapi" fn(
        handle: *mut c_void,
        protocol: *const Guid,
        interface: *mut *mut c_void,
    ) -> usize,
    _reserved: usize,
    _register_protocol_notify: usize,
    _locate_handle: usize,
    _locate_device_path: usize,
    _install_configuration_table: usize,
    _load_image: usize,
    _start_image: usize,
    _exit: usize,
    _unload_image: usize,
    exit_boot_services:
        unsafe extern "efiapi" fn(image_handle: *mut c_void, map_key: usize) -> usize,
    _get_next_monotonic_count: usize,
    stall: unsafe extern "efiapi" fn(microseconds: usize) -> usize,
    _set_watchdog_timer: usize,
    _connect_controller: usize,
    _disconnect_controller: usize,
    _open_protocol: usize,
    _close_protocol: usize,
    _open_protocol_information: usize,
    _protocols_per_handle: usize,
    _locate_handle_buffer: usize,
    locate_protocol: unsafe extern "efiapi" fn(
        protocol: *const Guid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> usize,
}

/// Minimal `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`.
#[repr(C)]
struct SimpleFileSystemProtocol {
    _revision: u64,
    open_volume: unsafe extern "efiapi" fn(
        this: *mut SimpleFileSystemProtocol,
        root: *mut *mut FileProtocol,
    ) -> usize,
}

/// Minimal `EFI_FILE_PROTOCOL`, just enough for `Open`, `Read`, `SetPosition`, `GetInfo`.
#[repr(C)]
struct FileProtocol {
    _revision: u64,
    open: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
    ) -> usize,
    _close: usize,
    _delete: usize,
    read: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> usize,
    _write: usize,
    _get_position: usize,
    set_position: unsafe extern "efiapi" fn(this: *mut FileProtocol, position: u64) -> usize,
    get_info: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        information_type: *const Guid,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> usize,
}

/// EFI_FILE_INFO GUID: `{09576E92-6D3F-11D2-8E39-00A0C969723B}`.
const FILE_INFO_GUID: Guid = Guid {
    data1: 0x0957_6e92,
    data2: 0x6d3f,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// Minimal `EFI_FILE_INFO` prefix, just enough to reach `file_size`.
#[repr(C)]
struct FileInfo {
    _size: u64,
    file_size: u64,
    _physical_size: u64,
    _create_time: [u8; 16],
    _last_access_time: [u8; 16],
    _modification_time: [u8; 16],
    _attribute: u64,
    // `file_name: [u16; N]` follows; unused here.
}

/// Minimal `EFI_LOADED_IMAGE_PROTOCOL` prefix, just enough to reach `device_handle`.
#[repr(C)]
struct LoadedImageProtocol {
    _revision: u32,
    _parent_handle: *mut c_void,
    _system_table: *mut c_void,
    device_handle: *mut c_void,
    // remaining fields unused.
}

/// `EFI_GRAPHICS_OUTPUT_MODE_INFORMATION`, per the UEFI spec. Only the fields needed to compute a
/// pixel's byte offset in the framebuffer are kept; `pixel_format`/`pixel_bitmask` are omitted
/// since this driver only supports the common 32-bit BGR/RGB pixel formats (format 0 or 1),
/// checked by the caller before trusting `pixels_per_scan_line`.
#[repr(C)]
struct GraphicsOutputModeInformation {
    _version: u32,
    horizontal_resolution: u32,
    vertical_resolution: u32,
    pixel_format: u32,
    _pixel_information: [u32; 4],
    pixels_per_scan_line: u32,
}

/// `EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE`, per the UEFI spec.
#[repr(C)]
struct GraphicsOutputProtocolMode {
    _max_mode: u32,
    _mode: u32,
    info: *const GraphicsOutputModeInformation,
    _size_of_info: usize,
    frame_buffer_base: u64,
    frame_buffer_size: usize,
}

/// Minimal `EFI_GRAPHICS_OUTPUT_PROTOCOL` prefix, just enough to reach `mode` (the other two
/// function-pointer fields, `QueryMode`/`SetMode`, are unused: the firmware's console-selected
/// mode is accepted as-is rather than negotiating a different one).
#[repr(C)]
struct GraphicsOutputProtocol {
    _query_mode: usize,
    _set_mode: usize,
    _blt: usize,
    mode: *const GraphicsOutputProtocolMode,
}

/// Framebuffer info handed off to the kernel, mirroring what the kernel-side driver needs to plot
/// pixels: a linear, firmware-mapped (identity-mapped at this stage, like everything else before
/// the kernel builds its own page tables) base address, its stride in pixels (which can exceed
/// `width` -- e.g. padded for alignment -- so must never be assumed equal to `width`), the
/// visible resolution, and the GOP-reported total framebuffer size in bytes. `frame_buffer_size`
/// is the authoritative bound on how much of `base` is actually mapped: `pixels_per_scan_line *
/// height` can exceed it (observed under some Hyper-V synthetic GOP implementations), and writing
/// past it faults with no IDT/GDT set up yet, which escalates straight to a triple fault/reset --
/// so any raw framebuffer write (including the diagnostic fills in `efi_main`) must clamp its
/// pixel count against this field, not just `pixels_per_scan_line * height`. `base` is `0` if no
/// GOP framebuffer could be found, which the kernel must check before using any other field.
#[repr(C)]
pub struct FramebufferInfo {
    pub base: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scan_line: u32,
    pub bgr: bool,
    pub frame_buffer_size: usize,
}

/// Locates `EFI_GRAPHICS_OUTPUT_PROTOCOL` (if present) and reads out its currently active mode's
/// framebuffer base/stride/resolution/size. Returns `None` (not a hard failure) if GOP isn't
/// available at all, or if its pixel format isn't one of the two common 32-bit-per-pixel formats
/// this minimal driver supports (`PixelRedGreenBlueReserved8BitPerColor` = 0,
/// `PixelBlueGreenRedReserved8BitPerColor` = 1) -- video output is a nice-to-have at this stage,
/// not something that should block booting if e.g. only a text-mode-only/non-GOP console is
/// present.
///
/// # Safety
///
/// - Must be called while boot services are still active (uses `locate_protocol`).
unsafe fn locate_gop_framebuffer(boot_services: *mut BootServices) -> Option<FramebufferInfo> {
    let bs = &*boot_services;

    let mut interface: *mut c_void = ptr::null_mut();
    let status =
        (bs.locate_protocol)(&GRAPHICS_OUTPUT_PROTOCOL_GUID, ptr::null_mut(), &mut interface);
    if status != EFI_SUCCESS || interface.is_null() {
        shim_print("[shim] GOP not found, skipping framebuffer handoff");
        return None;
    }

    let gop = &*(interface as *const GraphicsOutputProtocol);
    if gop.mode.is_null() {
        return None;
    }
    let mode = &*gop.mode;
    if mode.info.is_null() || mode.frame_buffer_base == 0 {
        return None;
    }
    let info = &*mode.info;

    const PIXEL_RGB: u32 = 0;
    const PIXEL_BGR: u32 = 1;
    let bgr = match info.pixel_format {
        PIXEL_RGB => false,
        PIXEL_BGR => true,
        _ => {
            shim_print("[shim] GOP pixel format unsupported (not 32bpp RGB/BGR), skipping");
            return None;
        }
    };

    shim_print_label_hex("[shim] GOP framebuffer@", mode.frame_buffer_base, "");
    shim_print_label_hex("[shim] GOP framebuffer_size=", mode.frame_buffer_size as u64, "");
    Some(FramebufferInfo {
        base: mode.frame_buffer_base,
        width: info.horizontal_resolution,
        height: info.vertical_resolution,
        pixels_per_scan_line: info.pixels_per_scan_line,
        bgr,
        frame_buffer_size: mode.frame_buffer_size,
    })
}

/// `EFI_MEMORY_DESCRIPTOR`, per the UEFI spec. `physical_end` is *not* a real UEFI field (the spec
/// only gives `number_of_pages`); it's computed on read-out below so call sites can keep using a
/// simple `end - start` without re-deriving it everywhere.
#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryDescriptor {
    typ: u32,
    _pad: u32,
    physical_start: u64,
    _virtual_start: u64,
    number_of_pages: u64,
    _attribute: u64,
}

impl MemoryDescriptor {
    fn physical_end(&self) -> u64 {
        self.physical_start + self.number_of_pages * 4096
    }
}

/// Finds the ACPI 2.0 RSDP in the UEFI configuration table. Must be called while boot services
/// are still active (walking `configuration_table` afterward is unsupported by the spec).
///
/// # Safety
///
/// - `system_table` must point to a valid, currently-active `EFI_SYSTEM_TABLE`.
unsafe fn find_acpi_rsdp(system_table: *const SystemTable) -> Option<usize> {
    let system_table_ref = &*system_table;
    let entries = core::slice::from_raw_parts(
        system_table_ref.configuration_table,
        system_table_ref.number_of_table_entries,
    );
    for entry in entries {
        if entry.vendor_guid == ACPI_20_TABLE_GUID {
            return Some(entry.vendor_table as usize);
        }
    }
    None
}

/// Loads `\EFI\BOOT\KERNEL.ELF` from the same volume this shim itself was loaded from, into a
/// freshly `allocate_pool`'d buffer. Returns `(data_ptr, size)` on success.
///
/// # Safety
///
/// - `image_handle` must be this shim's own image handle, as passed in to `efi_main`.
/// - `boot_services` must point to a valid, currently-active `EFI_BOOT_SERVICES`.
unsafe fn load_kernel_elf(
    image_handle: *mut c_void,
    boot_services: *mut BootServices,
) -> Option<(*mut u8, usize)> {
    let bs = &*boot_services;

    let mut loaded_image_interface: *mut c_void = ptr::null_mut();
    let status = (bs.handle_protocol)(
        image_handle,
        &LOADED_IMAGE_PROTOCOL_GUID,
        &mut loaded_image_interface,
    );
    if status != EFI_SUCCESS || loaded_image_interface.is_null() {
        shim_print("[shim] handle_protocol(LoadedImage) FAILED");
        return None;
    }
    let loaded_image = &*(loaded_image_interface as *const LoadedImageProtocol);

    let mut fs_interface: *mut c_void = ptr::null_mut();
    let status = (bs.handle_protocol)(
        loaded_image.device_handle,
        &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
        &mut fs_interface,
    );
    if status != EFI_SUCCESS || fs_interface.is_null() {
        shim_print("[shim] handle_protocol(SimpleFileSystem) FAILED");
        return None;
    }
    let fs = &*(fs_interface as *const SimpleFileSystemProtocol);

    let mut root: *mut FileProtocol = ptr::null_mut();
    let status = (fs.open_volume)(fs_interface as *mut SimpleFileSystemProtocol, &mut root);
    if status != EFI_SUCCESS || root.is_null() {
        shim_print("[shim] open_volume FAILED");
        return None;
    }

    let mut kernel_file: *mut FileProtocol = ptr::null_mut();
    let status = ((&*root).open)(
        root,
        &mut kernel_file,
        KERNEL_PATH_UTF16.as_ptr(),
        EFI_FILE_MODE_READ,
        0,
    );
    if status != EFI_SUCCESS || kernel_file.is_null() {
        shim_print("[shim] open(KERNEL.ELF) FAILED");
        return None;
    }
    let kernel_file_ref = &*kernel_file;

    // Query file size via EFI_FILE_INFO.
    let mut info_buffer_size: usize = size_of::<FileInfo>() + 64;
    let mut info_buffer: *mut c_void = ptr::null_mut();
    (bs.allocate_pool)(EFI_LOADER_DATA, info_buffer_size, &mut info_buffer);
    let status = (kernel_file_ref.get_info)(
        kernel_file,
        &FILE_INFO_GUID,
        &mut info_buffer_size,
        info_buffer,
    );
    if status != EFI_SUCCESS {
        shim_print("[shim] get_info(KERNEL.ELF) FAILED");
        return None;
    }
    let file_size = (&*(info_buffer as *const FileInfo)).file_size as usize;

    let mut data_buffer: *mut c_void = ptr::null_mut();
    let status = (bs.allocate_pool)(EFI_LOADER_DATA, file_size, &mut data_buffer);
    if status != EFI_SUCCESS || data_buffer.is_null() {
        shim_print("[shim] allocate_pool(kernel image) FAILED");
        return None;
    }

    let mut read_size = file_size;
    let status = (kernel_file_ref.read)(kernel_file, &mut read_size, data_buffer);
    if status != EFI_SUCCESS || read_size != file_size {
        shim_print("[shim] read(KERNEL.ELF) FAILED");
        return None;
    }

    Some((data_buffer as *mut u8, file_size))
}

/// Minimal ELF64 header prefix, just enough to find the program header table and entry point.
#[repr(C)]
struct Elf64Header {
    e_ident: [u8; 16],
    _e_type: u16,
    _e_machine: u16,
    _e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    _e_shoff: u64,
    _e_flags: u32,
    _e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    // remaining fields unused.
}

/// `Elf64_Phdr`, just the fields needed to load `PT_LOAD` segments.
#[repr(C)]
struct Elf64ProgramHeader {
    p_type: u32,
    _p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    _p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    _p_align: u64,
}

const PT_LOAD: u32 = 1;
const EFI_PAGE_SIZE: usize = 4096;

/// A merged, page-aligned run of one or more overlapping/adjacent `PT_LOAD` segments, allocated
/// and copied as a single unit so segments that share a page (common when segments are packed
/// tightly by the linker) don't each try to `AllocateAddress` the same page out from under each
/// other.
struct Cluster {
    page_start: u64,
    page_count: u64,
}

/// Parses the ELF64 program header table in `elf_data`, merges `PT_LOAD` segments into
/// page-aligned clusters, allocates each cluster at its fixed physical address (identity-mapped,
/// matching the linker script), copies each segment's file contents into place (zero-filling the
/// `p_memsz - p_filesz` BSS tail), and returns the ELF entry point.
///
/// # Safety
///
/// - `elf_data` must point to `elf_size` valid bytes containing a well-formed, statically-linked
///   (non-PIE) ELF64 executable, as produced by the kernel's own linker script.
/// - `boot_services` must point to a valid, currently-active `EFI_BOOT_SERVICES`.
unsafe fn load_elf_segments(
    boot_services: *mut BootServices,
    elf_data: *mut u8,
    elf_size: usize,
) -> Option<usize> {
    let bs = &*boot_services;

    if elf_size < size_of::<Elf64Header>() {
        shim_print("[shim] ELF too small");
        return None;
    }
    let header = &*(elf_data as *const Elf64Header);
    if &header.e_ident[0..4] != b"\x7FELF" {
        shim_print("[shim] bad ELF magic");
        return None;
    }

    const MAX_SEGMENTS: usize = 16;
    let mut ranges: [(u64, u64); MAX_SEGMENTS] = [(0, 0); MAX_SEGMENTS];
    let mut range_count = 0;

    for i in 0..header.e_phnum as usize {
        let ph_addr = elf_data as usize + header.e_phoff as usize + i * header.e_phentsize as usize;
        let ph = &*(ph_addr as *const Elf64ProgramHeader);
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        if range_count >= MAX_SEGMENTS {
            shim_print("[shim] too many PT_LOAD segments, halting");
            return None;
        }
        let page_start = ph.p_vaddr & !(EFI_PAGE_SIZE as u64 - 1);
        let page_end = (ph.p_vaddr + ph.p_memsz + EFI_PAGE_SIZE as u64 - 1)
            & !(EFI_PAGE_SIZE as u64 - 1);
        ranges[range_count] = (page_start, page_end);
        range_count += 1;
    }

    // Insertion-sort the (small, <= MAX_SEGMENTS) range list by start address.
    for i in 1..range_count {
        let mut j = i;
        while j > 0 && ranges[j - 1].0 > ranges[j].0 {
            ranges.swap(j - 1, j);
            j -= 1;
        }
    }

    // Merge adjacent/overlapping ranges into clusters.
    let mut clusters: [Cluster; MAX_SEGMENTS] = core::array::from_fn(|_| Cluster {
        page_start: 0,
        page_count: 0,
    });
    let mut cluster_count = 0;
    for &(start, end) in ranges.iter().take(range_count) {
        if cluster_count > 0 {
            let last = &mut clusters[cluster_count - 1];
            let last_end = last.page_start + last.page_count * EFI_PAGE_SIZE as u64;
            if start <= last_end {
                let new_end = last_end.max(end);
                last.page_count = (new_end - last.page_start) / EFI_PAGE_SIZE as u64;
                continue;
            }
        }
        clusters[cluster_count] = Cluster {
            page_start: start,
            page_count: (end - start) / EFI_PAGE_SIZE as u64,
        };
        cluster_count += 1;
    }

    // Allocate each cluster at its fixed physical address.
    for cluster in clusters.iter().take(cluster_count) {
        let mut addr = cluster.page_start;
        shim_print_label_hex("[shim] allocating cluster@", addr, "");
        let status = (bs.allocate_pages)(
            EFI_ALLOCATE_ADDRESS,
            EFI_LOADER_DATA,
            cluster.page_count as usize,
            &mut addr,
        );
        if status != EFI_SUCCESS {
            shim_print_label_hex("[shim] AllocateAddress FAILED for cluster@", cluster.page_start, "");
            return None;
        }
    }

    // Copy each segment's file contents (identity-mapped vaddr == paddr), zero-filling BSS.
    for i in 0..header.e_phnum as usize {
        let ph_addr = elf_data as usize + header.e_phoff as usize + i * header.e_phentsize as usize;
        let ph = &*(ph_addr as *const Elf64ProgramHeader);
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        let src = (elf_data as usize + ph.p_offset as usize) as *const u8;
        let dst = ph.p_vaddr as *mut u8;
        ptr::copy_nonoverlapping(src, dst, ph.p_filesz as usize);
        if ph.p_memsz > ph.p_filesz {
            let bss_start = dst.add(ph.p_filesz as usize);
            ptr::write_bytes(bss_start, 0, (ph.p_memsz - ph.p_filesz) as usize);
        }
    }

    Some(header.e_entry as usize)
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image_handle: *mut c_void, system_table: *mut c_void) -> usize {
    unsafe {
        let system_table_ref = &*(system_table as *const SystemTable);
        shim_init_serial(system_table as *const SystemTable);
        shim_print("[shim] efi_main entered");

        let rsdp_addr = find_acpi_rsdp(system_table as *const SystemTable).unwrap_or(0);
        shim_print("[shim] RSDP lookup done");

        // Query GOP for the active video mode's linear framebuffer, while boot services (needed
        // for locate_protocol) are still available. Best-effort: a missing/unsupported GOP mode
        // just means no video output, not a boot failure (see locate_gop_framebuffer's doc
        // comment). The FramebufferInfo itself is placed in a fixed static (not the stack) so its
        // address stays valid and known after the stack switch below, to be handed to the kernel
        // as a plain pointer argument.
        static mut FRAMEBUFFER_INFO: FramebufferInfo = FramebufferInfo {
            base: 0,
            width: 0,
            height: 0,
            pixels_per_scan_line: 0,
            bgr: false,
            frame_buffer_size: 0,
        };
        if let Some(fb) = locate_gop_framebuffer(system_table_ref.boot_services) {
            FRAMEBUFFER_INFO = fb;
        }
        let framebuffer_info_addr = ptr::addr_of!(FRAMEBUFFER_INFO) as usize;

        // Number of whole u32 pixels that are actually safe to write, clamped to the GOP's
        // reported `frame_buffer_size` -- `pixels_per_scan_line * height` alone can overrun the
        // real mapping (see `FramebufferInfo`'s doc comment), and an out-of-bounds write here
        // happens before any IDT/GDT is set up, which faults straight into a triple fault/reset --
        // so any raw framebuffer write (including the diagnostic fills in `efi_main`) must clamp its
        // pixel count against this field, not just `pixels_per_scan_line * height`. `base` is `0` if no
        // GOP framebuffer could be found, which the kernel must check before using any other field.
        fn safe_pixel_count(fb: &FramebufferInfo) -> usize {
            let reported = (fb.pixels_per_scan_line as usize) * (fb.height as usize);
            let bound = fb.frame_buffer_size / 4;
            reported.min(bound)
        }

        let (elf_data, elf_size) =
            match load_kernel_elf(image_handle, system_table_ref.boot_services) {
                Some(v) => v,
                None => {
                    shim_print("[shim] load_kernel_elf FAILED, halting");
                    loop {}
                }
            };
        shim_print("[shim] kernel ELF loaded from disk");

        let entry = match load_elf_segments(system_table_ref.boot_services, elf_data, elf_size) {
            Some(v) => v,
            None => {
                shim_print("[shim] load_elf_segments FAILED, halting");
                loop {}
            }
        };
        shim_print("[shim] ELF segments staged, entry point resolved");

        // Allocate a dedicated stack for the kernel. Without this, the kernel would keep running
        // on UEFI's own boot-services stack, whose memory is firmware-owned and no longer
        // guaranteed valid/mapped once ExitBootServices() succeeds -- the first exception taken
        // afterwards would then fault trying to push its own stack frame (a double fault that
        // itself can't be delivered either, escalating straight to an unattributable triple
        // fault, exactly what was observed). 128 KiB, EFI_ALLOCATE_ANY_PAGES so it doesn't
        // collide with the kernel's own fixed-address PT_LOAD segments above.
        const KERNEL_STACK_PAGES: usize = 32; // 32 * 4 KiB = 128 KiB
        let bs = &*system_table_ref.boot_services;
        let mut stack_phys: u64 = 0;
        let status = (bs.allocate_pages)(
            EFI_ALLOCATE_ANY_PAGES,
            EFI_LOADER_DATA,
            KERNEL_STACK_PAGES,
            &mut stack_phys,
        );
        if status != EFI_SUCCESS {
            shim_print("[shim] kernel stack allocation FAILED, halting");
            loop {}
        }
        // Stacks grow down; start RSP at the top of the allocated region. Also keep it 16-byte
        // aligned per the SysV ABI's call-boundary requirement.
        let kernel_stack_top = (stack_phys as usize + KERNEL_STACK_PAGES * 0x1000) & !0xF;
        shim_print("[shim] kernel stack allocated");

        // Print the handoff addresses here, *before* ExitBootServices -- doing this after
        // ExitBootServices succeeds leaves almost no visible time on the vmconnect console before
        // either a clean halt or a triple fault, making it impossible to actually read these
        // values. Boot services (including Stall) are still fully valid here, so follow up with a
        // multi-second Stall to give enough time to actually read the screen before continuing.
        shim_print_symbolicated("[shim]  - `efi_main` = ", efi_main as *const () as usize);
        shim_print_symbolicated("[shim]  - `kernel_entry` = ", entry);
        shim_print_symbolicated("[shim]  - `rsdp_addr` = ", rsdp_addr);
        shim_print_symbolicated("[shim]  - `framebuffer_info_addr` = ", framebuffer_info_addr);
        shim_print_label_hex("[shim]  - `frame_buffer_size` = ", FRAMEBUFFER_INFO.frame_buffer_size as u64, "");
        if !cfg!(feature = "qemu") {
            shim_print("[shim] Stalling 5s before ExitBootServices (read the addresses above now)...");
            (bs.stall)(5_000_000);
        }

        // Fetch the current memory map and call `ExitBootServices` before jumping to the kernel;
        // firmware requires an exact, matching `map_key`, and *any* UEFI call between fetching the
        // map and calling ExitBootServices (even a ConOut/serial print) can itself allocate memory
        // and invalidate that key. So: no `shim_print`/`Stall` calls between the final
        // `GetMemoryMap` refresh (inside the loop below) and `ExitBootServices` itself -- diagnose
        // elsewhere in this function instead, not inside that loop.
        let mut map_size: usize = 0;
        let mut map_key: usize = 0;
        let mut descriptor_size: usize = 0;
        let mut descriptor_version: u32 = 0;

        // First call is expected to fail with EFI_BUFFER_TOO_SMALL, just to learn `map_size`.
        (bs.get_memory_map)(
            &mut map_size,
            ptr::null_mut(),
            &mut map_key,
            &mut descriptor_size,
            &mut descriptor_version,
        );

        // Pad generously: allocating more descriptor slots can itself grow the memory map.
        map_size += 8 * descriptor_size.max(size_of::<u64>() * 8);
        let mut map_buffer: *mut c_void = ptr::null_mut();
        (bs.allocate_pool)(EFI_LOADER_DATA, map_size, &mut map_buffer);
        shim_print("[shim] memory map buffer allocated, retrying ExitBootServices up to 3x");

        let mut status = EFI_SUCCESS;
        for _ in 0..3 {
            let mut this_map_size = map_size;
            (bs.get_memory_map)(
                &mut this_map_size,
                map_buffer,
                &mut map_key,
                &mut descriptor_size,
                &mut descriptor_version,
            );

            status = (bs.exit_boot_services)(image_handle, map_key);
            if status == EFI_SUCCESS {
                break;
            }
            // Stale map_key (something allocated/freed between GetMemoryMap and here) -- refresh
            // and retry with no prints or other UEFI calls in between this time.
        }

        if status != EFI_SUCCESS {
            shim_print_label_hex("[shim] ExitBootServices FAILED after 3 attempts, status=", status as u64, ", halting");
            loop {}
        }

        // DIAGNOSTIC STAGE GATE: bisects exactly where post-ExitBootServices execution dies, by
        // halting (cli/hlt loop) at increasing points instead of running the whole handoff every
        // time. Raise DIAG_STAGE only once the previous stage is confirmed to NOT reset the VM.
        //   0 = halt immediately, before touching the framebuffer or EFER at all.
        //   1 = blue fill only, then halt.
        //   2 = blue + red fill + EFER NXE clear, then halt.
        //   3 = + green fill, then halt (before stack switch/call into kernel_entry).
        //   4 = full handoff (stack switch + call into kernel_entry) -- original behavior.
        const DIAG_STAGE: u8 = if cfg!(feature = "qemu") { 4 } else { 3 };

        if DIAG_STAGE == 0 {
            loop {
                asm!("cli", "hlt", options(nomem, nostack));
            }
        }

        // DIAGNOSTIC: shim_print/Stall no longer work past this point (boot services are gone),
        // so paint the GOP framebuffer directly instead -- still identity-mapped, no firmware
        // call needed. Blue = "ExitBootServices succeeded"; red = "about to touch EFER"; green
        // = "survived EFER, about to switch stack and call kernel_entry". Remove once the handoff
        // is confirmed clean. All three fills are clamped via `safe_pixel_count` to the GOP's
        // reported `frame_buffer_size` so they cannot write past the actual mapped framebuffer.
        if !cfg!(feature = "qemu") && FRAMEBUFFER_INFO.base != 0 {
            let fb = FRAMEBUFFER_INFO.base as *mut u32;
            let count = safe_pixel_count(&FRAMEBUFFER_INFO);
            for i in 0..count {
                fb.add(i).write_volatile(0x0000_00FF); // blue
            }
        }

        if DIAG_STAGE == 1 {
            loop {
                asm!("cli", "hlt", options(nomem, nostack));
            }
        }

        // Clear EFER.NXE (bit 11) before jumping. UEFI firmware commonly enables NX enforcement
        // (EFER.NXE=1) and marks general-purpose/loader-data pages as no-execute per the memory
        // map's EFI_MEMORY_XP attribute. The kernel's PT_LOAD segments were allocated as
        // EFI_LOADER_DATA above, which can leave their mapping marked NX in UEFI's own page
        // tables (still active here -- this kernel does not yet build its own page tables). If
        // so, the very first instruction fetch at the kernel entry point faults immediately,
        // before anything else in the kernel runs -- indistinguishable from "nothing happens"
        // since there is no IDT/stack trust yet to even deliver that fault cleanly, cascading
        // straight to a triple fault. Disabling NX enforcement entirely sidesteps this until the
        // kernel has its own page tables with correctly-marked executable regions.
        if !cfg!(feature = "qemu") && FRAMEBUFFER_INFO.base != 0 {
            let fb = FRAMEBUFFER_INFO.base as *mut u32;
            let count = safe_pixel_count(&FRAMEBUFFER_INFO);
            for i in 0..count {
                fb.add(i).write_volatile(0x00FF_0000); // red
            }
        }
        const IA32_EFER: u32 = 0xC000_0080;
        const EFER_NXE: u64 = 1 << 11;
        let efer_low: u32;
        let efer_high: u32;
        asm!(
            "rdmsr",
            in("ecx") IA32_EFER,
            out("eax") efer_low,
            out("edx") efer_high,
        );
        let efer = ((efer_high as u64) << 32 | efer_low as u64) & !EFER_NXE;
        asm!(
            "wrmsr",
            in("ecx") IA32_EFER,
            in("eax") efer as u32,
            in("edx") (efer >> 32) as u32,
        );

        if DIAG_STAGE == 2 {
            loop {
                asm!("cli", "hlt", options(nomem, nostack));
            }
        }

        // DIAGNOSTIC: green = "survived EFER rdmsr/wrmsr, about to switch stack and call
        // kernel_entry". Remove once the handoff is confirmed clean.
        if !cfg!(feature = "qemu") && FRAMEBUFFER_INFO.base != 0 {
            let fb = FRAMEBUFFER_INFO.base as *mut u32;
            let count = safe_pixel_count(&FRAMEBUFFER_INFO);
            for i in 0..count {
                fb.add(i).write_volatile(0x0000_FF00); // green
            }
        }

        if DIAG_STAGE == 3 {
            loop {
                asm!("cli", "hlt", options(nomem, nostack));
            }
        }

        // Switch to the dedicated kernel stack and jump, all in one inline-asm block so there is
        // no window where the compiler could spill/use the (now-invalid) old stack in between the
        // RSP switch and the call. `_start` now takes a second argument, `framebuffer_info_addr`
        // (passed in `rsi` per the SysV ABI), pointing at the static `FramebufferInfo` populated
        // above; a `base` of 0 in it means "no GOP framebuffer available", which the kernel-side
        // driver must check before touching any other field.
        let kernel_entry: extern "sysv64" fn(usize, usize) -> ! = core::mem::transmute(entry);
        asm!(
            "mov rsp, {stack}",
            "call {entry}",
            stack = in(reg) kernel_stack_top,
            entry = in(reg) kernel_entry,
            in("rdi") rsdp_addr,
            in("rsi") framebuffer_info_addr,
            options(noreturn),
        );
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    shim_print("[shim] PANIC");
    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}


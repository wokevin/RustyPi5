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

const EFI_SUCCESS: usize = 0;
const EFI_FILE_MODE_READ: u64 = 0x0000_0000_0000_0001;
const EFI_ALLOCATE_ANY_PAGES: u32 = 0;
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
    _stall: usize,
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

/// Minimal prefix of `EFI_SYSTEM_TABLE`, just enough to reach `boot_services` and the
/// configuration table.
///
/// # Safety
///
/// This layout must exactly match the UEFI spec's `EFI_SYSTEM_TABLE` layout up through
/// `configuration_table`; any drift will read garbage.
#[repr(C)]
struct SystemTable {
    _header: [u8; 24], // EFI_TABLE_HEADER
    _firmware_vendor: usize,
    _firmware_revision: u32,
    _padding: u32, // alignment for the next pointer field on x86_64
    _console_in_handle: usize,
    _con_in: usize,
    _console_out_handle: usize,
    con_out: *mut c_void,
    _standard_error_handle: usize,
    _std_err: usize,
    _runtime_services: usize,
    boot_services: *mut BootServices,
    number_of_table_entries: usize,
    configuration_table: *const ConfigurationTable,
}

/// Minimal ELF64 header, just enough to find the program header table and entry point.
#[repr(C)]
struct Elf64Header {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    _e_shoff: u64,
    _e_flags: u32,
    _e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    // remaining fields unused.
}

/// Minimal ELF64 program header, just enough to load `PT_LOAD` segments.
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

/// Walk the UEFI configuration table looking for the ACPI 2.0 RSDP.
///
/// # Safety
///
/// - `system_table` must point to a valid, currently-active `EFI_SYSTEM_TABLE`.
unsafe fn find_acpi_rsdp(system_table: *const SystemTable) -> Option<usize> {
    let system_table = &*system_table;

    for i in 0..system_table.number_of_table_entries {
        let entry = &*system_table.configuration_table.add(i);

        if entry.vendor_guid == ACPI_20_TABLE_GUID {
            return Some(entry.vendor_table as usize);
        }
    }

    None
}

/// Formats a small ASCII label plus a numeric EFI status code and prints it via `shim_print`,
/// e.g. `[shim] open_volume status=0x800000000000000e`. Used to pinpoint exactly which UEFI call
/// inside `load_kernel_elf` is failing, since a bare `None` gives no diagnostic information.
fn shim_print_status(label: &str, status: usize) {
    let mut buf = [0u8; 64];
    let mut i = 0;
    for b in label.bytes() {
        if i >= buf.len() {
            break;
        }
        buf[i] = b;
        i += 1;
    }
    for b in b" status=0x" {
        if i >= buf.len() {
            break;
        }
        buf[i] = *b;
        i += 1;
    }
    let hex = b"0123456789abcdef";
    for shift in (0..16).rev() {
        if i >= buf.len() {
            break;
        }
        let nibble = ((status >> (shift * 4)) & 0xF) as usize;
        buf[i] = hex[nibble];
        i += 1;
    }
    shim_print(unsafe { core::str::from_utf8_unchecked(&buf[..i]) });
}

/// Opens `\EFI\BOOT\KERNEL.ELF` on the same volume the shim itself was loaded from, and reads its
/// full contents into a freshly `AllocatePool`-allocated buffer.
///
/// # Safety
///
/// - `image_handle` / `system_table` must be the values handed to `efi_main`.
/// - Must be called while boot services are still active.
unsafe fn load_kernel_elf(
    image_handle: *mut c_void,
    boot_services: *mut BootServices,
) -> Option<(*mut u8, usize)> {
    let bs = &*boot_services;

    // EFI_LOADED_IMAGE_PROTOCOL -> device_handle, so we open the file system on the same volume
    // the shim was booted from instead of assuming a hardcoded handle.
    let mut loaded_image: *mut c_void = ptr::null_mut();
    let status = (bs.handle_protocol)(
        image_handle,
        &LOADED_IMAGE_PROTOCOL_GUID,
        &mut loaded_image,
    );
    shim_print_status("[shim] handle_protocol(LOADED_IMAGE)", status);
    if status != EFI_SUCCESS || loaded_image.is_null() {
        return None;
    }
    let device_handle = (&*(loaded_image as *const LoadedImageProtocol)).device_handle;

    let mut fs_protocol: *mut c_void = ptr::null_mut();
    let status = (bs.handle_protocol)(
        device_handle,
        &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
        &mut fs_protocol,
    );
    shim_print_status("[shim] handle_protocol(SIMPLE_FILE_SYSTEM)", status);
    if status != EFI_SUCCESS || fs_protocol.is_null() {
        return None;
    }
    let fs_protocol = fs_protocol as *mut SimpleFileSystemProtocol;

    let mut root: *mut FileProtocol = ptr::null_mut();
    let status = ((&*fs_protocol).open_volume)(fs_protocol, &mut root);
    shim_print_status("[shim] open_volume", status);
    if status != EFI_SUCCESS || root.is_null() {
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
    shim_print_status("[shim] open(KERNEL.ELF)", status);
    if status != EFI_SUCCESS || kernel_file.is_null() {
        return None;
    }
    let kernel_file = &*kernel_file;

    // Query the file size via GetInfo(EFI_FILE_INFO).
    let mut info_buf = [0u8; 512];
    let mut info_size = size_of::<[u8; 512]>();
    let status = (kernel_file.get_info)(
        kernel_file as *const FileProtocol as *mut FileProtocol,
        &FILE_INFO_GUID,
        &mut info_size,
        info_buf.as_mut_ptr() as *mut c_void,
    );
    shim_print_status("[shim] get_info(FILE_INFO)", status);
    if status != EFI_SUCCESS {
        return None;
    }
    let file_info = &*(info_buf.as_ptr() as *const FileInfo);
    let file_size = file_info.file_size as usize;

    // Stage the ELF file in page-granular memory via AllocatePages (EFI_ALLOCATE_ANY_PAGES)
    // rather than AllocatePool. Pool allocations commonly land in low memory, which can overlap
    // the kernel's own fixed PT_LOAD addresses (e.g. 0x200000) -- when that happens, the later
    // fixed-address AllocatePages call for that kernel segment fails, since those physical pages
    // are already allocated (busy) regardless of their nominal memory type. AllocatePages with
    // EFI_ALLOCATE_ANY_PAGES lets firmware pick any free range, which in practice tends to avoid
    // the kernel's own low fixed addresses.
    let pages_needed = (file_size + 0xFFF) / 0x1000;
    let mut staging_addr: u64 = 0;
    let status = (bs.allocate_pages)(
        EFI_ALLOCATE_ANY_PAGES,
        EFI_LOADER_DATA,
        pages_needed.max(1),
        &mut staging_addr,
    );
    shim_print_status("[shim] allocate_pages(ELF staging)", status);
    if status != EFI_SUCCESS {
        return None;
    }
    let buffer = staging_addr as *mut c_void;

    let mut read_size = file_size;
    let status = (kernel_file.read)(
        kernel_file as *const FileProtocol as *mut FileProtocol,
        &mut read_size,
        buffer,
    );
    shim_print_status("[shim] read(KERNEL.ELF)", status);
    if status != EFI_SUCCESS || read_size != file_size {
        return None;
    }

    Some((buffer as *mut u8, file_size))
}

/// Parses the in-memory ELF64 image, `AllocatePages`-loads each `PT_LOAD` segment at its
/// specified physical/virtual address (this kernel links as if identity-mapped), and returns the
/// entry point.
///
/// # Safety
///
/// - `elf_data` must point to `elf_size` bytes of a valid, statically-linked ELF64 executable.
/// - Must be called while boot services are still active (uses `AllocatePages`).
unsafe fn load_elf_segments(
    boot_services: *mut BootServices,
    elf_data: *const u8,
    elf_size: usize,
) -> Option<usize> {
    if elf_size < size_of::<Elf64Header>() {
        return None;
    }

    let header = &*(elf_data as *const Elf64Header);

    // Sanity-check the ELF magic (`0x7F 'E' 'L' 'F'`) and that this is a 64-bit executable.
    if &header.e_ident[0..4] != b"\x7FELF" || header.e_ident[4] != 2 {
        return None;
    }

    let bs = &*boot_services;
    let ph_table = elf_data.add(header.e_phoff as usize) as *const Elf64ProgramHeader;

    // Collect every PT_LOAD segment's page-aligned [start, end) range. This kernel's linker
    // script places segments in two physically disjoint regions -- the tiny AP trampoline at
    // 0x8000, and the main kernel image as several adjacent/page-sharing segments starting at
    // 0x800000 -- so a single AllocateAddress spanning everything from the lowest to the highest
    // p_vaddr (0x8000 to ~0x811158) would incorrectly try to reserve the large, mostly-reserved
    // gap in between too, and fail. Conversely, allocating each segment separately fails for a
    // different reason: adjacent/page-sharing segments within the main image share physical
    // pages, so the first segment's allocation consumes the very page the next segment's
    // AllocateAddress then finds already claimed (observed directly: 0x800000 succeeds, then
    // 0x80E920 -- on the same page range as the first segment's tail -- fails).
    //
    // The correct middle ground: merge only segments whose page ranges are contiguous or
    // overlapping into clusters, and issue one AllocateAddress per cluster. This naturally
    // produces one cluster for the trampoline and one for the main kernel image, matching the
    // two physically disjoint regions the linker script actually describes.
    const MAX_SEGMENTS: usize = 16;
    let mut ranges: [(u64, u64); MAX_SEGMENTS] = [(0, 0); MAX_SEGMENTS];
    let mut range_count = 0usize;

    for i in 0..header.e_phnum as usize {
        let ph = &*ph_table.add(i);
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        if range_count >= MAX_SEGMENTS {
            shim_print("[shim] FATAL: too many PT_LOAD segments (increase MAX_SEGMENTS)");
            return None;
        }
        let start = ph.p_vaddr & !0xFFFu64;
        let end = (ph.p_vaddr + ph.p_memsz + 0xFFF) & !0xFFFu64;
        ranges[range_count] = (start, end);
        range_count += 1;
    }
    if range_count == 0 {
        return None;
    }

    // Simple insertion sort by start address (range_count is tiny, no need for anything fancier).
    for i in 1..range_count {
        let mut j = i;
        while j > 0 && ranges[j - 1].0 > ranges[j].0 {
            ranges.swap(j - 1, j);
            j -= 1;
        }
    }

    // Merge adjacent/overlapping ranges into clusters, allocating each cluster as soon as the
    // next range proves disjoint from it (or we run out of ranges).
    let mut cluster_start = ranges[0].0;
    let mut cluster_end = ranges[0].1;
    for idx in 1..=range_count {
        let is_last = idx == range_count;
        let starts_new_cluster = !is_last && ranges[idx].0 > cluster_end;

        if is_last || starts_new_cluster {
            // Diagnostic: report what memory type (per the UEFI memory map) currently covers the
            // start of this cluster, before attempting AllocateAddress. This tells us
            // definitively *why* the allocation fails (e.g. EfiReservedMemoryType) instead of
            // guessing.
            report_memory_type_at(boot_services, "cluster", cluster_start);

            let pages = ((cluster_end - cluster_start) as usize) / 0x1000;
            let mut phys_addr = cluster_start;
            let status = (bs.allocate_pages)(
                /* EFI_ALLOCATE_ADDRESS */ 2,
                EFI_LOADER_DATA,
                pages,
                &mut phys_addr,
            );
            // This kernel is statically linked at fixed addresses (not position-independent), so
            // the entry point (`header.e_entry`) and every segment's `p_vaddr` are load-bearing:
            // the kernel assumes it lives exactly where the ELF says it does. If AllocateAddress
            // fails here (e.g. firmware has already reserved that physical range), falling back
            // to a different address is actively wrong: the code ends up loaded somewhere the
            // ELF header doesn't know about, while `e_entry` (used later to jump) still points
            // at the *original* fixed address, which may now be unmapped/unbacked -- an instant
            // instruction-fetch fault on entry, indistinguishable from "nothing happens" with no
            // IDT/stack trust yet to report it. Treat this as a hard failure instead of silently
            // falling back, so it surfaces as a visible diagnostic instead of a silent triple
            // fault.
            if status != EFI_SUCCESS {
                shim_print("[shim] FATAL: AllocateAddress failed for a PT_LOAD cluster");
                return None;
            }

            if !is_last {
                cluster_start = ranges[idx].0;
                cluster_end = ranges[idx].1;
            }
        } else if ranges[idx].1 > cluster_end {
            cluster_end = ranges[idx].1;
        }
    }

    // Every cluster is now reserved; copy each segment's file bytes (and zero-fill its
    // .bss-style tail) into its slice of whichever cluster reservation backs it.
    for i in 0..header.e_phnum as usize {
        let ph = &*ph_table.add(i);
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }

        let dest = ph.p_vaddr as usize;

        let src = elf_data.add(ph.p_offset as usize);
        ptr::copy_nonoverlapping(src, dest as *mut u8, ph.p_filesz as usize);

        // Zero-fill the `.bss`-style tail (`p_memsz > p_filesz`).
        if ph.p_memsz > ph.p_filesz {
            let zero_start = (dest as *mut u8).add(ph.p_filesz as usize);
            ptr::write_bytes(zero_start, 0, (ph.p_memsz - ph.p_filesz) as usize);
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

        // One-time diagnostic: list every free (EfiConventionalMemory) region >= 2 MiB, before
        // any AllocatePages/AllocateAddress calls below can consume/split one. Used to pick a
        // genuinely free fixed kernel load address instead of guessing one address at a time via
        // report_memory_type_at's per-segment check in load_elf_segments.
        dump_conventional_memory(system_table_ref.boot_services);

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

        // Fetch the current memory map and call `ExitBootServices` before jumping to the kernel;
        // firmware requires an exact, matching `map_key`, and *any* UEFI call between fetching the
        // map and calling ExitBootServices (even a ConOut/serial print) can itself allocate memory
        // and invalidate that key. So: no `shim_print` calls in this section, and retry once via a
        // fresh GetMemoryMap if the firmware reports the key is stale (EFI_INVALID_PARAMETER).
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
        shim_print("[shim] memory map buffer allocated");

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
            shim_print("[shim] ExitBootServices FAILED, halting");
            loop {}
        }
        shim_print("[shim] ExitBoot Services succeeded, jumping to kernel entry");

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
        const IA32_EFER: u32 = 0xC000_0080;
        const EFER_NXE: u64 = 1 << 11;
        let efer_low: u32;
        let efer_high: u32;
        asm!(
            "rdmsr",
            in("ecx") IA32_EFER,
            out("eax") efer_low,
            out("edx") efer_high,
            options(nomem, nostack, preserves_flags),
        );
        let efer = ((efer_high as u64) << 32) | (efer_low as u64);
        let new_efer = efer & !EFER_NXE;
        asm!(
            "wrmsr",
            in("ecx") IA32_EFER,
            in("eax") (new_efer & 0xFFFF_FFFF) as u32,
            in("edx") (new_efer >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );

        // Switch to the dedicated kernel stack and jump, all in one inline-asm block so there is
        // no window where the compiler could spill/use the (now-invalid) old stack in between the
        // RSP switch and the call.
        let kernel_entry: extern "sysv64" fn(usize) -> ! = core::mem::transmute(entry);
        asm!(
            "mov rsp, {stack}",
            "call {entry}",
            stack = in(reg) kernel_stack_top,
            entry = in(reg) kernel_entry,
            in("rdi") rsdp_addr,
            options(noreturn),
        );
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

/// Standard `EFI_MEMORY_DESCRIPTOR` layout (UEFI spec). `descriptor_size` returned by
/// `GetMemoryMap` may be larger than `size_of::<MemoryDescriptor>()` (future spec revisions can
/// append fields), so callers must always stride by `descriptor_size`, never
/// `size_of::<MemoryDescriptor>()`.
#[repr(C)]
struct MemoryDescriptor {
    memory_type: u32,
    _padding: u32,
    physical_start: u64,
    _virtual_start: u64,
    number_of_pages: u64,
    _attribute: u64,
}

/// Walks the current UEFI memory map once and prints every `EfiConventionalMemory` (type 7)
/// region's start address and size in pages, via `shim_print`. Intended to be called once, very
/// early in `efi_main` (before any `AllocatePages`/`AllocateAddress` calls that could themselves
/// shrink/split/consume a candidate region), so a genuinely free fixed load address for the
/// kernel's static `PT_LOAD` segments can be chosen with confidence instead of trial-and-error
/// against `report_memory_type_at` one guess at a time.
///
/// # Safety
///
/// - Must be called while boot services are still active.
unsafe fn dump_conventional_memory(boot_services: *mut BootServices) {
    let bs = &*boot_services;

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
    let status = (bs.allocate_pool)(EFI_LOADER_DATA, map_size, &mut map_buffer);
    if status != EFI_SUCCESS || map_buffer.is_null() {
        shim_print("[shim] dump_conventional_memory: allocate_pool FAILED");
        return;
    }

    let mut this_map_size = map_size;
    let status = (bs.get_memory_map)(
        &mut this_map_size,
        map_buffer,
        &mut map_key,
        &mut descriptor_size,
        &mut descriptor_version,
    );
    if status != EFI_SUCCESS {
        shim_print("[shim] dump_conventional_memory: get_memory_map FAILED");
        return;
    }

    const EFI_CONVENTIONAL_MEMORY: u32 = 7;

    let num_descriptors = this_map_size / descriptor_size;
    let base = map_buffer as *const u8;

    shim_print("[shim] --- conventional (free) memory regions ---");
    for i in 0..num_descriptors {
        let desc = &*(base.add(i * descriptor_size) as *const MemoryDescriptor);
        if desc.memory_type != EFI_CONVENTIONAL_MEMORY {
            continue;
        }
        // Only regions with at least 512 pages (2 MiB) are worth considering as a kernel load
        // site; smaller scraps would just make picking a new fixed origin harder to reason about.
        if desc.number_of_pages < 512 {
            continue;
        }
        shim_print_label_hex("[shim] free@", desc.physical_start, "");
        shim_print_label_hex("[shim]   pages=", desc.number_of_pages, "");
    }
    shim_print("[shim] --- end conventional memory regions ---");
}

/// Fetches the current UEFI memory map into a freshly `AllocatePool`-ed buffer (caller leaks it;
/// this is a short-lived diagnostic call during early boot) and reports, via `shim_print`, which
/// memory descriptor (if any) covers `phys_addr`. Used to understand *why* a fixed-address
/// `AllocatePages` call might fail for a specific physical address, instead of guessing.
///
/// # Safety
///
/// - Must be called while boot services are still active.
unsafe fn report_memory_type_at(boot_services: *mut BootServices, label: &str, phys_addr: u64) {
    let bs = &*boot_services;

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
    let status = (bs.allocate_pool)(EFI_LOADER_DATA, map_size, &mut map_buffer);
    if status != EFI_SUCCESS || map_buffer.is_null() {
        shim_print("[shim] report_memory_type_at: allocate_pool FAILED");
        return;
    }

    let mut this_map_size = map_size;
    let status = (bs.get_memory_map)(
        &mut this_map_size,
        map_buffer,
        &mut map_key,
        &mut descriptor_size,
        &mut descriptor_version,
    );
    if status != EFI_SUCCESS {
        shim_print("[shim] report_memory_type_at: get_memory_map FAILED");
        return;
    }

    let num_descriptors = this_map_size / descriptor_size;
    let base = map_buffer as *const u8;

    let mut found = false;
    for i in 0..num_descriptors {
        let desc = &*(base.add(i * descriptor_size) as *const MemoryDescriptor);
        let start = desc.physical_start;
        let end = start + desc.number_of_pages * 0x1000;
        if phys_addr >= start && phys_addr < end {
            found = true;
            // Print label, the memory type (as decimal), and whether it's a "free" type
            // (0 = EfiReservedMemoryType is NOT free; 3/4/7 = Loader/BootServices Code/Data and
            // Conventional memory ARE free/available for AllocateAddress).
            let mut buf = *b"[shim] mem@XXXXXXXXXXXX type=XXX\0";
            // "[shim] mem@" is 11 chars (indices 0..10), so the first hex digit goes at index 11.
            // Print the queried phys_addr (e.g. ph.p_vaddr) itself -- NOT desc.physical_start,
            // which is merely the start of whichever memory-map region happens to contain it.
            // Printing the descriptor's start address here was the actual bug: it silently
            // substituted a plausible-looking but unrelated address (the covering region's
            // boundary) for the segment address actually being checked, making it look like
            // the shim had read the wrong p_vaddr from the ELF when it hadn't.
            let hex_start = 11;
            for j in 0..12 {
                let nibble = (phys_addr >> ((11 - j) * 4)) & 0xF;
                buf[hex_start + j] = if nibble < 10 {
                    b'0' + nibble as u8
                } else {
                    b'A' + (nibble - 10) as u8
                };
            }
            let type_start = 29;
            let t = desc.memory_type;
            buf[type_start] = b'0' + ((t / 100) % 10) as u8;
            buf[type_start + 1] = b'0' + ((t / 10) % 10) as u8;
            buf[type_start + 2] = b'0' + (t % 10) as u8;
            let msg_bytes = &buf[..buf.len() - 1];
            if let Ok(msg) = core::str::from_utf8(msg_bytes) {
                shim_print(msg);
            }
            break;
        }
    }

    if !found {
        shim_print_label_hex("[shim] mem@", phys_addr, " type=NOT_IN_MAP (reserved/unmapped)");
    }

    let _ = label; // label currently unused beyond call-site context; kept for future use.
}

/// Prints `prefix` + `addr` (as 12 hex digits) + `suffix`, all concatenated, via `shim_print`.
fn shim_print_label_hex(prefix: &str, addr: u64, suffix: &str) {
    let mut buf = [0u8; 96];
    let mut pos = 0;
    for &b in prefix.as_bytes() {
        if pos >= buf.len() {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }
    for j in 0..12 {
        if pos >= buf.len() {
            break;
        }
        let nibble = (addr >> ((11 - j) * 4)) & 0xF;
        buf[pos] = if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'A' + (nibble - 10) as u8
        };
        pos += 1;
    }
    for &b in suffix.as_bytes() {
        if pos >= buf.len() {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }
    if let Ok(msg) = core::str::from_utf8(&buf[..pos]) {
        shim_print(msg);
    }
}

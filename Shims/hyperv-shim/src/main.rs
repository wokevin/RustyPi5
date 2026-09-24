#![no_std]
#![no_main]
#![feature(abi_efiapi)]

use core::panic::PanicInfo;

#[no_mangle]
pub extern "efiapi" fn efi_main(_handle: *mut core::ffi::c_void, _system_table: *mut core::ffi::c_void) -> usize {
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

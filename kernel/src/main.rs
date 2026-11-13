#![no_std]
#![no_main]

mod fb;
mod mb2;

use core::fmt::Write;

use arch_x86_64::serial;

/// Entry point called from the 64-bit trampoline in `arch_x86_64`.
///
/// `mb2_info` is the physical address of the multiboot2 information structure,
/// which is identity-mapped.
#[no_mangle]
pub extern "C" fn kmain(mb2_info: u64) -> ! {
    serial::init();
    let mut out = serial::Writer;

    let _ = writeln!(out, "\nigis-v2: long mode entered");
    let _ = writeln!(out, "multiboot2 info: {:#010x}", mb2_info);

    match mb2::framebuffer(mb2_info as usize) {
        Some(info) => {
            let _ = writeln!(
                out,
                "framebuffer: {}x{} {}bpp pitch {} type {} at {:#010x}",
                info.width, info.height, info.bpp, info.pitch, info.kind, info.addr
            );
            if fb::clear(&info) {
                let _ = writeln!(out, "framebuffer cleared");
            } else {
                let _ = writeln!(out, "framebuffer: unsupported pixel format");
            }
        }
        None => {
            let _ = writeln!(out, "no framebuffer");
            arch_x86_64::halt();
        }
    }

    let _ = writeln!(out, "boot stub complete; halting");
    arch_x86_64::halt();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let _ = writeln!(serial::Writer, "PANIC: {info}");
    arch_x86_64::halt();
}

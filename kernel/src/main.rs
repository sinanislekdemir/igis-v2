#![no_std]
#![no_main]

mod console;
mod fb;
mod font8x8;
mod frame;
mod mb2;
mod sync;

use arch_x86_64::{idt, serial};

/// Entry point called from the 64-bit trampoline in `arch_x86_64`.
///
/// `mb2_info` is the physical address of the multiboot2 information structure,
/// which is identity-mapped.
#[no_mangle]
pub extern "C" fn kmain(mb2_info: u64) -> ! {
    serial::init();

    let fb = mb2::framebuffer(mb2_info as usize);
    console::init(fb);

    println!();
    println!("igis-v2: long mode entered");
    println!("multiboot2 info: {:#010x}", mb2_info);

    match fb {
        Some(info) => println!(
            "framebuffer: {}x{} {}bpp type {} at {:#010x}",
            info.width, info.height, info.bpp, info.kind, info.addr
        ),
        None => {
            println!("no framebuffer");
            arch_x86_64::halt();
        }
    }

    idt::init(0x08);
    println!("idt: 32 exception gates installed");

    println!("memory map:");
    let mut available_bytes = 0u64;
    mb2::memory_regions(mb2_info as usize, |region| {
        let kind = match region.kind {
            1 => "available",
            2 => "reserved",
            3 => "acpi reclaimable",
            4 => "acpi nvs",
            5 => "bad memory",
            _ => "other",
        };
        println!(
            "  {:#012x}..{:#012x}  {}",
            region.base,
            region.base.saturating_add(region.len),
            kind
        );
        if region.kind == 1 {
            available_bytes = available_bytes.saturating_add(region.len);
        }
    });
    println!(
        "available: {} MiB",
        available_bytes / (1024 * 1024)
    );

    frame::init(mb2_info as usize, fb);
    println!(
        "frames: {} free of {} ({} MiB usable)",
        frame::free_frames(),
        frame::total_frames(),
        frame::free_frames() * frame::FRAME_SIZE / (1024 * 1024)
    );

    // Self-test: two frames, write and read one, release both.
    match (frame::alloc_frame(), frame::alloc_frame()) {
        (Some(first), Some(second)) => {
            // SAFETY: both frames are outside the kernel image and were just
            // reserved by the allocator; the identity map covers them.
            unsafe { core::ptr::write_volatile(first as *mut u64, 0x1234_5678_9abc_def0) };
            // SAFETY: reading back the same reserved frame.
            let read = unsafe { core::ptr::read_volatile(first as *const u64) };
            println!(
                "frame test: {:#x} reads back {:#018x}, next {:#x}",
                first, read, second
            );
            frame::free_frame(first);
            frame::free_frame(second);
            println!("frames after test: {} free", frame::free_frames());
        }
        _ => println!("frame test: allocation failed"),
    }

    println!("boot stub complete; halting");
    arch_x86_64::halt();
}

/// Called by `arch_x86_64` for any architectural exception.
#[no_mangle]
pub extern "C" fn exception_dispatch(frame: *const idt::InterruptFrame) -> ! {
    // SAFETY: `frame` points at the stack image built by `isr_common`.
    let frame = unsafe { &*frame };

    println!();
    println!(
        "EXCEPTION {} ({}), error {:#x}",
        frame.vector,
        idt::name(frame.vector as usize),
        frame.error
    );
    println!(
        "  rip={:#018x} cs={:#x} rflags={:#x} rsp={:#x} ss={:#x}",
        frame.rip, frame.cs, frame.rflags, frame.rsp, frame.ss
    );
    println!("  rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x}", frame.rax, frame.rbx, frame.rcx, frame.rdx);

    arch_x86_64::halt();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!();
    println!("PANIC: {info}");
    arch_x86_64::halt();
}

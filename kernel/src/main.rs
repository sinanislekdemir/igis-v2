#![no_std]
#![no_main]

mod clock;
mod console;
mod ext2;
mod fb;
mod font8x8;
mod frame;
mod mb2;
mod module;
mod repl;
mod sync;

use arch_x86_64::{apic, gdt, idt, serial};

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

    // SAFETY: boot runs on one core with interrupts disabled; the GDT and TSS
    // are installed exactly once, before anything can fault.
    unsafe { gdt::init() };
    println!("gdt: loaded, task register {:#x}", gdt::task_register());

    idt::init(gdt::KERNEL_CODE);
    println!("idt: exception gates, double-fault IST and timer gates installed");

    match apic::init() {
        Some(apic) => {
            let ticks_per_ms = apic.calibrate();
            println!(
                "apic: id {}, {} ticks/ms",
                apic.id(),
                ticks_per_ms
            );
            apic.start_periodic(ticks_per_ms.max(1));

            // SAFETY: the IDT and the timer are installed; interrupts may run.
            unsafe { core::arch::asm!("sti", options(nostack)) };

            let start = clock::now_ns();
            let mut guard = 0u64;
            while clock::now_ns() < start + 20_000_000 {
                core::hint::spin_loop();
                guard += 1;
                if guard > 5_000_000_000 {
                    break;
                }
            }
            println!("clock: {} ms since boot", clock::now_ns() / 1_000_000);
        }
        None => println!("apic: not present; no timer"),
    }

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

    println!("modules:");
    mb2::for_each_module(mb2_info as usize, |found, command| {
        let command = core::str::from_utf8(command).unwrap_or("<non-utf8>");
        println!(
            "  {:#x}..{:#x} ({} KiB) `{command}`",
            found.start,
            found.end,
            found.len() / 1024
        );
    });
    match module::find(mb2_info as usize, "apps") {
        Some(apps) => {
            println!(
                "  apps: {:#x}..{:#x} ({} KiB)",
                apps.start(),
                apps.start() + apps.len(),
                apps.len() / 1024
            );
            let device = ext2::BlockDevice::new(apps.bytes());
            match ext2::Ext2::mount(device) {
                Ok(fs) => {
                    println!(
                        "ext2: mounted, {} byte blocks, {} KiB",
                        fs.block_size(),
                        fs.size_bytes() / 1024
                    );
                    let mut entries = 0u32;
                    let listed = fs.list(&mut |name, inode, size| {
                        let name = core::str::from_utf8(name).unwrap_or("<non-utf8>");
                        println!("  {name:<24} inode {inode:>4} {size:>6} bytes");
                        entries += 1;
                        Ok(())
                    });
                    match listed {
                        Ok(()) => println!("  {entries} root entries"),
                        Err(status) => println!("  root listing failed: {}", status.name()),
                    }
                    match fs.file_size("shell") {
                        Ok(size) => println!("ext2: shell is {size} bytes"),
                        Err(status) => println!("ext2: shell: {}", status.name()),
                    }

                    repl::boot(&fs);
                }
                Err(status) => {
                    println!("ext2: mount refused ({})", status.name());
                    println!("no entry");
                    arch_x86_64::halt();
                }
            }
        }
        None => {
            println!("  apps: missing");
            println!("no entry");
            arch_x86_64::halt();
        }
    }
}

/// Called by the timer stub in `arch_x86_64` on every tick.
#[no_mangle]
pub extern "C" fn timer_tick() {
    apic::eoi();
    clock::advance_tick();
}

/// Called by `arch_x86_64` for any architectural exception.
#[no_mangle]
pub extern "C" fn exception_dispatch(frame: *const idt::InterruptFrame) -> ! {
    // SAFETY: `frame` points at the stack image built by `isr_common`.
    let frame = unsafe { &*frame };

    // The console lock may be held by the code that faulted, so the fault path
    // writes to serial directly (spec 0002: "Log to serial, halt").
    use core::fmt::Write as _;
    let mut out = serial::Writer;
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "EXCEPTION {} ({}), error {:#x}",
        frame.vector,
        idt::name(frame.vector as usize),
        frame.error
    );
    let _ = writeln!(
        out,
        "  rip={:#018x} cs={:#x} rflags={:#x} rsp={:#x} ss={:#x}",
        frame.rip, frame.cs, frame.rflags, frame.rsp, frame.ss
    );
    let _ = writeln!(
        out,
        "  rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x}",
        frame.rax, frame.rbx, frame.rcx, frame.rdx
    );

    let (cr2, cr3): (u64, u64);
    // SAFETY: reading control registers is legal at ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2);
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
    }
    let _ = writeln!(out, "  cr2={cr2:#x} cr3={cr3:#x}");

    arch_x86_64::halt();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // As above: never take the console lock on the way out.
    use core::fmt::Write as _;
    let mut out = serial::Writer;
    let _ = writeln!(out);
    let _ = writeln!(out, "PANIC: {info}");
    arch_x86_64::halt();
}

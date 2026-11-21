#![no_std]
#![no_main]

mod clock;
mod event;
mod mb2;
mod module;
mod net;
mod repl;
mod stream;
mod sync;

mod mm;
mod platform;
mod sched;
mod ui;
mod vfs;

// The crate keeps the flat `crate::name` paths its modules already use while
// the files live in their subject directories (ADR 0052).
pub(crate) use mm::{frame, guard};
pub(crate) use platform::x86_64::{ata, mouse, pci, smp, usb};
pub(crate) use sched::task;
pub(crate) use ui::{console, display, font, font8x8, gfx, session, window};
pub(crate) use vfs::{ext2, fs};

use arch_x86_64::{apic, gdt, idt, ioapic, keyboard, serial};

/// Entry point called from the 64-bit trampoline in `arch_x86_64`.
///
/// `mb2_info` is the physical address of the multiboot2 information structure,
/// which is identity-mapped.
#[no_mangle]
pub extern "C" fn kmain(mb2_info: u64) -> ! {
    serial::init();
    keyboard::init();

    let fb = mb2::framebuffer(mb2_info as usize);

    // The frame allocator needs only the memory map and the framebuffer range,
    // so it runs before the console: the shadow screen is allocated from it at
    // boot, and the console attaches once the shadow exists (spec 0009).
    frame::init(mb2_info as usize, fb);
    // The Bochs dispi LFB stays at the boot framebuffer's PCI BAR while the
    // mode changes, so reserve the largest supported aperture up front: a
    // larger mode must never overlap a frame the allocator could hand to a
    // shadow or a task region (spec 0022).
    let lfb_reserved = if arch_x86_64::bochs::available() {
        fb.map(|info| {
            let end = info
                .addr
                .saturating_add(16 * 1024 * 1024)
                .min(0x1_0000_0000);
            if end > info.addr {
                frame::reserve_range(info.addr, end);
            }
            end.saturating_sub(info.addr)
        })
    } else {
        None
    };
    let graphics = gfx::init(fb);
    // The window service narrows this clip when it repaints a window (spec
    // 0011, W10). Check the save/narrow/restore arithmetic before the console
    // paints, while the shadow is still the zeroed image, so the restored
    // pixels are exact and nothing is presented.
    let clip_check = gfx::clip_self_check();
    console::init();

    // Console routing (spec 0012): application bytes land in the terminal
    // window and kernel diagnostics in the system-log window, each with real
    // foreground/background pixels in the shadow. It writes its two probe
    // lines through the ordinary paths, clears the grids, and leaves the
    // serial transcript intact.
    match console::route_self_check() {
        Ok(true) => println!("console: route ok"),
        Ok(false) => println!("console: serial only"),
        Err(step) => {
            println!("console: route FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    // Runtime display modes (spec 0022). The Bochs VBE dispi is the QEMU std
    // VGA register interface; its presence is logged before any mode change.
    if arch_x86_64::bochs::available() {
        println!(
            "display: bochs vbe revision {:#06x}, LFB {} KiB reserved",
            arch_x86_64::bochs::revision(),
            lfb_reserved.unwrap_or(0) / 1024
        );
    } else {
        println!("display: no Bochs VBE dispi (runtime mode changes disabled)");
    }

    // Console input is a raw byte ring (spec 0005, ADR 0037). FIFO order, a
    // partial pop, empty reads and the "newest dropped and counted" overflow
    // policy are proved on a scratch ring before the service ever runs.
    match console::input_self_check() {
        Ok(()) => println!("console: input ok"),
        Err(step) => {
            println!("console: input FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    // Console sessions keep the newest output and refuse an oversized input
    // buffer atomically (spec 0021). The check uses scratch rings.
    match session::self_check() {
        Ok(()) => println!("console: session ok"),
        Err(step) => {
            println!("console: session FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    // The keyboard decoder feeds the event queue (spec 0010, I18; ADR 0037).
    // The canned scancodes prove the key layer without a real keyboard; a
    // mismatch halts so it cannot pass silently.
    match keyboard::decoder_self_check() {
        Ok(()) => println!("keyboard: decoder ok"),
        Err(step) => {
            println!("keyboard: decoder FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    // The proportional font's source table (spec 0015): every printable glyph
    // renders to a valid 12-row box, the variable advances hold, and the 8×8
    // fallback is eight pixels per byte. It also warms the glyph cache before
    // the first window title is drawn.
    match font::self_check() {
        Ok(()) => println!("font: glyphs ok"),
        Err(step) => {
            println!("font: glyphs FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    println!();
    println!("igis-v2: long mode entered");
    println!("multiboot2 info: {:#010x}", mb2_info);

    match fb {
        Some(info) => println!(
            "framebuffer: {}x{} {}bpp type {} at {:#010x} pitch {}",
            info.width, info.height, info.bpp, info.kind, info.addr, info.pitch
        ),
        None => println!("framebuffer: none"),
    }
    match graphics {
        gfx::Init::Enabled(report) => println!(
            "graphics: {}x{} {}bpp pitch {} shadow {:#x} ({} KiB, {} frames, free {} -> {})",
            report.width,
            report.height,
            report.bpp,
            report.pitch,
            report.shadow,
            report.shadow_bytes / 1024,
            report.shadow_frames,
            report.free_before,
            report.free_after
        ),
        gfx::Init::Disabled(gfx::Disabled::NoFramebuffer) => {
            println!("graphics: disabled (no framebuffer); console on serial");
        }
        gfx::Init::Disabled(gfx::Disabled::UnsupportedMode { bpp, kind }) => {
            println!("graphics: disabled (mode {bpp}bpp type {kind}); console on serial")
        }
        gfx::Init::Disabled(gfx::Disabled::BadGeometry) => {
            println!("graphics: disabled (bad geometry); console on serial");
        }
        gfx::Init::Disabled(gfx::Disabled::NoMemory { bytes, frames }) => println!(
            "graphics: disabled (shadow {} KiB, {frames} frames); console on serial",
            bytes / 1024
        ),
    }
    println!(
        "frames: {} free of {} ({} MiB usable)",
        frame::free_frames(),
        frame::total_frames(),
        frame::free_frames() * frame::FRAME_SIZE / (1024 * 1024)
    );

    // The window table's policy (spec 0011): geometry, z-order, focus,
    // ownership and the limits, run on a scratch table with fixed geometry.
    // A mismatch halts, so a broken rule cannot pass silently.
    match clip_check {
        Some(true) => println!("graphics: clip ok"),
        Some(false) => {
            println!("graphics: clip FAIL");
            arch_x86_64::halt();
        }
        // Graphics disabled or the mode too small for the test region: there
        // is no clip to exercise and every window call is already `Invalid`.
        None => {}
    }
    match window::self_check() {
        Ok(()) => println!("window: rules ok"),
        Err(step) => {
            println!("window: rules FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // Composition and repair bookkeeping (spec 0011, W11): damage merging, the
    // visible-part subtraction and the damage each operation records. Pure, so
    // it never touches the screen the console is drawing on.
    match window::paint_self_check() {
        Ok(()) => println!("window: paint ok"),
        Err(step) => {
            println!("window: paint FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // Damage coalescing and present batching (spec 0011, W11; spec 0016): the
    // merge rules, the coverage rule, and the measured reduction on a realistic
    // move, all on scratch state.
    match window::damage_self_check() {
        Ok(stats) => println!(
            "window: damage ok (present {} -> {} rects, {} -> {} px)",
            stats.before_rects, stats.after_rects, stats.before_px, stats.after_px
        ),
        Err(step) => {
            println!("window: damage FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // Routing and expose ownership (spec 0011, W12/W16/W17): pending exposes,
    // owner isolation, topmost-first order and hit-testing, all on scratch
    // state.
    match window::route_self_check() {
        Ok(()) => println!("window: route ok"),
        Err(step) => {
            println!("window: route FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // Service-owned pointer interaction (spec 0011, W22/W23): title-bar drag,
    // close-box classification, clamp, release and cancellation, all on
    // scratch state.
    match window::drag_self_check() {
        Ok(()) => println!("window: drag ok"),
        Err(step) => {
            println!("window: drag FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // The close box posts an owner request (spec 0011, W23/W25): flag, take
    // order, owner isolation, owner-0 exemption and flag cleanup, all on
    // scratch state.
    match window::close_self_check() {
        Ok(()) => println!("window: close ok"),
        Err(step) => {
            println!("window: close FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // The one machine-wide modal popup grab (spec 0011, W26; ADR 0044): owner,
    // `Full`, update, invalid extent, owner-only release and termination
    // release, all on scratch state.
    match window::grab_self_check() {
        Ok(()) => println!("window: grab ok"),
        Err(step) => {
            println!("window: grab FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // The persistent pointer-region claim (spec 0011, W27; ADR 0045): claim,
    // `Full`, update, invalid extent, owner-only release, rectangle hit,
    // coexistence with the modal grab and termination release, all on scratch
    // state.
    match window::region_self_check() {
        Ok(()) => println!("window: region ok"),
        Err(step) => {
            println!("window: region FAIL at step {step}");
            arch_x86_64::halt();
        }
    }
    // Window-addressed input delivery (spec 0011, W29; ADR 0047): the hit,
    // focus, region-precedence, addressed, owner-0, no-target and non-owner
    // cases on a scratch table.
    match window::target_self_check() {
        Ok(()) => println!("window: target ok"),
        Err(step) => {
            println!("window: target FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    // The pointer device and the software cursor (spec 0010). This needs the
    // shadow (screen size, cursor compositing) and the console (boot log), and
    // it runs before the scheduler so the first claim sees a live pointer.
    mouse::init();

    // SAFETY: boot runs on one core with interrupts disabled; the GDT and TSS
    // are installed exactly once, before anything can fault.
    unsafe { gdt::init() };
    println!("gdt: loaded, task register {:#x}", gdt::task_register());

    idt::init(gdt::KERNEL_CODE);
    println!("idt: exception gates, double-fault IST and timer gates installed");

    let mut apic_id = None;
    match apic::init() {
        Some(apic) => {
            let ticks_per_ms = apic.calibrate();
            println!("apic: id {}, {} ticks/ms", apic.id(), ticks_per_ms);
            apic.start_periodic(ticks_per_ms.max(1));
            apic_id = Some(apic.id());

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

    // The PS/2 auxiliary interrupt (spec 0010, M2.2.7; ADR 0023). The IDT gate
    // exists (idt::init above) and the local APIC is enabled, so the IOAPIC can
    // route IRQ12 to this core; only then does the controller get its aux
    // interrupt bit. An absent IOAPIC or local APIC is a log line and the
    // existing polling drain stays in place, never a hang.
    let aux_irq = match apic_id {
        Some(destination) => match ioapic::init(12, apic::AUX_VECTOR, destination) {
            Some(version) => {
                println!(
                    "ioapic: version {:#04x}, IRQ12 -> vector {:#x} on apic {} (level)",
                    version & 0xFF,
                    apic::AUX_VECTOR,
                    destination
                );
                true
            }
            None => {
                println!("ioapic: absent or unusable; PS/2 polling only");
                false
            }
        },
        None => {
            println!("ioapic: no local APIC; PS/2 polling only");
            false
        }
    };
    mouse::enable_irq(aux_irq);

    // The absolute pointer (spec 0010, M2.2.6): scan PCI for the PIIX3 UHCI
    // controller and enumerate the USB tablet. This runs after the APIC timer
    // so the bounded port-reset and transfer waits use the running clock, and
    // before the scheduler so the first claim sees the final pointer source.
    let usb_controller = pci::init();
    usb::init(usb_controller);

    // The NIC (spec 0020, M10.1): one legacy virtio-net function behind the
    // `net` device seam. It runs after the USB tablet so the tablet's PCI scan
    // and enumeration order are unchanged, and before the scheduler so the
    // service opportunity can drain frames from the first claim.
    //
    // The polling fallback is selectable at boot (spec 0020, N4): `xtask run
    // --net-irq poll` passes `igis.net_irq=poll` on the multiboot2 command
    // line, which GRUB carries in its command-line tag. Nothing else changes,
    // so the polled proof runs the same kernel and the same image.
    let mut net_poll_only = false;
    mb2::for_each_command_line(mb2_info as usize, |line| {
        if line
            .split(|byte| *byte == b' ')
            .any(|word| word == b"igis.net_irq=poll")
        {
            net_poll_only = true;
        }
    });
    net::init();

    // The NIC's PCI INTx line (QEMU GSI 11 for both virtio-net and
    // e1000/e1000e at 00:03.0) goes through the I/O APIC to this core on
    // `apic::NET_VECTOR`. The IOAPIC page is already split (IRQ12 above), so
    // this programs one more redirection entry and leaves PS/2 alone. A failed
    // route, a requested poll boot or a device that refuses to unmask leaves
    // the existing polling drain in charge; nothing hangs.
    if net_poll_only {
        println!("net: polling (igis.net_irq=poll)");
    }
    let routed = if net_poll_only {
        false
    } else {
        match apic_id {
            Some(destination) => match ioapic::init(11, apic::NET_VECTOR, destination) {
                Some(version) => {
                    println!(
                        "ioapic: version {:#04x}, IRQ11 -> vector {:#x} on apic {} (level)",
                        version & 0xFF,
                        apic::NET_VECTOR,
                        destination
                    );
                    true
                }
                None => {
                    println!("ioapic: IRQ11 route unavailable; net polling only");
                    false
                }
            },
            None => {
                println!("ioapic: no local APIC; net polling only");
                false
            }
        }
    };
    if net::enable_irq(routed) {
        println!("net: irq ok (vector {:#x})", apic::NET_VECTOR);
    } else if !net_poll_only {
        println!("net: polling");
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
    println!("available: {} MiB", available_bytes / (1024 * 1024));

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

    // Everything after early boot runs on a dedicated, guarded stack. The
    // in-kernel compiler is recursive and `spawn` compiles application source,
    // so a depth bug used to write into the boot page tables directly above
    // this stack (Q42, arch_x86_64/src/stack.rs). The guard turns that into a
    // clean #PF; the compiler's own nesting limit is the first line of defence.
    let frames = arch_x86_64::stack::GUARD_FRAMES + arch_x86_64::stack::STACK_FRAMES;
    let guard = match frame::alloc_contiguous(frames) {
        Some(guard) => guard,
        None => {
            println!("stack: cannot allocate {frames} contiguous frames");
            arch_x86_64::halt();
        }
    };
    // SAFETY: the frames were just reserved, the guard is 4 KiB aligned and
    // below 4 GiB, and this runs before the boot stack is abandoned.
    let stack = unsafe { guard::install_guarded(guard) };
    // The boot core's TSS should name its real kernel stack, not the static AP
    // stack `gdt::init` put there (nothing runs at ring 3, but the field must
    // describe reality).
    gdt::set_kernel_stack(smp::core_index(), stack.top);
    println!(
        "stack: guard {:#x}, top {:#x}, {} KiB (higher-half alias: {})",
        stack.guard,
        stack.top,
        arch_x86_64::stack::STACK_BYTES / 1024,
        if stack.aliased { "yes" } else { "no" }
    );

    // SAFETY: `top` is the mapped top of the new stack; the continuation never
    // returns, so the boot stack is abandoned once, on purpose. Interrupts stay
    // enabled across the switch.
    unsafe { arch_x86_64::stack::switch_to(stack.top, kmain_on_stack, mb2_info) }
}

/// The rest of `kmain`, on the guarded stack.
///
/// The multiboot2 structure is still identity-mapped and owned by the
/// bootloader; the frame allocator already reserved it.
extern "C" fn kmain_on_stack(mb2_info: u64) -> ! {
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
                Ok(image) => {
                    println!(
                        "ext2: mounted, {} byte blocks, {} KiB",
                        image.block_size(),
                        image.size_bytes() / 1024
                    );
                    // Publish the router before anything reads a path: the boot
                    // source, `core.spawn`, the compiler's `/lib` reader and the
                    // file builtins all go through `fs` (spec 0019, A4/A5).
                    fs::init(&image);
                    let mut entries = 0u32;
                    let mut name = [0u8; abi::fs::NAME_CAP];
                    loop {
                        match fs::list("", entries, &mut name) {
                            Ok(entry) => {
                                let name = core::str::from_utf8(&name[..entry.name_len])
                                    .unwrap_or("<non-utf8>");
                                println!("  {name:<24} {:>6} bytes", entry.size);
                                entries += 1;
                            }
                            Err(abi::Status::NotFound) => break,
                            Err(status) => {
                                println!("  root listing failed: {}", status.name());
                                break;
                            }
                        }
                    }
                    println!("  {entries} root entries");
                    match fs::file_size("base/shell") {
                        Ok(size) => println!("ext2: base/shell is {size} bytes"),
                        Err(status) => println!("ext2: base/shell: {}", status.name()),
                    }

                    // Spec 0018, L1/L6: the image is a real tree and the file
                    // API walks paths. Prove the walk before any request can be
                    // served; a mismatch halts like the other boot self-checks.
                    match fs::tree_self_check() {
                        Ok(()) => println!("ext2: tree ok"),
                        Err(step) => {
                            println!("ext2: tree FAIL at step {step}");
                            arch_x86_64::halt();
                        }
                    }

                    // Spec 0019, H1/H3: a real home disk is optional. A present
                    // image is mounted through `ext2w` and proved; an absent or
                    // invalid one leaves `/home` on the application image.
                    fs::mount_home();

                    // Bring up the application processors: the boot core's
                    // APIC, GDT/IDT, frame allocator and published mounts are
                    // all ready and the module memory is never unmapped. Absent
                    // ids simply do not report.
                    stream::init();

                    // Spec 0004 C6: the guarded-stack placement invariant.
                    // `Region::allocate_guarded` enforces it and rejects a bad
                    // geometry as `Status::Invalid`; the same rule is run here
                    // over scratch tuples, so a broken layout cannot reach a
                    // task guard. Pure arithmetic; no frame is allocated.
                    match task::stack_layout_self_check() {
                        Ok(()) => println!("stack: layout ok"),
                        Err(step) => {
                            println!("stack: layout FAIL at step {step}");
                            arch_x86_64::halt();
                        }
                    }

                    // Q50: the id-tagged kill record and the block re-check,
                    // proved on real slot state with a scratch task. Runs before
                    // the APs start, so no other core can claim that task.
                    match task::kill_self_check() {
                        Ok(()) => println!("kill: records ok"),
                        Err(step) => {
                            println!("kill: records FAIL at step {step}");
                            arch_x86_64::halt();
                        }
                    }

                    smp::start();

                    // The REPL leaves the boot task in the runnable set; the
                    // scheduler loop owns it and the service queue from here.
                    let _boot_task = repl::boot();
                    sched::run_forever();
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

/// Called by the wake-IPI stub in `arch_x86_64`.
///
/// The scheduling half only acknowledges: the interrupted core re-checks the
/// runnable set after `hlt` (spec 0007). The same vector also carries the guard
/// TLB shootdown, so the handler reloads CR3 when a newer flush is pending
/// (`smp::flush_all`, spec 0002 H2).
#[no_mangle]
pub extern "C" fn wake_ipi() {
    // A halt or reboot request stops every other core through this same
    // vector (spec 0017). Acknowledging and halting here makes a request
    // reliable even when a core is mid-task rather than idle; the boot core
    // uses the same path, so a requester that is an AP also stops it.
    if smp::shutdown_requested() {
        smp::note_stopped(smp::core_index());
        arch_x86_64::halt();
    }
    smp::on_ipi();
    apic::eoi();
}

/// Called by the PS/2 auxiliary (IRQ12) stub in `arch_x86_64` (spec 0010,
/// ADR 0023). Moves bytes into the kernel ring and acknowledges the LAPIC; it
/// takes no kernel lock.
#[no_mangle]
pub extern "C" fn aux_irq() {
    mouse::on_irq();
}

/// Called by the NIC PCI INTx (IRQ11) stub in `arch_x86_64` (spec 0020, N4).
/// Acknowledges the device, sets the pending flag and EOIs the LAPIC; the used
/// rings are drained later, in `net::service`.
#[no_mangle]
pub extern "C" fn net_irq() {
    net::on_irq();
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

    let (cr2, cr3): (u64, u64);
    // SAFETY: reading control registers is legal at ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2);
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
    }

    // A page fault inside a known guard names the stack that ran out. This is
    // a hard bug backstop, not a per-task fault: the interpreter's own frame
    // checks terminate a legal overflow with `Faulted(StackOverflow)` before
    // the hardware guard is reachable (spec 0002 H2). Any other fault keeps the
    // old behaviour. Both halt.
    if frame.vector == 14 {
        if let Some(info) = guard::describe(cr2) {
            match info.kind {
                guard::GuardKind::Task => {
                    let _ = writeln!(out, "  stack guard: task {} (a kernel bug)", info.who);
                }
                guard::GuardKind::ApCore => {
                    let _ = writeln!(out, "  stack guard: ap core {}", info.who);
                }
                guard::GuardKind::BootCore => {
                    let _ = writeln!(out, "  stack guard: boot core");
                }
            }
        }
    }

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

//! SMP discovery, per-core state and the wake-up path (spec 0002 "Boot",
//! spec 0007 "SMP bring-up" and "Interrupts").
//!
//! Discovery is by attempt: there is no ACPI/MADT parser, so the boot core
//! sends INIT-SIPI-SIPI to APIC ids `1..MAX_CORES` through the trampoline in
//! `arch_x86_64::smp` and waits a bounded 100 ms for each to report. An id that
//! never reports is simply not online; a `-smp 1` or `-smp 2` machine boots and
//! schedules on the cores it has.
//!
//! The core index is internal (0 is the boot core); the APIC id is the identity
//! software sees (`syslib.cpu_id`). Every field here is atomic because the boot
//! core, the AP entry and the scheduler loops on all cores touch them.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use arch_x86_64::{apic, gdt, idt, pit, smp as arch};

use crate::clock;
use crate::println;

/// Largest number of cores, from the architecture layer.
pub const MAX_CORES: usize = arch::MAX_CORES;

/// How long the boot core waits for the APs to report online (spec 0007).
const ONLINE_WAIT_NS: u64 = 100_000_000;

/// How long the boot core waits for stop acknowledgements.
const STOP_WAIT_NS: u64 = 1_000_000_000;

/// Per-core state. `idle` and `need_resched` are the wake-up protocol: a
/// producer publishes work first, then claims an idle core by clearing `idle`
/// and sends the IPI; the target re-checks the runnable set after `hlt`.
struct Core {
    online: AtomicBool,
    idle: AtomicBool,
    need_resched: AtomicBool,
    stopped: AtomicBool,
    apic_id: AtomicU32,
    /// IPIs this core observed waking it from `hlt`.
    wakes: AtomicU64,
}

impl Core {
    const fn new() -> Self {
        Core {
            online: AtomicBool::new(false),
            idle: AtomicBool::new(false),
            need_resched: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            apic_id: AtomicU32::new(0),
            wakes: AtomicU64::new(0),
        }
    }
}

static CORES: [Core; MAX_CORES] = [const { Core::new() }; MAX_CORES];

/// APIC id -> core index, or `NONE`. Ids are 8-bit in xAPIC mode.
const NONE: u8 = u8::MAX;
static CORE_OF_APIC: [AtomicU8; 256] = [const { AtomicU8::new(NONE) }; 256];

static ONLINE: AtomicUsize = AtomicUsize::new(0);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// The calling core's index. 0 when the APIC is unavailable or unregistered.
pub fn core_index() -> usize {
    let id = (apic::local_id() & 0xFF) as usize;
    match CORE_OF_APIC[id].load(Ordering::SeqCst) {
        NONE => 0,
        index => usize::from(index),
    }
}

/// The calling core's APIC id (`syslib.cpu_id`).
pub fn apic_id() -> u32 {
    apic::local_id()
}

pub fn is_online(index: usize) -> bool {
    CORES[index].online.load(Ordering::SeqCst)
}

pub fn online_count() -> usize {
    ONLINE.load(Ordering::SeqCst)
}

pub fn apic_id_of(index: usize) -> u32 {
    CORES[index].apic_id.load(Ordering::SeqCst)
}

/// Marks `index` online and records its APIC id.
fn register(index: usize, id: u32) {
    CORES[index].apic_id.store(id, Ordering::SeqCst);
    CORE_OF_APIC[(id & 0xFF) as usize].store(index as u8, Ordering::SeqCst);
    if !CORES[index].online.swap(true, Ordering::SeqCst) {
        ONLINE.fetch_add(1, Ordering::SeqCst);
    }
}

/// Records that this core is (about to be) idle. Set before the work check so a
/// producer that publishes between the check and `hlt` still finds it and sends
/// the IPI; cleared when the core is about to look for work again.
pub fn mark_idle(index: usize) {
    CORES[index].idle.store(true, Ordering::SeqCst);
}

/// Clears this core's idle flag as it leaves the idle loop.
pub fn mark_busy(index: usize) {
    CORES[index].idle.store(false, Ordering::SeqCst);
}

/// Claims one idle core and sends it the wake IPI.
///
/// The publication of work must happen before this call. Nothing is sent when
/// every other core is busy: a busy core re-scans the runnable set after its
/// quantum. The caller's own core is skipped.
pub fn kick_one() {
    let self_index = core_index();
    for index in 0..MAX_CORES {
        if index == self_index || !is_online(index) {
            continue;
        }
        if CORES[index].idle.swap(false, Ordering::SeqCst) {
            CORES[index].need_resched.store(true, Ordering::SeqCst);
            apic::send_ipi(apic_id_of(index), apic::WAKE_VECTOR);
            return;
        }
    }
}

/// Consumes this core's pending kick. True when an IPI (or a producer's flag)
/// asked for a re-check.
pub fn take_kick(index: usize) -> bool {
    CORES[index].need_resched.swap(false, Ordering::SeqCst)
}

/// Counts one observed wake-up on `index`.
pub fn note_wake(index: usize) {
    CORES[index].wakes.fetch_add(1, Ordering::SeqCst);
}

pub fn wake_count(index: usize) -> u64 {
    CORES[index].wakes.load(Ordering::SeqCst)
}

/// True when every online core except `except` is idle.
pub fn all_idle_except(except: usize) -> bool {
    (0..MAX_CORES).all(|index| {
        index == except || !is_online(index) || CORES[index].idle.load(Ordering::SeqCst)
    })
}

/// Tells every online core to stop and waits, bounded, for the acknowledgements
/// (spec 0007, "Termination" of the system: the boot core prints one summary).
pub fn stop_all() -> (usize, usize, [bool; MAX_CORES]) {
    SHUTDOWN.store(true, Ordering::SeqCst);
    apic::broadcast_wake();

    // The boot core does not acknowledge (it is the one asking); the target is
    // every other online core.
    let expected = online_count().saturating_sub(1);
    let deadline = clock::now_ns().saturating_add(STOP_WAIT_NS);
    loop {
        let mut stopped = [false; MAX_CORES];
        let mut count = 0;
        for (index, core) in CORES.iter().enumerate().take(MAX_CORES) {
            if index == 0 {
                continue;
            }
            let ack = core.online.load(Ordering::SeqCst) && core.stopped.load(Ordering::SeqCst);
            stopped[index] = ack;
            count += usize::from(ack);
        }
        if count >= expected || clock::now_ns() >= deadline {
            return (count, expected, stopped);
        }
        core::hint::spin_loop();
    }
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Acknowledges a shutdown request from the idle loop; the caller then halts.
pub fn note_stopped(index: usize) {
    CORES[index].stopped.store(true, Ordering::SeqCst);
}

/// Starts the APs. Returns the number of online cores, boot core included.
///
/// The image and every kernel subsystem the AP entry touches (GDT, IDT, console,
/// APIC, frame allocator) must already be initialised; the boot core's timer is
/// already running, which is what makes the bounded waits below terminate even
/// on a machine with no APs at all.
pub fn start() -> usize {
    let boot_id = apic::local_id();
    register(0, boot_id);

    if online_count() >= MAX_CORES {
        return online_count();
    }
    // SAFETY: paging is on, the low 1 MiB is reserved by the frame allocator,
    // and nothing else has written the trampoline or parameter pages.
    if !unsafe { arch::upload(ap_entry) } {
        println!("smp: trampoline does not fit its page; boot core only");
        return online_count();
    }

    let vector = arch::trampoline_vector();
    // INIT to every candidate id, the standard 10 ms wait, then two SIPIs
    // about 200 us apart. Deliveries to absent ids are simply never delivered.
    for id in 1..MAX_CORES as u32 {
        apic::send_init(id);
    }
    // SAFETY: channel 2 of the PIT is used only by these bring-up delays.
    unsafe { pit::delay_us(10_000) };
    for id in 1..MAX_CORES as u32 {
        apic::send_startup(id, vector);
    }
    // SAFETY: as above; the SIPI spacing must not depend on the tick.
    unsafe { pit::delay_us(200) };
    for id in 1..MAX_CORES as u32 {
        apic::send_startup(id, vector);
    }

    // Bounded: ids that never report are not online. A machine with -smp 4
    // reports three APs and leaves ids 4.. alone.
    let deadline = clock::now_ns().saturating_add(ONLINE_WAIT_NS);
    while online_count() < MAX_CORES && clock::now_ns() < deadline {
        core::hint::spin_loop();
    }

    // Prove the IPI path: wake every AP once and count the acknowledgements the
    // idle loops report. `need_resched` is set before the IPI, so an AP that
    // was still registering when the IPI arrived still observes the kick.
    let ap_count = online_count() - 1;
    if ap_count > 0 {
        for index in 1..MAX_CORES {
            if is_online(index) {
                CORES[index].need_resched.store(true, Ordering::SeqCst);
                apic::send_ipi(apic_id_of(index), apic::WAKE_VECTOR);
            }
        }
        let deadline = clock::now_ns().saturating_add(ONLINE_WAIT_NS);
        let acked = |min: u64| {
            (1..MAX_CORES)
                .filter(|&index| is_online(index) && wake_count(index) >= min)
                .count()
        };
        while acked(1) < ap_count && clock::now_ns() < deadline {
            core::hint::spin_loop();
        }
        println!("smp: IPI wake: {} of {} APs acknowledged", acked(1), ap_count);
    }

    print_summary();
    online_count()
}

/// The `smp: N cores online ...` line, one atomic console write.
fn print_summary() {
    let count = online_count();
    let mut ids = [0u32; MAX_CORES];
    let mut online = 0;
    for core in CORES.iter().take(MAX_CORES) {
        if core.online.load(Ordering::SeqCst) {
            ids[online] = core.apic_id.load(Ordering::SeqCst);
            online += 1;
        }
    }
    println!(
        "smp: {count} cores online (boot + {} APs), apic ids {:?}",
        count.saturating_sub(1),
        &ids[..online]
    );
}

/// The AP entry point, called by the trampoline with the claimed core index.
///
/// The trampoline has already enabled paging with the kernel's PML4 and is
/// running on this core's stack. Everything below is ordinary kernel Rust.
extern "C" fn ap_entry(index: u64) -> ! {
    let core = (index as usize).min(MAX_CORES - 1);

    // SAFETY: the GDT and IDT were built by the boot core before the SIPI and
    // are read-only from here; this core loads them before interrupts exist.
    unsafe { gdt::load(core) };
    // SAFETY: as above.
    unsafe { idt::load() };

    match apic::init() {
        Some(apic) => {
            apic.enable();
            let id = apic.id();
            register(core, id);
            println!("smp: core {core} online (apic id {id})");
        }
        None => {
            println!("smp: core {core} has no local APIC; halting it");
            arch_x86_64::halt();
        }
    }

    arch_x86_64::enable_interrupts();
    crate::sched::run_secondary(core)
}

//! The boot core's interpreter loop (spec 0007).
//!
//! One core, one loop, one global runnable set: claim a task, advance it for a
//! quantum, release it or terminate it, repeat. The per-core shape is already
//! the final one (Sch1), but APs are not started yet (Sch11, second half).
//!
//! # Claim, quantum and budget
//!
//! [`task::claim`] removes a [`Task`] from the runnable table under its slot
//! lock; the move is the exclusive ownership transfer (Sch2, Sch3). The loop
//! then runs at most [`QUANTUM_STEPS`] bytecodes with no scheduler lock held,
//! and puts the value back with [`task::release`] (Sch7).
//!
//! Two limits apply to a task. The quantum is fairness: expiry releases and
//! re-enqueues, never faults. The total CPU budget is the watchdog and expires
//! unconditionally into `Faulted(BudgetExceeded)` (Sch8). The budget is tracked
//! two ways:
//!
//! - **CPU time** ([`TOTAL_BUDGET_NS`]) from claim/release timestamps;
//! - **bytecodes** ([`TOTAL_BUDGET_STEPS`]), because the clock ticks at 1 ms
//!   (spec 0007, "Time") and a 10 000-step quantum can measure as 0 ns. Until
//!   the clock is finer than the quantum, the step cap is the watchdog of
//!   record and the time budget is the backstop for slow builtins.
//!
//! # Realtime
//!
//! `core.realtime_on()` admits the task to the realtime class through
//! [`admit_realtime`]: one realtime task per online core, never preempted at
//! quantum expiry, and exempt from the total budget. The replacement watchdog
//! is the one-second heartbeat ([`HEARTBEAT_NS`]) measured in non-blocked time;
//! a task whose accumulated claim-to-release time passes its deadline is
//! terminated with `Faulted(WatchdogTimeout)`. The admission slot is released
//! only in `terminate`, so a blocked realtime task keeps it (ADR 0016).
//!
//! # Interrupts
//!
//! Interrupts stay enabled: the timer tick only advances the global clock and
//! acknowledges the APIC, and it never claims a task or takes a scheduler lock,
//! so it cannot re-enter the loop. Claim and release therefore only need the
//! slot spin lock, not `cli`. No lock is held across [`Machine::run`]; the
//! verifier scratch lock is dropped after parsing, and the console lock is
//! taken and released inside the host's `console_write`.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use abi::{ExitReason, FaultCode, Status};
use interpreter::{Diagnostic, Host, Outcome, Program, StreamError, WaitKind, WaitOutcome};

use crate::clock;
use crate::console;
use crate::ext2::Ext2;
use crate::frame;
use crate::println;
use crate::repl;
use crate::smp;
use crate::sync::SpinLock;
use crate::task::{self, Task, TaskClass, WaitFor};

/// Bytecodes executed per claim (spec 0007 Q1, provisional).
///
/// Step count is the primary quantum: it is independent of host speed and of
/// the coarse 1 ms tick. Tune after measurement.
pub const QUANTUM_STEPS: u64 = 10_000;

/// Total CPU time a task may consume before it is terminated (spec 0007 Q2,
/// provisional). Blocked time is not counted; a task that never reaches a safe
/// point cannot be killed (accepted cooperative liveness hole). Not applied to
/// the realtime class, where the heartbeat replaces it.
pub const TOTAL_BUDGET_NS: u64 = 10_000_000_000;

/// Hard ceiling on total bytecodes per task.
///
/// The 1 ms clock cannot resolve sub-millisecond quanta, so most claims add
/// 0 ns to [`TOTAL_BUDGET_NS`]. This cap is the watchdog that actually fires
/// in the first milestone; 200 000 000 steps is the same order of magnitude as
/// ten seconds at interpreter speed. Not applied to the realtime class.
pub const TOTAL_BUDGET_STEPS: u64 = 200_000_000;

/// One second of non-blocked realtime time between heartbeats (spec 0007,
/// ADR 0016). A fixed compile-time constant; OPEN-QUESTIONS Q4 closed on it.
pub const HEARTBEAT_NS: u64 = 1_000_000_000;

/// Realtime tasks admitted machine-wide (spec 0007, Sch13).
///
/// The cap is [`smp::online_count`] at admission time; it never decreases in
/// this milestone. A blocked realtime task keeps its slot, so the count does
/// not fall when a controller waits on a device.
static REALTIME_ADMITTED: AtomicUsize = AtomicUsize::new(0);

/// Reserves one realtime admission slot, or refuses with `Status::Full`.
///
/// The CAS loop makes concurrent promotions on different cores safe: two tasks
/// cannot both observe room for the last slot. The online count is sampled per
/// attempt; an earlier `Full` is not re-evaluated when an AP arrives later, so
/// the caller retries.
pub fn admit_realtime() -> Status {
    let capacity = smp::online_count().max(1);
    let mut admitted = REALTIME_ADMITTED.load(Ordering::Acquire);
    loop {
        if admitted >= capacity {
            return Status::Full;
        }
        match REALTIME_ADMITTED.compare_exchange_weak(
            admitted,
            admitted + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Status::Ok,
            Err(observed) => admitted = observed,
        }
    }
}

/// Releases an admission slot. Called exactly once, from `terminate`, for a
/// task whose `admitted` flag is still set; never on block or release.
fn release_realtime() {
    REALTIME_ADMITTED.fetch_sub(1, Ordering::AcqRel);
}

/// Verifier scratch: one bit per 4-byte instruction word. Shared by the loader
/// (initial validation) and the loop (re-parse per quantum). One buffer per
/// core: the interpreter must have no global mutable state (Sch6), so two cores
/// parsing at the same time each use their own scratch and never wait on each
/// other.
const VERIFY_BYTES: usize = compiler::MAX_CODE_BYTES / 4 / 8;
static VERIFY: [SpinLock<[u8; VERIFY_BYTES]>; smp::MAX_CORES] =
    [const { SpinLock::new([0; VERIFY_BYTES]) }; smp::MAX_CORES];

// Per-core loop statistics; the boot core sums them in the final summary.
static CLAIMS: [AtomicU64; smp::MAX_CORES] = [const { AtomicU64::new(0) }; smp::MAX_CORES];
static YIELDS: [AtomicU64; smp::MAX_CORES] = [const { AtomicU64::new(0) }; smp::MAX_CORES];
static EXITS: [AtomicU64; smp::MAX_CORES] = [const { AtomicU64::new(0) }; smp::MAX_CORES];

/// The kernel's implementation of the interpreter host interface (spec 0005).
///
/// `current` is the id of the task being advanced, so `core.wait` on oneself is
/// `Invalid` rather than a permanent block. `image` is the read-only ext2
/// mount the REPL service compiles from.
struct KernelHost<'a> {
    image: &'a Ext2<'a>,
    current: u64,
    /// The task's class at quantum start. A `realtime_on` on an already
    /// realtime task is a no-op that consumes no second admission slot.
    class: TaskClass,
    /// `realtime_on` succeeded during this quantum. The task value is owned by
    /// the core, not the host, so the loop applies the promotion afterwards.
    promoted: bool,
    /// Heartbeats observed in this quantum. The loop resets the deadline from
    /// the accumulated non-blocked time after adding this quantum's elapsed
    /// time; a heartbeat in the middle of a quantum is therefore honoured at
    /// the next boundary.
    heartbeats: u64,
    /// The diagnostic image carried by the exit record `wait` most recently
    /// served, and its id. `core.run` calls `diagnostic_for` right after that
    /// wait in the same builtin step, so a per-quantum cache is enough; the
    /// exit ring itself is consumed by the wait.
    exit_diag: [u8; repl::DIAG_BYTES],
    exit_diag_id: u64,
    exit_diag_ready: bool,
}

impl Host for KernelHost<'_> {
    fn console_write(&mut self, bytes: &[u8]) {
        crate::console::write_bytes(bytes);
    }

    fn read_line(&mut self, buf: &mut [u8]) -> Option<usize> {
        console::read_line(buf)
    }

    fn clock(&mut self) -> u64 {
        clock::now_ns()
    }

    // The first `syslib` primitives (spec 0003, "Hardware access"): x86 port
    // I/O over the existing `arch_x86_64::port` helpers. `Port` is staged as a
    // `uint` (Q45); the interpreter already narrowed it to 16 bits, and the
    // caller of `syslib` is responsible for using a port that is safe to touch
    // (no capability model, H4).
    fn port_in8(&mut self, port: u16) -> u8 {
        // SAFETY: the application named a port through `syslib`; reading a port
        // has no memory side effects, and there is no capability check by
        // design (spec 0003, H4).
        unsafe { arch_x86_64::port::inb(port) }
    }

    fn port_out8(&mut self, port: u16, value: u8) {
        // SAFETY: as above; the caller chose the port. The kernel does not
        // arbitrate device ownership in the first milestone.
        unsafe { arch_x86_64::port::outb(port, value) }
    }

    // The core the calling task is running on right now, as an APIC id
    // (`syslib.cpu_id`). A task is migratable, so this is the proof hook for
    // distribution, not an affinity (ADR 0011).
    fn cpu_id(&mut self) -> u64 {
        u64::from(crate::smp::apic_id())
    }

    // Realtime promotion and heartbeat (spec 0007, ADR 0016). Admission is a
    // machine-wide budget of one realtime task per online core; the task value
    // stays with the core, so the host only records what the loop must apply.
    fn realtime_on(&mut self) -> u64 {
        if self.class == TaskClass::Realtime || self.promoted {
            // One-way promotion: a repeat call is `Ok` and takes no slot.
            return Status::Ok as u64;
        }
        let status = admit_realtime();
        if status == Status::Ok {
            self.promoted = true;
        }
        status as u64
    }

    fn heartbeat(&mut self) {
        // A normal task's heartbeat is a no-op (its total budget still applies).
        if self.class == TaskClass::Realtime || self.promoted {
            self.heartbeats += 1;
        }
    }

    // `syslib` graphics (spec 0009). The machine has already checked extents
    // and colours; the kernel's `gfx` module clips to the clip region and the
    // screen and serialises drawing with its own lock, so an application on any
    // core sees one image.
    fn screen_width(&mut self) -> u64 {
        u64::from(crate::gfx::size().0)
    }

    fn screen_height(&mut self) -> u64 {
        u64::from(crate::gfx::size().1)
    }

    fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        crate::gfx::fill_rect(x, y, w, h, color)
    }

    fn fill_dither(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        crate::gfx::fill_dither(x, y, w, h, color)
    }

    fn draw_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        crate::gfx::draw_rect(x, y, w, h, color)
    }

    fn draw_text(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
        crate::gfx::draw_text(x, y, text, color)
    }

    fn get_pixel(&mut self, x: i64, y: i64) -> u32 {
        crate::gfx::get_pixel(x, y)
    }

    fn present(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status {
        crate::gfx::present(x, y, w, h)
    }

    fn clear(&mut self, color: u32) -> Status {
        crate::gfx::clear(color)
    }

    // Streams (spec 0006): the kernel table owns the buffer and the generation
    // check. The machine turns `WouldBlock` into a block for the blocking
    // opcodes and into a staged status for `try_*`; the host is always
    // non-blocking so it can be called from any core.
    fn stream_send(&mut self, handle: u64, bytes: &[u8]) -> Result<(), StreamError> {
        crate::stream::send(self.current, handle, bytes)
    }

    fn stream_recv(&mut self, handle: u64, dst: &mut [u8]) -> Result<usize, StreamError> {
        crate::stream::recv(self.current, handle, dst)
    }

    fn spawn(&mut self, path: &[u8], args: &[u8], diag: &mut [u8]) -> Result<u64, Status> {
        repl::request_spawn(self.image, self.current, path, args, diag)
    }

    fn wait(&mut self, id: u64) -> WaitOutcome {
        if id == 0 || id == self.current {
            return WaitOutcome::Status(Status::Invalid);
        }
        // A record written by `terminate` is consumed first: the child may
        // have finished before the parent reached `wait`. A request failure
        // also carries the compiler's diagnostic; keep it for `core.run`.
        if let Some((reason, diag)) = repl::take_exit(id) {
            self.exit_diag_id = id;
            self.exit_diag_ready = false;
            if let Some(diag) = diag {
                self.exit_diag = diag;
                self.exit_diag_ready = true;
            }
            return WaitOutcome::Exited(reason);
        }
        if task::is_live(id) || repl::is_pending(id) {
            return WaitOutcome::Blocked;
        }
        // The child may have terminated between the two checks above. The exit
        // record is written before the slot is released, so a second look
        // finds it when the child is really gone.
        if let Some((reason, diag)) = repl::take_exit(id) {
            self.exit_diag_id = id;
            self.exit_diag_ready = false;
            if let Some(diag) = diag {
                self.exit_diag = diag;
                self.exit_diag_ready = true;
            }
            return WaitOutcome::Exited(reason);
        }
        WaitOutcome::Status(Status::NotFound)
    }

    fn diagnostic_for(&mut self, id: u64, diag: &mut [u8]) -> bool {
        if !self.exit_diag_ready || id != self.exit_diag_id {
            return false;
        }
        let copied = diag.len().min(self.exit_diag.len());
        diag[..copied].copy_from_slice(&self.exit_diag[..copied]);
        true
    }

    fn kill(&mut self, id: u64) -> Status {
        if id == 0 || id == self.current {
            return Status::Invalid;
        }
        match task::request_kill(id) {
            Status::NotFound => {
                // The request may still be in the queue: cancel it and record
                // `Killed` under the reserved id (spec 0007).
                if repl::cancel_request(id) {
                    repl::record_exit(id, ExitReason::Killed);
                    task::wake(id);
                    Status::Ok
                } else {
                    Status::NotFound
                }
            }
            status => status,
        }
    }
}

/// Verifies a package and decodes its sections.
///
/// Both the loader and the loop use this; the scratch lock is held only for
/// the duration of the parse, and the returned [`Program`] borrows `code`, not
/// the scratch.
pub fn parse_program(code: &[u8]) -> Result<Program<'_>, Diagnostic> {
    parse_program_on(smp::core_index(), code)
}

/// As [`parse_program`], on one core's scratch buffer.
fn parse_program_on(core: usize, code: &[u8]) -> Result<Program<'_>, Diagnostic> {
    let mut verify = VERIFY[core].lock();
    Program::parse(code, &mut verify[..])
}

/// The ext2 mount the REPL service compiles from, published before the APs
/// start (every core's loop needs it).
static IMAGE: SpinLock<Option<&'static Ext2<'static>>> = SpinLock::new(None);

/// Publishes the mount for the AP loops. Called once on the boot core, before
/// the first SIPI.
pub fn set_image(image: &Ext2<'_>) {
    // SAFETY: the mount borrows the GRUB module's bytes, which the bootloader
    // owns for the lifetime of the system; nothing unmaps or frees them (see
    // `module::bytes`). Extending the borrow to 'static is what lets the AP
    // loops share the read-only mount without an allocator.
    let image: &'static Ext2<'static> = unsafe { core::mem::transmute(image) };
    *IMAGE.lock() = Some(image);
}

/// The published mount, for a core that did not receive it as an argument.
fn image() -> &'static Ext2<'static> {
    match *IMAGE.lock() {
        Some(image) => image,
        None => {
            println!("sched: no image published; this core halts");
            arch_x86_64::halt();
        }
    }
}

/// The boot core's scheduler loop. Never returns.
///
/// The loop ends when no task is live and no request is pending: with no
/// console input there is no other source of work, so it prints the summary and
/// halts. A blocked task keeps the system alive (it will be woken by a child
/// termination or a finished console line), so the check is "no live task", not
/// "boot task finished".
pub fn run_forever(image: &Ext2<'_>) -> ! {
    loop_body(smp::core_index(), image)
}

/// An AP's scheduler loop: the same claim → quantum → release cycle over the
/// same global runnable set (Sch1–Sch3). Any core may claim any task.
pub fn run_secondary(core: usize) -> ! {
    loop_body(core, image())
}

fn loop_body(core: usize, image: &Ext2<'_>) -> ! {
    loop {
        // The service opportunity runs before every claim: at most one
        // pending request is compiled and enqueued, on whichever core got
        // here first (spec 0007, "Services and task creation").
        service(image);

        // The atomic take under the slot lock is the claim (Sch2, Sch3).
        match task::claim() {
            Some(mut task) => {
                // A realtime task is not released at quantum expiry: this inner
                // loop runs further quanta on the same core until it blocks,
                // exits, observes a kill, or misses its heartbeat deadline. A
                // normal task always leaves after one quantum.
                loop {
                    let started = clock::now_ns();
                    let quantum = run_quantum(core, &mut task, image);
                    let elapsed = clock::now_ns().saturating_sub(started);

                    task.budget_ns = task.budget_ns.saturating_add(elapsed);
                    task.claims += 1;
                    CLAIMS[core].fetch_add(1, Ordering::Relaxed);

                    // Apply what the host observed. Promotion is one-way and
                    // takes effect at this boundary, when the core still owns
                    // the task value. The deadline is measured in the
                    // accumulated claim-to-release time, which never includes
                    // blocked time.
                    if quantum.promoted {
                        task.class = TaskClass::Realtime;
                        task.admitted = true;
                        task.heartbeat_deadline_ns = task.budget_ns.saturating_add(HEARTBEAT_NS);
                    }
                    if task.class == TaskClass::Realtime {
                        task.heartbeats += quantum.heartbeats;
                        if quantum.heartbeats > 0 {
                            task.heartbeat_deadline_ns =
                                task.budget_ns.saturating_add(HEARTBEAT_NS);
                        }
                    }

                    // A kill may have been requested while this core owned the
                    // task value (in the slot) or, for a running task, by
                    // another core through `KILL_FLAGS`. Both are honoured at
                    // this safe point (spec 0007, "Termination").
                    let killed = task.kill_requested || task::take_kill(task.slot);

                    match quantum.outcome {
                        Outcome::Exited(reason) => {
                            terminate(core, task, reason);
                            break;
                        }
                        _ if killed => {
                            terminate(core, task, ExitReason::Killed);
                            break;
                        }
                        Outcome::Blocked(kind) => {
                            // The blocked task keeps its slot but stops being
                            // claimable; a realtime task also keeps its
                            // admission slot while blocked. A child termination
                            // wakes `ChildExit`; a finished console line wakes
                            // `Console` (Sch9).
                            match kind {
                                WaitKind::ChildExit(child) => {
                                    println!("sched: task {} blocked on task {}", task.id, child);
                                    task::block(task, WaitFor::Child(child));
                                    // The child may have recorded its exit
                                    // between the failed `wait` and the block
                                    // above; the wake-up path then found nothing
                                    // to wake.
                                    if repl::has_exit(child) {
                                        task::wake(child);
                                    }
                                }
                                WaitKind::Console => {
                                    println!("sched: task {} blocked on console", task.id);
                                    task::block(task, WaitFor::Console);
                                }
                                WaitKind::Stream(handle) => {
                                    println!(
                                        "sched: task {} blocked on stream {handle:#x}",
                                        task.id
                                    );
                                    let id = task.id;
                                    task::block(task, WaitFor::Stream(handle));
                                    // Space/data/close may have arrived between
                                    // the failed attempt and the block; the
                                    // wake path then found nobody to wake.
                                    if crate::stream::ready_for(id, handle) {
                                        task::wake_stream(handle);
                                    }
                                }
                            }
                            break;
                        }
                        Outcome::Yielded if task.class == TaskClass::Realtime => {
                            // No quantum preemption (Sch14): the quantum still
                            // expires so the core can observe the kill flag and
                            // the heartbeat deadline, but the task is not
                            // released. The total CPU budget does not apply.
                            if task.budget_ns >= task.heartbeat_deadline_ns {
                                terminate(
                                    core,
                                    task,
                                    ExitReason::Faulted(FaultCode::WatchdogTimeout),
                                );
                                break;
                            }
                            task.yields += 1;
                            YIELDS[core].fetch_add(1, Ordering::Relaxed);
                        }
                        Outcome::Yielded => {
                            let expired = task.budget_ns >= TOTAL_BUDGET_NS
                                || task.machine.steps() >= TOTAL_BUDGET_STEPS;
                            if expired {
                                // Sch8: unconditional, no `fault` handler in ABI v1.
                                terminate(
                                    core,
                                    task,
                                    ExitReason::Faulted(FaultCode::BudgetExceeded),
                                );
                            } else {
                                task.yields += 1;
                                YIELDS[core].fetch_add(1, Ordering::Relaxed);
                                // Release re-enqueues; the release path kicks
                                // one idle core so the task can migrate
                                // immediately.
                                task::release(task);
                            }
                            break;
                        }
                    }
                }
            }
            None => idle(core),
        }
    }
}

/// What the host observed during one quantum. The task value belongs to the
/// core, so the host cannot change the class or the deadline itself; it records
/// the promotion and the heartbeat count here and the loop applies them at the
/// quantum boundary, when the core still owns the value.
struct Quantum {
    outcome: Outcome,
    promoted: bool,
    heartbeats: u64,
}

/// Advances a claimed task by one quantum.
///
/// The package is re-parsed from the placed code region on every claim: the
/// bytes the task executes are the bytes the verifier checked, and nothing
/// self-referential has to live inside the [`Task`]. Parsing is host-side work
/// and is not charged to the quantum.
fn run_quantum(core: usize, task: &mut Task, image: &Ext2<'_>) -> Quantum {
    let Task {
        id,
        class,
        regions,
        machine,
        args,
        ..
    } = task;
    let code = regions.code.as_slice();
    let program = match parse_program_on(core, code) {
        Ok(program) => program,
        Err(_) => {
            // The loader already validated this package; reaching here means
            // the code region changed under the task. Terminate, do not retry.
            return Quantum {
                outcome: Outcome::Exited(ExitReason::Faulted(FaultCode::IllegalOperation)),
                promoted: false,
                heartbeats: 0,
            };
        }
    };

    let mut host = KernelHost {
        image,
        current: *id,
        class: *class,
        promoted: false,
        heartbeats: 0,
        exit_diag: [0u8; repl::DIAG_BYTES],
        exit_diag_id: 0,
        exit_diag_ready: false,
    };
    let outcome = machine.run(
        &program,
        &mut host,
        regions.state.as_slice_mut(),
        regions.stack.as_slice_mut(),
        &args[..],
        QUANTUM_STEPS,
    );
    Quantum {
        outcome,
        promoted: host.promoted,
        heartbeats: host.heartbeats,
    }
}

/// Reports, records and reclaims a terminated task.
///
/// The exit record goes into the global ring and every task blocked on this id
/// is woken, in that order: the woken parent's retried `core.wait` must find
/// the record. The regions are reclaimed here (Sch10); the id and reason
/// survive in the ring until consumed or evicted.
///
/// A realtime task releases its admission slot exactly once here, before its
/// regions are reclaimed. A block or a release never reaches this function, so
/// a blocked controller keeps its slot (spec 0007, Sch13).
fn terminate(core: usize, mut task: Task, reason: ExitReason) {
    let id = task.id;
    let claims = task.claims;
    let yields = task.yields;
    let heartbeats = task.heartbeats;
    let realtime = task.class == TaskClass::Realtime;
    EXITS[core].fetch_add(1, Ordering::Relaxed);

    if task.admitted {
        task.admitted = false;
        release_realtime();
    }

    match reason {
        ExitReason::Ok if realtime => println!(
            "task {id}: exit Ok after {claims} claims ({yields} yields, realtime, {heartbeats} heartbeats)"
        ),
        ExitReason::Ok => {
            println!("task {id}: exit Ok after {claims} claims ({yields} yields)")
        }
        ExitReason::Killed if realtime => println!(
            "task {id}: exit Killed after {claims} claims ({yields} yields, realtime, {heartbeats} heartbeats)"
        ),
        ExitReason::Killed => {
            println!("task {id}: exit Killed after {claims} claims ({yields} yields)")
        }
        ExitReason::Faulted(code) if realtime => println!(
            "task {id}: exit Faulted({}) after {claims} claims ({yields} yields, realtime, {heartbeats} heartbeats)",
            code.name()
        ),
        ExitReason::Faulted(code) => println!(
            "task {id}: exit Faulted({}) after {claims} claims ({yields} yields)",
            code.name()
        ),
    }

    repl::record_exit(id, reason);
    task::wake(id);
    task.reclaim();
}

/// The REPL service opportunity: at most one pending request per call (spec
/// 0007, "Services and task creation"). It runs before every claim so a
/// permanently runnable machine still services its queue.
///
/// Input is polled here too: with no input the loop `hlt`s, the timer wakes it
/// every millisecond, and this is where a typed line reaches the line ring and
/// wakes a task blocked in `core.read_line`.
fn service(image: &Ext2<'_>) {
    console::poll_input();
    if console::has_line() {
        task::wake_console();
    }
    repl::service(image);
}

/// Nothing is runnable. Wait for the next interrupt and re-check, unless there
/// is no work anywhere, in which case the boot core prints the summary, stops
/// the other cores and halts.
///
/// A blocked task is live, so the system halts only after the last child and
/// parent are gone — never while a wake-up is still possible.
fn idle(core: usize) {
    loop {
        // Announce that this core is idle *before* checking for work: a
        // producer publishes work first and then claims an idle core, so either
        // the check below sees the work, or the producer's kick finds the flag
        // and sends the IPI.
        smp::mark_idle(core);

        if smp::take_kick(core) {
            smp::note_wake(core);
        }

        // Input is work, and this loop is the only place a halted core can
        // notice it: the service opportunity that normally polls COM1/PS-2
        // lives in the main loop, which an idle core does not reach. Polling
        // here is what turns a typed line into a wake-up.
        console::poll_input();

        // Interrupts off for the check; `sti; hlt` is one atomic idle step, so
        // an IPI that arrives after the check is pending when `hlt` runs.
        arch_x86_64::disable_interrupts();
        if smp::shutdown_requested() {
            smp::note_stopped(core);
            arch_x86_64::halt();
        }
        if task::has_runnable() || repl::has_pending() || console::has_line() {
            arch_x86_64::enable_interrupts();
            smp::mark_busy(core);
            return;
        }
        if core == 0 {
            try_shutdown();
        }
        arch_x86_64::wait_for_interrupt();
    }
}

/// The system-wide idle decision: with no live task, no pending request and no
/// console line anywhere, and every other core idle, the boot core stops them,
/// prints the summary and halts.
///
/// Returns only when some work is still able to appear; once it decides to
/// stop the machine it never returns.
fn try_shutdown() {
    if task::live_count() != 0 || repl::has_pending() || console::has_line() {
        return;
    }
    if !smp::all_idle_except(0) {
        return;
    }

    // Waiting for the acknowledgements needs the clock, which advances in the
    // boot core's timer interrupt; the idle loop called this with interrupts
    // off.
    arch_x86_64::enable_interrupts();
    let (stopped, expected, acks) = smp::stop_all();
    for (index, ack) in acks.iter().enumerate().take(smp::MAX_CORES) {
        if index != 0 && smp::is_online(index) && !ack {
            println!("smp: core {index} did not acknowledge the stop request");
        }
    }
    println!("smp: {stopped} of {expected} APs stopped");

    println!("scheduler: idle summary");
    let (mut claims, mut yields, mut exits) = (0u64, 0u64, 0u64);
    for index in 0..smp::MAX_CORES {
        if !smp::is_online(index) {
            continue;
        }
        let (c, y, e) = core_stats(index);
        claims += c;
        yields += y;
        exits += e;
        println!(
            "  core {} (apic {}): {c} claims, {y} yields, {e} exits",
            index,
            smp::apic_id_of(index)
        );
    }
    println!(
        "  total: {claims} claims, {yields} yields, {exits} exits, {} free frames",
        frame::free_frames()
    );
    println!("halt: no console input");
    arch_x86_64::halt();
}

/// One core's loop counters.
fn core_stats(index: usize) -> (u64, u64, u64) {
    (
        CLAIMS[index].load(Ordering::Relaxed),
        YIELDS[index].load(Ordering::Relaxed),
        EXITS[index].load(Ordering::Relaxed),
    )
}

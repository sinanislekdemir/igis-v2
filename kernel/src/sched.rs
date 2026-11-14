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
//! # Interrupts
//!
//! Interrupts stay enabled: the timer tick only advances the global clock and
//! acknowledges the APIC, and it never claims a task or takes a scheduler lock,
//! so it cannot re-enter the loop. Claim and release therefore only need the
//! slot spin lock, not `cli`. No lock is held across [`Machine::run`]; the
//! verifier scratch lock is dropped after parsing, and the console lock is
//! taken and released inside the host's `console_write`.

use core::sync::atomic::{AtomicU64, Ordering};

use abi::{ExitReason, FaultCode, Status};
use interpreter::{Diagnostic, Host, Outcome, Program, WaitKind, WaitOutcome};

use crate::clock;
use crate::console;
use crate::ext2::Ext2;
use crate::frame;
use crate::println;
use crate::repl;
use crate::sync::SpinLock;
use crate::task::{self, Task, WaitFor};

/// Bytecodes executed per claim (spec 0007 Q1, provisional).
///
/// Step count is the primary quantum: it is independent of host speed and of
/// the coarse 1 ms tick. Tune after measurement.
pub const QUANTUM_STEPS: u64 = 10_000;

/// Total CPU time a task may consume before it is terminated (spec 0007 Q2,
/// provisional). Blocked time is not counted; a task that never reaches a safe
/// point cannot be killed (accepted cooperative liveness hole).
pub const TOTAL_BUDGET_NS: u64 = 10_000_000_000;

/// Hard ceiling on total bytecodes per task.
///
/// The 1 ms clock cannot resolve sub-millisecond quanta, so most claims add
/// 0 ns to [`TOTAL_BUDGET_NS`]. This cap is the watchdog that actually fires
/// in the first milestone; 200 000 000 steps is the same order of magnitude as
/// ten seconds at interpreter speed.
pub const TOTAL_BUDGET_STEPS: u64 = 200_000_000;

/// Verifier scratch: one bit per 4-byte instruction word. Shared by the loader
/// (initial validation) and the loop (re-parse per quantum). A lock is enough
/// for one core; SMP needs per-core scratch (Sch6).
const VERIFY_BYTES: usize = compiler::MAX_CODE_BYTES / 4 / 8;
static VERIFY: SpinLock<[u8; VERIFY_BYTES]> = SpinLock::new([0; VERIFY_BYTES]);

// Loop statistics; the summary prints them when the system goes idle.
static CLAIMS: AtomicU64 = AtomicU64::new(0);
static YIELDS: AtomicU64 = AtomicU64::new(0);
static EXITS: AtomicU64 = AtomicU64::new(0);

/// The kernel's implementation of the interpreter host interface (spec 0005).
///
/// `current` is the id of the task being advanced, so `core.wait` on oneself is
/// `Invalid` rather than a permanent block. `image` is the read-only ext2
/// mount the REPL service compiles from.
struct KernelHost<'a> {
    image: &'a Ext2<'a>,
    current: u64,
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
    let mut verify = VERIFY.lock();
    Program::parse(code, &mut verify[..])
}

/// The scheduler loop. Never returns.
///
/// `image` is the read-only ext2 mount the service compiles requests from.
/// The loop ends when no task is live and no request is pending: with no
/// console input there is no other source of work, so it prints the summary and
/// halts. A blocked task keeps the system alive (it will be woken by a child
/// termination or a finished console line), so the check is "no live task", not
/// "boot task finished".
pub fn run_forever(image: &Ext2<'_>) -> ! {
    loop {
        service(image);

        match task::claim() {
            Some(mut task) => {
                let started = clock::now_ns();
                let outcome = run_quantum(&mut task, image);
                let elapsed = clock::now_ns().saturating_sub(started);

                task.budget_ns = task.budget_ns.saturating_add(elapsed);
                task.claims += 1;
                CLAIMS.fetch_add(1, Ordering::Relaxed);

                match outcome {
                    Outcome::Exited(reason) => terminate(task, reason),
                    Outcome::Blocked(kind) => {
                        if task.kill_requested {
                            // A kill that arrived while the task was blocked
                            // must not be lost by blocking it again.
                            terminate(task, ExitReason::Killed);
                            continue;
                        }
                        // The blocked task keeps its slot but stops being
                        // claimable. A child termination wakes `ChildExit`; a
                        // finished console line wakes `Console` (Sch9).
                        match kind {
                            WaitKind::ChildExit(child) => {
                                println!("sched: task {} blocked on task {}", task.id, child);
                                task::block(task, WaitFor::Child(child));
                            }
                            WaitKind::Console => {
                                println!("sched: task {} blocked on console", task.id);
                                task::block(task, WaitFor::Console);
                            }
                        }
                    }
                    Outcome::Yielded => {
                        if task.kill_requested {
                            // `core.kill` marks the task and requests a yield;
                            // the next boundary is where it dies (spec 0007).
                            terminate(task, ExitReason::Killed);
                            continue;
                        }
                        let expired = task.budget_ns >= TOTAL_BUDGET_NS
                            || task.machine.steps() >= TOTAL_BUDGET_STEPS;
                        if expired {
                            // Sch8: unconditional, no `fault` handler in ABI v1.
                            terminate(task, ExitReason::Faulted(FaultCode::BudgetExceeded));
                        } else {
                            task.yields += 1;
                            YIELDS.fetch_add(1, Ordering::Relaxed);
                            task::release(task);
                        }
                    }
                }
            }
            None => idle(),
        }
    }
}

/// Advances a claimed task by one quantum.
///
/// The package is re-parsed from the placed code region on every claim: the
/// bytes the task executes are the bytes the verifier checked, and nothing
/// self-referential has to live inside the [`Task`]. Parsing is host-side work
/// and is not charged to the quantum.
fn run_quantum(task: &mut Task, image: &Ext2<'_>) -> Outcome {
    let Task {
        id,
        regions,
        machine,
        args,
        ..
    } = task;
    let code = regions.code.as_slice();
    let program = match parse_program(code) {
        Ok(program) => program,
        Err(_) => {
            // The loader already validated this package; reaching here means
            // the code region changed under the task. Terminate, do not retry.
            return Outcome::Exited(ExitReason::Faulted(FaultCode::IllegalOperation));
        }
    };

    let mut host = KernelHost {
        image,
        current: *id,
        exit_diag: [0u8; repl::DIAG_BYTES],
        exit_diag_id: 0,
        exit_diag_ready: false,
    };
    machine.run(
        &program,
        &mut host,
        regions.state.as_slice_mut(),
        regions.stack.as_slice_mut(),
        &args[..],
        QUANTUM_STEPS,
    )
}

/// Reports, records and reclaims a terminated task.
///
/// The exit record goes into the global ring and every task blocked on this id
/// is woken, in that order: the woken parent's retried `core.wait` must find
/// the record. The regions are reclaimed here (Sch10); the id and reason
/// survive in the ring until consumed or evicted.
fn terminate(task: Task, reason: ExitReason) {
    let id = task.id;
    let claims = task.claims;
    let yields = task.yields;
    EXITS.fetch_add(1, Ordering::Relaxed);

    match reason {
        ExitReason::Ok => {
            println!("task {id}: exit Ok after {claims} claims ({yields} yields)")
        }
        ExitReason::Killed => {
            println!("task {id}: exit Killed after {claims} claims ({yields} yields)")
        }
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
/// is no live task and no pending request, in which case print the summary and
/// halt.
///
/// A blocked task is live, so the system halts only after the last child and
/// parent are gone — never while a wake-up is still possible.
fn idle() {
    if task::live_count() == 0 && !repl::has_pending() {
        println!(
            "scheduler: idle: {} claims, {} yields, {} exits, {} free frames",
            CLAIMS.load(Ordering::Relaxed),
            YIELDS.load(Ordering::Relaxed),
            EXITS.load(Ordering::Relaxed),
            frame::free_frames()
        );
        println!("halt: no console input");
        arch_x86_64::halt();
    }

    // `sti; hlt`: the timer keeps the clock running, and a wake-up (a service
    // request or a child termination) re-checks the set after this returns.
    arch_x86_64::wait_for_interrupt();
}

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

use abi::{ExitReason, FaultCode};
use interpreter::{Diagnostic, Host, Outcome, Program};

use crate::clock;
use crate::frame;
use crate::println;
use crate::sync::SpinLock;
use crate::task::{self, Task};

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
struct KernelHost;

impl Host for KernelHost {
    fn console_write(&mut self, bytes: &[u8]) {
        crate::console::write_bytes(bytes);
    }

    fn clock(&mut self) -> u64 {
        clock::now_ns()
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
/// `boot_task` is the id of the application the REPL enqueued at boot; once it
/// has terminated there is no other source of work, so the loop prints the
/// summary and halts.
pub fn run_forever(boot_task: u64) -> ! {
    let mut boot_done = false;

    loop {
        service();

        match task::claim() {
            Some(mut task) => {
                let started = clock::now_ns();
                let outcome = run_quantum(&mut task);
                let elapsed = clock::now_ns().saturating_sub(started);

                task.budget_ns = task.budget_ns.saturating_add(elapsed);
                task.claims += 1;
                CLAIMS.fetch_add(1, Ordering::Relaxed);

                match outcome {
                    Outcome::Exited(reason) => {
                        boot_done |= task.id == boot_task;
                        terminate(task, reason);
                    }
                    Outcome::Yielded => {
                        let expired = task.budget_ns >= TOTAL_BUDGET_NS
                            || task.machine.steps() >= TOTAL_BUDGET_STEPS;
                        if expired {
                            // Sch8: unconditional, no `fault` handler in ABI v1.
                            boot_done |= task.id == boot_task;
                            terminate(task, ExitReason::Faulted(FaultCode::BudgetExceeded));
                        } else {
                            task.yields += 1;
                            YIELDS.fetch_add(1, Ordering::Relaxed);
                            task::release(task);
                        }
                    }
                }
            }
            None => idle(boot_done),
        }
    }
}

/// Advances a claimed task by one quantum.
///
/// The package is re-parsed from the placed code region on every claim: the
/// bytes the task executes are the bytes the verifier checked, and nothing
/// self-referential has to live inside the [`Task`]. Parsing is host-side work
/// and is not charged to the quantum.
fn run_quantum(task: &mut Task) -> Outcome {
    let Task {
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

    let mut host = KernelHost;
    machine.run(
        &program,
        &mut host,
        regions.state.as_slice_mut(),
        regions.stack.as_slice_mut(),
        &args[..],
        QUANTUM_STEPS,
    )
}

/// Reports and reclaims a terminated task. It is not in the runnable set: this
/// core owns it (Sch10).
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

    task.reclaim();
}

/// A stub for the REPL service dispatcher (spec 0007, "Services and task
/// creation"). There is no request queue yet: `repl::boot` enqueues the boot
/// task before the loop starts, and the dispatcher runs before every claim.
fn service() {}

/// Nothing is runnable. Wait for the next interrupt and re-check, unless the
/// boot work is finished, in which case print the summary and halt.
fn idle(boot_done: bool) {
    if boot_done {
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

    // `sti; hlt`: the timer keeps the clock running, and any wake-up (a future
    // service request or stream event) re-checks the set after this returns.
    arch_x86_64::wait_for_interrupt();
}

//! Task records and the global runnable set (spec 0007).
//!
//! A task is a value: its regions plus its interpreter state. The runnable set
//! is a fixed table of [`MAX_TASKS`] slots, each guarded by a spin lock.
//! [`claim`] takes the [`Task`] out of its slot; while a core holds the value
//! it is the only owner and the task is invisible to every other core. [`release`]
//! puts the same value back into the slot it came from (Sch2, Sch3).
//!
//! The take-under-lock *is* the ownership transfer, so there is no separate
//! state machine and no lock held while the task runs. On SMP the scan can be
//! replaced by per-slot atomics or per-core queues without changing the
//! protocol (spec 0007 Q2); the scan is deterministic but not FIFO, which is
//! acceptable for the single-core milestone (Sch12 is a SHOULD).
//!
//! Every task owns its regions, its `Machine` and its argument buffer, so the
//! record can be executed by any core (spec 0004, C8). Region allocation is the
//! only dynamic step and it happens before the task enters the set.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use abi::Status;
use interpreter::Machine;

use crate::frame;
use crate::println;
use crate::sync::SpinLock;

/// Maximum tasks in the runnable set (and thus alive at once). Fixed: the
/// kernel has no allocator.
pub const MAX_TASKS: usize = 64;

/// Argument buffer size: `[len: u64][data: 1024]` (spec 0008, "Values in the
/// VM"). Delivered to `main(args: const string[1024])`.
pub const ARGS_BYTES: usize = 1024;

/// Task ids are global, monotonic and never reused, starting at 1 (ADR 0017).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Reserves a fresh global task id. `spawn` reserves one before the request is
/// queued, so the caller can be handed the id immediately (spec 0007).
pub fn reserve_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Whether a task may be claimed.
///
/// A `Blocked` task stays in its slot (so its regions and machine are never
/// lost) but [`claim`] skips it; the wake-up path flips it back to `Runnable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// In the runnable set; any core may claim it.
    Runnable,
    /// Waiting for a condition; not claimable.
    Blocked,
}

/// What a blocked task is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitFor {
    /// The child task with this global id has terminated (`core.wait`).
    Child(u64),
    /// One finished console line (`core.read_line`).
    Console,
}

/// The scheduling class of a task (spec 0007, ADR 0016).
///
/// Promotion is one-way: a task starts `Normal` and becomes `Realtime` at the
/// end of the quantum in which `core.realtime_on()` succeeded. The class is
/// part of the task value, so it travels with the task across cores; it is not
/// an affinity (a realtime task is still migratable at release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskClass {
    /// Fair, preemptive quantum and the total CPU budget.
    Normal,
    /// Claimed first, not released at quantum expiry, exempt from the total
    /// budget, and bound by the one-second heartbeat deadline.
    Realtime,
}

/// One physically contiguous task region.
#[derive(Clone, Copy)]
pub struct Region {
    pub start: u64,
    pub frames: usize,
    pub len: usize,
}

impl Region {
    /// Allocates `len` bytes (at least one frame, so the slice is never empty).
    fn allocate(len: usize) -> Result<Self, Status> {
        let len = len.max(1);
        let frames = (len + frame::FRAME_SIZE as usize - 1) / frame::FRAME_SIZE as usize;
        let start = frame::alloc_contiguous(frames).ok_or(Status::Full)?;
        Ok(Region { start, frames, len })
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `start`/`len` describe frames reserved by the frame allocator
        // and identity-mapped by the boot page tables. `&self` allows shared
        // access only; the region is live for as long as the task exists.
        unsafe { core::slice::from_raw_parts(self.start as *const u8, self.len) }
    }

    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        // SAFETY: as above, and `&mut self` gives exclusive access to the
        // region (the task's owner is the only writer, spec 0004 C8/C9).
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u8, self.len) }
    }

    fn free(self) {
        frame::free_contiguous(self.start, self.frames);
    }
}

/// The regions a task owns (spec 0004). The `fpu` region arrives with the
/// floating-point work; ABI v1 packages do not declare one yet.
pub struct Regions {
    pub code: Region,
    pub state: Region,
    pub stack: Region,
}

/// A runnable or running application instance.
///
/// The value moves between the runnable set and the core that claimed it; no
/// application state ever stays in a core's registers across a release
/// (Sch5), because the state lives in `regions` and the `Machine`.
pub struct Task {
    /// Global, monotonic, never reused; reserved before the task is placed.
    pub id: u64,
    /// Slot the task is (or will be) enqueued in; set by [`spawn`].
    pub slot: usize,
    /// Runnable or blocked; a blocked task stays in its slot but is not claimed.
    pub state: TaskState,
    /// The condition this task is blocked on, if any.
    pub waiting: Option<WaitFor>,
    /// Scheduling class (spec 0007, ADR 0016). Set once, at the end of the
    /// quantum in which `core.realtime_on()` returned `Ok`.
    pub class: TaskClass,
    /// This task holds one of the global realtime admission slots. Released
    /// exactly once, by [`crate::sched`] in `terminate`, never on block or
    /// release, so a blocked realtime task keeps its slot.
    pub admitted: bool,
    /// Accumulated non-blocked time at which the next heartbeat is due
    /// (`budget_ns + HEARTBEAT_NS` at promotion and after every heartbeat).
    pub heartbeat_deadline_ns: u64,
    /// Heartbeats observed so far, printed at termination.
    pub heartbeats: u64,
    /// `core.kill` arrived; terminate at the next quantum boundary.
    pub kill_requested: bool,
    /// Claims so far, printed at termination.
    pub claims: u64,
    /// Releases so far (quantum expiry), printed at termination.
    pub yields: u64,
    /// Accumulated time between claim and release, in nanoseconds.
    pub budget_ns: u64,
    /// Fixed `string[1024]` argument (spec 0007, "Services and task creation").
    pub args: [u8; 8 + ARGS_BYTES],
    pub regions: Regions,
    pub machine: Machine,
}

impl Task {
    /// Reserves and zeroes the task's regions under a caller-reserved id.
    ///
    /// Nothing is enqueued here: the caller fills `code` and then calls
    /// [`spawn`]. The id is reserved before a `spawn` request is queued, so it
    /// is passed in rather than assigned here.
    pub fn place(
        code_len: usize,
        state_len: usize,
        stack_len: usize,
        id: u64,
    ) -> Result<Self, Status> {
        let code = Region::allocate(code_len)?;
        let state = match Region::allocate(state_len) {
            Ok(state) => state,
            Err(status) => {
                code.free();
                return Err(status);
            }
        };
        let stack = match Region::allocate(stack_len) {
            Ok(stack) => stack,
            Err(status) => {
                code.free();
                state.free();
                return Err(status);
            }
        };

        let mut task = Task {
            id,
            slot: usize::MAX,
            state: TaskState::Runnable,
            waiting: None,
            class: TaskClass::Normal,
            admitted: false,
            heartbeat_deadline_ns: 0,
            heartbeats: 0,
            kill_requested: false,
            claims: 0,
            yields: 0,
            budget_ns: 0,
            args: [0; 8 + ARGS_BYTES],
            regions: Regions { code, state, stack },
            machine: Machine::new(),
        };
        task.regions.code.as_slice_mut().fill(0);
        task.regions.state.as_slice_mut().fill(0);
        task.regions.stack.as_slice_mut().fill(0);
        Ok(task)
    }

    /// Frees every region (Sch10) and releases the slot. Called once, when the
    /// task terminates or cannot be enqueued.
    ///
    /// The realtime admission slot is *not* released here: termination does it
    /// before calling this, so a task that is reclaimed without ever being
    /// admitted (a failed spawn, a stale slot) cannot double-release.
    pub fn reclaim(self) {
        self.regions.code.free();
        self.regions.state.free();
        self.regions.stack.free();
        if let Some(slot) = SLOT_ID.get(self.slot) {
            // The slot id is cleared only after the regions are back with the
            // allocator, so a slot is never reused while its task still owns
            // memory. A stale kill request must not reach the next tenant.
            if let Some(kill) = KILL_FLAGS.get(self.slot) {
                kill.store(false, Ordering::Release);
            }
            slot.store(0, Ordering::Release);
        }
    }
}

/// The global runnable set. One slot per possible live task.
static SLOTS: [SpinLock<Option<Task>>; MAX_TASKS] = [const { SpinLock::new(None) }; MAX_TASKS];

/// The task id assigned to each slot, or 0 when the slot is free.
///
/// A slot is reserved for one task from [`spawn`] until [`Task::reclaim`]. The
/// value moves out of the slot while the task runs, so the slot itself is empty
/// then; without this reservation a concurrent `spawn` on another core could
/// hand that slot to a child, and the release would overwrite the child. The
/// bookkeeping also makes a *running* task visible to [`is_live`], which
/// `core.wait` needs (a running child is alive).
static SLOT_ID: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// Pending `core.kill` requests, one per slot.
///
/// A kill for a task that is *running* on another core cannot reach the task
/// value (it lives in that core's registers and stack), so the request parks
/// here and the owning core terminates the task at its next quantum boundary
/// (Sch8's safe-point rule, with the boundary as the safe point).
static KILL_FLAGS: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];

/// Enqueues `task` in the first free slot (Sch2).
///
/// The caller must have created the record with [`Task::place`]. On
/// `Status::Full` the task is reclaimed here: the caller cannot reach it any
/// more, and leaking its frames would break the deterministic-reclaim rule.
pub fn spawn(task: Task) -> Result<(), Status> {
    let mut task = task;
    task.state = TaskState::Runnable;
    task.waiting = None;
    let id = task.id;
    for (index, slot) in SLOTS.iter().enumerate() {
        // Reserve the slot for this task first; a slot whose task is running
        // elsewhere is not empty but is still reserved, so the CAS fails.
        if SLOT_ID[index]
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            continue;
        }
        task.slot = index;
        let mut guard = slot.lock();
        *guard = Some(task);
        drop(guard);
        // New work in the shared set: wake one idle core so a `-smp N` run
        // spreads the task instead of leaving it to the enqueuing core.
        crate::smp::kick_one();
        return Ok(());
    }
    task.reclaim();
    Err(Status::Full)
}

/// True when at least one slot holds a runnable task.
///
/// The scheduler's idle decision needs this: with no runnable task, no live
/// blocked task and no pending request, no core has work.
pub fn has_runnable() -> bool {
    SLOTS.iter().any(|slot| {
        slot.lock()
            .as_ref()
            .is_some_and(|task| task.state == TaskState::Runnable)
    })
}

/// Claims one runnable task, or `None` when no slot is runnable.
///
/// The `take` under the slot lock is the exclusive ownership transfer: the
/// task is removed from the set before the value is returned. No lock is held
/// after this returns. A `Blocked` task stays in its slot and is skipped
/// (Sch9); the wake-up path marks it `Runnable` again.
///
/// Realtime tasks are claimed before normal ones (Sch14): one full scan for
/// each class. The class is read from the task value, so a task that was
/// promoted while running is already claimable as realtime after its release.
pub fn claim() -> Option<Task> {
    claim_class(TaskClass::Realtime).or_else(|| claim_class(TaskClass::Normal))
}

/// Claims a runnable task of one class, scanning the whole table.
fn claim_class(class: TaskClass) -> Option<Task> {
    for slot in SLOTS.iter() {
        let mut guard = slot.lock();
        let wanted = guard
            .as_ref()
            .is_some_and(|task| task.state == TaskState::Runnable && task.class == class);
        if wanted {
            return guard.take();
        }
    }
    None
}

/// Puts a claimed task back into its slot after a blocking builtin.
///
/// The record stays where it is (regions and machine included) but is no
/// longer claimable until the matching wake-up clears the condition.
pub fn block(task: Task, wait: WaitFor) {
    let mut task = task;
    task.state = TaskState::Blocked;
    task.waiting = Some(wait);
    put(task);
}

/// Makes every task blocked on `child` runnable again.
///
/// The task is mutated in place under its slot lock: it never left the slot,
/// so no value move or fresh claim is needed.
pub fn wake(child: u64) {
    wake_matching(|wait| wait == WaitFor::Child(child));
}

/// Makes every task blocked on the console runnable again.
pub fn wake_console() {
    wake_matching(|wait| wait == WaitFor::Console);
}

fn wake_matching(matches: impl Fn(WaitFor) -> bool) {
    let mut woken = false;
    for slot in SLOTS.iter() {
        let mut guard = slot.lock();
        if let Some(task) = guard.as_mut() {
            if task.state == TaskState::Blocked && task.waiting.is_some_and(|wait| matches(wait)) {
                task.waiting = None;
                task.state = TaskState::Runnable;
                woken = true;
            }
        }
    }
    if woken {
        // A wake on this core must not depend on another idle core noticing:
        // kick one so the woken task can be claimed wherever it is cheapest.
        crate::smp::kick_one();
    }
}

/// Requests termination of `id` at its next safe point (`core.kill`).
///
/// A blocked task is made runnable as well, so the request cannot sit behind a
/// wait that will never complete. A task another core has claimed cannot be
/// reached (its value is not in the table), so the request is recorded in
/// [`KILL_FLAGS`] and the owning core terminates it at its next quantum
/// boundary. The task is not reclaimed here.
pub fn request_kill(id: u64) -> Status {
    for (index, slot) in SLOTS.iter().enumerate() {
        if SLOT_ID[index].load(Ordering::Acquire) != id {
            continue;
        }
        // Recorded for a task that is running elsewhere; the scheduler checks
        // it after every quantum.
        KILL_FLAGS[index].store(true, Ordering::SeqCst);
        let mut guard = slot.lock();
        let Some(task) = guard.as_mut() else {
            // Claimed by another core; the flag above is the whole request.
            return Status::Ok;
        };
        task.kill_requested = true;
        task.machine.request_yield();
        if task.state == TaskState::Blocked {
            task.waiting = None;
            task.state = TaskState::Runnable;
            drop(guard);
            crate::smp::kick_one();
        }
        return Status::Ok;
    }
    Status::NotFound
}

/// Takes a pending kill request for the task in `slot` (set by [`request_kill`]
/// while the task was claimed by another core).
pub fn take_kill(slot: usize) -> bool {
    KILL_FLAGS
        .get(slot)
        .is_some_and(|flag| flag.swap(false, Ordering::SeqCst))
}

/// True when `id` names a live task (runnable, blocked or running) in a
/// reserved slot.
///
/// A `spawn` request reserves its id before the task exists; the request
/// queue is the other half of "live" and is checked by the REPL service.
pub fn is_live(id: u64) -> bool {
    id != 0
        && SLOT_ID
            .iter()
            .any(|slot| slot.load(Ordering::Acquire) == id)
}

/// Number of live tasks (runnable, blocked or running).
pub fn live_count() -> usize {
    SLOT_ID
        .iter()
        .filter(|slot| slot.load(Ordering::Acquire) != 0)
        .count()
}

/// Releases a claimed task back into its recorded slot (Sch7).
pub fn release(task: Task) {
    let mut task = task;
    task.state = TaskState::Runnable;
    task.waiting = None;
    put(task);
    // The released task may be claimed by any core; tell an idle one so work
    // migrates instead of piling up behind the core that just released it.
    crate::smp::kick_one();
}

/// Places a claimed task value into its reserved slot.
fn put(task: Task) {
    let index = task.slot;
    let Some(slot) = SLOTS.get(index) else {
        println!("sched: task {} released before it was enqueued", task.id);
        task.reclaim();
        arch_x86_64::halt();
    };
    let mut guard = slot.lock();
    if let Some(stale) = guard.replace(task) {
        drop(guard);
        let stale_id = stale.id;
        stale.reclaim();
        println!("sched: two tasks in slot {index} (task {stale_id} evicted)");
        arch_x86_64::halt();
    }
}

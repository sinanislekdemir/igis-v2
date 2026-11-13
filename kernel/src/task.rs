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

use core::sync::atomic::{AtomicU64, Ordering};

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

    /// Frees every region (Sch10). Called once, when the task terminates or
    /// cannot be enqueued.
    pub fn reclaim(self) {
        self.regions.code.free();
        self.regions.state.free();
        self.regions.stack.free();
    }
}

/// The global runnable set. One slot per possible live task.
static SLOTS: [SpinLock<Option<Task>>; MAX_TASKS] = [const { SpinLock::new(None) }; MAX_TASKS];

/// Enqueues `task` in the first free slot (Sch2).
///
/// The caller must have created the record with [`Task::place`]. On
/// `Status::Full` the task is reclaimed here: the caller cannot reach it any
/// more, and leaking its frames would break the deterministic-reclaim rule.
pub fn spawn(task: Task) -> Result<(), Status> {
    let mut task = task;
    task.state = TaskState::Runnable;
    task.waiting = None;
    for (index, slot) in SLOTS.iter().enumerate() {
        let mut guard = slot.lock();
        if guard.is_none() {
            task.slot = index;
            *guard = Some(task);
            return Ok(());
        }
    }
    task.reclaim();
    Err(Status::Full)
}

/// Claims one runnable task, or `None` when no slot is runnable.
///
/// The `take` under the slot lock is the exclusive ownership transfer: the
/// task is removed from the set before the value is returned. No lock is held
/// after this returns. A `Blocked` task stays in its slot and is skipped
/// (Sch9); the wake-up path marks it `Runnable` again.
pub fn claim() -> Option<Task> {
    for slot in SLOTS.iter() {
        let mut guard = slot.lock();
        let runnable = guard
            .as_ref()
            .is_some_and(|task| task.state == TaskState::Runnable);
        if runnable {
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
    for slot in SLOTS.iter() {
        let mut guard = slot.lock();
        if let Some(task) = guard.as_mut() {
            if task.state == TaskState::Blocked && task.waiting.is_some_and(|wait| matches(wait)) {
                task.waiting = None;
                task.state = TaskState::Runnable;
            }
        }
    }
}

/// Requests termination of `id` at its next safe point (`core.kill`).
///
/// A blocked task is made runnable as well, so the request cannot sit behind a
/// wait that will never complete. The task is not reclaimed here; the
/// scheduler sees the flag at the next quantum boundary.
pub fn request_kill(id: u64) -> Status {
    for slot in SLOTS.iter() {
        let mut guard = slot.lock();
        if let Some(task) = guard.as_mut() {
            if task.id == id {
                task.kill_requested = true;
                task.machine.request_yield();
                if task.state == TaskState::Blocked {
                    task.waiting = None;
                    task.state = TaskState::Runnable;
                }
                return Status::Ok;
            }
        }
    }
    Status::NotFound
}

/// True when `id` names a live task (runnable or blocked) in a slot.
///
/// A `spawn` request reserves its id before the task exists; the request
/// queue is the other half of "live" and is checked by the REPL service.
pub fn is_live(id: u64) -> bool {
    SLOTS
        .iter()
        .any(|slot| slot.lock().as_ref().is_some_and(|task| task.id == id))
}

/// Number of live tasks (runnable or blocked).
pub fn live_count() -> usize {
    SLOTS.iter().filter(|slot| slot.lock().is_some()).count()
}

/// Releases a claimed task back into its recorded slot (Sch7).
pub fn release(task: Task) {
    let mut task = task;
    task.state = TaskState::Runnable;
    task.waiting = None;
    put(task);
}

/// Places a claimed task value into its recorded slot.
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

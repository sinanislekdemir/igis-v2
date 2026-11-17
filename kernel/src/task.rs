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
//! protocol (spec 0007 Q2). The scan starts at a rotating cursor (Sch12): a
//! busy task in a low slot must not starve the higher slots on a single core.
//!
//! Every task owns its regions, its `Machine` and its argument buffer, so the
//! record can be executed by any core (spec 0004, C8). Region allocation is the
//! only dynamic step and it happens before the task enters the set.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use abi::wait::{WaitSet, SOURCE_CONSOLE, SOURCE_INPUT};
use abi::Status;
use interpreter::Machine;

use crate::frame;
use crate::guard;
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
    /// Space, data or `Closed` on the stream with this endpoint handle
    /// (`core.send`/`core.recv`).
    Stream(u64),
    /// One event in the machine-wide input queue (`syslib.next_event`).
    Input,
    /// Any source in the set (`core.next_any`, spec 0007; Q39). One condition
    /// carries the whole set so both the interpreter and the scheduler see the
    /// same value.
    Any(WaitSet),
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
    /// First usable byte. For a guarded stack this is one frame above `base`.
    pub start: u64,
    /// Frames the region itself uses (the guard frame is not counted).
    pub frames: usize,
    /// Declared length in bytes, exactly the manifest size.
    pub len: usize,
    /// Allocation base and total frames, so `free` returns guard + region as
    /// one unit (spec 0007). Equal to `start`/`frames` without a guard.
    base: u64,
    span: usize,
}

impl Region {
    /// Allocates `len` bytes (at least one frame, so the slice is never empty).
    fn allocate(len: usize) -> Result<Self, Status> {
        let len = len.max(1);
        let frame_size = frame::FRAME_SIZE as usize;
        let frames = len / frame_size + if len % frame_size == 0 { 0 } else { 1 };
        let start = frame::alloc_contiguous(frames).ok_or(Status::Full)?;
        Ok(Region {
            start,
            frames,
            len,
            base: start,
            span: frames,
        })
    }

    /// Allocates `len` usable bytes plus one not-present guard frame below them
    /// (spec 0004 C6, ADR 0006).
    ///
    /// The lowest frame is unmapped before the task can be claimed; `start` and
    /// `len` describe only the stack above it, so the interpreter's bounds
    /// checks and its `StackOverflow` fault are unchanged and never rely on the
    /// hardware guard. The guard is restored in [`Region::free`] *before* the
    /// frames are returned to the allocator: a frame left unmapped would fault
    /// on the first touch of its next life.
    ///
    /// The allocation is `frames + 1` contiguous frames; failing to find that
    /// run is `Status::Full`, as for any other region.
    ///
    /// The constructed fields are checked against [`Region::layout_ok`] before
    /// the allocation is returned. A geometry bug is therefore a recoverable
    /// placement failure (`Status::Invalid`) instead of a fault at a task guard
    /// much later (Q47). The guard is restored and the frames are freed on the
    /// way out; only a failure to restore the guard halts (the existing kernel
    /// bug path in [`Region::free`]).
    fn allocate_guarded(len: usize, task: u64) -> Result<Self, Status> {
        let len = len.max(1);
        let frame_size = frame::FRAME_SIZE as usize;
        let frames = len / frame_size + if len % frame_size == 0 { 0 } else { 1 };
        let Some(span) = frames.checked_add(1) else {
            return Err(Status::Invalid);
        };
        let base = frame::alloc_contiguous(span).ok_or(Status::Full)?;
        let Some(start) = base.checked_add(frame::FRAME_SIZE) else {
            frame::free_contiguous(base, span);
            return Err(Status::Invalid);
        };
        if !guard::guard(base, guard::GuardKind::Task, task) {
            frame::free_contiguous(base, span);
            return Err(Status::Full);
        }
        // Read the entry back: a stack whose guard silently stayed present is
        // worse than a placement failure. The page above the guard is the
        // stack's first frame, which must still be mapped.
        if !guard::check(base) {
            guard::unguard(base);
            frame::free_contiguous(base, span);
            return Err(Status::Full);
        }
        let region = Region {
            start,
            frames,
            len,
            base,
            span,
        };
        if !region.geometry_ok() {
            println!(
                "stack: guarded region layout invalid (base {:#x}, start {:#x}, frames {}, len {}); rejecting",
                base, start, frames, len
            );
            if !guard::unguard(base) {
                println!(
                    "guard: cannot restore the frame at {:#x} (task stack); halting",
                    base
                );
                arch_x86_64::halt();
            }
            frame::free_contiguous(base, span);
            return Err(Status::Invalid);
        }
        Ok(region)
    }

    /// Pure geometry rule for a guarded stack (spec 0004 C6).
    ///
    /// A guarded allocation is `span = frames + 1` contiguous frames: the
    /// lowest is the not-present guard and the `frames` above it are usable.
    /// `start` is exactly one frame above `base`, and `[start, start + len)`
    /// must lie inside those usable frames, so the region can never overlap the
    /// guard or run past the allocation. Every arithmetic step is checked: a
    /// geometry bug is rejected at placement, not faulted at a task guard much
    /// later (Q47).
    fn layout_ok(base: u64, start: u64, frames: usize, len: usize, span: usize) -> bool {
        if frames == 0 || len == 0 || base % frame::FRAME_SIZE != 0 {
            return false;
        }
        if frames.checked_add(1) != Some(span) {
            return false;
        }
        let Some(guard_end) = base.checked_add(frame::FRAME_SIZE) else {
            return false;
        };
        if start != guard_end {
            return false;
        }
        let Some(capacity) = (frames as u64).checked_mul(frame::FRAME_SIZE) else {
            return false;
        };
        if len as u64 > capacity {
            return false;
        }
        let Some(end) = start.checked_add(len as u64) else {
            return false;
        };
        let Some(allocation_end) = (span as u64)
            .checked_mul(frame::FRAME_SIZE)
            .and_then(|bytes| base.checked_add(bytes))
        else {
            return false;
        };
        start >= guard_end && end <= allocation_end
    }

    /// Whether this region satisfies the guarded-stack layout above.
    fn geometry_ok(&self) -> bool {
        Self::layout_ok(self.base, self.start, self.frames, self.len, self.span)
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

    /// Returns the whole allocation as one unit, restoring a guard first.
    ///
    /// Every destruction path reaches this through [`Task::reclaim`]: normal
    /// exit, kill, fault, a failed `spawn`, a verifier rejection and a stale
    /// slot eviction. A failure to restore the guard is a kernel bug (the frame
    /// would be handed out while still unmapped), so it halts.
    fn free(self) {
        if self.base != self.start && !guard::unguard(self.base) {
            println!(
                "guard: cannot restore the frame at {:#x} (task stack); halting",
                self.base
            );
            arch_x86_64::halt();
        }
        frame::free_contiguous(self.base, self.span);
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
        let stack = match Region::allocate_guarded(stack_len, id) {
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
        // Every path that destroys a task goes through here (normal exit,
        // kill, fault, a failed spawn and a stale-slot eviction), so this is
        // the one place that guarantees no stream endpoint is left behind
        // (spec 0006, S7). Close wakes blocked peers after dropping the
        // stream lock. The task's windows go with it (spec 0011, W8): the
        // window table takes its own lock and never a slot lock, so the order
        // is safe.
        crate::stream::close_task(self.id);
        crate::window::close_task(self.id);
        self.regions.code.free();
        self.regions.state.free();
        self.regions.stack.free();
        if SLOT_ID.get(self.slot).is_none() {
            return;
        }
        // The reservation and the kill record are cleared under the slot lock,
        // and only by the id that owns the reservation. `request_kill` publishes
        // its record under the same lock, so a record can never outlive the id
        // it names: either the killer stores it before this clear (and it is
        // removed here) or the killer's re-check sees the cleared reservation
        // and stores nothing. The slot is never reused while its task still
        // owns memory because the id is cleared only after the regions are back
        // with the allocator.
        let slot = SLOTS[self.slot].lock();
        if SLOT_ID[self.slot].load(Ordering::Acquire) == self.id {
            let _ = KILL_ID[self.slot].compare_exchange(
                self.id,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            SLOT_ID[self.slot].store(0, Ordering::Release);
        }
        drop(slot);
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

/// Pending `core.kill` request per slot, tagged with the target task id
/// (`0` = none).
///
/// A kill for a task that is *running* on another core cannot reach the task
/// value (it lives in that core's registers and stack), so the request parks
/// here and the owning core terminates the task at its next quantum boundary
/// (Sch8's safe-point rule, with the boundary as the safe point).
///
/// The record carries the id, not just a boolean: task ids are monotonic and
/// never reused, so a request that is still parked when its task terminates can
/// never match the slot's next tenant. Publishing and clearing happen under the
/// owning slot's lock, which makes the check-and-store atomic with respect to
/// `spawn`, `reclaim` and `block`.
static KILL_ID: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// Rotating start of the claim scan (Sch12).
///
/// After a successful claim the cursor moves to the next slot, so on a single
/// core the scan visits slots 0, 1, 2, ... and wraps instead of always
/// preferring a low slot. A busy task in a low slot therefore cannot starve
/// the higher slots. A global atomic is enough because the claim already takes
/// a slot lock; on SMP the races make the rotation approximate, never unfair
/// in the sense of a permanently skipped slot.
static NEXT_CLAIM: AtomicUsize = AtomicUsize::new(0);

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

/// Claims a runnable task of one class, scanning the whole table from the
/// rotating cursor (Sch12).
fn claim_class(class: TaskClass) -> Option<Task> {
    let start = NEXT_CLAIM.load(Ordering::Relaxed) % MAX_TASKS;
    for offset in 0..MAX_TASKS {
        let index = (start + offset) % MAX_TASKS;
        let mut guard = SLOTS[index].lock();
        let wanted = guard
            .as_ref()
            .is_some_and(|task| task.state == TaskState::Runnable && task.class == class);
        if wanted {
            let task = guard.take();
            NEXT_CLAIM.store((index + 1) % MAX_TASKS, Ordering::Relaxed);
            return task;
        }
    }
    None
}

/// Puts a claimed task back into its slot after a blocking builtin.
///
/// The record stays where it is (regions and machine included) but is no
/// longer claimable until the matching wake-up clears the condition.
///
/// The kill record is re-checked *under the slot lock*, because the scheduler's
/// quantum-boundary check happens before this call: a `core.kill` published in
/// between (the owning core still had the task value, so the killer found the
/// slot empty) would otherwise leave the task blocked with no runnable owner
/// and `core.wait` would hang. A matching id-tagged request is consumed, the
/// task stays `Runnable` with `kill_requested` set, and the scheduler
/// terminates it at the next boundary. This closes the race for every wait
/// kind, since all of them go through this function.
pub fn block(task: Task, wait: WaitFor) {
    let mut task = task;
    task.state = TaskState::Blocked;
    task.waiting = Some(wait);
    let index = task.slot;
    let Some(slot) = SLOTS.get(index) else {
        println!("sched: task {} blocked before it was enqueued", task.id);
        task.reclaim();
        arch_x86_64::halt();
    };
    let mut guard = slot.lock();
    let killed = take_record(&KILL_ID[index], task.id);
    if killed {
        task.kill_requested = true;
        task.state = TaskState::Runnable;
        task.waiting = None;
    }
    if let Some(stale) = guard.replace(task) {
        drop(guard);
        let stale_id = stale.id;
        stale.reclaim();
        println!("sched: two tasks in slot {index} (task {stale_id} evicted)");
        arch_x86_64::halt();
    }
    drop(guard);
    if killed {
        // The task is runnable and must be observed; do not leave it to a core
        // that is about to `hlt`.
        crate::smp::kick_one();
    }
}

/// Makes every task blocked on `child` runnable again.
///
/// The task is mutated in place under its slot lock: it never left the slot,
/// so no value move or fresh claim is needed.
pub fn wake(child: u64) {
    wake_matching(|wait| wait == WaitFor::Child(child));
}

/// Whether `wait` is satisfied by `source` becoming ready.
///
/// A single-source wait matches its own source; an `Any` set matches when it
/// contains the source. `Child` is never source-driven.
fn waits_on_source(wait: WaitFor, source: u64) -> bool {
    match wait {
        WaitFor::Console => source == SOURCE_CONSOLE,
        WaitFor::Input => source == SOURCE_INPUT,
        WaitFor::Stream(handle) => handle == source,
        WaitFor::Any(set) => set.contains(source),
        WaitFor::Child(_) => false,
    }
}

/// True when one source is ready for `task` (`core.next_any`).
fn source_ready(task: u64, source: u64) -> bool {
    if source == SOURCE_CONSOLE {
        crate::console::has_line()
    } else if source == SOURCE_INPUT {
        // `syslib.next_event` also returns a pending `Expose` (spec 0011,
        // W16), so the input source means "next_event would not block".
        crate::event::has_event() || crate::window::has_expose(task)
    } else {
        crate::stream::readable(task, source)
    }
}

/// True when any source in `set` is ready for `task`. The scheduler calls this
/// after putting a task to sleep on `WaitFor::Any`, closing the window between
/// the failed probe and the block (spec 0007).
pub fn wait_any_ready(task: u64, set: WaitSet) -> bool {
    for index in 0..set.len() {
        let Some(source) = set.get(index) else {
            break;
        };
        if source_ready(task, source) {
            return true;
        }
    }
    false
}

/// Makes every task whose wait intersects `set` runnable. Used by the
/// scheduler's post-block recheck: at least one source in `set` is ready, so a
/// task waiting on any of them must get a chance to observe it.
pub fn wake_any_sources(set: WaitSet) {
    wake_matching(|wait| {
        (0..set.len()).any(|index| {
            set.get(index)
                .is_some_and(|source| waits_on_source(wait, source))
        })
    });
}

/// Makes every task blocked on `source` runnable, including an `Any` set that
/// contains it.
fn wake_source(source: u64) {
    wake_matching(|wait| waits_on_source(wait, source));
}

/// Makes every task blocked on the console runnable again.
pub fn wake_console() {
    wake_source(SOURCE_CONSOLE);
}

/// Makes every task blocked on `handle` runnable again.
///
/// A send wakes a blocked receiver, a receive that frees space wakes a blocked
/// sender, and a close wakes both. The caller must have dropped the stream
/// lock first: this takes the slot locks, and the stream path never nests the
/// two.
pub fn wake_stream(handle: u64) {
    wake_source(handle);
}

/// Makes every task blocked in `syslib.next_event` runnable again.
///
/// The event queue calls this after it inserts an event, with its own lock
/// dropped so the slot locks are never nested inside the queue lock.
pub fn wake_input() {
    wake_source(SOURCE_INPUT);
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
/// [`KILL_ID`] and the owning core terminates it at its next quantum boundary
/// (or when it next blocks, see [`block`]). The task is not reclaimed here.
///
/// The reservation is re-read under the slot lock before anything is stored:
/// `reclaim` clears it under the same lock, so a record can never be published
/// for a slot whose id has already gone away.
pub fn request_kill(id: u64) -> Status {
    for (index, slot) in SLOTS.iter().enumerate() {
        if SLOT_ID[index].load(Ordering::Acquire) != id {
            continue;
        }
        let mut guard = slot.lock();
        if SLOT_ID[index].load(Ordering::Acquire) != id {
            // A concurrent termination cleared the reservation (or the slot was
            // handed to a different tenant) between the scan and the lock.
            drop(guard);
            continue;
        }
        match guard.as_mut() {
            Some(task) => {
                // The value is in the set, so no parked record is needed; a
                // record left over from a release is redundant with the field.
                let _ = KILL_ID[index].compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
                task.kill_requested = true;
                task.machine.request_yield();
                if task.state == TaskState::Blocked {
                    task.waiting = None;
                    task.state = TaskState::Runnable;
                    drop(guard);
                    crate::smp::kick_one();
                }
            }
            None => {
                // Claimed by another core; the id-tagged record is the whole
                // request. A later tenant of this slot has a different id and
                // ignores it.
                KILL_ID[index].store(id, Ordering::SeqCst);
            }
        }
        return Status::Ok;
    }
    Status::NotFound
}

/// Consumes an id-tagged kill record for `id`, or returns `false`.
///
/// The record is cleared only when it names `id`: a stale request left behind
/// by a previous tenant is never mistaken for this task's.
fn take_record(record: &AtomicU64, id: u64) -> bool {
    record
        .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Takes a pending kill request for the task in `slot` (set by [`request_kill`]
/// while the task was claimed by another core).
pub fn take_kill(slot: usize, id: u64) -> bool {
    KILL_ID
        .get(slot)
        .is_some_and(|record| take_record(record, id))
}

/// Boot self-check for the guarded-stack placement invariant (spec 0004 C6).
///
/// Pure arithmetic only: the same [`Region::layout_ok`] the guarded constructor
/// enforces is run over valid and invalid scratch tuples. No frame is allocated
/// and no PTE is touched. `Err(step)` names the first mismatch; the caller halts
/// loudly so a broken rule cannot pass silently.
pub fn stack_layout_self_check() -> Result<(), u8> {
    const F: u64 = frame::FRAME_SIZE;
    let base = 0x0020_0000u64;
    let frames = 64usize;
    let span = frames + 1;
    let start = base + F;
    let len = frames * F as usize;

    // Valid: a full region, a minimal region and a partial last frame.
    if !Region::layout_ok(base, start, frames, len, span) {
        return Err(1);
    }
    if !Region::layout_ok(base, start, 1, 1, 2) {
        return Err(2);
    }
    if !Region::layout_ok(base, start, frames, 100, span) {
        return Err(3);
    }
    // The usable region starts exactly one frame above the guard.
    if Region::layout_ok(base, base, frames, len, span) {
        return Err(4);
    }
    if Region::layout_ok(base, base + 2 * F, frames, len, span) {
        return Err(5);
    }
    // A region always has at least one frame and one byte.
    if Region::layout_ok(base, start, 0, len, 1) {
        return Err(6);
    }
    if Region::layout_ok(base, start, frames, 0, span) {
        return Err(7);
    }
    // The span is the guard plus exactly the region's frames.
    if Region::layout_ok(base, start, frames, len, frames) {
        return Err(8);
    }
    // The declared length cannot exceed the usable frames.
    if Region::layout_ok(base, start, frames, len + 1, span) {
        return Err(9);
    }
    // The base must be frame-aligned.
    if Region::layout_ok(base + 1, start, frames, len, span) {
        return Err(10);
    }
    // No overflow at the guard end or the region end.
    let top = u64::MAX & !(F - 1);
    if Region::layout_ok(top, top.wrapping_add(F), 1, F as usize, 2) {
        return Err(11);
    }
    let high = top - F;
    if Region::layout_ok(high, high + F, 1, F as usize, 2) {
        return Err(12);
    }
    // `frames * FRAME_SIZE` must not overflow either.
    if Region::layout_ok(base, start, usize::MAX - 1, 1, usize::MAX) {
        return Err(13);
    }
    Ok(())
}

/// Boot self-check for the id-tagged kill record and the block race (Q50).
///
/// Two properties are proved on real slot and record state, with a real task
/// value (not a mock):
///
/// 1. A record names exactly one id: a request for another id neither consumes
///    it nor is consumed by it, and a later store replaces it.
/// 2. The lost-wake interleaving: the owner has passed the quantum-boundary
///    check and still owns the task value (the slot is empty) when the killer
///    publishes the record; [`block`] must then put the task back `Runnable`
///    with `kill_requested` set, not asleep.
///
/// It runs before `smp::start`, so no other core can claim the scratch task,
/// and returns `Err(step)` on the first mismatch. The scratch task's regions are
/// reclaimed, so the free-frame count must be unchanged.
pub fn kill_self_check() -> Result<(), u8> {
    // 1. The record is tenant-tagged.
    let record = AtomicU64::new(0);
    if take_record(&record, 5) {
        return Err(1);
    }
    record.store(5, Ordering::SeqCst);
    if take_record(&record, 6) {
        return Err(2);
    }
    if !take_record(&record, 5) {
        return Err(3);
    }
    if take_record(&record, 5) {
        return Err(4);
    }
    record.store(5, Ordering::SeqCst);
    record.store(7, Ordering::SeqCst);
    if take_record(&record, 5) {
        return Err(5);
    }
    if !take_record(&record, 7) {
        return Err(6);
    }

    let free_before = frame::free_frames();

    // 2. The block re-check. The task is placed and its slot reserved, but the
    //    value stays with this core, exactly like a claimed task; the killer
    //    finds the reservation and an empty slot and parks the record.
    let id = reserve_id();
    let mut task = Task::place(16, 16, 64, id).map_err(|_| 7u8)?;
    let Some(index) = reserve_slot(id) else {
        task.reclaim();
        return Err(8);
    };
    task.slot = index;
    KILL_ID[index].store(id, Ordering::SeqCst);
    block(task, WaitFor::Console);
    let killed_ok = {
        let guard = SLOTS[index].lock();
        guard.as_ref().is_some_and(|held| {
            held.id == id
                && held.state == TaskState::Runnable
                && held.kill_requested
                && held.waiting.is_none()
        })
    };
    if !killed_ok {
        return Err(9);
    }
    {
        let mut guard = SLOTS[index].lock();
        let held = guard.take().ok_or(10u8)?;
        drop(guard);
        // The block path consumed the record; the boundary check must not see
        // it a second time.
        if take_kill(index, id) {
            return Err(11);
        }
        held.reclaim();
    }
    if frame::free_frames() != free_before {
        return Err(12);
    }

    // 3. A request that finds the value in the slot marks it directly and wakes
    //    a blocked task, so `wait` cannot sit behind a dead wait.
    let id = reserve_id();
    let mut task = Task::place(16, 16, 64, id).map_err(|_| 13u8)?;
    let Some(index) = reserve_slot(id) else {
        task.reclaim();
        return Err(14);
    };
    task.slot = index;
    block(task, WaitFor::Console);
    if request_kill(id) != Status::Ok {
        return Err(15);
    }
    let woken_ok = {
        let guard = SLOTS[index].lock();
        guard.as_ref().is_some_and(|held| {
            held.id == id
                && held.state == TaskState::Runnable
                && held.kill_requested
                && held.waiting.is_none()
        })
    };
    if !woken_ok {
        return Err(16);
    }
    {
        let mut guard = SLOTS[index].lock();
        let held = guard.take().ok_or(17u8)?;
        drop(guard);
        held.reclaim();
    }
    if frame::free_frames() != free_before {
        return Err(18);
    }
    if request_kill(u64::MAX) != Status::NotFound {
        return Err(19);
    }
    Ok(())
}

/// Reserves a free slot for a scratch task id (self-check only).
fn reserve_slot(id: u64) -> Option<usize> {
    (0..MAX_TASKS).find(|&index| {
        SLOT_ID[index]
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    })
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

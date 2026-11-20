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

pub mod task;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use abi::{ExitReason, FaultCode, Status};
use interpreter::{Diagnostic, Host, Outcome, Program, StreamError, WaitKind, WaitOutcome};

use crate::clock;
use crate::console;
use crate::frame;
use crate::println;
use crate::repl;
use crate::smp;
use crate::sync::SpinLock;
use crate::task::{Task, TaskClass, WaitFor};

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
/// `Invalid` rather than a permanent block. File reads go through the kernel's
/// one [`crate::fs`] router, which serves the read-only application image and
/// the optional home mount.
struct KernelHost {
    current: u64,
    /// Console session inherited at spawn (`0` for an unbound task). A terminal
    /// emulator reads/writes its session through the `session_*` builtins; a
    /// shell child uses it for `core.write`/`read_input` (spec 0021).
    session: u64,
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

impl Host for KernelHost {
    fn console_write(&mut self, bytes: &[u8]) {
        crate::console::write_bytes(self.session, bytes);
    }

    fn read_input(&mut self, dst: &mut [u8]) -> usize {
        console::read_input(self.session, dst)
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

    // The proportional font (spec 0015). The metrics are 0 and the drawing
    // primitives `Invalid` when graphics are disabled, matching the other
    // screen calls; the window chrome uses the font module directly because
    // it only runs while the window service is live.
    fn text_width_prop(&mut self, text: &[u8]) -> u64 {
        crate::gfx::text_width_prop(text)
    }

    fn text_height_prop(&mut self) -> u64 {
        crate::gfx::text_height_prop()
    }

    fn draw_text_prop(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
        crate::gfx::draw_text_prop(x, y, text, color)
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

    // Input (spec 0010). The pointer is one machine-wide value; it lives in the
    // graphics module because the cursor is composited at present time. These
    // are the read-only views an application gets, plus the debug-only
    // injection hook (I10–I12).
    fn pointer_x(&mut self) -> i64 {
        crate::gfx::pointer().0
    }

    fn pointer_y(&mut self) -> i64 {
        crate::gfx::pointer().1
    }

    fn pointer_buttons(&mut self) -> u64 {
        crate::gfx::pointer().2
    }

    fn inject_pointer(&mut self, dx: i64, dy: i64, buttons: u32) -> Status {
        crate::mouse::inject(dx, dy, buttons)
    }

    /// Which device currently drives the pointer (spec 0010, I27). The tablet
    /// wins over the PS/2 fallback once enumeration succeeds.
    fn pointer_source(&mut self) -> u64 {
        u64::from(crate::gfx::pointer_source())
    }

    // The machine-wide event queue (spec 0010, I19), routed at poll time
    // (spec 0011, W16/W25). Producers keep `window = 0` and may run under the
    // graphics lock, so stamping there would nest graphics -> window against
    // the repair's window -> graphics. Here no graphics lock is held:
    //
    // 1. A pending close request for the calling task comes first; only the
    //    owner can take it, topmost-first, and the event names the window so
    //    the owner can close exactly that one.
    // 2. A pending `Expose` for the calling task comes next; only the owner
    //    can take it, and the event names the whole content rectangle.
    // 3. Otherwise the global queue supplies the event. A `PointerDown` is
    //    hit-tested against the topmost frame, which is raised and focused
    //    before the click is delivered (the W7 affordance). Other pointer and
    //    key events target the focused window. An explicitly addressed event
    //    (non-zero `window`, including an injected `Expose` or `Close`) is
    //    passed through unchanged, so `inject_event` remains the deterministic
    //    test hook.
    fn poll_event(&mut self, event: &mut [u8]) -> Status {
        if let Some(id) = crate::window::take_close(self.current) {
            let close = abi::Event {
                kind: abi::EventKind::Close,
                x: 0,
                y: 0,
                code: 0,
                window: id as i64,
            };
            abi::layout::event::write(event, close);
            return Status::Ok;
        }
        if let Some(id) = crate::window::take_expose(self.current) {
            let expose = abi::Event {
                kind: abi::EventKind::Expose,
                x: 0,
                y: 0,
                code: 0,
                window: id as i64,
            };
            abi::layout::event::write(event, expose);
            return Status::Ok;
        }
        // The modal popup grab owns queued input (spec 0011, W26): a non-owner
        // sees `WouldBlock` without consuming anything, so the events are kept
        // for the owner; the owner receives them unchanged (no hit-test, no
        // raise), because a producer always writes `window = 0` and the owner
        // applies its own inside/outside policy.
        if !crate::window::grab_allows(self.current) {
            return Status::WouldBlock;
        }
        // Window-addressed delivery (spec 0011, W29; ADR 0047). The queue is
        // still one FIFO, but a queued event belongs to the task that owns its
        // target; a non-owner sees the head, does not consume it, and gets
        // `WouldBlock`. A window-less event with no target keeps the ordinary
        // competition. The peek releases the queue lock before the window lock
        // is taken, so the two never nest. The owner's normal routing below
        // still runs (a down raises/focuses/stamps, moves and keys name the
        // focus, an addressed record passes through); a press inside a
        // persistent region claim is delivered unchanged (W27).
        //
        // The modal grab takes precedence (W26): while it is held its owner
        // receives every queued event unchanged, whatever the focus is, so the
        // target gate is not applied.
        if !crate::window::grab_active() {
            let mut head = [0u8; abi::layout::event::SIZE as usize];
            if crate::event::peek(&mut head) == Status::Ok {
                if let Some(queued) = abi::layout::event::read(&head) {
                    if self
                        .input_target(&queued)
                        .is_some_and(|owner| owner != self.current)
                    {
                        return Status::WouldBlock;
                    }
                }
            }
        }
        let status = crate::event::poll(event);
        if status != Status::Ok {
            return status;
        }
        let Some(mut routed) = abi::layout::event::read(event) else {
            return Status::Invalid;
        };
        if routed.window == 0 && !crate::window::grab_active() {
            match routed.kind {
                abi::EventKind::PointerDown => {
                    // A press in a persistent region claim was delivered to
                    // its owner unchanged (W27): no hit-test, no raise, so the
                    // app-drawn bar sees `window = 0` and its coordinates.
                    if crate::window::region_owner_at(routed.x, routed.y).is_none() {
                        if let Some(id) = crate::window::route_click(routed.x, routed.y) {
                            routed.window = id as i64;
                        }
                    }
                }
                abi::EventKind::Expose => {}
                abi::EventKind::Close => {}
                _ => routed.window = crate::window::focus() as i64,
            }
        }
        abi::layout::event::write(event, routed);
        Status::Ok
    }

    fn inject_event(&mut self, event: &[u8]) -> Status {
        crate::event::inject(event)
    }

    fn event_drops(&mut self) -> u64 {
        crate::event::drops()
    }

    fn event_flush(&mut self) -> u64 {
        crate::event::flush()
    }

    // Multi-source waits (spec 0003, spec 0007; Q39, ADR 0036). The probes are
    // the same readiness functions the scheduler's post-block recheck uses, so
    // a source cannot be "ready" for the builtin and "not ready" for the wake.
    fn console_byte_ready(&mut self) -> bool {
        crate::console::has_byte_for(self.session)
    }

    fn input_ready(&mut self) -> bool {
        // The modal popup grab (W26) hides the shared queue from every
        // non-owner, and a press in another task's claimed region (W27) is not
        // a non-owner's to consume; the caller's own expose/close still count.
        crate::task::input_queue_ready(self.current)
            || crate::window::has_expose(self.current)
            || crate::window::has_close(self.current)
    }

    fn stream_readable(&mut self, handle: u64) -> bool {
        crate::stream::readable(self.current, handle)
    }

    /// Presents a window's content, clipped to its visible parts (spec 0011,
    /// W17). The host supplies the caller as the owner; the window table takes
    /// its own lock and no graphics lock is held on entry.
    fn window_present(&mut self, id: u64) -> Status {
        crate::window::present(self.current, id)
    }

    fn window_minimized(&mut self, id: u64) -> bool {
        crate::window::is_minimized(id)
    }

    // Window service (spec 0011). The kernel table owns the geometry, z-order,
    // focus and ownership; the host supplies the calling task id as the owner,
    // because the interpreter has no task identity. A disabled screen makes
    // every call the service's disabled result (W13).
    fn window_open(&mut self, x: i64, y: i64, w: i64, h: i64, title: &[u8]) -> Result<u64, Status> {
        crate::window::open(self.current, x, y, w, h, title)
    }

    fn window_close(&mut self, id: u64) -> Status {
        crate::window::close(self.current, id)
    }

    fn window_raise(&mut self, id: u64) -> Status {
        crate::window::raise(id)
    }

    fn window_move(&mut self, id: u64, x: i64, y: i64) -> Status {
        crate::window::move_to(self.current, id, x, y)
    }

    fn window_frame_rect(&mut self, id: u64) -> Result<abi::Rect, Status> {
        crate::window::frame_rect(id)
    }

    fn window_content_rect(&mut self, id: u64) -> Result<abi::Rect, Status> {
        crate::window::content_rect(id)
    }

    fn window_focus(&mut self) -> u64 {
        crate::window::focus()
    }

    /// Read-only enumeration for the desktop shell's window list (spec 0011,
    /// W19; spec 0012, D15). The kernel table owns the z-order and the title;
    /// the interpreter copies the snapshot into the caller's out-parameters.
    fn window_info(&mut self, index: u64) -> Option<interpreter::WindowInfo> {
        let info = crate::window::info(index)?;
        Some(interpreter::WindowInfo {
            id: info.id,
            frame: info.frame,
            title_len: info.title_len,
            title: info.title,
        })
    }

    /// The one machine-wide modal popup input grab (spec 0011, W26; ADR 0044).
    /// The host supplies the caller as the owner; `popup_release` wakes input
    /// waiters after the window unlock so queued events are deliverable again.
    fn popup_grab(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status {
        crate::window::popup_grab(self.current, x, y, w, h)
    }

    fn popup_release(&mut self) -> Status {
        crate::window::popup_release(self.current)
    }

    /// The one machine-wide persistent pointer-region claim (spec 0011, W27;
    /// ADR 0045). The host supplies the caller as the owner; a release wakes
    /// input waiters after the window unlock.
    fn region_claim(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status {
        crate::window::region_claim(self.current, x, y, w, h)
    }

    fn region_release(&mut self) -> Status {
        crate::window::region_release(self.current)
    }

    // Terminal emulator console sessions (spec 0021, ADR 0053). The owner is
    // the caller; the host supplies the task id so the language needs none.
    fn session_open(&mut self) -> Result<u64, Status> {
        crate::session::open(self.current)
    }

    fn session_close(&mut self, id: u64) -> Status {
        crate::session::close(self.current, id)
    }

    fn session_send(&mut self, id: u64, data: &[u8]) -> Result<usize, Status> {
        crate::session::send(self.current, id, data)
    }

    fn session_recv(&mut self, id: u64, dst: &mut [u8]) -> Result<usize, Status> {
        crate::session::recv(self.current, id, dst)
    }

    fn session_readable(&mut self, id: u64) -> bool {
        crate::session::readable(self.current, id)
    }

    // System control (spec 0017): the two primitives never return on a live
    // kernel, because the requesting task's core is the one that halts/resets.
    // A `Status` return type keeps the ABI uniform; it is unreachable here.
    fn halt(&mut self) -> Status {
        crate::sched::halt_system()
    }

    fn reboot(&mut self) -> Status {
        crate::sched::reboot()
    }

    // Application files (spec 0013, spec 0018, spec 0019). The one `fs` router
    // serves `home/...` from the optional home mount and everything else from
    // the read-only application image. The machine has validated the path
    // syntax; the host re-checks it defensively, converts the language `string`
    // to `str` and lets the driver walk the tree. No allocation.
    fn fs_list(
        &mut self,
        path: &[u8],
        index: u64,
        name: &mut [u8],
    ) -> Result<interpreter::FsEntry, Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_dir(path);
        if status != Status::Ok {
            return Err(status);
        }
        let index = u32::try_from(index).map_err(|_| Status::NotFound)?;
        let entry = crate::fs::list(path, index, name)?;
        Ok(interpreter::FsEntry {
            name_len: entry.name_len,
            kind: entry.kind,
            size: entry.size,
        })
    }

    fn fs_read(&mut self, path: &[u8], offset: u64, dst: &mut [u8]) -> Result<usize, Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        match abi::fs::validate_path(path) {
            Status::Ok => crate::fs::read_at(path, offset, dst),
            status => Err(status),
        }
    }

    fn fs_size(&mut self, path: &[u8]) -> Result<u64, Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        match abi::fs::validate_path(path) {
            Status::Ok => crate::fs::file_size(path),
            status => Err(status),
        }
    }

    /// `core.fs_write` (spec 0019, A1–A3): only a `home` path is writable, and
    /// the router takes the `HOME` lock around the ext2w writer.
    fn fs_write(&mut self, path: &[u8], offset: u64, data: &[u8]) -> Result<usize, Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        match abi::fs::validate_writable(path) {
            Status::Ok => crate::fs::write_at(path, offset, data),
            status => Err(status),
        }
    }

    /// `core.fs_remove`, `core.fs_mkdir` and `core.fs_rename` (spec 0019,
    /// A1/A2): all three require a `home/...` path and take the same router
    /// lock as a write.
    fn fs_remove(&mut self, path: &[u8]) -> Result<(), Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        match abi::fs::validate_writable(path) {
            Status::Ok => crate::fs::remove(path),
            status => Err(status),
        }
    }

    fn fs_mkdir(&mut self, path: &[u8]) -> Result<(), Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        match abi::fs::validate_writable(path) {
            Status::Ok => crate::fs::mkdir(path),
            status => Err(status),
        }
    }

    fn fs_rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Status> {
        let from = core::str::from_utf8(from).map_err(|_| Status::Invalid)?;
        let to = core::str::from_utf8(to).map_err(|_| Status::Invalid)?;
        match (abi::fs::validate_writable(from), abi::fs::validate_writable(to)) {
            (Status::Ok, Status::Ok) => crate::fs::rename(from, to),
            (status, _) if status != Status::Ok => Err(status),
            (_, status) => Err(status),
        }
    }

    /// `core.fs_truncate` (spec 0019, A7): `home` only, through the router.
    fn fs_truncate(&mut self, path: &[u8], size: u64) -> Result<(), Status> {
        let path = core::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        match abi::fs::validate_writable(path) {
            Status::Ok => crate::fs::truncate(path, size),
            status => Err(status),
        }
    }

    // Networking (spec 0020, N12–N18; ADR 0048–0050). The wire stack and the
    // device live in `crate::net`; the host only supplies the calling task as
    // the socket owner, so an application can never name another task's
    // socket. This round is the non-blocking form: `socket_send` and
    // `socket_recv` report `WouldBlock` and the caller retries.
    fn net_config(&mut self, ip: u32, mask: u32, gateway: u32, dns: u32) -> Status {
        crate::net::config(ip, mask, gateway, dns)
    }

    fn net_info(&mut self) -> Result<interpreter::NetInfo, Status> {
        crate::net::info().map(|info| interpreter::NetInfo {
            ip: info.ip,
            mask: info.mask,
            gateway: info.gateway,
            dns: info.dns,
        })
    }

    fn net_mac(&mut self) -> u64 {
        crate::net::mac()
    }

    /// `core.net_stats` (spec 0020, N22): the wire stack's bounded counters.
    fn net_stats(&mut self) -> Result<interpreter::NetStats, Status> {
        crate::net::stats().map(|stats| interpreter::NetStats {
            rx_frames: stats.rx_frames,
            tx_frames: stats.tx_frames,
            drops: stats.drops,
            errors: stats.errors,
        })
    }

    fn socket_open(
        &mut self,
        kind: u64,
        ip: u32,
        port: u16,
        local: u16,
    ) -> Result<u64, Status> {
        crate::net::open(kind, ip, port, local, self.current)
    }

    fn socket_send(&mut self, handle: u64, data: &[u8]) -> Result<usize, Status> {
        crate::net::send(handle, self.current, data)
    }

    fn socket_recv(&mut self, handle: u64, dst: &mut [u8]) -> Result<usize, Status> {
        crate::net::recv(handle, self.current, dst)
    }

    fn socket_close(&mut self, handle: u64) -> Status {
        crate::net::close(handle, self.current)
    }

    fn socket_accept(&mut self, handle: u64) -> Result<u64, Status> {
        crate::net::accept(handle, self.current)
    }

    fn socket_timeout(&mut self, handle: u64, ms: u64) -> Status {
        crate::net::timeout(handle, self.current, ms)
    }

    fn socket_timed_out(&mut self, handle: u64) -> bool {
        crate::net::timed_out(handle, self.current)
    }

    fn socket_arm_wait(&mut self, handle: u64) -> Status {
        crate::net::arm_read_wait(handle, self.current)
    }

    fn socket_readable(&mut self, handle: u64) -> bool {
        crate::net::select_ready(handle, self.current)
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
        let session = if self.session != 0 {
            self.session
        } else {
            crate::session::owned_by(self.current).unwrap_or(0)
        };
        repl::request_spawn(self.current, session, path, args, diag)
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

impl KernelHost {
    /// The task that owns `event`'s window target (spec 0011, W29; ADR 0047).
    /// `None` means no window target, so the ordinary first-poller rule
    /// applies. One kernel-internal rule serves both `poll_event` and
    /// `task::input_queue_ready`; it is not a language-visible primitive.
    fn input_target(&self, event: &abi::Event) -> Option<u64> {
        crate::window::input_target(event)
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

/// The boot core's scheduler loop. Never returns.
///
/// The loop ends when no task is live and no request is pending: with no
/// console input there is no other source of work, so it prints the summary and
/// halts. A blocked task keeps the system alive (it will be woken by a child
/// termination or console input), so the check is "no live task", not
/// "boot task finished".
pub fn run_forever() -> ! {
    loop_body(smp::core_index())
}

/// An AP's scheduler loop: the same claim → quantum → release cycle over the
/// same global runnable set (Sch1–Sch3). Any core may claim any task.
pub fn run_secondary(core: usize) -> ! {
    loop_body(core)
}

fn loop_body(core: usize) -> ! {
    loop {
        // The service opportunity runs before every claim: at most one
        // pending request is compiled and enqueued, on whichever core got
        // here first (spec 0007, "Services and task creation").
        service();

        // The atomic take under the slot lock is the claim (Sch2, Sch3).
        match task::claim() {
            Some(mut task) => {
                // A realtime task is not released at quantum expiry: this inner
                // loop runs further quanta on the same core until it blocks,
                // exits, observes a kill, or misses its heartbeat deadline. A
                // normal task always leaves after one quantum.
                loop {
                    let started = clock::now_ns();
                    let quantum = run_quantum(core, &mut task);
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
                    // another core through the id-tagged `KILL_ID` record. Both
                    // are honoured at this safe point (spec 0007,
                    // "Termination"). `task::block` re-checks the record again
                    // under the slot lock, because this check precedes the
                    // block and a request published in between would otherwise
                    // be lost (spec 0007, Sch17).
                    let killed = task.kill_requested || task::take_kill(task.slot, task.id);

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
                            // wakes `ChildExit`; console bytes wake a matching
                            // `Any` source `0` (Sch9).
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
                                WaitKind::Input => {
                                    println!("sched: task {} blocked on input", task.id);
                                    let id = task.id;
                                    task::block(task, WaitFor::Input);
                                    // An event may have been queued, or an
                                    // expose/close marked for this task,
                                    // between the empty read and the block
                                    // above. The queue probe also respects the
                                    // modal grab (W26) and the claimed region
                                    // (W27), so a non-owner does not spin on a
                                    // press that is not its to consume.
                                    if crate::task::input_queue_ready(id)
                                        || crate::window::has_expose(id)
                                        || crate::window::has_close(id)
                                    {
                                        task::wake_input();
                                    }
                                }
                                WaitKind::Any(set) => {
                                    println!(
                                        "sched: task {} blocked on {} source(s)",
                                        task.id,
                                        set.len()
                                    );
                                    let id = task.id;
                                    let session = task.session;
                                    task::block(task, WaitFor::Any(set));
                                    // A source may have become ready between
                                    // the failed probe and the block; the
                                    // wake-up path then found nobody to wake.
                                    if task::wait_any_ready(id, session, set) {
                                        task::wake_any_sources(set);
                                    }
                                }
                                WaitKind::Net(handle) => {
                                    println!(
                                        "sched: task {} blocked on socket {handle:#x}",
                                        task.id
                                    );
                                    let id = task.id;
                                    task::block(task, WaitFor::Net(handle));
                                    // Data/space/EOF/close may have arrived
                                    // between the failed call and the block;
                                    // the same direction probe the retry uses
                                    // closes the wire.
                                    if crate::net::wait_ready(handle, id) {
                                        task::wake_net(handle);
                                    }
                                }
                                WaitKind::Deadline(ns) => {
                                    println!(
                                        "sched: task {} blocked until {ns} ns",
                                        task.id
                                    );
                                    task::block(task, WaitFor::Deadline(ns));
                                    let now = clock::now_ns();
                                    if now >= ns {
                                        task::wake_deadlines(now);
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
fn run_quantum(core: usize, task: &mut Task) -> Quantum {
    let Task {
        id,
        class,
        session,
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
        current: *id,
        session: *session,
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
/// every millisecond, and this is where a typed byte reaches the console ring
/// and wakes a task blocked on source `0` (`core.next_any`).
fn service() {
    console::poll_input();
    crate::mouse::poll();
    crate::usb::poll();
    crate::net::service();
    task::wake_deadlines(clock::now_ns());
    if console::has_byte() {
        task::wake_console_input();
    }
    repl::service();
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
        // here is what turns a typed byte into a wake-up, and what keeps the
        // pointer moving while the system is otherwise idle (spec 0010, I14).
        console::poll_input();
        crate::mouse::poll();
        crate::usb::poll();
        crate::net::service();
        task::wake_deadlines(clock::now_ns());

        // Interrupts off for the check; `sti; hlt` is one atomic idle step, so
        // an IPI that arrives after the check is pending when `hlt` runs.
        arch_x86_64::disable_interrupts();
        if smp::shutdown_requested() {
            smp::note_stopped(core);
            arch_x86_64::halt();
        }
        if task::has_runnable() || repl::has_pending() || console::has_byte() {
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
/// console byte anywhere, and every other core idle, the boot core stops them,
/// prints the summary and halts.
///
/// Returns only when some work is still able to appear; once it decides to
/// stop the machine it never returns.
fn try_shutdown() {
    if task::live_count() != 0 || repl::has_pending() || console::has_byte() {
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

/// Requests a machine-wide halt and never returns (spec 0017).
///
/// This is the `core.halt` path. The calling core is inside an interpreter
/// quantum; the shutdown flag is set and every other online core is woken on
/// the existing vector, where [`crate::wake_ipi`] acknowledges and halts it even
/// if it is mid-task. The wait is bounded by the clock and a spin ceiling (the
/// boot core's timer can stop first when the caller is an AP), then the calling
/// core halts too. Serial stops once the print below drains.
pub fn halt_system() -> ! {
    // Write the request straight to serial rather than through `println!`: the
    // System Log repaint would paint over the desktop's final halt screen.
    // The summary below already avoids the console lock for the same reason
    // (another core may halt holding it).
    {
        use core::fmt::Write as _;
        let mut out = arch_x86_64::serial::Writer;
        let _ = writeln!(out, "system: halt requested");
    }
    arch_x86_64::enable_interrupts();
    // Count the calling core as stopped before the broadcast: when it is an AP,
    // the boot core (the clock source) halts in the same broadcast and the
    // acknowledgement count would otherwise never include this core.
    smp::note_stopped(smp::core_index());
    let (stopped, expected, _) = smp::stop_all();
    // Write the summary straight to serial: another core may have halted while
    // holding the console lock, so the summary must not take it (the fault path
    // does the same). The stop has already happened by here.
    {
        use core::fmt::Write as _;
        let mut out = arch_x86_64::serial::Writer;
        let _ = writeln!(out, "system: {stopped} of {expected} APs stopped");
    }
    arch_x86_64::halt();
}

/// Requests a system reset and never returns (spec 0017).
///
/// This is the `core.reboot` path. The shutdown request stops the other cores
/// through the wake vector first (a reset would otherwise leave them in an
/// unknown state), then the architecture layer drives the platform reset
/// channels and halts if the firmware ignores them. QEMU exits or restarts on
/// the reset (the probe harness distinguishes the two).
pub fn reboot() -> ! {
    println!("system: reboot requested");
    smp::request_shutdown();
    arch_x86_64::reboot();
}

/// One core's loop counters.
fn core_stats(index: usize) -> (u64, u64, u64) {
    (
        CLAIMS[index].load(Ordering::Relaxed),
        YIELDS[index].load(Ordering::Relaxed),
        EXITS[index].load(Ordering::Relaxed),
    )
}

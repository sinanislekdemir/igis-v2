//! The machine-wide input event queue (spec 0010, I19; ADR 0021).
//!
//! Input devices do not deliver events straight to an application: the kernel
//! owns one bounded FIFO and applications read it through
//! `syslib.next_event`/`syslib.poll_event`. The queue is machine-wide, so it
//! has no focus or window routing until M2.3; any task may read it.
//!
//! # Producers
//!
//! - the pointer paths in [`crate::gfx`] enqueue `PointerMove`, `PointerDown`
//!   and `PointerUp` when the machine-wide pointer changes, including moves
//!   made through the debug-only `syslib.inject_pointer`;
//! - the keyboard drain in [`crate::console`] enqueues `KeyDown`/`KeyUp` from
//!   `arch_x86_64::keyboard`'s decoded events;
//! - `syslib.inject_event` enqueues a caller-built record image.
//!
//! # Overflow policy
//!
//! The queue holds [`CAPACITY`] events. When it is full the **newest** event is
//! dropped and [`drops`] counts it, so a slow reader loses recent input rather
//! than the event it is about to process.
//!
//! # Wake-ups
//!
//! An insertion wakes every task blocked on `WaitFor::Input` (SMP-safe, through
//! [`crate::task::wake_input`]). The queue lock is never held while a task slot
//! lock is taken: [`push`] records whether it inserted, drops the queue lock,
//! and then wakes.

use abi::layout;
use abi::{Event, EventKind, Status};

use crate::sync::SpinLock;

/// Queue capacity in events. With a reader draining it every service
/// opportunity, 64 events cover a full second at the keyboard's maximum
/// make/break rate; a reader that stalls loses the newest input first.
pub const CAPACITY: usize = 64;

struct Queue {
    events: [[u8; layout::event::SIZE as usize]; CAPACITY],
    head: usize,
    len: usize,
    drops: u64,
}

impl Queue {
    const fn new() -> Self {
        Queue {
            events: [[0; layout::event::SIZE as usize]; CAPACITY],
            head: 0,
            len: 0,
            drops: 0,
        }
    }
}

static QUEUE: SpinLock<Queue> = SpinLock::new(Queue::new());

/// Appends one event. A full queue drops the newest event and counts it.
pub fn push(event: Event) {
    let inserted = {
        let mut queue = QUEUE.lock();
        if queue.len == CAPACITY {
            queue.drops = queue.drops.saturating_add(1);
            false
        } else {
            let slot = (queue.head + queue.len) % CAPACITY;
            layout::event::write(&mut queue.events[slot], event);
            queue.len += 1;
            true
        }
    };
    if inserted {
        crate::task::wake_input();
    }
}

/// Enqueues the pointer transitions between two machine-wide states.
///
/// A move is one `PointerMove`; a button change is one `PointerDown` with the
/// newly pressed bits and one `PointerUp` with the released bits. The calls are
/// made while the graphics lock is held so two cores cannot enqueue each
/// other's deltas out of order; the queue lock never takes the graphics lock,
/// so the order `graphics → events` has no cycle.
pub fn push_pointer(
    old_x: i64,
    old_y: i64,
    old_buttons: u64,
    new_x: i64,
    new_y: i64,
    new_buttons: u64,
) {
    if old_x != new_x || old_y != new_y {
        push(Event {
            kind: EventKind::PointerMove,
            x: new_x,
            y: new_y,
            code: new_buttons,
            window: 0,
        });
    }
    let pressed = new_buttons & !old_buttons;
    if pressed != 0 {
        push(Event {
            kind: EventKind::PointerDown,
            x: new_x,
            y: new_y,
            code: pressed,
            window: 0,
        });
    }
    let released = old_buttons & !new_buttons;
    if released != 0 {
        push(Event {
            kind: EventKind::PointerUp,
            x: new_x,
            y: new_y,
            code: released,
            window: 0,
        });
    }
}

/// Enqueues one decoded keyboard transition (spec 0010, I18).
pub fn push_key(key: arch_x86_64::keyboard::KeyEvent) {
    let code = abi::key::pack(key.code, key.text, key.modifiers);
    let kind = if key.pressed {
        EventKind::KeyDown
    } else {
        EventKind::KeyUp
    };
    push(Event {
        kind,
        x: 0,
        y: 0,
        code,
        window: 0,
    });
}

/// Pops the oldest event into `dst` (record-shaped), or reports `WouldBlock`.
///
/// `dst` is written only on `Ok`, so a failed read leaves the caller's record
/// untouched.
pub fn poll(dst: &mut [u8]) -> Status {
    let event = {
        let mut queue = QUEUE.lock();
        if queue.len == 0 {
            return Status::WouldBlock;
        }
        let slot = queue.head;
        queue.head = (queue.head + 1) % CAPACITY;
        queue.len -= 1;
        queue.events[slot]
    };
    let Some(dst) = dst.get_mut(..layout::event::SIZE as usize) else {
        return Status::Invalid;
    };
    dst.copy_from_slice(&event);
    Status::Ok
}

/// True when at least one event is queued.
pub fn has_event() -> bool {
    QUEUE.lock().len != 0
}

/// The number of events dropped because the queue was full since boot.
pub fn drops() -> u64 {
    QUEUE.lock().drops
}

/// Discards every queued event and returns how many were discarded. The drop
/// counter is not reset.
pub fn flush() -> u64 {
    let mut queue = QUEUE.lock();
    let flushed = queue.len as u64;
    queue.head = 0;
    queue.len = 0;
    flushed
}

/// Queues a caller-built event image (`syslib.inject_event`, debug-only).
///
/// The image is validated through [`layout::event::read`], so an unknown kind
/// ordinal is `Status::Invalid` and nothing is queued.
pub fn inject(image: &[u8]) -> Status {
    match layout::event::read(image) {
        Some(event) => {
            push(event);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

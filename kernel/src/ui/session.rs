//! Kernel-owned console sessions (spec 0021, ADR 0053).
//!
//! A session is a pair of bounded byte rings: the terminal emulator (the
//! owner) writes keystrokes into the input ring and reads screen bytes from the
//! output ring; a shell spawned by the owner inherits the session, so its
//! `core.write` and `core.read_input` become ordinary byte streams. The kernel
//! stores no terminal cells.
//!
//! The table is one slot per task slot. Each ring is small because the emulator
//! has no scrollback and drains continuously; output overflow keeps the newest
//! bytes, so the screen shows the most recent output.

use core::sync::atomic::{AtomicU64, Ordering};

use abi::Status;

use crate::sync::SpinLock;

/// Input ring capacity (emulator to shell).
pub const IN_BYTES: usize = 512;

/// Output ring capacity (shell to emulator).
pub const OUT_BYTES: usize = 4096;

/// One bounded byte ring.
struct Ring<const N: usize> {
    data: [u8; N],
    head: usize,
    count: usize,
    dropped: u64,
}

impl<const N: usize> Ring<N> {
    const fn new() -> Self {
        Ring {
            data: [0; N],
            head: 0,
            count: 0,
            dropped: 0,
        }
    }

    fn free(&self) -> usize {
        N - self.count
    }

    fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    /// Appends every byte or nothing. Returns `false` when the ring is full.
    fn push_all(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > self.free() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let mut tail = (self.head + self.count) % N;
        for byte in bytes {
            self.data[tail] = *byte;
            tail = (tail + 1) % N;
        }
        self.count += bytes.len();
        true
    }

    /// Appends as many bytes as fit, keeping the newest when full.
    fn push_keep_newest(&mut self, bytes: &[u8]) -> usize {
        let mut copied = 0;
        for byte in bytes {
            if self.count == N {
                self.head = (self.head + 1) % N;
                self.count -= 1;
                self.dropped = self.dropped.saturating_add(1);
            }
            let tail = (self.head + self.count) % N;
            self.data[tail] = *byte;
            self.count += 1;
            copied += 1;
        }
        copied
    }

    fn pop(&mut self, out: &mut [u8]) -> usize {
        let copied = self.count.min(out.len());
        for (index, slot) in out.iter_mut().take(copied).enumerate() {
            *slot = self.data[(self.head + index) % N];
        }
        self.head = (self.head + copied) % N;
        self.count -= copied;
        copied
    }
}

/// One console session.
struct Session {
    id: u64,
    owner: u64,
    /// Live slave tasks bound to this session (the shell and its children).
    bound: usize,
    closed: bool,
    finished: bool,
    input: Ring<IN_BYTES>,
    output: Ring<OUT_BYTES>,
}

impl Session {
    fn new(id: u64, owner: u64) -> Self {
        Session {
            id,
            owner,
            bound: 0,
            closed: false,
            finished: false,
            input: Ring::new(),
            output: Ring::new(),
        }
    }

    fn reset(&mut self) {
        self.input.clear();
        self.output.clear();
    }
}

/// One session slot per task slot; the effective terminal count is bounded by
/// the kernel's task and window tables, not by a product-level limit.
const SLOTS: usize = crate::task::MAX_TASKS;

static TABLE: [SpinLock<Option<Session>>; SLOTS] = [const { SpinLock::new(None) }; SLOTS];

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Finds the slot holding `id`, or `None`.
fn slot_of(id: u64) -> Option<usize> {
    for (index, slot) in TABLE.iter().enumerate() {
        if slot.lock().as_ref().is_some_and(|session| session.id == id) {
            return Some(index);
        }
    }
    None
}

/// Returns the live session owned by `owner`, if any.
pub fn owned_by(owner: u64) -> Option<u64> {
    for slot in TABLE.iter() {
        if let Some(session) = slot.lock().as_ref() {
            if session.owner == owner && !session.closed {
                return Some(session.id);
            }
        }
    }
    None
}

/// Creates a session owned by `owner` (spec 0021, S3).
pub fn open(owner: u64) -> Result<u64, Status> {
    if owner == 0 {
        return Err(Status::Invalid);
    }
    if owned_by(owner).is_some() {
        return Err(Status::Full);
    }
    for slot in TABLE.iter() {
        let mut guard = slot.lock();
        if guard.is_none() {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            *guard = Some(Session::new(id, owner));
            return Ok(id);
        }
    }
    Err(Status::Full)
}

/// Closes the caller's session and wakes its waiters (spec 0021, S3).
pub fn close(owner: u64, id: u64) -> Status {
    let Some(index) = slot_of(id) else {
        return Status::NotFound;
    };
    {
        let mut slot = TABLE[index].lock();
        let Some(session) = slot.as_mut() else {
            return Status::NotFound;
        };
        if session.owner != owner {
            return Status::Invalid;
        }
        session.closed = true;
        session.reset();
        if session.bound == 0 {
            *slot = None;
        }
    }
    crate::task::wake_session(id);
    crate::task::wake_console_input();
    Status::Ok
}

/// Copies `data` into the session input ring. Returns `WouldBlock` and copies
/// nothing when the whole buffer does not fit (spec 0021, S4).
pub fn send(owner: u64, id: u64, data: &[u8]) -> Result<usize, Status> {
    let Some(index) = slot_of(id) else {
        return Err(Status::NotFound);
    };
    let mut wake = false;
    let result = {
        let mut slot = TABLE[index].lock();
        let Some(session) = slot.as_mut() else {
            return Err(Status::NotFound);
        };
        if session.owner != owner {
            return Err(Status::Invalid);
        }
        if session.closed || session.finished {
            return Err(Status::Closed);
        }
        if session.input.push_all(data) {
            wake = !data.is_empty();
            Ok(data.len())
        } else {
            Err(Status::WouldBlock)
        }
    };
    if wake {
        crate::task::wake_console_input();
    }
    result
}

/// Copies from the session output ring (spec 0021, S5).
pub fn recv(owner: u64, id: u64, dst: &mut [u8]) -> Result<usize, Status> {
    let Some(index) = slot_of(id) else {
        return Err(Status::NotFound);
    };
    let mut slot = TABLE[index].lock();
    let Some(session) = slot.as_mut() else {
        return Err(Status::NotFound);
    };
    if session.owner != owner {
        return Err(Status::Invalid);
    }
    if session.output.count != 0 {
        return Ok(session.output.pop(dst));
    }
    if session.closed || session.finished {
        Err(Status::Closed)
    } else {
        Err(Status::WouldBlock)
    }
}

/// True when the owner's session source is ready (spec 0021, S9).
pub fn readable(owner: u64, id: u64) -> bool {
    let Some(index) = slot_of(id) else {
        return false;
    };
    let slot = TABLE[index].lock();
    let Some(session) = slot.as_ref() else {
        return false;
    };
    session.owner == owner && (session.output.count != 0 || session.closed || session.finished)
}

/// Binds a freshly spawned child to `id`. Returns the id it should carry, or 0
/// when the session is no longer live.
pub fn bind(id: u64) -> u64 {
    if id == 0 {
        return 0;
    }
    let Some(index) = slot_of(id) else {
        return 0;
    };
    let mut slot = TABLE[index].lock();
    let Some(session) = slot.as_mut() else {
        return 0;
    };
    if session.closed || session.finished {
        return 0;
    }
    session.bound += 1;
    id
}

/// True when a task bound to `id` has input bytes ready.
pub fn input_ready(id: u64) -> bool {
    let Some(index) = slot_of(id) else {
        return false;
    };
    let slot = TABLE[index].lock();
    let Some(session) = slot.as_ref() else {
        return false;
    };
    !session.closed && session.input.count != 0
}

/// Reads from a bound task's session input ring.
pub fn read_input(id: u64, out: &mut [u8]) -> usize {
    let Some(index) = slot_of(id) else {
        return 0;
    };
    let mut slot = TABLE[index].lock();
    let Some(session) = slot.as_mut() else {
        return 0;
    };
    if session.closed {
        return 0;
    }
    session.input.pop(out)
}

/// Appends a bound task's output to its session. Returns `false` when the
/// session is gone, so the caller can fall back to serial.
pub fn write_output(id: u64, bytes: &[u8]) -> bool {
    let Some(index) = slot_of(id) else {
        return false;
    };
    let mut copied = 0;
    let alive = {
        let mut slot = TABLE[index].lock();
        let Some(session) = slot.as_mut() else {
            return false;
        };
        if session.closed {
            false
        } else {
            copied = session.output.push_keep_newest(bytes);
            true
        }
    };
    if alive && copied != 0 {
        crate::task::wake_session(id);
    }
    alive
}

/// Drops a task's binding and closes an owner's session on termination
/// (spec 0021, S8). Called from [`crate::task::Task::reclaim`], before the
/// task's regions are freed.
pub fn task_ended(task_id: u64, bound: u64) {
    let mut wake_session = 0;
    let mut wake_input = false;

    if bound != 0 {
        if let Some(index) = slot_of(bound) {
            let mut slot = TABLE[index].lock();
            if let Some(session) = slot.as_mut() {
                if session.owner == task_id {
                    session.closed = true;
                    session.reset();
                    wake_session = session.id;
                    wake_input = true;
                    if session.bound == 0 {
                        *slot = None;
                    }
                } else if session.bound != 0 {
                    session.bound -= 1;
                    if session.bound == 0 {
                        session.finished = true;
                        wake_session = session.id;
                        if session.closed {
                            *slot = None;
                        }
                    }
                }
            }
        }
    }

    // An owner whose task field stayed 0 still owns a table entry; close it
    // even when `bound` did not name it.
    for slot in TABLE.iter() {
        let mut guard = slot.lock();
        let Some(session) = guard.as_mut() else {
            continue;
        };
        if session.owner != task_id {
            continue;
        }
        session.closed = true;
        session.reset();
        wake_session = session.id;
        wake_input = true;
        if session.bound == 0 {
            *guard = None;
        }
    }

    if wake_session != 0 {
        crate::task::wake_session(wake_session);
    }
    if wake_input {
        crate::task::wake_console_input();
    }
}

/// Appends one serial byte to a session input ring. Returns `false` when the
/// session is gone or full, so [`crate::console`] can fall back to the global
/// ring.
pub fn input_push(id: u64, byte: u8) -> bool {
    let Some(index) = slot_of(id) else {
        return false;
    };
    let pushed = {
        let mut slot = TABLE[index].lock();
        let Some(session) = slot.as_mut() else {
            return false;
        };
        if session.closed {
            false
        } else {
            session.input.push_all(&[byte])
        }
    };
    if pushed {
        crate::task::wake_console_input();
    }
    pushed
}

/// Boot self-check for the ring policy: FIFO order, full-buffer refusal and
/// the newest-wins output overflow.
pub fn self_check() -> Result<(), u32> {
    let mut ring = Ring::<8>::new();
    if !ring.push_all(b"abc") {
        return Err(1);
    }
    if ring.push_all(b"defghij") {
        return Err(2);
    }
    let mut out = [0u8; 4];
    if ring.pop(&mut out[..2]) != 2 || out[..2] != *b"ab" {
        return Err(3);
    }
    if !ring.push_all(b"xyz") {
        return Err(4);
    }
    if ring.pop(&mut out) != 4 || out != *b"cxyz" {
        return Err(5);
    }

    let mut output = Ring::<4>::new();
    output.push_keep_newest(b"abcdef");
    if output.count != 4 {
        return Err(6);
    }
    if output.pop(&mut out) != 4 || out != *b"cdef" {
        return Err(7);
    }
    if output.dropped != 2 {
        return Err(8);
    }
    Ok(())
}

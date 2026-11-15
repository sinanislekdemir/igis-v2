//! Kernel-owned byte FIFOs with endpoint handles (spec 0006).
//!
//! A stream is one buffer of `capacity` bytes plus a head/count pair and two
//! endpoint sides. Applications never see the buffer: `state` holds an opaque
//! 8-byte handle, and every send/receive copies through the kernel (S2, S6).
//!
//! # Dynamic loading
//!
//! Tasks are created at run time, so two peers cannot be loaded together. An
//! endpoint is created when the first task that declares it loads: its stream
//! exists immediately, with the other side not yet live. A peer that arrives
//! later with the same name and the opposite direction pairs into the same
//! record; a second endpoint with the same direction, a capacity mismatch or a
//! declaration against a closed stream is a load error. The wait is ordinary
//! blocking: a receiver on an empty waiting stream blocks, and a sender may
//! fill a waiting stream up to its capacity (spec 0006, "Dynamic loading").
//!
//! # Handles and generation
//!
//! A handle is `(generation << 8) | index`. A recycled table slot gets a new
//! generation, so a stale handle can never reach a different stream; every
//! operation validates both fields.
//!
//! # Locking
//!
//! One spin lock guards the table, the arena and every stream buffer. No task
//! slot lock is ever taken while it is held: an operation records whether a
//! wake is needed, drops the guard, and then calls [`crate::task::wake_stream`].
//! The loader and `terminate` use the same rule, so there is no path that holds
//! the stream lock and a slot lock at once.

use crate::sync::SpinLock;
use interpreter::StreamError;

/// Stream table slots. Matches the compiler's per-application endpoint cap.
pub const MAX_STREAMS: usize = 32;

/// Kernel-owned byte arena for every stream buffer (S1/S2). It is never mapped
/// into an application region and never grows.
pub const ARENA_BYTES: usize = 256 * 1024;

/// Longest stored name, matching spec 0006 S12 (`string[32]`).
const NAME_BYTES: usize = 32;

/// Free-list entries. One allocation can produce at most one extra fragment,
/// and there are at most [`MAX_STREAMS`] live buffers; the slack absorbs
/// fragmentation before coalescing.
const MAX_BLOCKS: usize = MAX_STREAMS + 8;

#[derive(Clone, Copy)]
struct Block {
    off: u32,
    len: u32,
}

#[derive(Clone, Copy)]
struct Record {
    active: bool,
    /// Incremented when the slot is recycled; zero means never used.
    generation: u64,
    name_len: u8,
    name: [u8; NAME_BYTES],
    capacity: u32,
    has_buffer: bool,
    buffer_off: u32,
    head: u32,
    count: u32,
    /// Terminal: set when either side closes. A closed record never pairs and
    /// discards queued bytes immediately (S7).
    closed: bool,
    sender_live: bool,
    sender_task: u64,
    receiver_live: bool,
    receiver_task: u64,
}

const EMPTY_RECORD: Record = Record {
    active: false,
    generation: 0,
    name_len: 0,
    name: [0; NAME_BYTES],
    capacity: 0,
    has_buffer: false,
    buffer_off: 0,
    head: 0,
    count: 0,
    closed: false,
    sender_live: false,
    sender_task: 0,
    receiver_live: false,
    receiver_task: 0,
};

struct Streams {
    records: [Record; MAX_STREAMS],
    blocks: [Block; MAX_BLOCKS],
    block_count: usize,
    data: [u8; ARENA_BYTES],
}

const EMPTY_BLOCK: Block = Block { off: 0, len: 0 };

impl Streams {
    const fn new() -> Self {
        Streams {
            records: [EMPTY_RECORD; MAX_STREAMS],
            blocks: [EMPTY_BLOCK; MAX_BLOCKS],
            block_count: 1,
            data: [0; ARENA_BYTES],
        }
    }

    fn init(&mut self) {
        self.records = [EMPTY_RECORD; MAX_STREAMS];
        self.blocks = [EMPTY_BLOCK; MAX_BLOCKS];
        self.blocks[0] = Block {
            off: 0,
            len: ARENA_BYTES as u32,
        };
        self.block_count = 1;
    }

    /// First-fit allocation over the free list.
    fn alloc(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return None;
        }
        for index in 0..self.block_count {
            if self.blocks[index].len < len {
                continue;
            }
            let off = self.blocks[index].off;
            if self.blocks[index].len == len {
                self.remove_block(index);
            } else {
                self.blocks[index].off = off + len;
                self.blocks[index].len -= len;
            }
            return Some(off);
        }
        None
    }

    /// Returns a region and coalesces it with adjacent free blocks. The block
    /// array is sized so this cannot overflow for the stream table's own
    /// allocations; if it ever did, leaking the block is safer than corrupting
    /// the list.
    fn release(&mut self, off: u32, len: u32) {
        if len == 0 {
            return;
        }
        let mut index = 0;
        while index < self.block_count && self.blocks[index].off < off {
            index += 1;
        }
        if index > 0 {
            let previous = self.blocks[index - 1];
            if previous.off + previous.len == off {
                self.blocks[index - 1].len = previous.len.checked_add(len).unwrap_or(previous.len);
                self.coalesce_next(index - 1);
                return;
            }
        }
        if self.block_count == MAX_BLOCKS {
            return;
        }
        for slot in (index..self.block_count).rev() {
            self.blocks[slot + 1] = self.blocks[slot];
        }
        self.blocks[index] = Block { off, len };
        self.block_count += 1;
        self.coalesce_next(index);
    }

    fn coalesce_next(&mut self, index: usize) {
        if index + 1 >= self.block_count {
            return;
        }
        let here = self.blocks[index];
        let next = self.blocks[index + 1];
        if here.off + here.len == next.off {
            self.blocks[index].len = here.len.saturating_add(next.len);
            self.remove_block(index + 1);
        }
    }

    fn remove_block(&mut self, index: usize) {
        for slot in index..self.block_count - 1 {
            self.blocks[slot] = self.blocks[slot + 1];
        }
        self.block_count -= 1;
    }

    fn find_name(&self, name: &[u8]) -> Option<usize> {
        self.records.iter().position(|record| {
            record.active
                && record.name_len as usize == name.len()
                && record.name[..name.len()] == *name
        })
    }

    fn free_record(&mut self, index: usize) {
        let record = &mut self.records[index];
        record.active = false;
        record.generation = record.generation.wrapping_add(1).max(1);
        record.name_len = 0;
        record.name = [0; NAME_BYTES];
        record.capacity = 0;
        record.has_buffer = false;
        record.buffer_off = 0;
        record.head = 0;
        record.count = 0;
        record.closed = false;
        record.sender_live = false;
        record.sender_task = 0;
        record.receiver_live = false;
        record.receiver_task = 0;
    }
}

static STREAMS: SpinLock<Streams> = SpinLock::new(Streams::new());

/// Initialises the stream table and arena. Called once at boot, before any task
/// can load.
pub fn init() {
    STREAMS.lock().init();
}

fn encode(index: usize, generation: u64) -> u64 {
    (generation << 8) | index as u64
}

fn decode(handle: u64) -> Option<(usize, u64)> {
    let index = (handle & 0xFF) as usize;
    let generation = handle >> 8;
    if generation == 0 || index >= MAX_STREAMS {
        return None;
    }
    Some((index, generation))
}

/// Why an endpoint could not be created or paired (spec 0006, "Dynamic
/// loading"). The loader turns these into `Diagnostic` messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// Every stream table slot is occupied.
    TableFull,
    /// The byte arena cannot cover the request.
    ArenaFull,
    /// A sender with this name is already live.
    SenderExists,
    /// A receiver with this name is already live.
    ReceiverExists,
    /// The live peer declared a different capacity.
    CapacityMismatch { existing: u32, requested: u32 },
    /// The named stream closed; a new declaration cannot resurrect it.
    Closed,
    /// The descriptor itself is malformed.
    Invalid,
}

/// Creates or pairs one endpoint owned by `task`.
///
/// `direction` is [`abi::direction::IN`] or [`abi::direction::OUT`]. Returns
/// the 8-byte handle the loader writes into the task's `state`. A new stream
/// allocates its buffer here; a paired endpoint uses the existing buffer.
pub fn open(task: u64, name: &[u8], direction: u8, capacity: u32) -> Result<u64, OpenError> {
    if task == 0
        || name.is_empty()
        || name.len() > NAME_BYTES - 1
        || capacity == 0
        || capacity > 65_536
        || (direction != abi::direction::IN && direction != abi::direction::OUT)
    {
        return Err(OpenError::Invalid);
    }
    let mut streams = STREAMS.lock();
    if let Some(index) = streams.find_name(name) {
        let record = &mut streams.records[index];
        if record.closed {
            return Err(OpenError::Closed);
        }
        // Same direction first: a second sender/receiver is the more precise
        // conflict even when it also chose a different capacity.
        if direction == abi::direction::OUT && record.sender_live {
            return Err(OpenError::SenderExists);
        }
        if direction == abi::direction::IN && record.receiver_live {
            return Err(OpenError::ReceiverExists);
        }
        if record.capacity != capacity {
            return Err(OpenError::CapacityMismatch {
                existing: record.capacity,
                requested: capacity,
            });
        }
        if direction == abi::direction::OUT {
            record.sender_live = true;
            record.sender_task = task;
        } else {
            record.receiver_live = true;
            record.receiver_task = task;
        }
        return Ok(encode(index, record.generation));
    }

    let Some(index) = streams.records.iter().position(|record| !record.active) else {
        return Err(OpenError::TableFull);
    };
    let Some(off) = streams.alloc(capacity) else {
        return Err(OpenError::ArenaFull);
    };
    let generation = streams.records[index].generation.wrapping_add(1).max(1);
    let mut record = EMPTY_RECORD;
    record.active = true;
    record.generation = generation;
    record.name_len = name.len() as u8;
    record.name[..name.len()].copy_from_slice(name);
    record.capacity = capacity;
    record.has_buffer = true;
    record.buffer_off = off;
    if direction == abi::direction::OUT {
        record.sender_live = true;
        record.sender_task = task;
    } else {
        record.receiver_live = true;
        record.receiver_task = task;
    }
    streams.records[index] = record;
    Ok(encode(index, generation))
}

/// Validates a handle and returns `(index, generation)` for a live record.
fn lookup(streams: &Streams, handle: u64) -> Option<(usize, u64)> {
    let (index, generation) = decode(handle)?;
    let record = &streams.records[index];
    if !record.active || record.generation != generation {
        return None;
    }
    Some((index, generation))
}

/// Copies `bytes` into the queue; the caller has checked the free space.
fn write_ring(streams: &mut Streams, index: usize, bytes: &[u8]) {
    let record = streams.records[index];
    let base = record.buffer_off as usize;
    let capacity = record.capacity as usize;
    let mut at = (record.head as usize + record.count as usize) % capacity;
    for byte in bytes {
        streams.data[base + at] = *byte;
        at += 1;
        if at == capacity {
            at = 0;
        }
    }
    streams.records[index].count = record.count + bytes.len() as u32;
}

/// Reads up to `dst.len()` bytes from the queue; the caller has checked that
/// the queue is non-empty.
fn read_ring(streams: &mut Streams, index: usize, dst: &mut [u8]) -> usize {
    let record = streams.records[index];
    let base = record.buffer_off as usize;
    let capacity = record.capacity as usize;
    let copied = (record.count as usize).min(dst.len());
    let mut at = record.head as usize;
    for slot in dst.iter_mut().take(copied) {
        *slot = streams.data[base + at];
        at += 1;
        if at == capacity {
            at = 0;
        }
    }
    streams.records[index].head = at as u32;
    streams.records[index].count = record.count - copied as u32;
    copied
}

/// `core.send`/`core.try_send`: one atomic copy into the stream owned by
/// `task`'s sender side. `WouldBlock` means nothing was copied.
pub fn send(task: u64, handle: u64, bytes: &[u8]) -> Result<(), StreamError> {
    {
        let mut streams = STREAMS.lock();
        let Some((index, _)) = lookup(&streams, handle) else {
            return Err(StreamError::Invalid);
        };
        let record = streams.records[index];
        if !record.sender_live || record.sender_task != task {
            return Err(StreamError::Invalid);
        }
        if record.closed {
            return Err(StreamError::Closed);
        }
        if bytes.len() as u64 > u64::from(record.capacity) {
            return Err(StreamError::CapacityExceeded);
        }
        let free = record.capacity as usize - record.count as usize;
        if bytes.len() > free {
            return Err(StreamError::WouldBlock);
        }
        write_ring(&mut streams, index, bytes);
    }
    // A send made data available. Waking is done after the stream lock is
    // dropped; `wake_stream` takes slot locks.
    crate::task::wake_stream(handle);
    Ok(())
}

/// `core.recv`/`core.try_recv`: copy at most `dst.len()` bytes into `dst`.
/// `Closed` is reported only after every queued byte has been drained.
pub fn recv(task: u64, handle: u64, dst: &mut [u8]) -> Result<usize, StreamError> {
    let copied = {
        let mut streams = STREAMS.lock();
        let Some((index, _)) = lookup(&streams, handle) else {
            return Err(StreamError::Invalid);
        };
        let record = streams.records[index];
        if !record.receiver_live || record.receiver_task != task {
            return Err(StreamError::Invalid);
        }
        if record.count == 0 {
            return if record.closed {
                Err(StreamError::Closed)
            } else {
                Err(StreamError::WouldBlock)
            };
        }
        read_ring(&mut streams, index, dst)
    };
    // A receive freed space; wake a sender blocked on this stream.
    crate::task::wake_stream(handle);
    Ok(copied)
}

/// True when `task`'s blocked operation on `handle` can make progress (or must
/// observe `Closed`). The scheduler calls this after putting a task to sleep to
/// close the window between the failed attempt and the block.
pub fn ready_for(task: u64, handle: u64) -> bool {
    let streams = STREAMS.lock();
    let Some((index, _)) = lookup(&streams, handle) else {
        // A stale handle must not leave the task asleep forever.
        return true;
    };
    let record = &streams.records[index];
    if record.closed {
        return true;
    }
    if record.sender_live && record.sender_task == task {
        return record.count < record.capacity;
    }
    if record.receiver_live && record.receiver_task == task {
        return record.count > 0;
    }
    true
}

/// Closes every endpoint `task` owns. Queued bytes are discarded immediately,
/// the peer is woken to observe `Closed`, and the buffer returns to the arena
/// once the record has no live side. Called from [`crate::task::Task::reclaim`]
/// for every termination path (normal, killed and faulted).
pub fn close_task(task: u64) {
    let mut wake = [0u64; MAX_STREAMS];
    let mut wake_count = 0usize;
    let mut freed = [(0u32, 0u32); MAX_STREAMS];
    let mut freed_count = 0usize;
    {
        let mut streams = STREAMS.lock();
        for index in 0..MAX_STREAMS {
            let mut record = streams.records[index];
            if !record.active {
                continue;
            }
            let mut changed = false;
            if record.sender_live && record.sender_task == task {
                record.sender_live = false;
                record.sender_task = 0;
                changed = true;
            }
            if record.receiver_live && record.receiver_task == task {
                record.receiver_live = false;
                record.receiver_task = 0;
                changed = true;
            }
            if !changed {
                continue;
            }
            if wake_count < wake.len() {
                wake[wake_count] = encode(index, record.generation);
                wake_count += 1;
            }
            record.closed = true;
            // Closing is terminal for the whole stream, so the buffer can go
            // back to the arena now even if the peer is still live: S7 discards
            // queued data immediately and a closed record never pairs again.
            // The record itself stays until the surviving side ends, which
            // keeps the name reserved and lets the peer observe `Closed`.
            if record.has_buffer {
                record.has_buffer = false;
                if freed_count < freed.len() {
                    freed[freed_count] = (record.buffer_off, record.capacity);
                    freed_count += 1;
                }
            }
            record.head = 0;
            record.count = 0;
            if !record.sender_live && !record.receiver_live {
                streams.free_record(index);
            } else {
                streams.records[index] = record;
            }
        }
        for (off, len) in &freed[..freed_count] {
            streams.release(*off, *len);
        }
    }
    for handle in &wake[..wake_count] {
        crate::task::wake_stream(*handle);
    }
}

/// Removes one endpoint created for a load that later failed.
///
/// The loader calls this with the reserved task id; it behaves exactly like
/// termination, so a peer that already observed the pairing sees `Closed`
/// rather than a half-created stream.
pub fn close_owner(task: u64) {
    close_task(task);
}

//! The REPL service: a request queue, the compile-and-place path and the
//! exit-reason ring (spec 0007, ADR 0017).
//!
//! Applications ask for another application with `core.spawn`, which reserves
//! a global task id and enqueues a [`Request`]. [`service`] runs before every
//! claim and processes at most one request per opportunity: it reads the source
//! from ext2, compiles it in the kernel with the same static buffers the boot
//! path uses, validates the package, places `code`/`state`/`stack` and enqueues
//! the task under the reserved id.
//!
//! A failure is not reported to the caller directly (`core.spawn` returns
//! `Status.Ok` once the id is reserved; the compile happens later). It is
//! written to the console and recorded in the exit ring as `Faulted(IoError)`
//! under the reserved id, so a parent blocked in `core.wait` wakes and observes
//! a failed child.
//!
//! The exit ring holds the last [`EXIT_RING_CAPACITY`] terminations as
//! `{id, reason}`. `core.wait` consumes the record; the oldest is evicted when
//! the ring is full. Regions are already reclaimed at that point: the record is
//! bounded kernel state, not a zombie task.
//!
//! Every buffer the queue, the compiler and the verifier need is static
//! (`.bss`); no part of this path allocates (spec 0008, B11).

use abi::layout;
use abi::{
    ExitReason, FaultCode, ManifestHeader, Status, ABI_VERSION, MANIFEST_HEADER_SIZE,
    MANIFEST_MAGIC, PROFILE_X86_64_LE,
};

use crate::ext2::Ext2;
use crate::println;
use crate::sched;
use crate::sync::SpinLock;
use crate::task::{self, Task, ARGS_BYTES};

/// Maximum path length accepted by `core.spawn` (spec 0005).
pub const PATH_BYTES: usize = 256;

/// Pending `spawn` requests (spec 0005/0007: 1024 by default).
pub const QUEUE_CAPACITY: usize = 1024;

/// Exit records kept for `core.wait` (spec 0005/0007: 64 by default).
pub const EXIT_RING_CAPACITY: usize = 64;

/// Source buffer: one unit, capped by the compiler.
static SOURCE: SpinLock<[u8; compiler::MAX_SOURCE_BYTES]> =
    SpinLock::new([0; compiler::MAX_SOURCE_BYTES]);
/// Compiler scratch arena (tokens, AST, tables, instruction image).
static ARENA: SpinLock<[u8; compiler::MAX_ARENA_BYTES]> =
    SpinLock::new([0; compiler::MAX_ARENA_BYTES]);
/// Code-blob buffer the compiler writes into before it is copied to a task
/// region.
static CODE_SCRATCH: SpinLock<[u8; compiler::MAX_CODE_BYTES]> =
    SpinLock::new([0; compiler::MAX_CODE_BYTES]);
/// Manifest buffer.
static MANIFEST: SpinLock<[u8; compiler::MAX_MANIFEST_BYTES]> =
    SpinLock::new([0; compiler::MAX_MANIFEST_BYTES]);
/// The compiler is a single resource; one compile at a time.
static COMPILER_LOCK: SpinLock<()> = SpinLock::new(());

/// Largest `state` or `stack` region a manifest may request (matching the
/// compiler/verifier section cap).
const MAX_REGION_BYTES: usize = 1 << 20;

/// One pending compile-and-run request (spec 0007, "Services and task
/// creation"). `args` is the child's `string[1024]` image, length prefix
/// included; `args_len` is the payload length, at most [`ARGS_BYTES`].
#[derive(Clone, Copy)]
struct Request {
    reserved_id: u64,
    requester: u64,
    path_len: u16,
    args_len: u16,
    path: [u8; PATH_BYTES],
    args: [u8; 8 + ARGS_BYTES],
}

const EMPTY_REQUEST: Request = Request {
    reserved_id: 0,
    requester: 0,
    path_len: 0,
    args_len: 0,
    path: [0; PATH_BYTES],
    args: [0; 8 + ARGS_BYTES],
};

/// A fixed ring of requests. `next` is the oldest slot; `len` entries are
/// live. Both are only touched under the lock.
struct Queue {
    entries: [Request; QUEUE_CAPACITY],
    next: usize,
    len: usize,
}

impl Queue {
    const fn new() -> Self {
        Queue {
            entries: [EMPTY_REQUEST; QUEUE_CAPACITY],
            next: 0,
            len: 0,
        }
    }

    fn push(&mut self, request: Request) -> Result<(), Status> {
        if self.len == QUEUE_CAPACITY {
            return Err(Status::Full);
        }
        let index = (self.next + self.len) % QUEUE_CAPACITY;
        self.entries[index] = request;
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<Request> {
        if self.len == 0 {
            return None;
        }
        let request = self.entries[self.next];
        self.len -= 1;
        self.next = (self.next + 1) % QUEUE_CAPACITY;
        Some(request)
    }

    fn live_index(&self, id: u64) -> Option<usize> {
        for offset in 0..self.len {
            let index = (self.next + offset) % QUEUE_CAPACITY;
            if self.entries[index].reserved_id == id {
                return Some(offset);
            }
        }
        None
    }

    fn contains(&self, id: u64) -> bool {
        self.live_index(id).is_some()
    }

    /// Removes one request by reserved id, preserving the order of the rest.
    fn remove(&mut self, id: u64) -> bool {
        let Some(offset) = self.live_index(id) else {
            return false;
        };
        for step in offset..self.len - 1 {
            let from = (self.next + step + 1) % QUEUE_CAPACITY;
            let to = (self.next + step) % QUEUE_CAPACITY;
            self.entries[to] = self.entries[from];
        }
        self.len -= 1;
        true
    }
}

static REQUESTS: SpinLock<Queue> = SpinLock::new(Queue::new());

/// Size of the `Diagnostic` record image carried by a failed request
/// ([`layout::diagnostic::SIZE`] bytes).
pub const DIAG_BYTES: usize = layout::diagnostic::SIZE as usize;

/// One consumed-once exit record.
///
/// `diag` carries the request-completion diagnostic: a load failure the service
/// discovers after `spawn` returned fills it with the compiler's `Diagnostic`
/// image. `core.run` copies it into the caller's out-parameter; `core.wait` has
/// no such parameter and observes `Faulted(IoError)` only.
#[derive(Clone, Copy)]
struct ExitRecord {
    id: u64,
    reason: ExitReason,
    has_diag: bool,
    diag: [u8; DIAG_BYTES],
}

const EMPTY_EXIT: ExitRecord = ExitRecord {
    id: 0,
    reason: ExitReason::Ok,
    has_diag: false,
    diag: [0; DIAG_BYTES],
};

/// The global exit-reason ring. `next` is the slot the next record overwrites,
/// which is also the oldest record. An `id` of zero marks an empty slot (task
/// ids start at one).
struct ExitRing {
    entries: [ExitRecord; EXIT_RING_CAPACITY],
    next: usize,
}

impl ExitRing {
    const fn new() -> Self {
        ExitRing {
            entries: [EMPTY_EXIT; EXIT_RING_CAPACITY],
            next: 0,
        }
    }

    fn record(&mut self, id: u64, reason: ExitReason) {
        self.entries[self.next] = ExitRecord {
            id,
            reason,
            has_diag: false,
            diag: [0; DIAG_BYTES],
        };
        self.next = (self.next + 1) % EXIT_RING_CAPACITY;
    }

    fn record_failure(&mut self, id: u64, reason: ExitReason, diag: &[u8]) {
        let mut record = ExitRecord {
            id,
            reason,
            has_diag: true,
            diag: [0; DIAG_BYTES],
        };
        let copied = diag.len().min(DIAG_BYTES);
        record.diag[..copied].copy_from_slice(&diag[..copied]);
        self.entries[self.next] = record;
        self.next = (self.next + 1) % EXIT_RING_CAPACITY;
    }

    fn take(&mut self, id: u64) -> Option<(ExitReason, Option<[u8; DIAG_BYTES]>)> {
        for entry in self.entries.iter_mut() {
            if entry.id == id {
                entry.id = 0;
                let diag = if entry.has_diag {
                    Some(entry.diag)
                } else {
                    None
                };
                return Some((entry.reason, diag));
            }
        }
        None
    }
}

static EXITS: SpinLock<ExitRing> = SpinLock::new(ExitRing::new());

/// Boots the first application: `shell`, else `entrypoint`, else `no entry`.
///
/// Returns the id of the task left in the runnable set. A failed attempt
/// burns its reserved id (ids are never reused) and falls through to the next
/// name, as before.
pub fn boot(image: &Ext2<'_>) -> u64 {
    for name in ["shell", "entrypoint"] {
        let id = task::reserve_id();
        let mut diag = [0u8; DIAG_BYTES];
        if load(image, name, id, &[], &mut diag).is_ok() {
            return id;
        }
    }

    println!("no entry");
    arch_x86_64::halt();
}

/// Validates a `core.spawn` path (spec 0005): relative, at most
/// [`PATH_BYTES`], no leading/trailing `/`, no empty or `..` component.
pub fn validate_path(path: &str) -> Status {
    if path.len() > PATH_BYTES {
        return Status::CapacityExceeded;
    }
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Status::Invalid;
    }
    if path.split('/').any(|part| part.is_empty() || part == "..") {
        return Status::Invalid;
    }
    Status::Ok
}

/// The kernel side of `core.spawn`: validate, fill the diagnostic, reserve an
/// id, enqueue.
///
/// The path is checked against ext2 now so a missing or non-regular file is
/// reported as a `Status` immediately, as spec 0005 requires. Every synchronous
/// failure fills `diag` with the `Diagnostic` record fields at
/// [`layout::diagnostic`] offsets; success clears it. A compile error is only
/// discovered by the service; it is printed, recorded as `Faulted(IoError)` and
/// carries the compiler's diagnostic in the exit record, which `core.run`
/// copies into the caller's `diag` (spec 0005, "Implemented ABI").
pub fn request_spawn(
    image: &Ext2<'_>,
    requester: u64,
    path: &[u8],
    args: &[u8],
    diag: &mut [u8],
) -> Result<u64, Status> {
    let describe = |diag: &mut [u8], message: &[u8]| {
        layout::diagnostic::write(diag, b"", message, 0, 0);
    };
    let path = match core::str::from_utf8(path) {
        Ok(path) => path,
        Err(_) => {
            describe(diag, b"path is not valid UTF-8");
            return Err(Status::Invalid);
        }
    };
    match validate_path(path) {
        Status::Ok => {}
        Status::Invalid => {
            describe(
                diag,
                b"path must be relative with no empty or `..` component",
            );
            return Err(Status::Invalid);
        }
        Status::CapacityExceeded => {
            describe(diag, b"path is longer than 256 bytes");
            return Err(Status::CapacityExceeded);
        }
        status => {
            describe(diag, b"invalid path");
            return Err(status);
        }
    }
    if args.len() > ARGS_BYTES {
        describe(diag, b"argument is longer than 1024 bytes");
        return Err(Status::CapacityExceeded);
    }
    if core::str::from_utf8(args).is_err() {
        describe(diag, b"argument is not valid UTF-8");
        return Err(Status::Invalid);
    }
    // `file_size` also refuses a directory with `Invalid`.
    match image.file_size(path) {
        Ok(_) => {}
        Err(Status::NotFound) => {
            describe(diag, b"No such application");
            return Err(Status::NotFound);
        }
        Err(Status::Invalid) => {
            describe(diag, b"not a regular application file");
            return Err(Status::Invalid);
        }
        Err(status) => {
            describe(diag, b"application file cannot be read");
            return Err(status);
        }
    }

    let id = task::reserve_id();
    let mut request = EMPTY_REQUEST;
    request.reserved_id = id;
    request.requester = requester;
    request.path_len = path.len() as u16;
    request.path[..path.len()].copy_from_slice(path.as_bytes());
    request.args_len = args.len() as u16;
    request.args[..8].copy_from_slice(&(args.len() as u64).to_le_bytes());
    request.args[8..8 + args.len()].copy_from_slice(args);
    if let Err(status) = REQUESTS.lock().push(request) {
        describe(diag, b"request queue is full");
        return Err(status);
    }

    layout::diagnostic::clear(diag);
    println!("repl: task {requester} requested `{path}` as task {id}");
    Ok(id)
}

/// Processes at most one pending request. Called before every claim.
pub fn service(image: &Ext2<'_>) {
    let request = REQUESTS.lock().pop();
    let Some(request) = request else {
        return;
    };

    let name = match core::str::from_utf8(&request.path[..request.path_len as usize]) {
        Ok(name) => name,
        Err(_) => {
            println!("repl: task {}: malformed request path", request.reserved_id);
            let mut diag = [0u8; DIAG_BYTES];
            layout::diagnostic::write(&mut diag, b"", b"request path is not valid UTF-8", 0, 0);
            record_exit_with_diag(
                request.reserved_id,
                ExitReason::Faulted(FaultCode::IoError),
                &diag,
            );
            task::wake(request.reserved_id);
            return;
        }
    };

    println!(
        "repl: service: task {} requested by {} (`{name}`)",
        request.reserved_id, request.requester
    );
    let args = &request.args[..8 + request.args_len as usize];
    let mut diag = [0u8; DIAG_BYTES];
    if load(image, name, request.reserved_id, args, &mut diag).is_err() {
        // The caller already has the id and may already be blocked in `run`.
        // A synthetic fault record wakes it; the diagnostic rides along so a
        // `run` caller can report the compiler's message. The regions were
        // never placed (or were reclaimed by `load`), so no task exists.
        println!(
            "repl: task {}: request failed; no task created",
            request.reserved_id
        );
        record_exit_with_diag(
            request.reserved_id,
            ExitReason::Faulted(FaultCode::IoError),
            &diag,
        );
        task::wake(request.reserved_id);
    }
}

/// Records a termination for `core.wait`.
pub fn record_exit(id: u64, reason: ExitReason) {
    EXITS.lock().record(id, reason);
}

/// Records a request failure with the compiler's `Diagnostic` image.
pub fn record_exit_with_diag(id: u64, reason: ExitReason, diag: &[u8]) {
    EXITS.lock().record_failure(id, reason, diag);
}

/// Consumes the exit record for `id`, if present. The optional diagnostic is
/// the request-completion path's compiler image; only `core.run` reads it.
pub fn take_exit(id: u64) -> Option<(ExitReason, Option<[u8; DIAG_BYTES]>)> {
    EXITS.lock().take(id)
}

/// True when `id` is reserved by a request that has not been serviced yet.
pub fn is_pending(id: u64) -> bool {
    REQUESTS.lock().contains(id)
}

/// Cancels a queued request by reserved id (`core.kill` before it is serviced).
pub fn cancel_request(id: u64) -> bool {
    REQUESTS.lock().remove(id)
}

/// True when at least one request is waiting.
pub fn has_pending() -> bool {
    REQUESTS.lock().len != 0
}

/// Reads, compiles, validates, places and enqueues one application under a
/// reserved id.
///
/// `Err(())` means "not runnable": a missing file, an over-cap source, a
/// failed compile and a rejected package are all reported and treated the same
/// way. On `Ok`, the task is in the runnable set and the scheduler owns it.
fn load(image: &Ext2<'_>, name: &str, id: u64, args: &[u8], diag: &mut [u8]) -> Result<(), ()> {
    let describe = |diag: &mut [u8], message: &[u8]| {
        layout::diagnostic::write(diag, b"", message, 0, 0);
    };
    println!("repl: trying `{name}`");

    let mut source = SOURCE.lock();
    let source_len = match image.read_file(name, &mut source[..]) {
        Ok(len) => len,
        Err(Status::NotFound) => {
            println!("repl: {name}: not found");
            describe(diag, b"No such application");
            return Err(());
        }
        Err(Status::CapacityExceeded) => {
            println!(
                "repl: {name}: source is larger than the {}-byte cap",
                compiler::MAX_SOURCE_BYTES
            );
            describe(diag, b"source is larger than the compiler cap");
            return Err(());
        }
        Err(status) => {
            println!("repl: {name}: read failed ({})", status.name());
            describe(diag, b"application file cannot be read");
            return Err(());
        }
    };

    // One compile at a time. All four buffers are held until the package has
    // been validated and copied into the task's code region.
    let _compiler = COMPILER_LOCK.lock();
    let mut arena = ARENA.lock();
    let mut code_scratch = CODE_SCRATCH.lock();
    let mut manifest = MANIFEST.lock();

    let package = match compiler::compile_into(
        &source[..source_len],
        name,
        &mut arena[..],
        &mut code_scratch[..],
        &mut manifest[..],
    ) {
        Ok(package) => package,
        Err(diagnostic) => {
            println!("repl: {name}: {diagnostic}");
            layout::diagnostic::write(
                diag,
                diagnostic.unit.as_str().as_bytes(),
                diagnostic.message.as_str().as_bytes(),
                i64::from(diagnostic.line),
                i64::from(diagnostic.column),
            );
            return Err(());
        }
    };

    let header = match manifest_header(package.manifest) {
        Ok(header) => header,
        Err(status) => {
            println!("repl: {name}: bad manifest ({})", status.name());
            describe(diag, b"bad application manifest");
            return Err(());
        }
    };
    if header.code_len as usize != package.code.len() {
        println!(
            "repl: {name}: manifest code length {} does not match {} bytes",
            header.code_len,
            package.code.len()
        );
        describe(diag, b"manifest code length does not match the package");
        return Err(());
    }
    let state_size = header.state_size as usize;
    let stack_size = header.stack_size as usize;
    if state_size > MAX_REGION_BYTES || stack_size > MAX_REGION_BYTES {
        println!("repl: {name}: region size over the {MAX_REGION_BYTES}-byte cap");
        describe(diag, b"application region size is over the cap");
        return Err(());
    }

    let code_len = package.code.len();
    let mut task = match Task::place(code_len, state_size, stack_size, id) {
        Ok(task) => task,
        Err(status) => {
            println!("repl: {name}: cannot place regions ({})", status.name());
            describe(diag, b"cannot place application regions");
            return Err(());
        }
    };
    task.regions.code.as_slice_mut()[..code_len].copy_from_slice(package.code);
    let copied = args.len().min(task.args.len());
    task.args[..copied].copy_from_slice(&args[..copied]);

    println!(
        "repl: {name}: {} bytes code ({} frame{}), state {} ({}), stack {} ({})",
        code_len,
        task.regions.code.frames,
        plural(task.regions.code.frames),
        state_size,
        task.regions.state.frames,
        stack_size,
        task.regions.stack.frames
    );

    // The compiler buffers are no longer needed; only the placed regions and
    // the verifier scratch remain.
    drop(arena);
    drop(code_scratch);
    drop(manifest);
    drop(_compiler);

    // Validate the bytes the task will actually execute, not the compiler's
    // scratch. A rejected package never enters the runnable set.
    if let Err(diagnostic) = sched::parse_program(task.regions.code.as_slice()) {
        println!("repl: {name}: verifier rejected the package: {diagnostic}");
        describe(diag, b"verifier rejected the package");
        task.reclaim();
        return Err(());
    }

    let id = task.id;
    match task::spawn(task) {
        Ok(()) => {
            println!("repl: {name}: spawned as task {id}");
            Ok(())
        }
        Err(status) => {
            // `spawn` reclaimed the task's regions.
            println!("repl: {name}: cannot enqueue ({})", status.name());
            describe(diag, b"cannot enqueue the application task");
            Err(())
        }
    }
}

fn plural(frames: usize) -> &'static str {
    if frames == 1 {
        ""
    } else {
        "s"
    }
}

/// Decodes and validates the manifest half of a package.
fn manifest_header(bytes: &[u8]) -> Result<ManifestHeader, Status> {
    let header = bytes
        .get(..MANIFEST_HEADER_SIZE)
        .and_then(ManifestHeader::from_bytes)
        .ok_or(Status::Invalid)?;
    if header.magic != MANIFEST_MAGIC
        || header.abi_version != ABI_VERSION
        || header.profile != PROFILE_X86_64_LE
    {
        return Err(Status::Invalid);
    }
    // ABI v1 declares no stream endpoints; a manifest that does is from a
    // newer compiler than this kernel.
    if header.endpoint_count != 0 {
        return Err(Status::Invalid);
    }
    Ok(header)
}

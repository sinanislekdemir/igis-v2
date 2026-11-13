//! The boot REPL service: read an application's source from the ext2 image,
//! compile it in the kernel, place its regions and enqueue it (spec 0002,
//! ADR 0017).
//!
//! This is the loader, not the runner: it leaves a [`Task`] in the scheduler's
//! runnable set and returns. [`crate::sched::run_forever`] claims the task and
//! advances it across quanta. Console input does not exist yet, so the system
//! halts when the boot application terminates and the set runs dry.
//!
//! Every buffer the compiler and the verifier need is static (`.bss`); no part
//! of this path allocates (spec 0008, B11).

use abi::{
    ManifestHeader, Status, ABI_VERSION, MANIFEST_HEADER_SIZE, MANIFEST_MAGIC, PROFILE_X86_64_LE,
};

use crate::ext2::Ext2;
use crate::println;
use crate::sched;
use crate::sync::SpinLock;
use crate::task::{self, Task};

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

/// Boots the first application: `shell`, else `entrypoint`, else `no entry`.
///
/// Returns the id of the task left in the runnable set. A name that is missing
/// or fails to compile falls through to the next one, as before.
pub fn boot(image: &Ext2<'_>) -> u64 {
    for name in ["shell", "entrypoint"] {
        if let Ok(id) = load(image, name) {
            return id;
        }
    }

    println!("no entry");
    arch_x86_64::halt();
}

/// Reads, compiles, validates, places and enqueues one application.
///
/// `Err(())` means "not runnable"; the boot fallback tries the next name. A
/// missing file, an over-cap source, a failed compile and a rejected package
/// are all reported and treated the same way. On `Ok`, the task is in the
/// runnable set and the scheduler owns it.
fn load(image: &Ext2<'_>, name: &str) -> Result<u64, ()> {
    println!("repl: trying `{name}`");

    let mut source = SOURCE.lock();
    let source_len = match image.read_file(name, &mut source[..]) {
        Ok(len) => len,
        Err(Status::NotFound) => {
            println!("repl: {name}: not found");
            return Err(());
        }
        Err(Status::CapacityExceeded) => {
            println!(
                "repl: {name}: source is larger than the {}-byte cap",
                compiler::MAX_SOURCE_BYTES
            );
            return Err(());
        }
        Err(status) => {
            println!("repl: {name}: read failed ({})", status.name());
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
            return Err(());
        }
    };

    let header = match manifest_header(package.manifest) {
        Ok(header) => header,
        Err(status) => {
            println!("repl: {name}: bad manifest ({})", status.name());
            return Err(());
        }
    };
    if header.code_len as usize != package.code.len() {
        println!(
            "repl: {name}: manifest code length {} does not match {} bytes",
            header.code_len,
            package.code.len()
        );
        return Err(());
    }
    let state_size = header.state_size as usize;
    let stack_size = header.stack_size as usize;
    if state_size > MAX_REGION_BYTES || stack_size > MAX_REGION_BYTES {
        println!("repl: {name}: region size over the {MAX_REGION_BYTES}-byte cap");
        return Err(());
    }

    let code_len = package.code.len();
    let mut task = match Task::place(code_len, state_size, stack_size) {
        Ok(task) => task,
        Err(status) => {
            println!("repl: {name}: cannot place regions ({})", status.name());
            return Err(());
        }
    };
    task.regions.code.as_slice_mut()[..code_len].copy_from_slice(package.code);

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
        task.reclaim();
        return Err(());
    }

    let id = task.id;
    match task::spawn(task) {
        Ok(()) => {
            println!("repl: {name}: spawned as task {id}");
            Ok(id)
        }
        Err(status) => {
            // `spawn` reclaimed the task's regions.
            println!("repl: {name}: cannot enqueue ({})", status.name());
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

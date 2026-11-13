//! The boot REPL service: read an application's source from the ext2 image,
//! compile it in the kernel, place its regions and run it (spec 0002, ADR 0017).
//!
//! This is the degenerate scheduler for the first milestone: exactly one task,
//! started at boot on the boot core, run in fixed-size slices until it exits.
//! The [`Task`] value already owns its regions and its [`Machine`], so the real
//! scheduler can claim it from a runnable set and release it again without
//! changing this compilation path. Console input does not exist yet, so the
//! system halts when the boot application exits.
//!
//! Every buffer the compiler and the verifier need is static (`.bss`); no part
//! of this path allocates (spec 0008, B11).

use abi::{
    ExitReason, FaultCode, ManifestHeader, Status, ABI_VERSION, MANIFEST_HEADER_SIZE,
    MANIFEST_MAGIC, PROFILE_X86_64_LE,
};
use interpreter::{Host, Machine, Outcome, Program};

use crate::console;
use crate::ext2::Ext2;
use crate::frame;
use crate::println;
use crate::sync::SpinLock;

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
/// Verifier scratch: one bit per 4-byte instruction word.
const VERIFY_BYTES: usize = compiler::MAX_CODE_BYTES / 4 / 8;
static VERIFY: SpinLock<[u8; VERIFY_BYTES]> = SpinLock::new([0; VERIFY_BYTES]);
/// Argument buffer: `[len: u64][data: 1024]` (spec 0008, "Values in the VM").
const ARGS_BYTES: usize = 1024;
static ARGS: SpinLock<[u8; 8 + ARGS_BYTES]> = SpinLock::new([0; 8 + ARGS_BYTES]);
/// The compiler is a single resource; one compile at a time.
static COMPILER_LOCK: SpinLock<()> = SpinLock::new(());

/// Instructions executed per scheduling slice. The real scheduler will replace
/// this with a quantum; it is a fairness hint, not a kill.
const SLICE_INSTRUCTIONS: u64 = 100_000;
/// Total instruction budget for the boot application. This is the watchdog,
/// not the quantum (spec 0002, "Safe points, quanta and budgets").
const TOTAL_BUDGET: u64 = 200_000_000;

/// Largest `state` or `stack` region a manifest may request (matching the
/// compiler/verifier section cap).
const MAX_REGION_BYTES: usize = 1 << 20;

/// One physically contiguous task region.
#[derive(Clone, Copy)]
struct Region {
    start: u64,
    frames: usize,
    len: usize,
}

impl Region {
    /// Allocates `len` bytes (at least one frame, so the slice is never empty).
    fn allocate(len: usize) -> Result<Self, Status> {
        let len = len.max(1);
        let frames = (len + frame::FRAME_SIZE as usize - 1) / frame::FRAME_SIZE as usize;
        let start = frame::alloc_contiguous(frames).ok_or(Status::Full)?;
        Ok(Region { start, frames, len })
    }

    fn as_slice_mut(&mut self) -> &mut [u8] {
        // SAFETY: `start`/`len` describe frames reserved by the frame allocator
        // and identity-mapped by the boot page tables. `&mut self` gives
        // exclusive access to the region.
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u8, self.len) }
    }

    fn free(self) {
        frame::free_contiguous(self.start, self.frames);
    }
}

/// The regions a task owns (spec 0004). The `fpu` region arrives with the
/// floating-point work; ABI v1 packages do not declare one yet.
struct Regions {
    code: Region,
    state: Region,
    stack: Region,
}

/// A runnable application instance. This is the value the real scheduler will
/// claim, run for a quantum and release.
pub struct Task {
    regions: Regions,
    machine: Machine,
}

impl Task {
    /// Reserves and zeroes the task's regions.
    fn new(code_len: usize, state_len: usize, stack_len: usize) -> Result<Self, Status> {
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
            regions: Regions { code, state, stack },
            machine: Machine::new(),
        };
        task.regions.code.as_slice_mut().fill(0);
        task.regions.state.as_slice_mut().fill(0);
        task.regions.stack.as_slice_mut().fill(0);
        Ok(task)
    }

    /// Frees every region. The real scheduler will call this when a task
    /// terminates.
    fn reclaim(self) {
        self.regions.code.free();
        self.regions.state.free();
        self.regions.stack.free();
    }
}

/// The kernel's implementation of the interpreter host interface (spec 0005).
struct KernelHost;

impl Host for KernelHost {
    fn console_write(&mut self, bytes: &[u8]) {
        console::write_bytes(bytes);
    }

    fn clock(&mut self) -> u64 {
        crate::clock::now_ns()
    }
}

/// Boots the first application: `shell`, else `entrypoint`, else `no entry`.
///
/// Never returns: after the chosen task exits its regions are reclaimed and the
/// system halts (there is no console input yet).
pub fn boot(image: &Ext2<'_>) -> ! {
    for name in ["shell", "entrypoint"] {
        if try_run(image, name).is_ok() {
            println!("halt: no console input");
            arch_x86_64::halt();
        }
    }

    println!("no entry");
    arch_x86_64::halt();
}

/// Reads, compiles, places and runs one application.
///
/// `Err(())` means "not runnable"; the boot fallback tries the next name. A
/// missing file, an over-cap source and a failed compile are all reported and
/// treated the same way. Once a task starts, this runs it to completion and
/// returns `Ok(())`.
fn try_run(image: &Ext2<'_>, name: &str) -> Result<(), ()> {
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
    let mut task = match Task::new(code_len, state_size, stack_size) {
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

    // The manifest and code buffers are no longer needed; only the placed
    // regions and the verifier scratch remain.
    drop(arena);
    drop(code_scratch);
    drop(manifest);
    drop(_compiler);

    // Parse from the placed region, not from the compiler scratch: the
    // program must describe the bytes the task will execute. The slice is
    // derived from a raw address so the borrow does not cover the `Task`
    // value (whose other fields are borrowed mutably below).
    let reason = {
        // SAFETY: the code region is live until `task.reclaim()` at the end of
        // this function, and only read while the task runs.
        let code = unsafe {
            core::slice::from_raw_parts(task.regions.code.start as *const u8, task.regions.code.len)
        };
        let mut verify = VERIFY.lock();
        let program = match Program::parse(code, &mut verify[..]) {
            Ok(program) => program,
            Err(diagnostic) => {
                println!("repl: {name}: verifier rejected the package: {diagnostic}");
                task.reclaim();
                return Err(());
            }
        };
        drop(verify);

        let mut args = ARGS.lock();
        args[..8].copy_from_slice(&0u64.to_le_bytes());

        println!("running {name}");

        let mut host = KernelHost;
        let mut remaining = TOTAL_BUDGET;
        let Task {
            machine, regions, ..
        } = &mut task;
        loop {
            let slice = remaining.min(SLICE_INSTRUCTIONS);
            match machine.run(
                &program,
                &mut host,
                regions.state.as_slice_mut(),
                regions.stack.as_slice_mut(),
                &args[..],
                slice,
            ) {
                Outcome::Exited(reason) => break reason,
                Outcome::Yielded => {
                    remaining = remaining.saturating_sub(slice);
                    if remaining == 0 {
                        // The real scheduler kills the task here (spec 0002);
                        // there is no `fault` handler in ABI v1.
                        break ExitReason::Faulted(FaultCode::BudgetExceeded);
                    }
                }
            }
        }
    };

    let free = frame::free_frames();
    match reason {
        ExitReason::Ok => println!("exit: Ok"),
        ExitReason::Killed => println!("exit: Killed"),
        ExitReason::Faulted(code) => println!("exit: Faulted({})", code.name()),
    }
    let frames = task.regions.code.frames + task.regions.state.frames + task.regions.stack.frames;
    task.reclaim();
    println!(
        "repl: reclaimed {frames} frames; {free} free before, {} free after",
        frame::free_frames()
    );
    Ok(())
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

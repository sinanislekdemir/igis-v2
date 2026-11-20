//! Not-present guard pages for kernel stacks (ADR 0006, spec 0002 H2).
//!
//! A guard turns a stack overflow into a clean `#PF` instead of a silent write
//! into whatever follows the stack. Three kinds of stack are guarded:
//!
//! - the boot core's 256 KiB post-boot stack, on frames from the allocator
//!   ([`install_guarded`], Q42);
//! - one AP kernel stack per possible core, on a static `.bss` frame
//!   (`arch_x86_64::smp`);
//! - every application task's declared stack, on one extra frame immediately
//!   below the stack region ([`crate::task`], spec 0004 C6).
//!
//! # Why the guard is below the region, not inside it
//!
//! The boot page tables map memory with 2 MiB pages. Installing a guard means
//! splitting the containing 2 MiB page into 4 KiB entries once, then clearing
//! **one** entry. The other 511 entries stay present and writable, so several
//! stacks inside one 2 MiB page — which is the common case for task stacks —
//! coexist in one shared table.
//!
//! # Tables and the registry
//!
//! [`TABLES`] is a fixed cache of [`MAX_TABLES`] 4 KiB tables, each keyed by the
//! 2 MiB base it covers. A table is never freed: the directory entry keeps
//! pointing at it, so reclaiming the table could only be done by restoring a
//! huge page, which buys nothing in the first milestone. Exhausting the cache
//! makes the next guard fail, which a placement path reports as `Full`.
//!
//! [`REGISTRY`] names the guarded frames so the `#PF` handler can say which
//! stack was hit. It is bounded by [`MAX_GUARDS`]; a guard needs one entry.
//!
//! # SMP TLB correctness
//!
//! A task stack frame may have been mapped and used by another core before it
//! was freed and reused as a guard. After the PTE is cleared, that core's TLB
//! may still hold a present, writable translation, and a stale write would then
//! bypass the guard (or, after [`unguard`], fault on a frame the allocator has
//! already handed out again). So every PTE change is followed by a local CR3
//! reload and an IPI shootdown to every online core (`crate::smp::flush_all`),
//! which waits, bounded, for each target to reload its CR3.
//!
//! A task guard hit halts the machine with a named message. It is a hard bug
//! backstop, not the per-task fault path: the interpreter checks every frame
//! extent against the declared stack length and terminates a legal overflow
//! with `Faulted(StackOverflow)` before the hardware guard can be reached.

use arch_x86_64::paging;
use arch_x86_64::KERNEL_BASE;

use crate::println;
use crate::sync::SpinLock;

/// 4 KiB tables the guard cache can install (one per 2 MiB page).
pub const MAX_TABLES: usize = 32;

/// Guarded frames the fault handler can name at once.
pub const MAX_GUARDS: usize = 96;

/// One 4 KiB page table. `#[repr(align(4096))]` is what makes it a valid
/// paging-structure page; it lives in `.bss` and is written by
/// [`paging::split_huge_page`].
#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut TABLE_MEMORY: [PageTable; MAX_TABLES] = [const { PageTable([0; 512]) }; MAX_TABLES];

/// The 2 MiB base each cached table was installed for. Entries `0..count` are
/// live; a table is never released.
struct Tables {
    bases: [u64; MAX_TABLES],
    count: usize,
}

impl Tables {
    const fn new() -> Self {
        Tables {
            bases: [0; MAX_TABLES],
            count: 0,
        }
    }

    fn find(&self, base: u64) -> Option<usize> {
        self.bases[..self.count]
            .iter()
            .position(|candidate| *candidate == base)
    }

    /// Reserves the next table slot for `base`; the caller fills and installs
    /// it before the entry counts as live.
    fn claim(&mut self, base: u64) -> Option<usize> {
        if self.count == MAX_TABLES {
            return None;
        }
        let index = self.count;
        self.bases[index] = base;
        self.count += 1;
        Some(index)
    }
}

static TABLES: SpinLock<Tables> = SpinLock::new(Tables::new());

/// What a guarded frame protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    /// The boot core's post-boot kernel stack.
    BootCore,
    /// An AP's static kernel stack; `who` is the core index.
    ApCore,
    /// An application task's declared stack; `who` is the global task id.
    Task,
}

/// One guarded frame, as the fault handler sees it.
#[derive(Debug, Clone, Copy)]
pub struct GuardInfo {
    /// First byte of the not-present frame, as a physical/identity address.
    pub frame: u64,
    pub kind: GuardKind,
    /// Task id or AP core index; zero for the boot core.
    pub who: u64,
}

/// A fixed table of guarded frames, searched by frame address.
struct Registry {
    entries: [Option<GuardInfo>; MAX_GUARDS],
}

impl Registry {
    const fn new() -> Self {
        Registry {
            entries: [None; MAX_GUARDS],
        }
    }

    fn find(&self, frame: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.is_some_and(|info| info.frame == frame))
    }

    fn free(&self) -> Option<usize> {
        self.entries.iter().position(|entry| entry.is_none())
    }

    fn first_of(&self, kind: GuardKind) -> Option<GuardInfo> {
        self.entries
            .iter()
            .flatten()
            .find(|info| info.kind == kind)
            .copied()
    }
}

static REGISTRY: SpinLock<Registry> = SpinLock::new(Registry::new());

/// Physical address of cached table `index`.
fn table_physical(index: usize) -> u64 {
    // SAFETY: `index` is inside `TABLE_MEMORY`; `addr_of!` forms no reference
    // to the static.
    unsafe { core::ptr::addr_of!(TABLE_MEMORY[index]) as *const u8 as u64 - KERNEL_BASE }
}

/// Normalises a stack address to the physical frame the page tables describe.
///
/// Task and boot stacks come from the frame allocator as identity addresses
/// (already physical). An AP stack lives in the kernel image's `.bss`, so its
/// guard and the `CR2` of a fault there are higher-half virtual addresses; the
/// page-table walk always needs the physical one. The registry stores physical
/// frames, so a fault through either alias of a stack names the same guard.
fn frame_physical(address: u64) -> Option<u64> {
    let physical = if address >= KERNEL_BASE {
        address - KERNEL_BASE
    } else {
        address
    };
    if physical >= paging::MAX_PHYS {
        return None;
    }
    Some(physical & !(paging::PAGE_SIZE - 1))
}

/// Finds (or installs) the shared 4 KiB table covering the 2 MiB page at
/// `base`. Called with [`TABLES`] held.
fn ensure_table(tables: &mut Tables, base: u64) -> Option<u64> {
    if let Some(index) = tables.find(base) {
        return Some(table_physical(index));
    }
    let index = tables.claim(base)?;
    let physical = table_physical(index);
    // SAFETY: the table was just reserved in `.bss`, is 4 KiB aligned, is not
    // in use for anything else, and `base` is inside the identity map.
    if !unsafe { paging::split_huge_page(base, physical) } {
        tables.count -= 1;
        return None;
    }
    Some(physical)
}

/// Makes `frame` a not-present guard page.
///
/// Returns `false` when the address is not a frame, the table cache or registry
/// is full, or the frame is already guarded. The caller must have exclusive
/// ownership of the frame (freshly allocated, or a static `.bss` page that is
/// not live yet).
pub fn guard(frame: u64, kind: GuardKind, who: u64) -> bool {
    if frame & (paging::PAGE_SIZE - 1) != 0 {
        return false;
    }
    let Some(frame) = frame_physical(frame) else {
        return false;
    };
    let base = frame & !(paging::HUGE_SIZE - 1);
    let index = ((frame - base) / paging::PAGE_SIZE) as usize;

    let mut tables = TABLES.lock();
    let mut registry = REGISTRY.lock();
    if registry.find(frame).is_some() {
        return false;
    }
    let Some(slot) = registry.free() else {
        return false;
    };
    if ensure_table(&mut tables, base).is_none() {
        return false;
    }
    if !paging::set_pte_present(base, index, false) {
        return false;
    }
    registry.entries[slot] = Some(GuardInfo { frame, kind, who });
    drop(registry);
    drop(tables);

    crate::smp::flush_all();
    true
}

/// Restores `frame` as an ordinary present page and frees its registry slot.
///
/// Must be called before the frame is returned to the frame allocator; a frame
/// freed while still unmapped faults on first touch in its next life.
/// Returns `false` when `frame` is not a known guard.
pub fn unguard(frame: u64) -> bool {
    let Some(frame) = frame_physical(frame) else {
        return false;
    };
    let base = frame & !(paging::HUGE_SIZE - 1);
    let index = ((frame - base) / paging::PAGE_SIZE) as usize;

    // Same lock order as `guard`: tables, then registry.
    let _tables = TABLES.lock();
    let mut registry = REGISTRY.lock();
    let Some(slot) = registry.find(frame) else {
        return false;
    };
    if !paging::set_pte_present(base, index, true) {
        return false;
    }
    registry.entries[slot] = None;
    drop(registry);
    drop(_tables);

    crate::smp::flush_all();
    true
}

/// True when `frame` is currently a guard page.
pub fn is_guarded(frame: u64) -> bool {
    let Some(frame) = frame_physical(frame) else {
        return false;
    };
    REGISTRY.lock().find(frame).is_some()
}

/// Names the guard at `address` (a `CR2` value), if any.
///
/// The fault handler prints the result; `None` means an ordinary fault (or a
/// fault inside the guard code itself, which holds the registry lock and is
/// already fatal), so the handler still prints CR2 and halts. `try_lock` keeps
/// the reporter from waiting on a lock its own interrupted code may hold.
pub fn describe(address: u64) -> Option<GuardInfo> {
    let frame = frame_physical(address)?;
    let registry = REGISTRY.try_lock()?;
    registry
        .entries
        .iter()
        .flatten()
        .find(|info| info.frame == frame)
        .copied()
}

/// Number of cached 4 KiB tables (the `tables M` boot self-check).
pub fn table_count() -> usize {
    TABLES.lock().count
}

/// Read back one installed guard from the page tables.
///
/// Checks the two properties the boot self-check relies on: the guard entry is
/// not present, and the page immediately above it (the first stack page) is.
pub fn check(frame: u64) -> bool {
    let Some(frame) = frame_physical(frame) else {
        return false;
    };
    paging::pte_present(frame) == Some(false)
        && paging::pte_present(frame + paging::PAGE_SIZE) == Some(true)
}

/// The boot core's guard frame, if [`install_guarded`] ran.
pub fn boot_guard() -> Option<GuardInfo> {
    REGISTRY.lock().first_of(GuardKind::BootCore)
}

/// The installed boot stack, all addresses as the frame allocator returned them.
#[derive(Debug, Clone, Copy)]
pub struct BootStack {
    /// First byte of the not-present guard frame.
    pub guard: u64,
    /// One past the last usable stack byte; the initial RSP.
    pub top: u64,
    /// True when the guard is also visible through the higher-half alias.
    pub aliased: bool,
}

/// Installs the guard below the boot core's post-boot stack.
///
/// # Safety
/// Must be called exactly once, after the frame allocator reserved `guard` and
/// the `STACK_FRAMES` frames above it, and before the boot stack is abandoned.
/// `guard` must be 4 KiB aligned, below 4 GiB, and not currently in use.
pub unsafe fn install_guarded(frame: u64) -> BootStack {
    debug_assert_eq!(frame & (paging::PAGE_SIZE - 1), 0);

    if !guard(frame, GuardKind::BootCore, 0) {
        println!("stack: cannot install the guard at {frame:#x}; halting");
        arch_x86_64::halt();
    }
    if !check(frame) {
        println!("stack: guard at {frame:#x} did not install; halting");
        arch_x86_64::halt();
    }

    BootStack {
        guard: frame,
        top: frame
            + (arch_x86_64::stack::GUARD_FRAMES + arch_x86_64::stack::STACK_FRAMES) as u64
                * paging::PAGE_SIZE,
        aliased: frame < (1 << 30),
    }
}

/// Counts live guards of one kind; used by the boot self-check line.
pub fn count_of(kind: GuardKind) -> usize {
    let registry = REGISTRY.lock();
    registry
        .entries
        .iter()
        .flatten()
        .filter(|info| info.kind == kind)
        .count()
}

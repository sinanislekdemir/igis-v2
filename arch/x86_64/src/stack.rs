//! Guarded post-boot kernel stack (Q42, ADR 0006).
//!
//! Early boot runs on the stack in `.boot`, which sits directly above the boot
//! page tables. `kmain` used to keep compiling applications there, and twice a
//! recursive compile overflowed into the page tables (16 → 64 → 128 KiB). That
//! failure mode is silent corruption, so everything after the frame allocator
//! is ready runs on a stack with a not-present guard page below it.
//!
//! [`install_guarded`] takes `STACK_FRAMES + 1` contiguous frames from the
//! frame allocator. The lowest frame becomes the guard; the upper
//! `STACK_FRAMES` are the stack. The boot page tables map memory with 2 MiB
//! pages, so the 2 MiB page that contains the guard is split into 4 KiB PTEs:
//!
//! - a static 4 KiB page table in `.bss` (below `_kernel_end`, so the frame
//!   allocator already reserved it);
//! - all 512 entries present and writable, preserving access for everything
//!   else in that 2 MiB range;
//! - the guard frame's PTE cleared, so a write there is a `#PF`;
//! - the covering page-directory entry pointed at the new table, `PS` clear;
//! - CR3 reloaded to flush the TLB (and the paging-structure caches).
//!
//! The higher-half alias is shared, not duplicated: `PML4[511] → PDPT1[510] →
//! PD0` maps the first GiB, so for a guard below 1 GiB both the identity and
//! `0xFFFFFFFF80000000 + guard` views use the same PD entry and the same new
//! page table. Above 1 GiB there is no higher-half alias of the guard at all,
//! so it is equally absent. [`KernelStack::aliased`] reports which case holds.
//!
//! A guard hit is a normal page fault. `#PF` runs on its own IST stack (see
//! `gdt`/`idt`), because the exception frame cannot be pushed on the exhausted
//! stack; the existing handler prints CR2 (the guard address), CR3 and the
//! registers and halts. The double-fault IST remains the backstop.
//!
//! Per-task stacks are not guarded here. Task regions are adjacent frames and
//! guarding them is a separate memory-model change; the interpreter's own
//! frames still come from the declared task stack (spec 0002, H2 future work).

use core::arch::asm;
use core::ptr::addr_of_mut;

use crate::KERNEL_BASE;

/// Frames above the guard. 64 frames = 256 KiB, chosen from the measured
/// compiler cost: the worst nesting level is roughly 5.6 KiB of stack, and the
/// compiler's fixed limit is 32 levels, so the deepest accepted source needs
/// about 190 KiB. The remainder is headroom for the scheduler and host paths
/// (the existing `shell` uses about 70 KiB at 15 levels).
pub const STACK_FRAMES: usize = 64;

/// The single guard frame below the stack.
pub const GUARD_FRAMES: usize = 1;

/// Bytes usable as stack.
pub const STACK_BYTES: u64 = STACK_FRAMES as u64 * 4096;

/// One 4 KiB page table. `#[repr(align(4096))]` is what makes it a valid
/// paging-structure page; it lives in `.bss` and is written once.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut GUARD_PT: PageTable = PageTable([0; 512]);

extern "C" {
    /// `pd0` from `boot.s`: the first page directory, identity-mapping the
    /// first GiB with 2 MiB pages. It is linked at its physical address and is
    /// reachable through the identity map.
    static pd0: u8;
}

/// The installed stack's bounds, all physical/identity addresses.
#[derive(Debug, Clone, Copy)]
pub struct KernelStack {
    /// First byte of the not-present guard frame.
    pub guard: u64,
    /// One past the last usable stack byte; the initial RSP.
    pub top: u64,
    /// True when the guard is also visible through the higher-half alias.
    pub aliased: bool,
}

/// Splits the guard's 2 MiB page and returns the new stack top.
///
/// # Safety
/// Must be called exactly once, after the frame allocator reserved `guard`
/// (and the frames above it) and before the boot stack is abandoned. `guard`
/// must be 4 KiB aligned, below 4 GiB, and not currently in use.
pub unsafe fn install_guarded(guard: u64) -> KernelStack {
    debug_assert_eq!(guard & 0xfff, 0);
    debug_assert!(guard < MAX_PHYS_STACK_GUARD);

    let base = guard & !0x1f_ffff;
    let guard_index = ((guard - base) >> 12) as usize;

    // The page directory that covers this 2 MiB region. `pd0..pd3` are
    // contiguous, one per GiB (boot.s).
    // SAFETY: `pd0` is the first of four page directories in `.boot`; the
    // index is bounded by the 4 GiB identity map.
    let pd0_phys = core::ptr::addr_of!(pd0) as u64;
    let directory = (pd0_phys + (guard >> 30) * 4096) as *mut u64;
    let pde_index = ((guard >> 21) & 511) as usize;

    // Fill the table with present+writable 4 KiB mappings for the same 2 MiB
    // range the huge page covered, then clear only the guard.
    let table = addr_of_mut!(GUARD_PT) as *mut u64;
    for index in 0..512u64 {
        core::ptr::write_volatile(table.add(index as usize), (base + index * 4096) | 0x3);
    }
    core::ptr::write_volatile(table.add(guard_index), 0);

    let table_phys = table as u64 - KERNEL_BASE;
    core::ptr::write_volatile(directory.add(pde_index), table_phys | 0x3);

    // CR3 reload flushes the TLB and the paging-structure caches; the asm
    // without `nomem` is also the compiler barrier for the table writes.
    let cr3: u64;
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    asm!("mov cr3, {}", in(reg) cr3, options(nostack));

    KernelStack {
        guard,
        top: guard + (GUARD_FRAMES + STACK_FRAMES) as u64 * 4096,
        aliased: guard < (1 << 30),
    }
}

/// Switches to `top` once and enters `entry(mb2_info)`; never returns.
///
/// SysV ABI: `top` is 16-byte aligned, `call` pushes the return address, and
/// the callee sees the expected `rsp % 16 == 8`. The old stack is abandoned
/// deliberately. Interrupts stay enabled; a timer tick between the `mov rsp`
/// and the `call` pushes onto the new, already-mapped stack.
///
/// # Safety
/// `top` must be the top of a mapped, writable stack; `entry` must never return.
pub unsafe fn switch_to(top: u64, entry: extern "C" fn(u64) -> !, mb2_info: u64) -> ! {
    debug_assert_eq!(top & 0xf, 0);
    asm!(
        "mov rsp, {top}",
        "xor ebp, ebp",
        "call {entry}",
        top = in(reg) top,
        entry = in(reg) entry as usize,
        in("rdi") mb2_info,
        options(noreturn),
    )
}

/// Highest physical address the identity map can guard (exclusive).
pub const MAX_PHYS_STACK_GUARD: u64 = 4 * 1024 * 1024 * 1024;

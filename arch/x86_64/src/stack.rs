//! The boot core's guarded post-boot kernel stack (Q42, ADR 0006).
//!
//! Early boot runs on the stack in `.boot`, which sits directly above the boot
//! page tables. `kmain` used to keep compiling applications there, and twice a
//! recursive compile overflowed into the page tables (16 → 64 → 128 KiB). That
//! failure mode is silent corruption, so everything after the frame allocator
//! is ready runs on a stack with a not-present guard page below it.
//!
//! The allocation and the page-table work live in `kernel::guard`, which owns
//! the shared split-table cache, the guard registry and the cross-core TLB
//! shootdown. This module only provides the size policy, the frame layout and
//! the switch to the new stack:
//!
//! - `STACK_FRAMES + 1` contiguous frames are taken from the frame allocator;
//! - the lowest frame is the guard (unmapped by `kernel::guard`);
//! - the upper [`STACK_FRAMES`] frames are the stack;
//! - [`switch_to`] abandons the old stack and enters the continuation.
//!
//! A guard hit is a normal page fault. `#PF` runs on its own IST stack (see
//! `gdt`/`idt`), because the exception frame cannot be pushed on the exhausted
//! stack; the handler names the guard and halts.
//!
//! Task stacks have their own guard frame installed by `kernel::guard` too
//! (spec 0002 H2, spec 0004 C6); see `kernel/src/task.rs`.

use core::arch::asm;

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

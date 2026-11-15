//! Application-processor bring-up: the trampoline upload and per-core stacks
//! (spec 0002 "Boot", spec 0007 "SMP bring-up").
//!
//! Discovery is by attempt: the kernel has no ACPI/MADT parser, so the boot
//! core copies [`ap.s`](../../x86_64/src/ap.s) next to a parameter page in
//! reserved memory below 1 MiB and sends INIT-SIPI-SIPI to APIC ids
//! `1..MAX_CORES`. An id that never reports stays offline; the machine boots
//! with the cores it has.
//!
//! Nothing here allocates. The per-core stacks are static `.bss`, so an AP can
//! start within a few microseconds of its SIPI and needs no allocator.
//!
//! Each [`ApStack`] is a not-present guard frame followed by the stack body
//! (ADR 0006, spec 0002 H2). The boot core clears the guard's PTE — through
//! `kernel::guard` over [`crate::paging`] — before the SIPIs are sent; the
//! trampoline and the TSS use the top of the body, never the guard.

use core::ptr::addr_of;

/// Largest number of cores the kernel supports. Core index 0 is the boot core;
/// APs take `1..MAX_CORES` as they come up. The index is internal: the APIC id
/// is the identity software sees (`syslib.cpu_id`).
pub const MAX_CORES: usize = 8;

/// Kernel stack an AP runs on, per core. Static; the boot core keeps its
/// guarded 256 KiB stack (`stack.rs`), and every AP stack has its own guard
/// frame ahead of the body (below).
///
/// It must cover the compiler: every core services the request queue before a
/// claim (spec 0007, "Services and task creation"), so any core can compile an
/// application. The compiler's bounded recursion costs up to ~200 KiB at the
/// 32-level limit (Q42), and a `stdlib`-using application already needs ~60 KiB
/// on a release build. A 64 KiB stack overflowed into the neighbouring `.bss`
/// (the next core's stack) with no guard to stop it, so the size matches the
/// boot core's stack instead.
pub const AP_STACK_BYTES: usize = 256 * 1024;

/// Physical page the trampoline is copied to. Must be 4 KiB aligned and below
/// 1 MiB: the SIPI vector is `AP_TRAMPOLINE >> 12` and firmware/real-mode code
/// can only name addresses below 1 MiB.
pub const AP_TRAMPOLINE: u64 = 0x8000;

/// Physical page the trampoline reads its parameters from.
pub const AP_PARAMETERS: u64 = 0x9000;

/// PML4 physical address, written by the boot core.
const PARAM_PML4: usize = 0x00;
/// 64-bit kernel entry point, called with the core index in `rdi`.
const PARAM_ENTRY: usize = 0x08;
/// `MAX_CORES` stack tops.
const PARAM_STACKS: usize = 0x10;
/// Atomic next-index counter, starts at 1.
const PARAM_NEXT: usize = 0x50;

extern "C" {
    static ap_start: u8;
    static ap_end: u8;
    static pml4: u8;
}

/// One AP kernel stack: a not-present guard frame, then the body. `repr(C)`
/// fixes the guard at offset 0 and the body immediately above it; the runtime
/// addresses are taken with `addr_of!`, never by forming a reference.
#[repr(C, align(4096))]
struct ApStack {
    guard: [u8; 4096],
    body: [u8; AP_STACK_BYTES],
}

/// One kernel stack per core, including the boot core's unused entry (index 0
/// keeps the table trivially indexable by core index).
static mut AP_STACKS: [ApStack; MAX_CORES] = [const {
    ApStack {
        guard: [0; 4096],
        body: [0; AP_STACK_BYTES],
    }
}; MAX_CORES];

/// The vector a SIPI must carry to start an AP at [`AP_TRAMPOLINE`].
pub const fn trampoline_vector() -> u8 {
    (AP_TRAMPOLINE >> 12) as u8
}

/// The guard frame below `index`'s stack, 4 KiB aligned.
///
/// The boot core makes it not present before the SIPIs are sent, so an AP stack
/// overflow is a `#PF` on the guard instead of a silent write into the previous
/// core's stack. Index 0 is the boot core's unused static entry; its real stack
/// comes from the frame allocator.
pub fn guard_frame(index: usize) -> u64 {
    // SAFETY: `addr_of!` forms no reference; the address of a static is stable
    // for the lifetime of the system, and `index < MAX_CORES` at both call
    // sites (the AP entry and the boot core's bring-up).
    unsafe { addr_of!(AP_STACKS[index].guard) as *const u8 as u64 }
}

/// Top of the AP kernel stack body for `index`, 16-byte aligned.
pub fn stack_top(index: usize) -> u64 {
    // SAFETY: as above; `addr_of!` does not materialise a `&mut`.
    let base = unsafe { addr_of!(AP_STACKS[index].body) as *const u8 as u64 };
    (base + AP_STACK_BYTES as u64) & !0xf
}

/// Copies the trampoline and fills the parameter page.
///
/// Returns `false` when the blob does not fit its page (a build-time defect, so
/// the caller treats it as "boot with the boot core only").
///
/// # Safety
/// Must be called once, after paging is on and before the first SIPI. The
/// physical pages at [`AP_TRAMPOLINE`] and [`AP_PARAMETERS`] must be reserved
/// (the frame allocator reserves the low 1 MiB) and are overwritten.
pub unsafe fn upload(entry: extern "C" fn(u64) -> !) -> bool {
    let start = addr_of!(ap_start) as u64;
    let end = addr_of!(ap_end) as u64;
    let len = end.saturating_sub(start);
    if len == 0 || len > 4096 {
        return false;
    }

    // The blob is linked in the higher half; both views are mapped.
    let source = start as *const u8;
    let target = AP_TRAMPOLINE as *mut u8;
    for offset in 0..len as usize {
        core::ptr::write_volatile(
            target.add(offset),
            core::ptr::read_volatile(source.add(offset)),
        );
    }

    let parameters = AP_PARAMETERS as *mut u8;
    for offset in 0..4096usize {
        core::ptr::write_volatile(parameters.add(offset), 0);
    }
    let write_u64 = |offset: usize, value: u64| {
        core::ptr::write_volatile((AP_PARAMETERS as *mut u8).add(offset) as *mut u64, value);
    };
    let write_u32 = |offset: usize, value: u32| {
        core::ptr::write_volatile((AP_PARAMETERS as *mut u8).add(offset) as *mut u32, value);
    };

    // `pml4` lives in `.boot`, which is linked at its physical address.
    write_u64(PARAM_PML4, addr_of!(pml4) as u64);
    write_u64(PARAM_ENTRY, entry as u64);
    for index in 0..MAX_CORES {
        write_u64(PARAM_STACKS + index * 8, stack_top(index));
    }
    write_u32(PARAM_NEXT, 1);
    true
}

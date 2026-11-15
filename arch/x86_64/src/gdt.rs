//! Global descriptor table and the per-core task state segments.
//!
//! The boot stub installs a minimal GDT so it can enter long mode. This module
//! replaces it with the kernel's GDT: null, kernel code, kernel data, and one
//! 64-bit TSS per core. Every core loads the same table and `ltr`s its own TSS
//! descriptor, so the double-fault and page-fault IST stacks are per core: an
//! AP that overflows its stack must not try to reuse the boot core's IST.
//!
//! A page fault needs its own stack because the fault may be a kernel stack
//! overflow: the exception frame cannot be pushed on the stack that just ran
//! out. The guard stack design is in `stack.rs` (Q42); the AP stacks in `smp`
//! are unguarded (per-task and per-core guards are future work).

use core::arch::asm;
use core::mem::size_of;

use crate::smp::MAX_CORES;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;

/// First GDT index that holds a TSS descriptor (two entries each).
const FIRST_TSS_INDEX: usize = 3;
const GDT_ENTRIES: usize = FIRST_TSS_INDEX + 2 * MAX_CORES;

/// Bytes of double-fault (IST1) and page-fault (IST2) stack per core.
const IST_BYTES: usize = 16 * 1024;

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Self {
        Tss {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: 0,
        }
    }
}

#[repr(C, align(16))]
struct Stack([u8; IST_BYTES]);

static mut TSS: [Tss; MAX_CORES] = [const { Tss::new() }; MAX_CORES];
static mut IST_STACKS: [[Stack; 2]; MAX_CORES] =
    [const { [Stack([0; IST_BYTES]), Stack([0; IST_BYTES])] }; MAX_CORES];

#[repr(C, align(8))]
struct Gdt([u64; GDT_ENTRIES]);

static mut GDT: Gdt = Gdt([0; GDT_ENTRIES]);

fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let limit = limit as u64;
    let mut low = 0u64;
    low |= limit & 0xFFFF;
    low |= (base & 0xFFFF) << 16;
    low |= ((base >> 16) & 0xFF) << 32;
    low |= 0x89 << 40; // present, DPL 0, system, type 9 (available 64-bit TSS)
    low |= ((limit >> 16) & 0xF) << 48;
    low |= ((base >> 24) & 0xFF) << 56;
    (low, base >> 32)
}

/// The `ltr` selector for `core`'s TSS.
pub fn tss_selector(core: usize) -> u16 {
    ((FIRST_TSS_INDEX + 2 * core) as u16) * 8
}

/// Fills the GDT and every TSS, then loads the table on the calling core.
///
/// # Safety
/// Must be called exactly once during boot, before any code uses the TSS or an
/// AP loads the table.
pub unsafe fn init() {
    for core in 0..MAX_CORES {
        let tss = core::ptr::addr_of_mut!(TSS[core]);
        (*tss).iomap_base = size_of::<Tss>() as u16;
        (*tss).rsp0 = crate::smp::stack_top(core);

        let ist = core::ptr::addr_of!(IST_STACKS[core]) as *const u8 as u64;
        let double_fault = ist + IST_BYTES as u64;
        let page_fault = double_fault + IST_BYTES as u64;
        (*tss).ist[0] = double_fault;
        (*tss).ist[1] = page_fault;

        let gdt = core::ptr::addr_of_mut!(GDT);
        let index = FIRST_TSS_INDEX + 2 * core;
        let (low, high) = tss_descriptor(tss as u64, (size_of::<Tss>() - 1) as u32);
        (*gdt).0[index] = low;
        (*gdt).0[index + 1] = high;
    }

    let gdt = core::ptr::addr_of_mut!(GDT);
    (*gdt).0[0] = 0;
    (*gdt).0[1] = 0x00AF_9A00_0000_FFFF; // kernel code
    (*gdt).0[2] = 0x00CF_9200_0000_FFFF; // kernel data

    load(0);
}

/// Loads the shared GDT and this core's TSS.
///
/// # Safety
/// [`init`] must have run. Interrupts must be disabled (the boot core calls
/// this before `sti`; an AP calls it before its first IDT load).
pub unsafe fn load(core: usize) {
    let gdt = core::ptr::addr_of!(GDT);
    let pointer = DescriptorPointer {
        limit: (size_of::<Gdt>() - 1) as u16,
        base: gdt as u64,
    };

    asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    asm!(
        "mov ax, 0x10",
        "mov ds, ax",
        "mov es, ax",
        "mov ss, ax",
        "mov fs, ax",
        "mov gs, ax",
        out("ax") _,
        options(nostack),
    );
    let selector = tss_selector(core);
    asm!("mov ax, {selector:x}", "ltr ax", selector = in(reg) selector, out("ax") _, options(nostack));
}

/// Sets `RSP0` for `core`: the stack the CPU switches to if ring 3 ever enters
/// ring 0. Nothing runs at ring 3 today, but the field must describe the core's
/// kernel stack rather than another core's spent one.
pub fn set_kernel_stack(core: usize, top: u64) {
    // SAFETY: `core` < `MAX_CORES` (the caller's index), and a core only writes
    // its own TSS. `Tss` is packed, so the compiler emits an unaligned store.
    unsafe { (*core::ptr::addr_of_mut!(TSS[core])).rsp0 = top };
}

pub fn task_register() -> u16 {
    let selector: u16;
    // SAFETY: `str` is readable at ring 0 and has no side effects.
    unsafe { asm!("str {0:x}", out(reg) selector, options(nomem, nostack, preserves_flags)) };
    selector
}

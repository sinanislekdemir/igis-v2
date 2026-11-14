//! Global descriptor table and the task state segment.
//!
//! The boot stub installs a minimal GDT so it can enter long mode. This module
//! replaces it with the kernel's GDT: null, kernel code, kernel data, and a
//! 64-bit TSS with IST stacks for the double fault and page fault. A page
//! fault needs its own stack because the fault may be a kernel stack overflow:
//! the exception frame cannot be pushed on the stack that just ran out. The
//! guard stack design is in `stack.rs` (Q42).

use core::arch::asm;
use core::mem::size_of;
use core::ptr::addr_of_mut;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const TSS_SELECTOR: u16 = 0x18;

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

#[repr(C, align(16))]
struct Stack([u8; 16 * 1024]);

static mut TSS: Tss = Tss {
    reserved0: 0,
    rsp0: 0,
    rsp1: 0,
    rsp2: 0,
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: 0,
};

static mut DOUBLE_FAULT_STACK: Stack = Stack([0; 16 * 1024]);
/// Page-fault IST. A guard-page hit has already consumed the kernel stack, so
/// the hardware frame must go somewhere else.
static mut PAGE_FAULT_STACK: Stack = Stack([0; 16 * 1024]);

#[repr(C, align(8))]
struct Gdt([u64; 5]);

static mut GDT: Gdt = Gdt([
    0,
    0x00AF_9A00_0000_FFFF, // kernel code
    0x00CF_9200_0000_FFFF, // kernel data
    0,                     // TSS low (filled in `init`)
    0,                     // TSS high
]);

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

/// Install the kernel GDT and load the TSS. Interrupts must be disabled.
///
/// # Safety
/// Must be called exactly once during boot, before any code uses the TSS.
pub unsafe fn init() {
    let tss = addr_of_mut!(TSS);
    (*tss).iomap_base = size_of::<Tss>() as u16;

    let df_top = addr_of_mut!(DOUBLE_FAULT_STACK) as u64 + size_of::<Stack>() as u64;
    (*tss).ist[0] = df_top;

    let pf_top = addr_of_mut!(PAGE_FAULT_STACK) as u64 + size_of::<Stack>() as u64;
    (*tss).ist[1] = pf_top;

    let gdt = addr_of_mut!(GDT);
    let (low, high) = tss_descriptor(tss as u64, (size_of::<Tss>() - 1) as u32);
    (*gdt).0[3] = low;
    (*gdt).0[4] = high;

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
    asm!("mov ax, 0x18", "ltr ax", out("ax") _, options(nostack));
}

pub fn task_register() -> u16 {
    let selector: u16;
    // SAFETY: `str` is readable at ring 0 and has no side effects.
    unsafe { asm!("str {0:x}", out(reg) selector, options(nomem, nostack, preserves_flags)) };
    selector
}

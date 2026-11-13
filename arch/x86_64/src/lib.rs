#![no_std]

//! x86_64 boot path and platform helpers.
//!
//! The 32-bit stub, the page tables and the long-mode transition live in
//! `boot.s`. Everything above the transition is ordinary Rust.

core::arch::global_asm!(include_str!("boot.s"));
core::arch::global_asm!(include_str!("interrupts.s"));

pub mod apic;
pub mod gdt;
pub mod idt;
pub mod keyboard;
pub mod port;
pub mod serial;

/// Virtual base of the kernel image; physical addresses map through this offset.
pub const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Wait for the next interrupt, then return with interrupts enabled.
///
/// `sti` takes effect after the following instruction, so `sti; hlt` has no
/// window in which an interrupt can arrive between the two (spec 0007, idle).
pub fn wait_for_interrupt() {
    // SAFETY: `sti`/`hlt` are legal at ring 0; the kernel is idle and only an
    // interrupt (timer or IPI) can make progress.
    unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack, preserves_flags)) }
}

/// Stop this core forever. Interrupts are disabled.
pub fn halt() -> ! {
    loop {
        // SAFETY: `cli` is legal at ring 0 and this core has nothing left to do.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack, preserves_flags)) }
    }
}

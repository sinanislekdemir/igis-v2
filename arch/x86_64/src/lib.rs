#![no_std]

//! x86_64 boot path and platform helpers.
//!
//! The 32-bit stub, the page tables and the long-mode transition live in
//! `boot.s`. Everything above the transition is ordinary Rust.

core::arch::global_asm!(include_str!("boot.s"));

pub mod serial;

/// Stop this core forever. Interrupts are disabled.
pub fn halt() -> ! {
    loop {
        // SAFETY: `cli` is legal at ring 0 and this core has nothing left to do.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack, preserves_flags)) }
    }
}

//! x86 port I/O.

use core::arch::asm;

/// # Safety
/// The caller must know that `port` is safe to write and that no other code
/// depends on its current value.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

/// # Safety
/// The caller must know that reading `port` has no side effects it cannot cope
/// with.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

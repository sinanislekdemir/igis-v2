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

/// # Safety
/// The caller must know that `port` is safe to write and that no other code
/// depends on its current value.
#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
}

/// # Safety
/// The caller must know that reading `port` has no side effects it cannot cope
/// with.
#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack, preserves_flags));
    value
}

/// # Safety
/// The caller must know that `port` is safe to write and that no other code
/// depends on its current value.
#[inline]
pub unsafe fn outl(port: u16, value: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}

/// # Safety
/// The caller must know that reading `port` has no side effects it cannot cope
/// with.
#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    value
}

/// # Safety
/// The caller must know that `port` is safe to read and that `out` is valid
/// for `out.len()` 16-bit writes by the device.
#[inline]
pub unsafe fn insw(port: u16, out: &mut [u16]) {
    asm!(
        "rep insw",
        in("dx") port,
        in("rdi") out.as_mut_ptr(),
        in("rcx") out.len(),
        options(nostack, preserves_flags)
    );
}

/// # Safety
/// The caller must know that `port` is safe to write and that `data` remains
/// valid for the duration of the transfer.
#[inline]
pub unsafe fn outsw(port: u16, data: &[u16]) {
    asm!(
        "rep outsw",
        in("dx") port,
        in("rsi") data.as_ptr(),
        in("rcx") data.len(),
        options(nostack, preserves_flags)
    );
}

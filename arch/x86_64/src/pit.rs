//! PIT channel 2 as a one-shot delay.
//!
//! The APIC timer calibration already uses channel 2 in mode 0; the same
//! mechanism gives an interrupt-independent busy wait for the bring-up
//! sequence (the INIT-SIPI-SIPI spacing must not depend on the tick, which only
//! starts after the boot core's timer is programmed and which APs do not get).

use core::hint::spin_loop;

use crate::port::{inb, outb};

/// PIT input frequency (1.193182 MHz).
const PIT_HZ: u64 = 1_193_182;

/// Busy-waits for about `us` microseconds.
///
/// # Safety
/// The PIT channel 2 gate (port 0x61) must not be owned by another driver. The
/// kernel has no other channel-2 user, and the keyboard does not touch it.
pub unsafe fn delay_us(us: u32) {
    let mut control = inb(0x61);
    control = (control & !0x02) & !0x01; // speaker off, gate low
    outb(0x61, control);

    outb(0x43, 0xB0); // channel 2, mode 0 (one-shot), lobyte/hibyte, binary
    let count = ((us as u64 * PIT_HZ) / 1_000_000).clamp(1, 0xFFFF) as u16;
    outb(0x42, count as u8);
    outb(0x42, (count >> 8) as u8);

    control |= 0x01; // gate high starts the count
    outb(0x61, control);

    while inb(0x61) & 0x20 == 0 {
        spin_loop();
    }
}

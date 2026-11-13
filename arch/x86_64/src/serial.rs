//! COM1 serial output. The only console that works before the framebuffer is up.

use core::fmt;

use crate::port::{inb, outb};

const COM1: u16 = 0x3F8;

pub fn init() {
    // SAFETY: COM1 is the conventional 16550 UART port; writing its registers is
    // legal at ring 0 and there is no other owner.
    unsafe {
        outb(COM1 + 1, 0x00); // no interrupts
        outb(COM1 + 3, 0x80); // DLAB
        outb(COM1 + 0, 0x01); // divisor: 115200 baud
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8N1
        outb(COM1 + 2, 0xC7); // FIFO on, clear, 14-byte threshold
        outb(COM1 + 4, 0x03); // DTR + RTS
    }
}

fn ready() -> bool {
    // SAFETY: reading the line status register of COM1.
    unsafe { inb(COM1 + 5) & 0x20 != 0 }
}

pub fn write_byte(byte: u8) {
    if byte == b'\n' {
        write_byte_raw(b'\r');
    }
    write_byte_raw(byte);
}

fn write_byte_raw(byte: u8) {
    while !ready() {
        core::hint::spin_loop();
    }
    // SAFETY: the transmitter is empty, so the byte is accepted.
    unsafe { outb(COM1, byte) }
}

pub fn write_str(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}

/// `core::fmt::Write` adapter so `writeln!` works.
pub struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

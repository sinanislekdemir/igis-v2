//! Polled PS/2 keyboard.
//!
//! The controller is polled from the kernel's service opportunity, not driven
//! by an IRQ. Firmware leaves the device in translated scan-code set 1, so a
//! make/break code arrives as one byte at port `0x60`; the controller status at
//! `0x64` says whether a byte is waiting and whether it came from the mouse
//! (bit 5), which is drained and discarded.
//!
//! The translator keeps Shift, Caps Lock, Ctrl and the `0xE0` extended prefix
//! in one atomic byte. It emits exactly the console byte stream of spec 0005:
//! US ASCII for printable keys, `\n` for Enter, `0x7F` for Backspace, `\t` for
//! Tab and `0x03` for Ctrl+C. Every other key is dropped.

use core::sync::atomic::{AtomicU8, Ordering};

use crate::port::{inb, outb};

const STATUS_PORT: u16 = 0x64;
const DATA_PORT: u16 = 0x60;

const SHIFT: u8 = 1 << 0;
const CAPS: u8 = 1 << 1;
const CTRL: u8 = 1 << 2;
const EXTENDED: u8 = 1 << 3;

/// Modifier state. One core today; the atomic keeps it safe if polling ever
/// moves behind another context.
static STATE: AtomicU8 = AtomicU8::new(0);

/// Enables the first PS/2 port and drains whatever firmware left behind.
///
/// The scan-code set is deliberately not reprogrammed: the firmware already
/// selected translated set 1, which is what [`translate`] expects.
pub fn init() {
    // SAFETY: the i8042 status and data ports are the conventional PS/2 ports;
    // the kernel is the only owner.
    unsafe {
        for _ in 0..100_000 {
            if inb(STATUS_PORT) & 0x02 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        outb(STATUS_PORT, 0xAE); // enable the keyboard interface
        for _ in 0..16 {
            if inb(STATUS_PORT) & 0x01 == 0 {
                break;
            }
            let _ = inb(DATA_PORT);
        }
    }
}

/// Drains controller bytes until one translates to a console byte, or the
/// output buffer is empty. Mouse bytes and non-text keys are discarded.
pub fn try_read_byte() -> Option<u8> {
    let mut state = STATE.load(Ordering::Relaxed);
    let result = drain(&mut state);
    STATE.store(state, Ordering::Relaxed);
    result
}

fn drain(state: &mut u8) -> Option<u8> {
    for _ in 0..32 {
        // SAFETY: reading the i8042 status and data ports.
        let status = unsafe { inb(STATUS_PORT) };
        if status & 0x01 == 0 {
            return None;
        }
        let code = unsafe { inb(DATA_PORT) };
        if status & 0x20 != 0 {
            // Auxiliary (mouse) byte: read to clear the buffer, then ignore.
            continue;
        }
        if let Some(byte) = translate(code, state) {
            return Some(byte);
        }
    }
    None
}

/// Translates one scan-code set 1 byte. Returns `None` for modifiers, releases
/// and dropped keys.
fn translate(code: u8, state: &mut u8) -> Option<u8> {
    if *state & EXTENDED != 0 {
        *state &= !EXTENDED;
        match code {
            0x1D => *state |= CTRL,
            0x9D => *state &= !CTRL,
            0x1C => return Some(b'\n'), // keypad Enter is still Enter
            _ => {}
        }
        return None;
    }
    if code == 0xE0 {
        *state |= EXTENDED;
        return None;
    }
    if code & 0x80 != 0 {
        match code {
            0xAA | 0xB6 => *state &= !SHIFT,
            0x9D => *state &= !CTRL,
            _ => {}
        }
        return None;
    }

    match code {
        0x2A | 0x36 => {
            *state |= SHIFT;
            None
        }
        0x3A => {
            *state ^= CAPS;
            None
        }
        0x1D => {
            *state |= CTRL;
            None
        }
        0x1C => Some(b'\n'),
        0x0E => Some(0x7F),
        0x0F => Some(b'\t'),
        0x39 => Some(b' '),
        0x2E if *state & CTRL != 0 => Some(0x03), // Ctrl+C
        code => {
            if *state & CTRL != 0 {
                // Only Ctrl+C is text in this stream.
                return None;
            }
            printable(code, *state)
        }
    }
}

fn printable(code: u8, state: u8) -> Option<u8> {
    let shift = state & SHIFT != 0;

    let letter = match code {
        0x10 => b'q',
        0x11 => b'w',
        0x12 => b'e',
        0x13 => b'r',
        0x14 => b't',
        0x15 => b'y',
        0x16 => b'u',
        0x17 => b'i',
        0x18 => b'o',
        0x19 => b'p',
        0x1E => b'a',
        0x1F => b's',
        0x20 => b'd',
        0x21 => b'f',
        0x22 => b'g',
        0x23 => b'h',
        0x24 => b'j',
        0x25 => b'k',
        0x26 => b'l',
        0x2C => b'z',
        0x2D => b'x',
        0x2E => b'c',
        0x2F => b'v',
        0x30 => b'b',
        0x31 => b'n',
        0x32 => b'm',
        _ => 0,
    };
    if letter != 0 {
        let upper = shift ^ (state & CAPS != 0);
        return Some(if upper {
            letter.to_ascii_uppercase()
        } else {
            letter
        });
    }

    let (plain, shifted) = match code {
        0x02 => (b'1', b'!'),
        0x03 => (b'2', b'@'),
        0x04 => (b'3', b'#'),
        0x05 => (b'4', b'$'),
        0x06 => (b'5', b'%'),
        0x07 => (b'6', b'^'),
        0x08 => (b'7', b'&'),
        0x09 => (b'8', b'*'),
        0x0A => (b'9', b'('),
        0x0B => (b'0', b')'),
        0x0C => (b'-', b'_'),
        0x0D => (b'=', b'+'),
        0x1A => (b'[', b'{'),
        0x1B => (b']', b'}'),
        0x27 => (b';', b':'),
        0x28 => (b'\'', b'"'),
        0x29 => (b'`', b'~'),
        0x2B => (b'\\', b'|'),
        0x33 => (b',', b'<'),
        0x34 => (b'.', b'>'),
        0x35 => (b'/', b'?'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

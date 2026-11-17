//! Polled PS/2 keyboard: the console byte stream and key events.
//!
//! The controller is polled from the kernel's service opportunity, not driven
//! by an IRQ. Firmware leaves the device in translated scan-code set 1, so a
//! make/break code arrives as one byte at port `0x60`; the controller status at
//! `0x64` says whether a byte is waiting and whether it came from the mouse
//! (bit 5), which is left for the mouse path.
//!
//! One pass feeds two consumers from the same bytes (spec 0010, I15/I18):
//!
//! - the console byte stream of spec 0005: US ASCII for printable keys, `\n`
//!   for Enter, `0x7F` for Backspace, `\t` for Tab and `0x03` for Ctrl+C; every
//!   other key produces no console byte;
//! - a [`KeyEvent`] for every make and break of a recognised key, including
//!   modifiers, arrows and function keys. The event carries a stable key code,
//!   the text byte under the current modifiers, and the modifier state; the
//!   kernel packs it into the `Event` record (spec 0010, I19).
//!
//! The translator keeps Shift, Caps Lock, Ctrl, Alt, the `0xE0` extended prefix
//! and a small skip counter (for the `0xE1` Pause sequence) in one atomic
//! byte.

use core::sync::atomic::{AtomicU8, Ordering};

use abi::key as key_code;

use crate::port::{inb, outb};

const STATUS_PORT: u16 = 0x64;
const DATA_PORT: u16 = 0x60;

const SHIFT: u8 = 1 << 0;
const CAPS: u8 = 1 << 1;
const CTRL: u8 = 1 << 2;
const ALT: u8 = 1 << 3;
const EXTENDED: u8 = 1 << 4;
/// Skip mask for the `0xE1` Pause sequence: bits 5..=7 hold a count 0..=5.
const SKIP_SHIFT: u8 = 5;
const SKIP_MASK: u8 = 0b111 << SKIP_SHIFT;

/// Modifier bits carried by [`KeyEvent`] (spec 0010, I18). The values match
/// `abi::key::MOD_*`.
pub const MOD_SHIFT: u8 = 1 << 0;
/// Control is down.
pub const MOD_CTRL: u8 = 1 << 1;
/// Alt is down.
pub const MOD_ALT: u8 = 1 << 2;
/// Caps Lock is on.
pub const MOD_CAPS: u8 = 1 << 3;

/// One decoded key transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Stable key code: the unshifted ASCII for printable keys (letters are
    /// uppercase), or one of `abi::key`'s `0x100`-range constants.
    pub code: u16,
    /// The ASCII/control byte this key produces under the current modifiers,
    /// or 0 for a non-text key or a release.
    pub text: u8,
    /// Modifier state after the transition (`MOD_*` bits).
    pub modifiers: u8,
    /// True for a make (press), false for a break (release).
    pub pressed: bool,
}

/// What one scancode produced. A byte can produce both a console byte and a key
/// event (Enter, for example), one of them, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded {
    /// The console byte this key would produce under the retired line
    /// discipline. The kernel no longer consumes it (ADR 0037: the keyboard is
    /// events only and serial is the byte stream); the decoder self-check still
    /// pins the historical mapping, and the event's text byte is what a key
    /// consumer uses.
    pub console: Option<u8>,
    /// The key event, if any.
    pub key: Option<KeyEvent>,
}

impl Decoded {
    const NONE: Decoded = Decoded {
        console: None,
        key: None,
    };
}

/// Modifier state. Interrupts do not read it; the atomic keeps it safe if
/// polling ever moves behind another context.
static STATE: AtomicU8 = AtomicU8::new(0);

/// Enables the first PS/2 port and drains whatever firmware left behind.
///
/// The scan-code set is deliberately not reprogrammed: the firmware already
/// selected translated set 1, which is what [`decode`] expects.
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

/// Drains controller bytes and reports what each one produced.
///
/// Aux (mouse) bytes are left for `mouse::poll`; a keyboard byte that produces
/// neither a console byte nor an event is dropped. The callback sees both
/// outputs of one byte in arrival order, and the decoder state is held across
/// the whole drain.
pub fn poll(mut sink: impl FnMut(Decoded)) {
    let mut state = STATE.load(Ordering::Relaxed);
    for _ in 0..64 {
        // SAFETY: reading the i8042 status and data ports; the kernel is the
        // only owner and interrupts never touch them.
        let status = unsafe { inb(STATUS_PORT) };
        if status & 0x01 == 0 {
            break;
        }
        if status & 0x20 != 0 {
            // Auxiliary (mouse) byte: leave it for `mouse::poll`, which drains
            // it in the same service opportunity and leaves keyboard bytes for
            // this path (spec 0010, I15).
            break;
        }
        let code = unsafe { inb(DATA_PORT) };
        let decoded = decode(code, &mut state);
        if decoded != Decoded::NONE {
            sink(decoded);
        }
    }
    STATE.store(state, Ordering::Relaxed);
}

/// Decodes one scan-code set 1 byte. Pure, so the boot self-check exercises the
/// same code as the live path.
pub fn decode(code: u8, state: &mut u8) -> Decoded {
    if *state & SKIP_MASK != 0 {
        *state -= 1 << SKIP_SHIFT;
        return Decoded::NONE;
    }
    if *state & EXTENDED != 0 {
        *state &= !EXTENDED;
        return decode_extended(code, state);
    }
    if code == 0xE0 {
        *state |= EXTENDED;
        return Decoded::NONE;
    }
    if code == 0xE1 {
        // Pause/Break sends five bytes after the prefix; drop them whole so
        // they do not look like Ctrl/NumLock transitions.
        *state = (*state & !SKIP_MASK) | (5 << SKIP_SHIFT);
        return Decoded::NONE;
    }
    if code & 0x80 != 0 {
        return decode_break(code & 0x7F, state);
    }
    decode_press(code, state)
}

/// A make (press) byte without the extended prefix.
fn decode_press(code: u8, state: &mut u8) -> Decoded {
    match code {
        0x2A => {
            *state |= SHIFT;
            return key_event(key_code::LEFT_SHIFT, 0, *state, true);
        }
        0x36 => {
            *state |= SHIFT;
            return key_event(key_code::RIGHT_SHIFT, 0, *state, true);
        }
        0x1D => {
            *state |= CTRL;
            return key_event(key_code::LEFT_CTRL, 0, *state, true);
        }
        0x38 => {
            *state |= ALT;
            return key_event(key_code::LEFT_ALT, 0, *state, true);
        }
        0x3A => {
            *state ^= CAPS;
            return key_event(key_code::CAPS_LOCK, 0, *state, true);
        }
        0x45 => return key_event(key_code::NUM_LOCK, 0, *state, true),
        0x46 => return key_event(key_code::SCROLL_LOCK, 0, *state, true),
        0x01 => return special(key_code::ESCAPE, None, state),
        0x0E => {
            // The console uses DEL; the event names the key.
            return special(key_code::BACKSPACE, Some(0x7F), state);
        }
        0x0F => return special(key_code::TAB, Some(b'\t'), state),
        0x1C => return special(key_code::ENTER, Some(b'\n'), state),
        0x39 => return text_key(u16::from(b' '), b' ', b' ', state),
        0x3B..=0x44 => {
            let index = u16::from(code - 0x3B);
            return special(key_code::F1 + index, None, state);
        }
        0x57 => return special(key_code::F11, None, state),
        0x58 => return special(key_code::F12, None, state),
        0x47 => return special(key_code::HOME, None, state),
        0x48 => return special(key_code::UP, None, state),
        0x49 => return special(key_code::PAGE_UP, None, state),
        0x4B => return special(key_code::LEFT, None, state),
        0x4D => return special(key_code::RIGHT, None, state),
        0x4F => return special(key_code::END, None, state),
        0x50 => return special(key_code::DOWN, None, state),
        0x51 => return special(key_code::PAGE_DOWN, None, state),
        0x52 => return special(key_code::INSERT, None, state),
        0x53 => return special(key_code::DELETE, None, state),
        _ => {}
    }

    if let Some(lower) = letter(code) {
        let upper = lower.to_ascii_uppercase();
        let shifted = *state & SHIFT != 0;
        let caps = *state & CAPS != 0;
        let text = if shifted ^ caps { upper } else { lower };
        return text_key(u16::from(upper), text, lower, state);
    }
    if let Some((plain, shifted)) = punctuation(code) {
        let text = if *state & SHIFT != 0 { shifted } else { plain };
        return text_key(u16::from(plain), text, plain, state);
    }
    Decoded::NONE
}

/// A break (release) byte without the extended prefix. The key code is the
/// stable physical identity; a release carries no text.
fn decode_break(code: u8, state: &mut u8) -> Decoded {
    match code {
        0x2A | 0x36 => {
            *state &= !SHIFT;
            let key = if code == 0x2A {
                key_code::LEFT_SHIFT
            } else {
                key_code::RIGHT_SHIFT
            };
            return key_event(key, 0, *state, false);
        }
        0x1D => {
            *state &= !CTRL;
            return key_event(key_code::LEFT_CTRL, 0, *state, false);
        }
        0x38 => {
            *state &= !ALT;
            return key_event(key_code::LEFT_ALT, 0, *state, false);
        }
        0x3A => return key_event(key_code::CAPS_LOCK, 0, *state, false),
        0x45 => return key_event(key_code::NUM_LOCK, 0, *state, false),
        0x46 => return key_event(key_code::SCROLL_LOCK, 0, *state, false),
        0x01 => return key_event(key_code::ESCAPE, 0, *state, false),
        0x0E => return key_event(key_code::BACKSPACE, 0, *state, false),
        0x0F => return key_event(key_code::TAB, 0, *state, false),
        0x1C => return key_event(key_code::ENTER, 0, *state, false),
        0x39 => return key_event(u16::from(b' '), 0, *state, false),
        0x3B..=0x44 => {
            return key_event(key_code::F1 + u16::from(code - 0x3B), 0, *state, false);
        }
        0x57 => return key_event(key_code::F11, 0, *state, false),
        0x58 => return key_event(key_code::F12, 0, *state, false),
        0x47 => return key_event(key_code::HOME, 0, *state, false),
        0x48 => return key_event(key_code::UP, 0, *state, false),
        0x49 => return key_event(key_code::PAGE_UP, 0, *state, false),
        0x4B => return key_event(key_code::LEFT, 0, *state, false),
        0x4D => return key_event(key_code::RIGHT, 0, *state, false),
        0x4F => return key_event(key_code::END, 0, *state, false),
        0x50 => return key_event(key_code::DOWN, 0, *state, false),
        0x51 => return key_event(key_code::PAGE_DOWN, 0, *state, false),
        0x52 => return key_event(key_code::INSERT, 0, *state, false),
        0x53 => return key_event(key_code::DELETE, 0, *state, false),
        _ => {}
    }

    if let Some(lower) = letter(code) {
        return key_event(u16::from(lower.to_ascii_uppercase()), 0, *state, false);
    }
    if let Some((plain, _)) = punctuation(code) {
        return key_event(u16::from(plain), 0, *state, false);
    }
    Decoded::NONE
}

/// A make byte after the `0xE0` prefix.
fn decode_extended(code: u8, state: &mut u8) -> Decoded {
    let pressed = code & 0x80 == 0;
    let make = code & 0x7F;
    match (make, pressed) {
        (0x1D, true) => {
            *state |= CTRL;
            key_event(key_code::RIGHT_CTRL, 0, *state, true)
        }
        (0x1D, false) => {
            *state &= !CTRL;
            key_event(key_code::RIGHT_CTRL, 0, *state, false)
        }
        (0x38, true) => {
            *state |= ALT;
            key_event(key_code::RIGHT_ALT, 0, *state, true)
        }
        (0x38, false) => {
            *state &= !ALT;
            key_event(key_code::RIGHT_ALT, 0, *state, false)
        }
        (0x1C, true) => special(key_code::ENTER, Some(b'\n'), state),
        (0x1C, false) => key_event(key_code::ENTER, 0, *state, false),
        (0x35, true) => text_key(u16::from(b'/'), b'/', b'/', state),
        (0x35, false) => key_event(u16::from(b'/'), 0, *state, false),
        (0x47, true) => special(key_code::HOME, None, state),
        (0x48, true) => special(key_code::UP, None, state),
        (0x49, true) => special(key_code::PAGE_UP, None, state),
        (0x4B, true) => special(key_code::LEFT, None, state),
        (0x4D, true) => special(key_code::RIGHT, None, state),
        (0x4F, true) => special(key_code::END, None, state),
        (0x50, true) => special(key_code::DOWN, None, state),
        (0x51, true) => special(key_code::PAGE_DOWN, None, state),
        (0x52, true) => special(key_code::INSERT, None, state),
        (0x53, true) => special(key_code::DELETE, None, state),
        (0x47, false) => key_event(key_code::HOME, 0, *state, false),
        (0x48, false) => key_event(key_code::UP, 0, *state, false),
        (0x49, false) => key_event(key_code::PAGE_UP, 0, *state, false),
        (0x4B, false) => key_event(key_code::LEFT, 0, *state, false),
        (0x4D, false) => key_event(key_code::RIGHT, 0, *state, false),
        (0x4F, false) => key_event(key_code::END, 0, *state, false),
        (0x50, false) => key_event(key_code::DOWN, 0, *state, false),
        (0x51, false) => key_event(key_code::PAGE_DOWN, 0, *state, false),
        (0x52, false) => key_event(key_code::INSERT, 0, *state, false),
        (0x53, false) => key_event(key_code::DELETE, 0, *state, false),
        _ => Decoded::NONE,
    }
}

fn key_event(code: u16, text: u8, state: u8, pressed: bool) -> Decoded {
    Decoded {
        console: None,
        key: Some(KeyEvent {
            code,
            text,
            modifiers: modifiers(state),
            pressed,
        }),
    }
}

/// Maps the internal modifier bits to the exported `MOD_*` bits. The two
/// layouts differ deliberately: the state byte keeps room for the extended
/// prefix and the Pause skip count.
fn modifiers(state: u8) -> u8 {
    let mut bits = 0;
    if state & SHIFT != 0 {
        bits |= MOD_SHIFT;
    }
    if state & CTRL != 0 {
        bits |= MOD_CTRL;
    }
    if state & ALT != 0 {
        bits |= MOD_ALT;
    }
    if state & CAPS != 0 {
        bits |= MOD_CAPS;
    }
    bits
}

/// A key that is not text-producing but may still have a console byte (Enter,
/// Backspace, Tab).
fn special(code: u16, console: Option<u8>, state: &u8) -> Decoded {
    Decoded {
        console,
        key: Some(KeyEvent {
            code,
            text: 0,
            modifiers: modifiers(*state),
            pressed: true,
        }),
    }
}

/// A text-producing key: the console byte obeys the old line-discipline rules
/// (Ctrl+C only under Ctrl), while the event carries the real text.
fn text_key(code: u16, text: u8, lower: u8, state: &u8) -> Decoded {
    let event_text = if *state & CTRL != 0 {
        ctrl_text(lower)
    } else {
        text
    };
    let console = if *state & CTRL != 0 {
        if event_text == 0x03 {
            Some(0x03)
        } else {
            None
        }
    } else {
        Some(text)
    };
    Decoded {
        console,
        key: Some(KeyEvent {
            code,
            text: event_text,
            modifiers: modifiers(*state),
            pressed: true,
        }),
    }
}

/// The control code a key produces under Ctrl. Only letters and the bracketing
/// punctuation that has a standard mapping; every other key keeps its text.
fn ctrl_text(lower: u8) -> u8 {
    match lower.to_ascii_lowercase() {
        b'a'..=b'z' => lower.to_ascii_lowercase() & 0x1F,
        b'[' => 0x1B,
        b'\\' => 0x1C,
        b']' => 0x1D,
        b'^' => 0x1E,
        b'_' => 0x1F,
        b'@' => 0x00,
        _ => lower,
    }
}

/// Lower-case letter for a letter key, or `None`.
fn letter(code: u8) -> Option<u8> {
    Some(match code {
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
        _ => return None,
    })
}

/// `(plain, shifted)` for a digit or punctuation key, or `None`.
fn punctuation(code: u8) -> Option<(u8, u8)> {
    Some(match code {
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
    })
}

/// Feeds canned scancodes through [`decode`] so the boot log proves the key
/// layer without a real keyboard (spec 0010, I18). Returns the number of the
/// first failing step, or `Ok(())`.
pub fn decoder_self_check() -> Result<(), usize> {
    #[derive(Clone, Copy)]
    enum Expect {
        /// The byte sequence must produce no key event.
        None,
        /// The byte sequence must produce exactly this key event.
        Is(KeyEvent),
    }

    fn check(state: &mut u8, bytes: &[u8], console: Option<u8>, key: Expect) -> bool {
        let mut got_console = None;
        let mut got_key = None;
        for byte in bytes {
            let decoded = decode(*byte, state);
            if decoded.console.is_some() {
                got_console = decoded.console;
            }
            if decoded.key.is_some() {
                got_key = decoded.key;
            }
        }
        let key_ok = match key {
            Expect::None => got_key.is_none(),
            Expect::Is(want) => got_key == Some(want),
        };
        got_console == console && key_ok
    }

    let shift = KeyEvent {
        code: key_code::LEFT_SHIFT,
        text: 0,
        modifiers: MOD_SHIFT,
        pressed: true,
    };
    let caps = KeyEvent {
        code: key_code::CAPS_LOCK,
        text: 0,
        modifiers: MOD_CAPS,
        pressed: true,
    };
    let ctrl = KeyEvent {
        code: key_code::LEFT_CTRL,
        text: 0,
        modifiers: MOD_CTRL,
        pressed: true,
    };

    let cases: &[(&[u8], Option<u8>, Expect)] = &[
        // 1. 'a' press and release.
        (
            &[0x1E],
            Some(b'a'),
            Expect::Is(KeyEvent {
                code: u16::from(b'A'),
                text: b'a',
                modifiers: 0,
                pressed: true,
            }),
        ),
        (
            &[0x9E],
            None,
            Expect::Is(KeyEvent {
                code: u16::from(b'A'),
                text: 0,
                modifiers: 0,
                pressed: false,
            }),
        ),
        // 2. Shift, then 'A', then Shift release.
        (&[0x2A], None, Expect::Is(shift)),
        (
            &[0x1E],
            Some(b'A'),
            Expect::Is(KeyEvent {
                code: u16::from(b'A'),
                text: b'A',
                modifiers: MOD_SHIFT,
                pressed: true,
            }),
        ),
        (
            &[0xAA],
            None,
            Expect::Is(KeyEvent {
                code: key_code::LEFT_SHIFT,
                text: 0,
                modifiers: 0,
                pressed: false,
            }),
        ),
        // 3. Caps Lock toggles the letter case and reports its own state.
        (&[0x3A], None, Expect::Is(caps)),
        (
            &[0x1E],
            Some(b'A'),
            Expect::Is(KeyEvent {
                code: u16::from(b'A'),
                text: b'A',
                modifiers: MOD_CAPS,
                pressed: true,
            }),
        ),
        (
            &[0xBA],
            None,
            Expect::Is(KeyEvent {
                code: key_code::CAPS_LOCK,
                text: 0,
                modifiers: MOD_CAPS,
                pressed: false,
            }),
        ),
        (
            &[0x1E],
            Some(b'A'),
            Expect::Is(KeyEvent {
                code: u16::from(b'A'),
                text: b'A',
                modifiers: MOD_CAPS,
                pressed: true,
            }),
        ),
        // A second Caps press toggles it off; break codes do not toggle.
        (
            &[0x3A],
            None,
            Expect::Is(KeyEvent {
                code: key_code::CAPS_LOCK,
                text: 0,
                modifiers: 0,
                pressed: true,
            }),
        ),
        (
            &[0xBA],
            None,
            Expect::Is(KeyEvent {
                code: key_code::CAPS_LOCK,
                text: 0,
                modifiers: 0,
                pressed: false,
            }),
        ),
        (
            &[0x1E],
            Some(b'a'),
            Expect::Is(KeyEvent {
                code: u16::from(b'A'),
                text: b'a',
                modifiers: 0,
                pressed: true,
            }),
        ),
        // 4. Ctrl+C is still the console interrupt and a control event.
        (&[0x1D], None, Expect::Is(ctrl)),
        (
            &[0x2E],
            Some(0x03),
            Expect::Is(KeyEvent {
                code: u16::from(b'C'),
                text: 0x03,
                modifiers: MOD_CTRL,
                pressed: true,
            }),
        ),
        (
            &[0x9D],
            None,
            Expect::Is(KeyEvent {
                code: key_code::LEFT_CTRL,
                text: 0,
                modifiers: 0,
                pressed: false,
            }),
        ),
        // 5. Extended arrow, function key, Enter and Backspace.
        (
            &[0xE0, 0x4B],
            None,
            Expect::Is(KeyEvent {
                code: key_code::LEFT,
                text: 0,
                modifiers: 0,
                pressed: true,
            }),
        ),
        (
            &[0x3B],
            None,
            Expect::Is(KeyEvent {
                code: key_code::F1,
                text: 0,
                modifiers: 0,
                pressed: true,
            }),
        ),
        (
            &[0x1C],
            Some(b'\n'),
            Expect::Is(KeyEvent {
                code: key_code::ENTER,
                text: 0,
                modifiers: 0,
                pressed: true,
            }),
        ),
        (
            &[0x0E],
            Some(0x7F),
            Expect::Is(KeyEvent {
                code: key_code::BACKSPACE,
                text: 0,
                modifiers: 0,
                pressed: true,
            }),
        ),
        // 6. A complete Pause sequence produces nothing at all.
        (&[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5], None, Expect::None),
    ];

    let mut state = 0u8;
    for (index, (bytes, console, key)) in cases.iter().enumerate() {
        if !check(&mut state, bytes, *console, *key) {
            return Err(index + 1);
        }
    }
    Ok(())
}

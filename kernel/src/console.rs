//! Framebuffer text console. Every character is also mirrored to COM1, so a
//! headless run still shows the same output.
//!
//! # Staged input line discipline (spec 0005, Q44)
//!
//! The target design keeps the kernel out of line editing: input is a byte
//! stream, and `stdlib`/the shell echo and assemble lines. A shell can now
//! index and compare buffers, but the language still has no way to delete or
//! insert a byte inside one, so this milestone stages that work here, exactly
//! like the scalar `spawn`/`wait` ABI. The service opportunity polls COM1 and the
//! PS/2 keyboard into one static FIFO; printable bytes are appended to a
//! pending line and echoed, Backspace erases, Enter completes the line into a
//! small ring, and Ctrl+C clears it. `core.read_line` pops a finished line and
//! otherwise blocks the caller. Replacing this with application-side editing
//! is required when `stdlib` string editing (length, truncate, insert) lands; byte
//! indexing alone cannot delete a character (Q44).

use core::fmt::{self, Write};

use arch_x86_64::{keyboard, serial};

use crate::font8x8::FONT8X8_BASIC;
use crate::mb2::Framebuffer;
use crate::sync::SpinLock;

const SCALE: usize = 2;
const GLYPH: usize = 8 * SCALE;
const FG: u32 = 0x00D8_E0E0;
const BG: u32 = 0x0010_1818;

/// Longest pending line. A byte that would exceed it is dropped.
pub const LINE_BYTES: usize = 256;
/// Finished lines held for `core.read_line`; the oldest is overwritten when
/// the ring is full.
pub const LINE_RING: usize = 4;

pub struct Console {
    fb: Option<Framebuffer>,
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
}

static CONSOLE: SpinLock<Option<Console>> = SpinLock::new(None);

impl Console {
    fn new(fb: Option<Framebuffer>) -> Self {
        let mut console = Self {
            fb: None,
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
        };

        if let Some(screen) = fb {
            if screen.usable() {
                screen.clear(BG);
                console.cols = screen.width as usize / GLYPH;
                console.rows = screen.height as usize / GLYPH;
                console.fb = Some(screen);
            }
        }

        console
    }

    fn scroll(&mut self) {
        if let Some(fb) = &self.fb {
            fb.scroll_up(GLYPH, BG);
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.rows > 0 && self.row >= self.rows {
            self.scroll();
            self.row = self.rows - 1;
        }
    }

    fn draw(&self, code: u8) {
        let (Some(fb), true) = (&self.fb, code < 0x80) else {
            return;
        };

        let glyph = &FONT8X8_BASIC[code as usize];
        let x0 = self.col * GLYPH;
        let y0 = self.row * GLYPH;

        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                let on = bits & (1 << col) != 0;
                let color = if on { FG } else { BG };
                fb.fill_rect(x0 + col * SCALE, y0 + row * SCALE, SCALE, SCALE, color);
            }
        }
    }

    fn put_byte(&mut self, byte: u8) {
        serial::write_byte(byte);

        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\x08' => {
                if self.col > 0 {
                    self.col -= 1;
                    self.draw(b' ');
                }
            }
            b'\t' => {
                let next = (self.col / 4 + 1) * 4;
                while self.col < next {
                    serial::write_byte(b' ');
                    self.draw(b' ');
                    self.col += 1;
                    if self.rows > 0 && self.col >= self.cols {
                        self.newline();
                    }
                }
            }
            0x20..=0x7E => {
                self.draw(byte);
                self.col += 1;
                if self.rows > 0 && self.col >= self.cols {
                    self.newline();
                }
            }
            _ => {}
        }
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.put_byte(byte);
        }
        Ok(())
    }
}

pub fn init(fb: Option<Framebuffer>) {
    *CONSOLE.lock() = Some(Console::new(fb));
}

/// Entry point for the `print!`/`println!` macros.
pub fn _print(args: fmt::Arguments<'_>) {
    let mut guard = CONSOLE.lock();
    if let Some(console) = guard.as_mut() {
        let _ = console.write_fmt(args);
    }
}

/// Writes raw application bytes to the console (the interpreter's host
/// `console_write`, spec 0005). The bytes are not validated: they may be
/// arbitrary binary data, and non-printable bytes are dropped by the text
/// console after being mirrored to serial.
pub fn write_bytes(bytes: &[u8]) {
    let mut guard = CONSOLE.lock();
    if let Some(console) = guard.as_mut() {
        for byte in bytes {
            console.put_byte(*byte);
        }
    }
}

// ---- staged input -------------------------------------------------------

/// One completed line.
#[derive(Clone, Copy)]
struct Line {
    len: u16,
    data: [u8; LINE_BYTES],
}

const EMPTY_LINE: Line = Line {
    len: 0,
    data: [0; LINE_BYTES],
};

/// Pending line, finished-line ring and the indices that make it a queue.
struct Input {
    pending_len: usize,
    pending: [u8; LINE_BYTES],
    lines: [Line; LINE_RING],
    next: usize,
    len: usize,
}

impl Input {
    const fn new() -> Self {
        Input {
            pending_len: 0,
            pending: [0; LINE_BYTES],
            lines: [EMPTY_LINE; LINE_RING],
            next: 0,
            len: 0,
        }
    }

    fn feed(&mut self, byte: u8) {
        match byte {
            b'\r' | b'\n' => {
                self.complete();
                echo(b"\n");
            }
            0x7F | 0x08 => {
                if self.pending_len > 0 {
                    self.pending_len -= 1;
                    // Move back, overwrite the glyph with a space, move back.
                    echo(b"\x08 \x08");
                }
            }
            0x03 => {
                self.pending_len = 0;
                echo(b"^C\n");
            }
            b'\t' | 0x20..=0x7E => self.append(byte),
            _ => {}
        }
    }

    fn append(&mut self, byte: u8) {
        if self.pending_len < LINE_BYTES {
            self.pending[self.pending_len] = byte;
            self.pending_len += 1;
            echo(&[byte]);
        }
        // At capacity the byte is refused: it is neither stored nor echoed,
        // so the visible line always matches the stored one.
    }

    fn complete(&mut self) {
        let slot = if self.len == LINE_RING {
            // Full: overwrite the oldest finished line.
            let slot = self.next;
            self.next = (self.next + 1) % LINE_RING;
            slot
        } else {
            let slot = (self.next + self.len) % LINE_RING;
            self.len += 1;
            slot
        };
        let line = &mut self.lines[slot];
        line.len = self.pending_len as u16;
        line.data[..self.pending_len].copy_from_slice(&self.pending[..self.pending_len]);
        self.pending_len = 0;
    }

    fn pop(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let slot = self.next;
        self.next = (self.next + 1) % LINE_RING;
        self.len -= 1;
        let line = &self.lines[slot];
        let copied = (line.len as usize).min(out.len());
        out[..copied].copy_from_slice(&line.data[..copied]);
        Some(copied)
    }
}

static INPUT: SpinLock<Input> = SpinLock::new(Input::new());

fn echo(bytes: &[u8]) {
    write_bytes(bytes);
}

/// Polls both byte sources into the line discipline. Called once per service
/// opportunity, before the scheduler claims a task.
pub fn poll_input() {
    let mut input = INPUT.lock();
    for _ in 0..64 {
        match serial::try_read_byte() {
            Some(byte) => input.feed(byte),
            None => break,
        }
    }
    for _ in 0..32 {
        match keyboard::try_read_byte() {
            Some(byte) => input.feed(byte),
            None => break,
        }
    }
}

/// Pops one finished line into `out`, truncating to its length. Returns the
/// copied length, or `None` when no line is complete. The rest of an
/// over-long line is discarded (staged behaviour, spec 0005 Q44).
pub fn read_line(out: &mut [u8]) -> Option<usize> {
    INPUT.lock().pop(out)
}

/// True when at least one finished line is waiting.
pub fn has_line() -> bool {
    INPUT.lock().len != 0
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::console::_print(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

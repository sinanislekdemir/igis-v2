//! Framebuffer text console. Every character is also mirrored to COM1, so a
//! headless run still shows the same output.

use core::fmt::{self, Write};

use arch_x86_64::serial;

use crate::font8x8::FONT8X8_BASIC;
use crate::mb2::Framebuffer;
use crate::sync::SpinLock;

const SCALE: usize = 2;
const GLYPH: usize = 8 * SCALE;
const FG: u32 = 0x00D8_E0E0;
const BG: u32 = 0x0010_1818;

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

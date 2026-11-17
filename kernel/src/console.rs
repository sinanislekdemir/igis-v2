//! Kernel text windows: a terminal for applications and a system log for the
//! kernel (spec 0012, ADR 0025).
//!
//! The M2.1 console painted at screen coordinates and scrolled the whole
//! shadow, so any text output after the desktop composed also scrolled every
//! window. Since M2.3.4 the console paints **two kernel-owned text windows**
//! (owner `0`): one receives `core.write` and the staged line-discipline echo,
//! the other receives `print!`/`println!`. Each keeps its own grid, cursor and
//! per-window scroll, so text never moves the desktop or another window.
//!
//! Every byte is still mirrored to COM1, so a headless run reads the same
//! transcript as before.
//!
//! # Locking
//!
//! One [`CONSOLE`] lock owns both grids. Drawing takes `CONSOLE`, then asks the
//! window table for the content's visible parts (`WINDOW`), then paints through
//! `gfx` (`SCREEN`): `CONSOLE → WINDOW → SCREEN`. The window service's repair
//! takes `WINDOW → SCREEN`; no path takes `WINDOW` and then `CONSOLE`. Pending
//! `Expose` flags for owner `0` are pulled by [`pump`] at the service
//! opportunity rather than pushed from the repair, which keeps that order.
//!
//! A cell is painted only inside the window's **visible parts** (its content
//! minus the frames of higher windows). The console passes that clip explicitly
//! to `gfx` (`draw_cell`/`present_within`) instead of narrowing the shadow's one
//! global clip: a whole-grid repaint must not clip an application drawing on
//! another core.
//!
//! # Staged input line discipline (spec 0005, Q44)
//!
//! The target design keeps the kernel out of line editing: input is a byte
//! stream, and `stdlib`/the shell echo and assemble lines. The language still
//! has no way to delete or insert a byte inside a buffer, so this milestone
//! stages that work here. The service opportunity polls COM1 and the PS/2
//! keyboard into one static FIFO; printable bytes are appended to a pending
//! line and echoed into the terminal window, Backspace erases, Enter completes
//! the line into a small ring, and Ctrl+C clears it. `core.read_line` pops a
//! finished line and otherwise blocks the caller.

use core::fmt::{self, Write};

use abi::{Rect, Status};
use arch_x86_64::{keyboard, serial};

use crate::font8x8::FONT8X8_BASIC;
use crate::gfx;
use crate::sync::SpinLock;
use crate::window;

/// Longest pending line. A byte that would exceed it is dropped.
pub const LINE_BYTES: usize = 256;
/// Finished lines held for `core.read_line`; the oldest is overwritten when
/// the ring is full.
pub const LINE_RING: usize = 4;

/// Cell size in pixels: 8×8 glyphs at scale 1 (spec 0012 D2).
const CELL: i64 = 8;

/// Bounded stale-geometry retries for a console paint (spec 0012 D3).
///
/// A cell/part paint computes the window's visible parts, then draws and
/// presents. A window can move above the console between the two steps on
/// another core; painting the old parts would then overwrite the higher
/// window. Each paint therefore compares `window::generation()` before and
/// after and retries with fresh parts. A move after the final check is covered
/// by the service's own expose for the next pump.
const PAINT_RETRIES: usize = 4;

/// Largest grid a terminal can hold. A 1024×768 screen needs 62×89 cells; the
/// bound leaves room for a much larger mode without a per-screen allocation.
const MAX_COLS: usize = 128;
const MAX_ROWS: usize = 128;

/// Window margins (spec 0012 D7): an 8-pixel screen margin, a 26-pixel top
/// margin that keeps the frames clear of the 22-pixel menu bar, and an 8-pixel
/// gap between the two windows.
const MARGIN: i64 = 8;
const TOP: i64 = 26;
const GAP: i64 = 8;

/// Desktop colours, matching the window service (spec 0011, W11).
const DESKTOP_BASE: u32 = 0x00FF_FFFF;
const DESKTOP_DITHER: u32 = 0x0000_0000;

/// Terminal ink: black on white.
const TERM_FG: u32 = 0x0000_0000;
const TERM_BG: u32 = 0x00FF_FFFF;
/// System-log ink: black on a light grey, so the two windows read differently.
const LOG_FG: u32 = 0x0000_0000;
const LOG_BG: u32 = 0x00F0_F0F0;

/// One text window grid: the kernel terminal, an application's attached
/// terminal, or the system log.
struct Terminal {
    /// Window id, or 0 while unattached.
    id: u64,
    /// Owning task (`0` for the kernel).
    owner: u64,
    attached: bool,
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
    /// Content origin in screen pixels.
    x: i64,
    y: i64,
    fg: u32,
    bg: u32,
    /// The window-service generation the cached geometry was read at, so a
    /// moved terminal is followed without a table query per byte.
    generation: u64,
    grid: [[u8; MAX_COLS]; MAX_ROWS],
}

const EMPTY_TERMINAL: Terminal = Terminal {
    id: 0,
    owner: 0,
    attached: false,
    cols: 0,
    rows: 0,
    col: 0,
    row: 0,
    x: 0,
    y: 0,
    fg: 0,
    bg: 0,
    generation: 0,
    grid: [[0; MAX_COLS]; MAX_ROWS],
};

impl Terminal {
    /// Resets every field in place (the grid is 16 KiB, so no stack temporary).
    fn reset(&mut self, fg: u32, bg: u32) {
        self.id = 0;
        self.owner = 0;
        self.attached = false;
        self.cols = 0;
        self.rows = 0;
        self.col = 0;
        self.row = 0;
        self.x = 0;
        self.y = 0;
        self.fg = fg;
        self.bg = bg;
        self.generation = 0;
        for line in self.grid.iter_mut() {
            line.fill(0);
        }
    }

    fn attach(&mut self, owner: u64, id: u64, x: i64, y: i64, cols: usize, rows: usize) {
        self.id = id;
        self.owner = owner;
        self.attached = true;
        self.x = x;
        self.y = y;
        self.cols = cols;
        self.rows = rows;
    }

    /// Drops the attachment; text then reaches serial only until the next
    /// attach (spec 0014).
    fn detach(&mut self) {
        self.id = 0;
        self.owner = 0;
        self.attached = false;
        self.generation = 0;
    }

    /// Re-reads the window's content rectangle and follows a move (spec 0014).
    /// Returns whether the geometry changed; a missing window reports `false`
    /// and is detached by [`pump`] instead.
    fn sync(&mut self) -> bool {
        if !self.attached {
            return false;
        }
        let Ok(rect) = window::content_rect(self.id) else {
            return false;
        };
        self.generation = window::generation();
        let cols = (rect.w / CELL).clamp(0, MAX_COLS as i64) as usize;
        let rows = (rect.h / CELL).clamp(0, MAX_ROWS as i64) as usize;
        let changed =
            self.x != rect.x || self.y != rect.y || self.cols != cols || self.rows != rows;
        if changed {
            self.x = rect.x;
            self.y = rect.y;
            self.cols = cols;
            self.rows = rows;
            self.col = self.col.min(self.cols.saturating_sub(1));
            self.row = self.row.min(self.rows.saturating_sub(1));
        }
        changed
    }

    /// Clears the character grid and puts the cursor at the home cell, then
    /// repaints the window background.
    fn clear_contents(&mut self) {
        for line in self.grid.iter_mut() {
            line.fill(0);
        }
        self.col = 0;
        self.row = 0;
        self.repaint();
    }

    /// Moves the grid one row up and clears the bottom row (spec 0012 D2).
    fn scroll(&mut self) {
        if self.rows <= 1 {
            for cell in self.grid[0].iter_mut() {
                *cell = 0;
            }
        } else {
            self.grid[..self.rows].copy_within(1..self.rows, 0);
            self.grid[self.rows - 1].fill(0);
        }
        self.repaint();
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.rows > 0 && self.row >= self.rows {
            self.scroll();
            self.row = self.rows - 1;
        }
    }

    /// Draws the cell background and glyph inside every visible part that the
    /// cell intersects, and presents that intersection (spec 0012 D3).
    fn paint_cell(&mut self, col: usize, row: usize) {
        if !self.attached || col >= self.cols || row >= self.rows {
            return;
        }
        let x0 = self.x + col as i64 * CELL;
        let y0 = self.y + row as i64 * CELL;
        let x1 = x0 + CELL;
        let y1 = y0 + CELL;
        let code = self.grid[row][col];
        let mut parts = [Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        }; window::MAX_DAMAGE];
        let mut attempts = 0;
        loop {
            let generation = window::generation();
            let count = window::visible_content_parts(self.id, &mut parts);
            for part in parts[..count].iter() {
                if part.x >= x1
                    || part.y >= y1
                    || part.x.saturating_add(part.w) <= x0
                    || part.y.saturating_add(part.h) <= y0
                {
                    continue;
                }
                let clip = gfx::Clip {
                    x: part.x,
                    y: part.y,
                    w: part.w,
                    h: part.h,
                };
                let _ = gfx::draw_cell(x0, y0, code, self.fg, self.bg, clip);
                let _ = gfx::present_within(x0, y0, CELL, CELL, clip);
            }
            attempts += 1;
            if window::generation() == generation || attempts >= PAINT_RETRIES {
                break;
            }
        }
    }

    /// Repaints the whole grid into every visible part of the content
    /// rectangle, presenting each part once (spec 0012 D3).
    fn repaint(&mut self) {
        if !self.attached {
            return;
        }
        let mut parts = [Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        }; window::MAX_DAMAGE];
        let mut attempts = 0;
        loop {
            let generation = window::generation();
            let count = window::visible_content_parts(self.id, &mut parts);
            for part in parts[..count].iter() {
                let clip = gfx::Clip {
                    x: part.x,
                    y: part.y,
                    w: part.w,
                    h: part.h,
                };
                for row in 0..self.rows {
                    let y = self.y + row as i64 * CELL;
                    if y >= part.y.saturating_add(part.h) || y + CELL <= part.y {
                        continue;
                    }
                    for col in 0..self.cols {
                        let x = self.x + col as i64 * CELL;
                        if x >= part.x.saturating_add(part.w) || x + CELL <= part.x {
                            continue;
                        }
                        let code = self.grid[row][col];
                        let _ = gfx::draw_cell(x, y, code, self.fg, self.bg, clip);
                    }
                }
                let _ = gfx::present_within(part.x, part.y, part.w, part.h, clip);
            }
            attempts += 1;
            if window::generation() == generation || attempts >= PAINT_RETRIES {
                break;
            }
        }
    }

    /// Applies one byte to the terminal state; serial mirroring happens in the
    /// callers, so control bytes are handled exactly once.
    fn feed(&mut self, byte: u8) {
        if !self.attached {
            return;
        }
        // A window move bumps the service generation; follow the content
        // rectangle before painting into the old origin.
        if window::generation() != self.generation {
            self.sync();
        }
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\x08' => {
                if self.col > 0 {
                    self.col -= 1;
                    self.grid[self.row][self.col] = b' ';
                    self.paint_cell(self.col, self.row);
                }
            }
            b'\t' => {
                let next = (self.col / 4 + 1) * 4;
                while self.col < next {
                    if self.col >= self.cols {
                        self.newline();
                        break;
                    }
                    self.grid[self.row][self.col] = b' ';
                    self.paint_cell(self.col, self.row);
                    self.col += 1;
                }
            }
            0x20..=0x7E => {
                if self.col >= self.cols {
                    self.newline();
                }
                if self.row < self.rows && self.col < self.cols {
                    self.grid[self.row][self.col] = byte;
                    self.paint_cell(self.col, self.row);
                    self.col += 1;
                }
            }
            _ => {}
        }
    }

    fn put_byte(&mut self, byte: u8) {
        serial::write_byte(byte);
        self.feed(byte);
    }
}

impl Write for Terminal {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.put_byte(byte);
        }
        Ok(())
    }
}

/// The two kernel text windows and the gate that keeps pre-init output off the
/// screen.
struct Console {
    enabled: bool,
    term: Terminal,
    log: Terminal,
}

const EMPTY_CONSOLE: Console = Console {
    enabled: false,
    term: EMPTY_TERMINAL,
    log: EMPTY_TERMINAL,
};

static CONSOLE: SpinLock<Console> = SpinLock::new(EMPTY_CONSOLE);

impl Console {
    /// Derives the two content rectangles from the screen and opens both
    /// windows (spec 0012 D1/D7). A screen too small for two 8×8 windows
    /// leaves both unattached; output stays on serial.
    fn open_windows(&mut self, width: i64, height: i64) {
        let mut content_w = (width - (MARGIN * 2 + GAP + 4)) / 2;
        let mut content_h = height - TOP - MARGIN - 17;
        // Round down to whole cells, then clamp to the static grid bound.
        content_w = (content_w / CELL) * CELL;
        content_h = (content_h / CELL) * CELL;
        let cols = (content_w / CELL).min(MAX_COLS as i64);
        let rows = (content_h / CELL).min(MAX_ROWS as i64);
        if cols < 1 || rows < 1 {
            return;
        }
        let content_w = cols * CELL;
        let content_h = rows * CELL;
        let second_x = MARGIN + content_w + 2 + GAP;
        if second_x + content_w + 2 + MARGIN > width || TOP + content_h + 17 + MARGIN > height {
            return;
        }

        let cols = cols as usize;
        let rows = rows as usize;
        if let Ok(id) = window::open(0, MARGIN, TOP, content_w, content_h, b"Terminal") {
            self.term.attach(0, id, MARGIN + 1, TOP + 16, cols, rows);
        }
        if let Ok(id) = window::open(0, second_x, TOP, content_w, content_h, b"System Log") {
            self.log.attach(0, id, second_x + 1, TOP + 16, cols, rows);
        }
    }
}

/// Initialises the console: paints the desktop, opens the terminal and system
/// log windows, and paints their empty grids (spec 0012 D1).
pub fn init() {
    let mut console = CONSOLE.lock();
    console.term.reset(TERM_FG, TERM_BG);
    console.log.reset(LOG_FG, LOG_BG);

    let (width, height) = gfx::size();
    if width > 0 && height > 0 {
        // The desktop background, before the first window opens; the service
        // repaints the same pattern under each new frame.
        let _ = gfx::clear(DESKTOP_BASE);
        let _ = gfx::fill_dither(0, 0, width as i64, height as i64, DESKTOP_DITHER);
        let _ = gfx::present(0, 0, width as i64, height as i64);
        console.open_windows(width as i64, height as i64);
        console.term.repaint();
        console.log.repaint();
    }
    console.enabled = true;
    // Opening a window marks its own expose; the empty grid is now painted, so
    // no repaint is pending.
    while window::take_expose(0).is_some() {}
}

/// Entry point for the `print!`/`println!` macros: kernel diagnostics go to the
/// system log window (spec 0012 D5).
pub fn _print(args: fmt::Arguments<'_>) {
    let mut console = CONSOLE.lock();
    if !console.enabled {
        return;
    }
    let _ = console.log.write_fmt(args);
}

/// Writes raw application bytes to the terminal window (the interpreter's host
/// `console_write`, spec 0005). The bytes are not validated: they may be
/// arbitrary binary data, and non-printable bytes are dropped by the text
/// terminal after being mirrored to serial.
pub fn write_bytes(bytes: &[u8]) {
    let mut console = CONSOLE.lock();
    if !console.enabled {
        return;
    }
    for byte in bytes {
        console.term.put_byte(*byte);
    }
}

/// Attaches the console's machine-wide terminal grid to a window owned by
/// `owner` (`core.terminal_attach`, spec 0014).
///
/// The kernel host supplies the caller's task id. The check and the geometry
/// read happen under the console lock and the window lock in that order
/// (`CONSOLE → WINDOW`); closing the kernel's boot Terminal runs the ordinary
/// repair (`WINDOW → SCREEN`) and never calls back into the console.
///
/// Only one task may hold the terminal. The kernel's boot Terminal is released
/// by the first successful attach; a later attach by a different live task is
/// `Full` until the holder's window is closed (task termination). The same
/// owner may rebind to another of its windows.
pub fn attach(owner: u64, id: u64) -> Status {
    let mut console = CONSOLE.lock();
    if !console.enabled {
        return Status::Invalid;
    }
    let rect = match window::owned_content_rect(owner, id) {
        Ok(rect) => rect,
        Err(status) => return status,
    };
    let cols = (rect.w / CELL).clamp(0, MAX_COLS as i64) as usize;
    let rows = (rect.h / CELL).clamp(0, MAX_ROWS as i64) as usize;
    if cols < 1 || rows < 1 {
        return Status::Invalid;
    }
    // A prior owner whose task already terminated is detached even if the
    // service opportunity has not run [`pump`] yet.
    if console.term.attached && console.term.owner != 0 && !window::exists(console.term.id) {
        console.term.detach();
    }
    if console.term.attached {
        if console.term.owner == owner {
            if console.term.id == id {
                return Status::Ok;
            }
            // The same task may move its terminal to another window it owns.
        } else if console.term.owner != 0 {
            return Status::Full;
        } else {
            // The kernel's boot terminal steps aside for the first task. Its
            // record and grid are dropped; the window itself is closed through
            // the ordinary owner-0 path.
            let old = console.term.id;
            console.term.detach();
            if old != 0 {
                let _ = window::close(0, old);
            }
        }
    }
    console.term.reset(TERM_FG, TERM_BG);
    console.term.attach(owner, id, rect.x, rect.y, cols, rows);
    console.term.generation = window::generation();
    console.term.repaint();
    Status::Ok
}

/// Pulls pending `Expose` flags and repaints the matching grid (spec 0012 D4,
/// spec 0014): the attached terminal by **id** (which may be any task's window)
/// and the System Log by owner `0`. Taking the terminal's own expose by id
/// leaves the owner's other windows' exposes in the service queue.
///
/// A terminal whose window disappeared (its task terminated) is detached here;
/// output then reaches serial only until the next attach. Called at the end of
/// [`poll_input`], so every service opportunity and idle-poll cycle repairs the
/// console windows.
pub fn pump() {
    let mut console = CONSOLE.lock();
    if !console.enabled {
        return;
    }
    if console.term.attached && console.term.owner != 0 && !window::exists(console.term.id) {
        console.term.detach();
    }
    if console.term.attached {
        let id = console.term.id;
        if window::take_expose_id(id) {
            console.term.sync();
            console.term.repaint();
        } else if window::generation() != console.term.generation && console.term.sync() {
            console.term.repaint();
        }
    }
    while let Some(id) = window::take_expose(0) {
        if console.log.attached && console.log.id == id {
            console.log.repaint();
        }
    }
}

/// Boot self-check for the console routing and the per-window pixels
/// (spec 0012 D1/D5): writes one probe line through the real `core.write` path
/// and one through the real `println!` path, checks that each landed in its own
/// grid and that the first glyph cell really has foreground and background
/// pixels in the shadow, then clears both grids.
///
/// Returns `Ok(false)` when the windows do not exist (graphics disabled or the
/// screen too small); the boot then reports the serial-only path. A mismatch is
/// `Err(step)` and `kmain` halts.
pub fn route_self_check() -> Result<bool, u32> {
    let attached = {
        let console = CONSOLE.lock();
        console.enabled && console.term.attached && console.log.attached
    };
    if !attached {
        return Ok(false);
    }

    write_bytes(b"console: terminal ok\n");
    _print(format_args!("console: log ok\n"));

    let mut console = CONSOLE.lock();
    // Each stream reached only its own grid. Byte 9 distinguishes the two
    // probes: `t` in "terminal", `l` in "log".
    if &console.term.grid[0][..7] != b"console" || console.term.grid[0][9] != b't' {
        return Err(1);
    }
    if &console.log.grid[0][..7] != b"console" || console.log.grid[0][9] != b'l' {
        return Err(2);
    }
    // The second row is blank again after the probe's newline.
    if console.term.grid[1][0] != 0 || console.log.grid[1][0] != 0 {
        return Err(3);
    }

    // The first glyph cell is really painted in the shadow: one foreground
    // pixel and one background pixel of the 'c' glyph (the cursor is not in
    // the shadow, so `get_pixel` sees the glyph).
    let glyph = &FONT8X8_BASIC[b'c' as usize];
    let mut ink = None;
    let mut paper = None;
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..8 {
            if bits & (1 << col) != 0 {
                if ink.is_none() {
                    ink = Some((col as i64, row as i64));
                }
            } else if paper.is_none() {
                paper = Some((col as i64, row as i64));
            }
        }
    }
    let (Some((ink_x, ink_y)), Some((paper_x, paper_y))) = (ink, paper) else {
        return Err(4);
    };
    if gfx::get_pixel(console.term.x + ink_x, console.term.y + ink_y) != console.term.fg
        || gfx::get_pixel(console.term.x + paper_x, console.term.y + paper_y) != console.term.bg
    {
        return Err(5);
    }
    if gfx::get_pixel(console.log.x + ink_x, console.log.y + ink_y) != console.log.fg
        || gfx::get_pixel(console.log.x + paper_x, console.log.y + paper_y) != console.log.bg
    {
        return Err(6);
    }

    console.term.clear_contents();
    console.log.clear_contents();
    console.enabled = true;
    while window::take_expose(0).is_some() {}
    Ok(true)
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

/// Polls both byte sources into the line discipline, feeds the keyboard's
/// decoded key events into the machine-wide event queue, and pulls pending
/// kernel-window repaints. Called once per service opportunity, before the
/// scheduler claims a task (spec 0012 D4).
pub fn poll_input() {
    let mut input = INPUT.lock();
    for _ in 0..64 {
        match serial::try_read_byte() {
            Some(byte) => input.feed(byte),
            None => break,
        }
    }
    keyboard::poll(|decoded| {
        if let Some(byte) = decoded.console {
            input.feed(byte);
        }
        if let Some(key) = decoded.key {
            crate::event::push_key(key);
        }
    });
    drop(input);
    pump();
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

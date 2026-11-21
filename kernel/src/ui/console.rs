//! Kernel text windows: a terminal for applications and a system log for the
//! kernel (spec 0012, ADR 0025).
//!
//! The M2.1 console painted at screen coordinates and scrolled the whole
//! shadow, so any text output after the desktop composed also scrolled every
//! window. Since M2.3.4 the console paints **two kernel-owned text windows**
//! (owner `0`): one receives `core.write` (applications echo their own input),
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
//! # Console input (spec 0005, ADR 0037)
//!
//! The console is a **raw byte stream**. The service opportunity drains COM1
//! into a fixed ring; the kernel does not echo, edit or assemble lines. The
//! PS/2 keyboard is a different device: its bytes become key events in the
//! machine-wide queue (spec 0010) and never enter the byte ring, so a shell
//! that reads both sees each key once. `core.read_input` copies whatever bytes
//! are waiting without blocking, and an application that wants to sleep does
//! so on the Q39 source `0` ("a console byte is available"). Line editing,
//! echo and prompts are application code.

use core::fmt::{self, Write};

use abi::Rect;
use arch_x86_64::{keyboard, serial};

use crate::font8x8::FONT8X8_BASIC;
use crate::gfx;
use crate::sync::SpinLock;
use crate::window;

/// Capacity of the console input ring. Serial typing is far slower than a
/// service opportunity, so the ring only has to absorb a paste or a QMP burst.
/// A byte that arrives when the ring is full is dropped (newest dropped) and
/// counted; the visible transcript is unaffected because the kernel never
/// echoes input.
pub const INPUT_BYTES: usize = 512;

/// Cell size in pixels: 8×8 glyphs at scale 1 (spec 0012 D2).
const CELL: i64 = 8;

/// Largest grid a terminal can hold. A 1024×768 screen needs 62×89 cells; the
/// bound leaves room for a much larger mode without a per-screen allocation.
const MAX_COLS: usize = 128;
const MAX_ROWS: usize = 128;

/// Window margins (spec 0012 D7): an 8-pixel screen margin, a 26-pixel top
/// margin that keeps the frames clear of the 22-pixel menu bar, and an 8-pixel
/// gap between the two windows.
const MARGIN: i64 = 8;
const TOP: i64 = 26;

/// Desktop colours, matching the window service (spec 0011, W11).
const DESKTOP_BASE: u32 = 0x00FF_FFFF;
const DESKTOP_DITHER: u32 = 0x0000_0000;

/// System-log ink: black on a light grey.
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
    /// The window is minimized; update the grid but do not paint (M11).
    hidden: bool,
    /// Cells drawn since the last present. One bounded bounding box is enough:
    /// a write run is a line, and the whole box is presented once per flush
    /// (spec 0012, D3; spec 0016).
    dirty: Option<Rect>,
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
    hidden: false,
    dirty: None,
    grid: [[0; MAX_COLS]; MAX_ROWS],
};

/// The bounding-box union of two rectangles.
fn union_rect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = a.x.saturating_add(a.w).max(b.x.saturating_add(b.w));
    let y1 = a.y.saturating_add(a.h).max(b.y.saturating_add(b.h));
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

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
        self.hidden = false;
        self.dirty = None;
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

    /// Records a painted cell rectangle for the next [`Terminal::flush`].
    fn mark_dirty(&mut self, rect: Rect) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some(current) => union_rect(current, rect),
            None => rect,
        });
    }

    /// Presents the cells drawn since the last flush in one go (spec 0012 D3;
    /// spec 0016). The visible parts are read now, under the window table lock
    /// across the present (`WINDOW → SCREEN`), and only the dirty rectangle is
    /// copied. A window that moved above the terminal in between is already in
    /// the shadow, so the copy cannot resurrect stale pixels.
    fn flush(&mut self) {
        let Some(dirty) = self.dirty.take() else {
            return;
        };
        if !self.attached || self.hidden || dirty.w <= 0 || dirty.h <= 0 {
            return;
        }
        let id = self.id;
        let _ = window::paint_visible_parts(id, &mut |parts| {
            for part in parts {
                let x0 = dirty.x.max(part.x);
                let y0 = dirty.y.max(part.y);
                let x1 = dirty
                    .x
                    .saturating_add(dirty.w)
                    .min(part.x.saturating_add(part.w));
                let y1 = dirty
                    .y
                    .saturating_add(dirty.h)
                    .min(part.y.saturating_add(part.h));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let clip = gfx::Clip {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                };
                let _ = gfx::present_within(x0, y0, x1 - x0, y1 - y0, clip);
            }
        });
    }

    /// Draws the cell background and glyph inside every visible part that the
    /// cell intersects (spec 0012 D3). The cell is not presented here: its
    /// rectangle joins the terminal's dirty box and one [`Terminal::flush`]
    /// presents the run, so typing a line costs one present, not one per cell.
    ///
    /// The window table lock is held for the whole paint, so a window opened,
    /// raised or moved on another core cannot change the visible parts between
    /// the geometry read and the pixels.
    fn paint_cell(&mut self, col: usize, row: usize) {
        if !self.attached || self.hidden || col >= self.cols || row >= self.rows {
            return;
        }
        let x0 = self.x + col as i64 * CELL;
        let y0 = self.y + row as i64 * CELL;
        let x1 = x0 + CELL;
        let y1 = y0 + CELL;
        let code = self.grid[row][col];
        let id = self.id;
        let fg = self.fg;
        let bg = self.bg;
        let _ = window::paint_visible_parts(id, &mut |parts| {
            for part in parts {
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
                let _ = gfx::draw_cell(x0, y0, code, fg, bg, clip);
            }
        });
        self.mark_dirty(Rect {
            x: x0,
            y: y0,
            w: CELL,
            h: CELL,
        });
    }

    /// Repaints the whole grid into every visible part of the content
    /// rectangle, presenting each part once (spec 0012 D3). As in
    /// [`Terminal::paint_cell`], the window lock is held across the paint.
    fn repaint(&mut self) {
        if !self.attached || self.hidden {
            return;
        }
        // The whole content is drawn and presented below, so any pending
        // per-cell dirty box is covered.
        self.dirty = None;
        let id = self.id;
        let x0 = self.x;
        let y0 = self.y;
        let cols = self.cols;
        let rows = self.rows;
        let fg = self.fg;
        let bg = self.bg;
        let grid = &self.grid;
        let _ = window::paint_visible_parts(id, &mut |parts| {
            for part in parts {
                let clip = gfx::Clip {
                    x: part.x,
                    y: part.y,
                    w: part.w,
                    h: part.h,
                };
                for row in 0..rows {
                    let y = y0 + row as i64 * CELL;
                    if y >= part.y.saturating_add(part.h) || y + CELL <= part.y {
                        continue;
                    }
                    for col in 0..cols {
                        let x = x0 + col as i64 * CELL;
                        if x >= part.x.saturating_add(part.w) || x + CELL <= part.x {
                            continue;
                        }
                        let code = grid[row][col];
                        let _ = gfx::draw_cell(x, y, code, fg, bg, clip);
                    }
                }
                let _ = gfx::present_within(part.x, part.y, part.w, part.h, clip);
            }
        });
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

/// The kernel System Log and the gate that keeps pre-init output off the
/// screen. Application output is a console session, not a kernel grid
/// (spec 0021, ADR 0053).
struct Console {
    enabled: bool,
    log: Terminal,
}

const EMPTY_CONSOLE: Console = Console {
    enabled: false,
    log: EMPTY_TERMINAL,
};

static CONSOLE: SpinLock<Console> = SpinLock::new(EMPTY_CONSOLE);

impl Console {
    /// Derives the System Log's content rectangle from the screen and opens the
    /// one kernel text window (spec 0012 D1/D7). A screen too small for an 8×8
    /// window leaves it unattached; kernel output stays on serial.
    fn open_windows(&mut self, width: i64, height: i64) {
        let mut content_w = width - (MARGIN * 2 + 2);
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
        if TOP + content_h + 17 + MARGIN > height {
            return;
        }
        let cols = cols as usize;
        let rows = rows as usize;
        if let Ok(id) = window::open_bottom(0, MARGIN, TOP, content_w, content_h, b"System Log") {
            self.log.attach(0, id, MARGIN + 1, TOP + 16, cols, rows);
        }
    }
}

/// Reopens the kernel System Log after a runtime display mode change
/// (spec 0022, D9). The log's content rectangle is derived from the screen at
/// init; without this it would keep its old (nearly full-screen) size and hide
/// the desktop on a smaller mode. The log is closed and reopened at the new
/// geometry, which clears its previous diagnostics; kernel output continues in
/// the new grid.
pub fn screen_changed() {
    let (width, height) = gfx::size();
    if width == 0 || height == 0 {
        return;
    }
    let mut console = CONSOLE.lock();
    if !console.enabled {
        return;
    }
    if console.log.id != 0 {
        let _ = window::close(0, console.log.id);
    }
    console.log.reset(LOG_FG, LOG_BG);
    console.open_windows(width as i64, height as i64);
    console.log.repaint();
    while window::take_expose(0).is_some() {}
}

/// Initialises the console: paints the desktop, opens the System Log and paints
/// its empty grid (spec 0012 D1).
pub fn init() {
    let mut console = CONSOLE.lock();
    console.log.reset(LOG_FG, LOG_BG);

    let (width, height) = gfx::size();
    if width > 0 && height > 0 {
        // The desktop background, before the first window opens; the service
        // repaints the same pattern under each new frame.
        let _ = gfx::clear(DESKTOP_BASE);
        let _ = gfx::fill_dither(0, 0, width as i64, height as i64, DESKTOP_DITHER);
        let _ = gfx::present(0, 0, width as i64, height as i64);
        console.open_windows(width as i64, height as i64);
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
    // A minimized log and an open app-drawn pull-down both suppress painting;
    // the grid still records the bytes and `pump` repaints once the menu closes
    // or the window is restored. Serial mirroring is unaffected.
    console.log.hidden = console.log.attached
        && (!window::is_visible(console.log.id) || window::grab_active());
    let _ = console.log.write_fmt(args);
    console.log.flush();
}

/// Writes raw application bytes to the terminal window (the interpreter's host
/// `console_write`, spec 0005). The bytes are not validated: they may be
/// arbitrary binary data, and non-printable bytes are dropped by the text
/// terminal after being mirrored to serial.
///
/// All cells of the call are drawn first and the terminal's dirty rectangle is
/// presented once, so a line costs one present instead of one per cell
/// (spec 0016).
pub fn write_bytes(session: u64, bytes: &[u8]) {
    // The console lock makes one `core.write` call atomic on serial relative to
    // `println!`/`_print`; without it a harness transcript can split an
    // application line with kernel diagnostics. A bound task's bytes also go
    // to its session output ring; an unbound task has no kernel terminal, so
    // serial is the only destination (spec 0021, S7).
    let _console = CONSOLE.lock();
    for byte in bytes {
        serial::write_byte(*byte);
    }
    if session != 0 {
        let _ = crate::session::write_output(session, bytes);
    }
}

/// Pulls the System Log's pending `Expose` and follows a moved window
/// (spec 0012 D4). The log is the only kernel-owned text window from M11; a
/// terminal is an application emulator (spec 0021). Called at the end of
/// [`poll_input`], so every service opportunity and idle-poll cycle repairs it.
pub fn pump() {
    let mut console = CONSOLE.lock();
    if !console.enabled || !console.log.attached {
        return;
    }
    // A minimized kernel window is not painted; a pending expose is cleared so
    // the restore path can mark a fresh one.
    if !window::is_visible(console.log.id) {
        while window::take_expose(0).is_some() {}
        return;
    }
    // While an app-drawn pull-down owns the modal grab, leave the expose
    // pending: repainting the log here would draw over the menu. The next
    // service opportunity after the menu closes repaints it.
    if window::grab_active() {
        return;
    }
    if console.log.hidden {
        console.log.hidden = false;
        console.log.repaint();
    }
    while let Some(id) = window::take_expose(0) {
        if console.log.id == id {
            console.log.hidden = false;
            console.log.repaint();
        }
    }
    if window::generation() != console.log.generation && console.log.sync() {
        console.log.repaint();
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
        console.enabled && console.log.attached
    };
    if !attached {
        return Ok(false);
    }

    _print(format_args!("console: log ok\n"));

    let mut console = CONSOLE.lock();
    // The probe reached the one kernel grid. Byte 9 is `l` in "log".
    if &console.log.grid[0][..7] != b"console" || console.log.grid[0][9] != b'l' {
        return Err(1);
    }
    // The second row is blank again after the probe's newline.
    if console.log.grid[1][0] != 0 {
        return Err(2);
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
        return Err(3);
    };
    if gfx::get_pixel(console.log.x + ink_x, console.log.y + ink_y) != console.log.fg
        || gfx::get_pixel(console.log.x + paper_x, console.log.y + paper_y) != console.log.bg
    {
        return Err(4);
    }

    console.log.clear_contents();
    console.enabled = true;
    while window::take_expose(0).is_some() {}
    Ok(true)
}

// ---- console input byte ring (spec 0005, ADR 0037) ----------------------

/// Fixed raw byte ring. The kernel stores bytes and hands them out; it never
/// interprets, echoes or assembles a line.
struct Input {
    data: [u8; INPUT_BYTES],
    head: usize,
    count: usize,
    /// Bytes refused because the ring was full. Diagnostic only; the boot
    /// self-check pins the policy.
    dropped: u64,
}

const EMPTY_INPUT: Input = Input {
    data: [0; INPUT_BYTES],
    head: 0,
    count: 0,
    dropped: 0,
};

impl Input {
    const fn new() -> Self {
        EMPTY_INPUT
    }

    /// Appends one byte. The ring keeps the oldest `INPUT_BYTES` bytes: a byte
    /// that arrives at capacity is dropped and counted, so the visible
    /// sequence never rewinds and the policy is deterministic.
    fn push(&mut self, byte: u8) {
        if self.count == INPUT_BYTES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let tail = (self.head + self.count) % INPUT_BYTES;
        self.data[tail] = byte;
        self.count += 1;
    }

    /// Copies up to `out.len()` bytes in arrival order and consumes them.
    fn pop(&mut self, out: &mut [u8]) -> usize {
        let copied = self.count.min(out.len());
        for (index, slot) in out.iter_mut().take(copied).enumerate() {
            *slot = self.data[(self.head + index) % INPUT_BYTES];
        }
        self.head = (self.head + copied) % INPUT_BYTES;
        self.count -= copied;
        copied
    }
}

static INPUT: SpinLock<Input> = SpinLock::new(Input::new());

/// Drains COM1 into the byte ring and the PS/2 keyboard into the event queue,
/// then pulls pending kernel-window repaints. Called once per service
/// opportunity, before the scheduler claims a task (spec 0012 D4).
///
/// The device split is deliberate (ADR 0037): **serial is the byte stream and
/// the keyboard is events**. Feeding keyboard bytes into the ring too would
/// make a shell that reads both see every key twice.
pub fn poll_input() {
    // Serial bytes belong to the focused window owner's console session when
    // it has one; otherwise they enter the global ring (spec 0021, S10).
    let focus = window::focus();
    let owner = if focus == 0 {
        0
    } else {
        window::owner_of(focus)
    };
    let session = if owner == 0 {
        0
    } else {
        crate::session::owned_by(owner)
            .unwrap_or_else(|| crate::task::session_of_task(owner))
    };
    for _ in 0..64 {
        match serial::try_read_byte() {
            Some(byte) => {
                if session == 0 || !crate::session::input_push(session, byte) {
                    INPUT.lock().push(byte);
                }
            }
            None => break,
        }
    }
    keyboard::poll(|decoded| {
        if let Some(key) = decoded.key {
            crate::event::push_key(key);
        }
    });
    pump();
}

/// Copies the console input bytes that are waiting into `out` and returns the
/// count (`0` when none). Raw and non-blocking: the caller sleeps on source
/// `0` with `core.next_any` (spec 0003, "Multi-source waits").
pub fn read_input(session: u64, out: &mut [u8]) -> usize {
    if session != 0 {
        return crate::session::read_input(session, out);
    }
    INPUT.lock().pop(out)
}

/// True when the task's console input source has a byte waiting: its session
/// input ring when bound, otherwise the global serial ring.
pub fn has_byte_for(session: u64) -> bool {
    if session != 0 {
        crate::session::input_ready(session)
    } else {
        has_byte()
    }
}

/// True when at least one global console input byte is waiting.
pub fn has_byte() -> bool {
    INPUT.lock().count != 0
}

/// Boot self-check for the byte ring (pure; uses a scratch ring, not the live
/// one). Proves FIFO order, the partial-pop split, empty reads, and the
/// "newest dropped and counted" overflow policy.
pub fn input_self_check() -> Result<(), u32> {
    let mut ring = Input::new();
    let mut out = [0u8; 4];

    if ring.pop(&mut out) != 0 {
        return Err(1);
    }
    ring.push(b'a');
    ring.push(b'b');
    ring.push(b'c');
    if ring.pop(&mut out[..2]) != 2 || out[..2] != *b"ab" {
        return Err(2);
    }
    if ring.pop(&mut out) != 1 || out[0] != b'c' {
        return Err(3);
    }
    if ring.pop(&mut out) != 0 {
        return Err(4);
    }

    let mut index = 0;
    while index < INPUT_BYTES {
        ring.push((index & 0xFF) as u8);
        index += 1;
    }
    ring.push(b'x');
    ring.push(b'y');
    if ring.count != INPUT_BYTES || ring.dropped != 2 {
        return Err(5);
    }
    // The oldest bytes survived; the two overflow bytes are gone.
    if ring.pop(&mut out) != 4 || out != [0, 1, 2, 3] {
        return Err(6);
    }
    let remaining = ring.count;
    let mut last = [0u8; INPUT_BYTES];
    if ring.pop(&mut last) != remaining {
        return Err(7);
    }
    if last[remaining - 1] != ((INPUT_BYTES - 1) & 0xFF) as u8 {
        return Err(8);
    }
    Ok(())
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

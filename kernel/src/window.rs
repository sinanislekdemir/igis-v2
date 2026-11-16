//! The window service: records, lifecycle, composition and repair (spec 0011).
//!
//! There is no per-window canvas (ADR 0020): a window is a small record in a
//! fixed table, and the application draws into the one shadow through `syslib`.
//! This module owns the table, its policy and the service-drawn chrome:
//!
//! - a window has a frame origin, a **content** size, a title and an owner task;
//! - the frame is derived: content plus a 1-pixel border and a 16-pixel title
//!   bar, so `frame = (x, y, w + 2, h + 17)`;
//! - one bottom-to-top z-order, one focus (an id, or 0), and monotonic ids that
//!   are never reused;
//! - `close`/`move` are owner-only, `raise` is any task's, and
//!   [`close_task`] removes a terminating task's windows;
//! - every mutation records damage and runs one synchronous repair pass under
//!   the table lock: the desktop is restored where a window was removed or
//!   moved, the chrome of every damaged window is redrawn bottom-to-top (only
//!   in its visible parts), and the damage union is presented once.
//!
//! Client content is not kept by the service: a repair redraws chrome and the
//! desktop, never another window's content. M2.3.3's `Expose` will ask the
//! owner to repaint when a repair uncovers its content.
//!
//! # Locking
//!
//! The table is one machine-wide [`SpinLock`]. The wrappers read the screen size
//! (the graphics lock) before taking the table lock; a repair takes the table
//! lock first and then calls `gfx` (window → graphics). No path takes the
//! graphics lock and then the table lock, so there is no cycle, and the repair
//! is deliberately synchronous under the table lock so no snapshot is needed.
//! `Task::reclaim` calls [`close_task`] with no scheduler lock held.
//!
//! # Self-checks
//!
//! [`self_check`] runs the table policy on a scratch table, and
//! [`paint_self_check`] runs the damage arithmetic and visibility subtraction on
//! scratch state; neither touches the screen. `kmain` halts on a mismatch.

use abi::{Rect, Status};

use crate::gfx;
use crate::sync::SpinLock;

/// Maximum live windows machine-wide (spec 0011, W9).
pub const MAX_WINDOWS: usize = 16;

/// Title capacity in bytes; longer titles are truncated (W9).
pub const TITLE_CAP: usize = 64;

/// Title bar height in pixels (W3).
pub const TITLE_BAR_HEIGHT: i64 = 16;

/// Border width in pixels on the left, right and bottom (W3).
pub const BORDER_WIDTH: i64 = 1;

/// Minimum content width in pixels (W9).
pub const MIN_CONTENT_W: i64 = 8;

/// Minimum content height in pixels (W9).
pub const MIN_CONTENT_H: i64 = 8;

/// Maximum pending damage rectangles per list (W11). A bounded list is enough
/// for 16 windows; overflow drops damage rather than corrupting state.
pub const MAX_DAMAGE: usize = 32;

/// Desktop base colour under the 50% dither (spec 0011, W11).
const DESKTOP_BASE: u32 = 0x00FF_FFFF;

/// Desktop dither colour: black on every `(X + Y)` even pixel, exactly the
/// pattern `samples/apps/desktop` uses.
const DESKTOP_DITHER: u32 = 0x0000_0000;

/// Chrome ink (border, title stripes, close-box outline, title text).
const CHROME_INK: u32 = 0x0000_0000;

/// Focused title bar: white with horizontal stripes (the System 7.5 look).
const TITLE_FOCUSED: u32 = 0x00FF_FFFF;

/// Unfocused title bar: a plain light grey, visibly distinct without stripes.
const TITLE_UNFOCUSED: u32 = 0x00CC_CCCC;

/// One window record. `used == false` is a free storage slot.
#[derive(Clone, Copy)]
struct Window {
    id: u64,
    owner: u64,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    title_len: usize,
    title: [u8; TITLE_CAP],
    /// A whole-content repaint is pending for this window (spec 0011, W12).
    expose_pending: bool,
    used: bool,
}

impl Window {
    const EMPTY: Window = Window {
        id: 0,
        owner: 0,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        title_len: 0,
        title: [0; TITLE_CAP],
        expose_pending: false,
        used: false,
    };

    /// The whole window outline (spec 0011, W3).
    fn frame(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.w.saturating_add(BORDER_WIDTH * 2),
            h: self.h.saturating_add(TITLE_BAR_HEIGHT + BORDER_WIDTH),
        }
    }

    /// The drawable rectangle inside the frame (spec 0011, W3).
    fn content(&self) -> Rect {
        Rect {
            x: self.x.saturating_add(BORDER_WIDTH),
            y: self.y.saturating_add(TITLE_BAR_HEIGHT),
            w: self.w,
            h: self.h,
        }
    }
}

/// The title-bar band of a window: the region whose style depends on focus.
fn title_rect(window: &Window) -> Rect {
    Rect {
        x: window.x,
        y: window.y,
        w: window.w.saturating_add(BORDER_WIDTH * 2),
        h: TITLE_BAR_HEIGHT,
    }
}

/// Whether two rectangles share at least one pixel.
fn intersects(a: Rect, b: Rect) -> bool {
    let ax1 = a.x.saturating_add(a.w);
    let ay1 = a.y.saturating_add(a.h);
    let bx1 = b.x.saturating_add(b.w);
    let by1 = b.y.saturating_add(b.h);
    a.x < bx1 && b.x < ax1 && a.y < by1 && b.y < ay1
}

/// Whether two rectangles overlap or share an edge (mergeable damage).
fn touches(a: Rect, b: Rect) -> bool {
    let ax1 = a.x.saturating_add(a.w);
    let ay1 = a.y.saturating_add(a.h);
    let bx1 = b.x.saturating_add(b.w);
    let by1 = b.y.saturating_add(b.h);
    a.x <= bx1 && b.x <= ax1 && a.y <= by1 && b.y <= ay1
}

/// The bounding-box union of two rectangles.
fn union(a: Rect, b: Rect) -> Rect {
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

/// One desktop-restore request: paint the 50% dither in the parts of `rect` not
/// covered by windows at or above z-order index `covers_from`.
///
/// `covers_from == count` means "no live window covers this" (a newly opened
/// window's own frame is filled with the desktop before its chrome is drawn).
#[derive(Clone, Copy)]
struct Erase {
    rect: Rect,
    covers_from: usize,
}

impl Erase {
    const EMPTY: Erase = Erase {
        rect: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        covers_from: 0,
    };
}

/// Bounded desktop-restore damage. Entries with the same coverage limit merge
/// when they touch; overflow drops the extra request (a stale pixel, never a
/// corrupted high window).
struct EraseList {
    entries: [Erase; MAX_DAMAGE],
    count: usize,
}

impl EraseList {
    const fn new() -> Self {
        EraseList {
            entries: [Erase::EMPTY; MAX_DAMAGE],
            count: 0,
        }
    }

    fn clear(&mut self) {
        self.count = 0;
    }

    fn add(&mut self, rect: Rect, covers_from: usize) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        for index in 0..self.count {
            if self.entries[index].covers_from == covers_from
                && touches(self.entries[index].rect, rect)
            {
                self.entries[index].rect = union(self.entries[index].rect, rect);
                return;
            }
        }
        if self.count < MAX_DAMAGE {
            self.entries[self.count] = Erase { rect, covers_from };
            self.count += 1;
        }
    }

    /// True when an entry with this coverage limit contains `rect` (used by the
    /// paint self-check).
    fn covers(&self, rect: Rect, covers_from: usize) -> bool {
        self.entries[..self.count].iter().any(|entry| {
            entry.covers_from == covers_from
                && entry.rect.x <= rect.x
                && entry.rect.y <= rect.y
                && entry.rect.x.saturating_add(entry.rect.w) >= rect.x.saturating_add(rect.w)
                && entry.rect.y.saturating_add(entry.rect.h) >= rect.y.saturating_add(rect.h)
        })
    }
}

/// Bounded chrome damage: every window whose frame intersects one of these
/// rectangles redraws its chrome in the intersection of its visible parts and
/// this damage.
struct ChromeList {
    rects: [Rect; MAX_DAMAGE],
    count: usize,
}

impl ChromeList {
    const fn new() -> Self {
        ChromeList {
            rects: [Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            }; MAX_DAMAGE],
            count: 0,
        }
    }

    fn clear(&mut self) {
        self.count = 0;
    }

    fn add(&mut self, rect: Rect) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        for index in 0..self.count {
            if touches(self.rects[index], rect) {
                self.rects[index] = union(self.rects[index], rect);
                return;
            }
        }
        if self.count < MAX_DAMAGE {
            self.rects[self.count] = rect;
            self.count += 1;
        }
    }

    fn covers(&self, rect: Rect) -> bool {
        self.rects[..self.count].iter().any(|entry| {
            entry.x <= rect.x
                && entry.y <= rect.y
                && entry.x.saturating_add(entry.w) >= rect.x.saturating_add(rect.w)
                && entry.y.saturating_add(entry.h) >= rect.y.saturating_add(rect.h)
        })
    }

    fn intersects(&self, rect: Rect) -> bool {
        self.rects[..self.count]
            .iter()
            .any(|entry| intersects(*entry, rect))
    }
}

/// Exact, non-merging rectangle pieces (spec 0011, W10/W11): the visible parts
/// of a window after subtracting higher windows. Pieces must stay separate, so
/// they are appended instead of merged into a bounding box that could cover a
/// hole. Overflow drops pieces (under-paint, never over-paint).
struct Parts {
    rects: [Rect; MAX_DAMAGE],
    count: usize,
}

impl Parts {
    const fn new() -> Self {
        Parts {
            rects: [Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            }; MAX_DAMAGE],
            count: 0,
        }
    }

    fn push(&mut self, rect: Rect) {
        if rect.w <= 0 || rect.h <= 0 || self.count == MAX_DAMAGE {
            return;
        }
        self.rects[self.count] = rect;
        self.count += 1;
    }
}

/// Appends the parts of `rect` that are outside `cover` (up to four bands).
fn subtract_into(rect: Rect, cover: Rect, out: &mut Parts) {
    if !intersects(rect, cover) {
        out.push(rect);
        return;
    }
    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x.saturating_add(rect.w);
    let y1 = rect.y.saturating_add(rect.h);
    let cx0 = cover.x.max(x0);
    let cy0 = cover.y.max(y0);
    let cx1 = cover.x.saturating_add(cover.w).min(x1);
    let cy1 = cover.y.saturating_add(cover.h).min(y1);
    if cy0 > y0 {
        out.push(Rect {
            x: x0,
            y: y0,
            w: rect.w,
            h: cy0 - y0,
        });
    }
    if cy1 < y1 {
        out.push(Rect {
            x: x0,
            y: cy1,
            w: rect.w,
            h: y1 - cy1,
        });
    }
    let middle = cy1 - cy0;
    if cx0 > x0 {
        out.push(Rect {
            x: x0,
            y: cy0,
            w: cx0 - x0,
            h: middle,
        });
    }
    if cx1 < x1 {
        out.push(Rect {
            x: cx1,
            y: cy0,
            w: x1 - cx1,
            h: middle,
        });
    }
}

/// Accumulates the bounding box presented once at the end of a repair pass.
struct Presentation {
    rect: Option<Rect>,
}

impl Presentation {
    const fn new() -> Self {
        Presentation { rect: None }
    }

    fn add(&mut self, rect: Rect) {
        self.rect = Some(match self.rect {
            Some(current) => union(current, rect),
            None => rect,
        });
    }
}

/// The machine-wide table: storage plus the bottom-to-top z-order (spec 0011,
/// W5/W6). The policy methods take the screen size and the caller's task id so
/// the same code runs on the global table and on the boot self-check's scratch
/// table.
struct Table {
    windows: [Window; MAX_WINDOWS],
    /// Storage indices, bottom first, exactly `count` live entries.
    order: [usize; MAX_WINDOWS],
    count: usize,
    /// Focused window id, or 0.
    focus: u64,
    /// Next id to assign; starts at 1 and skips 0 on wrap (W2).
    next_id: u64,
    /// Pending desktop restore and chrome damage for the next repair pass.
    erases: EraseList,
    chrome: ChromeList,
}

impl Table {
    const fn new() -> Self {
        Table {
            windows: [Window::EMPTY; MAX_WINDOWS],
            order: [0; MAX_WINDOWS],
            count: 0,
            focus: 0,
            next_id: 1,
            erases: EraseList::new(),
            chrome: ChromeList::new(),
        }
    }

    fn clear_damage(&mut self) {
        self.erases.clear();
        self.chrome.clear();
    }

    /// Position of `id` in the z-order, or `None`.
    fn position(&self, id: u64) -> Option<usize> {
        if id == 0 {
            return None;
        }
        (0..self.count).find(|&pos| self.windows[self.order[pos]].id == id)
    }

    /// Removes the window at `pos` and updates the focus fallback (W6).
    fn remove_at(&mut self, pos: usize) {
        let slot = self.order[pos];
        let removed = self.windows[slot].id;
        self.windows[slot] = Window::EMPTY;
        for index in pos..self.count - 1 {
            self.order[index] = self.order[index + 1];
        }
        self.count -= 1;
        if self.focus == removed {
            self.focus = if self.count == 0 {
                0
            } else {
                self.windows[self.order[self.count - 1]].id
            };
        }
    }

    fn open(
        &mut self,
        screen_w: i64,
        screen_h: i64,
        owner: u64,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        title: &[u8],
    ) -> Result<u64, Status> {
        if screen_w <= 0 || screen_h <= 0 || owner == 0 {
            return Err(Status::Invalid);
        }
        if w < MIN_CONTENT_W || h < MIN_CONTENT_H {
            return Err(Status::Invalid);
        }
        let frame_w = w.saturating_add(BORDER_WIDTH * 2);
        let frame_h = h.saturating_add(TITLE_BAR_HEIGHT + BORDER_WIDTH);
        if frame_w > screen_w || frame_h > screen_h {
            return Err(Status::Invalid);
        }
        if self.count == MAX_WINDOWS {
            return Err(Status::Full);
        }
        let Some(slot) = (0..MAX_WINDOWS).find(|&index| !self.windows[index].used) else {
            return Err(Status::Full);
        };

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        // The origin is clamped so the whole frame stays on screen (W4).
        let x = x.clamp(0, screen_w - frame_w);
        let y = y.clamp(0, screen_h - frame_h);

        let mut window = Window {
            id,
            owner,
            x,
            y,
            w,
            h,
            ..Window::EMPTY
        };
        window.used = true;
        let mut length = title.len().min(TITLE_CAP);
        // Truncate on a code-point boundary: `title` is a `string`, so a byte
        // at the cut that is a continuation byte starts inside the last kept
        // code point.
        while length > 0 && length < title.len() && (title[length] & 0xC0) == 0x80 {
            length -= 1;
        }
        window.title_len = length;
        window.title[..length].copy_from_slice(&title[..length]);

        let frame = window.frame();
        let old_focus = self.focus;
        self.windows[slot] = window;
        self.order[self.count] = slot;
        self.count += 1;
        self.focus = id;

        // The new window covers the desktop and any lower window: restore the
        // desktop across its whole frame (no window above it yet), then draw
        // its chrome. The previous focus loses the focused title bar.
        self.erases.add(frame, self.count);
        self.chrome.add(frame);
        if old_focus != 0 && old_focus != id {
            if let Some(pos) = self.position(old_focus) {
                self.chrome.add(title_rect(&self.windows[self.order[pos]]));
            }
        }
        Ok(id)
    }

    fn close(&mut self, owner: u64, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        if self.windows[self.order[pos]].owner != owner {
            return Status::Invalid;
        }
        let frame = self.windows[self.order[pos]].frame();
        let was_focused = self.focus == id;
        self.remove_at(pos);

        // The window is gone: the desktop returns where no live window covers
        // it, and the windows it uncovered redraw their chrome.
        self.erases.add(frame, 0);
        self.chrome.add(frame);
        if was_focused {
            if let Some(pos) = self.position(self.focus) {
                self.chrome.add(title_rect(&self.windows[self.order[pos]]));
            }
        }
        Status::Ok
    }

    fn raise(&mut self, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        let slot = self.order[pos];
        let frame = self.windows[slot].frame();
        let old_focus = self.focus;
        for index in pos..self.count - 1 {
            self.order[index] = self.order[index + 1];
        }
        self.order[self.count - 1] = slot;
        self.focus = id;

        // Raising adds coverage and uncovers nothing that was above: redraw
        // the raised window's chrome (its newly visible parts), the title bars
        // of the old and new focus, and every window the raised frame overlaps
        // (their chrome may have been covered or uncovered at the overlap).
        self.chrome.add(frame);
        if old_focus != 0 && old_focus != id {
            if let Some(pos) = self.position(old_focus) {
                self.chrome.add(title_rect(&self.windows[self.order[pos]]));
            }
        }
        for pos in 0..self.count - 1 {
            let other = self.windows[self.order[pos]].frame();
            if intersects(other, frame) {
                self.chrome.add(other);
            }
        }
        Status::Ok
    }

    fn move_to(
        &mut self,
        screen_w: i64,
        screen_h: i64,
        owner: u64,
        id: u64,
        x: i64,
        y: i64,
    ) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        let window = &mut self.windows[self.order[pos]];
        if window.owner != owner {
            return Status::Invalid;
        }
        let frame = window.frame();
        if screen_w <= 0 || screen_h <= 0 || frame.w > screen_w || frame.h > screen_h {
            return Status::Invalid;
        }
        let new_x = x.clamp(0, screen_w - frame.w);
        let new_y = y.clamp(0, screen_h - frame.h);
        if window.x == new_x && window.y == new_y {
            return Status::Ok;
        }
        window.x = new_x;
        window.y = new_y;
        let moved = window.frame();

        // The old frame becomes desktop where no live window covers it; the
        // new frame covers the desktop and any lower window. Windows above the
        // moved one keep their pixels, so the new frame is filled only in the
        // parts below it. Chrome is redrawn for every window over both frames.
        self.erases.add(frame, 0);
        self.erases.add(moved, pos + 1);
        self.chrome.add(frame);
        self.chrome.add(moved);
        Status::Ok
    }

    fn frame_rect(&self, id: u64) -> Result<Rect, Status> {
        self.position(id)
            .map(|pos| self.windows[self.order[pos]].frame())
            .ok_or(Status::NotFound)
    }

    fn content_rect(&self, id: u64) -> Result<Rect, Status> {
        self.position(id)
            .map(|pos| self.windows[self.order[pos]].content())
            .ok_or(Status::NotFound)
    }

    /// Removes every window owned by `owner`, applying the focus fallback after
    /// each removal (W8). The damage accumulates and one repair pass covers all
    /// removed frames, so a terminated task's windows disappear together.
    fn close_task(&mut self, owner: u64) {
        let mut removed = false;
        let mut pos = 0;
        while pos < self.count {
            if self.windows[self.order[pos]].owner == owner {
                let frame = self.windows[self.order[pos]].frame();
                self.remove_at(pos);
                self.erases.add(frame, 0);
                self.chrome.add(frame);
                removed = true;
            } else {
                pos += 1;
            }
        }
        // Only an actual removal can move the focus, so a task that owned no
        // window records no damage (and wakes nobody with a spurious expose).
        if removed {
            if let Some(pos) = self.position(self.focus) {
                self.chrome.add(title_rect(&self.windows[self.order[pos]]));
            }
        }
    }

    /// The parts of `rect` not covered by live windows at z-order indices
    /// `covers_from..count`. Returns exact pieces (no bounding-box merge), so a
    /// hidden hole is never painted.
    fn visible_parts(&self, rect: Rect, covers_from: usize) -> Parts {
        let mut parts = Parts::new();
        parts.push(rect);
        let mut pos = covers_from;
        while pos < self.count && parts.count > 0 {
            let cover = self.windows[self.order[pos]].frame();
            let mut next = Parts::new();
            for index in 0..parts.count {
                subtract_into(parts.rects[index], cover, &mut next);
            }
            parts = next;
            pos += 1;
        }
        parts
    }

    /// Paints one window's chrome under the current clip.
    ///
    /// Inside-out: title background, stripes, close box, title band and text,
    /// the separator under the title, and the outer border last. The content
    /// rectangle is never touched.
    fn draw_chrome(&self, slot: usize) {
        let window = &self.windows[slot];
        let frame = window.frame();
        let focused = window.id == self.focus;
        let title_bg = if focused {
            TITLE_FOCUSED
        } else {
            TITLE_UNFOCUSED
        };
        let inner_x = frame.x + BORDER_WIDTH;
        let inner_w = frame.w - BORDER_WIDTH * 2;
        let title_h = TITLE_BAR_HEIGHT - 1;

        let _ = gfx::fill_rect(inner_x, frame.y + 1, inner_w, title_h, title_bg);
        if focused {
            // Horizontal stripes, the System 7.5 active title bar.
            let mut row = frame.y + 2;
            while row < frame.y + title_h {
                let _ = gfx::fill_rect(inner_x, row, inner_w, 1, CHROME_INK);
                row += 2;
            }
        }

        // Close box: an 11x11 outlined box on the left of the title bar.
        let box_x = frame.x + 4;
        let box_y = frame.y + 3;
        let _ = gfx::fill_rect(box_x, box_y, 11, 11, title_bg);
        let _ = gfx::draw_rect(box_x, box_y, 11, 11, CHROME_INK);

        // Title text: truncate to the band right of the close box, then centre
        // it. The band is repainted in the title colour so stripes never cross
        // a glyph.
        let available = inner_w - 20;
        let mut chars = 0;
        if available >= 8 && window.title_len > 0 {
            let max_chars = (available / 8) as usize;
            chars = window.title_len.min(max_chars);
        }
        if chars > 0 {
            let text_w = chars as i64 * 8;
            let band_x = inner_x + 20 + (available - text_w) / 2;
            let _ = gfx::fill_rect(band_x - 2, frame.y + 3, text_w + 4, 10, title_bg);
            let _ = gfx::draw_text(band_x, frame.y + 4, &window.title[..chars], CHROME_INK);
        }

        // Separator under the title bar, then the outer border last so no
        // interior fill can spill onto it.
        let _ = gfx::fill_rect(
            inner_x,
            frame.y + TITLE_BAR_HEIGHT - 1,
            inner_w,
            1,
            CHROME_INK,
        );
        let _ = gfx::draw_rect(frame.x, frame.y, frame.w, frame.h, CHROME_INK);
    }

    /// Marks one whole-content expose on every live window whose frame
    /// intersects the current chrome damage (spec 0011, W12). Coalesced per
    /// window: a pending expose means "repaint the whole content once".
    fn mark_exposes(&mut self) -> bool {
        let mut marked = false;
        for pos in 0..self.count {
            let slot = self.order[pos];
            if !self.windows[slot].expose_pending
                && self.chrome.intersects(self.windows[slot].frame())
            {
                self.windows[slot].expose_pending = true;
                marked = true;
            }
        }
        marked
    }

    /// Takes the topmost pending expose owned by `owner`, clearing it.
    fn take_expose(&mut self, owner: u64) -> Option<u64> {
        for pos in (0..self.count).rev() {
            let slot = self.order[pos];
            if self.windows[slot].owner == owner && self.windows[slot].expose_pending {
                self.windows[slot].expose_pending = false;
                return Some(self.windows[slot].id);
            }
        }
        None
    }

    /// Whether `owner` has any pending expose.
    fn has_expose(&self, owner: u64) -> bool {
        (0..self.count).any(|pos| {
            let slot = self.order[pos];
            self.windows[slot].owner == owner && self.windows[slot].expose_pending
        })
    }

    /// The topmost window whose frame contains `(x, y)` (spec 0011, W16).
    fn hit_test(&self, x: i64, y: i64) -> Option<u64> {
        for pos in (0..self.count).rev() {
            let window = &self.windows[self.order[pos]];
            let frame = window.frame();
            if x >= frame.x
                && y >= frame.y
                && x < frame.x.saturating_add(frame.w)
                && y < frame.y.saturating_add(frame.h)
            {
                return Some(window.id);
            }
        }
        None
    }

    /// Presents a window's content rectangle, clipped to its visible parts
    /// (spec 0011, W17). The caller must own the window.
    fn present(&self, owner: u64, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        let window = &self.windows[self.order[pos]];
        if window.owner != owner {
            return Status::Invalid;
        }
        let content = window.content();
        let parts = self.visible_parts(content, pos + 1);
        for part in parts.rects[..parts.count].iter() {
            if gfx::set_clip(part.x, part.y, part.w, part.h) {
                let _ = gfx::present(part.x, part.y, part.w, part.h);
                gfx::reset_clip();
            }
        }
        Status::Ok
    }

    /// Runs one synchronous repair pass (spec 0011, W11).
    ///
    /// 1. The desktop is restored in the erased parts no live window covers.
    /// 2. Every window whose frame intersects the chrome damage redraws its
    ///    chrome, bottom-to-top, clipped to its visible parts inside the
    ///    damaged area. Higher windows are never overwritten.
    /// 3. The union of all damage is presented once; `present` composites the
    ///    cursor as usual.
    ///
    /// Runs under the table lock (window → graphics), so the z-order it walks
    /// is the one the caller just produced. The repair is synchronous: no
    /// snapshot and no client call.
    fn repair(&mut self) -> bool {
        if self.erases.count == 0 && self.chrome.count == 0 {
            return false;
        }
        if gfx::size().0 == 0 {
            self.clear_damage();
            return false;
        }
        let marked = self.mark_exposes();
        gfx::reset_clip();
        let mut present = Presentation::new();

        for index in 0..self.erases.count {
            let erase = self.erases.entries[index];
            let parts = self.visible_parts(erase.rect, erase.covers_from);
            for part in parts.rects[..parts.count].iter() {
                if gfx::set_clip(part.x, part.y, part.w, part.h) {
                    let _ = gfx::fill_rect(part.x, part.y, part.w, part.h, DESKTOP_BASE);
                    let _ = gfx::fill_dither(part.x, part.y, part.w, part.h, DESKTOP_DITHER);
                    gfx::reset_clip();
                }
            }
            present.add(erase.rect);
        }

        if self.chrome.count > 0 {
            for pos in 0..self.count {
                let slot = self.order[pos];
                let frame = self.windows[slot].frame();
                if !self.chrome.intersects(frame) {
                    continue;
                }
                let visible = self.visible_parts(frame, pos + 1);
                for part in visible.rects[..visible.count].iter() {
                    for index in 0..self.chrome.count {
                        let damage = self.chrome.rects[index];
                        let x0 = part.x.max(damage.x);
                        let y0 = part.y.max(damage.y);
                        let x1 = part
                            .x
                            .saturating_add(part.w)
                            .min(damage.x.saturating_add(damage.w));
                        let y1 = part
                            .y
                            .saturating_add(part.h)
                            .min(damage.y.saturating_add(damage.h));
                        if x1 <= x0 || y1 <= y0 {
                            continue;
                        }
                        if gfx::set_clip(x0, y0, x1 - x0, y1 - y0) {
                            self.draw_chrome(slot);
                            gfx::reset_clip();
                        }
                    }
                }
            }
            for index in 0..self.chrome.count {
                present.add(self.chrome.rects[index]);
            }
        }

        if let Some(rect) = present.rect {
            let _ = gfx::present(rect.x, rect.y, rect.w, rect.h);
        }
        self.clear_damage();
        marked
    }
}

static TABLE: SpinLock<Table> = SpinLock::new(Table::new());

/// Opens a window owned by `owner`: frame origin `(x, y)`, content `(w, h)`.
///
/// A disabled screen has no geometry, so `open` is `Invalid` (W13). The screen
/// size is read before the table lock is taken; the repair pass (if the open
/// succeeded) runs under it.
pub fn open(owner: u64, x: i64, y: i64, w: i64, h: i64, title: &[u8]) -> Result<u64, Status> {
    let (screen_w, screen_h) = gfx::size();
    let mut table = TABLE.lock();
    let result = table.open(screen_w as i64, screen_h as i64, owner, x, y, w, h, title);
    let wake = if result.is_ok() {
        table.repair()
    } else {
        false
    };
    drop(table);
    if wake {
        crate::task::wake_input();
    }
    result
}

/// Closes a window; only its owner may (W7).
pub fn close(owner: u64, id: u64) -> Status {
    let mut table = TABLE.lock();
    let status = table.close(owner, id);
    let wake = if status == Status::Ok {
        table.repair()
    } else {
        false
    };
    drop(table);
    if wake {
        crate::task::wake_input();
    }
    status
}

/// Raises and focuses a window; any task may (W7).
pub fn raise(id: u64) -> Status {
    let mut table = TABLE.lock();
    let status = table.raise(id);
    let wake = if status == Status::Ok {
        table.repair()
    } else {
        false
    };
    drop(table);
    if wake {
        crate::task::wake_input();
    }
    status
}

/// Moves a window owned by `owner`, clamping the origin on screen (W4/W7).
pub fn move_to(owner: u64, id: u64, x: i64, y: i64) -> Status {
    let (screen_w, screen_h) = gfx::size();
    let mut table = TABLE.lock();
    let status = table.move_to(screen_w as i64, screen_h as i64, owner, id, x, y);
    let wake = if status == Status::Ok {
        table.repair()
    } else {
        false
    };
    drop(table);
    if wake {
        crate::task::wake_input();
    }
    status
}

/// The frame rectangle of `id`; `Invalid` when graphics are disabled (W13).
pub fn frame_rect(id: u64) -> Result<Rect, Status> {
    if gfx::size().0 == 0 {
        return Err(Status::Invalid);
    }
    TABLE.lock().frame_rect(id)
}

/// The content rectangle of `id`; `Invalid` when graphics are disabled (W13).
pub fn content_rect(id: u64) -> Result<Rect, Status> {
    if gfx::size().0 == 0 {
        return Err(Status::Invalid);
    }
    TABLE.lock().content_rect(id)
}

/// The focused window id, or 0 (W6).
pub fn focus() -> u64 {
    TABLE.lock().focus
}

/// Closes every window owned by a terminating task and repairs the screen once.
/// Called from `Task::reclaim`, so it covers normal exit, kill, fault, a failed
/// spawn and a stale-slot eviction (W8).
pub fn close_task(owner: u64) {
    let mut table = TABLE.lock();
    table.close_task(owner);
    let wake = table.repair();
    drop(table);
    if wake {
        crate::task::wake_input();
    }
}

/// Takes the topmost pending whole-content `Expose` owned by `owner`, if any
/// (spec 0011, W12). Only the owner can take its own expose.
pub fn take_expose(owner: u64) -> Option<u64> {
    TABLE.lock().take_expose(owner)
}

/// Whether `owner` has a pending `Expose`. The scheduler's input block re-check
/// uses it so an expose marked between the empty read and the block is not lost.
pub fn has_expose(owner: u64) -> bool {
    TABLE.lock().has_expose(owner)
}

/// Routes a pointer-down at `(x, y)`: hit-tests the topmost frame, raises and
/// focuses that window, repairs, and returns its id (spec 0011, W16). Returns
/// `None` on the desktop or when graphics are disabled.
pub fn route_click(x: i64, y: i64) -> Option<u64> {
    if gfx::size().0 == 0 {
        return None;
    }
    let mut table = TABLE.lock();
    let id = table.hit_test(x, y)?;
    if table.raise(id) != Status::Ok {
        return None;
    }
    let wake = table.repair();
    drop(table);
    if wake {
        crate::task::wake_input();
    }
    Some(id)
}

/// Presents a window's content, clipped to its visible parts (spec 0011, W17).
/// `Invalid` when graphics are disabled, `NotFound` for an unknown id, and
/// `Invalid` for a window owned by another task.
pub fn present(owner: u64, id: u64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    TABLE.lock().present(owner, id)
}

/// Boot self-check: runs the policy on a scratch table with fixed 1024 × 768
/// geometry. Returns the first failing step, or `Ok(())`.
///
/// The global table and the screen are untouched, so the check cannot perturb a
/// later application.
pub fn self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;
    let mut table = Table::new();

    // 1. First open: id 1, focused, geometry derives from the content size.
    let one = table.open(SCREEN_W, SCREEN_H, 7, 10, 20, 100, 50, b"one");
    if one != Ok(1) || table.focus != 1 {
        return Err(1);
    }
    let frame = table.frame_rect(1).map_err(|_| 2u32)?;
    let content = table.content_rect(1).map_err(|_| 3u32)?;
    if frame
        != (Rect {
            x: 10,
            y: 20,
            w: 102,
            h: 67,
        })
        || content
            != (Rect {
                x: 11,
                y: 36,
                w: 100,
                h: 50,
            })
    {
        return Err(4);
    }

    // 2. Second open is on top and focused; the first window is unchanged.
    let two = table.open(SCREEN_W, SCREEN_H, 8, 300, 200, 160, 90, b"two");
    if two != Ok(2) || table.focus != 2 {
        return Err(5);
    }
    if table.frame_rect(1)
        != Ok(Rect {
            x: 10,
            y: 20,
            w: 102,
            h: 67,
        })
    {
        return Err(6);
    }

    // 3. A non-owner cannot close or move; focus and geometry stay put.
    if table.close(8, 1) != Status::Invalid
        || table.move_to(SCREEN_W, SCREEN_H, 8, 1, 0, 0) != Status::Invalid
    {
        return Err(7);
    }
    if table.focus != 2
        || table.frame_rect(1)
            != Ok(Rect {
                x: 10,
                y: 20,
                w: 102,
                h: 67,
            })
    {
        return Err(8);
    }

    // 4. Raise is any task's and focuses.
    if table.raise(1) != Status::Ok || table.focus != 1 {
        return Err(9);
    }

    // 5. Move clamps and does not change focus.
    if table.move_to(SCREEN_W, SCREEN_H, 7, 1, 5, 6) != Status::Ok || table.focus != 1 {
        return Err(10);
    }
    if table.frame_rect(1)
        != Ok(Rect {
            x: 5,
            y: 6,
            w: 102,
            h: 67,
        })
        || table.content_rect(1)
            != Ok(Rect {
                x: 6,
                y: 22,
                w: 100,
                h: 50,
            })
    {
        return Err(11);
    }

    // 6. Closing the focused window falls back to the topmost remaining one.
    if table.close(7, 1) != Status::Ok || table.focus != 2 {
        return Err(12);
    }
    if table.close(8, 2) != Status::Ok || table.focus != 0 || table.count != 0 {
        return Err(13);
    }

    // 7. Origins are clamped; a negative origin lands at (0, 0).
    let clamped = table.open(SCREEN_W, SCREEN_H, 7, -50, -50, 100, 50, b"c");
    if clamped != Ok(3) {
        return Err(14);
    }
    if table.frame_rect(3)
        != Ok(Rect {
            x: 0,
            y: 0,
            w: 102,
            h: 67,
        })
    {
        return Err(15);
    }
    if table.close(7, 3) != Status::Ok {
        return Err(16);
    }

    // 8. Minimum content and an oversized frame are refused.
    if table.open(SCREEN_W, SCREEN_H, 7, 0, 0, 7, 50, b"") != Err(Status::Invalid)
        || table.open(SCREEN_W, SCREEN_H, 7, 0, 0, 100, 752, b"") != Err(Status::Invalid)
    {
        return Err(17);
    }

    // 9. Unknown ids, including 0, are NotFound; a disabled screen is Invalid.
    if table.close(7, 0) != Status::NotFound
        || table.raise(99) != Status::NotFound
        || table.frame_rect(99) != Err(Status::NotFound)
        || table.open(0, 0, 7, 0, 0, 100, 50, b"") != Err(Status::Invalid)
    {
        return Err(18);
    }

    // 10. Task termination closes every window the task owns.
    if table.open(SCREEN_W, SCREEN_H, 9, 0, 0, 100, 50, b"a") != Ok(4)
        || table.open(SCREEN_W, SCREEN_H, 9, 200, 100, 100, 50, b"b") != Ok(5)
        || table.focus != 5
    {
        return Err(19);
    }
    table.close_task(9);
    if table.count != 0 || table.focus != 0 {
        return Err(20);
    }

    // 11. A long title is truncated at 64 bytes (all ASCII).
    let long = [b'x'; 100];
    let truncated = table.open(SCREEN_W, SCREEN_H, 7, 0, 0, 100, 50, &long);
    if truncated != Ok(6) {
        return Err(21);
    }
    let pos = table.position(6).ok_or(22u32)?;
    if table.windows[table.order[pos]].title_len != TITLE_CAP {
        return Err(23);
    }
    table.close_task(7);

    // 12. The table is full at 16 and frees on close.
    let mut index = 0;
    while index < MAX_WINDOWS {
        if table.open(SCREEN_W, SCREEN_H, 12, 0, 0, 100, 50, b"f") != Ok(7 + index as u64) {
            return Err(24);
        }
        index += 1;
    }
    if table.open(SCREEN_W, SCREEN_H, 12, 0, 0, 100, 50, b"full") != Err(Status::Full) {
        return Err(25);
    }
    table.close_task(12);
    if table.count != 0 || table.focus != 0 {
        return Err(26);
    }

    Ok(())
}

/// Boot self-check for composition and repair (spec 0011, W11): the damage
/// lists, their union/merge rules, the visible-part subtraction, and the damage
/// each lifecycle operation records. Pure: it works on a scratch table and
/// never touches the screen.
///
/// Returns the first failing step, or `Ok(())`; `kmain` prints
/// `window: paint ok` and halts on a mismatch.
pub fn paint_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;

    // 1. Erase damage merges touching requests with the same coverage limit and
    //    keeps a different limit separate.
    let mut erases = EraseList::new();
    erases.add(
        Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        },
        1,
    );
    erases.add(
        Rect {
            x: 10,
            y: 0,
            w: 10,
            h: 10,
        },
        1,
    );
    if erases.count != 1
        || erases.entries[0].rect
            != (Rect {
                x: 0,
                y: 0,
                w: 20,
                h: 10,
            })
    {
        return Err(1);
    }
    erases.add(
        Rect {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        },
        0,
    );
    if erases.count != 2 {
        return Err(2);
    }

    // 2. Chrome damage merges touching rectangles and keeps a disjoint one.
    let mut chrome = ChromeList::new();
    chrome.add(Rect {
        x: 0,
        y: 0,
        w: 4,
        h: 4,
    });
    chrome.add(Rect {
        x: 4,
        y: 0,
        w: 4,
        h: 4,
    });
    if chrome.count != 1
        || chrome.rects[0]
            != (Rect {
                x: 0,
                y: 0,
                w: 8,
                h: 4,
            })
    {
        return Err(3);
    }
    chrome.add(Rect {
        x: 20,
        y: 20,
        w: 2,
        h: 2,
    });
    if chrome.count != 2 {
        return Err(4);
    }

    // 3. Visible parts: a frame cut out of a larger rectangle leaves four
    //    bands that do not touch the frame and preserve the exact area.
    let mut table = Table::new();
    let id = table
        .open(SCREEN_W, SCREEN_H, 5, 100, 100, 80, 60, b"vis")
        .map_err(|_| 5u32)?;
    let frame = table.frame_rect(id).map_err(|_| 6u32)?;
    let outer = Rect {
        x: frame.x - 2,
        y: frame.y - 2,
        w: frame.w + 4,
        h: frame.h + 4,
    };
    let parts = table.visible_parts(outer, 0);
    if parts.count != 4 {
        return Err(7);
    }
    let mut area = 0i64;
    for part in parts.rects[..parts.count].iter() {
        if intersects(*part, frame) {
            return Err(8);
        }
        area += part.w * part.h;
    }
    if area != outer.w * outer.h - frame.w * frame.h {
        return Err(9);
    }
    if table.visible_parts(frame, 0).count != 0 {
        return Err(10);
    }
    table.close_task(5);

    // 4. Lifecycle damage. Opening records the frame for the desktop restore
    //    (with no coverage subtraction) and for the chrome, plus the previous
    //    focus title bar.
    let mut table = Table::new();
    let a = table
        .open(SCREEN_W, SCREEN_H, 1, 100, 100, 200, 120, b"Alpha")
        .map_err(|_| 11u32)?;
    let fa = table.frame_rect(a).map_err(|_| 12u32)?;
    if !table.chrome.covers(fa) || !table.erases.covers(fa, table.count) {
        return Err(13);
    }

    table.clear_damage();
    let b = table
        .open(SCREEN_W, SCREEN_H, 1, 200, 160, 200, 120, b"Beta")
        .map_err(|_| 14u32)?;
    let fb = table.frame_rect(b).map_err(|_| 15u32)?;
    let ta = title_of(&table, a).ok_or(16u32)?;
    if !table.chrome.covers(fb) || !table.chrome.covers(ta) {
        return Err(17);
    }

    // Raising `a` repaints its frame, the old focus title and the overlapping
    // window's frame.
    table.clear_damage();
    if table.raise(a) != Status::Ok {
        return Err(18);
    }
    let tb = title_of(&table, b).ok_or(19u32)?;
    if !table.chrome.covers(fa) || !table.chrome.covers(tb) || !table.chrome.covers(fb) {
        return Err(20);
    }

    // Moving the top window records both frames for the desktop restore and
    // the chrome; the new frame subtracts only the windows above it.
    table.clear_damage();
    let old = table.frame_rect(a).map_err(|_| 21u32)?;
    if table.move_to(SCREEN_W, SCREEN_H, 1, a, 300, 220) != Status::Ok {
        return Err(22);
    }
    let new = table.frame_rect(a).map_err(|_| 23u32)?;
    if !table.erases.covers(old, 0) || !table.erases.covers(new, table.count) {
        return Err(24);
    }
    if !table.chrome.covers(old) || !table.chrome.covers(new) {
        return Err(25);
    }

    // Closing the focused top window restores the desktop under it and moves
    // the focused title bar to the fallback window.
    table.clear_damage();
    if table.close(1, a) != Status::Ok || table.focus != b {
        return Err(26);
    }
    let tb = title_of(&table, b).ok_or(27u32)?;
    if !table.erases.covers(new, 0) || !table.chrome.covers(new) || !table.chrome.covers(tb) {
        return Err(28);
    }

    // A refused operation records no damage.
    table.clear_damage();
    if table.raise(999) != Status::NotFound
        || table.close(1, 999) != Status::NotFound
        || table.close(2, b) != Status::Invalid
        || table.move_to(SCREEN_W, SCREEN_H, 2, b, 0, 0) != Status::Invalid
        || table.erases.count != 0
        || table.chrome.count != 0
    {
        return Err(29);
    }

    table.close_task(1);
    if table.count != 0 {
        return Err(30);
    }

    Ok(())
}

/// The title-bar rectangle of a window id in a scratch table.
fn title_of(table: &Table, id: u64) -> Option<Rect> {
    table
        .position(id)
        .map(|pos| title_rect(&table.windows[table.order[pos]]))
}

/// Boot self-check for routing and expose ownership (spec 0011, W12/W16/W17):
/// pending-expose marking, owner isolation, topmost-first take order, the
/// hit-test order and the loss of an expose when its window closes. Pure: it
/// works on a scratch table and never touches the screen.
///
/// Returns the first failing step, or `Ok(())`; `kmain` prints
/// `window: route ok` and halts on a mismatch.
pub fn route_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;

    // Two windows of different owners, not overlapping.
    let mut table = Table::new();
    let a = table
        .open(SCREEN_W, SCREEN_H, 1, 10, 10, 100, 50, b"A")
        .map_err(|_| 1u32)?;
    let b = table
        .open(SCREEN_W, SCREEN_H, 2, 400, 400, 100, 50, b"B")
        .map_err(|_| 2u32)?;
    if a != 1 || b != 2 {
        return Err(3);
    }

    // Raising A damages A (its frame) and B (the old focus title bar). The
    // expose is per window and only its owner can take it.
    table.clear_damage();
    if table.raise(a) != Status::Ok {
        return Err(4);
    }
    if !table.mark_exposes() {
        return Err(5);
    }
    if !table.has_expose(1) || !table.has_expose(2) {
        return Err(6);
    }
    if table.take_expose(1) != Some(a) {
        return Err(7);
    }
    if table.has_expose(1) {
        return Err(8);
    }
    if !table.has_expose(2) {
        return Err(9);
    }
    if table.take_expose(2) != Some(b) {
        return Err(10);
    }
    if table.has_expose(2) {
        return Err(11);
    }

    // Hit-testing the topmost frame.
    if table.hit_test(50, 20) != Some(a) || table.hit_test(450, 410) != Some(b) {
        return Err(12);
    }
    if table.hit_test(900, 700).is_some() {
        return Err(13);
    }
    if table.move_to(SCREEN_W, SCREEN_H, 2, b, 20, 20) != Status::Ok {
        return Err(14);
    }
    // A was raised above B earlier, so the overlap still hits A.
    if table.hit_test(30, 30) != Some(a) {
        return Err(15);
    }
    table.clear_damage();
    if table.raise(b) != Status::Ok {
        return Err(16);
    }
    if table.hit_test(30, 30) != Some(b) {
        return Err(17);
    }

    // Exposes are taken topmost first and coalesce per window.
    table.mark_exposes();
    if table.take_expose(1) != Some(a) {
        return Err(18);
    }
    if table.take_expose(2) != Some(b) {
        return Err(19);
    }
    // No new damage: a second pass marks nothing.
    table.clear_damage();
    if table.mark_exposes() {
        return Err(20);
    }

    // A closed window loses its pending expose.
    let c = table
        .open(SCREEN_W, SCREEN_H, 3, 600, 100, 100, 50, b"C")
        .map_err(|_| 21u32)?;
    table.clear_damage();
    if table.raise(c) != Status::Ok {
        return Err(22);
    }
    table.mark_exposes();
    if !table.has_expose(3) {
        return Err(23);
    }
    if table.close(3, c) != Status::Ok {
        return Err(24);
    }
    if table.take_expose(3).is_some() {
        return Err(25);
    }

    Ok(())
}

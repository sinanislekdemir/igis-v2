//! The window service: records, z-order, focus and ownership (spec 0011).
//!
//! There is no per-window canvas (ADR 0020): a window is a small record in a
//! fixed table, and the application draws into the one shadow through `syslib`.
//! This module owns the table and its policy:
//!
//! - a window has a frame origin, a **content** size, a title and an owner task;
//! - the frame is derived: content plus a 1-pixel border and a 16-pixel title
//!   bar, so `frame = (x, y, w + 2, h + 17)`;
//! - one bottom-to-top z-order, one focus (an id, or 0), and monotonic ids that
//!   are never reused;
//! - `close`/`move` are owner-only, `raise` is any task's, and
//!   [`close_task`] removes a terminating task's windows.
//!
//! Composition and repair (chrome, damage, `present`) belong to M2.3.2 and are
//! not here; the clip helpers they use live in [`crate::gfx`].
//!
//! # Locking
//!
//! The table is one machine-wide [`SpinLock`]. The wrappers read the screen size
//! (a graphics lock) *before* taking the table lock, and nothing takes the two in
//! the other order, so there is no cycle. `Task::reclaim` calls [`close_task`]
//! with no scheduler lock held.
//!
//! # Self-check
//!
//! [`self_check`] runs the same policy on a scratch table with fixed geometry,
//! so ownership, focus fallback, clamping and the limits are proved without an
//! application; `kmain` halts on a mismatch.

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
}

impl Table {
    const fn new() -> Self {
        Table {
            windows: [Window::EMPTY; MAX_WINDOWS],
            order: [0; MAX_WINDOWS],
            count: 0,
            focus: 0,
            next_id: 1,
        }
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

        self.windows[slot] = window;
        self.order[self.count] = slot;
        self.count += 1;
        self.focus = id;
        Ok(id)
    }

    fn close(&mut self, owner: u64, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        if self.windows[self.order[pos]].owner != owner {
            return Status::Invalid;
        }
        self.remove_at(pos);
        Status::Ok
    }

    fn raise(&mut self, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        let slot = self.order[pos];
        for index in pos..self.count - 1 {
            self.order[index] = self.order[index + 1];
        }
        self.order[self.count - 1] = slot;
        self.focus = id;
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
        window.x = x.clamp(0, screen_w - frame.w);
        window.y = y.clamp(0, screen_h - frame.h);
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
    /// each removal (W8).
    fn close_task(&mut self, owner: u64) {
        let mut pos = 0;
        while pos < self.count {
            if self.windows[self.order[pos]].owner == owner {
                self.remove_at(pos);
            } else {
                pos += 1;
            }
        }
    }
}

static TABLE: SpinLock<Table> = SpinLock::new(Table::new());

/// Opens a window owned by `owner`: frame origin `(x, y)`, content `(w, h)`.
///
/// A disabled screen has no geometry, so `open` is `Invalid` (W13). The screen
/// size is read before the table lock is taken.
pub fn open(owner: u64, x: i64, y: i64, w: i64, h: i64, title: &[u8]) -> Result<u64, Status> {
    let (screen_w, screen_h) = gfx::size();
    TABLE
        .lock()
        .open(screen_w as i64, screen_h as i64, owner, x, y, w, h, title)
}

/// Closes a window; only its owner may (W7).
pub fn close(owner: u64, id: u64) -> Status {
    TABLE.lock().close(owner, id)
}

/// Raises and focuses a window; any task may (W7).
pub fn raise(id: u64) -> Status {
    TABLE.lock().raise(id)
}

/// Moves a window owned by `owner`, clamping the origin on screen (W4/W7).
pub fn move_to(owner: u64, id: u64, x: i64, y: i64) -> Status {
    let (screen_w, screen_h) = gfx::size();
    TABLE
        .lock()
        .move_to(screen_w as i64, screen_h as i64, owner, id, x, y)
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

/// Closes every window owned by a terminating task. Called from
/// `Task::reclaim`, so it covers normal exit, kill, fault, a failed spawn and a
/// stale-slot eviction (W8).
pub fn close_task(owner: u64) {
    TABLE.lock().close_task(owner);
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

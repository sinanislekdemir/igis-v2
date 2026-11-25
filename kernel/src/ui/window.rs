//! The window service: records, lifecycle and composition (spec 0011, 0026).
//!
//! Every window owns a retained **canvas** (spec 0026, C1; ADR 0060). The
//! application draws into it through its draw scope; the service owns the
//! screen. This module owns the table, its policy, the service-drawn chrome and
//! the one compositor:
//!
//! - a window has a frame origin, a **content** size, a title and an owner task;
//! - the frame is derived: content plus a 1-pixel border and a 16-pixel title
//!   bar, so `frame = (x, y, w + 2, h + 17)`;
//! - one bottom-to-top z-order, one focus (an id, or 0), and monotonic ids that
//!   are never reused;
//! - `close`/`move` are owner-only, `raise` is any task's, and
//!   [`close_task`] removes a terminating task's windows and planes;
//! - a **plane** is a screen rectangle above every window with its own canvas,
//!   created by `plane_create` and composed after the windows (spec 0026, C11);
//! - every mutation records damage (bounded erase/chrome lists and a `full`
//!   flag) and runs one synchronous recomposition under the table lock. The
//!   compositor paints the desktop pattern, then every live window's chrome and
//!   canvas bottom to top, then every presented plane, and presents the
//!   rectangles it was given.
//!
//! Client content is retained: a lower window repainting cannot reach a higher
//! window, and a move is an origin change, not a pixel copy (spec 0026, C10).
//!
//! Owner `0` is reserved for the kernel (spec 0012, D6): task ids start at `1`,
//! and the kernel's terminal and system-log windows use the same public
//! functions as an application window.
//!
//! # Locking
//!
//! The window table is one machine-wide [`SpinLock`]; the plane stack is a
//! second one, always taken after it. The wrappers read the screen size (the
//! graphics lock) before taking the table lock; a composition takes the table
//! lock, then the plane lock, then calls `gfx` (`WINDOW → PLANES → SCREEN`).
//! No path takes the graphics lock and then the window lock, so there is no
//! cycle. `Task::reclaim` calls [`close_task`] with no scheduler lock held.
//!
//! # Self-checks
//!
//! [`self_check`] runs the table policy on a scratch table, and
//! [`paint_self_check`] runs the damage arithmetic on scratch state; neither
//! touches the screen. `kmain` halts on a mismatch.

use abi::{Rect, Status};

use crate::frame;
use crate::gfx;
use crate::sync::SpinLock;

/// Maximum live windows machine-wide (spec 0011, W9).
pub const MAX_WINDOWS: usize = 16;

/// Maximum live planes machine-wide (spec 0026, C11).
pub const MAX_PLANES: usize = 8;

/// Title capacity in bytes; longer titles are truncated (W9).
pub const TITLE_CAP: usize = abi::layout::window::TITLE_CAP as usize;

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

/// A read-only window snapshot for `syslib.window_info` (spec 0011, W19;
/// spec 0012, D15). `title` is zero-padded and `title_len` names the used bytes.
#[derive(Clone, Copy)]
pub struct Info {
    /// The live window id.
    pub id: u64,
    /// The whole frame rectangle.
    pub frame: Rect,
    /// Used bytes of `title`, at most [`TITLE_CAP`].
    pub title_len: usize,
    /// Truncated title bytes.
    pub title: [u8; TITLE_CAP],
}

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
    /// The close box was pressed and the owner has not taken the request yet
    /// (spec 0011, W23/W25).
    close_pending: bool,
    /// The window is minimized: hidden from hit-testing and painting but still
    /// enumerated at the bottom of the z-order so the window list can restore
    /// it (M11 minimize).
    minimized: bool,
    /// The task that holds this window's draw scope, or `0` (spec 0026, C3).
    /// There is at most one scope per task, so setting a scope clears the
    /// caller's previous window. `0` is also the kernel's owner id, which has
    /// no scope.
    drawer: u64,
    /// Retained content pixels (spec 0026, C1; ADR 0060). `None` only in the
    /// boot self-checks' scratch tables, which never draw.
    canvas: Option<gfx::Canvas>,
    /// At least one present has happened; the compositor leaves the pixels
    /// beneath untouched until then (spec 0026, C15).
    canvas_valid: bool,
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
        close_pending: false,
        minimized: false,
        drawer: 0,
        canvas: None,
        canvas_valid: false,
        used: false,
    };

    /// The canvas positioned at the window's current content origin. A move is
    /// an origin change here; the retained pixels do not move (spec 0026,
    /// C10).
    fn canvas_at(&self) -> Option<gfx::Canvas> {
        self.canvas.map(|canvas| {
            let content = self.content();
            gfx::Canvas {
                ox: content.x,
                oy: content.y,
                ..canvas
            }
        })
    }

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

/// Allocates a zeroed canvas of `w × h` pixels from the frame allocator
/// (spec 0026, C1/C2). There is no budget constant: a failed allocation is the
/// only failure and the caller reports `Status::Full`.
fn alloc_canvas(w: i64, h: i64) -> Option<gfx::Canvas> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let bytes = (w as usize)
        .checked_mul(h as usize)?
        .checked_mul(4)?;
    let frames = bytes.div_ceil(frame::FRAME_SIZE as usize);
    let base = frame::alloc_contiguous(frames)?;
    // SAFETY: `alloc_contiguous` reserved exactly these contiguous,
    // identity-mapped frames for this canvas; zeroing them is the only write
    // before the window record owns them.
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, bytes) };
    Some(gfx::Canvas {
        base: base as *mut u8,
        bytes,
        pitch: (w as usize) * 4,
        w,
        h,
        ox: 0,
        oy: 0,
    })
}

/// Returns a canvas's frames to the allocator. A canvas with no bytes (a boot
/// self-check's placeholder) frees nothing.
fn free_canvas(canvas: gfx::Canvas) {
    if canvas.bytes == 0 {
        return;
    }
    let frames = canvas.bytes.div_ceil(frame::FRAME_SIZE as usize);
    frame::free_contiguous(canvas.base as u64, frames);
}

/// One plane record (spec 0026, C11): a screen rectangle above every window
/// with its own retained canvas. Only the boot self-check's scratch tables use
/// `canvas: None`.
#[derive(Clone, Copy)]
struct Plane {
    id: u64,
    owner: u64,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    /// The task that holds this plane's draw scope, or `0` (spec 0026, C3).
    drawer: u64,
    canvas: Option<gfx::Canvas>,
    /// At least one present has happened; the compositor skips the plane until
    /// then (spec 0026, C15).
    canvas_valid: bool,
}

impl Plane {
    const EMPTY: Plane = Plane {
        id: 0,
        owner: 0,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        drawer: 0,
        canvas: None,
        canvas_valid: false,
    };

    fn rect(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }

    /// The canvas positioned at the plane's current screen origin.
    fn canvas_at(&self) -> Option<gfx::Canvas> {
        self.canvas.map(|canvas| gfx::Canvas {
            ox: self.x,
            oy: self.y,
            ..canvas
        })
    }

    fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.w)
            && y < self.y.saturating_add(self.h)
    }
}

/// The machine-wide plane stack behind its own lock (spec 0026, C11; ADR
/// 0060). The array is compact and in creation order, so index order is
/// composite order: a later plane is above an earlier one. The window table
/// lock is always taken first (`WINDOW → PLANES → SCREEN`); the compositor and
/// every plane wrapper follow that order.
struct PlaneTable {
    planes: [Plane; MAX_PLANES],
    count: usize,
    /// Next id to assign; starts at 1 and skips 0 on wrap.
    next_id: u64,
}

impl PlaneTable {
    const fn new() -> Self {
        PlaneTable {
            planes: [Plane::EMPTY; MAX_PLANES],
            count: 0,
            next_id: 1,
        }
    }

    fn position(&self, id: u64) -> Option<usize> {
        if id == 0 {
            return None;
        }
        (0..self.count).find(|&pos| self.planes[pos].id == id)
    }

    /// Appends a plane above the existing ones. The caller has already
    /// validated the rectangle and allocated the canvas; the limit is checked
    /// before a canvas is allocated in [`plane_create`].
    fn create(
        &mut self,
        owner: u64,
        rect: Rect,
        canvas: Option<gfx::Canvas>,
    ) -> Result<u64, Status> {
        if self.count == MAX_PLANES {
            return Err(Status::Full);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }
        self.planes[self.count] = Plane {
            id,
            owner,
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            canvas,
            ..Plane::EMPTY
        };
        self.count += 1;
        Ok(id)
    }

    /// Removes the caller's plane, frees its canvas and returns the vacated
    /// rectangle for recomposition. Owner-only: `NotFound` for an unknown id
    /// and `Invalid` for another task's plane.
    fn destroy(&mut self, owner: u64, id: u64) -> Result<Rect, Status> {
        let Some(pos) = self.position(id) else {
            return Err(Status::NotFound);
        };
        if self.planes[pos].owner != owner {
            return Err(Status::Invalid);
        }
        let plane = self.planes[pos];
        if let Some(canvas) = plane.canvas {
            free_canvas(canvas);
        }
        for index in pos..self.count - 1 {
            self.planes[index] = self.planes[index + 1];
        }
        self.count -= 1;
        self.planes[self.count] = Plane::EMPTY;
        Ok(plane.rect())
    }

    /// Sets `owner`'s one draw scope to a plane. The caller still owns the
    /// window scope clearing (the table handles its own records).
    fn begin(&mut self, owner: u64, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        if self.planes[pos].owner != owner {
            return Status::Invalid;
        }
        self.clear_drawer(owner);
        self.planes[pos].drawer = owner;
        Status::Ok
    }

    /// Marks the plane's first/next present and returns its rectangle.
    fn present(&mut self, owner: u64, id: u64) -> Result<Rect, Status> {
        let Some(pos) = self.position(id) else {
            return Err(Status::NotFound);
        };
        if self.planes[pos].owner != owner {
            return Err(Status::Invalid);
        }
        self.planes[pos].canvas_valid = true;
        Ok(self.planes[pos].rect())
    }

    fn clear_drawer(&mut self, owner: u64) {
        if owner == 0 {
            return;
        }
        for index in 0..self.count {
            if self.planes[index].drawer == owner {
                self.planes[index].drawer = 0;
            }
        }
    }

    /// Removes every plane owned by `owner`, freeing its canvas and recording
    /// the vacated rectangles into `out`. Returns how many were removed.
    fn close_owner(&mut self, owner: u64, out: &mut [Rect; MAX_PLANES]) -> usize {
        let mut count = 0;
        let mut pos = 0;
        while pos < self.count {
            if self.planes[pos].owner == owner {
                if let Some(canvas) = self.planes[pos].canvas {
                    free_canvas(canvas);
                }
                out[count] = self.planes[pos].rect();
                count += 1;
                for index in pos..self.count - 1 {
                    self.planes[index] = self.planes[index + 1];
                }
                self.count -= 1;
                self.planes[self.count] = Plane::EMPTY;
            } else {
                pos += 1;
            }
        }
        count
    }

    /// Above every window: the topmost (latest) plane containing the point
    /// owns a press there (spec 0026, C12).
    fn owner_at(&self, x: i64, y: i64) -> Option<u64> {
        for index in (0..self.count).rev() {
            if self.planes[index].contains(x, y) {
                return Some(self.planes[index].owner);
            }
        }
        None
    }

    /// Clamps every plane origin into the screen after a mode change, keeping
    /// the retained canvas unchanged (spec 0026, C17). A plane larger than the
    /// new screen starts at 0 and is clipped by the compositor.
    fn clamp_to(&mut self, screen_w: i64, screen_h: i64) {
        for index in 0..self.count {
            let plane = &mut self.planes[index];
            plane.x = plane.x.clamp(0, (screen_w - plane.w).max(0));
            plane.y = plane.y.clamp(0, (screen_h - plane.h).max(0));
        }
    }

    /// The number of planes that would be painted (presented at least once),
    /// bottom to top. Used by the boot self-check; the compositor applies the
    /// same `canvas_valid` rule.
    fn composited(&self) -> usize {
        (0..self.count)
            .filter(|&index| self.planes[index].canvas_valid)
            .count()
    }
}

/// The one machine-wide plane stack (spec 0026, C11).
static PLANES: SpinLock<PlaneTable> = SpinLock::new(PlaneTable::new());

/// Clamps a requested plane rectangle to the screen. `None` when the rectangle
/// is non-positive or entirely off screen (spec 0026, C11).
fn clamp_plane(screen_w: i64, screen_h: i64, x: i64, y: i64, w: i64, h: i64) -> Option<Rect> {
    if screen_w <= 0 || screen_h <= 0 || w <= 0 || h <= 0 {
        return None;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = x.saturating_add(w).min(screen_w);
    let y1 = y.saturating_add(h).min(screen_h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// An in-progress pointer drag (spec 0011, W22): the window and the grab
/// offset from its frame origin. Set by a left-button press in the title bar
/// (outside the close box) and cleared on release, on a second owner change or
/// when the window disappears.
#[derive(Clone, Copy)]
struct Drag {
    id: u64,
    offset_x: i64,
    offset_y: i64,
    /// Whether any step actually changed the origin. A release with no motion
    /// ends the drag without requesting a full repaint.
    moved: bool,
}

/// The one machine-wide modal popup input grab (spec 0011, W26; ADR 0044).
///
/// An application-drawn popup (a pull-down menu) has no window, so without a
/// grab a click in it would be routed to the window underneath. While a grab is
/// held the window service ignores pointer presses and only the owner receives
/// queued input events; every other task's input readiness is false. The
/// rectangle is advisory: the owner decides inside/outside itself, and routing
/// is modal.
#[derive(Clone, Copy)]
struct Grab {
    owner: u64,
    rect: Rect,
}

/// What a left-button press did to the service (spec 0011, W22/W23). Only
/// `None` leaves the click to the client; the other outcomes consume the
/// queued `PointerDown`.
enum PressAction {
    /// No window interaction: the client sees the press.
    None,
    /// The service handled the press (title-bar drag or an owner-0 close box).
    Consumed,
    /// The close box of a task-owned window was pressed: the window's
    /// `close_pending` flag is set and its owner is asked to close it.
    Close,
    /// The minimize box was pressed: the window is hidden at the bottom of the
    /// z-order and can be restored from the window list.
    Minimize,
}

/// The close box is an 11 × 11 square at `(frame.x + 4, frame.y + 3)` (W22).
fn close_box_hit(frame: Rect, x: i64, y: i64) -> bool {
    x >= frame.x.saturating_add(4)
        && y >= frame.y.saturating_add(3)
        && x < frame.x.saturating_add(4 + 11)
        && y < frame.y.saturating_add(3 + 11)
}

/// The minimize box is an 11 × 11 square at the right end of the title bar,
/// `(frame.x + frame.w - 15, frame.y + 3)`.
fn hide_box_hit(frame: Rect, x: i64, y: i64) -> bool {
    x >= frame.x.saturating_add(frame.w).saturating_sub(15)
        && y >= frame.y.saturating_add(3)
        && x < frame.x.saturating_add(frame.w).saturating_sub(4)
        && y < frame.y.saturating_add(3 + 11)
}

/// The title bar is the 16-pixel band from the frame origin, including the
/// border and the close box (W3/W22). The close box is checked first.
fn title_bar_hit(frame: Rect, x: i64, y: i64) -> bool {
    x >= frame.x
        && y >= frame.y
        && x < frame.x.saturating_add(frame.w)
        && y < frame.y.saturating_add(TITLE_BAR_HEIGHT)
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

/// Whether `outer` fully contains `inner` (equal edges count).
fn contains(outer: Rect, inner: Rect) -> bool {
    inner.w <= 0
        || inner.h <= 0
        || (outer.x <= inner.x
            && outer.y <= inner.y
            && outer.x.saturating_add(outer.w) >= inner.x.saturating_add(inner.w)
            && outer.y.saturating_add(outer.h) >= inner.y.saturating_add(inner.h))
}

/// Greedy coalescing of a damage rectangle list (spec 0011, W11).
///
/// Empty rectangles are dropped, then any overlapping or edge/corner-touching
/// pair is replaced by its bounding box and the pass repeats until stable, so
/// contained rectangles disappear and a chain of touching pieces becomes one
/// piece. `MAX_DAMAGE` is small (32), so the bounded O(n^3) worst case costs
/// little and runs once per repair.
///
/// The output covers the input union: a merge only grows the covered area. The
/// added area of a diagonal merge is re-presented from the current shadow (and
/// re-painted from the window records), never left stale, so growth is safe.
fn coalesce<const N: usize>(rects: &mut [Rect; N], count: usize) -> usize {
    let mut count = count.min(N);
    let mut write = 0;
    for read in 0..count {
        if rects[read].w > 0 && rects[read].h > 0 {
            rects[write] = rects[read];
            write += 1;
        }
    }
    count = write;
    loop {
        let mut merged = false;
        'pairs: for left in 0..count {
            for right in (left + 1)..count {
                if touches(rects[left], rects[right]) {
                    rects[left] = union(rects[left], rects[right]);
                    rects[right] = rects[count - 1];
                    count -= 1;
                    merged = true;
                    break 'pairs;
                }
            }
        }
        if !merged {
            break;
        }
    }
    count
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

    /// Merges duplicate work before the paint pass (spec 0011, W11).
    ///
    /// Equal-coverage requests that touch are merged into their bounding box.
    /// A request contained in one whose coverage limit is at least as high is
    /// dropped: the covering request subtracts no more windows, so it paints
    /// the desktop across every pixel the contained one would. Different
    /// coverage limits are never merged, because their subtraction sets differ.
    fn coalesce(&mut self) {
        loop {
            let mut changed = false;
            'restart: for left in 0..self.count {
                for right in 0..self.count {
                    if left == right {
                        continue;
                    }
                    let a = self.entries[left];
                    let b = self.entries[right];
                    if contains(a.rect, b.rect) && a.covers_from >= b.covers_from {
                        self.entries[right] = self.entries[self.count - 1];
                        self.count -= 1;
                        changed = true;
                        break 'restart;
                    }
                    if a.covers_from == b.covers_from && touches(a.rect, b.rect) {
                        self.entries[left].rect = union(a.rect, b.rect);
                        self.entries[right] = self.entries[self.count - 1];
                        self.count -= 1;
                        changed = true;
                        break 'restart;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        if self.count == 0 {
            self.clear();
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

    /// Merges duplicate chrome work before the paint/present pass (spec 0011,
    /// W11). Chrome damage carries no coverage limit, so the generic greedy
    /// coalescing applies: contained rectangles disappear and touching pieces
    /// become one.
    fn coalesce(&mut self) {
        self.count = coalesce(&mut self.rects, self.count);
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

/// Coalescing model kept for the boot self-check (spec 0011, W11): it measures
/// how much present area the coalesced damage union would cover. The repair
/// itself now presents each painted rectangle exactly (W31), because a merged
/// bounding box could copy stale client pixels over a higher window.
const MAX_PRESENT: usize = MAX_DAMAGE * 2;

struct Presentation {
    rects: [Rect; MAX_PRESENT],
    count: usize,
}

impl Presentation {
    const fn new() -> Self {
        Presentation {
            rects: [Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            }; MAX_PRESENT],
            count: 0,
        }
    }

    fn add(&mut self, rect: Rect) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        if self.count < MAX_PRESENT {
            self.rects[self.count] = rect;
            self.count += 1;
        } else {
            // Never drop damage: fold the overflow into the last rectangle.
            let last = MAX_PRESENT - 1;
            self.rects[last] = union(self.rects[last], rect);
        }
    }

    /// Coalesces the accumulated rectangles (contained dropped, touching
    /// merged) and returns the resulting count.
    fn coalesce(&mut self) -> usize {
        self.count = coalesce(&mut self.rects, self.count);
        self.count
    }

    /// Total present area in pixels (the measurement metric; the old
    /// single-bounding-box behaviour is [`Presentation::bounds_area`]).
    fn area(&self) -> i64 {
        self.rects[..self.count]
            .iter()
            .map(|rect| rect.w.saturating_mul(rect.h))
            .sum()
    }

    /// Area of the single bounding box of every accumulated rectangle, i.e.
    /// what the pre-M2.6.2 repair presented.
    fn bounds_area(&self) -> i64 {
        if self.count == 0 {
            return 0;
        }
        let mut rect = self.rects[0];
        for next in self.rects[1..self.count].iter() {
            rect = union(rect, *next);
        }
        rect.w.saturating_mul(rect.h)
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
    /// The pointer drag in progress, if any (W22).
    drag: Option<Drag>,
    /// The modal popup grab, if any (W26).
    grab: Option<Grab>,
    /// The machine-wide reserved top strip; no window frame origin starts above
    /// it (spec 0026, C13). `0` clears it.
    work_area: i64,
    /// Pending desktop restore and chrome damage for the next repair pass.
    erases: EraseList,
    chrome: ChromeList,
    /// A full-stack recomposition is pending: the repair ignores the
    /// accumulated damage and composes the whole screen (W31). Set on a
    /// focus/raise switch and when a move completes.
    full: bool,
}

impl Table {
    const fn new() -> Self {
        Table {
            windows: [Window::EMPTY; MAX_WINDOWS],
            order: [0; MAX_WINDOWS],
            count: 0,
            focus: 0,
            next_id: 1,
            drag: None,
            grab: None,
            work_area: 0,
            erases: EraseList::new(),
            chrome: ChromeList::new(),
            full: false,
        }
    }

    fn clear_damage(&mut self) {
        self.erases.clear();
        self.chrome.clear();
        self.full = false;
    }

    /// Position of `id` in the z-order, or `None`.
    fn position(&self, id: u64) -> Option<usize> {
        if id == 0 {
            return None;
        }
        (0..self.count).find(|&pos| self.windows[self.order[pos]].id == id)
    }

    /// Clears every window scope held by `owner` (spec 0026, C3). Owner `0` is
    /// the kernel and records no scope, so it never clears one. The caller
    /// clears the owner's plane scope separately (`WINDOW → PLANES`).
    fn clear_drawer(&mut self, owner: u64) {
        if owner == 0 {
            return;
        }
        for slot in 0..MAX_WINDOWS {
            if self.windows[slot].used && self.windows[slot].drawer == owner {
                self.windows[slot].drawer = 0;
            }
        }
    }

    /// Sets the caller's window draw scope (spec 0026, C3/C20): `0` clears the
    /// scope and draws nothing (there is no screen surface), another id must be
    /// one of the caller's live windows. There is at most one scope per task,
    /// so the caller's previous window or plane scope is lost.
    fn begin(&mut self, owner: u64, id: u64) -> Status {
        if id == 0 {
            self.clear_drawer(owner);
            return Status::Ok;
        }
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        let slot = self.order[pos];
        if self.windows[slot].owner != owner {
            return Status::Invalid;
        }
        self.clear_drawer(owner);
        self.windows[slot].drawer = owner;
        Status::Ok
    }

    /// Removes the window at `pos` and updates the focus fallback (W6). Returns
    /// the freed canvas for the caller to release outside the record.
    fn remove_at(&mut self, pos: usize) -> Option<gfx::Canvas> {
        let slot = self.order[pos];
        let removed = self.windows[slot].id;
        let canvas = self.windows[slot].canvas.take();
        // Replacing the whole record also clears its draw scope (spec 0026,
        // C3): a closed window can no longer be drawn into.
        self.windows[slot] = Window::EMPTY;
        // A dragged window that disappears cancels the drag (W22).
        if self.drag.is_some_and(|drag| drag.id == removed) {
            self.drag = None;
        }
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
        canvas
    }

    /// Validates an `open` request without touching the table. The public
    /// wrappers run it before allocating so an invalid rectangle is `Invalid`,
    /// never `Full`.
    fn can_open(&self, screen_w: i64, screen_h: i64, w: i64, h: i64) -> Result<(), Status> {
        if screen_w <= 0 || screen_h <= 0 {
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
        // A frame taller than the screen minus the reserved strip can never
        // start below it (spec 0026, C13).
        let work_area = self.work_area.min(screen_h);
        if frame_h > screen_h - work_area {
            return Err(Status::Invalid);
        }
        if self.count == MAX_WINDOWS {
            return Err(Status::Full);
        }
        if !(0..MAX_WINDOWS).any(|index| !self.windows[index].used) {
            return Err(Status::Full);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
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
        bottom: bool,
        canvas: Option<gfx::Canvas>,
    ) -> Result<u64, Status> {
        if let Err(status) = self.can_open(screen_w, screen_h, w, h) {
            if let Some(canvas) = canvas {
                free_canvas(canvas);
            }
            return Err(status);
        }
        if let Some(canvas) = canvas {
            if canvas.bytes == 0 || canvas.w != w || canvas.h != h {
                free_canvas(canvas);
                return Err(Status::Invalid);
            }
        }
        let Some(slot) = (0..MAX_WINDOWS).find(|&index| !self.windows[index].used) else {
            if let Some(canvas) = canvas {
                free_canvas(canvas);
            }
            return Err(Status::Full);
        };

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        // The origin is clamped so the whole frame stays on screen and below
        // the machine-wide work area (W4; spec 0026, C13).
        let frame_w = w.saturating_add(BORDER_WIDTH * 2);
        let frame_h = h.saturating_add(TITLE_BAR_HEIGHT + BORDER_WIDTH);
        let work_area = self.work_area.min(screen_h);
        let x = x.clamp(0, screen_w - frame_w);
        let y = y.clamp(work_area, screen_h - frame_h);

        let mut window = Window {
            id,
            owner,
            x,
            y,
            w,
            h,
            canvas,
            ..Window::EMPTY
        };
        // A new window becomes its owner's draw scope (spec 0026, C3): a
        // single-window app need not call `window_begin` before drawing. There
        // is at most one scope per task, so an earlier window loses it. Owner
        // `0` is the kernel and records no scope.
        self.clear_drawer(owner);
        window.drawer = owner;
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
        if bottom {
            // Insert at the bottom of the z-order: the kernel System Log is a
            // screen-sized background window and must not cover an application
            // after a mode change reopened it.
            let mut index = self.count;
            while index > 0 {
                self.order[index] = self.order[index - 1];
                index -= 1;
            }
            self.order[0] = slot;
            self.count += 1;
            if self.focus == 0 {
                self.focus = id;
            }
            self.full = true;
            return Ok(id);
        }
        self.order[self.count] = slot;
        self.count += 1;
        self.focus = id;
        // A new topmost, focused window is a switch: rebuild the whole stack so
        // no window keeps stale pixels from before it appeared (W31).
        self.full = true;

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
        let canvas = self.remove_at(pos);
        if let Some(canvas) = canvas {
            free_canvas(canvas);
        }
        // Closing changes the visible stack (and may move the focus): rebuild
        // every window so the newly exposed pixels are not left stale (W31).
        self.full = true;

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
        let was_minimized = self.windows[slot].minimized;
        let was_top = pos + 1 == self.count;
        self.windows[slot].minimized = false;
        let old_focus = self.focus;
        for index in pos..self.count - 1 {
            self.order[index] = self.order[index + 1];
        }
        self.order[self.count - 1] = slot;
        self.focus = id;
        // A raise that changes the z-order, the focus or a minimized state is a
        // window switch: rebuild the whole stack so every newly uncovered
        // pixel is repainted by its owner (W31). A press on the already-topmost
        // focused window only starts a drag and does not pay for this.
        if !was_top || was_minimized || old_focus != id {
            self.full = true;
        }

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
        _copy_content: bool,
    ) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        if self.windows[self.order[pos]].owner != owner {
            return Status::Invalid;
        }
        let before = self.windows[self.order[pos]];
        let status = self.relocate(screen_w, screen_h, pos, x, y);
        // A completed programmatic move rebuilds the stack (W31); the pointer
        // drag defers the same work to its release.
        let moved = self.windows[self.order[pos]];
        if status == Status::Ok && (moved.x != before.x || moved.y != before.y) {
            self.full = true;
        }
        status
    }

    /// Moves the window at z-order position `pos`, clamping the origin on
    /// screen and recording the same damage as `window_move` (W4/W11).
    ///
    /// The content canvas is window-local, so a move is an origin change and
    /// nothing is copied (spec 0026, C10; ADR 0060): the compositor paints the
    /// same canvas at the new origin and the owner's `Expose` repaints only if
    /// its content needs recomputing.
    fn relocate(
        &mut self,
        screen_w: i64,
        screen_h: i64,
        pos: usize,
        x: i64,
        y: i64,
    ) -> Status {
        let window = &mut self.windows[self.order[pos]];
        let frame = window.frame();
        if screen_w <= 0 || screen_h <= 0 || frame.w > screen_w || frame.h > screen_h {
            return Status::Invalid;
        }
        let work_area = self.work_area.min(screen_h);
        if frame.h > screen_h - work_area {
            return Status::Invalid;
        }
        let new_x = x.clamp(0, screen_w - frame.w);
        let new_y = y.clamp(work_area, screen_h - frame.h);
        if window.x == new_x && window.y == new_y {
            return Status::Ok;
        }
        let topmost = pos + 1 == self.count;
        window.x = new_x;
        window.y = new_y;
        let moved = window.frame();

        // Record both frames as recomposition rectangles: the old frame shows
        // the desktop or whatever window is now underneath, and the new frame
        // shows the retained canvas at its new origin. The owner is still
        // exposed, but only because content may need recomputing (W31).
        self.erases.add(frame, 0);
        if !topmost {
            self.erases.add(moved, pos + 1);
        }
        self.chrome.add(frame);
        self.chrome.add(moved);
        Status::Ok
    }

    /// Handles a left-button press at `(x, y)` (W22/W23).
    ///
    /// Hides a window at the bottom of the z-order. It stays in the table so
    /// the desktop's window list can raise it again. Returns whether the
    /// window was visible.
    fn minimize(&mut self, id: u64) -> bool {
        let Some(pos) = self.position(id) else {
            return false;
        };
        let slot = self.order[pos];
        if self.windows[slot].minimized {
            return false;
        }
        let frame = self.windows[slot].frame();
        self.windows[slot].minimized = true;
        self.windows[slot].close_pending = false;
        // `order` is bottom-first; a minimized window goes to the bottom so
        // `info` lists it last and it covers nothing.
        for index in (1..=pos).rev() {
            self.order[index] = self.order[index - 1];
        }
        self.order[0] = slot;
        if self.focus == id {
            self.focus = self.top_visible().unwrap_or(0);
        }
        self.erases.add(frame, 0);
        self.chrome.add(frame);
        if let Some(pos) = self.position(self.focus) {
            self.chrome.add(title_rect(&self.windows[self.order[pos]]));
        }
        // Hiding a window changes the visible stack; rebuild it so the pixels
        // it uncovered are repainted, not left stale (W31).
        self.full = true;
        true
    }

    /// The topmost non-minimized window id, or `None`.
    fn top_visible(&self) -> Option<u64> {
        for pos in (0..self.count).rev() {
            let window = &self.windows[self.order[pos]];
            if !window.minimized {
                return Some(window.id);
            }
        }
        None
    }

    /// A press on the title bar (outside the close box) raises and focuses the
    /// window like `raise` and starts a drag; a press on a task-owned close box
    /// marks the window for a close request (the owner gets a `Close` event);
    /// an owner-0 close box is consumed but inert. A second press while a drag
    /// is in progress is ignored. Content presses and desktop presses are left
    /// to the client.
    fn press(&mut self, x: i64, y: i64) -> PressAction {
        if self.drag.is_some() {
            // A second press during a drag is swallowed like the first, so it
            // cannot reach a client and raise another window mid-drag (W22).
            return PressAction::Consumed;
        }
        let Some(id) = self.hit_test(x, y) else {
            return PressAction::None;
        };
        let Some(pos) = self.position(id) else {
            return PressAction::None;
        };
        let frame = self.windows[self.order[pos]].frame();
        let owner = self.windows[self.order[pos]].owner;
        if close_box_hit(frame, x, y) {
            if owner == 0 {
                return PressAction::Consumed;
            }
            self.windows[self.order[pos]].close_pending = true;
            return PressAction::Close;
        }
        if hide_box_hit(frame, x, y) {
            let _ = self.minimize(id);
            return PressAction::Minimize;
        }
        if !title_bar_hit(frame, x, y) {
            return PressAction::None;
        }
        if self.raise(id) != Status::Ok {
            return PressAction::None;
        }
        self.drag = Some(Drag {
            id,
            offset_x: x.saturating_sub(frame.x),
            offset_y: y.saturating_sub(frame.y),
            moved: false,
        });
        PressAction::Consumed
    }

    /// Moves the dragged window so its origin follows the pointer minus the
    /// grab offset (W22). Clamps like `move_to` and records the same damage, so
    /// the owner is exposed and the desktop restored underneath.
    fn drag_move(&mut self, screen_w: i64, screen_h: i64, x: i64, y: i64) -> Status {
        let Some(drag) = self.drag else {
            return Status::Ok;
        };
        let Some(pos) = self.position(drag.id) else {
            // The dragged window disappeared (owner terminated/closed): cancel.
            self.drag = None;
            return Status::NotFound;
        };
        let before = self.windows[self.order[pos]];
        let status = self.relocate(
            screen_w,
            screen_h,
            pos,
            x.saturating_sub(drag.offset_x),
            y.saturating_sub(drag.offset_y),
        );
        let moved = self.windows[self.order[pos]];
        if status == Status::Ok && (moved.x != before.x || moved.y != before.y) {
            if let Some(drag) = self.drag.as_mut() {
                drag.moved = true;
            }
        }
        status
    }

    /// Ends a drag; further motion does not move the window (W22). Returns
    /// whether the drag actually moved the window; a real drag requests the
    /// full-stack repaint at release (W31).
    fn end_drag(&mut self) -> bool {
        let moved = self.drag.is_some_and(|drag| drag.moved);
        self.drag = None;
        if moved {
            self.full = true;
        }
        moved
    }

    /// Takes or updates the one machine-wide modal popup grab for `owner`
    /// (W26). `Full` when another task holds it, `Invalid` for a non-positive
    /// extent. A repeat by the owner updates the advisory rectangle.
    fn grab(&mut self, owner: u64, x: i64, y: i64, w: i64, h: i64) -> Status {
        if w <= 0 || h <= 0 {
            return Status::Invalid;
        }
        if self.grab.is_some_and(|grab| grab.owner != owner) {
            return Status::Full;
        }
        // Starting or updating a popup ends any pointer drag: while a grab is
        // held the service performs no window interaction (W26).
        self.drag = None;
        self.grab = Some(Grab {
            owner,
            rect: Rect { x, y, w, h },
        });
        Status::Ok
    }

    /// Releases the caller's grab (W26): `Ok` for the owner, `NotFound` when
    /// none is held, `Invalid` for another task's grab.
    fn release_grab(&mut self, owner: u64) -> Status {
        match self.grab {
            None => Status::NotFound,
            Some(grab) if grab.owner != owner => Status::Invalid,
            Some(_) => {
                self.grab = None;
                Status::Ok
            }
        }
    }

    /// Whether queued input events may be delivered to `owner` (W26): always
    /// without a grab, and only to the owner with one.
    fn grab_allows(&self, owner: u64) -> bool {
        self.grab.is_none_or(|grab| grab.owner == owner)
    }

    /// Whether any task holds the modal grab.
    fn grab_active(&self) -> bool {
        self.grab.is_some()
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

    /// One read-only snapshot by z-order index: `0` is the topmost window
    /// (spec 0011, W19). `None` for an index at or past the live count.
    fn info(&self, index: u64) -> Option<Info> {
        if index >= self.count as u64 {
            return None;
        }
        // `order` is bottom-first, so the topmost snapshot reverses it.
        let pos = self.count - 1 - index as usize;
        let window = &self.windows[self.order[pos]];
        let mut info = Info {
            id: window.id,
            frame: window.frame(),
            title_len: window.title_len,
            title: [0; TITLE_CAP],
        };
        info.title[..window.title_len].copy_from_slice(&window.title[..window.title_len]);
        Some(info)
    }

    /// Removes every window owned by `owner`, applying the focus fallback after
    /// each removal (W8). The damage accumulates and one repair pass covers all
    /// removed frames, so a terminated task's windows disappear together.
    ///
    /// A terminating task's modal popup grab (W26) is released here; its
    /// planes are removed by the public wrapper (they live behind the plane
    /// lock). The caller wakes input waiters when either happened.
    fn close_task(&mut self, owner: u64) -> bool {
        let released_grab = self.grab.is_some_and(|grab| grab.owner == owner);
        if released_grab {
            self.grab = None;
        }
        let mut removed = false;
        let mut pos = 0;
        while pos < self.count {
            if self.windows[self.order[pos]].owner == owner {
                let frame = self.windows[self.order[pos]].frame();
                let canvas = self.remove_at(pos);
                if let Some(canvas) = canvas {
                    free_canvas(canvas);
                }
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
            self.full = true;
            if let Some(pos) = self.position(self.focus) {
                self.chrome.add(title_rect(&self.windows[self.order[pos]]));
            }
        }
        released_grab
    }

    /// The parts of `rect` not covered by live windows at z-order indices
    /// `covers_from..count`. Returns exact pieces (no bounding-box merge), so a
    /// hidden hole is never painted.
    fn visible_parts(&self, rect: Rect, covers_from: usize) -> Parts {
        let mut parts = Parts::new();
        parts.push(rect);
        let mut pos = covers_from;
        while pos < self.count && parts.count > 0 {
            if self.windows[self.order[pos]].minimized {
                pos += 1;
                continue;
            }
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
        if window.minimized {
            return;
        }
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

        // Minimize box: the matching box at the right end, with a short dash.
        let hide_x = frame.x + frame.w - 15;
        let _ = gfx::fill_rect(hide_x, box_y, 11, 11, title_bg);
        let _ = gfx::draw_rect(hide_x, box_y, 11, 11, CHROME_INK);
        let _ = gfx::fill_rect(hide_x + 2, box_y + 4, 7, 2, CHROME_INK);

        // Title text: truncate to the band between the two boxes, then centre
        // it. The band is repainted in the title colour so stripes never cross
        // a glyph. The service chrome uses the proportional font (spec 0015);
        // truncation is by measured width, so narrow letters fit more of the
        // title than the old fixed 8-pixel cells did.
        let available = inner_w - 38;
        let mut chars = 0;
        let mut text_w = 0i64;
        if available >= 4 && window.title_len > 0 {
            while chars < window.title_len {
                let advance = gfx::prop_advance(window.title[chars]);
                if text_w + advance > available {
                    break;
                }
                text_w += advance;
                chars += 1;
            }
        }
        if chars > 0 {
            let band_x = inner_x + 20 + (available - text_w) / 2;
            let _ = gfx::fill_rect(
                band_x - 2,
                frame.y + 2,
                text_w + 4,
                gfx::PROP_LINE_HEIGHT,
                title_bg,
            );
            let _ = gfx::draw_text_prop(band_x, frame.y + 2, &window.title[..chars], CHROME_INK);
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
            if self.windows[slot].minimized {
                continue;
            }
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

    /// Takes the topmost pending close request owned by `owner`, clearing it
    /// (spec 0011, W25). Mirrors `take_expose`: only the owner can take it.
    fn take_close(&mut self, owner: u64) -> Option<u64> {
        for pos in (0..self.count).rev() {
            let slot = self.order[pos];
            if self.windows[slot].owner == owner && self.windows[slot].close_pending {
                self.windows[slot].close_pending = false;
                return Some(self.windows[slot].id);
            }
        }
        None
    }

    /// Whether `owner` has any pending close request.
    fn has_close(&self, owner: u64) -> bool {
        (0..self.count).any(|pos| {
            let slot = self.order[pos];
            self.windows[slot].owner == owner && self.windows[slot].close_pending
        })
    }

    /// The topmost window whose frame contains `(x, y)` (spec 0011, W16).
    fn hit_test(&self, x: i64, y: i64) -> Option<u64> {
        for pos in (0..self.count).rev() {
            let window = &self.windows[self.order[pos]];
            if window.minimized {
                continue;
            }
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

    /// The owner of a live window id, if any (spec 0011, W29).
    fn owner_of(&self, id: u64) -> Option<u64> {
        self.position(id)
            .map(|pos| self.windows[self.order[pos]].owner)
    }

    /// The owner of the topmost window containing `(x, y)`, if it is a task
    /// window (spec 0011, W29). A hit on an owner-`0` kernel window has no
    /// task target, so the event stays window-less and free-for-all.
    fn hit_test_owner(&self, x: i64, y: i64) -> Option<u64> {
        let owner = self.owner_of(self.hit_test(x, y)?)?;
        (owner != 0).then_some(owner)
    }

    /// The owner of the focused window, if it is a task window (W29).
    fn focus_owner(&self) -> Option<u64> {
        let owner = self.owner_of(self.focus)?;
        (owner != 0).then_some(owner)
    }

    /// The task that may dequeue `event` from the machine-wide queue
    /// (spec 0011, W29; ADR 0047; spec 0026, C12). `None` keeps the ordinary
    /// window-less competition.
    ///
    /// The rule is one value for routing and readiness:
    ///
    /// - an event that already names a window (`window != 0`) belongs to that
    ///   window's owner; a vanished window or a kernel window has no task
    ///   target;
    /// - otherwise a `PointerDown` belongs to the topmost plane at the point
    ///   (the menu bar and its pull-downs, C12), else to the topmost window's
    ///   owner;
    /// - every other pointer and key event belongs to the focused window's
    ///   owner;
    /// - a window-less `Expose`/`Close` is a malformed injection and has no
    ///   target (the real ones are pending flags served before the queue).
    fn input_target(&self, planes: &PlaneTable, event: &abi::Event) -> Option<u64> {
        if event.window != 0 {
            let owner = self.owner_of(event.window as u64)?;
            return (owner != 0).then_some(owner);
        }
        match event.kind {
            abi::EventKind::PointerDown => planes
                .owner_at(event.x, event.y)
                .or_else(|| self.hit_test_owner(event.x, event.y)),
            abi::EventKind::PointerMove
            | abi::EventKind::PointerUp
            | abi::EventKind::KeyDown
            | abi::EventKind::KeyUp => self.focus_owner(),
            abi::EventKind::Expose | abi::EventKind::Close => None,
        }
    }

    /// Recomposes and presents a window after its owner draws (spec 0026, C7;
    /// C14/C15; ADR 0060). The first present makes the canvas part of the
    /// composition; before it the pixels beneath are left untouched. The caller
    /// must own the window.
    fn present(&mut self, owner: u64, id: u64) -> Status {
        let Some(pos) = self.position(id) else {
            return Status::NotFound;
        };
        let slot = self.order[pos];
        if self.windows[slot].owner != owner {
            return Status::Invalid;
        }
        // A minimized window is hidden: a late present from its owner (a timer,
        // a stale expose handler) must not put its content back over the
        // desktop (W30).
        if self.windows[slot].minimized {
            return Status::Ok;
        }
        self.windows[slot].canvas_valid = true;
        let frame = self.windows[slot].frame();
        self.compose(frame);
        Status::Ok
    }

    /// The one compositor: paints `rect` from the surface stack and presents it
    /// (spec 0026, C14; ADR 0060). Bottom to top: the desktop pattern, every
    /// live non-minimized window's chrome and valid canvas, every plane that
    /// has been presented, then a present.
    fn compose(&self, rect: Rect) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        if !gfx::set_clip(rect.x, rect.y, rect.w, rect.h) {
            return;
        }
        let _ = gfx::fill_rect(rect.x, rect.y, rect.w, rect.h, DESKTOP_BASE);
        let _ = gfx::fill_dither(rect.x, rect.y, rect.w, rect.h, DESKTOP_DITHER);
        for pos in 0..self.count {
            let slot = self.order[pos];
            if self.windows[slot].minimized {
                continue;
            }
            self.draw_chrome(slot);
            if self.windows[slot].canvas_valid {
                if let Some(canvas) = self.windows[slot].canvas_at() {
                    gfx::composite_canvas(&canvas);
                }
            }
        }
        // Planes are above every window, in creation order (spec 0026, C11/C14).
        // A plane is painted only after its first present (C15).
        {
            let planes = PLANES.lock();
            for index in 0..planes.count {
                let plane = &planes.planes[index];
                if plane.canvas_valid {
                    if let Some(canvas) = plane.canvas_at() {
                        gfx::composite_canvas(&canvas);
                    }
                }
            }
        }
        gfx::reset_clip();
        let _ = gfx::present(rect.x, rect.y, rect.w, rect.h);
    }

    /// Runs one synchronous recomposition pass (spec 0026, C14/C16).
    ///
    /// Damage is still recorded as before (the bounded erase/chrome lists and
    /// the `full` flag); this ticket only changes what the repair does with it.
    /// Exposes are marked exactly as before (after the damage is recorded), the
    /// lists are coalesced, then every damaged rectangle is recomposed and
    /// presented. A `full` request recomposes the whole screen once. There is no
    /// visible-parts subtraction, per-piece present or snapshot.
    ///
    /// Runs under the table lock (window → screen), so the z-order it walks is
    /// the one the caller just produced.
    fn repair(&mut self) -> bool {
        if !self.full && self.erases.count == 0 && self.chrome.count == 0 {
            return false;
        }
        let (screen_w, screen_h) = gfx::size();
        if screen_w == 0 {
            self.clear_damage();
            return false;
        }
        let screen = Rect {
            x: 0,
            y: 0,
            w: i64::from(screen_w),
            h: i64::from(screen_h),
        };
        if self.full {
            // A switch or completed move recomposes the whole screen:
            // incidental damage is subsumed by it.
            self.erases.clear();
            self.chrome.clear();
            self.erases.add(screen, 0);
            self.chrome.add(screen);
        }
        let marked = self.mark_exposes();
        // Coalesce after expose marking so the exact original damage decides
        // which clients repaint; the compose pass then works on the merged set.
        self.erases.coalesce();
        self.chrome.coalesce();

        if self.full {
            self.compose(screen);
        } else {
            for index in 0..self.erases.count {
                let rect = self.erases.entries[index].rect;
                self.compose(rect);
            }
            for index in 0..self.chrome.count {
                let rect = self.chrome.rects[index];
                self.compose(rect);
            }
        }

        self.clear_damage();
        marked
    }
}

static TABLE: SpinLock<Table> = SpinLock::new(Table::new());

/// Opens a window owned by `owner`: frame origin `(x, y)`, content `(w, h)`.
///
/// `owner` is a global task id, or `0` for the kernel (task ids start at `1`;
/// spec 0012, D6). A disabled screen has no geometry, so `open` is `Invalid`
/// (W13). The screen size is read before the table lock is taken; the repair
/// pass (if the open succeeded) runs under it.
pub fn open(owner: u64, x: i64, y: i64, w: i64, h: i64, title: &[u8]) -> Result<u64, Status> {
    let (screen_w, screen_h) = gfx::size();
    let mut table = TABLE.lock();
    // Validate before allocating so an invalid rectangle is `Invalid`, not
    // `Full`; a failed canvas allocation then opens nothing (spec 0026, C2).
    table.can_open(screen_w as i64, screen_h as i64, w, h)?;
    let Some(canvas) = alloc_canvas(w, h) else {
        return Err(Status::Full);
    };
    let result = table.open(
        screen_w as i64,
        screen_h as i64,
        owner,
        x,
        y,
        w,
        h,
        title,
        false,
        Some(canvas),
    );
    // A new window becomes its owner's scope, so the caller's plane scope (if
    // any) is replaced (spec 0026, C3).
    if result.is_ok() {
        PLANES.lock().clear_drawer(owner);
    }
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

/// Opens a window at the **bottom** of the z-order. Kernel-internal: the
/// System Log is a screen-sized diagnostic window and must stay behind
/// application windows, including when a mode change reopens it.
pub(crate) fn open_bottom(
    owner: u64,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    title: &[u8],
) -> Result<u64, Status> {
    let (screen_w, screen_h) = gfx::size();
    let mut table = TABLE.lock();
    table.can_open(screen_w as i64, screen_h as i64, w, h)?;
    let Some(canvas) = alloc_canvas(w, h) else {
        return Err(Status::Full);
    };
    let result = table.open(
        screen_w as i64,
        screen_h as i64,
        owner,
        x,
        y,
        w,
        h,
        title,
        true,
        Some(canvas),
    );
    if result.is_ok() {
        PLANES.lock().clear_drawer(owner);
    }
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

/// Rebuilds the window stack after a display mode change (spec 0022; spec
/// 0026, C17).
///
/// Window content sizes are kept: only the frame origins are scaled with the
/// screen and clamped below the reserved work area. The service then runs the
/// full-stack repair, which restores the desktop, redraws every chrome and
/// marks every live window for an `Expose`, so each owner repaints at its
/// refreshed content rectangle. Plane canvases are unchanged; their origins
/// are clamped to the new screen. A stale modal grab or drag is dropped.
pub fn screen_changed(old_width: u32, old_height: u32) {
    let (new_width, new_height) = gfx::size();
    if new_width == 0 {
        return;
    }
    let new_w = i64::from(new_width);
    let new_h = i64::from(new_height);
    let old_w = i64::from(old_width.max(1));
    let old_h = i64::from(old_height.max(1));

    let mut table = TABLE.lock();
    let work_area = table.work_area.min(new_h);
    for slot in 0..MAX_WINDOWS {
        let window = &mut table.windows[slot];
        if !window.used {
            continue;
        }
        let frame_w = window.w.saturating_add(BORDER_WIDTH * 2);
        let frame_h = window
            .h
            .saturating_add(TITLE_BAR_HEIGHT + BORDER_WIDTH);
        let x = if old_width == 0 {
            window.x
        } else {
            window.x.saturating_mul(new_w) / old_w
        };
        let y = if old_height == 0 {
            window.y
        } else {
            window.y.saturating_mul(new_h) / old_h
        };
        let max_x = (new_w - frame_w).max(0);
        let max_y = (new_h - frame_h).max(0);
        window.x = x.clamp(0, max_x);
        window.y = y.clamp(work_area.min(max_y), max_y);
    }
    table.drag = None;
    table.grab = None;
    PLANES.lock().clamp_to(new_w, new_h);
    table.full = true;
    let wake = table.repair();
    drop(table);
    if wake {
        crate::task::wake_input();
    }
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
    let status = table.move_to(screen_w as i64, screen_h as i64, owner, id, x, y, true);
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

/// Service-owned pointer interaction hook (spec 0011, W22/W23/W25).
///
/// Called by `gfx` from both pointer paths **after** the graphics lock is
/// dropped, so the repair can take `WINDOW → SCREEN` without inverting the
/// order. Returns `true` when the press was consumed by the service (a
/// title-bar drag or a close box); `gfx` then removes the matching queued
/// `PointerDown` so the client never sees it. The table lock is never held
/// across `task::wake_input`: a posted close request wakes the owner after the
/// unlock.
pub fn pointer_event(
    old_x: i64,
    old_y: i64,
    old_buttons: u64,
    x: i64,
    y: i64,
    buttons: u64,
) -> bool {
    let (screen_w, screen_h) = gfx::size();
    if screen_w == 0 {
        return false;
    }
    const LEFT: u64 = 1;
    let pressed = buttons & LEFT != 0 && old_buttons & LEFT == 0;
    let released = buttons & LEFT == 0 && old_buttons & LEFT != 0;
    let held = buttons & LEFT != 0;

    let mut table = TABLE.lock();
    let mut consumed = false;
    let mut close_requested = false;
    let mut dirty = false;

    // While a modal popup grab is held the service performs no window
    // interaction at all (W26): no drag starts, no close box posts. The owner
    // decides inside/outside from its advisory rectangle; the click still
    // reaches it through the queue as an ordinary event.
    if table.grab_active() {
        drop(table);
        return false;
    }

    // A plane owns a *new* press inside its rectangle (spec 0026, C12): the
    // menu bar wins over a window moved under it, so no drag starts and no
    // close request posts. The press is not consumed; the plane's owner
    // receives it unchanged. Moves and releases are deliberately not filtered:
    // a drag that started before the pointer crossed a plane still follows it.
    if pressed && PLANES.lock().owner_at(x, y).is_none() {
        match table.press(x, y) {
            PressAction::Consumed => {
                consumed = true;
                dirty = true;
            }
            PressAction::Close => {
                consumed = true;
                close_requested = true;
            }
            PressAction::Minimize => {
                consumed = true;
                dirty = true;
            }
            PressAction::None => {}
        }
    }
    if released {
        // A drag that moved gets the full-stack repaint on release; a plain
        // title-bar click that never moved records no extra work.
        if table.end_drag() {
            dirty = true;
        }
    } else if held && table.drag.is_some() && (x != old_x || y != old_y) {
        let _ = table.drag_move(screen_w as i64, screen_h as i64, x, y);
        dirty = true;
    }

    let wake = if dirty { table.repair() } else { false };
    drop(table);
    if wake || close_requested {
        // The owner may be blocked in `next_event`; a close request counts as
        // input readiness, exactly like an expose.
        crate::task::wake_input();
    }
    consumed
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

/// One read-only window snapshot by z-order index, topmost first (spec 0011,
/// W19; spec 0012, D15). `None` for an index at or past the live count.
pub fn info(index: u64) -> Option<Info> {
    TABLE.lock().info(index)
}

/// The focused window id, or 0 (W6).
pub fn focus() -> u64 {
    TABLE.lock().focus
}

/// The task owner of window `id`, or 0 for an unknown or kernel window.
/// Kernel-internal: the console routes serial bytes to the focused window's
/// console session (spec 0021, S10).
pub fn owner_of(id: u64) -> u64 {
    TABLE.lock().owner_of(id).unwrap_or(0)
}

/// Whether window `id` is live and not minimized. Kernel-internal: the console
/// leaves a minimized System Log unpainted until it is restored.
pub fn is_visible(id: u64) -> bool {
    let table = TABLE.lock();
    let Some(pos) = table.position(id) else {
        return false;
    };
    !table.windows[table.order[pos]].minimized
}

/// Whether window `id` is minimized (`syslib.window_minimized`, W30).
pub fn is_minimized(id: u64) -> bool {
    let table = TABLE.lock();
    let Some(pos) = table.position(id) else {
        return false;
    };
    table.windows[table.order[pos]].minimized
}

/// Closes every window and plane owned by a terminating task and repairs the
/// screen once. Called from `Task::reclaim`, so it covers normal exit, kill,
/// fault, a failed spawn and a stale-slot eviction (W8; spec 0026, C1). A
/// terminating task's modal popup grab (W26) is released here too, and input
/// waiters are woken so queued events flow again.
pub fn close_task(owner: u64) {
    let mut table = TABLE.lock();
    let released_input = table.close_task(owner);
    let mut removed = [Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }; MAX_PLANES];
    let plane_count = PLANES.lock().close_owner(owner, &mut removed);
    for rect in removed[..plane_count].iter() {
        table.erases.add(*rect, 0);
    }
    let wake = table.repair();
    drop(table);
    if wake || released_input || plane_count > 0 {
        crate::task::wake_input();
    }
}

/// Takes or updates the modal popup input grab for `owner` (spec 0011, W26;
/// ADR 0044). `Full` for another owner, `Invalid` for a non-positive extent; a
/// repeat by the owner updates the advisory rectangle and is `Ok`.
pub fn popup_grab(owner: u64, x: i64, y: i64, w: i64, h: i64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    TABLE.lock().grab(owner, x, y, w, h)
}

/// Releases the caller's modal popup grab (W26). `Ok` for the owner (`NotFound`
/// when none is held, `Invalid` for another task); the release wakes every
/// input waiter after the table unlock so queued events are deliverable again.
pub fn popup_release(owner: u64) -> Status {
    let status = TABLE.lock().release_grab(owner);
    if status == Status::Ok {
        crate::task::wake_input();
    }
    status
}

/// Whether queued input events may be delivered to `owner` (W26): true without
/// a grab, and only for the grab owner with one. Every input-readiness probe
/// uses this one rule (`poll_event`, `input_ready`, the scheduler's post-block
/// re-check and `next_any` source `1`).
pub fn grab_allows(owner: u64) -> bool {
    TABLE.lock().grab_allows(owner)
}

/// Whether any task holds the modal popup grab (W26). The poll-time router
/// skips hit-testing and raising while a grab is active.
pub fn grab_active() -> bool {
    TABLE.lock().grab_active()
}

/// The task that may dequeue `event` from the machine-wide queue (spec 0011,
/// W29; ADR 0047; spec 0026, C12). `None` means the event has no window target
/// and keeps the ordinary window-less competition.
///
/// Kernel-internal: `KernelHost::poll_event` gates on it and
/// `task::input_queue_ready` uses it for readiness, so the block/wake rule and
/// the consume rule cannot drift. The window lock is taken after the queue
/// lock is released (the callers peek first), so the two never nest; the plane
/// lock follows the window lock.
pub fn input_target(event: &abi::Event) -> Option<u64> {
    let table = TABLE.lock();
    let planes = PLANES.lock();
    table.input_target(&planes, event)
}

/// The owner of the topmost plane containing `(x, y)`, if any (spec 0026,
/// C12). The poll-time router and the pointer service use the same geometry.
pub fn plane_owner_at(x: i64, y: i64) -> Option<u64> {
    PLANES.lock().owner_at(x, y)
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

/// Takes the topmost pending close request owned by `owner`, if any (spec 0011,
/// W25). Only the owner can take it; the window stays until the owner closes it.
pub fn take_close(owner: u64) -> Option<u64> {
    TABLE.lock().take_close(owner)
}

/// Whether `owner` has a pending close request. Every input-readiness probe and
/// block re-check includes it, so a close genuinely wakes a blocked owner.
pub fn has_close(owner: u64) -> bool {
    TABLE.lock().has_close(owner)
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

/// Recomposes and presents a window after its owner draws (spec 0026, C7/C15;
/// ADR 0060). `Invalid` when graphics are disabled, `NotFound` for an unknown
/// id, and `Invalid` for a window owned by another task.
pub fn present(owner: u64, id: u64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    let mut table = TABLE.lock();
    table.present(owner, id)
}

/// Opens the caller's draw scope on one of its windows (`syslib.window_begin`,
/// spec 0026, C3). `id == 0` clears the caller's scope; there is no screen
/// surface to draw into (C20). `Invalid` when graphics are disabled,
/// `NotFound` for an unknown id, and `Invalid` for a window owned by another
/// task.
pub fn begin(owner: u64, id: u64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    let mut table = TABLE.lock();
    let status = table.begin(owner, id);
    if status == Status::Ok {
        PLANES.lock().clear_drawer(owner);
    }
    status
}

/// Creates a plane owned by `owner` (spec 0026, C11): a screen rectangle above
/// every window with a zeroed canvas, clamped to the screen. The limit is
/// checked before a canvas is allocated, so a full table never allocates.
pub fn plane_create(owner: u64, x: i64, y: i64, w: i64, h: i64) -> Result<u64, Status> {
    let (screen_w, screen_h) = gfx::size();
    if screen_w == 0 || screen_h == 0 {
        return Err(Status::Invalid);
    }
    let Some(rect) = clamp_plane(
        i64::from(screen_w),
        i64::from(screen_h),
        x,
        y,
        w,
        h,
    ) else {
        return Err(Status::Invalid);
    };
    let table = TABLE.lock();
    let mut planes = PLANES.lock();
    if planes.count == MAX_PLANES {
        return Err(Status::Full);
    }
    let Some(canvas) = alloc_canvas(rect.w, rect.h) else {
        return Err(Status::Full);
    };
    let id = planes.create(owner, rect, Some(canvas))?;
    drop(planes);
    drop(table);
    Ok(id)
}

/// Recomposes and presents a screen rectangle on request (`syslib.present`,
/// spec 0026, C7). The service repaints the rectangle from the surface stack
/// and presents it, so the request needs no scope and is idempotent.
pub fn recompose(x: i64, y: i64, w: i64, h: i64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    if w <= 0 || h <= 0 {
        return Status::Invalid;
    }
    TABLE.lock().compose(Rect { x, y, w, h });
    Status::Ok
}

/// Destroys the caller's plane and recomposites the vacated rectangle so the
/// windows underneath reappear (spec 0026, C11). Owner-only.
pub fn plane_destroy(owner: u64, id: u64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    let mut table = TABLE.lock();
    let rect = {
        let mut planes = PLANES.lock();
        planes.destroy(owner, id)
    };
    let rect = match rect {
        Ok(rect) => rect,
        Err(status) => return status,
    };
    // The vacated pixels are recomposed from the retained surfaces below; no
    // client round trip and no erase list (C16).
    table.erases.add(rect, 0);
    table.repair();
    Status::Ok
}

/// Opens the caller's draw scope on one of its planes (`syslib.plane_begin`,
/// spec 0026, C3/C11). Owner-only; replaces the caller's previous window or
/// plane scope.
pub fn plane_begin(owner: u64, id: u64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    let mut table = TABLE.lock();
    let mut planes = PLANES.lock();
    let status = planes.begin(owner, id);
    if status == Status::Ok {
        table.clear_drawer(owner);
    }
    status
}

/// Closes the caller's plane scope and recomposites the plane's rectangle
/// (`syslib.plane_present`, spec 0026, C11/C14). The first present makes the
/// plane visible; owner-only.
pub fn plane_present(owner: u64, id: u64) -> Status {
    if gfx::size().0 == 0 {
        return Status::Invalid;
    }
    let table = TABLE.lock();
    let rect = {
        let mut planes = PLANES.lock();
        planes.present(owner, id)
    };
    let rect = match rect {
        Ok(rect) => rect,
        Err(status) => return status,
    };
    table.compose(rect);
    Status::Ok
}

/// Reserves a machine-wide top strip into which window frames are not placed
/// (`syslib.work_area_reserve`, spec 0026, C13). `0` clears it; a negative
/// height or disabled graphics is `Invalid`. Last caller wins.
pub fn work_area_reserve(height: i64) -> Status {
    let (screen_w, screen_h) = gfx::size();
    if screen_w == 0 || screen_h == 0 {
        return Status::Invalid;
    }
    if height < 0 {
        return Status::Invalid;
    }
    TABLE.lock().work_area = height.min(i64::from(screen_h));
    Status::Ok
}

/// Runs `f` with the canvas of the surface `task` has a draw scope on, while
/// the window table lock is held (spec 0026, C3/C4; ADR 0060). A window scope
/// wins; otherwise the caller's plane scope is used. `None` when `task` has no
/// scope; `task == 0` (the kernel) never has one. Holding `WINDOW` across the
/// closure is what keeps a concurrent close from freeing the canvas mid-write.
pub(crate) fn with_canvas<T>(task: u64, f: impl FnOnce(&gfx::Canvas) -> T) -> Option<T> {
    if task == 0 {
        return None;
    }
    let table = TABLE.lock();
    for pos in 0..table.count {
        let slot = table.order[pos];
        if table.windows[slot].drawer == task {
            if let Some(canvas) = table.windows[slot].canvas_at() {
                return Some(f(&canvas));
            }
        }
    }
    let planes = PLANES.lock();
    for index in 0..planes.count {
        if planes.planes[index].drawer == task {
            if let Some(canvas) = planes.planes[index].canvas_at() {
                return Some(f(&canvas));
            }
        }
    }
    None
}

/// Paints into the canvas of `id` under the table lock without composing
/// (spec 0026, C4). The kernel console uses it for one cell; `None` when the
/// window is missing or has no canvas.
pub(crate) fn draw_on_window<T>(id: u64, f: impl FnOnce(&gfx::Canvas) -> T) -> Option<T> {
    let table = TABLE.lock();
    let pos = table.position(id)?;
    let canvas = table.windows[table.order[pos]].canvas_at()?;
    Some(f(&canvas))
}

/// Paints into the canvas of `id` under the table lock, marks the canvas valid
/// and recomposes the window frame (spec 0026, C14; ADR 0060). The kernel
/// console uses it for one flushed run and for a whole-grid repaint.
pub(crate) fn paint_window<T>(id: u64, f: impl FnOnce(&gfx::Canvas) -> T) -> Option<T> {
    let mut table = TABLE.lock();
    let pos = table.position(id)?;
    let slot = table.order[pos];
    let canvas = table.windows[slot].canvas_at()?;
    let value = f(&canvas);
    let frame = {
        let window = &mut table.windows[slot];
        window.canvas_valid = true;
        window.frame()
    };
    table.compose(frame);
    Some(value)
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
    let one = table.open(SCREEN_W, SCREEN_H, 7, 10, 20, 100, 50, b"one", false, None);
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
    let two = table.open(SCREEN_W, SCREEN_H, 8, 300, 200, 160, 90, b"two", false, None);
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
        || table.move_to(SCREEN_W, SCREEN_H, 8, 1, 0, 0, false) != Status::Invalid
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
    if table.move_to(SCREEN_W, SCREEN_H, 7, 1, 5, 6, false) != Status::Ok || table.focus != 1 {
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
    let clamped = table.open(SCREEN_W, SCREEN_H, 7, -50, -50, 100, 50, b"c", false, None);
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
    if table.open(SCREEN_W, SCREEN_H, 7, 0, 0, 7, 50, b"", false, None) != Err(Status::Invalid)
        || table.open(SCREEN_W, SCREEN_H, 7, 0, 0, 100, 752, b"", false, None) != Err(Status::Invalid)
    {
        return Err(17);
    }

    // 9. Unknown ids, including 0, are NotFound; a disabled screen is Invalid.
    if table.close(7, 0) != Status::NotFound
        || table.raise(99) != Status::NotFound
        || table.frame_rect(99) != Err(Status::NotFound)
        || table.open(0, 0, 7, 0, 0, 100, 50, b"", false, None) != Err(Status::Invalid)
    {
        return Err(18);
    }

    // 10. Task termination closes every window the task owns.
    if table.open(SCREEN_W, SCREEN_H, 9, 0, 0, 100, 50, b"a", false, None) != Ok(4)
        || table.open(SCREEN_W, SCREEN_H, 9, 200, 100, 100, 50, b"b", false, None) != Ok(5)
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
    let truncated = table.open(SCREEN_W, SCREEN_H, 7, 0, 0, 100, 50, &long, false, None);
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
        if table.open(SCREEN_W, SCREEN_H, 12, 0, 0, 100, 50, b"f", false, None) != Ok(7 + index as u64) {
            return Err(24);
        }
        index += 1;
    }
    if table.open(SCREEN_W, SCREEN_H, 12, 0, 0, 100, 50, b"full", false, None) != Err(Status::Full) {
        return Err(25);
    }
    table.close_task(12);
    if table.count != 0 || table.focus != 0 {
        return Err(26);
    }

    // 13. Owner 0 is the kernel (spec 0012, D6): it opens, focuses and is
    //     enumerated topmost-first like any other window, and its own close
    //     works. The previous steps leave `next_id` at 23.
    let kernel = table.open(SCREEN_W, SCREEN_H, 0, 10, 10, 100, 50, b"kernel", false, None);
    if kernel != Ok(23) || table.focus != 23 {
        return Err(27);
    }
    match table.info(0) {
        Some(info) if info.id == 23 && info.title_len == 6 && info.title[..6] == *b"kernel" => {}
        _ => return Err(28),
    }
    if table.info(1).is_some() {
        return Err(29);
    }
    if table.close(0, 23) != Status::Ok || table.focus != 0 {
        return Err(30);
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
        .open(SCREEN_W, SCREEN_H, 5, 100, 100, 80, 60, b"vis", false, None)
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
        .open(SCREEN_W, SCREEN_H, 1, 100, 100, 200, 120, b"Alpha", false, None)
        .map_err(|_| 11u32)?;
    let fa = table.frame_rect(a).map_err(|_| 12u32)?;
    if !table.chrome.covers(fa) || !table.erases.covers(fa, table.count) {
        return Err(13);
    }

    table.clear_damage();
    let b = table
        .open(SCREEN_W, SCREEN_H, 1, 200, 160, 200, 120, b"Beta", false, None)
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

    // Moving the top window records the old frame for recomposition and both
    // frames for the chrome. The damage lists are unchanged by the M16.4
    // cutover; only `relocate`'s pixel copy is gone (the canvas is window-local).
    table.clear_damage();
    let old = table.frame_rect(a).map_err(|_| 21u32)?;
    if table.move_to(SCREEN_W, SCREEN_H, 1, a, 300, 220, false) != Status::Ok {
        return Err(22);
    }
    let new = table.frame_rect(a).map_err(|_| 23u32)?;
    if !table.erases.covers(old, 0)
        || table.erases.covers(new, 0)
        || table.erases.covers(new, table.count)
    {
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
        || table.move_to(SCREEN_W, SCREEN_H, 2, b, 0, 0, false) != Status::Invalid
        || table.erases.count != 0
        || table.chrome.count != 0
    {
        return Err(29);
    }

    table.close_task(1);
    if table.count != 0 {
        return Err(30);
    }

    // 5. Full-stack requests (W31). A completed programmatic move and a drag
    //    release that actually moved set the flag; a raise of the already
    //    topmost, focused window and an in-progress drag step do not.
    let mut table = Table::new();
    let a = table
        .open(SCREEN_W, SCREEN_H, 1, 100, 100, 200, 100, b"full", false, None)
        .map_err(|_| 31u32)?;
    table.clear_damage();
    if table.full {
        return Err(32);
    }
    if table.raise(a) != Status::Ok || table.full {
        return Err(33);
    }
    if table.move_to(SCREEN_W, SCREEN_H, 1, a, 300, 300, false) != Status::Ok || !table.full {
        return Err(34);
    }
    table.clear_damage();
    let frame = table.frame_rect(a).map_err(|_| 35u32)?;
    match table.press(frame.x + 50, frame.y + 8) {
        PressAction::Consumed => {}
        _ => return Err(36),
    }
    if table.full || table.drag.is_none() {
        return Err(37);
    }
    if table.drag_move(SCREEN_W, SCREEN_H, 500, 400) != Status::Ok || table.full {
        return Err(38);
    }
    if !table.end_drag() || !table.full {
        return Err(39);
    }
    table.close_task(1);
    if table.count != 0 {
        return Err(40);
    }

    Ok(())
}

/// The damage-coalescing measurement from [`damage_self_check`]: present
/// rectangles and pixels before (one bounding box, the pre-M2.6.2 behaviour)
/// and after (coalesced rectangles, the current repair).
#[derive(Clone, Copy)]
pub struct DamageStats {
    pub before_rects: usize,
    pub after_rects: usize,
    pub before_px: i64,
    pub after_px: i64,
}

/// Boot self-check for damage coalescing and present batching (spec 0011,
/// W11): the generic merge (contained dropped, touching merged, disjoint kept),
/// the erase list's coverage rule, and a realistic window move measured before
/// and after coalescing. Pure: it works on scratch lists and a scratch table
/// and never touches the screen.
///
/// Returns the measurement, or the first failing step; `kmain` prints
/// `window: damage ok (present ...)` and halts on a mismatch.
pub fn damage_self_check() -> Result<DamageStats, u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;

    // 1. Generic coalescing: a contained rectangle disappears, a touching
    //    chain becomes one piece and a disjoint piece stays separate.
    let mut rects = [Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }; 8];
    rects[0] = Rect {
        x: 0,
        y: 0,
        w: 10,
        h: 10,
    };
    rects[1] = Rect {
        x: 2,
        y: 2,
        w: 4,
        h: 4,
    };
    rects[2] = Rect {
        x: 10,
        y: 0,
        w: 10,
        h: 10,
    };
    rects[3] = Rect {
        x: 40,
        y: 40,
        w: 5,
        h: 5,
    };
    rects[4] = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };
    let count = coalesce(&mut rects, 5);
    let expected = Rect {
        x: 0,
        y: 0,
        w: 20,
        h: 10,
    };
    let disjoint = Rect {
        x: 40,
        y: 40,
        w: 5,
        h: 5,
    };
    if count != 2 || !rects[..count].contains(&expected) || !rects[..count].contains(&disjoint) {
        return Err(1);
    }

    // 2. Erase coalescing respects the coverage limit. A contained request
    //    with a lower limit is redundant (the container paints a superset).
    let mut erases = EraseList::new();
    erases.add(
        Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        },
        2,
    );
    erases.add(
        Rect {
            x: 2,
            y: 2,
            w: 4,
            h: 4,
        },
        0,
    );
    erases.coalesce();
    if erases.count != 1 || erases.entries[0].covers_from != 2 {
        return Err(2);
    }

    // 3. The same containment with a higher limit is real work and survives.
    let mut erases = EraseList::new();
    erases.add(
        Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        },
        0,
    );
    erases.add(
        Rect {
            x: 2,
            y: 2,
            w: 4,
            h: 4,
        },
        2,
    );
    erases.coalesce();
    if erases.count != 2 {
        return Err(3);
    }

    // 4. Equal-coverage touching requests merge; different limits do not.
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
    erases.add(
        Rect {
            x: 20,
            y: 0,
            w: 10,
            h: 10,
        },
        2,
    );
    erases.coalesce();
    if erases.count != 2 {
        return Err(4);
    }
    if !erases.covers(
        Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 10,
        },
        1,
    ) {
        return Err(5);
    }

    // 5. Realistic repair: two windows, then a diagonal move of the lower one.
    //    The old repair presented one bounding box covering both the old and
    //    new positions; the coalesced present covers only the two frames.
    let mut table = Table::new();
    let alpha = table
        .open(SCREEN_W, SCREEN_H, 1, 100, 100, 200, 120, b"Alpha", false, None)
        .map_err(|_| 6u32)?;
    let _beta = table
        .open(SCREEN_W, SCREEN_H, 2, 600, 400, 200, 120, b"Beta", false, None)
        .map_err(|_| 7u32)?;
    table.clear_damage();
    if table.move_to(SCREEN_W, SCREEN_H, 1, alpha, 700, 600, false) != Status::Ok {
        return Err(8);
    }

    let mut originals = [Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }; MAX_PRESENT];
    let mut original_count = 0;
    let mut presentation = Presentation::new();
    for entry in table.erases.entries[..table.erases.count].iter() {
        presentation.add(entry.rect);
        originals[original_count] = entry.rect;
        original_count += 1;
    }
    for rect in table.chrome.rects[..table.chrome.count].iter() {
        presentation.add(*rect);
        originals[original_count] = *rect;
        original_count += 1;
    }
    if original_count == 0 || original_count > MAX_PRESENT {
        return Err(9);
    }
    let before_rects = original_count;
    let before_px = presentation.bounds_area();
    let after_rects = presentation.coalesce();
    let after_px = presentation.area();
    if after_rects == 0 || after_rects > before_rects {
        return Err(10);
    }
    if after_px <= 0 || after_px >= before_px {
        return Err(11);
    }
    // Every damaged rectangle is still covered by a coalesced present
    // rectangle: coalescing may grow the area but may never lose a pixel.
    for original in originals[..original_count].iter() {
        if !presentation.rects[..after_rects]
            .iter()
            .any(|rect| contains(*rect, *original))
        {
            return Err(12);
        }
    }

    table.close_task(1);
    table.close_task(2);

    Ok(DamageStats {
        before_rects,
        after_rects,
        before_px,
        after_px,
    })
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
        .open(SCREEN_W, SCREEN_H, 1, 10, 10, 100, 50, b"A", false, None)
        .map_err(|_| 1u32)?;
    let b = table
        .open(SCREEN_W, SCREEN_H, 2, 400, 400, 100, 50, b"B", false, None)
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
    // Enumeration is topmost first (spec 0011, W19): A was raised above B.
    match (table.info(0), table.info(1), table.info(2)) {
        (Some(top), Some(bottom), None) if top.id == a && bottom.id == b => {}
        _ => return Err(26),
    }
    if table.hit_test(900, 700).is_some() {
        return Err(13);
    }
    if table.move_to(SCREEN_W, SCREEN_H, 2, b, 20, 20, false) != Status::Ok {
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
        .open(SCREEN_W, SCREEN_H, 3, 600, 100, 100, 50, b"C", false, None)
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

/// Boot self-check for the service-owned pointer interactions (spec 0011,
/// W22/W23): title-bar vs close-box vs content classification, the grab
/// offset, clamping at all four edges, release, the ignored second press, the
/// owner-0 close-box exemption and drag cancellation when the window closes.
/// Pure: it works on a scratch table and never touches the screen or the task
/// table.
///
/// Returns the first failing step, or `Ok(())`; `kmain` prints
/// `window: drag ok` and halts on a mismatch.
pub fn drag_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;

    let mut table = Table::new();
    let id = table
        .open(SCREEN_W, SCREEN_H, 7, 100, 100, 200, 100, b"drag", false, None)
        .map_err(|_| 1u32)?;
    let frame = table.frame_rect(id).map_err(|_| 2u32)?;

    // 1. A title-bar press (outside the close box) is consumed, raises and
    //    focuses the window, and records the grab offset from the frame origin.
    let px = frame.x + 50;
    let py = frame.y + 8;
    match table.press(px, py) {
        PressAction::Consumed => {}
        _ => return Err(3),
    }
    let Some(drag) = table.drag else {
        return Err(4);
    };
    if drag.id != id || drag.offset_x != px - frame.x || drag.offset_y != py - frame.y {
        return Err(5);
    }
    if table.focus != id {
        return Err(6);
    }

    // 2. A second press while the drag is active is ignored (consumed, so it
    //    cannot reach a client): the drag is unchanged and no new grab replaces
    //    it.
    let other = table
        .open(SCREEN_W, SCREEN_H, 8, 600, 500, 100, 50, b"other", false, None)
        .map_err(|_| 7u32)?;
    let other_frame = table.frame_rect(other).map_err(|_| 8u32)?;
    match table.press(other_frame.x + 40, other_frame.y + 8) {
        PressAction::Consumed => {}
        _ => return Err(9),
    }
    match table.drag {
        Some(current) if current.id == id && current.offset_x == drag.offset_x => {}
        _ => return Err(10),
    }

    // 3. The drag follows the pointer minus the grab offset, with the same
    //    damage a `window_move` records.
    table.clear_damage();
    if table.drag_move(SCREEN_W, SCREEN_H, 400, 300) != Status::Ok {
        return Err(11);
    }
    let moved = Rect {
        x: 400 - drag.offset_x,
        y: 300 - drag.offset_y,
        w: frame.w,
        h: frame.h,
    };
    if table.frame_rect(id) != Ok(moved) {
        return Err(12);
    }
    if !table.erases.covers(frame, 0) || !table.chrome.covers(moved) || !table.chrome.covers(frame)
    {
        return Err(13);
    }

    // 4. All four edges clamp like `move_to`; the frame never leaves the screen.
    if table.drag_move(SCREEN_W, SCREEN_H, -500, -500) != Status::Ok
        || table.frame_rect(id)
            != Ok(Rect {
                x: 0,
                y: 0,
                w: frame.w,
                h: frame.h,
            })
    {
        return Err(14);
    }
    if table.drag_move(SCREEN_W, SCREEN_H, 5000, 5000) != Status::Ok
        || table.frame_rect(id)
            != Ok(Rect {
                x: SCREEN_W - frame.w,
                y: SCREEN_H - frame.h,
                w: frame.w,
                h: frame.h,
            })
    {
        return Err(15);
    }

    // 5. Release ends the drag; further motion does not move the window.
    let parked = table.frame_rect(id).map_err(|_| 16u32)?;
    table.end_drag();
    if table.drag.is_some() {
        return Err(17);
    }
    table.clear_damage();
    if table.drag_move(SCREEN_W, SCREEN_H, 200, 200) != Status::Ok {
        return Err(18);
    }
    if table.frame_rect(id) != Ok(parked) || table.erases.count != 0 || table.chrome.count != 0 {
        return Err(19);
    }

    // 6. A content press is not consumed and starts no drag.
    match table.press(parked.x + 40, parked.y + 40) {
        PressAction::None => {}
        _ => return Err(20),
    }
    if table.drag.is_some() {
        return Err(21);
    }

    // 7. The close box posts an owner request: owner 0 is consumed but inert,
    //    a task window yields `Close` and a pending flag the owner can take.
    let kernel = table
        .open(SCREEN_W, SCREEN_H, 0, 300, 100, 100, 50, b"kernel", false, None)
        .map_err(|_| 22u32)?;
    let kernel_frame = table.frame_rect(kernel).map_err(|_| 23u32)?;
    match table.press(kernel_frame.x + 5, kernel_frame.y + 5) {
        PressAction::Consumed => {}
        _ => return Err(24),
    }
    if table.drag.is_some() || table.has_close(0) {
        return Err(25);
    }
    let victim = table
        .open(SCREEN_W, SCREEN_H, 9, 500, 100, 100, 50, b"victim", false, None)
        .map_err(|_| 26u32)?;
    let victim_frame = table.frame_rect(victim).map_err(|_| 27u32)?;
    match table.press(victim_frame.x + 5, victim_frame.y + 5) {
        PressAction::Close => {}
        _ => return Err(28),
    }
    if table.drag.is_some() || !table.has_close(9) || table.take_close(9) != Some(victim) {
        return Err(29);
    }

    // 8. Closing the dragged window cancels the drag.
    let doomed = table
        .open(SCREEN_W, SCREEN_H, 7, 600, 300, 100, 50, b"doomed", false, None)
        .map_err(|_| 30u32)?;
    let doomed_frame = table.frame_rect(doomed).map_err(|_| 31u32)?;
    match table.press(doomed_frame.x + 40, doomed_frame.y + 8) {
        PressAction::Consumed => {}
        _ => return Err(32),
    }
    if table.drag.is_none() {
        return Err(33);
    }
    if table.close(7, doomed) != Status::Ok || table.drag.is_some() {
        return Err(34);
    }

    Ok(())
}

/// Boot self-check for the close request (spec 0011, W23/W25): a close-box
/// press sets a pending flag and is consumed, `take_close` returns only the
/// owner's topmost window and clears it, another owner sees nothing, owner-`0`
/// close boxes stay inert, and closing the window (or its owner's task) drops
/// the flag. Pure: it works on a scratch table and never touches the screen or
/// the task table.
///
/// Returns the first failing step, or `Ok(())`; `kmain` prints
/// `window: close ok` and halts on a mismatch.
pub fn close_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;

    let mut table = Table::new();
    let a = table
        .open(SCREEN_W, SCREEN_H, 7, 100, 100, 200, 100, b"A", false, None)
        .map_err(|_| 1u32)?;
    let b = table
        .open(SCREEN_W, SCREEN_H, 7, 400, 100, 200, 100, b"B", false, None)
        .map_err(|_| 2u32)?;
    let other = table
        .open(SCREEN_W, SCREEN_H, 8, 700, 100, 100, 50, b"other", false, None)
        .map_err(|_| 3u32)?;

    // 1. A close-box press on A posts a request for its owner and is consumed;
    //    it does not raise or focus the window.
    let a_frame = table.frame_rect(a).map_err(|_| 4u32)?;
    let focus = table.focus;
    match table.press(a_frame.x + 5, a_frame.y + 5) {
        PressAction::Close => {}
        _ => return Err(5),
    }
    if table.focus != focus || table.drag.is_some() {
        return Err(6);
    }
    if !table.has_close(7) {
        return Err(7);
    }
    // The owner takes the topmost pending request; taking clears it.
    if table.take_close(7) != Some(a) || table.has_close(7) {
        return Err(8);
    }

    // 2. Another owner sees nothing: A's request is not visible to owner 8.
    let other_frame = table.frame_rect(other).map_err(|_| 9u32)?;
    match table.press(other_frame.x + 5, other_frame.y + 5) {
        PressAction::Close => {}
        _ => return Err(10),
    }
    if table.take_close(7).is_some() || !table.has_close(8) {
        return Err(11);
    }
    if table.take_close(8) != Some(other) || table.has_close(8) {
        return Err(12);
    }

    // 3. Two pending requests: the topmost window's request comes first.
    let b_frame = table.frame_rect(b).map_err(|_| 13u32)?;
    for frame in [a_frame, b_frame] {
        match table.press(frame.x + 5, frame.y + 5) {
            PressAction::Close => {}
            _ => return Err(14),
        }
    }
    if table.take_close(7) != Some(b) || table.take_close(7) != Some(a) {
        return Err(15);
    }
    if table.has_close(7) {
        return Err(16);
    }

    // 4. Closing the window drops a pending request with the record.
    match table.press(a_frame.x + 5, a_frame.y + 5) {
        PressAction::Close => {}
        _ => return Err(17),
    }
    if table.close(7, a) != Status::Ok || table.has_close(7) {
        return Err(18);
    }
    if table.take_close(7).is_some() {
        return Err(19);
    }

    // 5. An owner-0 close box is consumed but posts nothing.
    let kernel = table
        .open(SCREEN_W, SCREEN_H, 0, 300, 400, 100, 50, b"kernel", false, None)
        .map_err(|_| 20u32)?;
    let kernel_frame = table.frame_rect(kernel).map_err(|_| 21u32)?;
    match table.press(kernel_frame.x + 5, kernel_frame.y + 5) {
        PressAction::Consumed => {}
        _ => return Err(22),
    }
    if table.has_close(0) {
        return Err(23);
    }

    // 6. Task termination closes its windows and drops their requests.
    match table.press(b_frame.x + 5, b_frame.y + 5) {
        PressAction::Close => {}
        _ => return Err(24),
    }
    if !table.has_close(7) {
        return Err(25);
    }
    table.close_task(7);
    if table.has_close(7) {
        return Err(26);
    }

    Ok(())
}

/// Boot self-check: the modal popup grab rules on a scratch table (spec 0011,
/// W26; ADR 0044). Pure: it neither allocates nor touches the global table or
/// the screen, so it cannot perturb a later application.
pub fn grab_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;
    let mut table = Table::new();
    let id = table
        .open(SCREEN_W, SCREEN_H, 7, 100, 100, 200, 100, b"menu", false, None)
        .map_err(|_| 1u32)?;
    if table.frame_rect(id).is_err() {
        return Err(2);
    }

    // 0. No grab: everyone may read input and a second release finds nothing.
    if !table.grab_allows(7) || !table.grab_allows(8) || table.grab_active() {
        return Err(3);
    }
    if table.release_grab(7) != Status::NotFound {
        return Err(4);
    }

    // 1. Task 7 takes the grab; only it may receive queued input.
    let rect = Rect {
        x: 50,
        y: 22,
        w: 176,
        h: 98,
    };
    if table.grab(7, rect.x, rect.y, rect.w, rect.h) != Status::Ok {
        return Err(5);
    }
    if !table.grab_active() || !table.grab_allows(7) || table.grab_allows(8) {
        return Err(6);
    }
    let Some(grab) = table.grab else {
        return Err(7);
    };
    if grab.owner != 7 || grab.rect.x != rect.x || grab.rect.y != rect.y {
        return Err(8);
    }

    // 2. Another task is `Full` and changes nothing.
    if table.grab(8, 0, 0, 10, 10) != Status::Full {
        return Err(9);
    }
    if !table.grab_allows(7) || table.grab_allows(8) {
        return Err(10);
    }

    // 3. The owner's repeat updates the rectangle and stays `Ok`.
    if table.grab(7, 60, 30, 120, 60) != Status::Ok {
        return Err(11);
    }
    match table.grab {
        Some(grab) if grab.owner == 7 && grab.rect.x == 60 && grab.rect.w == 120 => {}
        _ => return Err(12),
    }

    // 4. A non-positive extent is `Invalid` and leaves the grab unchanged.
    if table.grab(7, 60, 30, 0, 60) != Status::Invalid
        || table.grab(7, 60, 30, 120, -1) != Status::Invalid
    {
        return Err(13);
    }
    match table.grab {
        Some(grab) if grab.rect.h == 60 => {}
        _ => return Err(14),
    }

    // 5. Only the owner releases; another task is `Invalid` and the grab stays.
    if table.release_grab(8) != Status::Invalid || !table.grab_active() {
        return Err(15);
    }
    if table.release_grab(7) != Status::Ok || table.grab_active() {
        return Err(16);
    }
    if !table.grab_allows(8) {
        return Err(17);
    }
    if table.release_grab(7) != Status::NotFound {
        return Err(18);
    }

    // 6. Task termination releases its grab and reports it, so its caller can
    //    wake the waiters. Another task's grab is untouched.
    if table.grab(7, 0, 0, 10, 10) != Status::Ok {
        return Err(19);
    }
    if !table.close_task(7) || table.grab_active() {
        return Err(20);
    }
    if table.grab(8, 0, 0, 10, 10) != Status::Ok {
        return Err(21);
    }
    if table.close_task(7) || !table.grab_allows(8) {
        return Err(22);
    }

    Ok(())
}

/// Boot self-check: the plane stack and the work area on scratch state (spec
/// 0026, C11–C13). Proves the 8-plane limit, owner-only begin/present/destroy,
/// clamped geometry, that a plane is not composited before its first present,
/// and that destroying a plane yields exactly one damage rectangle (a second
/// destroy is `NotFound` and records nothing). Pure: no allocation, no global
/// table, no screen.
pub fn planes_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;

    let mut planes = PlaneTable::new();

    // 1. The stack is creation-ordered, ids start at 1 and a rectangle partly
    //    off screen is clamped to it.
    let bar = planes
        .create(
            7,
            clamp_plane(SCREEN_W, SCREEN_H, 0, 0, SCREEN_W, 22).ok_or(1u32)?,
            None,
        )
        .map_err(|_| 1u32)?;
    if bar != 1 || planes.count != 1 || planes.planes[0].rect().h != 22 {
        return Err(1);
    }
    let menu = planes
        .create(
            7,
            clamp_plane(SCREEN_W, SCREEN_H, -10, 100, 50, 40).ok_or(2u32)?,
            None,
        )
        .map_err(|_| 2u32)?;
    if menu != 2
        || planes.planes[1].rect()
            != (Rect {
                x: 0,
                y: 100,
                w: 40,
                h: 40,
            })
    {
        return Err(2);
    }

    // 2. A non-positive or entirely off-screen rectangle is refused.
    if clamp_plane(SCREEN_W, SCREEN_H, 0, 0, 0, 10).is_some()
        || clamp_plane(SCREEN_W, SCREEN_H, 0, 0, 10, -1).is_some()
        || clamp_plane(SCREEN_W, SCREEN_H, SCREEN_W, 0, 10, 10).is_some()
        || clamp_plane(SCREEN_W, SCREEN_H, 0, SCREEN_H + 4, 10, 10).is_some()
    {
        return Err(3);
    }

    // 3. A plane is not composited before its first present (C15); the first
    //    present marks it and a second does not duplicate it.
    if planes.composited() != 0 {
        return Err(4);
    }
    let rect = planes.present(7, bar).map_err(|_| 5u32)?;
    if rect.h != 22 || planes.composited() != 1 {
        return Err(5);
    }
    planes.present(7, bar).map_err(|_| 6u32)?;
    if planes.composited() != 1 {
        return Err(6);
    }

    // 4. The topmost plane at a point owns a press there (C12).
    if planes.owner_at(10, 10) != Some(7) {
        return Err(7);
    }
    if planes.owner_at(0, 99).is_some() {
        return Err(8);
    }
    if planes.owner_at(5, 100) != Some(7) {
        return Err(9);
    }
    if planes.owner_at(900, 700).is_some() {
        return Err(10);
    }

    // 5. Owner checks: another task cannot begin, present or destroy; an
    //    unknown id is `NotFound` and changes nothing.
    if planes.begin(8, bar) != Status::Invalid
        || planes.present(8, bar) != Err(Status::Invalid)
        || planes.destroy(8, bar) != Err(Status::Invalid)
        || planes.begin(7, 99) != Status::NotFound
        || planes.present(7, 99) != Err(Status::NotFound)
        || planes.destroy(7, 99) != Err(Status::NotFound)
    {
        return Err(11);
    }

    // 6. The scope is one per task: beginning a second plane clears the first.
    if planes.begin(7, bar) != Status::Ok
        || planes.planes[0].drawer != 7
        || planes.begin(7, menu) != Status::Ok
        || planes.planes[0].drawer != 0
        || planes.planes[1].drawer != 7
    {
        return Err(12);
    }
    planes.clear_drawer(7);
    if planes.planes[1].drawer != 0 {
        return Err(13);
    }

    // 7. Destroy is owner-only, frees the slot and reports the vacated
    //    rectangle exactly once: a second destroy is `NotFound` and there is no
    //    damage to record.
    let vacated = planes.destroy(7, bar).map_err(|_| 14u32)?;
    if vacated
        != (Rect {
            x: 0,
            y: 0,
            w: SCREEN_W,
            h: 22,
        })
        || planes.count != 1
        || planes.destroy(7, bar) != Err(Status::NotFound)
    {
        return Err(14);
    }

    // 8. The limit is 8 and frees on destroy.
    while planes.count < MAX_PLANES {
        planes
            .create(
                7,
                Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                },
                None,
            )
            .map_err(|_| 15u32)?;
    }
    if planes.count != MAX_PLANES {
        return Err(15);
    }
    if planes
        .create(
            7,
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            None,
        )
        != Err(Status::Full)
    {
        return Err(16);
    }
    let spare = planes.planes[0].id;
    planes.destroy(7, spare).map_err(|_| 17u32)?;
    if planes
        .create(
            7,
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            None,
        )
        .is_err()
    {
        return Err(17);
    }

    // 9. Task termination removes its planes and reports their rectangles.
    let mut removed = [Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }; MAX_PLANES];
    let count = planes.close_owner(7, &mut removed);
    if count == 0 || planes.count != 0 {
        return Err(18);
    }
    for rect in removed[..count].iter() {
        if rect.w <= 0 || rect.h <= 0 {
            return Err(19);
        }
    }

    Ok(())
}

/// Boot self-check: the window draw scope on a scratch table (spec 0026, C3).
/// Pure: no allocation, no global table, no screen.
pub fn begin_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;
    let mut table = Table::new();
    let one = table
        .open(SCREEN_W, SCREEN_H, 7, 100, 100, 200, 100, b"one", false, None)
        .map_err(|_| 1u32)?;
    let two = table
        .open(SCREEN_W, SCREEN_H, 8, 320, 200, 160, 90, b"two", false, None)
        .map_err(|_| 1u32)?;
    let one_slot = table.position(one).ok_or(2u32)?;
    let two_slot = table.position(two).ok_or(2u32)?;

    // 1. `open` makes the new window its owner's scope, so a single-window app
    //    can draw without an explicit begin.
    if table.windows[table.order[one_slot]].drawer != 7
        || table.windows[table.order[two_slot]].drawer != 8
    {
        return Err(3);
    }

    // 2. Owner-only: another task's window and an unknown id change nothing.
    if table.begin(7, two) != Status::Invalid
        || table.begin(8, one) != Status::Invalid
        || table.begin(7, 999) != Status::NotFound
        || table.windows[table.order[one_slot]].drawer != 7
        || table.windows[table.order[two_slot]].drawer != 8
    {
        return Err(4);
    }

    // 3. A second window for the same task takes over its one scope.
    let three = table
        .open(SCREEN_W, SCREEN_H, 7, 500, 300, 120, 80, b"three", false, None)
        .map_err(|_| 5u32)?;
    let three_slot = table.position(three).ok_or(6u32)?;
    if table.windows[table.order[one_slot]].drawer != 0
        || table.windows[table.order[three_slot]].drawer != 7
    {
        return Err(7);
    }

    // 4. A second `begin` replaces the previous scope.
    if table.begin(7, one) != Status::Ok
        || table.windows[table.order[one_slot]].drawer != 7
        || table.windows[table.order[three_slot]].drawer != 0
    {
        return Err(8);
    }

    // 5. `0` clears the caller's scope; there is no screen surface (C20).
    if table.begin(7, 0) != Status::Ok || table.windows[table.order[one_slot]].drawer != 0 {
        return Err(9);
    }

    // 6. Closing the window clears its scope with the record.
    if table.begin(7, one) != Status::Ok || table.close(7, one) != Status::Ok {
        return Err(10);
    }
    if (0..table.count).any(|pos| table.windows[table.order[pos]].drawer == 7) {
        return Err(11);
    }
    if table.begin(7, one) != Status::NotFound {
        return Err(12);
    }

    Ok(())
}

/// Boot self-check: the window-addressed input target on a scratch table
/// (spec 0011, W29; ADR 0047). Pure: no allocation, no global table, no screen.
pub fn target_self_check() -> Result<(), u32> {
    const SCREEN_W: i64 = 1024;
    const SCREEN_H: i64 = 768;
    let mut table = Table::new();
    let one = table
        .open(SCREEN_W, SCREEN_H, 7, 100, 100, 200, 100, b"one", false, None)
        .map_err(|_| 1u32)?;
    let two = table
        .open(SCREEN_W, SCREEN_H, 8, 150, 150, 100, 50, b"two", false, None)
        .map_err(|_| 2u32)?;
    let mut planes = PlaneTable::new();

    // A helper event with the fields the target rule reads.
    let event = |kind: abi::EventKind, x: i64, y: i64, window: i64| abi::Event {
        kind,
        x,
        y,
        code: 0,
        window,
    };

    // 1. Ownership and hit-testing: B is topmost and focused; a point that
    //    only A covers belongs to A, an empty point has no owner.
    if table.owner_of(one) != Some(7)
        || table.owner_of(two) != Some(8)
        || table.owner_of(0).is_some()
        || table.owner_of(99).is_some()
        || table.focus_owner() != Some(8)
        || table.hit_test_owner(110, 110) != Some(7)
        || table.hit_test_owner(160, 160) != Some(8)
        || table.hit_test_owner(900, 700).is_some()
    {
        return Err(3);
    }

    // 2. The pointer-down target is the topmost hit owner; moves, releases and
    //    keys target the focused owner. A desktop press has no target.
    if table.input_target(&planes, &event(abi::EventKind::PointerDown, 160, 160, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 110, 110, 0)) != Some(7)
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 900, 700, 0)).is_some()
        || table.input_target(&planes, &event(abi::EventKind::PointerMove, 10, 10, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::PointerUp, 10, 10, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::KeyDown, 0, 0, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::KeyUp, 0, 0, 0)) != Some(8)
    {
        return Err(4);
    }

    // 3. An addressed event belongs to its window's owner, whatever its kind
    //    and coordinates; an unknown address has no target, and a window-less
    //    Expose/Close is malformed and has none either.
    if table.input_target(&planes, &event(abi::EventKind::KeyDown, 900, 700, one as i64)) != Some(7)
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 10, 10, two as i64)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::Close, 0, 0, 99)) != None
        || table.input_target(&planes, &event(abi::EventKind::Expose, 0, 0, 0)).is_some()
        || table.input_target(&planes, &event(abi::EventKind::Close, 0, 0, 0)).is_some()
    {
        return Err(5);
    }

    // 4. A non-owner is denied the target: only the owner may dequeue. An
    //    untargeted event keeps the ordinary competition.
    let addressed = event(abi::EventKind::KeyDown, 0, 0, one as i64);
    if table.input_target(&planes, &addressed) == Some(8)
        || table.input_target(&planes, &addressed) != Some(7)
        || table.input_target(&planes, &event(abi::EventKind::KeyDown, 0, 0, 0)) != Some(8)
    {
        return Err(6);
    }

    // 5. A kernel window (owner 0) has no task target: a press on it, or keys
    //    while it is focused, are free-for-all. An addressed event to it is
    //    likewise untargeted, because no task owns the window.
    let kernel = table
        .open(SCREEN_W, SCREEN_H, 0, 400, 400, 100, 50, b"kernel", false, None)
        .map_err(|_| 7u32)?;
    if table.owner_of(kernel) != Some(0)
        || table.focus_owner().is_some()
        || table.hit_test_owner(410, 410).is_some()
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 410, 410, 0)).is_some()
        || table.input_target(&planes, &event(abi::EventKind::KeyDown, 0, 0, 0)).is_some()
        || table.input_target(&planes, &event(abi::EventKind::KeyDown, 0, 0, kernel as i64)).is_some()
    {
        return Err(8);
    }

    // 6. Closing it falls back to B and the focus target returns.
    if table.close(0, kernel) != Status::Ok || table.focus_owner() != Some(8) {
        return Err(9);
    }

    // 7. A plane owns a window-less press inside its rectangle even when a
    //    window lies underneath (spec 0026, C12), and takes precedence over the
    //    hit test; moves and keys still target the focus, and an addressed
    //    event keeps its own window's owner.
    let bar = planes
        .create(
            9,
            clamp_plane(SCREEN_W, SCREEN_H, 0, 0, SCREEN_W, 22).ok_or(10u32)?,
            None,
        )
        .map_err(|_| 10u32)?;
    if table.input_target(&planes, &event(abi::EventKind::PointerDown, 10, 10, 0)) != Some(9)
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 160, 160, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 900, 700, 0)).is_some()
        || table.input_target(&planes, &event(abi::EventKind::PointerMove, 10, 10, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::KeyDown, 0, 0, 0)) != Some(8)
        || table.input_target(&planes, &event(abi::EventKind::PointerDown, 10, 10, one as i64))
            != Some(7)
    {
        return Err(11);
    }

    // 8. A later plane is above an earlier one at the same point; removing it
    //    returns the press to the hit test.
    let overlay = planes
        .create(
            7,
            clamp_plane(SCREEN_W, SCREEN_H, 0, 0, SCREEN_W, 22).ok_or(12u32)?,
            None,
        )
        .map_err(|_| 12u32)?;
    if table.input_target(&planes, &event(abi::EventKind::PointerDown, 10, 10, 0)) != Some(7) {
        return Err(12);
    }
    let mut removed = [Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }; MAX_PLANES];
    if planes.close_owner(7, &mut removed) != 1 {
        return Err(13);
    }
    planes.destroy(9, bar).map_err(|_| 13u32)?;
    if table.input_target(&planes, &event(abi::EventKind::PointerDown, 10, 10, 0)).is_some() {
        return Err(14);
    }
    if planes.count != 0 || overlay == bar {
        return Err(15);
    }

    Ok(())
}

//! The shadow screen, retained canvases and presentation (spec 0009, 0026;
//! ADR 0020, ADR 0060).
//!
//! One full-screen pixel buffer is the shadow the service composes and
//! presents. A window's content lives in its own [`Canvas`] (owned by the
//! window record); the [`composite_canvas`] step copies it into the shadow
//! under the current shadow clip. The window service is the only writer of the
//! shadow: applications draw into their canvas, the kernel console draws into
//! its canvas, and only service paint (chrome, desktop pattern, composition)
//! and the transitional screen scope use the shadow primitives here.
//!
//! [`present`] copies a rectangle to the linear framebuffer row by row, using
//! the framebuffer pitch.
//!
//! The shadow is allocated from the frame allocator at boot from the mode the
//! bootloader actually returned: nothing assumes 1024×768 and there is no
//! static `.bss` image. When the framebuffer is missing, is not 32 bpp RGB, or
//! the allocation fails, the module stays disabled and every primitive reports
//! [`Status::Invalid`]; the console then works on its serial path only.
//!
//! Colours are `0x00RRGGBB`; coordinates are pixels with the origin at the
//! top-left. Drawing is clipped to a rectangular region (initially the whole
//! screen) and then to the screen or canvas, and never faults: out-of-range
//! extents and over-wide colours are rejected by the interpreter before they
//! reach here.
//!
//! # Pointer and cursor (spec 0010, ADR 0021)
//!
//! The module also owns the machine-wide pointer state (`x`, `y`, buttons) and
//! the software cursor, because the cursor is composited in [`present`]: the
//! shadow rectangle is copied to the framebuffer first and the cursor pixels
//! are written on top afterwards, **into the framebuffer**. The shadow never
//! contains the cursor, so [`get_pixel`] always sees what an application drew.
//!
//! Keeping the pointer next to the shadow means `present` reads the position
//! under the lock it already holds: no second lock, no lock-order question.

use abi::Status;

use crate::font;
use crate::font8x8::FONT8X8_BASIC;
use crate::frame;
use crate::mb2::Framebuffer;
use crate::sync::SpinLock;

/// Pointer source values reported by `syslib.pointer_source` (spec 0010, I20).
pub const POINTER_SOURCE_NONE: u32 = 0;
pub const POINTER_SOURCE_PS2: u32 = 1;
pub const POINTER_SOURCE_TABLET: u32 = 2;

/// The source that currently drives the machine-wide pointer. It is separate
/// from [`SCREEN`] so it is defined even when graphics are disabled. The
/// absolute USB tablet wins when it is ready; the PS/2 aux device is the
/// fallback (spec 0010, I21).
static POINTER_SOURCE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(POINTER_SOURCE_NONE);

/// Serialises the whole pointer transaction: the state update under [`SCREEN`],
/// the service-owned window interaction (`window::pointer_event`) and the event
/// publication. This is the outermost pointer lock and is taken only by
/// [`update_pointer`] and [`set_pointer_absolute`], so its order is
/// `POINTER_ORDER → SCREEN` and `POINTER_ORDER → WINDOW → SCREEN`.
///
/// Without it, a blocked `next_event` task could be woken by the queued
/// `PointerDown` and consume the press before the producer learns that the
/// window service handled it (spec 0011, W24).
static POINTER_ORDER: SpinLock<()> = SpinLock::new(());

/// Largest accepted colour value: the top byte of `0x00RRGGBB` must be zero.
pub const MAX_COLOR: u32 = 0x00FF_FFFF;

/// Accepted pointer button mask: bit 0 left, bit 1 right, bit 2 middle
/// (spec 0010, I10).
pub const POINTER_BUTTONS_MASK: u32 = 0b111;

/// Cursor extent in pixels; the hotspot is the top-left pixel (spec 0010, I6).
const CURSOR_W: i64 = 16;
const CURSOR_H: i64 = 16;
const CURSOR_OUTLINE_COLOR: u32 = 0x0000_0000;
const CURSOR_FILL_COLOR: u32 = 0x00FF_FFFF;

/// Black outline of the 16x16 arrow; bit `x` of row `y` is the pixel at
/// `(x, y)`. Hotspot `(0, 0)`.
///
/// ```text
/// #...............
/// ##..............
/// #+#.............
/// #++#............
/// #+++#...........
/// #++++#..........
/// #+++++#.........
/// #++++++#........
/// #+++++###.......
/// ##+++#..........
/// #.#++#..........
/// #..#+#..........
/// #...##..........
/// #...............
/// ................
/// ................
/// ```
///
/// `#` is the outline, `+` is the white fill. The bit masks are precomputed so
/// compositing is a table lookup, not a rasterisation (spec 0010, I7).
const CURSOR_OUTLINE: [u16; 16] = [
    0x0001, 0x0003, 0x0007, 0x000f, 0x001f, 0x003f, 0x007f, 0x00ff, 0x01ff, 0x003f, 0x003d, 0x0039,
    0x0031, 0x0001, 0x0000, 0x0000,
];

/// White fill, a subset of [`CURSOR_OUTLINE`].
const CURSOR_FILL: [u16; 16] = [
    0x0000, 0x0000, 0x0002, 0x0006, 0x000e, 0x001e, 0x003e, 0x007e, 0x003e, 0x001c, 0x0018, 0x0010,
    0x0000, 0x0000, 0x0000, 0x0000,
];

/// What `init` did, for the boot log.
pub enum Init {
    /// Graphics are enabled; the report carries the measured numbers.
    Enabled(Report),
    /// Graphics are disabled; the reason is named for the boot log.
    Disabled(Disabled),
}

/// The measured graphics state after a successful allocation.
pub struct Report {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u8,
    pub shadow: u64,
    pub shadow_bytes: usize,
    pub shadow_frames: usize,
    pub free_before: u64,
    pub free_after: u64,
}

/// Why graphics were disabled.
pub enum Disabled {
    /// No multiboot2 framebuffer tag.
    NoFramebuffer,
    /// Not 32 bpp RGB.
    UnsupportedMode { bpp: u8, kind: u8 },
    /// Zero-sized mode or a pitch narrower than one row.
    BadGeometry,
    /// The frame allocator could not provide the contiguous shadow.
    NoMemory { bytes: usize, frames: usize },
}

/// A rectangle in screen coordinates.
#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Intersects `(x, y, w, h)` with `clip` and the surface extent
/// `[origin, origin + limit)` and returns half-open **screen** bounds
/// `(x0, y0, x1, y1)`, or `None` when nothing is visible.
///
/// Kept free-standing so [`Screen::present`], which copies straight to the
/// framebuffer and never forms a surface, uses the same arithmetic.
fn clip_bounds(
    clip: Rect,
    origin: (i64, i64),
    limit: (i64, i64),
    x: i64,
    y: i64,
    w: i64,
    h: i64,
) -> Option<(i64, i64, i64, i64)> {
    let clip_x1 = i64::from(clip.x) + i64::from(clip.w);
    let clip_y1 = i64::from(clip.y) + i64::from(clip.h);
    let limit_x1 = origin.0.saturating_add(limit.0);
    let limit_y1 = origin.1.saturating_add(limit.1);
    let x0 = x.max(i64::from(clip.x)).max(origin.0);
    let y0 = y.max(i64::from(clip.y)).max(origin.1);
    let x1 = x.saturating_add(w).min(clip_x1).min(limit_x1);
    let y1 = y.saturating_add(h).min(clip_y1).min(limit_y1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

/// A rectangle of pixels and the mapping from the surface's coordinates onto it
/// (spec 0026, C1/C4; ADR 0060).
///
/// `ox`/`oy` translate a surface coordinate to a buffer coordinate
/// (`buffer = surface - origin`) and `limit` is the buffer's extent in pixels.
/// A **window canvas** is surface-local (spec 0026 C4/C8): its origin is
/// `(0, 0)`, so the client's coordinates are the buffer's. A **plane canvas**
/// and the shadow keep screen coordinates anchored at their origin (C11/C12);
/// planes never move, so that origin cannot go stale. The client-facing
/// primitives run through [`Surf`] and still bounds-check every store, so a bad
/// range is ignored rather than panicking.
struct Surf<'a> {
    pixels: &'a mut [u8],
    pitch: usize,
    clip: Rect,
    ox: i64,
    oy: i64,
    limit: (i64, i64),
}

impl Surf<'_> {
    /// Intersects a rectangle with the clip region and the surface extent.
    /// Returns half-open **surface (buffer)** bounds or `None` when nothing is
    /// visible.
    fn clipped(&self, x: i64, y: i64, w: i64, h: i64) -> Option<(usize, usize, usize, usize)> {
        let (x0, y0, x1, y1) = clip_bounds(self.clip, (self.ox, self.oy), self.limit, x, y, w, h)?;
        Some((
            (x0 - self.ox) as usize,
            (y0 - self.oy) as usize,
            (x1 - self.ox) as usize,
            (y1 - self.oy) as usize,
        ))
    }

    /// Writes one pixel. Bounds are the caller's responsibility; a bad range is
    /// ignored rather than panicking.
    fn store(&mut self, x: usize, y: usize, color: u32) {
        let offset = y * self.pitch + x * 4;
        if let Some(slot) = self.pixels.get_mut(offset..offset + 4) {
            slot.copy_from_slice(&(color & MAX_COLOR).to_le_bytes());
        }
    }

    fn load(&self, x: usize, y: usize) -> u32 {
        let offset = y * self.pitch + x * 4;
        match self.pixels.get(offset..offset + 4) {
            Some(bytes) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            None => 0,
        }
    }

    /// Reads one pixel in the surface's own coordinates (spec 0026 C6/C8):
    /// window-local for a canvas, screen for the shadow or a plane. Off-surface
    /// coordinates read `0`.
    fn read(&self, x: i64, y: i64) -> u32 {
        let bx = x.saturating_sub(self.ox);
        let by = y.saturating_sub(self.oy);
        if bx < 0 || by < 0 || bx >= self.limit.0 || by >= self.limit.1 {
            return 0;
        }
        self.load(bx as usize, by as usize)
    }

    /// Writes a pixel through the clip region. Used by the text and dither
    /// paths, where a whole-glyph rectangle would need per-pixel masking.
    fn put(&mut self, x: i64, y: i64, color: u32) {
        let clip_x1 = i64::from(self.clip.x) + i64::from(self.clip.w);
        let clip_y1 = i64::from(self.clip.y) + i64::from(self.clip.h);
        let limit_x1 = self.ox.saturating_add(self.limit.0);
        let limit_y1 = self.oy.saturating_add(self.limit.1);
        if x < i64::from(self.clip.x)
            || y < i64::from(self.clip.y)
            || x >= clip_x1
            || y >= clip_y1
            || x < self.ox
            || y < self.oy
            || x >= limit_x1
            || y >= limit_y1
        {
            return;
        }
        self.store((x - self.ox) as usize, (y - self.oy) as usize, color);
    }

    fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) {
        let Some((x0, y0, x1, y1)) = self.clipped(x, y, w, h) else {
            return;
        };
        let bytes = (color & MAX_COLOR).to_le_bytes();
        for row in y0..y1 {
            let start = row * self.pitch + x0 * 4;
            let end = row * self.pitch + x1 * 4;
            if let Some(slice) = self.pixels.get_mut(start..end) {
                for pixel in slice.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&bytes);
                }
            }
        }
    }

    fn dither(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) {
        let Some((x0, y0, x1, y1)) = self.clipped(x, y, w, h) else {
            return;
        };
        for row in y0..y1 {
            for col in x0..x1 {
                if ((col as i64 + self.ox) + (row as i64 + self.oy)) & 1 == 0 {
                    self.store(col, row, color);
                }
            }
        }
    }

    /// Scales a `pw × ph` RGB buffer into a destination rectangle with
    /// nearest-neighbour sampling (spec 0009, G22). The destination rectangle
    /// is clipped exactly like [`Surf::fill`]; the source sample for
    /// destination pixel `(dx, dy)` is `((dx * pw) / w, (dy * ph) / h)`. The
    /// caller has checked the dimensions and the buffer length.
    fn blit(&mut self, x: i64, y: i64, w: i64, h: i64, pixels: &[u8], pw: i64, ph: i64) {
        let Some((x0, y0, x1, y1)) = self.clipped(x, y, w, h) else {
            return;
        };
        for row in y0..y1 {
            let dy = i128::from(row as i64 + self.oy - y);
            let sy = (dy * i128::from(ph) / i128::from(h)) as usize;
            for col in x0..x1 {
                let dx = i128::from(col as i64 + self.ox - x);
                let sx = (dx * i128::from(pw) / i128::from(w)) as usize;
                let index = (sy * pw as usize + sx) * 3;
                let Some(rgb) = pixels.get(index..index + 3) else {
                    continue;
                };
                let color =
                    (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
                self.store(col, row, color);
            }
        }
    }

    fn outline(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) {
        // Four one-pixel strips inside the same extent. They are drawn through
        // `fill` so the clip applies once per strip; overlapping corners are
        // idempotent.
        self.fill(x, y, w, 1, color);
        self.fill(x, y + h - 1, w, 1, color);
        self.fill(x, y + 1, 1, h - 2, color);
        self.fill(x + w - 1, y + 1, 1, h - 2, color);
    }

    fn text(&mut self, x: i64, y: i64, text: &[u8], color: u32) {
        for (index, byte) in text.iter().enumerate() {
            let cell_x = x + index as i64 * 8;
            if *byte >= 0x80 {
                continue;
            }
            let glyph = &FONT8X8_BASIC[*byte as usize];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        self.put(cell_x + col as i64, y + row as i64, color);
                    }
                }
            }
        }
    }

    /// Paints one 8×8 glyph cell with an opaque background through this
    /// surface's clip (spec 0026, C1/C4). The console uses it for its canvas.
    fn glyph(&mut self, x: i64, y: i64, code: u8, fg: u32, bg: u32) {
        self.fill(x, y, 8, 8, bg);
        if code < 0x80 {
            let glyph = &FONT8X8_BASIC[code as usize];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        self.put(x + col as i64, y + row as i64, fg);
                    }
                }
            }
        }
    }

    /// Draws proportional text with a transparent background (spec 0015).
    ///
    /// Printable ASCII uses the variable-width font; every other byte keeps an
    /// 8×8 cell with an eight-pixel advance, drawn two rows down so its ink
    /// sits on the proportional baseline. Bytes at or above `0x80` have no 8×8
    /// glyph and draw nothing. `y` is the top of the 12-pixel line box.
    ///
    /// The font cache is taken on a miss while the caller holds [`SCREEN`]; the
    /// documented order is `SCREEN → FONT` (nothing takes the font lock and
    /// then the screen).
    fn text_prop(&mut self, x: i64, y: i64, text: &[u8], color: u32) {
        let mut pen = x;
        for byte in text {
            match font::glyph(*byte) {
                Some(glyph) => {
                    for (row, bits) in glyph.rows.iter().enumerate() {
                        for col in 0..glyph.width {
                            let bit = glyph.left + col;
                            if bits & (1u16 << bit) != 0 {
                                self.put(pen + i64::from(bit), y + row as i64, color);
                            }
                        }
                    }
                    pen += i64::from(glyph.advance);
                }
                None => {
                    if *byte < 0x80 {
                        let glyph = &FONT8X8_BASIC[*byte as usize];
                        for (row, bits) in glyph.iter().enumerate() {
                            for col in 0..8 {
                                if bits & (1 << col) != 0 {
                                    self.put(pen + col as i64, y + 2 + row as i64, color);
                                }
                            }
                        }
                    }
                    pen += i64::from(font::FALLBACK_ADVANCE);
                }
            }
        }
    }
}

/// A retained rectangle of pixels owned by one window (spec 0026, C1; ADR
/// 0060). `base` is a frame-allocator allocation of `bytes` (no row padding:
/// `pitch == w * 4`); `ox`/`oy` are the surface's current screen origin (the
/// compositor's placement).
///
/// The owner task writes it under the window table lock and the compositor
/// reads it under the same lock, so one writer and one reader are serialised.
#[derive(Clone, Copy)]
pub struct Canvas {
    /// First byte of the canvas allocation (identity-mapped physical memory).
    pub base: *mut u8,
    /// Bytes owned by the canvas.
    pub bytes: usize,
    /// Bytes per row (`w * 4`, no padding).
    pub pitch: usize,
    /// Width in pixels.
    pub w: i64,
    /// Height in pixels.
    pub h: i64,
    /// Screen origin of the canvas top-left (for composition).
    pub ox: i64,
    /// Screen origin of the canvas top-left (for composition).
    pub oy: i64,
    /// True for a window content canvas, whose drawing coordinates are
    /// window-local `(0, 0)` at the canvas top-left (spec 0026 C4/C8). False
    /// for a plane, whose client keeps screen coordinates anchored at
    /// `(ox, oy)` (C11/C12). A plane never moves, so its origin is stable.
    pub local: bool,
}

// SAFETY: the canvas has exactly one writer (the owning task, serialised by the
// window table lock) and one reader (the compositor, under the same lock); the
// raw pointer is only dereferenced while that lock is held (spec 0026, C1;
// ADR 0060 "Lock order").
unsafe impl Send for Canvas {}
unsafe impl Sync for Canvas {}

/// Builds a [`Surf`] over a canvas (spec 0026, C4/C8/C11). A window canvas is
/// surface-local: its origin is `(0, 0)` and its clip and extent are the canvas
/// size, so the client's coordinates are the buffer's. A plane canvas keeps
/// screen coordinates anchored at its screen rectangle.
fn canvas_surf<'a>(c: &'a Canvas) -> Surf<'a> {
    // SAFETY: `base`/`bytes` are the canvas allocation, kept alive by the
    // window record and only accessed under the window table lock. `pitch`,
    // `limit` and the clip keep every store inside `w * h * 4` bytes.
    let pixels = unsafe { core::slice::from_raw_parts_mut(c.base, c.bytes) };
    let (ox, oy) = if c.local { (0, 0) } else { (c.ox, c.oy) };
    Surf {
        pixels,
        pitch: c.pitch,
        clip: Rect {
            x: ox as i32,
            y: oy as i32,
            w: c.w as i32,
            h: c.h as i32,
        },
        ox,
        oy,
        limit: (c.w, c.h),
    }
}

/// Pointer state, machine-wide (spec 0010, I4/I5).
#[derive(Clone, Copy)]
struct Pointer {
    x: i32,
    y: i32,
    /// Bit 0 left, bit 1 right, bit 2 middle.
    buttons: u8,
    /// False until the pointer device initialises or the first injection
    /// (spec 0010, I9).
    visible: bool,
}

/// The screen image and the geometry needed to present it.
struct Screen {
    width: usize,
    height: usize,
    /// Bytes per row in both the shadow and the framebuffer.
    pitch: usize,
    /// Physical (identity-mapped) framebuffer base.
    fb: u64,
    /// `pitch * height` bytes owned by the screen until the next mode change.
    shadow: &'static mut [u8],
    /// Physical base of `shadow`, for `free_contiguous` on a mode change.
    shadow_base: u64,
    /// Contiguous frames owned by `shadow`.
    shadow_frames: usize,
    /// Rectangular clip, intersected with the screen (spec 0009, G7).
    clip: Rect,
    /// The one pointer, and whether the cursor is composited (spec 0010).
    pointer: Pointer,
}

static SCREEN: SpinLock<Option<Screen>> = SpinLock::new(None);

impl Screen {
    /// The whole screen as a drawing surface. The shadow's origin is `(0, 0)`,
    /// its extent is the screen and its clip is the shadow's clip, so screen
    /// and buffer coordinates agree (spec 0026, C1; ADR 0060).
    fn surf(&mut self) -> Surf<'_> {
        Surf {
            pixels: &mut *self.shadow,
            pitch: self.pitch,
            clip: self.clip,
            ox: 0,
            oy: 0,
            limit: (self.width as i64, self.height as i64),
        }
    }

    /// Writes one pixel straight to the framebuffer. Only the cursor overlay
    /// uses this: every shadow pixel reaches the screen through `present`.
    fn write_fb(&self, x: usize, y: usize, color: u32) {
        let offset = y * self.pitch + x * 4;
        let destination = (self.fb as usize + offset) as *mut u8;
        let bytes = (color & MAX_COLOR).to_le_bytes();
        // SAFETY: the framebuffer is identity-mapped for `pitch * height`
        // bytes; `x`/`y` are inside the screen and the cursor overlay passes
        // only columns of a row this `present` already copied.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, 4) };
    }

    /// The visible cursor rectangle, clipped to the screen, or `None` when the
    /// cursor is hidden or entirely off screen.
    fn cursor_rect(&self) -> Option<(i64, i64, i64, i64)> {
        if !self.pointer.visible {
            return None;
        }
        let x = self.pointer.x as i64;
        let y = self.pointer.y as i64;
        let x1 = x.saturating_add(CURSOR_W).min(self.width as i64);
        let y1 = y.saturating_add(CURSOR_H).min(self.height as i64);
        if x1 <= x || y1 <= y {
            return None;
        }
        Some((x, y, x1 - x, y1 - y))
    }

    /// Overwrites the cursor pixels inside the rectangle `(x0..x1, y0..y1)` of
    /// the framebuffer. The shadow is not touched (spec 0010, I7).
    fn overlay_cursor(&self, x0: usize, y0: usize, x1: usize, y1: usize) {
        if !self.pointer.visible {
            return;
        }
        let cx = self.pointer.x as i64;
        let cy = self.pointer.y as i64;
        let start_x = (x0 as i64).max(cx);
        let start_y = (y0 as i64).max(cy);
        let end_x = (x1 as i64).min(cx + CURSOR_W);
        let end_y = (y1 as i64).min(cy + CURSOR_H);
        if start_x >= end_x || start_y >= end_y {
            return;
        }
        for y in start_y..end_y {
            let row = (y - cy) as usize;
            let outline = CURSOR_OUTLINE[row];
            let fill = CURSOR_FILL[row];
            for x in start_x..end_x {
                let bit = 1u16 << ((x - cx) as u32);
                let color = if fill & bit != 0 {
                    CURSOR_FILL_COLOR
                } else if outline & bit != 0 {
                    CURSOR_OUTLINE_COLOR
                } else {
                    continue;
                };
                self.write_fb(x as usize, y as usize, color);
            }
        }
    }

    /// Copies a clipped rectangle from the shadow to the framebuffer, one row
    /// at a time, using the pitch (spec 0009, G5), then composites the cursor
    /// on top of it (spec 0010, I7).
    fn present(&mut self, x: i64, y: i64, w: i64, h: i64) {
        let limit = (self.width as i64, self.height as i64);
        let Some((x0, y0, x1, y1)) = clip_bounds(self.clip, (0, 0), limit, x, y, w, h) else {
            return;
        };
        let row_bytes = (x1 - x0) as usize * 4;
        if row_bytes == 0 {
            return;
        }
        for row in y0..y1 {
            let source = row as usize * self.pitch + x0 as usize * 4;
            let destination = self.fb as usize + source;
            // SAFETY: both ranges are inside the shadow and the framebuffer
            // for this row; `x0..x1` is clipped to the screen and the pitch is
            // the framebuffer's own row stride.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.shadow.as_ptr().add(source),
                    destination as *mut u8,
                    row_bytes,
                );
            }
        }
        self.overlay_cursor(x0 as usize, y0 as usize, x1 as usize, y1 as usize);
    }

}

/// Initialises the shadow from the bootloader's framebuffer.
///
/// Must run after [`crate::frame::init`]. On success the screen owns
/// `pitch * height` bytes of physically contiguous frames until the system
/// halts; they are never freed.
pub fn init(framebuffer: Option<Framebuffer>) -> Init {
    let Some(info) = framebuffer else {
        return Init::Disabled(Disabled::NoFramebuffer);
    };
    if info.bpp != 32 || info.kind != 1 {
        return Init::Disabled(Disabled::UnsupportedMode {
            bpp: info.bpp,
            kind: info.kind,
        });
    }
    let width = info.width as usize;
    let height = info.height as usize;
    let pitch = info.pitch as usize;
    if width == 0 || height == 0 || pitch < width.saturating_mul(4) {
        return Init::Disabled(Disabled::BadGeometry);
    }
    let Some(bytes) = pitch.checked_mul(height) else {
        return Init::Disabled(Disabled::BadGeometry);
    };
    let frames = bytes.div_ceil(frame::FRAME_SIZE as usize);
    let free_before = frame::free_frames();
    let Some(base) = frame::alloc_contiguous(frames) else {
        return Init::Disabled(Disabled::NoMemory { bytes, frames });
    };
    let free_after = frame::free_frames();

    // SAFETY: `alloc_contiguous` just reserved exactly these frames, they are
    // identity-mapped below 4 GiB and 4 KiB aligned, and the screen keeps the
    // slice for the lifetime of the system.
    let shadow = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, bytes) };
    shadow.fill(0);

    *SCREEN.lock() = Some(Screen {
        width,
        height,
        pitch,
        fb: info.addr,
        shadow,
        shadow_base: base,
        shadow_frames: frames,
        clip: Rect {
            x: 0,
            y: 0,
            w: width as i32,
            h: height as i32,
        },
        pointer: Pointer {
            x: 0,
            y: 0,
            buttons: 0,
            visible: false,
        },
    });

    Init::Enabled(Report {
        width: info.width,
        height: info.height,
        pitch: info.pitch,
        bpp: info.bpp,
        shadow: base,
        shadow_bytes: bytes,
        shadow_frames: frames,
        free_before,
        free_after,
    })
}

/// Replaces the screen image after a display mode change.
///
/// The caller has already allocated `frames` contiguous frames at `base` and
/// passes a closure that programs the display device. The closure runs while
/// the screen lock is held, so no other core can draw into the old image or
/// present it to the new framebuffer between the device change and the swap.
/// Returns the old shadow's `(base, frames)` on success, for the caller to
/// free once this function has released the lock; `None` leaves the old screen
/// untouched and the caller frees the unused new shadow.
pub(crate) fn change_mode(
    info: Framebuffer,
    base: u64,
    frames: usize,
    apply: impl FnOnce() -> bool,
) -> Option<(u64, usize)> {
    let width = info.width as usize;
    let height = info.height as usize;
    let pitch = info.pitch as usize;
    if info.bpp != 32
        || info.kind != 1
        || width == 0
        || height == 0
        || pitch < width.saturating_mul(4)
    {
        return None;
    }
    let bytes = pitch.checked_mul(height)?;
    if frames != bytes.div_ceil(frame::FRAME_SIZE as usize) {
        return None;
    }

    let mut guard = SCREEN.lock();
    let screen = guard.as_mut()?;
    if !apply() {
        return None;
    }

    // SAFETY: the caller reserved exactly these frames, they are
    // identity-mapped below 4 GiB, and the old slice is not used again.
    let shadow = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, bytes) };
    let old = (screen.shadow_base, screen.shadow_frames);
    screen.width = width;
    screen.height = height;
    screen.pitch = pitch;
    screen.fb = info.addr;
    screen.shadow = shadow;
    screen.shadow_base = base;
    screen.shadow_frames = frames;
    screen.clip = Rect {
        x: 0,
        y: 0,
        w: width as i32,
        h: height as i32,
    };
    screen.pointer.x = screen.pointer.x.clamp(0, width as i32 - 1);
    screen.pointer.y = screen.pointer.y.clamp(0, height as i32 - 1);
    Some(old)
}

/// Physical linear-framebuffer base of the current mode, or 0 when graphics
/// are disabled. Used by the display module as the Bochs dispi LFB address.
pub fn framebuffer_addr() -> u64 {
    match &*SCREEN.lock() {
        Some(screen) => screen.fb,
        None => 0,
    }
}

/// Screen size in pixels, `(0, 0)` when graphics are disabled.
pub fn size() -> (u32, u32) {
    match &*SCREEN.lock() {
        Some(screen) => (screen.width as u32, screen.height as u32),
        None => (0, 0),
    }
}

/// Narrows the shadow clip to `(x, y, w, h)`, intersected with the screen.
///
/// This is a kernel-internal operation for the window service (spec 0011,
/// W10): the service saves the clip by narrowing it for the rectangle it is
/// painting and calls [`reset_clip`] afterwards. There is no `syslib` binding,
/// so an application cannot read or change it. Returns `false` when graphics
/// are disabled and nothing was changed.
pub fn set_clip(x: i64, y: i64, w: i64, h: i64) -> bool {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = x.saturating_add(w).min(screen.width as i64);
    let y1 = y.saturating_add(h).min(screen.height as i64);
    screen.clip = if x1 <= x0 || y1 <= y0 {
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        }
    } else {
        Rect {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as i32,
            h: (y1 - y0) as i32,
        }
    };
    true
}

/// Restores the full-screen clip (spec 0011, W10).
pub fn reset_clip() {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return;
    };
    screen.clip = Rect {
        x: 0,
        y: 0,
        w: screen.width as i32,
        h: screen.height as i32,
    };
}

/// Boot self-check for the clip save/narrow/restore the window service needs
/// (spec 0011, W10): narrows the clip, paints across its edge, verifies that
/// nothing outside was written, then restores the clip and the pixels.
///
/// Returns `None` when graphics are disabled or the mode is too small to hold
/// the test region, `Some(true)` when the arithmetic is correct, and
/// `Some(false)` on a mismatch. It runs before the console paints, so the
/// restored pixels are the zeroed shadow; it never presents.
pub fn clip_self_check() -> Option<bool> {
    let (screen_w, screen_h) = size();
    let (screen_w, screen_h) = (screen_w as i64, screen_h as i64);
    if screen_w < 32 || screen_h < 32 {
        return None;
    }
    const X: i64 = 16;
    const Y: i64 = 16;
    const W: i64 = 4;
    const H: i64 = 4;
    let mut saved = [[0u32; W as usize]; H as usize];
    for (row, line) in saved.iter_mut().enumerate() {
        for (col, pixel) in line.iter_mut().enumerate() {
            *pixel = get_pixel(X + col as i64, Y + row as i64);
        }
    }

    if !set_clip(X, Y, W, H) {
        return Some(false);
    }
    let painted = MAX_COLOR;
    let status = fill_rect(X - 2, Y - 2, W + 4, H + 4, painted);
    let inside = get_pixel(X, Y) == painted && get_pixel(X + W - 1, Y + H - 1) == painted;
    let outside = get_pixel(X - 1, Y) != painted && get_pixel(X + W, Y) != painted;
    reset_clip();

    for (row, line) in saved.iter().enumerate() {
        for (col, pixel) in line.iter().enumerate() {
            let _ = fill_rect(X + col as i64, Y + row as i64, 1, 1, *pixel);
        }
    }
    let restored = get_pixel(X, Y) == saved[0][0];

    Some(status == Status::Ok && inside && outside && restored)
}

/// Boot self-check for the surface primitive (spec 0026, C4; ADR 0060): draws
/// through a scratch surface with a non-zero origin (a plane, screen
/// coordinates), then through a window-local one, reads the buffer back and
/// verifies that translated coordinates land where they should and that nothing
/// outside the clip or the surface extent was written. It uses a fixed stack
/// buffer and never touches [`SCREEN`], so it runs with graphics enabled or
/// disabled.
pub fn surface_self_check() -> bool {
    const W: usize = 16;
    const H: usize = 12;
    const OX: i64 = 10;
    const OY: i64 = 6;
    const LIMIT_W: i64 = 10;
    const LIMIT_H: i64 = 8;
    const CLIP_X: i64 = 12;
    const CLIP_Y: i64 = 8;
    const CLIP_W: i64 = 4;
    const CLIP_H: i64 = 3;
    const PAINT: u32 = 0x0012_3456;
    const PUT: u32 = 0x00AB_CDEF;
    const OFF_CLIP: u32 = 0x00FF_FFFF;

    let mut scratch = [0u8; W * H * 4];
    let mut ok = true;

    // Screen (12, 8) is buffer (2, 2), so the clip covers buffer columns 2..6
    // and rows 2..5. A fill that starts above-left and extends past every edge
    // must leave exactly that rectangle.
    {
        let mut surf = Surf {
            pixels: &mut scratch,
            pitch: W * 4,
            clip: Rect {
                x: CLIP_X as i32,
                y: CLIP_Y as i32,
                w: CLIP_W as i32,
                h: CLIP_H as i32,
            },
            ox: OX,
            oy: OY,
            limit: (LIMIT_W, LIMIT_H),
        };
        surf.fill(CLIP_X - 2, CLIP_Y - 2, CLIP_W + 4, CLIP_H + 4, PAINT);
        // One pixel inside the clip (buffer 3,3) and one just outside each of
        // the four clip edges.
        surf.put(CLIP_X + 1, CLIP_Y + 1, PUT);
        surf.put(CLIP_X + CLIP_W, CLIP_Y, OFF_CLIP);
        surf.put(CLIP_X - 1, CLIP_Y, OFF_CLIP);
        surf.put(CLIP_X, CLIP_Y + CLIP_H, OFF_CLIP);
        surf.put(CLIP_X + CLIP_W - 1, CLIP_Y + CLIP_H, OFF_CLIP);

        for row in 0..H {
            for col in 0..W {
                let expected = if (col, row) == (3, 3) {
                    PUT
                } else if (2..6).contains(&col) && (2..5).contains(&row) {
                    PAINT
                } else {
                    0
                };
                if surf.load(col, row) != expected {
                    ok = false;
                }
            }
        }
    }

    // A clip larger than the surface still stops at the extent: the clip and
    // fill reach screen x 24 and y 20, but the limit is x 20 and y 14, so the
    // result is buffer columns 2..10 and rows 2..8.
    scratch.fill(0);
    {
        let mut surf = Surf {
            pixels: &mut scratch,
            pitch: W * 4,
            clip: Rect {
                x: CLIP_X as i32,
                y: CLIP_Y as i32,
                w: (LIMIT_W + 2) as i32,
                h: (LIMIT_H + 2) as i32,
            },
            ox: OX,
            oy: OY,
            limit: (LIMIT_W, LIMIT_H),
        };
        surf.fill(0, 0, W as i64 + 8, H as i64 + 8, PAINT);
        for row in 0..H {
            for col in 0..W {
                let expected = if (2..10).contains(&col) && (2..8).contains(&row) {
                    PAINT
                } else {
                    0
                };
                if surf.load(col, row) != expected {
                    ok = false;
                }
            }
        }
    }

    // A window canvas (C4/C8) is surface-local: origin zero, clip and limit at
    // the canvas size. A fill at local (0, 0) covers the buffer corner and a
    // read of a local coordinate answers from that pixel.
    scratch.fill(0);
    {
        let mut surf = Surf {
            pixels: &mut scratch,
            pitch: W * 4,
            clip: Rect {
                x: 0,
                y: 0,
                w: W as i32,
                h: H as i32,
            },
            ox: 0,
            oy: 0,
            limit: (W as i64, H as i64),
        };
        surf.fill(0, 0, 4, 3, PAINT);
        surf.put(3, 2, PUT);
        if surf.read(0, 0) != PAINT
            || surf.read(3, 2) != PUT
            || surf.read(4, 0) != 0
            || surf.read(0, 3) != 0
            || surf.read(-1, 0) != 0
        {
            ok = false;
        }
        for row in 0..H {
            for col in 0..W {
                let expected = if (col, row) == (3, 2) {
                    PUT
                } else if col < 4 && row < 3 {
                    PAINT
                } else {
                    0
                };
                if surf.load(col, row) != expected {
                    ok = false;
                }
            }
        }
    }

    ok
}

/// Applies a pointer delta with clamping to the screen (spec 0010, I4).
///
/// `dy` is screen-oriented: positive moves down. A zero-sized screen clamps to
/// `(0, 0)`. Pure, so the boot self-check uses the same arithmetic as the live
/// path.
pub fn apply_motion(x: i64, y: i64, dx: i64, dy: i64, width: i64, height: i64) -> (i64, i64) {
    let max_x = (width - 1).max(0);
    let max_y = (height - 1).max(0);
    (
        x.saturating_add(dx).clamp(0, max_x),
        y.saturating_add(dy).clamp(0, max_y),
    )
}

/// The machine-wide pointer state as `(x, y, buttons)` (spec 0010, I4).
pub fn pointer() -> (i64, i64, u64) {
    match &*SCREEN.lock() {
        Some(screen) => (
            screen.pointer.x as i64,
            screen.pointer.y as i64,
            u64::from(screen.pointer.buttons),
        ),
        None => (0, 0, 0),
    }
}

/// Moves the pointer, sets the buttons and repaints only the old and new
/// cursor rectangles (spec 0010, I4/I8). `dy` is screen-oriented: positive
/// moves down.
pub fn update_pointer(dx: i64, dy: i64, buttons: u32) -> Status {
    if buttons > POINTER_BUTTONS_MASK {
        return Status::Invalid;
    }
    // One serialised pointer transaction: state update + cursor, then the
    // service window hook (after the graphics lock is dropped), then event
    // publication. The order makes a handled press invisible to clients.
    let _order = POINTER_ORDER.lock();
    let (old_x, old_y, old_buttons, x, y) = {
        let mut guard = SCREEN.lock();
        let Some(screen) = guard.as_mut() else {
            return Status::Invalid;
        };
        let old = screen.cursor_rect();
        let old_x = i64::from(screen.pointer.x);
        let old_y = i64::from(screen.pointer.y);
        let old_buttons = u64::from(screen.pointer.buttons);
        let (width, height) = (screen.width as i64, screen.height as i64);
        let (x, y) = apply_motion(
            screen.pointer.x as i64,
            screen.pointer.y as i64,
            dx,
            dy,
            width,
            height,
        );
        screen.pointer.x = x as i32;
        screen.pointer.y = y as i32;
        screen.pointer.buttons = buttons as u8;
        screen.pointer.visible = true;

        let new = screen.cursor_rect();
        if old != new {
            if let Some((x, y, w, h)) = old {
                screen.present(x, y, w, h);
            }
        }
        if let Some((x, y, w, h)) = new {
            screen.present(x, y, w, h);
        }
        (old_x, old_y, old_buttons, x, y)
    };
    // Service-owned window interaction (spec 0011, W22/W23) runs only after
    // the graphics lock is dropped: the repair takes window -> graphics.
    let consumed = crate::window::pointer_event(
        old_x,
        old_y,
        old_buttons,
        x,
        y,
        u64::from(buttons),
    );
    // Publish after the service decision, suppressing the press it handled, so
    // no task woken by the publication can observe a consumed click (W24).
    crate::event::push_pointer(
        old_x,
        old_y,
        old_buttons,
        x,
        y,
        u64::from(buttons),
        consumed,
    );
    Status::Ok
}

/// Makes the cursor visible and paints it where the pointer is (spec 0010,
/// I9). Called when the pointer device initialises; returns false when
/// graphics are disabled.
pub fn show_cursor() -> bool {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    screen.pointer.visible = true;
    if let Some((x, y, w, h)) = screen.cursor_rect() {
        screen.present(x, y, w, h);
    }
    true
}

/// The source currently driving the pointer (spec 0010, I20): 0 none, 1 PS/2,
/// 2 USB tablet. The value is independent of whether graphics are enabled.
pub fn pointer_source() -> u32 {
    POINTER_SOURCE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Records which device drives the pointer. Called by the device bring-up
/// paths: PS/2 on a successful aux init, the tablet after enumeration, and
/// back to PS/2 (or none) if the tablet fails (spec 0010, I21).
pub fn set_pointer_source(source: u32) {
    POINTER_SOURCE.store(source, core::sync::atomic::Ordering::Relaxed);
}

/// Sets the pointer to an absolute screen position, with clamping to the
/// screen and only the old/new cursor rectangles repainted (spec 0010, I20).
///
/// This is the USB tablet path: the caller has already scaled the HID axis to
/// screen pixels. A repeated report is idempotent — the same position and
/// buttons repaint nothing — so a dropped or duplicated absolute report is
/// harmless. `buttons` above `0b111` is `Status.Invalid` and changes nothing.
pub fn set_pointer_absolute(x: i64, y: i64, buttons: u32) -> Status {
    if buttons > POINTER_BUTTONS_MASK {
        return Status::Invalid;
    }
    // Same single pointer transaction as `update_pointer` (spec 0011, W24).
    let _order = POINTER_ORDER.lock();
    let (old_x, old_y, old_buttons, x, y) = {
        let mut guard = SCREEN.lock();
        let Some(screen) = guard.as_mut() else {
            return Status::Invalid;
        };
        let max_x = (screen.width as i64 - 1).max(0);
        let max_y = (screen.height as i64 - 1).max(0);
        let x = x.clamp(0, max_x) as i32;
        let y = y.clamp(0, max_y) as i32;
        let buttons = buttons as u8;

        if screen.pointer.visible
            && screen.pointer.x == x
            && screen.pointer.y == y
            && screen.pointer.buttons == buttons
        {
            return Status::Ok;
        }

        let old = screen.cursor_rect();
        let old_x = i64::from(screen.pointer.x);
        let old_y = i64::from(screen.pointer.y);
        let old_buttons = u64::from(screen.pointer.buttons);
        screen.pointer.x = x;
        screen.pointer.y = y;
        screen.pointer.buttons = buttons;
        screen.pointer.visible = true;

        let new = screen.cursor_rect();
        if old != new {
            if let Some((x, y, w, h)) = old {
                screen.present(x, y, w, h);
            }
        }
        if let Some((x, y, w, h)) = new {
            screen.present(x, y, w, h);
        }
        (old_x, old_y, old_buttons, i64::from(x), i64::from(y))
    };
    // Window drag/close interaction, after the graphics lock is dropped
    // (spec 0011, W22/W23).
    let consumed = crate::window::pointer_event(
        old_x,
        old_y,
        old_buttons,
        x,
        y,
        u64::from(buttons),
    );
    // Publish after the service decision; a consumed press is never queued.
    crate::event::push_pointer(
        old_x,
        old_y,
        old_buttons,
        x,
        y,
        u64::from(buttons),
        consumed,
    );
    Status::Ok
}

fn valid_rect(w: i64, h: i64, color: u32) -> bool {
    w > 0 && h > 0 && color <= MAX_COLOR
}

/// Runs one primitive on the shadow under [`SCREEN`]. The surface is the whole
/// shadow with a zero origin, so screen and buffer coordinates agree; the
/// shadow's clip still bounds the write (spec 0026, C1; ADR 0060). Returns
/// `Invalid` and draws nothing when graphics are disabled.
fn with_surf(op: impl FnOnce(&mut Surf<'_>)) -> Status {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return Status::Invalid;
    };
    op(&mut screen.surf());
    Status::Ok
}

pub fn fill_rect(x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    with_surf(|surf| surf.fill(x, y, w, h, color))
}

pub fn fill_dither(x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    with_surf(|surf| surf.dither(x, y, w, h, color))
}

pub fn draw_rect(x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    with_surf(|surf| surf.outline(x, y, w, h, color))
}

// ---- proportional text (spec 0015) ---------------------------------------

/// The proportional font's line height in pixels (12). Kernel callers that
/// already know graphics are enabled, such as the window chrome, use this
/// constant directly.
pub const PROP_LINE_HEIGHT: i64 = font::LINE_HEIGHT as i64;

/// The advance of one byte in the proportional font (8 for a fallback byte).
pub fn prop_advance(byte: u8) -> i64 {
    i64::from(font::advance(byte))
}

/// `syslib.text_width_prop`: the measured advance of `text`, or 0 when graphics
/// are disabled (the same rule as `screen_width`).
pub fn text_width_prop(text: &[u8]) -> u64 {
    if SCREEN.lock().is_none() {
        return 0;
    }
    font::text_width(text)
}

/// `syslib.text_height_prop`: the 12-pixel line height, or 0 when graphics are
/// disabled.
pub fn text_height_prop() -> u64 {
    if SCREEN.lock().is_none() {
        return 0;
    }
    u64::from(font::LINE_HEIGHT)
}

/// `syslib.draw_text_prop`: draws proportional text into the shadow with a
/// transparent background. The interpreter rejects an over-wide colour; the
/// screen and the clip still bound the pixels, so an off-screen origin or a
/// partially visible string is clipped and never faults.
pub fn draw_text_prop(x: i64, y: i64, text: &[u8], color: u32) -> Status {
    if color > MAX_COLOR {
        return Status::Invalid;
    }
    with_surf(|surf| surf.text_prop(x, y, text, color))
}

pub fn get_pixel(x: i64, y: i64) -> u32 {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return 0;
    };
    if x < 0 || y < 0 || x >= screen.width as i64 || y >= screen.height as i64 {
        return 0;
    }
    screen.surf().load(x as usize, y as usize)
}

pub fn present(x: i64, y: i64, w: i64, h: i64) -> Status {
    if w <= 0 || h <= 0 {
        return Status::Invalid;
    }
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.present(x, y, w, h);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

// ---- canvas primitives (spec 0026, C4; ADR 0060) ------------------------

/// `syslib.fill_rect` into a window canvas. The argument checks are exactly
/// the shadow primitive's; coordinates are screen coordinates translated by the
/// canvas origin (M16.3 scaffolding). Never presents.
pub fn fill_rect_on(c: &Canvas, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    canvas_surf(c).fill(x, y, w, h, color);
    Status::Ok
}

/// `syslib.fill_dither` into a window canvas (spec 0026, C4).
pub fn fill_dither_on(c: &Canvas, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    canvas_surf(c).dither(x, y, w, h, color);
    Status::Ok
}

/// `syslib.draw_rect` into a window canvas (spec 0026, C4).
pub fn draw_rect_on(c: &Canvas, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    canvas_surf(c).outline(x, y, w, h, color);
    Status::Ok
}

/// `syslib.draw_text` into a window canvas (spec 0026, C4).
pub fn draw_text_on(c: &Canvas, x: i64, y: i64, text: &[u8], color: u32) -> Status {
    if color > MAX_COLOR {
        return Status::Invalid;
    }
    canvas_surf(c).text(x, y, text, color);
    Status::Ok
}

/// `syslib.draw_text_prop` into a window canvas (spec 0026, C4).
pub fn draw_text_prop_on(c: &Canvas, x: i64, y: i64, text: &[u8], color: u32) -> Status {
    if color > MAX_COLOR {
        return Status::Invalid;
    }
    canvas_surf(c).text_prop(x, y, text, color);
    Status::Ok
}

/// `syslib.blit_rgb` into a window canvas (spec 0026, C4). The buffer checks
/// are the shadow primitive's.
pub fn blit_rgb_on(
    c: &Canvas,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    pixels: &[u8],
    pw: i64,
    ph: i64,
) -> Status {
    if w <= 0 || h <= 0 || pw <= 0 || ph <= 0 {
        return Status::Invalid;
    }
    let needed = (pw as u64)
        .checked_mul(ph as u64)
        .and_then(|count| count.checked_mul(3));
    match needed {
        Some(needed) if pixels.len() as u64 >= needed => {}
        _ => return Status::Invalid,
    }
    canvas_surf(c).blit(x, y, w, h, pixels, pw, ph);
    Status::Ok
}

/// `syslib.clear` into a window canvas: fills the whole surface (spec 0026,
/// C5). It is scope-local, never screen-wide; the fill covers the surface's own
/// rectangle, whether that is window-local or plane-screen.
pub fn clear_on(c: &Canvas, color: u32) -> Status {
    if color > MAX_COLOR {
        return Status::Invalid;
    }
    let mut surf = canvas_surf(c);
    let x = i64::from(surf.clip.x);
    let y = i64::from(surf.clip.y);
    surf.fill(x, y, c.w, c.h, color);
    Status::Ok
}

/// `syslib.get_pixel` inside a draw scope (spec 0026, C6/C8): reads the scope
/// surface in its own coordinates — window-local for a window canvas, screen
/// for a plane — so a client's pixel self-check sees its own pixels before the
/// present. Off-surface coordinates read `0`.
pub fn read_pixel_on(c: &Canvas, x: i64, y: i64) -> u32 {
    canvas_surf(c).read(x, y)
}

/// Paints one opaque 8×8 glyph cell into a window canvas (spec 0026, C4). The
/// kernel console uses it for its text grid.
pub fn draw_cell_on(c: &Canvas, x: i64, y: i64, code: u8, fg: u32, bg: u32) -> Status {
    if fg > MAX_COLOR || bg > MAX_COLOR {
        return Status::Invalid;
    }
    canvas_surf(c).glyph(x, y, code, fg, bg);
    Status::Ok
}

/// Copies a canvas into the shadow under the current shadow clip, row by row
/// with the canvas pitch (spec 0026, C14; ADR 0060). It neither touches the
/// clip nor presents: the compositor presents the rectangle once after every
/// layer is painted.
pub fn composite_canvas(c: &Canvas) {
    if c.w <= 0 || c.h <= 0 || c.pitch == 0 {
        return;
    }
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return;
    };
    let clip = screen.clip;
    let clip_x1 = i64::from(clip.x) + i64::from(clip.w);
    let clip_y1 = i64::from(clip.y) + i64::from(clip.h);
    let x0 = c.ox.max(i64::from(clip.x)).max(0);
    let y0 = c.oy.max(i64::from(clip.y)).max(0);
    let x1 = c
        .ox
        .saturating_add(c.w)
        .min(clip_x1)
        .min(screen.width as i64);
    let y1 = c
        .oy
        .saturating_add(c.h)
        .min(clip_y1)
        .min(screen.height as i64);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let row_bytes = (x1 - x0) as usize * 4;
    let source_x = (x0 - c.ox) as usize;
    let source_y = (y0 - c.oy) as usize;
    let dest_x = x0 as usize;
    let shadow = screen.shadow.as_mut_ptr();
    let shadow_len = screen.shadow.len();
    for row in 0..(y1 - y0) as usize {
        let source = (source_y + row) * c.pitch + source_x * 4;
        let destination = (y0 as usize + row) * screen.pitch + dest_x * 4;
        if source + row_bytes > c.bytes || destination + row_bytes > shadow_len {
            continue;
        }
        // SAFETY: `source`/`destination` are inside the canvas and shadow
        // allocations for exactly `row_bytes`; all bounds are checked above.
        unsafe {
            core::ptr::copy_nonoverlapping(
                c.base.add(source),
                shadow.add(destination),
                row_bytes,
            );
        }
    }
}

//! The shadow screen and presentation (spec 0009, ADR 0020).
//!
//! One full-screen pixel buffer is the source of truth. Every `syslib` drawing
//! primitive, the text console and the future window service write here;
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
//! screen) and then to the screen, and never faults: out-of-range extents and
//! over-wide colours are rejected by the interpreter before they reach here.
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

/// An explicit clip rectangle for kernel-internal drawing (spec 0012, D3).
///
/// The window service's repairs save, narrow and restore the shadow's one
/// global clip (spec 0011, W10). The kernel text windows cannot use that for a
/// whole-grid repaint: the clip is global, so a long repaint would clip another
/// task's drawing on another core. These helpers therefore take the clip as an
/// argument and never touch [`Screen::clip`].
#[derive(Clone, Copy)]
pub struct Clip {
    /// Left edge in screen pixels.
    pub x: i64,
    /// Top edge in screen pixels.
    pub y: i64,
    /// Width in pixels.
    pub w: i64,
    /// Height in pixels.
    pub h: i64,
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
    /// Intersects a rectangle with the clip region and the screen. Returns
    /// half-open bounds `(x0, y0, x1, y1)` or `None` when nothing is visible.
    fn clipped(&self, x: i64, y: i64, w: i64, h: i64) -> Option<(usize, usize, usize, usize)> {
        let clip_x1 = self.clip.x as i64 + self.clip.w as i64;
        let clip_y1 = self.clip.y as i64 + self.clip.h as i64;
        let x0 = x.max(self.clip.x as i64).max(0);
        let y0 = y.max(self.clip.y as i64).max(0);
        let x1 = x.saturating_add(w).min(clip_x1).min(self.width as i64);
        let y1 = y.saturating_add(h).min(clip_y1).min(self.height as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
    }

    /// Intersects a rectangle with an explicit clip and the screen (spec 0012,
    /// D3). Unlike [`Screen::clipped`], the global `clip` is ignored.
    fn clipped_by(
        &self,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        clip: Clip,
    ) -> Option<(usize, usize, usize, usize)> {
        let clip_x1 = clip.x.saturating_add(clip.w);
        let clip_y1 = clip.y.saturating_add(clip.h);
        let x0 = x.max(clip.x).max(0);
        let y0 = y.max(clip.y).max(0);
        let x1 = x.saturating_add(w).min(clip_x1).min(self.width as i64);
        let y1 = y.saturating_add(h).min(clip_y1).min(self.height as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
    }

    /// Writes one pixel. Bounds are the caller's responsibility; a bad range is
    /// ignored rather than panicking.
    fn store(&mut self, x: usize, y: usize, color: u32) {
        let offset = y * self.pitch + x * 4;
        if let Some(slot) = self.shadow.get_mut(offset..offset + 4) {
            slot.copy_from_slice(&(color & MAX_COLOR).to_le_bytes());
        }
    }

    fn load(&self, x: usize, y: usize) -> u32 {
        let offset = y * self.pitch + x * 4;
        match self.shadow.get(offset..offset + 4) {
            Some(bytes) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            None => 0,
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

    /// Writes a pixel through the clip region. Used by the text and dither
    /// paths, where a whole-glyph rectangle would need per-pixel masking.
    fn put(&mut self, x: i64, y: i64, color: u32) {
        let clip_x1 = self.clip.x as i64 + self.clip.w as i64;
        let clip_y1 = self.clip.y as i64 + self.clip.h as i64;
        if x < self.clip.x as i64
            || y < self.clip.y as i64
            || x >= clip_x1
            || y >= clip_y1
            || x < 0
            || y < 0
            || x >= self.width as i64
            || y >= self.height as i64
        {
            return;
        }
        self.store(x as usize, y as usize, color);
    }

    /// Writes one pixel through an explicit clip (spec 0012, D3).
    fn put_in(&mut self, x: i64, y: i64, color: u32, clip: Clip) {
        if x < clip.x
            || y < clip.y
            || x >= clip.x.saturating_add(clip.w)
            || y >= clip.y.saturating_add(clip.h)
            || x < 0
            || y < 0
            || x >= self.width as i64
            || y >= self.height as i64
        {
            return;
        }
        self.store(x as usize, y as usize, color);
    }

    fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) {
        let Some((x0, y0, x1, y1)) = self.clipped(x, y, w, h) else {
            return;
        };
        let bytes = (color & MAX_COLOR).to_le_bytes();
        for row in y0..y1 {
            let start = row * self.pitch + x0 * 4;
            let end = row * self.pitch + x1 * 4;
            if let Some(slice) = self.shadow.get_mut(start..end) {
                for pixel in slice.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&bytes);
                }
            }
        }
    }

    /// Fills a rectangle through an explicit clip (spec 0012, D3).
    fn fill_in(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32, clip: Clip) {
        let Some((x0, y0, x1, y1)) = self.clipped_by(x, y, w, h, clip) else {
            return;
        };
        let bytes = (color & MAX_COLOR).to_le_bytes();
        for row in y0..y1 {
            let start = row * self.pitch + x0 * 4;
            let end = row * self.pitch + x1 * 4;
            if let Some(slice) = self.shadow.get_mut(start..end) {
                for pixel in slice.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&bytes);
                }
            }
        }
    }

    /// Paints one 8×8 glyph cell with an opaque background through an explicit
    /// clip, without presenting (spec 0012, D3).
    fn glyph_in(&mut self, x: i64, y: i64, code: u8, fg: u32, bg: u32, clip: Clip) {
        self.fill_in(x, y, 8, 8, bg, clip);
        if code < 0x80 {
            let glyph = &FONT8X8_BASIC[code as usize];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        self.put_in(x + col as i64, y + row as i64, fg, clip);
                    }
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
                if (row + col) & 1 == 0 {
                    self.store(col, row, color);
                }
            }
        }
    }

    /// Scales a `pw × ph` RGB buffer into a destination rectangle with
    /// nearest-neighbour sampling (spec 0009, G22). The destination rectangle
    /// is clipped exactly like [`Screen::fill`]; the source sample for
    /// destination pixel `(dx, dy)` is `((dx * pw) / w, (dy * ph) / h)`. The
    /// caller has checked the dimensions and the buffer length.
    fn blit(&mut self, x: i64, y: i64, w: i64, h: i64, pixels: &[u8], pw: i64, ph: i64) {
        let Some((x0, y0, x1, y1)) = self.clipped(x, y, w, h) else {
            return;
        };
        for row in y0..y1 {
            let dy = i128::from(row as i64 - x);
            let sy = (dy * i128::from(ph) / i128::from(h)) as usize;
            for col in x0..x1 {
                let dx = i128::from(col as i64 - x);
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

    /// Draws proportional text with a transparent background (spec 0015).
    ///
    /// Printable ASCII uses the variable-width font; every other byte keeps an
    /// 8×8 cell with an eight-pixel advance, drawn two rows down so its ink
    /// sits on the proportional baseline. Bytes at or above `0x80` have no 8×8
    /// glyph and draw nothing. `y` is the top of the 12-pixel line box.
    ///
    /// The font cache is taken under `SCREEN` on a miss; the documented order
    /// is `SCREEN → FONT` (nothing takes the font lock and then the screen).
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

    /// Copies a clipped rectangle from the shadow to the framebuffer, one row
    /// at a time, using the pitch (spec 0009, G5), then composites the cursor
    /// on top of it (spec 0010, I7).
    fn present(&mut self, x: i64, y: i64, w: i64, h: i64) {
        let Some((x0, y0, x1, y1)) = self.clipped(x, y, w, h) else {
            return;
        };
        let row_bytes = (x1 - x0) * 4;
        if row_bytes == 0 {
            return;
        }
        for row in y0..y1 {
            let source = row * self.pitch + x0 * 4;
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
        self.overlay_cursor(x0, y0, x1, y1);
    }

    /// Copies a rectangle to the framebuffer through an explicit clip
    /// (spec 0012, D3), then composites the cursor inside the copied region.
    fn present_in(&mut self, x: i64, y: i64, w: i64, h: i64, clip: Clip) {
        let Some((x0, y0, x1, y1)) = self.clipped_by(x, y, w, h, clip) else {
            return;
        };
        let row_bytes = (x1 - x0) * 4;
        if row_bytes == 0 {
            return;
        }
        for row in y0..y1 {
            let source = row * self.pitch + x0 * 4;
            let destination = self.fb as usize + source;
            // SAFETY: both ranges are inside the shadow and the framebuffer
            // for this row; the bounds are clipped to the screen and the
            // framebuffer pitch is the shadow's row stride.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.shadow.as_ptr().add(source),
                    destination as *mut u8,
                    row_bytes,
                );
            }
        }
        self.overlay_cursor(x0, y0, x1, y1);
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

/// Copies a rectangle inside the shadow (spec 0011, W28).
///
/// Kernel-internal: the window service uses it to move a topmost window's
/// content snapshot to the new origin before a repair. There is no `syslib`
/// binding, so an application cannot copy screen pixels. The copy is clipped to
/// the screen and the current shadow clip; a fully clipped request is a no-op
/// `Ok`. Rows are copied with an overlap-safe `copy_within`, and the row order
/// follows the vertical direction (`bottom-to-top` when the destination is
/// lower) so a vertically overlapping source is not clobbered. The cursor is
/// never touched: it lives in the framebuffer, composited by [`present`].
///
/// Returns `Invalid` for a non-positive extent or disabled graphics.
pub fn copy_rect(
    src_x: i64,
    src_y: i64,
    dst_x: i64,
    dst_y: i64,
    w: i64,
    h: i64,
) -> Status {
    if w <= 0 || h <= 0 {
        return Status::Invalid;
    }
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return Status::Invalid;
    };
    let screen_w = screen.width as i64;
    let screen_h = screen.height as i64;
    let clip_x0 = i64::from(screen.clip.x).max(0);
    let clip_y0 = i64::from(screen.clip.y).max(0);
    let clip_x1 = i64::from(screen.clip.x) + i64::from(screen.clip.w);
    let clip_y1 = i64::from(screen.clip.y) + i64::from(screen.clip.h);

    // Start with the destination rectangle clipped to the screen and clip.
    let mut left = dst_x.max(clip_x0);
    let mut top = dst_y.max(clip_y0);
    let mut right = dst_x.saturating_add(w).min(clip_x1).min(screen_w);
    let mut bottom = dst_y.saturating_add(h).min(clip_y1).min(screen_h);
    if right <= left || bottom <= top {
        return Status::Ok;
    }

    // The source keeps the same crop. Clip it to the screen by shrinking the
    // destination by the same amount, so source and destination stay aligned.
    let src_left = src_x.saturating_add(left - dst_x);
    if src_left < 0 {
        left -= src_left;
    }
    let src_top = src_y.saturating_add(top - dst_y);
    if src_top < 0 {
        top -= src_top;
    }
    let src_right = src_x.saturating_add(right - dst_x);
    if src_right > screen_w {
        right -= src_right - screen_w;
    }
    let src_bottom = src_y.saturating_add(bottom - dst_y);
    if src_bottom > screen_h {
        bottom -= src_bottom - screen_h;
    }
    if right <= left || bottom <= top {
        return Status::Ok;
    }

    let columns = (right - left) as usize;
    let rows = (bottom - top) as usize;
    let row_bytes = columns * 4;
    let src_column = (src_x + (left - dst_x)) as usize;
    let src_row = (src_y + (top - dst_y)) as usize;
    let dst_column = left as usize;
    let dst_row = top as usize;

    // Vertical overlap is the row-iteration order: moving down (destination
    // below the source) must start at the last row. Horizontal overlap is
    // handled by the slice's own `copy_within`.
    if dst_y >= src_y {
        for step in (0..rows).rev() {
            let source = (src_row + step) * screen.pitch + src_column * 4;
            let destination = (dst_row + step) * screen.pitch + dst_column * 4;
            screen
                .shadow
                .copy_within(source..source + row_bytes, destination);
        }
    } else {
        for step in 0..rows {
            let source = (src_row + step) * screen.pitch + src_column * 4;
            let destination = (dst_row + step) * screen.pitch + dst_column * 4;
            screen
                .shadow
                .copy_within(source..source + row_bytes, destination);
        }
    }
    Status::Ok
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

pub fn fill_rect(x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.fill(x, y, w, h, color);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

/// Draws a `pw × ph` RGB (three bytes per pixel, `0xRRGGBB`) buffer scaled into
/// the destination rectangle with nearest-neighbour sampling (spec 0009, G22).
/// The destination is clipped to the screen and the clip region like
/// [`fill_rect`]; a fully clipped rectangle is `Ok` with no writes. A
/// non-positive dimension, a buffer shorter than `pw * ph * 3` or disabled
/// graphics is `Invalid` and draws nothing.
pub fn blit_rgb(x: i64, y: i64, w: i64, h: i64, pixels: &[u8], pw: i64, ph: i64) -> Status {
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
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.blit(x, y, w, h, pixels, pw, ph);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

pub fn fill_dither(x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.dither(x, y, w, h, color);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

pub fn draw_rect(x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
    if !valid_rect(w, h, color) {
        return Status::Invalid;
    }
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.outline(x, y, w, h, color);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

pub fn draw_text(x: i64, y: i64, text: &[u8], color: u32) -> Status {
    if color > MAX_COLOR {
        return Status::Invalid;
    }
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.text(x, y, text, color);
            Status::Ok
        }
        None => Status::Invalid,
    }
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
    match SCREEN.lock().as_mut() {
        Some(screen) => {
            screen.text_prop(x, y, text, color);
            Status::Ok
        }
        None => Status::Invalid,
    }
}

pub fn get_pixel(x: i64, y: i64) -> u32 {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return 0;
    };
    if x < 0 || y < 0 || x >= screen.width as i64 || y >= screen.height as i64 {
        return 0;
    }
    screen.load(x as usize, y as usize)
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

/// Fills the whole screen and presents it (spec 0009, G6). This is a screen
/// operation, not a clipped primitive: it ignores the clip region.
pub fn clear(color: u32) -> Status {
    if color > MAX_COLOR {
        return Status::Invalid;
    }
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return Status::Invalid;
    };
    screen.fill(0, 0, screen.width as i64, screen.height as i64, color);
    screen.present(0, 0, screen.width as i64, screen.height as i64);
    Status::Ok
}

// ---- explicit-clip text drawing (kernel text windows, spec 0012) ---------

/// Paints one 8×8 glyph cell with an opaque background through an explicit
/// clip, without presenting and without touching the shadow's global clip
/// (spec 0012, D3). Kernel text windows use this so a long repaint cannot clip
/// an application drawing on another core.
pub fn draw_cell(x: i64, y: i64, code: u8, fg: u32, bg: u32, clip: Clip) -> bool {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    screen.glyph_in(x, y, code, fg, bg, clip);
    true
}

/// Presents a rectangle through an explicit clip, ignoring the global clip
/// (spec 0012, D3). `present` composites the cursor as usual.
pub fn present_within(x: i64, y: i64, w: i64, h: i64, clip: Clip) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    screen.present_in(x, y, w, h, clip);
    true
}

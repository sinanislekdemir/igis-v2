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

use abi::Status;

use crate::font8x8::FONT8X8_BASIC;
use crate::frame;
use crate::mb2::Framebuffer;
use crate::sync::SpinLock;

/// Largest accepted colour value: the top byte of `0x00RRGGBB` must be zero.
pub const MAX_COLOR: u32 = 0x00FF_FFFF;

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

/// The screen image and the geometry needed to present it.
struct Screen {
    width: usize,
    height: usize,
    /// Bytes per row in both the shadow and the framebuffer.
    pitch: usize,
    /// Physical (identity-mapped) framebuffer base.
    fb: u64,
    /// `pitch * height` bytes owned by the screen for the system's lifetime.
    shadow: &'static mut [u8],
    /// Rectangular clip, intersected with the screen (spec 0009, G7).
    clip: Rect,
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

    /// Copies a clipped rectangle from the shadow to the framebuffer, one row
    /// at a time, using the pitch (spec 0009, G5).
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
    }

    /// Moves the image up by `pixels` scan lines and fills the exposed band.
    fn scroll(&mut self, pixels: usize, color: u32) {
        if pixels == 0 || pixels >= self.height {
            self.fill(0, 0, self.width as i64, self.height as i64, color);
            return;
        }
        let row_bytes = self.pitch;
        let offset = pixels * row_bytes;
        let bytes = (self.height - pixels) * row_bytes;
        let base = self.shadow.as_mut_ptr();
        // SAFETY: `offset + bytes == pitch * height`, the shadow's own length;
        // the ranges overlap, so `copy` (memmove) is required.
        unsafe { core::ptr::copy(base.add(offset), base, bytes) };
        self.fill(
            0,
            (self.height - pixels) as i64,
            self.width as i64,
            pixels as i64,
            color,
        );
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
        clip: Rect {
            x: 0,
            y: 0,
            w: width as i32,
            h: height as i32,
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

/// Screen size in pixels, `(0, 0)` when graphics are disabled.
pub fn size() -> (u32, u32) {
    match &*SCREEN.lock() {
        Some(screen) => (screen.width as u32, screen.height as u32),
        None => (0, 0),
    }
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

// ---- console path -------------------------------------------------------

/// Paints one scaled 8x8 glyph cell with an opaque background and presents the
/// cell. The text console uses this so it draws through the shadow too
/// (spec 0009, G14).
pub fn console_glyph(x: usize, y: usize, code: u8, fg: u32, bg: u32, scale: usize) -> bool {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    let cell = (8 * scale) as i64;
    screen.fill(x as i64, y as i64, cell, cell, bg);
    if code < 0x80 {
        let glyph = &FONT8X8_BASIC[code as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << col) != 0 {
                    let px = x as i64 + (col * scale) as i64;
                    let py = y as i64 + (row * scale) as i64;
                    screen.fill(px, py, scale as i64, scale as i64, fg);
                }
            }
        }
    }
    screen.present(x as i64, y as i64, cell, cell);
    true
}

/// Scrolls the text image up by `pixels` rows and presents the screen.
pub fn console_scroll(pixels: usize, color: u32) -> bool {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    screen.scroll(pixels, color);
    let (width, height) = (screen.width as i64, screen.height as i64);
    screen.present(0, 0, width, height);
    true
}

/// Clears the text screen and presents it.
pub fn console_clear(color: u32) -> bool {
    let mut guard = SCREEN.lock();
    let Some(screen) = guard.as_mut() else {
        return false;
    };
    let (width, height) = (screen.width as i64, screen.height as i64);
    screen.fill(0, 0, width, height, color);
    screen.present(0, 0, width, height);
    true
}

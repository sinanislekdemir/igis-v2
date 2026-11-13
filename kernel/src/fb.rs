//! Linear framebuffer drawing primitives.

use crate::mb2::Framebuffer;

impl Framebuffer {
    pub fn usable(&self) -> bool {
        self.bpp == 32 && self.kind == 1
    }

    fn pixel(&self, x: usize, y: usize) -> *mut u32 {
        (self.addr + (y * self.pitch as usize + x * 4) as u64) as *mut u32
    }

    pub fn fill_rect(&self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let mut row = y;
        while row < y + h && row < self.height as usize {
            let mut col = x;
            while col < x + w && col < self.width as usize {
                // SAFETY: both bounds are checked against the tag geometry.
                unsafe { core::ptr::write_volatile(self.pixel(col, row), color) };
                col += 1;
            }
            row += 1;
        }
    }

    pub fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width as usize, self.height as usize, color);
    }

    /// Move the image up by `pixels` scan lines and clear the exposed band.
    pub fn scroll_up(&self, pixels: usize, color: u32) {
        let pitch = self.pitch as usize;
        let height = self.height as usize;
        if pixels == 0 || pixels >= height {
            self.clear(color);
            return;
        }
        let bytes = (height - pixels) * pitch;
        // SAFETY: both ranges lie inside the framebuffer; `copy` tolerates overlap.
        unsafe {
            core::ptr::copy(
                (self.addr + (pixels * pitch) as u64) as *const u8,
                self.addr as *mut u8,
                bytes,
            );
        }
        self.fill_rect(0, height - pixels, self.width as usize, pixels, color);
    }
}

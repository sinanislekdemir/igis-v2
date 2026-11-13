//! Linear framebuffer. In the first slice it only proves the tag and the
//! mapping work; the bitmap-font console comes next.

use crate::mb2::Framebuffer;

/// Clear the screen to a dark background. Returns false for a pixel format the
/// first slice does not support.
pub fn clear(fb: &Framebuffer) -> bool {
    if fb.bpp != 32 || fb.kind != 1 {
        return false;
    }

    // 0x00RRGGBB, little-endian in memory as B, G, R, 0.
    let color: u32 = 0x0010_1818;

    let mut y = 0u32;
    while y < fb.height {
        let row = (fb.addr + u64::from(y) * u64::from(fb.pitch)) as *mut u32;
        let mut x = 0u32;
        while x < fb.width {
            // SAFETY: the multiboot2 framebuffer tag describes a mapped range
            // of `pitch * height` bytes; x < width and bpp == 32.
            unsafe { core::ptr::write_volatile(row.add(x as usize), color) };
            x += 1;
        }
        y += 1;
    }

    true
}

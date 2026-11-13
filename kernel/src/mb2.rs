//! Minimal multiboot2 tag walking.
//!
//! The structure lives in identity-mapped physical memory and is owned by the
//! bootloader, so these reads are `unsafe` and unaligned-tolerant.

#[derive(Clone, Copy)]
pub struct Framebuffer {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    /// 0 = indexed, 1 = RGB, 2 = EGA text.
    pub kind: u8,
}

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub base: u64,
    pub len: u64,
    pub kind: u32,
}

unsafe fn read_u8(addr: usize) -> u8 {
    core::ptr::read_unaligned(addr as *const u8)
}

unsafe fn read_u32(addr: usize) -> u32 {
    core::ptr::read_unaligned(addr as *const u32)
}

unsafe fn read_u64(addr: usize) -> u64 {
    core::ptr::read_unaligned(addr as *const u64)
}

fn each_tag(info: usize, mut visit: impl FnMut(u32, usize, usize)) {
    // SAFETY: `info` is the pointer GRUB placed in ebx and verified by the stub.
    let total_size = unsafe { read_u32(info) } as usize;
    let mut offset = 8usize;

    while offset + 8 <= total_size {
        let tag = info + offset;
        // SAFETY: within the structure according to `total_size`.
        let kind = unsafe { read_u32(tag) };
        // SAFETY: the size field is part of every tag header.
        let size = unsafe { read_u32(tag + 4) } as usize;

        if kind == 0 || size < 8 || offset + size > total_size {
            break;
        }

        visit(kind, tag, size);
        offset += (size + 7) & !7;
    }
}

pub fn framebuffer(info: usize) -> Option<Framebuffer> {
    let mut found = None;

    each_tag(info, |kind, tag, size| {
        if kind == 8 && size >= 32 && found.is_none() {
            // SAFETY: the framebuffer tag is at least 32 bytes when present.
            found = Some(unsafe {
                Framebuffer {
                    addr: read_u64(tag + 8),
                    pitch: read_u32(tag + 16),
                    width: read_u32(tag + 20),
                    height: read_u32(tag + 24),
                    bpp: read_u8(tag + 28),
                    kind: read_u8(tag + 29),
                }
            });
        }
    });

    found
}

pub fn memory_regions(info: usize, mut visit: impl FnMut(MemoryRegion)) {
    each_tag(info, |kind, tag, size| {
        if kind != 6 || size < 16 {
            return;
        }

        // SAFETY: the memory-map tag is at least 16 bytes when present.
        let entry_size = unsafe { read_u32(tag + 8) } as usize;
        if entry_size < 24 {
            return;
        }

        let mut offset = 16usize;
        while offset + entry_size <= size {
            let entry = tag + offset;
            // SAFETY: the entry lies inside the tag.
            visit(unsafe {
                MemoryRegion {
                    base: read_u64(entry),
                    len: read_u64(entry + 8),
                    kind: read_u32(entry + 16),
                }
            });
            offset += entry_size;
        }
    });
}

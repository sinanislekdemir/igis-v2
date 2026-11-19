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

pub const MODULE_TAG: u32 = 3;

/// The boot loader's command line (multiboot2 tag type 1).
const COMMAND_LINE_TAG: u32 = 1;

/// Visits the boot command line, when the boot loader supplied one.
///
/// Layout (multiboot2 3.6.9): `type`, `size`, then a UTF-8 string terminated by
/// a NUL; the NUL is not part of the argument passed to `visit`. GRUB passes
/// what follows the `multiboot2` keyword in the menu entry, which is how
/// `xtask run --net-irq poll` selects the polled network path without a second
/// kernel image.
pub fn for_each_command_line(info: usize, mut visit: impl FnMut(&[u8])) {
    each_tag(info, |kind, tag, size| {
        if kind != COMMAND_LINE_TAG || size < 9 {
            return;
        }
        // SAFETY: the string follows the 8-byte tag header and `size` bounds
        // the tag.
        let bytes = unsafe { core::slice::from_raw_parts((tag + 8) as *const u8, size - 8) };
        let Some(length) = bytes.iter().position(|byte| *byte == 0) else {
            return;
        };
        visit(&bytes[..length]);
    });
}

/// A bootloader module: a byte range in identity-mapped physical memory.
#[derive(Clone, Copy, Debug)]
pub struct Module {
    pub start: u64,
    pub end: u64,
}

impl Module {
    pub fn len(&self) -> usize {
        (self.end - self.start) as usize
    }
}

/// Visits every module tag with its inline command line.
///
/// Layout (multiboot2 3.6.6): `type`, `size`, `mod_start`, `mod_end`, then the
/// UTF-8 command line, null-terminated, in the same tag.
pub fn for_each_module(info: usize, mut visit: impl FnMut(Module, &[u8])) {
    each_tag(info, |kind, tag, size| {
        if kind != MODULE_TAG || size < 17 {
            return;
        }
        // SAFETY: the module tag has at least the 16 bytes read here.
        let start = u64::from(unsafe { read_u32(tag + 8) });
        // SAFETY: as above.
        let end = u64::from(unsafe { read_u32(tag + 12) });
        // SAFETY: the command line starts at byte 16 and `size` bounds the tag.
        let bytes = unsafe { core::slice::from_raw_parts((tag + 16) as *const u8, size - 16) };
        let Some(length) = bytes.iter().position(|byte| *byte == 0) else {
            return;
        };
        if start < end {
            visit(Module { start, end }, &bytes[..length]);
        }
    });
}

/// Finds the module whose command line is exactly `name`.
///
/// The range is validated against the memory map: it must be non-empty, lie
/// inside one available region and fit in `usize` arithmetic. A module that
/// fails any check is treated as absent rather than read.
pub fn module(info: usize, name: &str) -> Option<Module> {
    let mut found = None;

    for_each_module(info, |module, command| {
        if found.is_some() || command != name.as_bytes() {
            return;
        }

        let mut available = false;
        memory_regions(info, |region| {
            if region.kind == 1
                && region.base <= module.start
                && module.end <= region.base.saturating_add(region.len)
            {
                available = true;
            }
        });
        if available && module.end <= usize::MAX as u64 {
            found = Some(module);
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

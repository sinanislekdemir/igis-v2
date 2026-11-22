//! Physical frame allocator.
//!
//! One bitmap over the first 4 GiB, one bit per 4 KiB frame. The bitmap is
//! static: the allocator cannot allocate its own metadata. Every frame starts
//! used; `init` frees the ranges the bootloader reported as available, then
//! reserves the kernel image, the multiboot2 structure and the framebuffer.

use core::sync::atomic::{AtomicU64, Ordering};

use arch_x86_64::KERNEL_BASE;

use crate::mb2::{self, Framebuffer};
use crate::sync::SpinLock;

pub const FRAME_SIZE: u64 = 4096;
pub const MAX_PHYS: u64 = 4 * 1024 * 1024 * 1024;
const FRAME_COUNT: usize = (MAX_PHYS / FRAME_SIZE) as usize;
const WORDS: usize = FRAME_COUNT / 64;

static BITMAP: SpinLock<[u64; WORDS]> = SpinLock::new([u64::MAX; WORDS]);
static FREE_FRAMES: AtomicU64 = AtomicU64::new(0);
static NEXT_HINT: AtomicU64 = AtomicU64::new(0);

extern "C" {
    static _kernel_end: u8;
}

fn set_used(words: &mut [u64], frame: usize) {
    words[frame / 64] |= 1 << (frame % 64);
}

fn set_free(words: &mut [u64], frame: usize) {
    words[frame / 64] &= !(1 << (frame % 64));
}

fn is_used(words: &[u64], frame: usize) -> bool {
    words[frame / 64] & (1 << (frame % 64)) != 0
}

fn mark_used(words: &mut [u64], start: u64, end: u64) {
    let first = (start / FRAME_SIZE) as usize;
    let last = ((end + FRAME_SIZE - 1) / FRAME_SIZE) as usize;
    let last = last.min(FRAME_COUNT);
    for frame in first..last {
        set_used(words, frame);
    }
}

fn mark_free(words: &mut [u64], start: u64, end: u64) {
    let first = (start / FRAME_SIZE) as usize;
    let last = (end / FRAME_SIZE) as usize;
    let last = last.min(FRAME_COUNT);
    for frame in first..last {
        set_free(words, frame);
    }
}

pub fn init(info: usize, framebuffer: Option<Framebuffer>) {
    let mut guard = BITMAP.lock();
    let words: &mut [u64] = &mut guard[..];

    mb2::memory_regions(info, |region| {
        if region.kind == 1 {
            let start = region.base;
            let end = region.base.saturating_add(region.len).min(MAX_PHYS);
            if start < MAX_PHYS && end > start {
                mark_free(words, start, end);
            }
        }
    });

    // Low 1 MiB holds firmware data and the real-mode trampoline area.
    mark_used(words, 0, 0x10_0000);

    // The kernel image, from the load address to the end of .bss.
    // SAFETY: `_kernel_end` is a linker-defined symbol in the kernel image.
    let kernel_end = unsafe { &_kernel_end as *const u8 as u64 } - KERNEL_BASE;
    mark_used(words, 0x10_0000, kernel_end);

    // The multiboot2 structure is owned by the bootloader.
    // SAFETY: `info` points at the structure; the total size is its first word.
    let info_size = unsafe { core::ptr::read_unaligned(info as *const u32) } as u64;
    mark_used(words, info as u64, info as u64 + info_size);

    // Any bootloader module is outside the kernel image but must never be
    // handed out as frames: the bootloader owns it for the lifetime of the
    // system.
    mb2::for_each_module(info, |module, _command| {
        mark_used(words, module.start, module.end);
    });

    // The framebuffer is device memory even when the map calls its range free.
    if let Some(fb) = framebuffer {
        let end = fb.addr + u64::from(fb.pitch) * u64::from(fb.height);
        if fb.addr < MAX_PHYS {
            mark_used(words, fb.addr, end.min(MAX_PHYS));
        }
    }

    let mut free = 0u64;
    for frame in 0..FRAME_COUNT {
        if !is_used(words, frame) {
            free += 1;
        }
    }
    FREE_FRAMES.store(free, Ordering::Relaxed);
}

pub fn free_frames() -> u64 {
    FREE_FRAMES.load(Ordering::Relaxed)
}

pub fn total_frames() -> u64 {
    FRAME_COUNT as u64
}

fn first_free(words: &[u64], from: usize, to: usize) -> Option<usize> {
    (from..to).find(|&frame| !is_used(words, frame))
}

/// Allocate one frame and return its physical address.
pub fn alloc_frame() -> Option<u64> {
    let mut guard = BITMAP.lock();
    let words: &mut [u64] = &mut guard[..];
    let hint = NEXT_HINT.load(Ordering::Relaxed) as usize;

    let frame = first_free(words, hint, FRAME_COUNT).or_else(|| first_free(words, 0, hint))?;

    set_used(words, frame);
    NEXT_HINT.store((frame + 1) as u64, Ordering::Relaxed);
    FREE_FRAMES.fetch_sub(1, Ordering::Relaxed);
    Some(frame as u64 * FRAME_SIZE)
}

pub fn free_frame(addr: u64) {
    let frame = (addr / FRAME_SIZE) as usize;
    if frame >= FRAME_COUNT {
        return;
    }
    let mut guard = BITMAP.lock();
    let words: &mut [u64] = &mut guard[..];
    if is_used(words, frame) {
        set_free(words, frame);
        FREE_FRAMES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Marks `[start, end)` as device memory. Used for a linear framebuffer whose
/// full aperture is larger than the mode the bootloader happened to set.
pub fn reserve_range(start: u64, end: u64) {
    let start = start.min(MAX_PHYS);
    let end = end.min(MAX_PHYS);
    if end <= start {
        return;
    }
    let mut guard = BITMAP.lock();
    let words: &mut [u64] = &mut guard[..];
    let first = (start / FRAME_SIZE) as usize;
    let last = ((end + FRAME_SIZE - 1) / FRAME_SIZE).min(FRAME_COUNT as u64) as usize;
    for frame in first..last {
        if !is_used(words, frame) {
            set_used(words, frame);
            FREE_FRAMES.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Allocates `count` physically contiguous frames and returns the address of
/// the first. Task regions are contiguous so that a region is one slice.
///
/// Returns `None` when no run of that length is free. The scan starts at the
/// allocator hint and wraps once, but a run never wraps around the end of
/// physical memory.
pub fn alloc_contiguous(count: usize) -> Option<u64> {
    if count == 0 {
        return None;
    }
    let count = count.min(FRAME_COUNT);

    let mut guard = BITMAP.lock();
    let words: &mut [u64] = &mut guard[..];

    let last_start = FRAME_COUNT - count;
    let hint = NEXT_HINT.load(Ordering::Relaxed) as usize % FRAME_COUNT;
    let hint = hint.min(last_start);

    let first = first_run(words, hint, last_start, count).or_else(|| {
        if hint > 0 {
            first_run(words, 0, hint - 1, count)
        } else {
            None
        }
    })?;

    for frame in first..first + count {
        set_used(words, frame);
    }
    NEXT_HINT.store(((first + count) % FRAME_COUNT) as u64, Ordering::Relaxed);
    FREE_FRAMES.fetch_sub(count as u64, Ordering::Relaxed);
    Some(first as u64 * FRAME_SIZE)
}

/// Releases `count` frames starting at `addr`.
pub fn free_contiguous(addr: u64, count: usize) {
    for index in 0..count {
        free_frame(addr + index as u64 * FRAME_SIZE);
    }
}

/// First run of `count` free frames whose first frame is in `first..=last`.
fn first_run(words: &[u64], first: usize, last: usize, count: usize) -> Option<usize> {
    if last < first {
        return None;
    }
    'start: for start in first..=last {
        for frame in start..start + count {
            if is_used(words, frame) {
                continue 'start;
            }
        }
        return Some(start);
    }
    None
}

//! Runtime display modes (spec 0022).
//!
//! The mode list is a fixed set of presets accepted by the Bochs VBE dispi.
//! A change allocates the new shadow first, programs the device while the
//! graphics lock is held, swaps the screen, then frees the old shadow and asks
//! the window service for a full repaint. A failed device request leaves the
//! old screen untouched.

use abi::Status;
use arch_x86_64::bochs;

use crate::mb2::Framebuffer;
use crate::sync::SpinLock;
use crate::{console, frame, gfx, window};

/// Serialises mode changes: two tasks may race to call `set_resolution`, but
/// the device and the one shadow are changed by one caller at a time.
static CHANGE: SpinLock<()> = SpinLock::new(());

fn preset(index: u64) -> Option<(u16, u16)> {
    bochs::PRESETS.get(index as usize).copied()
}

/// Number of selectable modes; `0` when the runtime VBE interface is absent.
pub fn mode_count() -> u64 {
    if bochs::available() {
        bochs::PRESETS.len() as u64
    } else {
        0
    }
}

/// Width of mode `index`, or `0` when the index is out of range.
pub fn mode_width(index: u64) -> u64 {
    preset(index).map_or(0, |(width, _)| u64::from(width))
}

/// Height of mode `index`, or `0` when the index is out of range.
pub fn mode_height(index: u64) -> u64 {
    preset(index).map_or(0, |(_, height)| u64::from(height))
}

fn offered(width: u16, height: u16) -> bool {
    bochs::PRESETS
        .iter()
        .any(|&(w, h)| w == width && h == height)
}

/// Changes the display to `width`x`height` at 32 bpp.
///
/// The new shadow is allocated before the device is touched, so a memory
/// failure cannot leave the machine in a mode the kernel cannot draw into.
/// On success the window service scales and clamps every window origin, marks
/// every live window for an `Expose` and repaints the whole stack.
pub fn set_resolution(width: u64, height: u64) -> Status {
    let Ok(w) = u16::try_from(width) else {
        return Status::Invalid;
    };
    let Ok(h) = u16::try_from(height) else {
        return Status::Invalid;
    };
    if w == 0 || h == 0 || !bochs::available() || !offered(w, h) {
        return if bochs::available() {
            Status::NotFound
        } else {
            Status::Invalid
        };
    }
    let (old_width, old_height) = gfx::size();
    if old_width == 0 {
        return Status::Invalid;
    }
    let lfb = gfx::framebuffer_addr();
    if lfb == 0 {
        return Status::Invalid;
    }

    let pitch = u32::from(w) * 4;
    let Some(bytes) = u64::from(pitch).checked_mul(u64::from(h)) else {
        return Status::Invalid;
    };
    let frames = bytes.div_ceil(frame::FRAME_SIZE) as usize;
    let Some(base) = frame::alloc_contiguous(frames) else {
        return Status::Full;
    };
    // SAFETY: the frames were just reserved contiguously and are identity
    // mapped below 4 GiB. The slice is not published until `change_mode`.
    unsafe {
        core::ptr::write_bytes(base as *mut u8, 0, bytes as usize);
    }

    let info = Framebuffer {
        addr: lfb,
        pitch,
        width: u32::from(w),
        height: u32::from(h),
        bpp: 32,
        kind: 1,
    };
    let _guard = CHANGE.lock();
    let changed = gfx::change_mode(info, base, frames, || {
        bochs::set_mode(w, h, lfb).is_some()
    });
    match changed {
        Some((old_base, old_frames)) => {
            frame::free_contiguous(old_base, old_frames);
            // The old framebuffer image is gone; rebuild the window stack.
            window::screen_changed(old_width, old_height);
            // The System Log is sized from the screen, not from its owner;
            // reopen it so a smaller mode cannot leave it covering the desktop.
            console::screen_changed();
            Status::Ok
        }
        None => {
            frame::free_contiguous(base, frames);
            Status::IoError
        }
    }
}

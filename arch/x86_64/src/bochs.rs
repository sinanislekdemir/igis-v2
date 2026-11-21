//! Bochs VBE extensions (the QEMU `std` VGA display interface).
//!
//! QEMU's standard VGA exposes the Bochs VBE register pair at I/O ports
//! `0x1CE` (index) and `0x1CF` (data). Writing the horizontal resolution,
//! vertical resolution, colour depth and virtual width, then enabling the
//! linear framebuffer, changes the real display mode without a BIOS call.
//! This is the runtime mode path used by the settings application; the VBE
//! BIOS `int 10h` path is not used because the kernel runs with paging on and
//! a real-mode trampoline is not needed for the QEMU target.
//!
//! Only 32 bpp direct-colour modes are accepted, matching the graphics module.

use crate::port::{inw, outw};

const INDEX: u16 = 0x1CE;
const DATA: u16 = 0x1CF;

const ID: u16 = 0x00;
const XRES: u16 = 0x01;
const YRES: u16 = 0x02;
const BPP: u16 = 0x03;
const ENABLE: u16 = 0x04;
const VIRT_WIDTH: u16 = 0x06;
const VIRT_HEIGHT: u16 = 0x07;

const ENABLED: u16 = 0x01;
const LFB_ENABLED: u16 = 0x40;
const NOCLEARMEM: u16 = 0x80;

const ID_MIN: u16 = 0xB0C0;
const ID_MAX: u16 = 0xB0C5;

/// Presets offered by the settings application. They are all within the
/// QEMU std VGA limits (2560x1600); a mode that the device rejects is reported
/// as unsupported rather than assumed.
pub const PRESETS: &[(u16, u16)] = &[
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 1024),
    (1440, 900),
    (1600, 1200),
    (1920, 1080),
];

fn write(index: u16, value: u16) {
    // SAFETY: the Bochs dispi index/data ports are fixed VGA device registers;
    // the caller runs at ring 0 with no competing user of the pair.
    unsafe {
        outw(INDEX, index);
        outw(DATA, value);
    }
}

fn read(index: u16) -> u16 {
    // SAFETY: reading the dispi data port has no side effects beyond selecting
    // the register; access is serialised by the display module.
    unsafe {
        outw(INDEX, index);
        inw(DATA)
    }
}

/// True when the Bochs VBE dispi is present at the standard port pair.
pub fn available() -> bool {
    (ID_MIN..=ID_MAX).contains(&read(ID))
}

/// The dispi interface revision (`0xB0C0`..`0xB0C5`), or 0 when absent.
pub fn revision() -> u16 {
    read(ID)
}

/// Sets a 32 bpp linear mode and returns its byte pitch and framebuffer base.
///
/// `lfb` is the linear framebuffer address from the bootloader: the Bochs
/// dispi changes the mode but not the PCI BAR, so this address stays valid.
/// Returns `None` when the device does not accept the requested geometry.
pub fn set_mode(width: u16, height: u16, lfb: u64) -> Option<Mode> {
    if !available() || width == 0 || height == 0 || lfb == 0 {
        return None;
    }
    write(XRES, width);
    write(YRES, height);
    write(BPP, 32);
    write(VIRT_WIDTH, width);
    write(VIRT_HEIGHT, height);
    write(ENABLE, ENABLED | LFB_ENABLED | NOCLEARMEM);

    // Read the values back: QEMU silently ignores an out-of-range request, so
    // a successful call is one whose geometry the device reports.
    if read(XRES) != width || read(YRES) != height || read(BPP) != 32 {
        return None;
    }
    let pitch = u32::from(width).checked_mul(4)?;
    Some(Mode {
        width,
        height,
        pitch,
        lfb,
    })
}

/// One applied mode.
#[derive(Clone, Copy, Debug)]
pub struct Mode {
    pub width: u16,
    pub height: u16,
    pub pitch: u32,
    pub lfb: u64,
}

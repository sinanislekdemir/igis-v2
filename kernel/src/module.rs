//! Bootloader modules: the source-only ext2 application image arrives as a
//! GRUB module (ADR 0015, ADR 0019).
//!
//! The module is a range of identity-mapped physical memory owned by the
//! bootloader. Nothing in the kernel frees or reuses it, so handing out a
//! `'static` byte slice is sound and keeps the block device allocation-free.

use crate::mb2;

/// A located module.
#[derive(Clone, Copy, Debug)]
pub struct Module {
    start: usize,
    len: usize,
}

impl Module {
    /// Start address in physical (identity-mapped) memory.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// The module's bytes.
    pub fn bytes(&self) -> &'static [u8] {
        // SAFETY: the multiboot2 module range was validated against the
        // memory map by `mb2::module`. GRUB owns it for the lifetime of the
        // system: the kernel never frees or remaps it.
        unsafe { core::slice::from_raw_parts(self.start as *const u8, self.len) }
    }
}

/// Finds the module whose command line is exactly `name`.
pub fn find(info: usize, name: &str) -> Option<Module> {
    let range = mb2::module(info, name)?;
    Some(Module {
        start: range.start as usize,
        len: range.len(),
    })
}

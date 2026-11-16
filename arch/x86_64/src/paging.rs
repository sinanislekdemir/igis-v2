//! 4 KiB paging primitives over the boot page tables (ADR 0006, spec 0002 H2).
//!
//! `boot.s` identity-maps the first 4 GiB with 2 MiB pages (`pd0..pd3`, one
//! page directory per GiB) and aliases the first GiB at
//! [`KERNEL_BASE`](crate::KERNEL_BASE) through `pdpt1[510] -> pd0`. A guard page
//! needs a not-present 4 KiB entry, so the 2 MiB page that contains it is split
//! once into a 4 KiB table; from then on the kernel owns individual entries.
//!
//! The higher-half alias is *shared*, not duplicated: below 1 GiB both the
//! identity view and the `KERNEL_BASE + physical` view use the same `pd0` entry
//! and therefore the same 4 KiB table. A guard below 1 GiB is absent in both
//! views; above 1 GiB only the identity view exists at all.
//!
//! Everything here is deliberately small and unsafe: the callers
//! (`kernel::guard`) own the table cache, the registry and the cross-core TLB
//! shootdown. These functions only read and write the paging structures.
//!
//! All page tables used by callers live in the kernel image (`.bss`), so their
//! physical addresses translate to a writable virtual address with
//! `physical + KERNEL_BASE`. The boot tables themselves are linked at their
//! physical address and are reachable through the identity map.

use core::arch::asm;
use core::sync::atomic::{fence, Ordering};

use crate::KERNEL_BASE;

/// Largest physical address the boot page tables map (exclusive): 4 GiB.
pub const MAX_PHYS: u64 = 4 * 1024 * 1024 * 1024;

/// One 4 KiB page.
pub const PAGE_SIZE: u64 = 4096;

/// One 2 MiB huge page.
pub const HUGE_SIZE: u64 = 2 * 1024 * 1024;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
/// Page-level write-through (PWT).
const WRITE_THROUGH: u64 = 1 << 3;
/// Page-level cache disable (PCD).
const CACHE_DISABLE: u64 = 1 << 4;
const PAGE_SIZE_BIT: u64 = 1 << 7;
/// Physical-address bits of a page-table entry (12..51).
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

extern "C" {
    /// `pd0` from `boot.s`: the first page directory, identity-mapping the
    /// first GiB with 2 MiB pages. `pd1..pd3` follow it contiguously, one per
    /// GiB, and the whole set is linked at its physical address.
    static pd0: u8;
}

/// The directory entry that covers `base`, or `None` above the identity map.
fn directory_entry(base: u64) -> Option<*mut u64> {
    if base >= MAX_PHYS {
        return None;
    }
    // SAFETY: `pd0` is the first of four page directories in `.boot`; the
    // index is bounded by the 4 GiB identity map.
    let pd0_phys = core::ptr::addr_of!(pd0) as u64;
    let directory = (pd0_phys + (base >> 30) * PAGE_SIZE) as *mut u64;
    Some(unsafe { directory.add(((base >> 21) & 511) as usize) })
}

/// The physical address of the 4 KiB table covering the 2 MiB page at `base`,
/// or `None` when the entry is still a huge page (or absent).
pub fn table_phys(base: u64) -> Option<u64> {
    let entry = directory_entry(base)?;
    // SAFETY: `entry` points inside a page directory in `.boot`.
    let value = unsafe { core::ptr::read_volatile(entry) };
    if value & PRESENT == 0 || value & PAGE_SIZE_BIT != 0 {
        return None;
    }
    Some(value & ADDR_MASK)
}

/// Splits the 2 MiB page at `base` into 4 KiB entries in `table_phys`.
///
/// Every entry in the new table is present and writable for the same 2 MiB
/// range the huge page covered; the caller clears the guard entry afterwards.
/// `table_phys` must be the physical address of a 4 KiB-aligned table in the
/// kernel image (`.bss`) that is not in use for anything else.
///
/// Returns `false` when `base` is not a 2 MiB-aligned address inside the
/// identity map, or when the directory entry is not currently a present huge
/// page (already split, or absent). Splitting is deliberately idempotent-free:
/// the kernel's guard table cache guarantees each 2 MiB page is split once.
pub unsafe fn split_huge_page(base: u64, table_phys: u64) -> bool {
    if base & (HUGE_SIZE - 1) != 0 || table_phys & (PAGE_SIZE - 1) != 0 {
        return false;
    }
    let Some(entry) = directory_entry(base) else {
        return false;
    };
    // SAFETY: `entry` points inside a page directory in `.boot`.
    let value = core::ptr::read_volatile(entry);
    if value & PRESENT == 0 || value & PAGE_SIZE_BIT == 0 {
        return false;
    }

    // SAFETY: `table_phys` names a table reserved by the caller; the kernel
    // image is mapped through the higher-half alias.
    let table = (table_phys + KERNEL_BASE) as *mut u64;
    for index in 0..512u64 {
        // SAFETY: `index` is inside the table.
        core::ptr::write_volatile(
            table.add(index as usize),
            (base + index * PAGE_SIZE) | PRESENT | WRITABLE,
        );
    }
    // The entries must be visible before the directory entry points at them.
    fence(Ordering::Release);
    // SAFETY: same as above; this switches the range from 2 MiB to 4 KiB
    // mappings. The tables are shared through the higher-half alias.
    core::ptr::write_volatile(entry, table_phys | PRESENT | WRITABLE);
    fence(Ordering::Release);
    true
}

/// Sets or clears the present bit of the 4 KiB entry for `base + index * 4096`.
///
/// `base` must be a 2 MiB-aligned address whose directory entry already points
/// at a 4 KiB table. Other bits (write permission, address) are preserved.
/// Returns `false` when there is no such table or `index` is out of range.
pub fn set_pte_present(base: u64, index: usize, present: bool) -> bool {
    if index >= 512 {
        return false;
    }
    let Some(physical) = table_phys(base) else {
        return false;
    };
    // SAFETY: the table was split by `split_huge_page`, lives in the kernel
    // image and is mapped through the higher-half alias; `index` is inside it.
    unsafe {
        let entry = ((physical + KERNEL_BASE) as *mut u64).add(index);
        let value = core::ptr::read_volatile(entry);
        let updated = if present {
            value | PRESENT
        } else {
            value & !PRESENT
        };
        core::ptr::write_volatile(entry, updated);
    }
    fence(Ordering::Release);
    true
}

/// Marks the 4 KiB entry covering `addr` uncacheable (`PWT | PCD`).
///
/// Device memory behind a write-back mapping can return stale reads or lose
/// writes on real hardware, so an MMIO page gets an uncached mapping before it
/// is touched. `addr` must lie in a 2 MiB page whose directory entry already
/// points at a 4 KiB table (split it with [`split_huge_page`] first). Other
/// bits (present, write permission, address) are preserved. The caller flushes
/// the TLB (or reloads CR3) afterwards.
///
/// Returns `false` when the range is not mapped as a 4 KiB table.
pub fn set_pte_uncached(addr: u64) -> bool {
    let base = addr & !(HUGE_SIZE - 1);
    let Some(physical) = table_phys(base) else {
        return false;
    };
    let index = ((addr & (HUGE_SIZE - 1)) / PAGE_SIZE) as usize;
    // SAFETY: the table was split by `split_huge_page`, lives in the kernel
    // image and is mapped through the higher-half alias; `index` is inside it.
    unsafe {
        let entry = ((physical + KERNEL_BASE) as *mut u64).add(index);
        let value = core::ptr::read_volatile(entry);
        core::ptr::write_volatile(entry, value | WRITE_THROUGH | CACHE_DISABLE);
    }
    fence(Ordering::Release);
    true
}

/// Reads back whether `addr` is mapped.
///
/// `None` means the address is outside the identity map or its directory entry
/// is not present. `Some(true)` covers both a 4 KiB present entry and a whole
/// 2 MiB huge page; `Some(false)` is a 4 KiB entry with the present bit clear,
/// which is what a guard page looks like.
pub fn pte_present(addr: u64) -> Option<bool> {
    let base = addr & !(HUGE_SIZE - 1);
    let entry = directory_entry(base)?;
    // SAFETY: `entry` points inside a page directory in `.boot`.
    let value = unsafe { core::ptr::read_volatile(entry) };
    if value & PRESENT == 0 {
        return None;
    }
    if value & PAGE_SIZE_BIT != 0 {
        return Some(true);
    }
    let physical = value & ADDR_MASK;
    let index = ((addr & (HUGE_SIZE - 1)) / PAGE_SIZE) as usize;
    // SAFETY: the directory entry names a 4 KiB table in the kernel image.
    let entry = unsafe { ((physical + KERNEL_BASE) as *const u64).add(index) };
    // SAFETY: `index` is inside the table.
    Some(unsafe { core::ptr::read_volatile(entry) } & PRESENT != 0)
}

/// Invalidates this core's TLB entry for `addr` (`invlpg`).
pub fn flush_page(addr: u64) {
    // SAFETY: `invlpg` is legal at ring 0; an address that is not mapped is
    // still a valid argument (it flushes any stale entry for it).
    unsafe { asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags)) };
}

/// Flushes this core's TLB and paging-structure caches by reloading CR3.
///
/// A reload is the portable whole-TLB flush: `invlpg` only names one linear
/// address, and a guard can be reached through the identity and higher-half
/// aliases.
pub fn reload_cr3() {
    let cr3: u64;
    // SAFETY: reading and rewriting CR3 with the same value is legal at ring 0.
    // The second asm has no `nomem`, so it is also a compiler barrier for the
    // page-table writes that preceded it.
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }
}

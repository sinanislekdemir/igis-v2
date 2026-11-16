//! The I/O APIC, used for one thing in this milestone: routing the PS/2
//! auxiliary interrupt (IRQ12) to the boot core (spec 0010, ADR 0023).
//!
//! The legacy 8259 PICs are masked at boot (`apic::init`), so an ISA IRQ only
//! reaches a core through the I/O APIC. Its registers are MMIO at the
//! conventional [`BASE`]: [`IOREGSEL`] selects a 32-bit register and
//! [`IOWIN`] reads or writes it.
//!
//! `boot.s` identity-maps the first 4 GiB with 2 MiB pages, so the register
//! page is mapped, but as *cacheable* memory. A device mapping must be
//! uncacheable on real hardware, so [`init`] splits the containing 2 MiB page
//! once, marks the 4 KiB register page `PWT | PCD`, and reloads CR3. This runs
//! before the APs start, so no other core can hold a stale translation.
//!
//! The redirection entry is programmed **level-triggered**. The i8042 keeps its
//! interrupt line high while its output buffer holds another byte; with an
//! edge-triggered entry a handler that drains less than everything would leave
//! the line high with no new edge and stall until the next unrelated event.
//! Level triggering makes the IOAPIC re-deliver after the EOI whenever the line
//! is still asserted, so the handler can bound its drain without losing bytes.
//!
//! An absent or implausible IOAPIC is `None`, never a hang: the caller keeps
//! polling the controller and the system continues.

use core::ptr::{addr_of, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::{paging, KERNEL_BASE};

/// The I/O APIC register window (IOAPIC, then the local APIC 2 MiB above it).
pub const BASE: u64 = 0xFEC0_0000;

/// The 2 MiB identity-mapped page that contains [`BASE`].
const PAGE_BASE: u64 = 0xFEC0_0000;

const IOREGSEL: u32 = 0x00;
const IOWIN: u32 = 0x10;

const REG_VERSION: u32 = 0x01;
const REG_REDIRECTION: u32 = 0x10;

/// Fixed delivery (bits 10:8 = 0), physical destination (bit 11 = 0),
/// active-high (bit 13 = 0), level-triggered (bit 15 = 1), unmasked (bit 16 = 0).
const REDIR_LEVEL: u32 = 1 << 15;

/// One 4 KiB page table for the split, in `.bss`. The IOAPIC is programmed once
/// at boot, so one table is enough and the guard cache is untouched.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut TABLE: PageTable = PageTable([0; 512]);

fn table_physical() -> u64 {
    // SAFETY: `addr_of!` of a static is safe; the kernel image is aliased high,
    // so the physical address is the virtual one minus the kernel base.
    addr_of!(TABLE) as u64 - KERNEL_BASE
}

fn select(register: u32) {
    // SAFETY: IOREGSEL is a 32-bit write-only register in the mapped window.
    unsafe { write_volatile((BASE + u64::from(IOREGSEL)) as *mut u32, register) }
}

fn read(register: u32) -> u32 {
    select(register);
    // SAFETY: IOWIN returns the register selected above.
    unsafe { read_volatile((BASE + u64::from(IOWIN)) as *const u32) }
}

fn write(register: u32, value: u32) {
    select(register);
    // SAFETY: as above; IOWIN is the selected register's data port.
    unsafe { write_volatile((BASE + u64::from(IOWIN)) as *mut u32, value) }
}

/// Maps the register page uncached, then routes `irq` to `vector` on the core
/// with APIC id `destination`.
///
/// Returns the IOAPIC version register on success (the boot log prints the
/// version byte), or `None` when the page cannot be remapped or the version
/// register does not look like an IOAPIC (all ones, zero, or fewer redirection
/// entries than `irq` needs). The caller then keeps polling.
pub fn init(irq: u8, vector: u8, destination: u32) -> Option<u32> {
    // Split the 2 MiB page once so the register page can get PWT|PCD.
    if paging::table_phys(PAGE_BASE).is_none() {
        // SAFETY: the table is a static 4 KiB-aligned page in `.bss`, unused by
        // anything else, and the page is the identity-mapped IOAPIC window.
        if !unsafe { paging::split_huge_page(PAGE_BASE, table_physical()) } {
            return None;
        }
    }
    if !paging::set_pte_uncached(BASE) {
        return None;
    }
    paging::flush_page(BASE);
    paging::reload_cr3();
    fence(Ordering::SeqCst);

    let version = read(REG_VERSION);
    let version_id = version & 0xFF;
    // The redirection-entry count lives in bits 23:16, encoded as `count - 1`.
    let redirections = (version >> 16) & 0xFF;
    if version == u32::MAX || version == 0 || version_id == 0 || redirections < u32::from(irq) {
        return None;
    }

    let entry = REG_REDIRECTION + 2 * u32::from(irq);
    // Destination first, with the entry masked while it is being written, so a
    // half-programmed entry can never deliver an interrupt.
    write(entry + 1, destination << 24);
    write(entry, u32::from(vector) | (1 << 16));
    write(entry, u32::from(vector) | REDIR_LEVEL);
    // The entry is visible before the controller can raise the line.
    fence(Ordering::SeqCst);
    Some(version)
}

//! PCI configuration-space scan (spec 0010, "Absolute pointer"; ADR 0022).
//!
//! x86-specific for now: configuration space is reached through the
//! 0xCF8/0xCFC port pair, and the UHCI I/O BAR is an x86 I/O port range. The
//! scan is bounded and allocation-free: bus 0, slots 0..31, functions 0..7, one
//! log line per function found. Its purpose is to locate the UHCI host
//! controller the USB tablet hangs on; the rest of the scan is boot evidence.
//!
//! ADR 0013 says machine discovery belongs behind `arch`/`platform`. This
//! module and [`crate::usb`] are the exception for the M2.2 slice: there is one
//! machine and one ISA, and moving them is a mechanical change when a second
//! machine exists (ADR 0022 records the debt).

use arch_x86_64::port::{inl, outl, outw};

use crate::println;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

/// Base class and subclass of a USB host controller (`0C.03`).
const CLASS_SERIAL_BUS: u8 = 0x0C;
const SUBCLASS_USB: u8 = 0x03;
/// Prog-if 0 is UHCI (prog-if 0x10 is OHCI, 0x20 EHCI, 0x30 xHCI).
const PROG_IF_UHCI: u8 = 0x00;

/// Command-register bits: I/O space and bus mastering.
const COMMAND_IO_SPACE: u16 = 1 << 0;
const COMMAND_BUS_MASTER: u16 = 1 << 2;

/// The UHCI controller the tablet driver drives.
#[derive(Clone, Copy)]
pub struct UsbController {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor: u16,
    pub device: u16,
    /// Base of the 32-byte UHCI I/O region (BAR4).
    pub io_base: u16,
}

fn address(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000
        | (u32::from(bus) << 16)
        | (u32::from(slot) << 11)
        | (u32::from(function) << 8)
        | u32::from(offset & 0xFC)
}

fn read_u32(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    // SAFETY: configuration space is accessed through the standard x86 pair;
    // the address is a pure value and reading the data port has no side effect
    // beyond the addressed configuration dword.
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, slot, function, offset));
        inl(CONFIG_DATA)
    }
}

fn read_u16(bus: u8, slot: u8, function: u8, offset: u8) -> u16 {
    let value = read_u32(bus, slot, function, offset & !3);
    (value >> ((offset & 2) * 8)) as u16
}

fn write_u16(bus: u8, slot: u8, function: u8, offset: u8, value: u16) {
    // A 16-bit access to the data port writes exactly the low or high half of
    // the addressed dword; the status half of the command/status register is
    // never touched.
    // SAFETY: as above; the data port supports 8-, 16- and 32-bit accesses and
    // the offset selects the half.
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, slot, function, offset & !3));
        outw(CONFIG_DATA + u16::from(offset & 2), value);
    }
}

/// Scans bus 0 and returns the first UHCI controller.
///
/// The first matching function has its I/O space and bus-mastering bits
/// enabled and BAR4 is taken as the I/O base. Everything else is logged and
/// ignored. No allocation, no dynamic tables, and a fixed 32 * 8 bound.
pub fn init() -> Option<UsbController> {
    let mut found = None;

    for slot in 0..32u8 {
        for function in 0..8u8 {
            let id = read_u32(0, slot, function, 0x00);
            let vendor = (id & 0xFFFF) as u16;
            if vendor == 0xFFFF {
                continue;
            }
            let device = (id >> 16) as u16;

            let class = read_u32(0, slot, function, 0x08);
            let revision = (class & 0xFF) as u8;
            let prog_if = ((class >> 8) & 0xFF) as u8;
            let subclass = ((class >> 16) & 0xFF) as u8;
            let base_class = ((class >> 24) & 0xFF) as u8;

            let bars = [
                read_u32(0, slot, function, 0x10),
                read_u32(0, slot, function, 0x14),
                read_u32(0, slot, function, 0x18),
                read_u32(0, slot, function, 0x1C),
                read_u32(0, slot, function, 0x20),
                read_u32(0, slot, function, 0x24),
            ];

            println!(
                "pci: {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}.{:02x}.{:02x} rev {:02x}",
                0u8, slot, function, vendor, device, base_class, subclass, prog_if, revision
            );

            if found.is_some()
                || base_class != CLASS_SERIAL_BUS
                || subclass != SUBCLASS_USB
                || prog_if != PROG_IF_UHCI
            {
                continue;
            }

            let bar4 = bars[4];
            if bar4 & 1 == 0 {
                println!("pci: USB controller has no I/O BAR4; ignored");
                continue;
            }

            let command = read_u16(0, slot, function, 0x04);
            write_u16(
                0,
                slot,
                function,
                0x04,
                command | COMMAND_IO_SPACE | COMMAND_BUS_MASTER,
            );

            found = Some(UsbController {
                bus: 0,
                slot,
                function,
                vendor,
                device,
                // An I/O BAR uses the low five bits as flags; the UHCI region
                // is 32 bytes, so this mask is the device's own granularity.
                io_base: (bar4 & !0x1F) as u16,
            });
        }
    }

    found
}

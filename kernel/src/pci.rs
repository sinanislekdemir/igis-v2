//! PCI configuration-space scan (spec 0010, "Absolute pointer"; ADR 0022; spec
//! 0020, N2; ADR 0049).
//!
//! x86-specific for now: configuration space is reached through the
//! 0xCF8/0xCFC port pair, and the device I/O BARs are x86 I/O port ranges. The
//! scan is bounded and allocation-free: bus 0, slots 0..31, functions 0..7.
//!
//! [`init`] keeps its M2.2 job — locate the UHCI host controller the USB tablet
//! hangs on and log one line per function found. [`find_virtio_net`] is the
//! default network path (M10.1) and [`find_e1000`] the second driver (M10.9):
//! each looks for its own function and enables the PCI command bits it needs. A
//! second scan of the same 256 functions is cheap and stays silent, so the boot
//! log still has exactly one PCI line per function.
//!
//! ADR 0013 says machine discovery belongs behind `arch`/`platform`. This
//! module, [`crate::usb`] and [`crate::net`] are the exception for the current
//! single-machine slices; moving them is a mechanical change when a second
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

/// Vendor id of every virtio device.
const VIRTIO_VENDOR: u16 = 0x1AF4;
/// Legacy virtio-net (0x1000) and the transitional virtio 1.0 id (0x1041).
const VIRTIO_NET_LEGACY: u16 = 0x1000;
const VIRTIO_NET_TRANSITIONAL: u16 = 0x1041;

/// Intel, and the e1000/e1000e controller ids the second driver accepts
/// (spec 0020, N2/ADR 0049). Only `82540EM` (QEMU `-device e1000`) and `82574L`
/// (QEMU `-device e1000e`) are exercised; the other ids share the register set
/// but are unproven here and listed so a machine does not silently fall back.
const INTEL_VENDOR: u16 = 0x8086;
const E1000_82540EM: u16 = 0x100E;
const E1000_82545EM: u16 = 0x100F;
const E1000_82574L: u16 = 0x10D3;
const E1000_I217: u16 = 0x153A;
const E1000_I219: u16 = 0x15B8;

/// Command-register bits: I/O space, memory space and bus mastering.
const COMMAND_IO_SPACE: u16 = 1 << 0;
const COMMAND_MEMORY_SPACE: u16 = 1 << 1;
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

/// One function found on bus 0.
#[derive(Clone, Copy)]
pub struct Device {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor: u16,
    pub device: u16,
    pub revision: u8,
    pub prog_if: u8,
    pub subclass: u8,
    pub base_class: u8,
    /// The six 32-bit BARs; bit 0 of a BAR is set for an I/O BAR.
    pub bars: [u32; 6],
}

/// The virtio-net function the kernel driver owns.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct VirtioNetController {
    pub vendor: u16,
    pub device: u16,
    pub slot: u8,
    pub function: u8,
    /// Base of the legacy 32-byte virtio I/O region (BAR0).
    pub io_base: u16,
}

/// The e1000-family function the kernel driver owns (spec 0020, N2).
#[derive(Clone, Copy)]
pub struct E1000Controller {
    pub vendor: u16,
    pub device: u16,
    pub slot: u8,
    pub function: u8,
    /// Physical base of the MMIO register window (BAR0, low four bits masked).
    pub mmio_base: u64,
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

/// Enables I/O space and bus mastering for one function.
fn enable_io_and_bus_master(device: &Device) {
    let command = read_u16(device.bus, device.slot, device.function, 0x04);
    write_u16(
        device.bus,
        device.slot,
        device.function,
        0x04,
        command | COMMAND_IO_SPACE | COMMAND_BUS_MASTER,
    );
}

/// Enables memory space and bus mastering for one function.
///
/// A device with a memory BAR does not need I/O space; the UHCI and virtio
/// paths keep their I/O variant unchanged.
fn enable_memory_and_bus_master(device: &Device) {
    let command = read_u16(device.bus, device.slot, device.function, 0x04);
    write_u16(
        device.bus,
        device.slot,
        device.function,
        0x04,
        command | COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER,
    );
}

/// Reads one function, or `None` when the slot is empty.
fn probe(bus: u8, slot: u8, function: u8) -> Option<Device> {
    let id = read_u32(bus, slot, function, 0x00);
    let vendor = (id & 0xFFFF) as u16;
    if vendor == 0xFFFF {
        return None;
    }
    let device = (id >> 16) as u16;

    let class = read_u32(bus, slot, function, 0x08);
    let revision = (class & 0xFF) as u8;
    let prog_if = ((class >> 8) & 0xFF) as u8;
    let subclass = ((class >> 16) & 0xFF) as u8;
    let base_class = ((class >> 24) & 0xFF) as u8;

    let bars = [
        read_u32(bus, slot, function, 0x10),
        read_u32(bus, slot, function, 0x14),
        read_u32(bus, slot, function, 0x18),
        read_u32(bus, slot, function, 0x1C),
        read_u32(bus, slot, function, 0x20),
        read_u32(bus, slot, function, 0x24),
    ];

    Some(Device {
        bus,
        slot,
        function,
        vendor,
        device,
        revision,
        prog_if,
        subclass,
        base_class,
        bars,
    })
}

/// Visits every function on bus 0 that has a real vendor id.
pub fn scan(mut visit: impl FnMut(&Device)) {
    for slot in 0..32u8 {
        for function in 0..8u8 {
            if let Some(device) = probe(0, slot, function) {
                visit(&device);
            }
        }
    }
}

/// Scans bus 0 and returns the first UHCI controller.
///
/// The first matching function has its I/O space and bus-mastering bits
/// enabled and BAR4 is taken as the I/O base. Everything else is logged and
/// ignored. No allocation, no dynamic tables, and a fixed 32 * 8 bound.
pub fn init() -> Option<UsbController> {
    let mut found = None;

    scan(|device| {
        println!(
            "pci: {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}.{:02x}.{:02x} rev {:02x}",
            device.bus,
            device.slot,
            device.function,
            device.vendor,
            device.device,
            device.base_class,
            device.subclass,
            device.prog_if,
            device.revision
        );

        if found.is_some()
            || device.base_class != CLASS_SERIAL_BUS
            || device.subclass != SUBCLASS_USB
            || device.prog_if != PROG_IF_UHCI
        {
            return;
        }

        let bar4 = device.bars[4];
        if bar4 & 1 == 0 {
            println!("pci: USB controller has no I/O BAR4; ignored");
            return;
        }

        enable_io_and_bus_master(device);

        found = Some(UsbController {
            bus: device.bus,
            slot: device.slot,
            function: device.function,
            vendor: device.vendor,
            device: device.device,
            // An I/O BAR uses the low five bits as flags; the UHCI region
            // is 32 bytes, so this mask is the device's own granularity.
            io_base: (bar4 & !0x1F) as u16,
        });
    });

    found
}

/// Finds a virtio-net function on bus 0 and enables I/O space + bus mastering.
///
/// The scan is deliberately silent: [`init`] already logged every function.
/// Only a legacy I/O BAR0 (bit 0 set) is accepted; a device without one is not
/// a driver this kernel can use and is skipped.
pub fn find_virtio_net() -> Option<VirtioNetController> {
    let mut found = None;

    scan(|device| {
        if found.is_some()
            || device.vendor != VIRTIO_VENDOR
            || (device.device != VIRTIO_NET_LEGACY && device.device != VIRTIO_NET_TRANSITIONAL)
        {
            return;
        }
        let bar0 = device.bars[0];
        if bar0 & 1 == 0 {
            return;
        }

        enable_io_and_bus_master(device);

        found = Some(VirtioNetController {
            vendor: device.vendor,
            device: device.device,
            slot: device.slot,
            function: device.function,
            // The legacy register window is 32 bytes; an I/O BAR's low five
            // bits are flags, not address.
            io_base: (bar0 & !0x1F) as u16,
        });
    });

    found
}

/// Finds an e1000/e1000e function on bus 0 and enables memory space plus bus
/// mastering (spec 0020, N2).
///
/// Only a memory BAR0 (bit 0 clear) is accepted; its low four bits are flags.
/// A 64-bit memory BAR takes its upper half from BAR1. An I/O BAR is not a
/// device this driver can drive and is skipped. The scan is silent for the same
/// reason [`find_virtio_net`] is: [`init`] already logged every function.
pub fn find_e1000() -> Option<E1000Controller> {
    let mut found = None;

    scan(|device| {
        if found.is_some()
            || device.vendor != INTEL_VENDOR
            || !matches!(
                device.device,
                E1000_82540EM | E1000_82545EM | E1000_82574L | E1000_I217 | E1000_I219
            )
        {
            return;
        }
        let bar0 = device.bars[0];
        if bar0 & 1 != 0 {
            return;
        }
        // BAR type bits 2:1: 00 = 32-bit memory, 10 = 64-bit memory.
        let mmio_base = match (bar0 >> 1) & 0b11 {
            0b00 => u64::from(bar0 & !0xF),
            0b10 => u64::from(bar0 & !0xF) | (u64::from(device.bars[1]) << 32),
            _ => return,
        };
        if mmio_base == 0 {
            return;
        }

        enable_memory_and_bus_master(device);

        found = Some(E1000Controller {
            vendor: device.vendor,
            device: device.device,
            slot: device.slot,
            function: device.function,
            mmio_base,
        });
    });

    found
}

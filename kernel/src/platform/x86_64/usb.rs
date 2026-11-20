//! Polled USB HID tablet on the PIIX3 UHCI controller (spec 0010,
//! "Absolute pointer"; ADR 0022).
//!
//! This is x86-specific for now. The controller is reached through the I/O BAR
//! found by [`crate::pci`], the schedule and every transfer descriptor are
//! static `.bss` memory, and the interrupt-IN endpoint is polled from the same
//! service opportunity as the PS/2 mouse. Nothing here allocates.
//!
//! # DMA addresses
//!
//! The kernel image is linked at [`arch_x86_64::KERNEL_BASE`] and the boot
//! page tables alias the first GiB there, so a static structure's physical
//! address is `virtual - KERNEL_BASE`. The frame list and the transfer
//! descriptors are written through those virtual addresses and handed to the
//! controller as physical ones.
//!
//! # The measured report
//!
//! QEMU's `usb-tablet` is a full-speed HID device (VID:PID `0627:0001`,
//! interface class 3/subclass 0/protocol 0, one interrupt-IN endpoint with an
//! 8-byte packet). Its report descriptor declares a five-button field and two
//! absolute 16-bit axes. The report on the wire, measured through QMP
//! `input-send-event` absolute injection, is:
//!
//! ```text
//! byte 0      buttons: bit 0 left, bit 1 right, bit 2 middle (bits 3–4 ignored)
//! byte 1..2   X, little-endian u16, 0..=32767
//! byte 3..4   Y, little-endian u16, 0..=32767, origin top-left
//! byte 5      wheel (unused)
//! ```
//!
//! The descriptor is checked at enumeration for exactly this shape (absolute
//! X/Y, 16-bit report size, two report counts, a one-byte button field). A
//! device whose descriptor differs is refused rather than guessed at; the PS/2
//! path stays the pointer.
//!
//! ADR 0013 puts machine discovery behind `arch`/`platform`; this module and
//! `kernel/src/pci.rs` are the M2.2 exception and move when a second machine
//! exists (ADR 0022).

use core::ptr::{addr_of_mut, read_volatile, write_volatile};

use arch_x86_64::port::{inw, outl, outw};
use arch_x86_64::KERNEL_BASE;

use crate::sync::SpinLock;
use crate::{clock, gfx, mouse, pci, println};

// ---- UHCI register offsets and bits (UHCI 1.1, QEMU `uhci-regs.h`) --------

const USBCMD: u16 = 0x00;
const USBSTS: u16 = 0x02;
const USBINTR: u16 = 0x04;
const FRNUM: u16 = 0x06;
const FLBASEADD: u16 = 0x08;
const PORTSC1: u16 = 0x10;
const PORTSC2: u16 = 0x12;

const CMD_RS: u16 = 1 << 0;
const CMD_HCRESET: u16 = 1 << 1;
const CMD_CF: u16 = 1 << 6;
/// `MAXP` = 01: 64-byte maximum packet (the full-speed maximum).
const CMD_MAXP64: u16 = 1 << 7;

const STS_USBINT: u16 = 1 << 0;
const STS_USBERR: u16 = 1 << 1;
const STS_RD: u16 = 1 << 2;
const STS_HSERR: u16 = 1 << 3;
const STS_HCPERR: u16 = 1 << 4;
const STS_HCHALTED: u16 = 1 << 5;
const STS_ALL: u16 = STS_USBINT | STS_USBERR | STS_RD | STS_HSERR | STS_HCPERR;

const PORT_CCS: u16 = 1 << 0;
const PORT_CSC: u16 = 1 << 1;
const PORT_EN: u16 = 1 << 2;
const PORT_ENC: u16 = 1 << 3;
const PORT_LSDA: u16 = 1 << 8;
const PORT_PR: u16 = 1 << 9;
const PORT_CHANGE: u16 = PORT_CSC | PORT_ENC;

// ---- Queue heads and transfer descriptors ---------------------------------

/// Link-pointer terminator (bit 0). A valid pointer has bit 0 clear.
const LINK_TERM: u32 = 1;
/// Link-pointer type: 1 = queue head, 0 = transfer descriptor.
const LINK_QH: u32 = 2;
/// Depth-first: continue with the next TD instead of returning to the QH link.
const LINK_DEPTH: u32 = 4;

const TD_ACTIVE: u32 = 1 << 23;
const TD_STALL: u32 = 1 << 22;
const TD_DBUFERR: u32 = 1 << 21;
const TD_BABBLE: u32 = 1 << 20;
const TD_TIMEOUT: u32 = 1 << 18;
const TD_BITSTUFF: u32 = 1 << 17;
const TD_ERRORS: u32 = TD_STALL | TD_DBUFERR | TD_BABBLE | TD_TIMEOUT | TD_BITSTUFF;
const TD_ACTLEN_MASK: u32 = 0x7FF;
/// Maximum error retries (bits 28:27), the UHCI recommended value.
const TD_ERROR_COUNT: u32 = 3 << 27;

const PID_IN: u8 = 0x69;
const PID_OUT: u8 = 0xE1;
const PID_SETUP: u8 = 0x2D;
const TD_TOGGLE: u32 = 1 << 19;

/// One queue head. The hardware reads the first two dwords; the rest keeps the
/// structure 16-byte aligned and roomy, matching the layout note in the plan.
#[repr(C, align(16))]
struct Qh {
    link: u32,
    element: u32,
    _reserved: [u32; 2],
}

impl Qh {
    const fn new() -> Self {
        Qh {
            link: LINK_TERM,
            element: LINK_TERM,
            _reserved: [0; 2],
        }
    }
}

/// One transfer descriptor: 16 bytes, 16-byte aligned.
#[repr(C, align(16))]
struct Td {
    link: u32,
    status: u32,
    token: u32,
    buffer: u32,
}

impl Td {
    const fn new() -> Self {
        Td {
            link: LINK_TERM,
            status: 0,
            token: 0,
            buffer: 0,
        }
    }
}

/// One 4 KiB frame list, 1024 entries. Every entry points at the active QH.
#[repr(C, align(4096))]
struct FrameList([u32; FRAME_COUNT]);

impl FrameList {
    const fn new() -> Self {
        FrameList([LINK_TERM; FRAME_COUNT])
    }
}

const FRAME_COUNT: usize = 1024;
/// Largest control data stage this driver issues (configuration descriptor).
const DATA_BYTES: usize = 128;
/// The measured report is 6 bytes; five are used (buttons, X, Y).
const REPORT_BYTES: usize = 5;
const HID_MAX_PACKET: usize = 8;

static mut FRAME_LIST: FrameList = FrameList::new();

#[repr(C, align(16))]
struct Dma {
    control_qh: Qh,
    interrupt_qh: Qh,
    control_tds: [Td; 3],
    interrupt_td: Td,
    setup: [u8; 8],
    data: [u8; DATA_BYTES],
}

impl Dma {
    const fn new() -> Self {
        Dma {
            control_qh: Qh::new(),
            interrupt_qh: Qh::new(),
            control_tds: [Td::new(), Td::new(), Td::new()],
            interrupt_td: Td::new(),
            setup: [0; 8],
            data: [0; DATA_BYTES],
        }
    }
}

static mut DMA: Dma = Dma::new();

// ---- Addresses ------------------------------------------------------------

fn physical(virtual_address: u64) -> u32 {
    debug_assert!(virtual_address >= KERNEL_BASE);
    let physical = virtual_address - KERNEL_BASE;
    debug_assert!(physical < 4 * 1024 * 1024 * 1024);
    physical as u32
}

unsafe fn frame_list_physical() -> u32 {
    physical(addr_of_mut!(FRAME_LIST) as u64)
}

unsafe fn control_qh_physical() -> u32 {
    physical(addr_of_mut!(DMA.control_qh) as u64)
}

unsafe fn interrupt_qh_physical() -> u32 {
    physical(addr_of_mut!(DMA.interrupt_qh) as u64)
}

unsafe fn control_td(index: usize) -> *mut Td {
    addr_of_mut!(DMA.control_tds[index])
}

unsafe fn interrupt_td() -> *mut Td {
    addr_of_mut!(DMA.interrupt_td)
}

unsafe fn setup_buffer() -> *mut u8 {
    addr_of_mut!(DMA.setup) as *mut u8
}

unsafe fn data_buffer() -> *mut u8 {
    addr_of_mut!(DMA.data) as *mut u8
}

unsafe fn pointer_physical<T>(pointer: *const T) -> u32 {
    physical(pointer as u64)
}

// ---- Driver state ---------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Off,
    Ready,
    Failed,
}

struct Usb {
    io_base: u16,
    port: u8,
    state: State,
    address: u8,
    endpoint: u8,
    max_packet: u16,
    toggle: bool,
}

static USB: SpinLock<Usb> = SpinLock::new(Usb {
    io_base: 0,
    port: 0,
    state: State::Off,
    address: 0,
    endpoint: 0,
    max_packet: HID_MAX_PACKET as u16,
    toggle: false,
});

#[derive(Clone, Copy)]
struct Tablet {
    vendor: u16,
    product: u16,
}

#[derive(Clone, Copy)]
struct Config {
    value: u8,
    interface: u8,
    endpoint: u8,
    max_packet: u16,
    report_length: u16,
}

#[derive(Clone, Copy)]
enum Fail {
    Reset,
    NoPort,
    LowSpeed,
    PortEnable,
    Timeout,
    Transfer,
    Descriptor,
    Unsupported,
}

impl Fail {
    fn name(self) -> &'static str {
        match self {
            Fail::Reset => "controller reset",
            Fail::NoPort => "no device",
            Fail::LowSpeed => "low-speed device",
            Fail::PortEnable => "port enable",
            Fail::Timeout => "transfer timeout",
            Fail::Transfer => "transfer error",
            Fail::Descriptor => "descriptor too short",
            Fail::Unsupported => "not an absolute tablet",
        }
    }
}

// ---- Low-level helpers ----------------------------------------------------

fn read16(io_base: u16, offset: u16) -> u16 {
    // SAFETY: the I/O region was found and enabled by `pci::init`.
    unsafe { inw(io_base + offset) }
}

fn write16(io_base: u16, offset: u16, value: u16) {
    // SAFETY: as above; every register write is to the UHCI I/O region.
    unsafe { outw(io_base + offset, value) }
}

fn write32(io_base: u16, offset: u16, value: u32) {
    // SAFETY: as above.
    unsafe { outl(io_base + offset, value) }
}

/// Bounded wait on a condition. The clock is the primary limit; the spin cap
/// keeps a machine without a running timer from hanging.
fn wait_ms(milliseconds: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = clock::now_ns().saturating_add(milliseconds.saturating_mul(1_000_000));
    let mut spins = 0u64;
    loop {
        if ready() {
            return true;
        }
        if clock::now_ns() >= deadline || spins >= 5_000_000 {
            return false;
        }
        spins += 1;
        core::hint::spin_loop();
    }
}

fn delay_ms(milliseconds: u64) {
    let _ = wait_ms(milliseconds, || false);
}

fn token(pid: u8, address: u8, endpoint: u8, toggle: bool, length: usize) -> u32 {
    let max_length = if length == 0 {
        TD_ACTLEN_MASK
    } else {
        (length - 1) as u32 & TD_ACTLEN_MASK
    };
    u32::from(pid)
        | (u32::from(address & 0x7F) << 8)
        | (u32::from(endpoint & 0x0F) << 15)
        | if toggle { TD_TOGGLE } else { 0 }
        | (max_length << 21)
}

unsafe fn write_qh(qh: *mut Qh, link: u32, element: u32) {
    write_volatile(addr_of_mut!((*qh).link), link);
    write_volatile(addr_of_mut!((*qh).element), element);
}

/// Writes a TD with the status last: the Active bit is what makes it reachable
/// to the controller, so every other field must already be in memory.
unsafe fn write_td(
    td: *mut Td,
    link: u32,
    pid: u8,
    address: u8,
    endpoint: u8,
    toggle: bool,
    length: usize,
    buffer: u32,
) {
    write_volatile(addr_of_mut!((*td).link), link);
    write_volatile(
        addr_of_mut!((*td).token),
        token(pid, address, endpoint, toggle, length),
    );
    write_volatile(addr_of_mut!((*td).buffer), buffer);
    write_volatile(addr_of_mut!((*td).status), TD_ACTIVE | TD_ERROR_COUNT);
}

unsafe fn deactivate_td(td: *mut Td) {
    write_volatile(addr_of_mut!((*td).status), 0);
}

/// Bounded spin on the controller reset bit, then set the frame list.
fn controller_init(io_base: u16) -> Result<(), Fail> {
    write16(io_base, USBCMD, CMD_HCRESET);
    if !wait_ms(20, || read16(io_base, USBCMD) & CMD_HCRESET == 0) {
        return Err(Fail::Reset);
    }

    write16(io_base, USBSTS, STS_ALL);
    write32(io_base, FLBASEADD, unsafe { frame_list_physical() });
    write16(io_base, FRNUM, 0);
    write16(io_base, USBINTR, 0);

    unsafe {
        write_qh(addr_of_mut!(DMA.control_qh), LINK_TERM, LINK_TERM);
        write_qh(addr_of_mut!(DMA.interrupt_qh), LINK_TERM, LINK_TERM);
        for index in 0..3 {
            deactivate_td(control_td(index));
        }
        deactivate_td(interrupt_td());
        let entries = addr_of_mut!(FRAME_LIST.0) as *mut u32;
        for index in 0..FRAME_COUNT {
            write_volatile(entries.add(index), control_qh_physical() | LINK_QH);
        }
    }

    write16(io_base, USBCMD, CMD_RS | CMD_CF | CMD_MAXP64);
    if !wait_ms(20, || read16(io_base, USBSTS) & STS_HCHALTED == 0) {
        return Err(Fail::Reset);
    }
    Ok(())
}

fn find_port(io_base: u16) -> Option<u8> {
    for port in 0..2u8 {
        let offset = PORTSC1 + u16::from(port) * 2;
        if read16(io_base, offset) & PORT_CCS != 0 {
            return Some(port);
        }
    }
    None
}

fn reset_port(io_base: u16, port: u8) -> Result<(), Fail> {
    let offset = if port == 0 { PORTSC1 } else { PORTSC2 };
    let initial = read16(io_base, offset);
    if initial & PORT_CCS == 0 {
        return Err(Fail::NoPort);
    }
    if initial & PORT_LSDA != 0 {
        return Err(Fail::LowSpeed);
    }

    // Acknowledge any stale change bits, then drive reset for 50 ms.
    write16(io_base, offset, PORT_CHANGE);
    write16(io_base, offset, PORT_PR);
    delay_ms(50);
    write16(io_base, offset, 0);
    delay_ms(10);
    // Real hardware enables the port after reset; QEMU requires the explicit
    // Enable bit. Writing it is harmless on both.
    write16(io_base, offset, PORT_EN);
    delay_ms(10);
    // Every PORTSC write starts from the read-only bits in QEMU, so the final
    // change-bit acknowledge must carry Enable again or it clears the port.
    write16(io_base, offset, PORT_EN | PORT_CHANGE);

    let status = read16(io_base, offset);
    if status & (PORT_CCS | PORT_EN) != (PORT_CCS | PORT_EN) {
        return Err(Fail::PortEnable);
    }
    Ok(())
}

// ---- Control transfers ----------------------------------------------------

/// Runs one control transfer through the control QH and its three TDs.
///
/// Returns the number of bytes the device wrote to the data stage. Every wait
/// is bounded; a STALL, a timeout or a short descriptor is an error, never a
/// retry loop.
fn control(
    usb: &Usb,
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: usize,
) -> Result<usize, Fail> {
    if length > DATA_BYTES {
        return Err(Fail::Descriptor);
    }
    let setup = [
        request_type,
        request,
        value as u8,
        (value >> 8) as u8,
        index as u8,
        (index >> 8) as u8,
        length as u8,
        (length >> 8) as u8,
    ];
    unsafe {
        let destination = setup_buffer();
        for (offset, byte) in setup.iter().enumerate() {
            write_volatile(destination.add(offset), *byte);
        }
    }

    let address = usb.address;
    let endpoint = 0u8;
    // A data stage exists only when the request asks for bytes. The status
    // stage is opposite the data direction; with no data it is always IN.
    let data_in = request_type & 0x80 != 0;
    let has_data = length != 0;
    let status_pid = if has_data && data_in { PID_OUT } else { PID_IN };

    let setup_td = unsafe { control_td(0) };
    let data_td = unsafe { control_td(1) };
    let status_td = unsafe { control_td(2) };

    unsafe {
        let setup_physical = pointer_physical(setup_buffer());
        let data_physical = pointer_physical(data_buffer());
        deactivate_td(data_td);
        deactivate_td(status_td);

        let status_link = LINK_TERM;
        write_td(
            status_td,
            status_link,
            status_pid,
            address,
            endpoint,
            true,
            0,
            0,
        );

        if has_data {
            let data_link = pointer_physical(status_td) | LINK_DEPTH;
            write_td(
                data_td,
                data_link,
                if data_in { PID_IN } else { PID_OUT },
                address,
                endpoint,
                true,
                length,
                data_physical,
            );
            write_td(
                setup_td,
                pointer_physical(data_td) | LINK_DEPTH,
                PID_SETUP,
                address,
                endpoint,
                false,
                8,
                setup_physical,
            );
        } else {
            write_td(
                setup_td,
                pointer_physical(status_td) | LINK_DEPTH,
                PID_SETUP,
                address,
                endpoint,
                false,
                8,
                setup_physical,
            );
        }

        // Arm the QH only after every TD field is in memory.
        let qh = addr_of_mut!(DMA.control_qh);
        write_volatile(addr_of_mut!((*qh).element), pointer_physical(setup_td));
    }

    let deadline = clock::now_ns().saturating_add(200_000_000);
    let mut spins = 0u64;
    loop {
        let last = unsafe { read_volatile(addr_of_mut!((*status_td).status)) };
        if last & TD_ACTIVE == 0 {
            break;
        }
        // A failure in an earlier stage stops the frame before the status TD,
        // so inspect every TD instead of waiting for the last one.
        for stage in 0..3 {
            let td = unsafe { control_td(stage) };
            let status = unsafe { read_volatile(addr_of_mut!((*td).status)) };
            if status & TD_ACTIVE == 0 && status & TD_ERRORS != 0 {
                return Err(Fail::Transfer);
            }
        }
        if clock::now_ns() >= deadline || spins >= 5_000_000 {
            return Err(Fail::Timeout);
        }
        spins += 1;
        core::hint::spin_loop();
    }

    for stage in 0..3 {
        let td = unsafe { control_td(stage) };
        let status = unsafe { read_volatile(addr_of_mut!((*td).status)) };
        if status & TD_ERRORS != 0 {
            return Err(Fail::Transfer);
        }
    }

    if !has_data {
        return Ok(0);
    }
    let status = unsafe { read_volatile(addr_of_mut!((*data_td).status)) };
    let encoded = status & TD_ACTLEN_MASK;
    let actual = if encoded == TD_ACTLEN_MASK {
        0
    } else {
        (encoded + 1) as usize
    };
    Ok(actual.min(length))
}

fn copy_data(count: usize) -> [u8; DATA_BYTES] {
    let mut buffer = [0u8; DATA_BYTES];
    let copied = count.min(DATA_BYTES);
    // SAFETY: the control transfer completed and wrote at most `copied` bytes
    // into the static data buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(data_buffer(), buffer.as_mut_ptr(), copied);
    }
    buffer
}

fn get_device_descriptor(usb: &Usb) -> Result<Tablet, Fail> {
    let length = control(usb, 0x80, 0x06, 0x0100, 0, 18)?;
    if length < 18 {
        return Err(Fail::Descriptor);
    }
    let data = copy_data(length);
    if data[0] < 18 || data[1] != 1 {
        return Err(Fail::Descriptor);
    }
    let vendor = u16::from_le_bytes([data[8], data[9]]);
    let product = u16::from_le_bytes([data[10], data[11]]);
    let bcd = u16::from_le_bytes([data[2], data[3]]);
    println!(
        "usb: device {vendor:04x}:{product:04x} bcdUSB {bcd:04x} maxp {}",
        data[7]
    );
    Ok(Tablet { vendor, product })
}

fn set_address(usb: &Usb, address: u8) -> Result<(), Fail> {
    control(usb, 0x00, 0x05, u16::from(address), 0, 0)?;
    Ok(())
}

fn get_configuration(usb: &Usb) -> Result<Config, Fail> {
    let length = control(usb, 0x80, 0x06, 0x0200, 0, DATA_BYTES)?;
    if length < 9 {
        return Err(Fail::Descriptor);
    }
    let data = copy_data(length);
    if data[0] < 9 || data[1] != 2 {
        return Err(Fail::Descriptor);
    }
    let total = usize::from(u16::from_le_bytes([data[2], data[3]]));
    if total < 9 || total > length {
        return Err(Fail::Descriptor);
    }

    let value = data[5];
    let mut offset = usize::from(data[0]);
    let mut interface: Option<(u8, u8, u8)> = None;
    let mut endpoint = None;
    let mut report_length = 0u16;
    // Endpoints belong to the interface descriptor that precedes them, so an
    // interrupt IN on some earlier non-HID interface must not be chosen.
    let mut in_hid_interface = false;

    while offset + 2 <= total {
        let descriptor_length = usize::from(data[offset]);
        if descriptor_length < 2 || offset + descriptor_length > total {
            break;
        }
        let kind = data[offset + 1];
        match kind {
            4 if descriptor_length >= 9 => {
                let number = data[offset + 2];
                let class = data[offset + 5];
                let subclass = data[offset + 6];
                let protocol = data[offset + 7];
                in_hid_interface = class == 3 && interface.is_none();
                if in_hid_interface {
                    interface = Some((number, subclass, protocol));
                }
            }
            5 if descriptor_length >= 7 => {
                let address = data[offset + 2];
                let attributes = data[offset + 3];
                let max_packet = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) & 0x07FF;
                if in_hid_interface
                    && address & 0x80 != 0
                    && attributes & 0x03 == 3
                    && endpoint.is_none()
                {
                    endpoint = Some((address & 0x0F, max_packet, data[offset + 6]));
                }
            }
            0x21 if descriptor_length >= 9 => {
                report_length = u16::from_le_bytes([data[offset + 7], data[offset + 8]]);
            }
            _ => {}
        }
        offset += descriptor_length;
    }

    let Some((interface, subclass, protocol)) = interface else {
        return Err(Fail::Unsupported);
    };
    let Some((endpoint, max_packet, interval)) = endpoint else {
        return Err(Fail::Unsupported);
    };
    if subclass != 0 || protocol != 0 {
        // A boot mouse (1/2) or keyboard (1/1) is not the absolute pointer.
        return Err(Fail::Unsupported);
    }
    if max_packet == 0 {
        return Err(Fail::Descriptor);
    }

    println!(
        "usb: config {} iface {} class 3/{subclass}/{protocol} endpoint {endpoint} IN maxp {max_packet} interval {interval}",
        value, interface
    );

    Ok(Config {
        value,
        interface,
        endpoint,
        max_packet,
        report_length,
    })
}

fn set_configuration(usb: &Usb, value: u8) -> Result<(), Fail> {
    control(usb, 0x00, 0x09, u16::from(value), 0, 0)?;
    Ok(())
}

fn hid_set_idle(usb: &Usb, interface: u8) -> Result<(), Fail> {
    control(usb, 0x21, 0x0A, 0, u16::from(interface), 0)?;
    Ok(())
}

fn get_report_descriptor(usb: &Usb, interface: u8) -> Result<usize, Fail> {
    // 64 bytes is enough for the button and X/Y items; QEMU's descriptor is
    // 74 bytes, which the HID descriptor's own length field reports.
    control(usb, 0x81, 0x06, 0x2200, u16::from(interface), 64)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The measured QEMU tablet shape: a five-button field (bits 0–2 used) in one
/// byte, two absolute 16-bit axes with logical maximum `0x7fff`.
fn is_tablet_report(data: &[u8]) -> bool {
    contains(data, &[0x09, 0x30, 0x09, 0x31])       // Usage X, Usage Y
        && contains(data, &[0x26, 0xFF, 0x7F])      // Logical Maximum 32767
        && contains(data, &[0x75, 0x10])            // Report Size 16
        && contains(data, &[0x95, 0x02])            // Report Count 2
        && contains(data, &[0x81, 0x02])            // Input (Data,Var,Abs)
        && contains(data, &[0x95, 0x05, 0x75, 0x01]) // 5 buttons, 1 bit
        && contains(data, &[0x95, 0x01, 0x75, 0x03]) // 3 padding bits
}

fn arm_interrupt(usb: &mut Usb, config: &Config) -> Result<(), Fail> {
    let max_packet = usize::from(config.max_packet).min(64);
    if max_packet < REPORT_BYTES {
        return Err(Fail::Descriptor);
    }
    usb.endpoint = config.endpoint;
    usb.max_packet = max_packet as u16;
    usb.toggle = false;

    let td = unsafe { interrupt_td() };
    let td_physical = unsafe { pointer_physical(td) };
    unsafe {
        write_volatile(addr_of_mut!((*td).link), td_physical | LINK_DEPTH);
        write_volatile(
            addr_of_mut!((*td).token),
            token(PID_IN, usb.address, usb.endpoint, false, max_packet),
        );
        write_volatile(addr_of_mut!((*td).buffer), pointer_physical(data_buffer()));
        write_volatile(addr_of_mut!((*td).status), TD_ACTIVE | TD_ERROR_COUNT);
        write_qh(addr_of_mut!(DMA.interrupt_qh), LINK_TERM, td_physical);
        let entries = addr_of_mut!(FRAME_LIST.0) as *mut u32;
        for index in 0..FRAME_COUNT {
            write_volatile(entries.add(index), interrupt_qh_physical() | LINK_QH);
        }
    }
    Ok(())
}

fn bring_up(usb: &mut Usb) -> Result<Tablet, Fail> {
    controller_init(usb.io_base)?;
    let port = find_port(usb.io_base).ok_or(Fail::NoPort)?;
    usb.port = port;
    reset_port(usb.io_base, port)?;

    usb.address = 0;
    let tablet = get_device_descriptor(usb)?;
    set_address(usb, 1)?;
    usb.address = 1;

    let config = get_configuration(usb)?;
    set_configuration(usb, config.value)?;
    hid_set_idle(usb, config.interface)?;
    let report_length = get_report_descriptor(usb, config.interface)?;
    let report = copy_data(report_length);
    if !is_tablet_report(&report[..report_length.min(DATA_BYTES)]) {
        return Err(Fail::Unsupported);
    }
    println!(
        "usb: HID report {} bytes (descriptor declares {}), abs X/Y 16-bit",
        report_length, config.report_length
    );

    arm_interrupt(usb, &config)?;
    Ok(tablet)
}

// ---- Boot entry and polling ----------------------------------------------

/// Runs the PCI scan result through UHCI enumeration and arms the interrupt
/// endpoint. A failure is a boot log line and leaves the PS/2 pointer in place.
///
/// Must run after the APIC timer so [`clock::now_ns`] advances during the
/// bounded port-reset and transfer waits.
pub fn init(controller: Option<pci::UsbController>) {
    let Some(controller) = controller else {
        println!("usb: no UHCI; PS/2 only");
        return;
    };

    let started = clock::now_ns();
    let mut guard = USB.lock();
    let usb = &mut *guard;
    usb.io_base = controller.io_base;
    usb.state = State::Off;

    println!(
        "usb: UHCI at {:#06x} (pci {:02x}:{:02x}.{} {:04x}:{:04x}) frame list {:#x}",
        controller.io_base,
        controller.bus,
        controller.slot,
        controller.function,
        controller.vendor,
        controller.device,
        unsafe { frame_list_physical() }
    );
    println!(
        "usb: USBSTS {:#06x} PORTSC1 {:#06x} PORTSC2 {:#06x}",
        read16(usb.io_base, USBSTS),
        read16(usb.io_base, PORTSC1),
        read16(usb.io_base, PORTSC2)
    );

    match bring_up(usb) {
        Ok(tablet) => {
            usb.state = State::Ready;
            gfx::set_pointer_source(gfx::POINTER_SOURCE_TABLET);
            gfx::show_cursor();
            println!(
                "usb: tablet {:04x}:{:04x} addr {} (abs X/Y)",
                tablet.vendor, tablet.product, usb.address
            );
        }
        Err(fail) => {
            usb.state = State::Failed;
            println!("usb: no tablet ({}); PS/2 only", fail.name());
            gfx::set_pointer_source(if mouse::ready() {
                gfx::POINTER_SOURCE_PS2
            } else {
                gfx::POINTER_SOURCE_NONE
            });
        }
    }

    println!(
        "usb: enumeration {} ms",
        clock::now_ns().saturating_sub(started) / 1_000_000
    );
}

fn actual_length(status: u32) -> usize {
    let encoded = status & TD_ACTLEN_MASK;
    if encoded == TD_ACTLEN_MASK {
        0
    } else {
        (encoded + 1) as usize
    }
}

/// Maps a HID absolute axis `0..=32767` to `0..=extent-1`, rounding to nearest.
///
/// The doubled numerator keeps the midpoint of an odd extent (512 of 1024,
/// 384 of 768) exact without floating point.
fn scale(value: u16, extent: i64) -> i64 {
    if extent <= 1 {
        return 0;
    }
    let maximum = (extent - 1) as u64;
    let value = u64::from(value.min(0x7FFF));
    ((value * maximum * 2 + 0x7FFF) / (2 * 0x7FFF)) as i64
}

/// Drains a completed interrupt-IN report into the pointer (spec 0010, I14).
///
/// Called from the service opportunity and the idle loop beside the PS/2 poll.
/// A NAK leaves the TD Active, so "no report" is simply an active TD and costs
/// no bus traffic beyond the controller's next frame. An absolute report is
/// idempotent: a dropped or repeated report re-states the same position.
pub fn poll() {
    let mut guard = USB.lock();
    let usb = &mut *guard;
    if usb.state != State::Ready {
        return;
    }

    let td = unsafe { interrupt_td() };
    let status = unsafe { read_volatile(addr_of_mut!((*td).status)) };
    if status & TD_ACTIVE != 0 {
        return;
    }

    if status & TD_ERRORS != 0 {
        println!("usb: tablet report error {:#010x}; PS/2 fallback", status);
        usb.state = State::Failed;
        gfx::set_pointer_source(if mouse::ready() {
            gfx::POINTER_SOURCE_PS2
        } else {
            gfx::POINTER_SOURCE_NONE
        });
        return;
    }

    let length = actual_length(status).min(DATA_BYTES);
    if length >= REPORT_BYTES {
        let mut report = [0u8; REPORT_BYTES];
        // SAFETY: the completed IN transfer wrote `length` bytes into the
        // static data buffer; at most `REPORT_BYTES` are copied here.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data_buffer() as *const u8,
                report.as_mut_ptr(),
                REPORT_BYTES,
            );
        }
        let buttons = report[0] & 0x07;
        let x = u16::from_le_bytes([report[1], report[2]]);
        let y = u16::from_le_bytes([report[3], report[4]]);
        let (width, height) = gfx::size();
        let screen_x = scale(x, i64::from(width));
        let screen_y = scale(y, i64::from(height));
        let _ = gfx::set_pointer_absolute(screen_x, screen_y, u32::from(buttons));
    }

    // Rearm: the TD is self-linked so the QH element stays on it; writing the
    // status last makes the new token and buffer visible before the transfer
    // becomes active again.
    usb.toggle = !usb.toggle;
    let packet = usize::from(usb.max_packet);
    let td_physical = unsafe { pointer_physical(td) };
    unsafe {
        write_volatile(addr_of_mut!((*td).link), td_physical | LINK_DEPTH);
        write_volatile(
            addr_of_mut!((*td).token),
            token(PID_IN, usb.address, usb.endpoint, usb.toggle, packet),
        );
        write_volatile(addr_of_mut!((*td).buffer), pointer_physical(data_buffer()));
        write_volatile(addr_of_mut!((*td).status), TD_ACTIVE | TD_ERROR_COUNT);
    }
}

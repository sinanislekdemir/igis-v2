//! Polled PS/2 auxiliary (mouse) device (spec 0010, ADR 0021).
//!
//! The i8042 controller is initialised once at boot: the auxiliary interface is
//! enabled, its interrupt is left cleared (the kernel polls), and the device is
//! put into a known state (defaults, 100 reports/s, 4 counts/mm, 1:1 scaling,
//! reporting on). Every wait is bounded; a missing controller or device is a
//! boot log line, never a hang.
//!
//! [`poll`] drains aux bytes from the same service opportunity that polls the
//! keyboard. The keyboard path leaves aux bytes alone and this path leaves
//! keyboard bytes alone, so two devices sharing one controller buffer cost one
//! extra service opportunity at most and never lose a byte (spec 0010, I15).
//!
//! Decoding is a pure 3-byte state machine ([`Decoder`]) and the position
//! arithmetic lives in [`crate::gfx`], so the boot self-check can run canned
//! packets through the same code with scratch state and leave the real pointer
//! untouched. Motion is 1:1; acceleration is future work.
//!
//! Nothing here is exposed as raw device state: applications read
//! `syslib.pointer_x`/`pointer_y`/`pointer_buttons` and move the pointer with
//! the debug-only `syslib.inject_pointer` (spec 0010, I10–I13).

use core::sync::atomic::{AtomicBool, Ordering};

use abi::Status;
use arch_x86_64::port::{inb, outb};

use crate::gfx;
use crate::println;
use crate::sync::SpinLock;

const STATUS_PORT: u16 = 0x64;
const DATA_PORT: u16 = 0x60;

const STATUS_OUTPUT_FULL: u8 = 0x01;
const STATUS_INPUT_FULL: u8 = 0x02;
const STATUS_FROM_AUX: u8 = 0x20;

/// One complete report, already sign-extended. `dy` is the device's direction:
/// positive means **up** (spec 0010, I3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    pub dx: i32,
    pub dy: i32,
    pub buttons: u8,
}

/// The 3-byte packet assembly, a pure state machine (spec 0010, I2).
///
/// Byte 0 must have bit 3 set; while assembling, every byte is data. An
/// overflow report is dropped whole — including its buttons — because the
/// next report restates them.
#[derive(Clone, Copy)]
pub struct Decoder {
    bytes: [u8; 3],
    len: usize,
    /// Packets dropped for a missing sync bit (a lost or mixed stream).
    resyncs: u32,
    /// Packets dropped because X or Y overflowed.
    overflows: u32,
}

impl Decoder {
    pub const fn new() -> Self {
        Decoder {
            bytes: [0; 3],
            len: 0,
            resyncs: 0,
            overflows: 0,
        }
    }

    /// Feeds one byte and returns a packet when a complete, trustworthy report
    /// has been assembled.
    pub fn push(&mut self, byte: u8) -> Option<Packet> {
        if self.len == 0 && byte & 0x08 == 0 {
            // Not a first byte: drop it and stay in sync hunting.
            self.resyncs = self.resyncs.saturating_add(1);
            return None;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        if self.len < 3 {
            return None;
        }
        let first = self.bytes[0];
        let x_low = self.bytes[1];
        let y_low = self.bytes[2];
        self.len = 0;

        if first & 0x40 != 0 || first & 0x80 != 0 {
            self.overflows = self.overflows.saturating_add(1);
            return None;
        }
        Some(Packet {
            dx: delta(first & 0x10 != 0, x_low),
            dy: delta(first & 0x20 != 0, y_low),
            buttons: first & 0b111,
        })
    }
}

/// Sign-extends the 9-bit axis value: the sign bit from byte 0 plus the low 8
/// bits.
fn delta(negative: bool, low: u8) -> i32 {
    if negative {
        i32::from(low) - 256
    } else {
        i32::from(low)
    }
}

/// One decoder for the whole machine. Two cores may reach the service
/// opportunity at once; the lock serialises assembly and the pointer update
/// (spec 0010, I16).
static DECODER: SpinLock<Decoder> = SpinLock::new(Decoder::new());

/// Logs the first real packet of the boot, once (PLAN M2.2.1).
static FIRST_PACKET: AtomicBool = AtomicBool::new(false);

/// Bounded controller waits. Under TCG a retry loop of this size is a few
/// milliseconds; an absent device is reported, not waited for.
const SPIN_LIMIT: u32 = 200_000;

fn wait_input_clear() -> bool {
    for _ in 0..SPIN_LIMIT {
        // SAFETY: reading the i8042 status port has no side effects.
        if unsafe { inb(STATUS_PORT) } & STATUS_INPUT_FULL == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn wait_output_full() -> bool {
    for _ in 0..SPIN_LIMIT {
        // SAFETY: as above.
        if unsafe { inb(STATUS_PORT) } & STATUS_OUTPUT_FULL != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn write_controller(byte: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }
    // SAFETY: the controller accepts a command only when its input buffer is
    // clear, which was just observed.
    unsafe { outb(STATUS_PORT, byte) };
    true
}

/// Sends one byte to the auxiliary device through the `0xD4` prefix.
fn write_aux(byte: u8) -> bool {
    if !write_controller(0xD4) {
        return false;
    }
    if !wait_input_clear() {
        return false;
    }
    // SAFETY: the controller is routed to the aux device for one byte.
    unsafe { outb(DATA_PORT, byte) };
    true
}

/// Reads one controller byte, or `None` on timeout.
fn read_data() -> Option<u8> {
    if !wait_output_full() {
        return None;
    }
    // SAFETY: the output buffer is full, so the read consumes one byte.
    Some(unsafe { inb(DATA_PORT) })
}

/// Drains stale bytes firmware left in the controller. Boot only: no owner is
/// waiting for them yet.
fn drain_stale() {
    for _ in 0..32 {
        // SAFETY: reading the status port.
        if unsafe { inb(STATUS_PORT) } & STATUS_OUTPUT_FULL == 0 {
            break;
        }
        // SAFETY: the output buffer is full.
        let _ = unsafe { inb(DATA_PORT) };
    }
}

/// Waits for the device's ACK (`0xFA`) after a command.
fn wait_ack() -> bool {
    for _ in 0..SPIN_LIMIT {
        match read_data() {
            Some(0xFA) => return true,
            Some(_) => continue,
            None => return false,
        }
    }
    false
}

/// Sends one aux command and waits for its ACK.
fn aux_command(command: u8) -> bool {
    write_aux(command) && wait_ack()
}

/// Sends one aux command with a single data byte (sample rate, resolution).
fn aux_command_data(command: u8, data: u8) -> bool {
    write_aux(command) && wait_ack() && write_aux(data) && wait_ack()
}

/// Initialises the controller and the aux device (spec 0010, I1).
///
/// Returns false when the controller or the device does not answer; the caller
/// logs the outcome and the system continues without a pointer.
fn controller_init() -> bool {
    drain_stale();
    // Enable the auxiliary interface.
    if !write_controller(0xA8) {
        return false;
    }
    // Read-modify-write the configuration byte: clear the aux interrupt (we
    // poll) and leave the keyboard bits, including translation, alone.
    if !write_controller(0x20) {
        return false;
    }
    let Some(config) = read_data() else {
        return false;
    };
    if !write_controller(0x60) || !write_controller(config & !0x02) {
        return false;
    }
    // Device defaults, then a known reporting shape.
    aux_command(0xF6)
        && aux_command_data(0xF3, 100)
        && aux_command_data(0xE8, 3)
        && aux_command(0xE6)
        && aux_command(0xF4)
}

/// Runs one canned byte sequence through the live decoder and the live clamp
/// arithmetic, on scratch state (spec 0010, "Kernel contract").
///
/// The check is mode-independent (a virtual 1024x768 screen) so it says the
/// same thing on every boot; the real pointer and the real device are
/// untouched.
fn decoder_self_check() -> Result<(), u32> {
    const W: i64 = 1024;
    const H: i64 = 768;

    let mut decoder = Decoder::new();
    let mut x = 100i64;
    let mut y = 100i64;
    let mut step = 1u32;

    // 1. A positive move: dx 10, dy 20 (up), no buttons.
    let mut packets = Vec3::new();
    for byte in [0x08, 10, 20] {
        packets.feed(&mut decoder, byte);
    }
    let packet = packets.take().ok_or(step)?;
    (x, y) = gfx::apply_motion(x, y, i64::from(packet.dx), -i64::from(packet.dy), W, H);
    if (x, y) != (110, 80) || packet.buttons != 0 {
        return Err(step);
    }

    // 2. A sign-extended negative delta: dx -10, dy +10 (up).
    step += 1;
    for byte in [0x18, 0xF6, 0x0A] {
        packets.feed(&mut decoder, byte);
    }
    let packet = packets.take().ok_or(step)?;
    (x, y) = gfx::apply_motion(x, y, i64::from(packet.dx), -i64::from(packet.dy), W, H);
    if (x, y) != (100, 70) {
        return Err(step);
    }

    // 3. An overflow report is ignored whole.
    step += 1;
    for byte in [0x48, 0x7F, 0x7F] {
        packets.feed(&mut decoder, byte);
    }
    if packets.take().is_some() {
        return Err(step);
    }
    if (x, y) != (100, 70) {
        return Err(step);
    }

    // 4. A desynced stream recovers: two bytes without bit 3 are dropped and
    //    the packet that follows applies exactly once.
    step += 1;
    for byte in [0x00, 0x01, 0x08, 5, 5] {
        packets.feed(&mut decoder, byte);
    }
    let packet = packets.take().ok_or(step)?;
    (x, y) = gfx::apply_motion(x, y, i64::from(packet.dx), -i64::from(packet.dy), W, H);
    if (x, y) != (105, 65) || packets.take().is_some() {
        return Err(step);
    }

    // 5. Clamping at both edges: a delta past the edge stops at it.
    step += 1;
    (x, y) = gfx::apply_motion(x, y, 100_000, 100_000, W, H);
    if (x, y) != (W - 1, H - 1) {
        return Err(step);
    }
    step += 1;
    (x, y) = gfx::apply_motion(x, y, -100_000, -100_000, W, H);
    if (x, y) != (0, 0) {
        return Err(step);
    }

    // 6. Buttons come from byte 0 bits 0..2.
    step += 1;
    for byte in [0x0F, 0, 0] {
        packets.feed(&mut decoder, byte);
    }
    let packet = packets.take().ok_or(step)?;
    if packet.buttons != 0b111 {
        return Err(step);
    }

    Ok(())
}

/// A tiny fixed-capacity collector, so the self-check needs no allocator and
/// no `Vec`.
struct Vec3 {
    items: [Option<Packet>; 3],
    len: usize,
}

impl Vec3 {
    const fn new() -> Self {
        Vec3 {
            items: [None; 3],
            len: 0,
        }
    }

    fn feed(&mut self, decoder: &mut Decoder, byte: u8) {
        if let Some(packet) = decoder.push(byte) {
            if self.len < self.items.len() {
                self.items[self.len] = Some(packet);
                self.len += 1;
            }
        }
    }

    fn take(&mut self) -> Option<Packet> {
        if self.len == 0 {
            return None;
        }
        let packet = self.items[0];
        self.items[0] = self.items[1];
        self.items[1] = self.items[2];
        self.items[2] = None;
        self.len -= 1;
        packet
    }
}

/// Boot entry point: controller setup, decoder self-check, cursor.
///
/// Runs after graphics are initialised (it needs the screen size and the
/// shadow) and after the console is attached (it logs through it).
pub fn init() {
    let device = controller_init();
    if device {
        println!("mouse: PS/2 aux ready");
    } else {
        println!("mouse: no aux device");
    }

    match decoder_self_check() {
        Ok(()) => println!("mouse: decoder ok"),
        Err(step) => {
            println!("mouse: decoder FAIL at step {step}");
            arch_x86_64::halt();
        }
    }

    if device {
        gfx::show_cursor();
    }
}

/// Drains aux bytes from the controller into the decoder, applying every
/// complete report to the pointer (spec 0010, I14).
///
/// A keyboard byte is left for `keyboard::try_read_byte`, which in turn leaves
/// an aux byte for this function. The decoder lock is held across the pointer
/// update so two cores cannot apply packets out of order (I16).
pub fn poll() {
    for _ in 0..64 {
        // SAFETY: reading the i8042 status port has no side effects.
        let status = unsafe { inb(STATUS_PORT) };
        if status & STATUS_OUTPUT_FULL == 0 || status & STATUS_FROM_AUX == 0 {
            return;
        }
        // SAFETY: the output buffer holds an aux byte.
        let byte = unsafe { inb(DATA_PORT) };
        let mut decoder = DECODER.lock();
        if let Some(packet) = decoder.push(byte) {
            apply(packet);
        }
    }
}

/// Applies one decoded report to the machine-wide pointer (spec 0010, I3/I4).
fn apply(packet: Packet) {
    if !FIRST_PACKET.swap(true, Ordering::Relaxed) {
        println!(
            "mouse: first packet dx={} dy={} buttons={}",
            packet.dx, packet.dy, packet.buttons
        );
    }
    // The device's positive dy is up; the screen's positive y is down.
    // `Invalid` means graphics are disabled: there is no pointer to move and
    // no cursor to repaint, which is the documented no-screen behaviour.
    let _ = gfx::update_pointer(
        i64::from(packet.dx),
        -i64::from(packet.dy),
        u32::from(packet.buttons),
    );
}

/// Moves the pointer by screen-oriented deltas and sets the buttons
/// (`syslib.inject_pointer`, the debug-only test hook; spec 0010, I10/I12).
pub fn inject(dx: i64, dy: i64, buttons: u32) -> Status {
    gfx::update_pointer(dx, dy, buttons)
}

//! The network device facade (spec 0020, N1–N4; ADR 0048/0049/0050).
//!
//! One static [`NetState`] holds the wire [`Stack`] and one concrete device.
//! The service opportunity drains up to [`RX_DRAIN`] frames into the stack and
//! then runs the stack's timers; every transmit closure borrows the device and
//! returns whether the frame reached the driver's TX ring.
//!
//! # Polling and the INTx route
//!
//! The driver is polled in the service opportunity ([`service`], called from
//! `sched::service` and the idle loop). M10.6 adds one PCI INTx line (QEMU GSI
//! 11) routed through the I/O APIC to the boot core as
//! [`arch_x86_64::apic::NET_VECTOR`]. [`on_irq`] is the whole handler: it sets a
//! pending flag, acknowledges the device with a read-only register access and
//! EOIs the local APIC. It deliberately takes no lock, prints nothing and does
//! not drain: the kernel may be interrupted while it holds [`NET`] (or any
//! other lock), and the device rings are not safe to walk from two contexts.
//! The used-ring drain therefore stays in [`service`], which already runs on
//! every schedulable opportunity, so an interrupt is only a wake-up hint and a
//! missed one costs at most one service period. Polling remains the path when
//! no line was routed, when the boot asked for it (`--net-irq poll`) or when
//! the device refused to unmask.
//!
//! The socket and configuration wrappers are pass-throughs to the `net`
//! stack and are called by the interpreter host with the calling task as the
//! socket owner (spec 0020, N12/N14). A missing device leaves every one of
//! them `IoError`/`Invalid` rather than failing at boot.

// The interpreter host (M10.2/M10.3) calls the socket and configuration
// wrappers; keep the full device-slice surface compiled now rather than
// suppressing it one item at a time.
#![allow(dead_code)]

mod e1000;
mod virtio;

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use ::net::{Config, Stats, Stack};

use abi::Status;

use crate::clock;
use crate::pci;
use crate::println;
use crate::sync::SpinLock;

use e1000::E1000;
use virtio::VirtioNet;

/// Largest frame the driver copies out of one RX buffer (2 KiB minus nothing;
/// the virtio header is inside the buffer and stripped by the driver).
const RX_FRAME_BYTES: usize = 2048;
/// Bounded frames drained per service opportunity, so a busy link cannot starve
/// the scheduler.
const RX_DRAIN: usize = 16;

/// The machine-wide configuration as source sees it (host order).
pub struct NetInfo {
    pub ip: u32,
    pub mask: u32,
    pub gateway: u32,
    pub dns: u32,
}

/// The one concrete device this machine drives.
enum Device {
    None,
    Virtio(VirtioNet),
    E1000(E1000),
}

impl Device {
    fn transmit(&mut self, frame: &[u8]) -> Status {
        match self {
            Device::None => Status::IoError,
            Device::Virtio(device) => device.transmit(frame),
            Device::E1000(device) => device.transmit(frame),
        }
    }

    fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        match self {
            Device::None => None,
            Device::Virtio(device) => device.receive(out),
            Device::E1000(device) => device.receive(out),
        }
    }

    fn present(&self) -> bool {
        matches!(self, Device::Virtio(_) | Device::E1000(_))
    }
}

/// The stack plus the device behind one lock. Split borrows keep the transmit
/// closure (`device`) disjoint from the protocol state (`stack`).
struct NetState {
    stack: Stack,
    device: Device,
    /// Scratch frame for the service drain; static so a service opportunity
    /// does not touch 2 KiB of kernel stack.
    rx_frame: [u8; RX_FRAME_BYTES],
}

impl NetState {
    const fn new() -> Self {
        Self {
            stack: Stack::new(),
            device: Device::None,
            rx_frame: [0; RX_FRAME_BYTES],
        }
    }
}

static NET: SpinLock<NetState> = SpinLock::new(NetState::new());

// ---- The interrupt path (spec 0020, N4) -----------------------------------
//
// The handler may interrupt any core while it holds any lock, so everything it
// needs is in these atomics: which device to acknowledge and how. `init` fills
// them before the IOAPIC entry can deliver anything.

/// A NIC interrupt is waiting for the next service opportunity.
static IRQ_PENDING: AtomicBool = AtomicBool::new(false);
/// Interrupts taken so far; the service opportunity prints the first non-zero
/// value once, like the PS/2 aux line does.
static IRQ_ENTRIES: AtomicU64 = AtomicU64::new(0);
/// The one-time `net: irq delivered ...` line has been printed.
static IRQ_REPORTED: AtomicBool = AtomicBool::new(false);
/// Whether an INTx line is routed and the device is delivering on it.
static IRQ_ROUTED: AtomicBool = AtomicBool::new(false);

const ACK_NONE: u8 = 0;
const ACK_VIRTIO: u8 = 1;
const ACK_E1000: u8 = 2;

/// How the handler acknowledges the device: [`ACK_NONE`], [`ACK_VIRTIO`] (read
/// the legacy ISR byte) or [`ACK_E1000`] (read `ICR`).
static IRQ_ACK: AtomicU8 = AtomicU8::new(ACK_NONE);
/// The base the acknowledge reads: the virtio I/O BAR or the e1000 MMIO BAR.
static IRQ_ACK_BASE: AtomicU64 = AtomicU64::new(0);

/// The NIC interrupt handler (spec 0020, N4). Called from the `irq_net` stub
/// with interrupts disabled.
///
/// Bounded and lock-free by construction: a pending flag, one device register
/// read (which is the acknowledgement; both devices clear on read) and the
/// local-APIC EOI. A spurious delivery with no device configured only sets the
/// flag and EOIs. The rings are drained by [`service`], never here.
pub fn on_irq() {
    IRQ_PENDING.store(true, Ordering::Release);
    IRQ_ENTRIES.fetch_add(1, Ordering::Relaxed);

    let base = IRQ_ACK_BASE.load(Ordering::Acquire);
    match IRQ_ACK.load(Ordering::Acquire) {
        ACK_VIRTIO => virtio::acknowledge_interrupt(base as u16),
        ACK_E1000 => e1000::acknowledge_interrupt(base),
        _ => {}
    }

    // EOI after the device acknowledge: the IOAPIC entry is level-triggered, so
    // a still-asserted line is re-delivered instead of lost.
    arch_x86_64::apic::eoi();
}

/// True when the machine is interrupt-driven (a route was programmed and the
/// device accepted it).
pub fn irq_active() -> bool {
    IRQ_ROUTED.load(Ordering::Acquire)
}

/// Packs six MAC bytes big-endian (`52:54:00:12:34:56` is `0x5254_0012_3456`).
fn pack_mac(mac: [u8; 6]) -> u64 {
    (u64::from(mac[0]) << 40)
        | (u64::from(mac[1]) << 32)
        | (u64::from(mac[2]) << 24)
        | (u64::from(mac[3]) << 16)
        | (u64::from(mac[4]) << 8)
        | u64::from(mac[5])
}

/// Finds, brings up and records the NIC. A missing or failed device is a
/// normal boot line and leaves every socket call disabled.
///
/// virtio-net is tried first and wins when both devices are present; a machine
/// with only an e1000/e1000e (VMware's standard NICs) runs the second driver
/// behind the same seam. `xtask --net-device` attaches exactly one of them.
pub fn init() {
    if let Some(controller) = pci::find_virtio_net() {
        init_virtio(&controller);
        return;
    }
    if let Some(controller) = pci::find_e1000() {
        init_e1000(&controller);
        return;
    }
    println!("net: no device");
}

fn init_virtio(controller: &pci::VirtioNetController) {
    // The value must live in the static before `bring_up`: the descriptor
    // addresses written there are the addresses of its own buffers.
    let outcome = {
        let mut guard = NET.lock();
        guard.device = Device::Virtio(VirtioNet::empty(controller.io_base));
        let NetState { stack, device, .. } = &mut *guard;
        match device {
            Device::Virtio(nic) => match nic.bring_up(controller) {
                Ok(tx_ok) => {
                    let mac = nic.mac();
                    stack.set_mac(mac);
                    Some((mac, tx_ok, nic.link_up()))
                }
                Err(()) => {
                    *device = Device::None;
                    None
                }
            },
            _ => None,
        }
    };

    let Some((mac, tx_ok, link_up)) = outcome else {
        println!("net: device init failed");
        return;
    };

    // Record how an interrupt is acknowledged *before* any line can deliver:
    // the statics are visible to the handler, which must not take `NET`.
    IRQ_ACK_BASE.store(u64::from(controller.io_base), Ordering::Release);
    IRQ_ACK.store(ACK_VIRTIO, Ordering::Release);

    println!(
        "net: virtio-net {:04x}:{:04x} mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        controller.vendor, controller.device, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    if link_up {
        println!("net: link up");
    } else {
        println!("net: link down");
    }
    println!(
        "net: queues 2x{} KiB, buffers {}x2 KiB + {}x2 KiB",
        virtio::QUEUE_MEMORY_KIB,
        virtio::RX_BUFFERS,
        virtio::TX_BUFFERS
    );
    if tx_ok {
        println!("net: tx ok");
    }
}

fn init_e1000(controller: &pci::E1000Controller) {
    // Same rule as virtio: the ring and buffer addresses are taken from the
    // value's final location inside the static.
    let outcome = {
        let mut guard = NET.lock();
        guard.device = Device::E1000(E1000::empty(controller.mmio_base));
        let NetState { stack, device, .. } = &mut *guard;
        match device {
            Device::E1000(nic) => match nic.bring_up(controller) {
                Ok(tx_ok) => {
                    let mac = nic.mac();
                    stack.set_mac(mac);
                    Some(Ok((mac, nic.link_up(), tx_ok, nic.mac_from_default())))
                }
                Err(error) => {
                    *device = Device::None;
                    Some(Err(error))
                }
            },
            _ => None,
        }
    };

    let (mac, link_up, tx_ok, mac_from_default) = match outcome {
        Some(Ok(state)) => state,
        Some(Err(e1000::InitError::Mmio)) => {
            println!("net: e1000 mmio unsupported");
            return;
        }
        _ => {
            println!("net: device init failed");
            return;
        }
    };

    IRQ_ACK_BASE.store(controller.mmio_base, Ordering::Release);
    IRQ_ACK.store(ACK_E1000, Ordering::Release);

    println!(
        "net: e1000 {:04x}:{:04x} pci 00:{:02x}.{} mmio {:#010x} mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        controller.vendor,
        controller.device,
        controller.slot,
        controller.function,
        controller.mmio_base,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
    if link_up {
        println!("net: link up");
    } else {
        println!("net: link down");
    }
    println!("net: buffers 32x2 KiB + 16x2 KiB");
    if mac_from_default {
        println!("net: e1000 reported no station address; using the default");
    }
    if tx_ok {
        println!("net: tx ok");
    }
}

/// Enables the device's interrupt delivery on the already-programmed line.
///
/// `routed` is the I/O APIC outcome from boot. Returns whether the machine is
/// now interrupt-driven; `false` means the caller's polling path stays and the
/// boot log says so. The device acknowledge is armed by [`init`] regardless,
/// so a line that fires unexpectedly can always be acknowledged.
pub fn enable_irq(routed: bool) -> bool {
    if !routed {
        return false;
    }
    let mut guard = NET.lock();
    let enabled = match &mut guard.device {
        // Legacy virtio interrupts need no enable step: the device asserts the
        // line once DRIVER_OK is set and notifications are not suppressed. The
        // boot-time test frame may already have asserted it, though, so read
        // the ISR once to drop the line: the PCI level latch only turns a
        // *change* into an interrupt, and a level that is still high would
        // swallow every later notification.
        Device::Virtio(device) => {
            let _ = device.acknowledge_interrupt();
            true
        }
        Device::E1000(device) => device.enable_irq(),
        Device::None => false,
    };
    drop(guard);
    IRQ_ROUTED.store(enabled, Ordering::Release);
    enabled
}

/// Printer for the first interrupt this machine takes. Runs in the service
/// opportunity, never in the handler, and only once.
fn report_irq_once() {
    if IRQ_ROUTED.load(Ordering::Acquire) {
        let entries = IRQ_ENTRIES.load(Ordering::Relaxed);
        if entries > 0 && !IRQ_REPORTED.swap(true, Ordering::AcqRel) {
            println!("net: irq delivered {entries} interrupt(s)");
        }
    }
}

/// Drains the device and runs the stack's timers. Called from the service
/// opportunity and the idle loop, and that is where all protocol work happens:
/// the interrupt handler only sets [`IRQ_PENDING`], so a missed interrupt is
/// harmless and a spurious one costs nothing.
pub fn service() {
    // The flag is advisory: the drain below always runs (polling is the
    // fallback), and clearing it here is what makes it "handled by the service
    // opportunity". The value is only used to keep the read honest.
    let _was_pending = IRQ_PENDING.swap(false, Ordering::AcqRel);
    report_irq_once();

    let mut guard = NET.lock();
    let now = clock::now_ns();
    let NetState {
        stack,
        device,
        rx_frame,
    } = &mut *guard;

    for _ in 0..RX_DRAIN {
        match device.receive(rx_frame) {
            Some(0) => continue,
            Some(len) => {
                stack.receive(&rx_frame[..len], now, &mut |out| {
                    device.transmit(out) == Status::Ok
                });
            }
            None => break,
        }
    }
    stack.service(now, &mut |out| device.transmit(out) == Status::Ok);
}

/// Stores the machine-wide IPv4 configuration.
pub fn config(ip: u32, mask: u32, gateway: u32, dns: u32) -> Status {
    let mut guard = NET.lock();
    if !guard.device.present() {
        return Status::IoError;
    }
    guard.stack.set_config(Config {
        ip,
        mask,
        gateway,
        dns,
    });
    Status::Ok
}

/// Reads the machine-wide IPv4 configuration.
pub fn info() -> Result<NetInfo, Status> {
    let guard = NET.lock();
    if !guard.device.present() {
        return Err(Status::IoError);
    }
    let config = guard.stack.config();
    Ok(NetInfo {
        ip: config.ip,
        mask: config.mask,
        gateway: config.gateway,
        dns: config.dns,
    })
}

/// Returns the packed MAC, or `0` when no device is present.
pub fn mac() -> u64 {
    let guard = NET.lock();
    pack_mac(guard.stack.mac())
}

/// The stack's bounded protocol counters (spec 0020, N22). `IoError` when no
/// device is present, so a caller can tell "nothing happened" from "no NIC".
pub fn stats() -> Result<Stats, Status> {
    let guard = NET.lock();
    if !guard.device.present() {
        return Err(Status::IoError);
    }
    Ok(guard.stack.stats())
}

/// Opens a socket (spec 0020, N12); TCP starts in SYN-SENT and `service`
/// drives the handshake.
pub fn open(kind: u64, ip: u32, port: u16, local: u16, owner: u64) -> Result<u64, Status> {
    let mut guard = NET.lock();
    if !guard.device.present() {
        return Err(Status::IoError);
    }
    guard.stack.open(kind, ip, port, local, owner)
}

/// Sends at most one segment or datagram; `WouldBlock` when the stack cannot
/// take it yet, so the caller retries.
pub fn send(handle: u64, owner: u64, data: &[u8]) -> Result<usize, Status> {
    let mut guard = NET.lock();
    if !guard.device.present() {
        return Err(Status::IoError);
    }
    let now = clock::now_ns();
    let NetState { stack, device, .. } = &mut *guard;
    stack.send(handle, owner, data, now, &mut |out| {
        device.transmit(out) == Status::Ok
    })
}

/// Receives queued socket data; `Ok(0)` is EOF and `WouldBlock` means nothing
/// is queued on a live connection.
pub fn recv(handle: u64, owner: u64, dst: &mut [u8]) -> Result<usize, Status> {
    let mut guard = NET.lock();
    if !guard.device.present() {
        return Err(Status::IoError);
    }
    guard.stack.recv(handle, owner, dst)
}

/// Closes a socket owned by `owner` (FIN for TCP, immediate for UDP).
pub fn close(handle: u64, owner: u64) -> Status {
    let mut guard = NET.lock();
    if !guard.device.present() {
        return Status::IoError;
    }
    let now = clock::now_ns();
    let NetState { stack, device, .. } = &mut *guard;
    stack.close(handle, owner, now, &mut |out| {
        device.transmit(out) == Status::Ok
    })
}

/// Pops the oldest completed connection from a listener (spec 0020, N10). The
/// returned handle is a normal connected socket owned by the listener's task.
pub fn accept(handle: u64, owner: u64) -> Result<u64, Status> {
    let mut guard = NET.lock();
    if !guard.device.present() {
        return Err(Status::IoError);
    }
    guard.stack.accept(handle, owner)
}

/// Reports whether a socket is readable (data, EOF, error, a completed
/// connect or a listener with a pending connection).
pub fn readable(handle: u64, owner: u64) -> bool {
    let guard = NET.lock();
    guard.stack.readable(handle, owner)
}

/// Closes every socket owned by a terminating task (spec 0020, N14). Called
/// from `Task::reclaim` before the task's regions are freed.
pub fn close_task(owner: u64) {
    let mut guard = NET.lock();
    let now = clock::now_ns();
    let NetState { stack, device, .. } = &mut *guard;
    stack.close_owner(owner, now, &mut |out| device.transmit(out) == Status::Ok);
}

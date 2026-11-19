//! The network device facade (spec 0020, N1–N4; ADR 0048/0049/0050).
//!
//! One static [`NetState`] holds the wire [`Stack`] and one concrete device.
//! The service opportunity drains up to [`RX_DRAIN`] frames into the stack and
//! then runs the stack's timers; every transmit closure borrows the device and
//! returns whether the frame reached the driver's TX ring.
//!
//! The socket and configuration wrappers are pass-throughs to the `net`
//! stack and are called by the interpreter host with the calling task as the
//! socket owner (spec 0020, N12/N14). A missing device leaves every one of
//! them `IoError`/`Invalid` rather than failing at boot.

// The interpreter host (M10.2/M10.3) calls the socket and configuration
// wrappers; keep the full device-slice surface compiled now rather than
// suppressing it one item at a time.
#![allow(dead_code)]

mod virtio;

use ::net::{Config, Stack};

use abi::Status;

use crate::clock;
use crate::pci;
use crate::println;
use crate::sync::SpinLock;

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
}

impl Device {
    fn transmit(&mut self, frame: &[u8]) -> Status {
        match self {
            Device::None => Status::IoError,
            Device::Virtio(device) => device.transmit(frame),
        }
    }

    fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        match self {
            Device::None => None,
            Device::Virtio(device) => device.receive(out),
        }
    }

    fn present(&self) -> bool {
        matches!(self, Device::Virtio(_))
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
pub fn init() {
    let Some(controller) = pci::find_virtio_net() else {
        println!("net: no device");
        return;
    };

    // The value must live in the static before `bring_up`: the descriptor
    // addresses written there are the addresses of its own buffers.
    let outcome = {
        let mut guard = NET.lock();
        guard.device = Device::Virtio(VirtioNet::empty(controller.io_base));
        let NetState { stack, device, .. } = &mut *guard;
        match device {
            Device::Virtio(nic) => match nic.bring_up(&controller) {
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
            Device::None => None,
        }
    };

    let Some((mac, tx_ok, link_up)) = outcome else {
        println!("net: device init failed");
        return;
    };

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

/// Drains the device and runs the stack's timers. Called from the service
/// opportunity and the idle loop.
pub fn service() {
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

/// Reports whether a socket is readable (data, EOF, error or a completed
/// connect).
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

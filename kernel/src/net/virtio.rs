//! Legacy (transitional) virtio-net over I/O BAR0 (spec 0020, N1–N4; ADR 0049).
//!
//! One RX and one TX split virtqueue, static descriptor/avail/used tables and
//! 32 + 16 fixed 2 KiB frame buffers. Everything lives in `.bss` inside
//! [`VirtioNet`], which the kernel stores in a static lock, so the buffers are
//! addressed once and never move.
//!
//! # DMA addresses
//!
//! The kernel is linked at [`KERNEL_BASE`](arch_x86_64::KERNEL_BASE) and the
//! boot page tables alias the first GiB there, exactly as `usb.rs` and
//! `ioapic.rs` rely on. A static's physical address is therefore
//! `virtual - KERNEL_BASE`; the queue PFN and every descriptor address use that
//! value.
//!
//! # Queue layout (legacy split ring)
//!
//! ```text
//! base + 0                    descriptors, 16 * size bytes
//! base + 16 * size            available ring, 6 + 2 * size bytes
//! align_up(previous, 4096)    used ring, 6 + 8 * size bytes
//! ```
//!
//! The used ring offset is computed from the size the device reports, because
//! that is how the device computes it (QEMU reports 256 for both queues). The
//! whole block is one page-aligned [`QueueMemory`]; each queue's reported size
//! must fit [`MAX_QUEUE_SIZE`] and at least the fixed buffer count.
//!
//! No offloads are negotiated: every frame carries the 10-byte
//! `virtio_net_hdr`, and the stack computes its own checksums.

use core::sync::atomic::{compiler_fence, Ordering};

use arch_x86_64::port::{inb, inl, inw, outb, outl, outw};
use arch_x86_64::KERNEL_BASE;

use abi::Status;

use crate::pci::VirtioNetController;

// ---- Legacy virtio PCI registers (BAR0) -----------------------------------

const REG_DEVICE_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SELECT: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
const REG_ISR: u16 = 0x13;
const REG_MAC: u16 = 0x14;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

/// `VIRTIO_NET_F_MAC`: the device configuration holds a MAC address.
const FEATURE_MAC: u32 = 1 << 5;

/// `VRING_DESC_F_WRITE`: the device writes into the buffer.
const DESC_F_WRITE: u16 = 2;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

const MAX_QUEUE_SIZE: usize = 256;
/// One page-aligned block per queue: descriptors, avail ring, used ring.
const QUEUE_MEMORY_BYTES: usize = 16 * 1024;
/// KiB per queue block, for the boot log.
pub const QUEUE_MEMORY_KIB: usize = QUEUE_MEMORY_BYTES / 1024;
const FRAME_BYTES: usize = 2048;
const VIRTIO_NET_HDR: usize = 10;
pub const RX_BUFFERS: usize = 32;
pub const TX_BUFFERS: usize = 16;

/// The QEMU default, used when the device does not offer `VIRTIO_NET_F_MAC`.
const DEFAULT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Bounded status wait; reset and FEATURES_OK complete immediately on QEMU.
const STATUS_SPINS: usize = 100_000;

const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// Physical address of an identity-mapped kernel static (see the module docs).
fn physical(virtual_address: u64) -> u64 {
    debug_assert!(virtual_address >= KERNEL_BASE);
    virtual_address - KERNEL_BASE
}

/// One page-aligned split-virtqueue memory block.
#[repr(C, align(4096))]
struct QueueMemory {
    bytes: [u8; QUEUE_MEMORY_BYTES],
}

impl QueueMemory {
    const fn new() -> Self {
        Self {
            bytes: [0; QUEUE_MEMORY_BYTES],
        }
    }

    fn base_mut(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr()
    }

    fn base(&self) -> *const u8 {
        self.bytes.as_ptr()
    }
}

/// One split virtqueue: its memory block plus the local indices.
struct Queue {
    memory: QueueMemory,
    index: u16,
    size: u16,
    /// Offset of the used ring from the memory base; page-aligned.
    used_offset: usize,
    /// Next available-ring index (free-running `u16`).
    avail_next: u16,
    /// Next used-ring index we have not consumed yet (free-running `u16`).
    used_seen: u16,
}

impl Queue {
    const fn new(index: u16) -> Self {
        Self {
            memory: QueueMemory::new(),
            index,
            size: 0,
            used_offset: 0,
            avail_next: 0,
            used_seen: 0,
        }
    }

    fn reset(&mut self, size: u16) {
        self.size = size;
        self.avail_next = 0;
        self.used_seen = 0;
        self.used_offset = align_up(16 * size as usize + 6 + 2 * size as usize, 4096);
        self.memory.bytes.fill(0);
    }

    /// Writes one 16-byte descriptor (`addr`, `len`, `flags`, `next = 0`).
    fn write_descriptor(&mut self, index: u16, address: u64, len: u32, flags: u16) {
        let base = self.memory.base_mut();
        let offset = 16 * index as usize;
        // SAFETY: `index` is below the descriptor count the device reported
        // (checked in `setup_queue`), and a descriptor is a naturally aligned
        // 16-byte record inside the page-aligned block.
        unsafe {
            let pointer = base.add(offset);
            core::ptr::write_volatile(pointer as *mut u64, address);
            core::ptr::write_volatile(pointer.add(8) as *mut u32, len);
            core::ptr::write_volatile(pointer.add(12) as *mut u16, flags);
            core::ptr::write_volatile(pointer.add(14) as *mut u16, 0);
        }
    }

    /// Publishes a descriptor to the available ring and notifies the device.
    fn publish(&mut self, index: u16, address: u64, len: u32, flags: u16, io_base: u16) {
        let size = self.size as usize;
        self.write_descriptor(index, address, len, flags);

        let base = self.memory.base_mut();
        let slot = self.avail_next as usize % size;
        // SAFETY: the available ring follows the descriptor table in the same
        // page-aligned block, `slot < size`, and `size` was read from the
        // device in `setup_queue`.
        unsafe {
            core::ptr::write_volatile(base.add(16 * size + 4 + 2 * slot) as *mut u16, index);
            compiler_fence(Ordering::SeqCst);
            self.avail_next = self.avail_next.wrapping_add(1);
            core::ptr::write_volatile(base.add(16 * size + 2) as *mut u16, self.avail_next);
            compiler_fence(Ordering::SeqCst);
        }

        // SAFETY: the notify register is part of the legacy I/O window.
        unsafe { outw(io_base + REG_QUEUE_NOTIFY, self.index) };
    }

    /// Reads the device-written used-ring index.
    fn used_index(&self) -> u16 {
        let base = self.memory.base();
        // SAFETY: the used ring is inside the block at `used_offset`; the
        // field is 2-byte aligned and updated by the device, so it is read
        // volatile.
        unsafe { core::ptr::read_volatile(base.add(self.used_offset + 2) as *const u16) }
    }

    /// Reads one used-ring element (`id`, `len`).
    fn used_element(&self, slot: u16) -> (u32, u32) {
        let base = self.memory.base();
        let offset = self.used_offset + 4 + 8 * slot as usize;
        // SAFETY: `slot < size` and the used ring is inside the block.
        unsafe {
            let pointer = base.add(offset);
            (
                core::ptr::read_volatile(pointer as *const u32),
                core::ptr::read_volatile(pointer.add(4) as *const u32),
            )
        }
    }
}

/// One legacy virtio-net device with its two queues and fixed frame buffers.
pub struct VirtioNet {
    io_base: u16,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
    rx_buffers: [[u8; FRAME_BYTES]; RX_BUFFERS],
    tx_buffers: [[u8; FRAME_BYTES]; TX_BUFFERS],
    /// Bit `i` is set while TX buffer `i` is owned by the device (16 buffers).
    tx_in_use: u32,
    link_up: bool,
}

fn read_status(io_base: u16) -> u8 {
    // SAFETY: the status register is part of the legacy I/O window.
    unsafe { inb(io_base + REG_STATUS) }
}

fn write_status(io_base: u16, status: u8) {
    // SAFETY: as above.
    unsafe { outb(io_base + REG_STATUS, status) };
}

fn wait_for_status(io_base: u16, expected: u8) -> bool {
    for _ in 0..STATUS_SPINS {
        if read_status(io_base) == expected {
            return true;
        }
    }
    false
}

/// Selects one queue, reads its size, and publishes the page frame number.
fn setup_queue(io_base: u16, index: u16, queue: &mut Queue) -> Result<(), ()> {
    // SAFETY: queue select/size/PFN are legacy I/O registers.
    unsafe { outw(io_base + REG_QUEUE_SELECT, index) };
    // SAFETY: as above.
    let size = unsafe { inw(io_base + REG_QUEUE_SIZE) };
    if size == 0 || size as usize > MAX_QUEUE_SIZE || size < RX_BUFFERS as u16 {
        return Err(());
    }
    queue.reset(size);

    let address = physical(queue.memory.base_mut() as *mut u8 as u64);
    // The legacy interface takes a page frame number, not a byte address.
    // SAFETY: as above.
    unsafe { outl(io_base + REG_QUEUE_PFN, (address >> 12) as u32) };
    Ok(())
}

impl VirtioNet {
    /// A blank device value; [`bring_up`](Self::bring_up) fills it in place.
    ///
    /// The kernel must store this in its static lock *before* calling
    /// `bring_up`: the descriptor addresses written there are the addresses of
    /// these buffers, and moving the value afterwards would invalidate them.
    pub const fn empty(io_base: u16) -> Self {
        Self {
            io_base,
            mac: DEFAULT_MAC,
            rx: Queue::new(RX_QUEUE),
            tx: Queue::new(TX_QUEUE),
            rx_buffers: [[0; FRAME_BYTES]; RX_BUFFERS],
            tx_buffers: [[0; FRAME_BYTES]; TX_BUFFERS],
            tx_in_use: 0,
            link_up: false,
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn link_up(&self) -> bool {
        self.link_up
    }

    /// Resets the device, negotiates the MAC feature, brings both queues up and
    /// queues one harmless test frame.
    ///
    /// `Ok(tx_ok)` reports whether the test frame reached the TX available
    /// ring; `Err(())` means the device did not answer as a legacy virtio-net
    /// device should, and the caller falls back to no device.
    pub fn bring_up(&mut self, controller: &VirtioNetController) -> Result<bool, ()> {
        self.io_base = controller.io_base;
        let io = self.io_base;

        // Reset and wait for the device to acknowledge it.
        write_status(io, 0);
        if !wait_for_status(io, 0) {
            return Err(());
        }

        let mut status = STATUS_ACKNOWLEDGE;
        write_status(io, status);
        status |= STATUS_DRIVER;
        write_status(io, status);

        // SAFETY: the feature registers are part of the legacy I/O window.
        let device_features = unsafe { inl(io + REG_DEVICE_FEATURES) };
        let mut guest_features = 0u32;
        if device_features & FEATURE_MAC != 0 {
            guest_features |= FEATURE_MAC;
        }
        // SAFETY: as above.
        unsafe { outl(io + REG_GUEST_FEATURES, guest_features) };

        // FEATURES_OK belongs to virtio 1.0; a pure 0.9.5 device clears it.
        // The transitional QEMU device accepts it. Continue either way: none of
        // the features we care about require the bit.
        write_status(io, status | STATUS_FEATURES_OK);
        if read_status(io) & STATUS_FEATURES_OK != 0 {
            status |= STATUS_FEATURES_OK;
        } else {
            write_status(io, status);
        }

        if guest_features & FEATURE_MAC != 0 {
            for i in 0..6u16 {
                // SAFETY: the legacy device configuration starts with the
                // six-byte MAC at 0x14.
                self.mac[i as usize] = unsafe { inb(io + REG_MAC + i) };
            }
        } else {
            self.mac = DEFAULT_MAC;
        }

        setup_queue(io, RX_QUEUE, &mut self.rx)?;
        setup_queue(io, TX_QUEUE, &mut self.tx)?;
        self.rx_prefill();

        write_status(io, status | STATUS_DRIVER_OK);
        self.link_up = true;

        // Prove the TX queue path without a protocol: a 60-byte broadcast frame
        // with the local experimental ethertype and a zero payload. Slirp
        // ignores it.
        let mut test = [0u8; 60];
        test[0..6].fill(0xFF);
        test[6..12].copy_from_slice(&self.mac);
        test[12] = 0x88;
        test[13] = 0xB5;
        Ok(self.transmit(&test) == Status::Ok)
    }

    /// Fills the RX available ring with all fixed buffers.
    fn rx_prefill(&mut self) {
        for i in 0..RX_BUFFERS {
            let address = physical(self.rx_buffers[i].as_mut_ptr() as u64);
            self.rx.publish(
                i as u16,
                address,
                FRAME_BYTES as u32,
                DESC_F_WRITE,
                self.io_base,
            );
        }
    }

    /// Reclaims completed TX descriptors.
    pub fn poll(&mut self) {
        let used = self.tx.used_index();
        let mut remaining = TX_BUFFERS;
        while self.tx.used_seen != used && remaining > 0 {
            let slot = self.tx.used_seen % self.tx.size;
            let (id, _len) = self.tx.used_element(slot);
            if (id as usize) < TX_BUFFERS {
                self.tx_in_use &= !(1 << id);
            }
            self.tx.used_seen = self.tx.used_seen.wrapping_add(1);
            remaining -= 1;
        }
    }

    /// Queues one Ethernet frame (without the virtio header).
    pub fn transmit(&mut self, frame: &[u8]) -> Status {
        self.poll();

        if frame.len() + VIRTIO_NET_HDR > FRAME_BYTES {
            return Status::CapacityExceeded;
        }
        let Some(index) = (0..TX_BUFFERS).find(|i| self.tx_in_use & (1 << i) == 0) else {
            return Status::Full;
        };
        self.tx_in_use |= 1 << index;

        let buffer = &mut self.tx_buffers[index];
        buffer[..VIRTIO_NET_HDR].fill(0);
        buffer[VIRTIO_NET_HDR..VIRTIO_NET_HDR + frame.len()].copy_from_slice(frame);
        let address = physical(buffer.as_ptr() as u64);
        let len = (VIRTIO_NET_HDR + frame.len()) as u32;

        self.tx.publish(index as u16, address, len, 0, self.io_base);
        Status::Ok
    }

    /// Copies one received frame into `out`; `None` when the RX ring is empty.
    ///
    /// A frame larger than `out` is truncated (the service buffer is 2 KiB, so
    /// this cannot happen for a valid Ethernet frame). A malformed used entry
    /// is skipped and reported as a zero-length frame.
    pub fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        let used = self.rx.used_index();
        if used == self.rx.used_seen {
            return None;
        }

        let slot = self.rx.used_seen % self.rx.size;
        let (id, len) = self.rx.used_element(slot);
        self.rx.used_seen = self.rx.used_seen.wrapping_add(1);

        if id as usize >= RX_BUFFERS {
            return Some(0);
        }

        let len = (len as usize).min(FRAME_BYTES);
        if len < VIRTIO_NET_HDR {
            // No payload to copy, but the buffer is still ours to return.
            let address = physical(self.rx_buffers[id as usize].as_mut_ptr() as u64);
            self.rx.publish(
                id as u16,
                address,
                FRAME_BYTES as u32,
                DESC_F_WRITE,
                self.io_base,
            );
            return Some(0);
        }

        // Copy the payload out *before* returning the buffer to the device: a
        // republished descriptor may be refilled immediately.
        let payload = len - VIRTIO_NET_HDR;
        let copied = payload.min(out.len());
        out[..copied].copy_from_slice(
            &self.rx_buffers[id as usize][VIRTIO_NET_HDR..VIRTIO_NET_HDR + copied],
        );

        let address = physical(self.rx_buffers[id as usize].as_mut_ptr() as u64);
        self.rx.publish(
            id as u16,
            address,
            FRAME_BYTES as u32,
            DESC_F_WRITE,
            self.io_base,
        );
        Some(copied)
    }

    /// Reads and clears the ISR byte (useful when M10.6 adds the INTx route).
    #[allow(dead_code)]
    pub fn acknowledge_interrupt(&mut self) -> u8 {
        // SAFETY: the ISR register is part of the legacy I/O window.
        unsafe { inb(self.io_base + REG_ISR) }
    }
}

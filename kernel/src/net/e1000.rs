//! Intel 82540EM (`e1000`) and 82574L (`e1000e`) over MMIO BAR0 (spec 0020,
//! N2; ADR 0049).
//!
//! This is the second driver behind the `net` device seam. VMware's standard
//! virtual NICs are `e1000`/`e1000e` (there is no virtio-net there), so the same
//! Ethernet/IP/UDP/TCP stack and the same `/lib` units must run over this
//! register set unchanged. The driver is static and allocation-free like
//! [`super::virtio`]: one value lives in the kernel's static lock and owns the
//! two descriptor rings and all frame buffers.
//!
//! # MMIO
//!
//! `boot.s` identity-maps the first 4 GiB with 2 MiB pages, so BAR0 is already
//! reachable at its physical address, but as *cacheable* RAM. Device registers
//! must be uncached: [`map_mmio`] splits the containing 2 MiB page into a static
//! 4 KiB table (the same trick `arch_x86_64::ioapic` uses) and marks the register
//! pages `PWT | PCD`. This runs from `net::init` on the boot core before the APs
//! start, so no other core holds a stale translation.
//!
//! # DMA addresses
//!
//! The kernel is linked at [`KERNEL_BASE`] and aliases the first GiB there, so a
//! static's physical address is `virtual - KERNEL_BASE`. The ring base registers
//! and every descriptor buffer address use that value. The addresses are taken
//! *after* [`E1000::empty`] has been stored in the static lock (see
//! [`E1000::bring_up`]), exactly like `VirtioNet`.
//!
//! # Rings
//!
//! Legacy (not extended) descriptors, 16 bytes each:
//!
//! ```text
//! RX: addr u64 | length u16 | checksum u16 | status u8 | errors u8 | special u16
//! TX: addr u64 | length u16 | cso u8 | cmd u8 | status u8 | css u8 | special u16
//! ```
//!
//! 32 RX descriptors with 2 KiB buffers and 16 TX descriptors with 2 KiB
//! buffers. Receive is one frame per call; the service opportunity loops. No
//! offloads are enabled, so the stack keeps computing its own checksums; only
//! the Ethernet CRC is handled by the device (`SECRC` in, `IFCS` out).
//!
//! Polling only: M10.6's INTx route belongs to virtio-net; this slice does not
//! claim an interrupt line. `ICR` is cleared and every interrupt is masked at
//! reset.

use core::sync::atomic::{compiler_fence, fence, Ordering};

use arch_x86_64::{paging, KERNEL_BASE};

use abi::Status;

use crate::clock;
use crate::pci::E1000Controller;

// ---- Register offsets (Intel 82540EM datasheet) ---------------------------

const CTRL: u32 = 0x00000;
const STATUS: u32 = 0x00008;
const ICR: u32 = 0x000C0;
const IMC: u32 = 0x000D8;
const RCTL: u32 = 0x00100;
const TCTL: u32 = 0x00400;
const RDBAL: u32 = 0x02800;
const RDBAH: u32 = 0x02804;
const RDLEN: u32 = 0x02808;
const RDH: u32 = 0x02810;
const RDT: u32 = 0x02818;
const TDBAL: u32 = 0x03800;
const TDBAH: u32 = 0x03804;
const TDLEN: u32 = 0x03808;
const TDH: u32 = 0x03810;
const TDT: u32 = 0x03818;
const MTA: u32 = 0x05200;
const RAL: u32 = 0x05400;
const RAH: u32 = 0x05404;

/// `CTRL.RST`: device reset (self-clearing).
const CTRL_RST: u32 = 0x0400_0000;
/// `CTRL.SLU`: set link up.
const CTRL_SLU: u32 = 0x0000_0040;
/// `STATUS.LU`: link is up.
const STATUS_LU: u32 = 0x0000_0002;

/// `RCTL`: receiver enable, store bad packets, unicast/multicast promiscuous,
/// broadcast accept, 2 KiB buffers (`BSIZE` bits 16:17 = 0), strip CRC.
const RCTL_VALUE: u32 = 0x2 | 0x4 | 0x8 | 0x10 | 0x8000 | 0x0400_0000;
/// `TCTL`: transmitter enable, pad short packets, collision threshold 0x10,
/// collision distance 0x40.
const TCTL_VALUE: u32 = 0x2 | 0x8 | 0x0000_0100 | 0x0000_0200;

/// `RXDESC.STATUS.DD`: the descriptor has been written by the device.
const RX_STATUS_DD: u8 = 0x01;
/// `RXDESC.STATUS.EOP`: end of packet (one buffer per frame at 2 KiB).
const RX_STATUS_EOP: u8 = 0x02;

/// `TXDESC.CMD`: end of packet, insert FCS, report status.
const TX_CMD: u8 = 0x01 | 0x02 | 0x08;
/// `TXDESC.STATUS.DD`: the device is done with the descriptor.
const TX_STATUS_DD: u8 = 0x01;

const RX_RING: usize = 32;
const TX_RING: usize = 16;
const FRAME_BYTES: usize = 2048;

/// Register pages marked uncached: offsets 0x0000..0x7FFF, which covers every
/// register this driver touches (the highest is `RAH` at 0x5404).
const MMIO_PAGES: u64 = 8;

/// Bounded reset/link waits. QEMU completes both immediately; a real device
/// needs a few milliseconds.
const RESET_TIMEOUT_NS: u64 = 100_000_000;
const LINK_TIMEOUT_NS: u64 = 200_000_000;
/// How long an all-zero station address is retried after reset.
const MAC_TIMEOUT_NS: u64 = 20_000_000;

/// The QEMU/VMware default, used when the device reports no station address.
const DEFAULT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

// ---- Descriptors and buffers ----------------------------------------------

/// A legacy receive descriptor (16 bytes, 16-byte aligned).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct RxDescriptor {
    address: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

impl RxDescriptor {
    const ZERO: Self = Self {
        address: 0,
        length: 0,
        checksum: 0,
        status: 0,
        errors: 0,
        special: 0,
    };
}

/// A legacy transmit descriptor (16 bytes, 16-byte aligned).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct TxDescriptor {
    address: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

impl TxDescriptor {
    const ZERO: Self = Self {
        address: 0,
        length: 0,
        cso: 0,
        cmd: 0,
        status: 0,
        css: 0,
        special: 0,
    };
}

/// One 2 KiB frame buffer, aligned for the device.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Frame([u8; FRAME_BYTES]);

impl Frame {
    const ZERO: Self = Self([0; FRAME_BYTES]);
}

// ---- MMIO mapping ---------------------------------------------------------

/// One 4 KiB page table for the split of the 2 MiB page that holds BAR0.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut MMIO_TABLE: PageTable = PageTable([0; 512]);

fn table_physical() -> u64 {
    // SAFETY: `addr_of!` forms a raw pointer without reading the static.
    (core::ptr::addr_of!(MMIO_TABLE) as *const u8 as u64) - KERNEL_BASE
}

/// Makes the device register pages uncached, or `false` when BAR0 is outside
/// the identity map or the page tables cannot be split.
fn map_mmio(base: u64) -> bool {
    if base >= paging::MAX_PHYS {
        return false;
    }
    let page = base & !(paging::HUGE_SIZE - 1);
    if paging::table_phys(page).is_none() {
        // SAFETY: the table is a static 4 KiB-aligned page in `.bss`, not used
        // by anything else, and `page` is inside the identity map.
        if !unsafe { paging::split_huge_page(page, table_physical()) } {
            return false;
        }
    }
    for index in 0..MMIO_PAGES {
        if !paging::set_pte_uncached(base + index * paging::PAGE_SIZE) {
            return false;
        }
    }
    for index in 0..MMIO_PAGES {
        paging::flush_page(base + index * paging::PAGE_SIZE);
    }
    paging::reload_cr3();
    fence(Ordering::SeqCst);
    true
}

// ---- The device -----------------------------------------------------------

fn read32(base: u64, offset: u32) -> u32 {
    // SAFETY: `offset` is one of the 32-bit registers above and `base` is the
    // uncached BAR0 mapping established by `map_mmio`.
    unsafe { core::ptr::read_volatile((base + u64::from(offset)) as *const u32) }
}

fn write32(base: u64, offset: u32, value: u32) {
    // SAFETY: as above.
    unsafe { core::ptr::write_volatile((base + u64::from(offset)) as *mut u32, value) };
}

/// Physical address of a kernel static (see the module docs).
fn physical(virtual_address: u64) -> u64 {
    debug_assert!(virtual_address >= KERNEL_BASE);
    virtual_address - KERNEL_BASE
}

/// Why a device could not be brought up.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    /// BAR0 is at or above the identity map, or its pages cannot be made
    /// uncached: the registers cannot be touched at all.
    Mmio,
    /// The device did not acknowledge reset or could not be configured.
    Device,
}

/// One e1000-family device with its rings and frame buffers.
pub struct E1000 {
    mmio: u64,
    mac: [u8; 6],
    rx_ring: [RxDescriptor; RX_RING],
    tx_ring: [TxDescriptor; TX_RING],
    rx_buffers: [Frame; RX_RING],
    tx_buffers: [Frame; TX_RING],
    /// Next RX descriptor the software consumer expects to be done.
    rx_next: usize,
    /// Next TX descriptor the software producer will fill. `TDT` is always
    /// `(tx_next)` after a transmit, so this cursor only moves forward.
    tx_next: usize,
    /// Bit `i` is set while TX descriptor `i` is owned by the device.
    tx_in_use: u16,
    /// The device reported an all-zero station address and the fallback is in
    /// use; the boot log says so instead of hiding it.
    mac_from_default: bool,
    link_up: bool,
}

impl E1000 {
    /// A blank device value; [`bring_up`](Self::bring_up) fills it in place.
    ///
    /// The kernel must store this in its static lock *before* calling
    /// `bring_up`: the ring and buffer addresses written there are the
    /// addresses of these fields, and moving the value afterwards would
    /// invalidate them.
    pub const fn empty(mmio: u64) -> Self {
        Self {
            mmio,
            mac: DEFAULT_MAC,
            rx_ring: [RxDescriptor::ZERO; RX_RING],
            tx_ring: [TxDescriptor::ZERO; TX_RING],
            rx_buffers: [Frame::ZERO; RX_RING],
            tx_buffers: [Frame::ZERO; TX_RING],
            rx_next: 0,
            tx_next: 0,
            tx_in_use: 0,
            mac_from_default: false,
            link_up: false,
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn link_up(&self) -> bool {
        self.link_up
    }

    /// True when the device reported no station address and the driver used the
    /// documented fallback.
    pub fn mac_from_default(&self) -> bool {
        self.mac_from_default
    }

    /// Resets the device, sizes both rings, and queues one harmless test frame.
    ///
    /// `Ok(tx_ok)` means the registers answered and the rings are live;
    /// `tx_ok` reports whether the test frame reached the TX tail. An `Err`
    /// means the device cannot be driven and the caller falls back to none.
    pub fn bring_up(&mut self, controller: &E1000Controller) -> Result<bool, InitError> {
        self.mmio = controller.mmio_base;
        if !map_mmio(self.mmio) {
            return Err(InitError::Mmio);
        }
        let mmio = self.mmio;

        // Reset. RST is self-clearing; a device that never clears it is not one
        // we can drive.
        write32(mmio, CTRL, read32(mmio, CTRL) | CTRL_RST);
        if !wait_clear(mmio, CTRL, CTRL_RST) {
            return Err(InitError::Device);
        }

        // No interrupts in this slice: clear the cause and mask everything.
        write32(mmio, ICR, u32::MAX);
        write32(mmio, IMC, u32::MAX);

        // Set link up (`STATUS.LU` reports the result) and read the station
        // address the device loaded from its EEPROM.
        write32(mmio, CTRL, read32(mmio, CTRL) | CTRL_SLU);
        match read_mac(mmio) {
            Some(mac) => self.mac = mac,
            None => {
                self.mac = DEFAULT_MAC;
                self.mac_from_default = true;
            }
        }

        // Clear the 128-dword multicast table.
        for index in 0..128u32 {
            write32(mmio, MTA + index * 4, 0);
        }

        self.rx_next = 0;
        self.tx_next = 0;
        self.tx_in_use = 0;
        self.setup_receive();
        self.setup_transmit();

        self.link_up = wait_set(mmio, STATUS, STATUS_LU, LINK_TIMEOUT_NS);

        // Prove the TX path without a protocol: a 60-byte broadcast frame with
        // the local experimental ethertype and a zero payload. Slirp ignores it.
        let mut test = [0u8; 60];
        test[0..6].fill(0xFF);
        test[6..12].copy_from_slice(&self.mac);
        test[12] = 0x88;
        test[13] = 0xB5;
        Ok(self.transmit(&test) == Status::Ok)
    }

    /// Programs the RX ring and enables the receiver.
    fn setup_receive(&mut self) {
        let mmio = self.mmio;
        for index in 0..RX_RING {
            let address = physical(self.rx_buffers[index].0.as_ptr() as u64);
            let descriptor = &mut self.rx_ring[index];
            descriptor.address = address;
            descriptor.length = 0;
            descriptor.checksum = 0;
            descriptor.status = 0;
            descriptor.errors = 0;
            descriptor.special = 0;
        }
        compiler_fence(Ordering::SeqCst);

        let base = physical(self.rx_ring.as_ptr() as u64);
        write32(mmio, RDBAL, base as u32);
        write32(mmio, RDBAH, (base >> 32) as u32);
        write32(mmio, RDLEN, (RX_RING * 16) as u32);
        write32(mmio, RDH, 0);
        // The descriptor at RDT is software-owned; descriptors [RDH, RDT) are
        // hardware-owned, so RDT = N-1 hands over N-1 buffers.
        write32(mmio, RDT, (RX_RING - 1) as u32);
        write32(mmio, RCTL, RCTL_VALUE);
    }

    /// Programs the TX ring and enables the transmitter.
    fn setup_transmit(&mut self) {
        let mmio = self.mmio;
        for index in 0..TX_RING {
            let address = physical(self.tx_buffers[index].0.as_ptr() as u64);
            let descriptor = &mut self.tx_ring[index];
            descriptor.address = address;
            descriptor.length = 0;
            descriptor.cso = 0;
            descriptor.cmd = 0;
            descriptor.status = 0;
            descriptor.css = 0;
            descriptor.special = 0;
        }
        compiler_fence(Ordering::SeqCst);

        let base = physical(self.tx_ring.as_ptr() as u64);
        write32(mmio, TDBAL, base as u32);
        write32(mmio, TDBAH, (base >> 32) as u32);
        write32(mmio, TDLEN, (TX_RING * 16) as u32);
        write32(mmio, TDH, 0);
        write32(mmio, TDT, 0);
        write32(mmio, TCTL, TCTL_VALUE);
    }

    /// Reclaims every TX descriptor the device has finished with.
    pub fn poll(&mut self) {
        for index in 0..TX_RING {
            if self.tx_in_use & (1 << index) == 0 {
                continue;
            }
            // SAFETY: `index` is inside the ring; the device owns the status
            // byte until it sets DD.
            let status = unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!(self.tx_ring[index].status))
            };
            if status & TX_STATUS_DD != 0 {
                self.tx_in_use &= !(1 << index);
                // SAFETY: as above; clearing DD prepares the descriptor for its
                // next use by `transmit`.
                unsafe {
                    core::ptr::write_volatile(
                        core::ptr::addr_of_mut!(self.tx_ring[index].status),
                        0,
                    )
                };
            }
        }
    }

    /// Queues one Ethernet frame (without the CRC; `IFCS` appends it).
    ///
    /// The producer cursor only moves forward: `TDT` names the descriptor the
    /// hardware will fetch next, so filling an *older* free descriptor would be
    /// invisible until the head lapped the whole ring. The descriptor at
    /// `tx_next` is reusable once the device has cleared `DD` (or never used
    /// it); `tx_in_use` therefore also caps the ring at `TX_RING - 1`
    /// outstanding frames, keeping `TDH != TDT` meaningful to the device.
    pub fn transmit(&mut self, frame: &[u8]) -> Status {
        if frame.len() > FRAME_BYTES {
            return Status::CapacityExceeded;
        }
        self.poll();

        let index = self.tx_next;
        if self.tx_in_use & (1 << index) != 0 {
            return Status::Full;
        }
        self.tx_buffers[index].0[..frame.len()].copy_from_slice(frame);
        self.tx_in_use |= 1 << index;

        // The address never changes; length/cmd/status are rewritten for this
        // frame. The doorbell comes after the descriptor is visible.
        // SAFETY: `index` is inside the ring and the descriptor is not owned by
        // the device (`tx_in_use` was clear).
        unsafe {
            let length = core::ptr::addr_of_mut!(self.tx_ring[index].length);
            core::ptr::write_volatile(length, frame.len() as u16);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.tx_ring[index].cso), 0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.tx_ring[index].css), 0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.tx_ring[index].special), 0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.tx_ring[index].cmd), TX_CMD);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.tx_ring[index].status), 0);
        }
        self.tx_next = (index + 1) % TX_RING;
        compiler_fence(Ordering::SeqCst);
        write32(self.mmio, TDT, self.tx_next as u32);
        Status::Ok
    }

    /// Copies one received frame into `out`; `None` when the RX ring is empty.
    ///
    /// A frame with an error or without `EOP` is dropped and reported as a
    /// zero-length frame, but its descriptor is still returned to the device so
    /// the ring can make progress. At most one descriptor is processed per call.
    pub fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        let index = self.rx_next;
        // SAFETY: `index` is inside the ring; the status byte is written by the
        // device, so it is read volatile.
        let status =
            unsafe { core::ptr::read_volatile(core::ptr::addr_of!(self.rx_ring[index].status)) };
        if status & RX_STATUS_DD == 0 {
            return None;
        }
        let length =
            unsafe { core::ptr::read_volatile(core::ptr::addr_of!(self.rx_ring[index].length)) }
                as usize;
        let errors =
            unsafe { core::ptr::read_volatile(core::ptr::addr_of!(self.rx_ring[index].errors)) };
        let eop = status & RX_STATUS_EOP != 0;

        let mut copied = 0;
        if errors == 0 && eop {
            let len = length.min(FRAME_BYTES);
            copied = len.min(out.len());
            out[..copied].copy_from_slice(&self.rx_buffers[index].0[..copied]);
        }

        // Return the descriptor: clear status/errors, then move the tail onto it
        // so the device can refill it after the head wraps.
        // SAFETY: the descriptor is software-owned until the RDT write below.
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.rx_ring[index].status), 0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(self.rx_ring[index].errors), 0);
        }
        self.rx_next = (index + 1) % RX_RING;
        compiler_fence(Ordering::SeqCst);
        write32(self.mmio, RDT, index as u32);

        Some(copied)
    }
}

/// Reads `RAL0`/`RAH0` into a station address.
///
/// The 8254x loads the address from its EEPROM shortly after reset; an all-zero
/// read is retried for a bounded time before the caller falls back, so a slow
/// autoload is not mistaken for a device without an address. `None` means the
/// device still reports zero.
fn read_mac(mmio: u64) -> Option<[u8; 6]> {
    let deadline = clock::now_ns().saturating_add(MAC_TIMEOUT_NS);
    loop {
        let ral = read32(mmio, RAL);
        let rah = read32(mmio, RAH);
        if ral != 0 || rah != 0 {
            return Some([
                (ral & 0xFF) as u8,
                ((ral >> 8) & 0xFF) as u8,
                ((ral >> 16) & 0xFF) as u8,
                ((ral >> 24) & 0xFF) as u8,
                ((rah >> 16) & 0xFF) as u8,
                ((rah >> 24) & 0xFF) as u8,
            ]);
        }
        if clock::now_ns() >= deadline {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Waits boundedly for every bit in `mask` to clear in `offset`.
fn wait_clear(mmio: u64, offset: u32, mask: u32) -> bool {
    let deadline = clock::now_ns().saturating_add(RESET_TIMEOUT_NS);
    loop {
        if read32(mmio, offset) & mask == 0 {
            return true;
        }
        if clock::now_ns() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Waits boundedly for every bit in `mask` to be set in `offset`.
fn wait_set(mmio: u64, offset: u32, mask: u32, timeout_ns: u64) -> bool {
    let deadline = clock::now_ns().saturating_add(timeout_ns);
    loop {
        if read32(mmio, offset) & mask == mask {
            return true;
        }
        if clock::now_ns() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

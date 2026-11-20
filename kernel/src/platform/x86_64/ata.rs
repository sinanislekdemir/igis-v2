//! A minimal ATA PIO driver for the `home` disk (spec 0019, H3; ADR 0040).
//!
//! One device: the primary master on `0x1F0..=0x1F7` with the control block at
//! `0x3F6`. Identification requires LBA28; transfers are one 512-byte sector
//! at a time, polled with a bounded BSY/DRQ wait and a 400 ns I/O delay. There
//! are no interrupts, no DMA and no write-back cache. An absent, non-ATA or
//! unreadable disk is never fatal: [`init`] returns `None` and the system boots
//! with `/home` read-only (spec 0019, H3).
//!
//! The driver sits behind [`ext2w::SectorDevice`] so the filesystem does not
//! know whether it is talking to ATA or a host memory buffer.

use abi::Status;
use arch_x86_64::port::{inb, insw, outb, outsw};
use ext2w::SectorDevice;

use crate::println;

/// Primary bus I/O base: data, error/features, sector count, LBA0..2, drive,
/// status/command.
const IO_BASE: u16 = 0x1F0;
/// Device control block (alternate status and SRST).
const CONTROL: u16 = 0x3F6;
/// The `0x80` POST diagnostic port, used only for its I/O latency.
const DELAY_PORT: u16 = 0x80;

const REG_DATA: u16 = 0;
const REG_ERROR: u16 = 1;
const REG_SECCOUNT: u16 = 2;
const REG_LBA0: u16 = 3;
const REG_LBA1: u16 = 4;
const REG_LBA2: u16 = 5;
const REG_DRIVE: u16 = 6;
const REG_STATUS: u16 = 7;

const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_DF: u8 = 0x20;
const STATUS_BSY: u8 = 0x80;

/// The drive/head register value for the primary master in LBA mode: bit 7
/// and bit 5 set, bit 6 (`LBA`) set, drive-select bit 4 clear.
const DRIVE_MASTER: u8 = 0xE0;
const CMD_READ_SECTORS: u8 = 0x20;
const CMD_WRITE_SECTORS: u8 = 0x30;
const CMD_FLUSH_CACHE: u8 = 0xE7;
const CMD_IDENTIFY: u8 = 0xEC;
const CONTROL_SRST: u8 = 0x04;

/// A poll limit that maps to roughly 0.4 s at the I/O delay below.
const POLL_LIMIT: u32 = 1_000_000;

/// One identified primary-master ATA disk (LBA28).
pub struct AtaDisk {
    sectors: u32,
}

impl AtaDisk {
    /// Number of 512-byte sectors on the disk.
    pub fn sectors(&self) -> u64 {
        u64::from(self.sectors)
    }

    fn setup_lba(&self, index: u32) -> Result<(), Status> {
        if u64::from(index) >= u64::from(self.sectors) {
            return Err(Status::IoError);
        }
        // SAFETY: the ATA task-file ports are fixed platform I/O. The caller
        // has identified this device as the primary master and the LBA is
        // inside its recorded capacity.
        unsafe {
            outb(
                IO_BASE + REG_DRIVE,
                DRIVE_MASTER | ((index >> 24) & 0x0F) as u8,
            );
            // ATA requires a 400 ns settle after selecting the drive before the
            // remaining task-file registers are written.
            io_delay();
            outb(IO_BASE + REG_SECCOUNT, 1);
            outb(IO_BASE + REG_LBA0, index as u8);
            outb(IO_BASE + REG_LBA1, (index >> 8) as u8);
            outb(IO_BASE + REG_LBA2, (index >> 16) as u8);
        }
        Ok(())
    }

    fn read_one(&self, index: u32, out: &mut [u8; 512]) -> Result<(), Status> {
        wait_not_busy()?;
        self.setup_lba(index)?;
        // SAFETY: as above; a read command has no memory side effects.
        unsafe { outb(IO_BASE + REG_STATUS, CMD_READ_SECTORS) };
        wait_drq()?;
        let mut words = [0u16; 256];
        // SAFETY: the device is ready to transfer one sector and `words` is
        // exactly 256 16-bit words.
        unsafe { insw(IO_BASE + REG_DATA, &mut words) };
        let status = status();
        if status & (STATUS_ERR | STATUS_DF) != 0 {
            let _ = error();
            return Err(Status::IoError);
        }
        for (index, word) in words.iter().enumerate() {
            out[index * 2] = *word as u8;
            out[index * 2 + 1] = (*word >> 8) as u8;
        }
        Ok(())
    }

    fn write_one(&mut self, index: u32, data: &[u8; 512]) -> Result<(), Status> {
        wait_not_busy()?;
        self.setup_lba(index)?;
        // SAFETY: as above; a write command has no memory side effects.
        unsafe { outb(IO_BASE + REG_STATUS, CMD_WRITE_SECTORS) };
        wait_drq()?;
        let mut words = [0u16; 256];
        for (index, word) in words.iter_mut().enumerate() {
            *word = u16::from(data[index * 2]) | (u16::from(data[index * 2 + 1]) << 8);
        }
        // SAFETY: the device is ready to accept one sector and `words` holds
        // exactly 256 words.
        unsafe { outsw(IO_BASE + REG_DATA, &words) };
        // The transfer is not complete until BSY drops again; a command written
        // while BSY is set is ignored.
        wait_not_busy()?;
        // Make the transfer durable before returning (spec 0019, H4).
        // SAFETY: a flush-cache command is a normal task-file write.
        unsafe { outb(IO_BASE + REG_STATUS, CMD_FLUSH_CACHE) };
        wait_not_busy()?;
        Ok(())
    }
}

impl SectorDevice for AtaDisk {
    fn sector_count(&self) -> u64 {
        self.sectors()
    }

    fn read_sector(&self, index: u64, out: &mut [u8; 512]) -> Result<(), Status> {
        let index = u32::try_from(index).map_err(|_| Status::IoError)?;
        match self.read_one(index, out) {
            Ok(()) => Ok(()),
            Err(status) => {
                reset();
                Err(status)
            }
        }
    }

    fn write_sector(&mut self, index: u64, data: &[u8; 512]) -> Result<(), Status> {
        let index = u32::try_from(index).map_err(|_| Status::IoError)?;
        match self.write_one(index, data) {
            Ok(()) => Ok(()),
            Err(status) => {
                reset();
                Err(status)
            }
        }
    }
}

/// Identifies the primary master and returns the disk, or `None`.
///
/// The log says which case happened; a missing or non-ATA device leaves the
/// caller with no home mount rather than an error.
pub fn init() -> Option<AtaDisk> {
    match identify() {
        Some(sectors) => {
            println!(
                "ata: primary master, LBA28, {sectors} sectors ({} KiB)",
                u64::from(sectors) * 512 / 1024
            );
            Some(AtaDisk { sectors })
        }
        None => {
            println!("ata: no primary master");
            None
        }
    }
}

/// Reads the identify response, or returns `None` for an absent/non-ATA device.
fn identify() -> Option<u32> {
    // SAFETY: identifying the primary master is a fixed task-file sequence; the
    // ports are the standard ATA ones and no other driver owns them.
    unsafe {
        outb(IO_BASE + REG_DRIVE, DRIVE_MASTER);
        io_delay();
        outb(IO_BASE + REG_SECCOUNT, 0);
        outb(IO_BASE + REG_LBA0, 0);
        outb(IO_BASE + REG_LBA1, 0);
        outb(IO_BASE + REG_LBA2, 0);
        outb(IO_BASE + REG_STATUS, CMD_IDENTIFY);
        io_delay();

        // No device (or a floating bus) leaves the status register at zero.
        if inb(IO_BASE + REG_STATUS) == 0 {
            return None;
        }
        if wait_not_busy().is_err() {
            reset();
            return None;
        }

        // A non-zero LBA1/LBA2 signature means ATAPI or SATA, not ATA.
        let signature =
            u16::from(inb(IO_BASE + REG_LBA1)) | (u16::from(inb(IO_BASE + REG_LBA2)) << 8);
        if signature != 0 {
            return None;
        }
        if wait_drq().is_err() {
            reset();
            return None;
        }

        let mut words = [0u16; 256];
        insw(IO_BASE + REG_DATA, &mut words);

        // Word 49 bit 9: LBA supported.
        if words[49] & (1 << 9) == 0 {
            return None;
        }
        let sectors = u32::from(words[60]) | (u32::from(words[61]) << 16);
        if sectors == 0 {
            return None;
        }
        Some(sectors)
    }
}

fn status() -> u8 {
    // SAFETY: reading the ATA status register has no side effects.
    unsafe { inb(IO_BASE + REG_STATUS) }
}

fn error() -> u8 {
    // SAFETY: reading the ATA error register clears it; the driver only does
    // this after it has observed an error.
    unsafe { inb(IO_BASE + REG_ERROR) }
}

/// Waits, bounded, for the device to drop BSY.
fn wait_not_busy() -> Result<(), Status> {
    for _ in 0..POLL_LIMIT {
        let status = status();
        if status == 0 {
            return Err(Status::IoError);
        }
        if status & STATUS_BSY == 0 {
            return Ok(());
        }
        io_delay();
    }
    Err(Status::IoError)
}

/// Waits, bounded, for the device to assert DRQ without an error.
fn wait_drq() -> Result<(), Status> {
    for _ in 0..POLL_LIMIT {
        let status = status();
        if status == 0 {
            return Err(Status::IoError);
        }
        if status & (STATUS_ERR | STATUS_DF) != 0 {
            let _ = error();
            return Err(Status::IoError);
        }
        if status & STATUS_BSY == 0 && status & STATUS_DRQ != 0 {
            return Ok(());
        }
        io_delay();
    }
    Err(Status::IoError)
}

/// Resets the bus after a timeout so the next access starts from a known state.
fn reset() {
    // SAFETY: SRST is a fixed control-port write and the delay reads only the
    // POST port. The wait is bounded, so a dead bus cannot hang boot.
    unsafe {
        outb(CONTROL, CONTROL_SRST);
        io_delay();
        outb(CONTROL, 0);
        io_delay();
    }
    let _ = wait_not_busy();
}

/// The conventional ATA 400 ns I/O delay: four reads of the POST port.
#[inline]
fn io_delay() {
    // SAFETY: port 0x80 is readable and has no side effects.
    unsafe {
        for _ in 0..4 {
            let _ = inb(DELAY_PORT);
        }
    }
}

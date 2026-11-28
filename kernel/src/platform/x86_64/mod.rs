//! x86_64 PC devices and AP bring-up: PCI configuration space, ATA PIO, the
//! UHCI/HID tablet, the PS/2 aux port, the Intel HDA controller, and the SMP
//! start-up path.

pub mod ata;
pub mod hda;
pub mod mouse;
pub mod pci;
pub mod smp;
pub mod usb;

//! Local APIC: enable it, mask the legacy PIC, calibrate the timer against the
//! PIT, and run a periodic tick.
//!
//! The tick is the system clock (spec 0007): one global nanosecond counter
//! advanced by the boot core. The TSC is deliberately unused.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::port::{inb, outb};

const APIC_BASE_MSR: u32 = 0x1B;

const REG_ID: u32 = 0x20;
const REG_EOI: u32 = 0xB0;
const REG_SVR: u32 = 0xF0;
const REG_LVT_TIMER: u32 = 0x320;
const REG_TIMER_INIT: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3E0;

pub const TIMER_VECTOR: u8 = 0x20;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

static BASE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct LocalApic {
    base: u64,
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nostack));
    ((high as u64) << 32) | low as u64
}

unsafe fn wrmsr(msr: u32, value: u64) {
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") value as u32,
        in("edx") (value >> 32) as u32,
        options(nostack),
    );
}

impl LocalApic {
    fn read(&self, offset: u32) -> u32 {
        // SAFETY: the APIC page is identity-mapped device memory.
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u32) }
    }

    fn write(&self, offset: u32, value: u32) {
        // SAFETY: the APIC page is identity-mapped device memory.
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u32, value) }
    }

    pub fn id(&self) -> u32 {
        self.read(REG_ID) >> 24
    }

    /// 10 ms of PIT time, expressed in APIC timer ticks per millisecond.
    pub fn calibrate(&self) -> u32 {
        // SAFETY: PIT and port 0x61 are the conventional platform timer.
        unsafe {
            let mut control = inb(0x61);
            control = (control & !0x02) & !0x01; // speaker off, gate low
            outb(0x61, control);

            outb(0x43, 0xB0); // channel 2, mode 0, lobyte/hibyte, binary
            outb(0x42, 0x9C); // 11932 = 10 ms at 1.193182 MHz
            outb(0x42, 0x2E);

            self.write(REG_TIMER_DIVIDE, 0x3); // divide by 16
            self.write(REG_TIMER_INIT, u32::MAX);

            control |= 0x01; // gate high starts the count
            outb(0x61, control);

            while inb(0x61) & 0x20 == 0 {
                core::hint::spin_loop();
            }

            let remaining = self.read(REG_TIMER_CURRENT);
            self.write(REG_TIMER_INIT, 0);

            (u32::MAX - remaining) / 10
        }
    }

    pub fn start_periodic(&self, ticks_per_ms: u32) {
        self.write(REG_LVT_TIMER, u32::from(TIMER_VECTOR) | (1 << 17)); // periodic
        self.write(REG_TIMER_DIVIDE, 0x3);
        self.write(REG_TIMER_INIT, ticks_per_ms.max(1));
    }

    pub fn eoi(&self) {
        self.write(REG_EOI, 0);
    }
}

/// End of interrupt for the ISR, without holding the device state anywhere.
pub fn eoi() {
    let base = BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: the APIC page is identity-mapped device memory.
        unsafe { core::ptr::write_volatile((base + REG_EOI as u64) as *mut u32, 0) };
    }
}

pub fn init() -> Option<LocalApic> {
    // CPUID.01H:EDX.APIC[bit 9]
    let leaf = core::arch::x86_64::__cpuid(1);
    if leaf.edx & (1 << 9) == 0 {
        return None;
    }

    // SAFETY: enabling the local APIC through its MSR is a privileged but
    // architecturally defined operation.
    unsafe {
        let current = rdmsr(APIC_BASE_MSR);
        wrmsr(APIC_BASE_MSR, (current & !0xFFF) | (1 << 11));
    }

    // SAFETY: reading back the base after enabling it.
    let base = unsafe { rdmsr(APIC_BASE_MSR) } & 0xFFFF_F000;
    let apic = LocalApic { base };
    BASE.store(base, Ordering::Relaxed);

    apic.write(REG_SVR, 0x1FF); // software enable, spurious vector 0xFF
    mask_legacy_pic();

    Some(apic)
}

/// Mask every legacy PIC line; the local APIC delivers our interrupts.
fn mask_legacy_pic() {
    // SAFETY: the 8259 PICs are at the conventional ports; masking them is
    // exactly what a kernel using the APIC must do.
    unsafe {
        outb(0x21, 0xFF);
        outb(0xA1, 0xFF);
    }
}

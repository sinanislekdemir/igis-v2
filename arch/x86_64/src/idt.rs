//! Interrupt descriptor table. The stubs and the common frame live in
//! `interrupts.s`; this module installs gates and describes the frame.

use core::arch::asm;
use core::ptr::addr_of_mut;

#[derive(Clone, Copy)]
#[repr(C)]
struct Entry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    kind: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl Entry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            kind: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn handler(address: u64, selector: u16, ist: u8) -> Self {
        Self {
            offset_low: address as u16,
            selector,
            ist,
            kind: 0x8E, // present, ring 0, 64-bit interrupt gate
            offset_mid: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

/// The frame as laid out by `interrupts.s`, lowest address first.
#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

extern "C" {
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_21();
    fn isr_22();
    fn isr_23();
    fn isr_24();
    fn isr_25();
    fn isr_26();
    fn isr_27();
    fn isr_28();
    fn isr_29();
    fn isr_30();
    fn isr_31();

    fn irq_timer();
    fn irq_spurious();
}

static STUBS: [unsafe extern "C" fn(); 32] = [
    isr_0, isr_1, isr_2, isr_3, isr_4, isr_5, isr_6, isr_7, isr_8, isr_9, isr_10, isr_11, isr_12,
    isr_13, isr_14, isr_15, isr_16, isr_17, isr_18, isr_19, isr_20, isr_21, isr_22, isr_23, isr_24,
    isr_25, isr_26, isr_27, isr_28, isr_29, isr_30, isr_31,
];

static mut IDT: [Entry; 256] = [Entry::missing(); 256];

/// Install the exception gates, the double-fault IST gate and the timer gates.
pub fn init(code_selector: u16) {
    // SAFETY: called once during boot with interrupts disabled; the only
    // outstanding writes are to IDT, which nothing else reads yet.
    unsafe {
        let idt = addr_of_mut!(IDT);
        for (vector, stub) in STUBS.iter().enumerate() {
            (*idt)[vector] = Entry::handler(*stub as u64, code_selector, 0);
        }

        // #DF runs on its own stack so a bad kernel stack cannot prevent the dump.
        let double_fault: unsafe extern "C" fn() = isr_8;
        (*idt)[8] = Entry::handler(double_fault as u64, code_selector, 1);

        // #PF runs on its own IST too: a kernel stack overflow leaves no room
        // for the exception frame, and the guard page then reports a clean
        // page fault instead of escalating to #DF (Q42, stack.rs).
        let page_fault: unsafe extern "C" fn() = isr_14;
        (*idt)[14] = Entry::handler(page_fault as u64, code_selector, 2);

        let timer_isr: unsafe extern "C" fn() = irq_timer;
        let spurious_isr: unsafe extern "C" fn() = irq_spurious;
        (*idt)[crate::apic::TIMER_VECTOR as usize] =
            Entry::handler(timer_isr as u64, code_selector, 0);
        (*idt)[crate::apic::SPURIOUS_VECTOR as usize] =
            Entry::handler(spurious_isr as u64, code_selector, 0);

        let pointer = DescriptorPointer {
            limit: (core::mem::size_of::<[Entry; 256]>() - 1) as u16,
            base: idt as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

pub fn name(vector: usize) -> &'static str {
    match vector {
        0 => "#DE divide error",
        1 => "#DB debug",
        2 => "NMI",
        3 => "#BP breakpoint",
        4 => "#OF overflow",
        5 => "#BR bound range",
        6 => "#UD invalid opcode",
        7 => "#NM device not available",
        8 => "#DF double fault",
        10 => "#TS invalid TSS",
        11 => "#NP segment not present",
        12 => "#SS stack-segment fault",
        13 => "#GP general protection",
        14 => "#PF page fault",
        16 => "#MF x87 floating point",
        17 => "#AC alignment check",
        18 => "#MC machine check",
        19 => "#XM SIMD floating point",
        20 => "#VE virtualization",
        21 => "#CP control protection",
        29 => "#VC VMM communication",
        30 => "#SX security",
        _ => "reserved",
    }
}

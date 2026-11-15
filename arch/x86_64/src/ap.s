// Application-processor trampoline (spec 0002 "Boot", spec 0007 "SMP
// bring-up").
//
// The boot core copies this blob to the fixed physical page at `AP_BASE` and
// fills the parameter page at `PARAM_BASE` (both reserved low memory, identity
// mapped by the boot page tables). A SIPI then starts each AP in 16-bit real
// mode at that page, and this code:
//
//   1. loads a tiny GDT and enters 32-bit protected mode,
//   2. sets CR4.PAE and EFER.LME, loads the shared PML4, enables paging,
//   3. far-jumps into the flat 64-bit code segment,
//   4. claims a core index with an atomic counter, picks that core's stack
//      from the parameter page and calls the kernel entry point.
//
// Everything the code addresses is `AP_BASE + <offset>`. The offsets are fixed
// numerically with `.org` (the blob is always copied to `AP_BASE`, so the
// SIPI's CS:IP, the GDT descriptor's base and the far-jump targets are all
// physical constants). Nothing here depends on where the section is linked.

.set AP_BASE, 0x8000
.set PARAM_BASE, 0x9000

// Parameter page fields; mirrored in `smp.rs`.
.set PARAM_PML4, 0x00
.set PARAM_ENTRY, 0x08
.set PARAM_STACKS, 0x10
.set PARAM_NEXT, 0x50

// Fixed offsets inside the trampoline page.
.set OFF_GDT, 0x80
.set OFF_GDT_DESC, 0x100
.set OFF_PM, 0x180
.set OFF_LM, 0x280

// The SIPI vector is AP_BASE >> 12 = 8.

.section .text.ap, "ax"

// ---------------------------------------------------------------- real mode
.code16
.global ap_start
ap_start:
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7000                  // scratch real-mode stack, below the blob

    mov bx, AP_BASE + OFF_GDT_DESC
    lgdt [bx]

    mov eax, cr0
    or eax, 1                       // CR0.PE
    mov cr0, eax

    // Far jump into the 32-bit code segment: 16:32 form (0x66 prefix).
    .byte 0x66, 0xEA
    .long AP_BASE + OFF_PM
    .short 0x18

// --------------------------------------------------------------------- data
// The layout matches the kernel GDT (`gdt.rs`): null, 64-bit code at 0x08,
// data at 0x10. The 32-bit code segment lives at 0x18 here only. Keeping
// 64-bit code at 0x08 means the AP is already running on the selector the
// kernel GDT gives it when `gdt::load` replaces this table.
.org OFF_GDT
ap_gdt:
    .quad 0
    .quad 0x00AF9A000000FFFF        // 0x08: 64-bit code, ring 0, base 0
    .quad 0x00CF92000000FFFF        // 0x10: data, ring 0, base 0
    .quad 0x00CF9A000000FFFF        // 0x18: 32-bit code, ring 0, base 0
ap_gdt_end:
.org OFF_GDT_DESC
ap_gdt_desc:
    .short ap_gdt_end - ap_gdt - 1
    .long AP_BASE + OFF_GDT

// ------------------------------------------------------------ protected mode
.org OFF_PM
.code32
ap_pm:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    mov eax, cr4
    or eax, 0x620                   // PAE | OSFXSR | OSXMMEXCPT
    mov cr4, eax

    mov ecx, 0xC0000080             // EFER
    rdmsr
    or eax, 0x900                   // LME | NXE
    wrmsr

    mov eax, [PARAM_BASE + PARAM_PML4]
    mov cr3, eax

    mov eax, cr0
    or eax, 0x80010023              // PG | WP | NE | MP | PE
    and eax, 0xFFFFFFF3             // clear EM and TS
    mov cr0, eax

    // Far jump into the 64-bit code segment (32:32 form).
    .byte 0xEA
    .long AP_BASE + OFF_LM
    .short 0x08

// ----------------------------------------------------------------- long mode
.org OFF_LM
.code64
ap_lm:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    // Claim this AP's core index: the counter starts at 1, so the old value is
    // the slot this core owns. `xadd` is atomic, so two APs never claim one.
    mov ecx, 1
    lock xadd dword ptr [PARAM_BASE + PARAM_NEXT], ecx
    mov eax, ecx

    lea rbx, [PARAM_BASE + PARAM_STACKS]
    mov rsp, qword ptr [rbx + rax*8]    // this core's kernel stack top
    mov rdi, rax                        // first argument: the core index
    mov rax, qword ptr [PARAM_BASE + PARAM_ENTRY]
    call rax                            // -> `smp::ap_entry`

ap_halt:
    cli
    hlt
    jmp ap_halt

.global ap_end
ap_end:

// Entry path for x86_64 under GRUB/multiboot2.
//
// GRUB enters `_start` in 32-bit protected mode with:
//   eax = 0x36D76289 (multiboot2 boot magic)
//   ebx = physical address of the multiboot2 information structure
//
// Layout invariant (see kernel/linker.ld): `.boot` is linked at its physical
// address (1 MiB), everything else at 0xFFFFFFFF80000000 + physical. The stub
// builds page tables that identity-map the first 4 GiB and alias the first GiB
// at 0xFFFFFFFF80000000, enables long mode, and jumps to the higher-half copy
// of itself before calling `kmain`.

.section .multiboot2, "a"
.balign 8
mb2_start:
    .long 0xE85250D6                        # magic
    .long 0                                 # architecture: i386
    .long mb2_end - mb2_start               # header length
    .long -(0xE85250D6 + 0 + (mb2_end - mb2_start))

    # framebuffer request tag: 1024x768x32
    .short 5
    .short 0
    .long 20
    .long 1024
    .long 768
    .long 32

    .balign 8
    .short 0                                # end tag
    .short 0
    .long 8
mb2_end:

.section .boot, "ax"
.code32
.global _start
_start:
    cli
    cmp eax, 0x36D76289
    jne .Lbad_magic
    mov edi, ebx                            # multiboot2 info pointer

    mov esp, offset stack_top

    # Identity map 0..4 GiB with 2 MiB pages: 4 page directories, 512 entries each.
    mov ecx, 0
    mov eax, 0x83                           # present | writable | page size
    lea edx, [pd0]
.Lfill_pd:
    mov [edx + ecx*8], eax
    add eax, 0x200000
    inc ecx
    cmp ecx, 2048
    jne .Lfill_pd

    # PDPT0[0..4] -> pd0..pd3 (identity)
    lea edx, [pdpt0]
    lea eax, [pd0]
    or eax, 3
    mov [edx], eax
    lea eax, [pd1]
    or eax, 3
    mov [edx + 8], eax
    lea eax, [pd2]
    or eax, 3
    mov [edx + 16], eax
    lea eax, [pd3]
    or eax, 3
    mov [edx + 24], eax

    # PDPT1[510] -> pd0, so 0xFFFFFFFF80000000 aliases physical 0..1 GiB.
    # (The higher-half window starts at PML4[511], PDPT[510].)
    lea edx, [pdpt1]
    lea eax, [pd0]
    or eax, 3
    mov [edx + 510*8], eax

    # PML4[0] -> pdpt0, PML4[511] -> pdpt1
    lea edx, [pml4]
    lea eax, [pdpt0]
    or eax, 3
    mov [edx], eax
    lea eax, [pdpt1]
    or eax, 3
    mov [edx + 511*8], eax

    # PAE, OSFXSR, OSXMMEXCPT
    mov eax, cr4
    or eax, (1 << 5) | (1 << 9) | (1 << 10)
    mov cr4, eax

    # EFER.LME | EFER.NXE
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8) | (1 << 11)
    wrmsr

    lea eax, [pml4]
    mov cr3, eax

    # CR0: set PG, WP, MP; clear EM and TS
    mov eax, cr0
    or eax, 0x80010001
    and eax, 0xFFFFFFF3
    or eax, 0x2
    mov cr0, eax

    lgdt [gdt64_ptr]
    .byte 0xEA
    .long long_mode_entry
    .short 0x08

.Lbad_magic:
    cli
    hlt
    jmp .Lbad_magic

.code64
long_mode_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    # Re-enter the same code through its higher-half alias.
    lea eax, [rip + long_mode_high]
    mov rcx, 0xFFFFFFFF80000000
    add rax, rcx
    jmp rax

long_mode_high:
    lea rsp, [rip + stack_top]
    xor rbp, rbp
    mov rax, [rip + kernel_entry]
    call rax

.Lhalt64:
    cli
    hlt
    jmp .Lhalt64

.balign 4096
pml4:  .space 4096, 0
pdpt0: .space 4096, 0
pdpt1: .space 4096, 0
pd0:   .space 4096, 0
pd1:   .space 4096, 0
pd2:   .space 4096, 0
pd3:   .space 4096, 0

.balign 16
stack:
    # 64 KiB: the in-kernel compiler is recursive and runs on this stack at
    # boot. It sits directly above the boot page tables, so an overflow here
    # corrupts paging; keep the headroom generous.
    .space 65536, 0
stack_top:

.balign 8
kernel_entry:
    .quad kmain

gdt64:
    .quad 0
    .quad 0x00AF9A000000FFFF                # 64-bit code, ring 0
    .quad 0x00CF92000000FFFF                # data, ring 0
gdt64_end:
gdt64_ptr:
    .short gdt64_end - gdt64 - 1
    .long gdt64

/* Higher-half kernel.
 *
 * `.boot` (multiboot2 header, 32-bit stub, page tables, early stack) is linked
 * at its physical load address so GRUB can enter it in 32-bit protected mode.
 * Everything else is linked at 0xFFFFFFFF80000000 + physical and becomes
 * reachable once the stub's page tables are active.
 */

ENTRY(_start)

KERNEL_BASE = 0xFFFFFFFF80000000;
KERNEL_PHYS = 1M;

SECTIONS
{
    . = KERNEL_PHYS;

    .boot : {
        KEEP(*(.multiboot2))
        *(.boot)
    }

    . = ALIGN(4K);
    _kernel_phys_start = .;

    . += KERNEL_BASE;

    .text : AT(ADDR(.text) - KERNEL_BASE) {
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_BASE) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_BASE) {
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss) - KERNEL_BASE) {
        *(.bss .bss.*)
        *(COMMON)
    }

    . = ALIGN(4K);
    _kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}

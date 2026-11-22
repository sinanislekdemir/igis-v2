# 0059: Boot only the kernel and mount the application image from an ATA disk

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** project owner
- **Supersedes:** 0015 (the delivery medium and the reader; the ext2 format choice stands)
- **Related specs:** 0018 (image filesystem), 0019 (home filesystem), 0025 (delivery)

## Context

ADR 0015 delivers the application image as a GRUB module: the bootloader reads
`apps.img` from the boot medium into guest memory and the kernel reads that memory
window as a borrowed slice. It was chosen when the image held a few megabytes of
source, and it removed a block driver, a cache and locking from the boot path. The same
ADR recorded the risk in one line — "the image size is resident in RAM for the lifetime
of the system" — and the escape: later block devices implement the same device seam and
the filesystem driver does not change.

M14 made the risk concrete. The image grew to 128 MiB because the PPM corpus is
123,045,476 B of it, and the boot now spends about 9.9 s asking GRUB to copy the whole
volume from the emulated CD into RAM. Total time from QEMU start to
`desktop_shell: ready` is 10.4 s on TCG/4 cores, and 128 MiB of the 256 MiB guest is
pinned forever (`available: 101 MiB`). The kernel's own share of that boot is 385 ms.

Meanwhile the home disk already proves the intended path: `ext2w` mounts ext2 over a
`SectorDevice` (ATA PIO), is host-tested, and is shared across cores behind one lock.
The system image is read-only, so it needs less than what home already does.

## Decision

We will boot with only `kernel.elf` and mount the application image from a block device:

- `grub.cfg` carries `multiboot2 /boot/kernel.elf` and no module; the ISO holds the
  kernel and GRUB files only.
- The application image is a raw, unpartitioned, read-only ext2 volume on `ide0` unit 1
  (`0x1F0`, drive `0xF0`). `/home` stays on `ide0` unit 0 and the boot CD on `ide1`
  unit 0.
- The kernel identifies the drive, mounts the volume read-only with `ext2w`, and
  publishes it through the existing file router before any AP starts.
- One ext2 implementation serves both mounts. `vfs/ext2` and the GRUB-module byte-slice
  path are removed, and with them the `'static` borrow extension in `fs::init`.
- Every harness attaches the volume through a temporary overlay (`snapshot=on`): QEMU's
  IDE model refuses a read-only block node, and the overlay gives the same protection.
  A boot must leave the image file byte-identical. Drive arguments come from one helper
  keyed by the ISO path.

because the image is read on demand (a root listing and the boot unit, not a corpus),
because the bootloader's job is to load the kernel, and because a second, untested ext2
parser over a borrowed slice is maintenance we no longer need.

This resolves OPEN-QUESTIONS Q59: the `RefCell` cache is not `Sync`, and a per-mount
`SpinLock` — the rule `/home` already follows — is the answer rather than a second
reader.

## Consequences

### Positive

- Boot no longer moves 128 MiB. GRUB reads the kernel only; the expected time to the
  desktop is under 2.5 s, and the kernel can draw a truthful boot screen while it mounts.
- 128 MiB returns to the allocator; the guest can shrink and the image may grow beyond
  RAM.
- One ext2 reader, host-tested by `ext2w`'s 22 tests, replaces a parser whose only proof
  was a boot self-check.
- The image is attached through a temporary overlay, so a kernel bug cannot corrupt it.

### Negative

- The image becomes a second artifact that every harness must attach; forgetting it
  turns a desktop boot into `apps: no image` and a halt.
- ATA must be alive before the file router is published, so a broken driver now fails
  the boot at the mount instead of merely losing `/home`.
- Reads are PIO and serialized: the compiler's `/lib` reads and the image viewer's rows
  share one channel lock with `/home` writes. Measured cost decides whether a cache or
  DMA is worth adding.
- The `ide0` unit 1 placement is a QEMU-visible layout, not a discovery mechanism: a
  machine with a different wiring needs the constant (or a later command-line override).

### Neutral / follow-up work

- `AGENTS.md`, `docs/HISTORY.md`, spec 0018 and the spec/ADR indexes get status rows
  when the work lands; ADR 0015 is marked superseded.
- The deferred virtio-blk item becomes the answer if PIO measures badly, not a
  prerequisite for this change.
- `apps.img` gets an `e2fsck -fn` in `xtask image` if the owner wants it (spec 0025 open
  question 1).
- A kernel boot panel with real mount progress becomes cheap, because the wait is now in
  the kernel (spec 0025 B16).

## Alternatives considered

| Alternative | Why not |
|---|---|
| `readonly=on` on the ATA volume | QEMU's `ide-hd` refuses a read-only block node ("Block node is read-only", verified on 11.1.3); `snapshot=on` discards guest writes to a temporary overlay and leaves the file byte-identical |
| Keep the module and shrink the corpus to a few MiB | Removes the symptom for the current corpus only; the boot still copies the whole image and pins it, and the next large asset returns to the same place |
| virtio-blk for the image | A second block driver, PCI scan and virtqueue work for a job the ATA driver already does; keep it for the DMA/PIO measurement |
| Partition table on one disk (image + home) | Adds an MBR parser and offset-aware `e2fsck` for no gain; three IDE units fit the QEMU model already |
| Boot from the image disk with no ISO | Requires embedding GRUB into a raw image, changes every harness invocation and loses the cheap `-cdrom` boot |
| Read the volume through a minimal ISO9660/ATAPI path | ATAPI and ISO9660 are both larger than the ATA SATA-style read already implemented |
| GRUB-side splash and keep the module | Hides a ten-second copy instead of removing it; GRUB cannot report module progress, so the screen would be static |

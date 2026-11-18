# 0040: A real ext2 writer on a dedicated ATA home disk behind a path router

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** project owner
- **Related specs:** 0019 (normative), amends 0005/0013/0018

## Context

The application image is a read-only ext2 module in RAM (ADR 0015) holding source
code (ADR 0019). Applications can read it but not write it, so the editor cannot
save. The owner asked for a writable `/home` and chose, explicitly, a **real ext2
writer with persistence** over a RAM overlay or a different format.

Persistence forces three decisions that the read-only path did not need:

1. Where the writable bytes live. The GRUB module is RAM; writing it would neither
   survive a reboot nor keep the application image immutable.
2. How the machine reaches a disk. The kernel has no block driver; the ext2 reader
   is a zero-copy view over an identity-mapped module.
3. How two filesystems look like one namespace to the language, the compiler and
   `core.spawn`.

The owner also fixed the first API surface: `write`, `remove`, `mkdir`, `rename`,
no truncate; writes persist across reboot.

## Decision

We will:

1. **Put `/home` on a dedicated writable ext2 image** (`build/home.img`, 8 MiB,
   1 KiB blocks) attached to QEMU as an IDE disk. The application image stays
   read-only and unchanged; no application can rewrite the code it runs.
2. **Write ext2 for real**, not an overlay: allocate and free blocks and inodes,
   grow and shrink directory entries, create directories, rename and replace
   files. `e2fsck -fn` on the host is the consistency proof; there is no journal.
3. **Start with an ATA PIO driver** on the primary bus (LBA28, 512-byte sectors,
   polling, no DMA/interrupts), because QEMU's PIIX3 IDE is the smallest device
   that is real. The driver sits behind a `SectorDevice` trait so a virtio-blk
   driver can replace it without touching the filesystem.
4. **Put the writer in a new `ext2w` crate** (`no_std`, allocation-free) over
   `SectorDevice`, with a small write-through block cache. It is unit-tested on
   the host against an in-memory device, which a kernel-only module could not be.
5. **Route by the first path component**, not a general mount table: `home` and
   `home/...` go to the writable mount (or to the application image's `home` when
   no disk is attached); every other path goes to the read-only application image.
   `/` lists `home` once. The REPL, the compiler's source read and the file
   builtins all use this one router.
6. **Expose four host builtins with no opcode**: `core.fs_write`,
   `core.fs_remove`, `core.fs_mkdir`, `core.fs_rename` (builtins 53–56; ABI v1
   stays at 75). Writes outside `home` are `Invalid`.
7. **Have no truncate.** A shorter replacement is `remove` + create or
   write-a-sibling-then-rename; rename replaces an existing regular file, which
   gives the editor a safe save path.
8. **Write synchronously**, data before inode before directory entry, bitmap
   updates before use, and mark the filesystem clean on mount without incrementing
   mount counts. The kernel never writes on a structure it could not validate.
9. **Implement the home mount in `langhost` over a temporary directory**, so the
   write primitives, the editor save and the router are testable on the host
   without a disk.

## Consequences

### Positive

- `/home` survives reboot and power cycles; the editor can save; applications can
  create files, directories and rename them.
- The application image remains immutable and source-only; a corrupted user file
  cannot break the code the system runs.
- The writer is host-testable and the drive is a normal QEMU file, so the
  persistence test is a reboot with the same image.
- No ABI change: four builtins, no opcode, no manifest/verifier change.

### Negative

- The `/home` directory in the application image is shadowed by the mount when a
  disk is present; the initial user files live in `samples/home/` and in the home
  image.
- No journal: a power loss during a write may leave an inconsistency that
  `e2fsck` must repair. The ordering reduces the window but does not close it.
- ATA PIO is slow and legacy; the first version trades performance for a small,
  well-understood driver. Every write is synchronous.
- Without truncate, saving a shorter file costs a remove/rename; the editor must
  use the temporary-file rename path, and a failed save can leave the temporary
  file behind (it is removed on the next save).
- The router is a deliberate special case, not a mount table; a second writable
  filesystem would need the general mechanism this decision avoids.

### Neutral / follow-up work

- virtio-blk behind `SectorDevice` when the ATA path is measured.
- A truncate primitive and an append-oriented API if the editor needs them.
- A journal (or ext4) if crash consistency becomes a requirement.
- `e2fsck -fy` repair at boot only if a real inconsistency is observed.

## Alternatives considered

| Alternative | Why not |
|---|---|
| RAM overlay for `/home` | The owner asked for persistence; an overlay loses writes on reboot. |
| Make the application image writable | It is a RAM module (no persistence), and applications would rewrite their own code. |
| A second in-memory filesystem mounted at `/home` | Still not persistent, and a mount layer for one filesystem buys nothing. |
| virtio-blk first | More code (PCI already exists, but virtqueue and multiple queues) for the same first proof; ATA hides behind the same trait. |
| Write a FAT or a custom format | The system already reads ext2 and the host has `mke2fs`/`e2fsck`; one format is less surface. |
| Journaling ext3/ext4 | Out of scope for the first writable slice; ext2 + fsck is the accepted risk. |
| General mount table | One writable filesystem and one read-only image; a first-component rule is simpler and testable. |
| Truncate in the first API | The owner excluded it; remove+rename covers replacement. |

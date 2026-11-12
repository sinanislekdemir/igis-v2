# 0015: Deliver applications on a read-only ext2 image over an in-memory block device

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture)

## Context

Vision R13 requires application loading to sit behind a provider interface, with a
ramdisk or module source first and block devices plus a filesystem later. The application
model does not change when delivery changes, so the first milestone needs a delivery
format that is real enough to exercise the filesystem path but small enough not to become
a project of its own.

The candidates were a custom minimal filesystem, FAT32, and ext2. A custom format would
be the smallest driver but would invent a toolchain and a format with no external tools.
FAT32 has broad tooling but a large surface of corner cases (long names, cluster chains,
FAT variants). ext2 has no journal, a simple documented on-disk structure, and is
produced by standard host tools (`mke2fs`, `debugfs`, `e2cp`).

Persistence is not required in the first milestone: the system is developed by rebuilding
the image on the host, and the in-system workflow is a REPL with no durable storage. That
removes writes, allocation and crash consistency from the problem.

## Decision

- Applications, their manifests and the bitmap font are stored on a **read-only ext2
  image**.
- The image is built on the host with standard ext2 tools and shipped as a **GRUB
  module**. GRUB places it in memory; the filesystem driver reads it through a
  `BlockDevice` abstraction over that memory window.
- The driver supports a deliberate subset: ext2 revisions 0 and 1, 1 KiB and 4 KiB block
  sizes, 32-bit block numbers, direct and singly/doubly/triply indirect blocks, and
  directory entries with file types. No journal, no ext4 extents, no 64-bit feature, no
  writes, no allocation.
- Mounting is read-only. No write, create, delete or rename operation exists in the first
  milestone.
- Later block devices (virtio-blk or ATA) implement the same `BlockDevice` trait; the
  filesystem driver does not change. That is the R13 "block device later" path without a
  second filesystem.
- The image is rebuilt to change an application. There is no in-system installation path
  in the first milestone.

## Consequences

### Positive

- No filesystem design work: the on-disk format is specified by ext2 and produced by
  mature host tools.
- A real filesystem path is exercised from the first milestone: superblock, group
  descriptors, inodes, directories and indirect blocks.
- No block-device driver, no DMA and no interrupts are needed to read the image, because
  it lives in memory.
- The same `BlockDevice` boundary later accepts a real disk without changing the
  filesystem code.
- Read-only removes the entire write path: allocation, bitmaps, journal replay and crash
  consistency are out of scope.

### Negative

- The ext2 subset is still a real parser: malicious or corrupt images must fail safely at
  load, and every field is untrusted input to the loader.
- Changing an application means rebuilding and rebooting; there is no persistence and no
  in-system development (vision OQ 5 resolved as REPL-only).
- ext2's 32-bit block numbers and indirect chains cap image size and add a worst-case
  read depth that a simpler format would not have.
- The image size is resident in RAM for the lifetime of the system.

### Neutral / follow-up work

- The host image builder and the loader share one manifest format; both paths (module now,
  disk later) consume the same manifest.
- A writable filesystem, an in-system editor and persistence are future work and require
  their own decision when the personal-system goal moves from horizon to milestone.
- Corruption handling is defined by the loader: an unreadable superblock or inode refuses
  the mount; a bad individual file refuses that application, not the boot.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Custom minimal filesystem | Smallest driver, but invents a format and a toolchain, and every future tool must be written from scratch. |
| FAT32 | Broad tooling, but long-name and cluster-chain corner cases are a larger and less interesting surface than ext2's. |
| TAR/CPIO ramdisk | Trivial to produce and read, but has no directories, no metadata and no path to a real block device; it would be thrown away. |
| Real block device (virtio-blk) now | Exercises the hardware path early, but adds a driver, DMA and interrupts to the first milestone for no application-model benefit. |
| Writable ext2 | Adds allocation, bitmaps and crash consistency before persistence is needed; rejected for the first milestone, not forever. |

# 0019: The writable home filesystem

- **Status:** Implemented
- **Scope:** `ext2w` (new crate), `kernel` (ATA driver, fs router, sched/REPL), `abi`,
  `compiler`, `interpreter`, `langhost`, `xtask`, `samples/apps`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0040
- **Updated:** 2026-11-18

## Context

The application image is read-only by design (spec 0015, ADR 0015) and carried as a
GRUB module in RAM: nothing written there could survive a reboot. Applications can
read it (`core.fs_list`/`fs_read`/`fs_size`, spec 0013) but cannot create, change or
delete a file, so the editor can only report `read-only: no writable filesystem`.

The owner chose a **real ext2 writer with persistence** for `/home`, not a RAM
overlay and not a second filesystem format. That makes the filesystem reachable
across a power cycle, which means it cannot live in the RAM module: it needs a real
block device and an on-disk image that QEMU keeps between boots.

**Non-goals.** No journal and no crash-consistency guarantee beyond ext2's own
ordering (a power loss during a write may need `e2fsck` on the host). Truncate is
part of the API (`core.fs_truncate`, A7, ADR 0041); a shorter replacement can also
be `remove` + create or write-then-rename. No writable application image, no write outside `/home`, no
permissions, no users, no sparse-file policy, no quotas, no atime/ctime updates
beyond what ext2 requires for a consistent tree.

## Requirements

### Storage

- **H1 (MUST):** The writable data lives in a dedicated ext2 image,
  `build/home.img`, 8 MiB, 1 KiB blocks, created by `xtask` with `mke2fs` and an
  initial tree from `samples/home/` (at least `readme.txt`).
- **H2 (MUST):** `xtask image` creates the image when missing and **never**
  overwrites an existing one unless `--reset-home` is passed; `xtask run` attaches
  it as an IDE disk (`-drive file=build/home.img,format=raw,if=ide`), and
  `--no-home` omits it.
- **H3 (MUST):** The kernel reaches it through a bounded ATA PIO driver on the
  primary bus (LBA28, 512-byte sectors, no DMA, no interrupts, polling with a
  timeout). An absent or unreadable disk is not an error: the system boots with
  `/home` read-only (the application image's `home` directory, H10).
- **H4 (MUST):** Writes are synchronous: a primitive returns only after the block
  cache has written the affected blocks. There is no unmount and no deferred
  write-back.
- **H5 (MUST):** After any session the image is consistent enough for
  `e2fsck -fn` to exit 0 (the host proof). Mounting marks the filesystem clean and
  does not increment the mount count.

### The writer

- **W1 (MUST):** A new `ext2w` crate (no_std, allocation-free) implements the
  read/write subset over a `SectorDevice` trait (`read_sector`/`write_sector` into a
  512-byte buffer, plus capacity). It is host-testable with an in-memory device.
- **W2 (MUST):** Supported ext2 features: 1 KiB blocks, revision 1, no journal, no
  extents, direct/single/double indirect blocks (the app image's subset, plus
  write). Mount validates the superblock, group descriptors, bitmaps and inode
  table; an unsupported or corrupt image is refused, never written.
- **W3 (MUST):** Operations: lookup, read, create a regular file, write at an
  offset (overwrite, append, and zero-fill a gap), remove a file, remove an empty
  directory, create a directory, rename a file or directory within and across
  directories, and replace an existing regular file by rename.
- **W4 (MUST):** Allocation updates the block and inode bitmaps, the group
  descriptor free counts and the superblock free counts, in that order; freeing
  reverses it. A full filesystem is `Status.CapacityExceeded` and leaves the tree
  unchanged.
- **W5 (MUST):** A directory entry is added in existing free space when possible and
  in a new block otherwise; removal merges the entry's record length into its
  predecessor (or leaves a free slot), and an empty directory block is released.
  `mkdir` creates `.` and `..` and updates both link counts.
- **W6 (MUST):** `rename` replaces an existing regular-file target atomically from
  the caller's point of view (old target unlinked, new link written, inode moved);
  a directory target that is not empty is `Invalid`. A cross-directory move updates
  the `..` entry of the moved directory.
- **W7 (MUST):** File data is written block by block; a partial block preserves the
  bytes outside the written range. A write past the end extends the size and
  zero-fills the gap. A write inside a file does not shrink it; `core.fs_truncate`
  (A7) is the only shrinking operation.

### API

- **A1 (MUST):** Five new `core` host builtins (no opcode; ABI v1 stays at 75):
  ```text
  core.fs_write(path: const string[0]; offset: uint;
                data: const bytes[0]; var written: int) -> Status
  core.fs_remove(path: const string[0]) -> Status
  core.fs_mkdir(path: const string[0]) -> Status
  core.fs_rename(from: const string[0]; to: const string[0]) -> Status
  core.fs_truncate(path: const string[0]; size: uint) -> Status
  ```
  They are builtins 53–57, available only on a writable mount.
- **A2 (MUST):** Only paths whose first component is `home` are writable. A
  `create`/`write`/`remove`/`mkdir`/`rename`/`truncate` on any other path is
  `Status.Invalid`; reads elsewhere are unchanged. Without a home disk the same
  calls are `Invalid` (the mount is read-only).
- **A3 (MUST):** `fs_write` writes `min(data.len, …)` bytes, returns the count in
  `written`, and leaves every out-parameter untouched on failure. Creating a
  missing file is implicit for `fs_write` with offset 0; a path that names a
  directory is `Invalid`; a parent that does not exist is `NotFound`.
- **A4 (MUST):** `fs_list`, `fs_read` and `fs_size` see the home tree for `home/*`
  and the application image elsewhere. `/` lists `home` once; listing `home` is the
  home image's root.
- **A5 (MUST):** `core.spawn`/`core.run` and the compiler read a `/home` source
  through the same router, so a user can write a program to `/home` and run it.
- **A6 (MUST):** `langhost` implements the home mount over a temporary directory
  (not the repository), so the write primitives and the editor save are testable
  on the host; a bare host keeps `Invalid`.
- **A7 (MUST):** `core.fs_truncate(path, size)` sets a regular file's length:
  shrinking frees every data block at or past `size` (direct/single/double
  indirect, including empty pointer blocks), clears the inode's pointers, zeroes
  the tail of the last partial block, and only then releases the bitmaps and
  counters; extending allocates and zero-fills up to `size` with capacity planned
  before any mutation (a full disk is `CapacityExceeded` with no change). A
  directory is `Invalid`, a missing path `NotFound`, and the current size is an
  `Ok` no-op. Only `home/*` (A2).

### Editor and browser

- **E1 (MUST):** The editor's `Ctrl+S` and its File-menu Save row write the
  document through `fs_write` after a `remove`/`rename` replacement (write a
  sibling temporary file, then rename it over the original), report `saved` or the
  failure status, and keep the document unchanged on failure. The temporary-file
  + rename path is kept even though `fs_truncate` exists: it is the closest thing
  to an atomic save without a journal, and a write+truncate save would leave a
  mixed document if a power loss hit between the two calls. An unnamed New File
  and File > Save As run the `/lib` Save File dialog first; its chosen `home/`
  target then goes through the same temporary-file + rename path, and an
  existing target is refused rather than overwritten.
- **E2 (MUST):** The browser can create a directory and remove an empty directory
  or a file in the home tree, with the same confirmation-free status line it uses
  today; operations outside `home` are refused.
- **E3 (MUST):** `base/touch <path>` creates an empty regular file under `home/`
  (one-byte `fs_write` at offset 0, then `fs_truncate(path, 0)`) and reports
  `created`; an existing regular file is a successful no-op reported as
  `exists` and is never truncated, a directory or other non-regular kind is
  `not a regular file`, a missing parent is `no such directory`, and a path
  outside `home` is `read-only`.

### Verification

- **V1 (MUST):** Boot self-check `ext2w: home ok` after a successful mount: read the
  initial file, create a scratch file, write, read it back, remove it, and check
  the free counts returned to their start.
- **V2 (MUST):** A persistence proof: write a file from an app, reboot the guest
  (or power-cycle the QEMU process) with the same `build/home.img`, boot, and read
  the file back with the same contents.
- **V3 (MUST):** `e2fsck -fn build/home.img` exits 0 after the persistence test
  (read-only check) and after a remove/rename/mkdir session.
- **V4 (MUST):** The full regression set still passes on `-smp 4` and `-smp 1`,
  with and without the home disk; the visual golden and the boot desktop are
  unchanged when no disk is attached.

## Design

### Why a separate image

The application image is a read-only GRUB module in RAM (ADR 0015). Writing it
would not persist and would let an application change the code it runs. A second
image attached as a disk gives persistence, keeps the app image immutable, and
matches the user's layout: `/home` is user data, everything else is system.

### Router

`kernel/src/fs.rs` resolves a path by its first component:

```
home            -> the writable ext2w mount (or the app image's home when absent)
home/...        -> the writable mount
anything else   -> the read-only application image (spec 0013)
```

`/` is listed from the application image (which contains `home`), `home` from the
writable mount. The REPL's source read, `core.spawn`/`core.run` and the file
builtins all go through the router, so no caller knows there are two filesystems.

### Device and cache

`kernel/src/ata.rs` drives the primary master through ports `0x1F0..=0x1F7` and
`0x3F6`: identify, LBA28, 512-byte read/write with a bounded poll of BSY/DRQ and a
reset on timeout. `ext2w` keeps a small fixed block cache (a handful of 1 KiB
blocks) with write-through on every mutation; there is no write-back queue, so a
return from a primitive means the disk has the blocks. The driver and the
filesystem are separated by the `SectorDevice` trait so a virtio-blk driver can
replace ATA later without touching `ext2w`.

### Consistency and failure

ext2 without a journal cannot promise crash consistency. The design mitigates the
common cases by ordering: data blocks first, then the inode (size and pointers),
then the directory entry; allocation updates the bitmaps before the block is used;
freeing updates the inode and directory before the bitmap. A power loss between
those steps may leave an orphaned block or a stale directory entry, which
`e2fsck -fn` reports (and a future `-fy` can repair). The kernel never panics on a
corrupt structure; it refuses the mount or returns `IoError`.

## Changelog

| Date | Change |
|------|--------|
| 2026-11-18 | Initial draft and acceptance: dedicated 8 MiB ext2 home image, ATA PIO, `ext2w`, the router, four write builtins (53–56), no truncate, no journal, `e2fsck` as the consistency proof. |
| 2026-11-18 | Implemented in M6.1–M6.4: read mount and router, the writer and `fs_write`, `remove`/`mkdir`/`rename` with the editor save and browser actions, and the reboot persistence harness. ABI v1 stays at 75 opcodes; the kernel's application image remains read-only. |
| 2026-11-18 | M7.1 adds `core.fs_truncate(path, size)` (builtin 57, ADR 0041): shrink frees blocks/pointers and zeroes the partial tail, extend zero-fills, only `home/*`; the editor keeps its temporary-file + rename save. ABI v1 stays at 75. |
| 2026-11-27 | `base/touch` (user request "touch komutu istiyorum"): creates an empty `home/` file with the browser's one-byte write + `fs_truncate(0)` pair. An existing regular file is a successful no-op and is never truncated; a directory is refused, a missing parent reports `no such directory`, and a non-home path is `read-only`. The command joins the shell's cwd-aware file-command group and `help` list. No file API, opcode, builtin or kernel change. |

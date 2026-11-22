# 0025: The application image on a block device

- **Status:** Accepted
- **Scope:** `xtask`, `kernel` (`main`, `platform/x86_64/ata`, `vfs/fs`, `vfs/ext2`),
  `ext2w`, `scripts`; boot delivery of `samples/apps`
- **Supersedes:** none (amends spec 0018 X1–X2 delivery; spec 0019 unchanged)
- **Superseded by:** none
- **ADRs:** 0014, 0015 (superseded by 0059), 0019, 0059
- **Updated:** 2026-11-22

## Context

ADR 0015 ships the application image as a GRUB **module**: `grub.cfg` calls
`module2 /boot/apps.img apps`, GRUB reads the file from the boot medium into guest
memory, and the kernel reads that memory window through `vfs/ext2` as a borrowed slice.
The decision was correct while the image held a few megabytes of source: no block
device, no DMA, no cache, no locking, and the compiler's `/lib` reader was a copy from
RAM.

M14 changed the size of the image. The PPM corpus is 123,045,476 B of the 128 MiB
filesystem, so every boot now asks GRUB to move 128 MiB from the emulated CD into guest
memory before the kernel starts. Measured on `-smp 4`, TCG, 1024x768:

| Time | Event |
|---|---|
| 0.0 s | QEMU starts; SeaBIOS text mode |
| 0.5 s | GRUB prints `Booting 'igis-v2'` and starts reading the module |
| 10.39 s | kernel entry (`igis-v2: long mode entered`) |
| 10.42 s | `console::init` presents the desktop dither and the System Log |
| 10.7 s | image tree check, home mount, SMP bring-up |
| 10.75 s | `system/entrypoint` compiled (66 ms) and spawned |
| 10.78 s | `desktop_shell: ready` |

Two costs follow. The visible one is 9.9 s of frozen firmware text; the invisible one is
that 128 MiB of the 256 MiB guest is resident for the life of the system (`available:
101 MiB`). ADR 0015 named this exact risk and left the escape in writing: "Later block
devices (virtio-blk or ATA) implement the same `BlockDevice` trait; the filesystem
driver does not change".

The home disk already proves the other half of the path: `ext2w` mounts ext2 over a
`SectorDevice` (ATA PIO) and is host-tested. What is missing is that the *system* image
uses the same route. The system image is read-only and may be mounted read-only.

Non-goals: changing the image contents (spec 0018/0024), the ext2 subset, the ext2
format, the `/home` volume, the ABI, or the desktop's boot order (ADR 0029).

## Requirements

### Boot medium

- **B1 (MUST):** The boot medium carries the kernel and the bootloader's own files
  only. `grub.cfg` contains no `module`/`module2` command and `build/igis.iso` contains
  no application image.
- **B2 (MUST):** GRUB hands the kernel no byte of the application image; the kernel
  reaches it only through the block device described in B3.

### Volume and device layout

- **B3 (MUST):** The application image is a raw, unpartitioned ext2 volume on its own
  ATA device, with this fixed layout on every harness and in `xtask run`:

  | Bus / unit | Ports | Drive | Device | Mode |
  |---|---|---|---|---|
  | `ide0` unit 0 | `0x1F0..=0x1F7`, `0x3F6` | `0xE0` | `/home`, 8 MiB (spec 0019) | read-write |
  | `ide0` unit 1 | `0x1F0..=0x1F7`, `0x3F6` | `0xF0` | application image, 128 MiB | read-only |
  | `ide1` unit 0 | `0x170..=0x177`, `0x376` | `0xE0` | boot CD | bootloader only |

- **B4 (MUST):** The image is the same ext2 tree as today: 1 KiB blocks, revision 1,
  `INCOMPAT_FILETYPE` as the only incompatible feature, built by `xtask image` from
  `samples/apps` and the generated `samples/image` corpus (spec 0018 X1–X2).
- **B5 (MUST):** The two drives on `ide0` share one register set, so every port
  sequence on that channel (identify, read, write) runs under one kernel lock, with the
  drive selected per command. There is no per-device channel lock.

### Mount and reads

- **B6 (MUST):** The kernel mounts the volume read-only at boot, after the APIC clock is
  running, before any AP starts, and before the REPL publishes a boot entry.
- **B7 (MUST):** One ext2 implementation serves both mounts: the application image and
  `/home` are `ext2w::Fs` mounts. `vfs/ext2` and the module byte-slice path are
  removed, together with the `'static` borrow extension in `fs::init`.
- **B8 (MUST):** Reads are on demand. No part of the image is copied into a kernel
  buffer at boot and no file content is cached; a failed read returns a status and never
  faults. `available` memory at boot reports the image absent (about 229 MiB free in a
  256 MiB guest instead of about 101 MiB).
- **B9 (MUST):** The file router takes one mount lock per call and never holds two. The
  lock order is `COMPILER_LOCK → SYSTEM|HOME → ATA channel`; console, window, graphics
  and scheduler locks are never taken below a mount lock. No interrupt handler calls
  the file API.
- **B10 (MUST):** The boot self-check over the new reader proves the same three facts as
  today (`tree ok`): `base/shell` exists and is a regular file, its first byte streams,
  and the root lists `base`, `apps`, `home`, `lib`, `system` and `image`.

### Failure

- **B11 (MUST):** A missing, unreadable or invalid volume is a named, non-faulting boot
  failure: `apps: no image` or `apps: mount FAIL (<status>)`, followed by the existing
  `no entry` halt. No panic, no guard hit, no window opened.
- **B12 (MUST):** The volume is attached so that guest writes can never reach the
  image file: every harness passes `snapshot=on` and `xtask run` does the same. QEMU's
  `ide-hd` refuses a read-only block node (`readonly=on` fails with "Block node is
  read-only"), so the temporary overlay is the equivalent protection. A boot must leave
  `apps.img` byte-identical (checked with `md5sum`). The kernel mounts the volume
  read-only regardless.

### Build and harnesses

- **B13 (MUST):** `xtask image` builds `build/apps.img` (product) or
  `build/apps-shell.img` (`--shell`) and a kernel-only ISO; the image contents are
  unchanged.
- **B14 (MUST):** `xtask run` attaches home at `ide0` unit 0, the matching application
  image at `ide0` unit 1 and the ISO as the CD; `--no-home` omits only home and
  `--shell` selects the shell image and ISO.
- **B15 (MUST):** One shared helper in `scripts/tablet_qmp.py` returns the drive
  arguments for an ISO path, mapping `igis.iso` to `apps.img` and `igis-shell.iso` to
  `apps-shell.img`. Every QEMU harness uses it; no script can boot a product ISO with a
  shell image or no image.
- **B16 (SHOULD):** On TCG with four cores, `desktop_shell: ready` arrives within 2.5 s
  of QEMU start (measured 10.4 s before the change). The measured value is recorded in
  the ticket's status row.

### Unchanged

- **B17 (MUST):** No ABI change: no opcode and no builtin, ABI v1 stays 75. The
  application-facing file API (spec 0013) and the compiler's `/lib` reader do not
  change.
- **B18 (MUST):** A write to any path outside `home/...` still returns `Invalid`
  (spec 0019 A2).

## Design

### Boot sequence

```
GRUB: multiboot2 /boot/kernel.elf; boot            (no module)
kmain  -> frame, gfx, console, self-checks, APIC clock
kmain_on_stack
  ata::init(Unit::Master)  -> home disk   (spec 0019)
  ata::init(Unit::Slave)   -> apps disk
  ext2w::mount(apps)       -> fs::init()  (read-only, published, tree ok)
  ext2w::mount_write(home) -> fs::mount_home()
  SMP bring-up, stream, task self-checks
  repl::boot               -> system/entrypoint
```

The mount moves from `mb2::module` to a device read. The order is otherwise unchanged,
so the APs still start after both mounts are published.

### Memory and time

| Quantity | Before | After |
|---|---|---|
| GRUB reads at boot | kernel + 128 MiB image | kernel only |
| Guest memory resident for the image | 128 MiB | none (page-cache-free PIO reads) |
| `available` in a 256 MiB guest | 101 MiB | about 229 MiB |
| Time to `desktop_shell: ready` | 10.4 s | target ≤ 2.5 s |

### Reader and locking

`ext2w::Fs<D: SectorDevice>` already reads a whole file, reads at an offset, lists a
directory, reports a size and finds a path. It validates the 1 KiB, revision-1,
`INCOMPAT_FILETYPE` subset and refuses anything else. The router holds
`SpinLock<Option<Fs<AtaDisk>>>` for the application image and the existing one for home;
a call takes exactly one of them, does its reads, and releases. `ext2w`'s block cache is
a `RefCell` and is not `Sync`; the mount lock is what makes it shareable across cores
(OPEN-QUESTIONS Q59). This matches the home mount's existing rule.

The ATA driver gains a `Unit` selector and one channel lock. `AtaDisk` keeps
`read_sector`/`write_sector` from `SectorDevice`; the channel lock is taken inside them,
so two mounts cannot interleave register writes.

## Interfaces

```rust
// kernel/src/platform/x86_64/ata.rs
pub enum Unit { Master, Slave }
pub fn init(unit: Unit) -> Option<AtaDisk>;

// kernel/src/vfs/fs.rs
pub fn init(apps: ext2w::Fs<AtaDisk>);   // read-only mount, published for all cores
pub fn mount_home();                      // unchanged: primary master, writable
```

`fs::list` keeps returning the router's own `Entry` (name buffer, name length, size,
kind), mapped from `ext2w::Entry`; callers do not change.

## Failure modes

| Condition | Result |
|---|---|
| `ide0` unit 1 absent | `apps: no image`, then `no entry` halt (B11) |
| Superblock bad or subset unsupported | `apps: mount FAIL (Invalid)`, then B11 |
| PIO timeout or device error during a read | the call returns `IoError`; the compiler reports it as a diagnostic |
| Image larger than LBA28 addresses | the driver refuses the device (128 GiB limit; the image is 128 MiB) |
| Host writes to the image | impossible: `readonly=on` (B12) and a read-only mount |
| IRQ handler calls the file API | deadlock; forbidden by B9 and documented at the mount |

## Alternatives considered

| Alternative | Why not |
|---|---|
| Keep the module, shrink the corpus | still a boot-time copy and still resident; fixes only the photo case |
| virtio-blk for the image | a second block driver for the same job the ATA path already does; keep it for the DMA follow-up |
| Partition table on the home disk | adds an MBR parser, offset-aware fsck and a format change for no gain |
| Boot from the image disk, no ISO | requires embedding GRUB into a raw image and changes every harness invocation |
| GRUB-side splash only | hides the wait instead of removing it; static text for ten seconds |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Should `xtask image` also run `e2fsck -fn` on `apps.img`? | owner; cheap, catches a bad corpus stage |
| 2 | Is the 1 KiB block size still the right choice for a 128 MiB volume? | after the change, if reads measure badly |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-22 | Initial draft: kernel-only boot medium, read-only ATA volume, one ext2 reader. |
| 2026-11-22 | Accepted by the owner in the boot-image session; implementation is M15 (PLAN.md). |
| 2026-11-22 | B12 amended after M15.2: QEMU's `ide-hd` refuses `readonly=on`, so the image is attached with `snapshot=on` and a boot must leave the file byte-identical. |

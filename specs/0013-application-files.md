# 0013: The application file API

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`ext2`, `sched`, `repl`),
  `langhost`, `samples/apps`; M2.5.1 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0015 (read-only ext2 delivery), 0019 (source-only delivery), 0027
- **Updated:** 2026-11-17

## Context

Until M2.5.1 only the kernel reads the application image: the boot path and the
REPL service compile `shell`/`entrypoint` and every `core.spawn` request from
ext2. The text editor (M2.5.3) and the file browser (M2.5.4) are applications,
so they need a read-only view of the same image. There is no writable
filesystem, no block-device handle for applications, and no path traversal below
the image root (spec 0002, "ext2 subset").

The driver is already the right shape: `Ext2` reads from an in-memory
`BlockDevice`, allocates nothing, validates every block number and streams an
inode's data blocks. M2.5.1 adds one indexed root-directory read and one
offset read, then exposes them through three bounded `core` builtins. It does
not add a file-descriptor model, a working directory, a writable API, or
subdirectory support.

## Requirements

### Surface and limits

- **F1 (MUST):** The application surface is exactly three `core` builtins, with
  the predeclared `Status` result (spec 0003, E1):

  ```text
  core.fs_list(path: const string[0]; index: uint;
               var name: string[64]; var kind: uint; var size: uint) -> Status
  core.fs_read(path: const string[0]; offset: uint;
               var dst: bytes[256]; var read: int) -> Status
  core.fs_size(path: const string[0]; var size: uint) -> Status
  ```

  They are host builtins 41, 42 and 43. No bytecode opcode is added; the ABI v1
  opcode table stays at **75** (spec 0008).
- **F2 (MUST):** Every path is a `string` (valid UTF-8), relative to the image
  root, at most 256 bytes, with no leading or trailing `/`, no empty component
  and no `..` component. A longer path is `Status.CapacityExceeded`; any other
  violation is `Status.Invalid`. The syntax validator is
  `abi::fs::validate_path`, the same function the REPL service uses for
  `core.spawn`, so there is one definition of "malformed path".
- **F3 (MUST):** M2.5.1 exposes the **root directory only**. `core.fs_list`
  accepts `""` or `"."` and reports every other path as `Status.Invalid`.
  `core.fs_read` and `core.fs_size` accept one root component with no `/`
  (`.` is syntactically one component but names the root directory, which is
  not readable, so the driver reports `Status.Invalid`). A path with a
  separator is `Status.Invalid`: subdirectories are unsupported, not merely
  missing. This is a documented limit of the slice, not an error mapping.
- **F4 (MUST):** Matching is exact, case-sensitive and byte-for-byte against
  the directory entry names. There is no glob, no extension convention, no
  hidden-file rule and no working directory.
- **F5 (MUST):** No primitive allocates. The interpreter stages a 64-byte name
  and a 256-byte read chunk on its own frame; the driver streams blocks into
  the caller's buffer. There is no per-open state, no descriptor table and no
  file cache.
- **F6 (MUST):** A failure writes **nothing** through an out-parameter. The
  language sees the old `name`/`kind`/`size`/`dst`/`read` values unchanged;
  only a successful call publishes new bytes.

### Listing

- **F7 (MUST):** `core.fs_list` returns the `index`-th named entry of the root,
  zero-based, in **directory order** (the on-disk order; it is not sorted).
  `.` and `..` are excluded. An index at or past the last entry is
  `Status.NotFound`.
- **F8 (MUST):** On `Ok` the call writes the entry name (at most 64 bytes), its
  kind and its inode size, and sets the name's runtime length. A name longer
  than 64 bytes is `Status.CapacityExceeded`; no name is truncated.
- **F9 (MUST):** `kind` is `0` for a regular file, `1` for a directory and `2`
  for any other inode (device, socket, symlink). The ext2 image built for M2
  contains the staged source files and, because `mke2fs` creates it, a
  `lost+found` directory; a browser must skip kind `1`, and the read primitives
  refuse it. `size` is the inode's byte size; for a directory it is the
  directory data size, not an entry count.

### Reading and size

- **F10 (MUST):** `core.fs_read` copies `min(256, size - offset)` bytes of a
  regular root file starting at byte `offset`. The compiler requires a
  `var bytes[N]` destination with `N >= 256`; the interpreter also checks that
  the referenced region has room for a whole 256-byte chunk before the host is
  called. A smaller destination is a compile error (or a structural fault for
  hand-made bytecode), never a partial read.
- **F11 (MUST):** On `Ok` the destination's runtime length and the `read`
  `int` both hold the copied count. An offset at or past the file's end is
  `Ok` with 0 bytes; end-of-file is not an error. An offset below the file's
  end returns at least one byte, so a caller can loop until it sees 0.
- **F12 (MUST):** The bytes are raw data: a `bytes` destination, no UTF-8
  validation. An application that wants text converts through the checked
  slice (`core.substr_into` with a `string` destination, or
  `stdlib.utf8_decode_into`).
- **F13 (MUST):** `core.fs_size` writes the byte size of a regular root file.
  It is the same `Ext2::file_size` the REPL service uses, so a size and a
  chunked read agree.
- **F14 (MUST):** The error statuses are fixed:

  | Situation | Status |
  |-----------|--------|
  | path longer than 256 bytes | `CapacityExceeded` |
  | malformed path (`/`, `//`, `..`, empty component) | `Invalid` |
  | `fs_list` path other than `""`/`"."`, or a path with `/` on read/size | `Invalid` |
  | name not in the root | `NotFound` |
  | entry is a directory or another non-regular kind (read/size) | `Invalid` |
  | entry name longer than the 64-byte destination (list) | `CapacityExceeded` |
  | list index at or past the last entry | `NotFound` |
  | corrupt inode, pointer block or data block | `IoError` (or `Invalid` for a hole) |

## Design

### One stateless view

A call names a file every time; there is no `open`/`close`, no per-task handle
and no offset carried by the kernel. The editor holds its own offset, buffer and
selection, which is exactly the state the M2.4.1 string helpers already model.
The cost is one root-directory lookup per read; source files are small and the
slice is not performance-critical. ADR 0027 records why handles were rejected
for M2.

### Path rules live once

`abi::fs` contains the limits (`PATH_MAX` 256, `NAME_CAP` 64, `READ_CHUNK` 256,
`KIND_FILE`/`KIND_DIR`/`KIND_OTHER`) and three validators:

- `validate_path` — the spawn syntax rule; the REPL service calls it, so
  `core.spawn` and the file primitives cannot drift apart;
- `validate_root` — `""` or `"."` for listing;
- `validate_root_name` — the spawn syntax plus "no `/`" for reading and size.

The interpreter calls them before the host, so a host with no filesystem still
reports a malformed path as `Invalid` rather than `NotFound`. The kernel host
re-checks the M2.5.1 subset defensively before touching the driver.

### Driver additions

Two methods join the existing `read_file`/`file_size`/`list`:

```rust
pub struct RootEntry {
    pub name_len: usize,
    pub kind: u64,
    pub size: u64,
}

impl Ext2<'_> {
    pub fn list_entry(&self, index: u32, name: &mut [u8]) -> Result<RootEntry, Status>;
    pub fn read_at(&self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status>;
}
```

`list_entry` walks the root's directory blocks until the index-th named entry,
resolves its inode for the kind and size, and copies the name into the caller's
buffer. `read_at` streams the inode's data blocks in order and copies only the
intersection of `[offset, offset + out.len())` with the file; the blocks before
the offset are skipped without a buffer and the walk stops copying once the
destination is full. Neither method allocates, and both use the existing
bounds-checked block/pointer helpers.

`mke2fs -d` also creates `lost+found` in the image root. The listing reports it
as kind `1`; the read and size primitives refuse it as `Invalid`. The image
builder continues to stage regular source files only.

### Host contract

The interpreter trait gains three methods with a no-filesystem default:

```rust
fn fs_list(&mut self, path: &[u8], index: u64, name: &mut [u8])
    -> Result<FsEntry, Status> { Err(Status::NotFound) }
fn fs_read(&mut self, path: &[u8], offset: u64, dst: &mut [u8])
    -> Result<usize, Status> { Err(Status::NotFound) }
fn fs_size(&mut self, path: &[u8]) -> Result<u64, Status> { Err(Status::NotFound) }
```

The machine passes a local `[u8; 64]` name buffer and a local `[u8; 256]` read
chunk, validates the path and the destination regions, calls the host, and
publishes the name/chunk with one copy only on `Ok`. `FsEntry` carries
`name_len`, `kind` and `size`.

The kernel host implements the three methods over the mounted `Ext2` the REPL
service already holds. The `langhost` host implements the same root-only view
over `samples/apps`: listing is sorted by name for determinism (a program may
not depend on the order), reads use the same 256-byte chunk, and directories or
symlinks are refused exactly as the kernel subset does. A bare test host keeps
the `NotFound` defaults.

## Interfaces

| Layer | Contract |
|-------|----------|
| Language | the three signatures in F1; `Status` results; `name`/`kind`/`size`/`dst`/`read` written only on `Ok` |
| Compiler | checks the path is a `string`, `uint` scalars, `var string[>= 64]` name, `var bytes[>= 256]` destination, `var int` read, `var uint` size; emits builtins 41–43 |
| Interpreter | path validation, destination region checks, 64/256-byte staging, one-copy publication, host dispatch |
| Kernel host | `Ext2::list_entry`/`read_at`/`file_size`; root-only; no allocation |
| `langhost` | the same view over `samples/apps`, sorted listing |
| REPL | unchanged; `read_file`/`file_size` and `validate_path` (now the shared `abi::fs` function) |

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| `fs_list("child", …)` or `fs_list("a/b", …)` | `Invalid`; nothing written |
| `fs_read("a/b", …)` or `fs_size("a/b", …)` | `Invalid`; subdirectories are unsupported |
| `fs_read(".", …)` or `fs_size(".", …)` | `Invalid`; the root is a directory, not a regular file |
| `fs_read("lost+found", …)` | `Invalid`; directories are listed but not readable |
| missing name | `NotFound`; out-parameters untouched |
| empty file, offset 0 | `Ok`, 0 bytes, destination length 0 |
| offset at or past the end | `Ok`, 0 bytes |
| `read` destination below 256 bytes | compile error (`bytes[128]`), not a runtime status |
| hand-made bytecode with a short destination region | `FaultCode.OutOfBounds`; the host is not called |
| name longer than 64 bytes in the listing | `CapacityExceeded`; nothing written |
| list index past the end | `NotFound`; nothing written |
| corrupt image block | `IoError` (hole: `Invalid`) |
| host with no filesystem | `NotFound` for every call |
| path longer than 256 bytes | `CapacityExceeded` |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Subdirectory listing and traversal | M2.5.4, if the browser must show a tree; add a new spec or amend this one |
| 2 | A writable save path for the editor | when a writable filesystem exists (spec 0002, deferred) |
| 3 | A file handle / open-once model | only if per-read root lookup becomes a measured problem |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial spec (M2.5.1): three root-only `core` builtins, 256-byte reads, `Status` errors, atomic out-parameters, the `abi::fs` path rules, `Ext2::list_entry`/`read_at`, kernel and `langhost` hosts. |

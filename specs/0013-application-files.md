# 0013: The application file API

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`ext2`, `sched`, `repl`),
  `langhost`, `samples/apps`; M2.5.1 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0015 (read-only ext2 delivery), 0019 (source-only delivery), 0027
- **Updated:** 2026-11-18

## Context

Until M2.5.1 only the kernel reads the application image: the boot path and the
REPL service compile `shell`/`entrypoint` and every `core.spawn` request from
ext2. The text editor (M2.5.3) and the file browser (M2.5.4) are applications,
so they need a read-only view of the same image. There is no writable
filesystem and no block-device handle for applications. M2.5.1 exposed the image
root only; M3.1 (spec 0018) generalised the view to the whole tree
(`lib`, `base`, `home`, `system`) with a path walk, and M3.4 teaches the browser
to descend into directories.

The driver is already the right shape: `Ext2` reads from an in-memory
`BlockDevice`, allocates nothing, validates every block number and streams an
inode's data blocks. It exposes three bounded `core` builtins over an indexed
directory read and an offset read. There is no file-descriptor model, no working
directory and no writable API.

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
- **F3 (MUST):** The view is the read-only image tree (spec 0018, L1/L6).
  `core.fs_list` accepts `""` or `"."` for the root or any directory path;
  `core.fs_read` and `core.fs_size` accept any regular-file path. Each component
  is at most 64 bytes, a path has at most 8 components, and a component that is
  missing, is a directory where a file is required, or is a non-regular kind is
  reported per F14. There is no `..`: a caller that needs a parent truncates the
  path it already holds.
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

- **F7 (MUST):** `core.fs_list` returns the `index`-th named entry of the
  directory at `path` (`""`/`"."` is the root), zero-based, in **directory
  order** (the on-disk order; it is not sorted). `.` and `..` are excluded. An
  index at or past the last entry is `Status.NotFound`; a path that is not a
  directory is `Status.Invalid`.
- **F8 (MUST):** On `Ok` the call writes the entry name (at most 64 bytes), its
  kind and its inode size, and sets the name's runtime length. A name longer
  than 64 bytes is `Status.CapacityExceeded`; no name is truncated.
- **F9 (MUST):** `kind` is `0` for a regular file, `1` for a directory and `2`
  for any other inode (device, socket, symlink). The image tree contains the
  four top-level directories, the staged source files and, because `mke2fs`
  creates it, a `lost+found` directory; a browser must not read kind `1` or
  kind `2` entries. `size` is the inode's byte size; for a directory it is the
  directory data size, not an entry count.

### Reading and size

- **F10 (MUST):** `core.fs_read` copies `min(256, size - offset)` bytes of a
  regular file starting at byte `offset`. The compiler requires a
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
- **F13 (MUST):** `core.fs_size` writes the byte size of a regular file at any
  path. It is the same `Ext2::file_size` the REPL service uses, so a size and a
  chunked read agree.
- **F14 (MUST):** The error statuses are fixed:

  | Situation | Status |
  |-----------|--------|
  | path longer than 256 bytes | `CapacityExceeded` |
  | malformed path (`/`, `//`, `.`/`..` component, empty component, over 8 components, component over 64 bytes) | `Invalid` |
  | `fs_list` path that is a regular file | `Invalid` |
  | missing component | `NotFound` |
  | entry is a directory or another non-regular kind (read/size) | `Invalid` |
  | entry name longer than the 64-byte destination (list) | `CapacityExceeded` |
  | list index at or past the last entry | `NotFound` |
  | corrupt inode, pointer block or data block | `IoError` (or `Invalid` for a hole) |

## Design

### One stateless view

A call names a file every time; there is no `open`/`close`, no per-task handle
and no offset carried by the kernel. The editor holds its own offset, buffer and
selection, which is exactly the state the M2.4.1 string helpers already model.
The cost is one directory walk per call (at most eight components); source files
are small and the slice is not performance-critical. ADR 0027 records why
handles were rejected for M2.

### Path rules live once

`abi::fs` contains the limits (`PATH_MAX` 256, `NAME_CAP` 64, `MAX_COMPONENTS`
8, `READ_CHUNK` 256, `KIND_FILE`/`KIND_DIR`/`KIND_OTHER`) and two validators:

- `validate_path` — the tree syntax rule (spec 0018 L6); the REPL service calls
  it, so `core.spawn` and the file primitives cannot drift apart;
- `validate_dir` — `""`/`"."` or a path that passes `validate_path` for listing.

The interpreter calls them before the host, so a host with no filesystem still
reports a malformed path as `Invalid` rather than `NotFound`. The kernel host
re-checks the rules defensively before touching the driver.

### Driver additions

Two methods join the existing `read_file`/`file_size`/`list`:

```rust
pub struct Entry {
    pub name_len: usize,
    pub kind: u64,
    pub size: u64,
}

impl Ext2<'_> {
    pub fn list_entry(&self, path: &str, index: u32, name: &mut [u8])
        -> Result<Entry, Status>;
    pub fn read_at(&self, path: &str, offset: u64, out: &mut [u8])
        -> Result<usize, Status>;
    pub fn find(&self, path: &str) -> Result<(u32, Inode), Status>;
}
```

`list_entry` walks the directory at `path` (`""`/`"."` is the root) block by
block until the index-th named entry, resolves its inode for the kind and size,
and copies the name into the caller's buffer. `read_at` walks the path, then
streams the inode's data blocks in order and copies only the intersection of
`[offset, offset + out.len())` with the file; the blocks before the offset are
skipped without a buffer and the walk stops copying once the destination is
full. Neither method allocates, and both use the existing bounds-checked
block/pointer helpers. A path walk never follows a symbolic link (the next
component then sees a non-directory and reports `Invalid`).

`mke2fs -d` also creates `lost+found` in the image root. The listing reports it
as kind `1`; the read and size primitives refuse it as `Invalid`.

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
service already holds. The `langhost` host implements the same tree view over
`samples/apps`: listing is sorted by name for determinism (a program may
not depend on the order), reads use the same 256-byte chunk, and a symlink or a
file used as a directory is refused exactly as the kernel subset does. A bare
test host keeps the `NotFound` defaults.

### The file browser (M2.5.4, navigation M3.4)

`samples/apps/base/browser` records the application-level contract over the
tree view:

- the browser shows **one directory at a time**. Its state is the current
  directory path (`""` is the root) and the selected index; the path line shows
  `igis:/` or `igis:/<dir>`. The listing is read with `fs_list` from index 0
  until `NotFound`, into 96 fixed entries; `.`/`..` never appear and no sorting
  is assumed. A directory with more than 96 entries reports a truncated listing
  rather than dropping silently;
- a kind-1 entry is shown as a folder. `Enter` (or a double-click) **descends**
  into it (`dir := dir + "/" + name`, or `name` at the root). `Backspace` (or
  `Ctrl+Up`) goes up by truncating the last component of the current path; at
  the root it does nothing. There is no `..` path and no backtracking stack;
- a kind-0 entry is previewed on `Enter`/double-click (at most 2048 bytes via
  `fs_size` and 256-byte `fs_read` chunks, one strict-UTF-8 `substr_into`), and
  the previous preview is left byte-identical on every failure class in F14;
- `Ctrl+L` launches the selected regular file by its **full image path**
  (`dir + "/" + name`, or the name at the root) with `core.spawn` + `core.wait`;
  a directory is entered instead, and a kind-2 entry is refused. Launching a
  `/lib` source is allowed; the spawn-time compile failure appears in the System
  Log and the browser reports a fault (`core.run` copies the compiler diagnostic
  into `diag`, `core.spawn` + `wait` does not); that is the compiler's
  classification, not a browser rule;
- a kind-2 entry is shown but cannot be opened, entered or launched;
- path construction is bounded by `PATH_MAX`: a name that would not fit sets a
  status line and is not opened;
- no browser action writes a file or adds a handle.

## Interfaces

| Layer | Contract |
|-------|----------|
| Language | the three signatures in F1; `Status` results; `name`/`kind`/`size`/`dst`/`read` written only on `Ok` |
| Compiler | checks the path is a `string`, `uint` scalars, `var string[>= 64]` name, `var bytes[>= 256]` destination, `var int` read, `var uint` size; emits builtins 41–43 |
| Interpreter | path validation, destination region checks, 64/256-byte staging, one-copy publication, host dispatch |
| Kernel host | `Ext2::find`/`list_entry`/`read_at`/`file_size` over the tree; no allocation |
| `langhost` | the same view over `samples/apps`, sorted listing, symlinks refused |
| REPL | `read_file`/`file_size` and `validate_path` (the shared `abi::fs` function) |

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
| 1 | ~~Subdirectory listing and traversal~~ | resolved by spec 0018 (M3.1) and the M3.4 browser navigation |
| 2 | A writable save path for the editor | when a writable filesystem exists (spec 0002, deferred) |
| 3 | A file handle / open-once model | only if the per-call path walk becomes a measured problem |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial spec (M2.5.1): three root-only `core` builtins, 256-byte reads, `Status` errors, atomic out-parameters, the `abi::fs` path rules, `Ext2::list_entry`/`read_at`, kernel and `langhost` hosts. |
| 2026-11-17 | M2.5.4 note: the file browser consumes the root view and records the application contract — 64-entry listing until `NotFound`, kind-1 and kind-2 entries shown but refused, a 2048-byte preview with atomic failure, and a `core.spawn` + `core.wait` launch for regular entries. No API change. |
| 2026-11-18 | M3.1 amendment: the view is the image tree (spec 0018 L1/L6) — `fs_list` takes any directory path or `""`/`"."`, `fs_read`/`fs_size` take any regular-file path, components are ≤64 bytes, a path is ≤8 components, `validate_root`/`validate_root_name` are gone in favour of `validate_path`/`validate_dir`, and `Ext2::find` walks one directory at a time without following symlinks. The failure table and interface rows are updated. |
| 2026-11-18 | M3.4 amendment: the browser navigates — one directory at a time with a `dir` path, `Enter`/double-click descends into a kind-1 entry, `Backspace`/`Ctrl+Up` truncates the current path to go up (no `..`), preview and launch use the full path, the listing capacity is 96, and path construction is bounded by `PATH_MAX`. |

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
to descend into directories. Spec 0019 adds the four writable primitives for
`/home` (a real ext2 writer on a dedicated disk); the read primitives below are
unchanged.

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
- the path line carries a visible **`Up` button** at its right (a 44×18
  `stdlib.ui_button`, left of the writability label); a pointer-down on it goes
  up exactly like `Backspace`/`Ctrl+Up` (M8.8). The status hint names
  `Bksp up`, so the keyboard route stays discoverable;
- the scrollbar thumb can be **grabbed and dragged** (M8.9): a pointer-down in
  the thumb (hit class 3) records the offset inside the thumb, pointer moves
  while the primary button is held map the thumb top onto `top` over the travel,
  and release ends the drag. A track click still pages, and the thumb geometry
  matches `stdlib.ui_scrollbar` (track `BodyH - 4`, 8-pixel floor, travel
  `track - thumb`);
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

### The text editor (M2.5.3; lexical highlighting M9.1; pointer/scroll M9.2; file dialog M9.3; 128 KiB buffer M9.4)

`samples/apps/base/editor` records the application-level contract over the file
API. The document, editing and save rules are in spec 0003 ("Application
buffers above 256 bytes") and spec 0019 (E1); M9.1 adds a view rule, M9.2 a
drawing/drag rule, M9.3 the file-open dialog and M9.4 the document capacity:

- the document is one `string[131072]` (128 KiB) buffer, the spec 0008
  per-unit source cap. M9.4 raised the M2.5.3 8 KiB capacity because every
  current source sample is larger: `beat` (554 B) and `deep` (874 B) opened,
  while `base/calculator` (32,516 B) was refused by `open_path` with a
  `too large (8192 max)` status line after the file dialog had correctly
  returned the path. The largest current sample is `editor` itself (81,509 B).
  `open_path` checks `core.fs_size` first and refuses anything over 131072
  bytes with `too large (131072 max)`; nothing is truncated. The app-local
  mutator's full-buffer boundary still returns `CapacityExceeded`, and the
  per-line block-state cache is `bytes[131073]`;

- a successfully opened path whose name starts with `base/`, `lib/` or
  `system/`, or whose last three bytes are `.ig`, enables **syntax
  highlighting**. Any other text file (for example `home/readme.txt`) stays
  plain; the File menu's **Highlight** row toggles colouring for the current
  document either way and carries a check mark while it is on. Opening a file
  resets the toggle from the path rule;
- the File menu is five rows — `Open`, `Save`, `-`, `Highlight`, `Quit` — so the
  quit row is index 4. `Ctrl+O`/`Ctrl+S`/`Ctrl+Q` keep their meanings; Highlight
  has no shortcut. The dropdown's checked mask carries the Highlight bit and is
  redrawn with the menu;
- **File > Open and `Ctrl+O` (M9.3)** run the `/lib` modal file-open dialog
  (`lib.file_dialog.open_file`, spec 0018 "The `/lib` file-open dialog") with
  the current path as its start. The dialog begins in the current document's
  directory with the current file name selected, lists the image with
  `core.fs_list` (up to 96 entries; directories and regular files are
  selectable, kind-2 entries are shown gray and refused), and returns a
  regular-file image path or a null-like cancel (`Status.Invalid` with the
  caller's destination untouched). Keys are Up/Down, PageUp/PageDown, Home/End,
  Enter, Backspace/Ctrl+Up and Escape; a row click selects, a double-click
  opens, and the visible `Up`/`Open`/`Cancel` buttons, the scrollbar track
  (page) and thumb (drag) and the dialog's close box work. A cancel or any
  failed choice leaves the document, path, caret, selection, scroll and
  highlighting untouched; a successful choice is re-read through the same
  `open_path` as before, and the editor raises its own window afterwards. The
  editor's `PathCapacity` is `abi::fs::PATH_MAX` (256) so the dialog's whole
  result fits, and `TempCapacity` is 260 for `<target>.tmp`. The check and
  `langhost` paths keep calling `open_path` directly and never open the dialog;
  no opcode, builtin, `Host` method, file API or `stdlib` function is added;
- highlighting is **lexical only**, not a parser or type checker. One visible
  line is classified into: keywords (`unit`, `uses`, `const`, `var`, `type`,
  `record`, `end`, `procedure`, `function`, `begin`, `if`, `then`, `else`,
  `while`, `do`, `case`, `of`, `return`, `array`, `ring`, `map`, `in_stream`,
  `out_stream`, `value`, `and`, `or`, `not`, `div`, `mod`, `fault`, `exit`,
  `true`, `false`), predeclared type names (`int`, `uint`, `byte`, `bool`,
  `f64`, `string`, `bytes`, `Status`, `FaultCode`, `Diagnostic`, `ExitReason`,
  `Rect`, `Event`, `EventKind`), strings, comments, numbers and default text;
- the palette is keyword `0x000080`, type `0x800000`, string `0x008000`,
  comment `0x808080`, number `0x804000`; punctuation, whitespace and other
  identifiers stay black. Selected cells remain white on the accent band, and
  the caret, scrolling, line/column readout, editing and save behaviour are
  unchanged;
- `//` runs to the end of its line. `{ ... }` is a block comment that ends at
  the first `}`, does not nest, and may span lines. A `//` or `{` inside a
  string is string content. A string runs to its closing quote (`\"` is skipped)
  or to the line end, matching the language's single-line literals. An
  unterminated string or block comment colours to the end of its line and, for
  a block comment, carries the comment state to following lines;
- the block-comment state at every line start is cached in a `bytes[131073]`
  table (M9.2/M9.4), refreshed by one allocation-free line-by-line scan whenever
  the document is published (an open or an edit), not once per frame. `draw_lines`
  reads the entry for its first visible line and carries the state through the
  visible rows, so a scrolled frame scans only the visible lines, never the
  document from byte 0. The line-by-line scan still makes a `//` end at its own
  newline, so it never swallows a later line's `{`. With highlighting off the
  old all-black/all-white drawing path is used exactly;
- a pointer event repaints only when it changes visible state (M9.2). A
  no-button move over the text does nothing; a move with the File menu open
  repaints only when the highlighted row changes; a thumb-drag move only when
  `top_line` changes; a press or release only when it opens or closes the menu,
  moves the caret or selection, pages the view, toggles a menu item or starts or
  ends a visible drag (pressing the thumb itself need not repaint). Every event
  is still delivered and handled; only the redundant frame is skipped. Key
  events and an owner `Expose` still repaint as before;
- the scrollbar thumb drags (M9.2, matching the browser's M8.9). A press in hit
  class 3 records the grab offset; held primary-button moves map the thumb top
  onto `top_line` over the travel `track - thumb` and clamp it, and a release or
  a move without the button ends the drag. The geometry matches
  `stdlib.ui_scrollbar` (track `ScrollH - 4`, visible fraction with an 8-pixel
  floor, `Rows` visible against `line_count(text)`), and track clicks still
  page. A page or drag clamps the caret into the visible lines so the next key
  does not jump the view;
- the normal-task CPU budget is unchanged. The user-reported
  `Faulted(BudgetExceeded)` after 2,893 claims was the 10 s watchdog, not a
  crash: before M9.2 every pointer event addressed to the editor repainted the
  whole 640×480 panel and rescanned the document for the block state, so moving
  the pointer over the editor spent the task's lifetime budget on frames whose
  visible pixels did not change. M9.2 removes that work rather than raising the
  budget (spec 0007);
- token boundaries are whole UTF-8 code points and every drawn run still goes
  through `core.substr_into` and the existing `snap_boundary`, so a multi-byte
  code point is never split. Non-ASCII default text stays black;
- no file API call, handle, opcode, builtin, `Host` method or `stdlib` function
  is added. `editor check` covers source-vs-plain detection, the menu check,
  every palette class, a block comment that spans lines and one that starts
  above the visible first line, that the toggle off leaves no palette pixel,
  (M9.2) a scrollbar-thumb press and drag through the injected pointer path to
  the bottom (`top_line = line_count - Rows`, the drag ended and the thumb
  redrawn at the bottom), and (M9.4) `open_path("base/calculator")` returning
  `Ok` with the whole 32,516-byte document (`core.fs_size` agrees), the
  `base/calculator` path and valid UTF-8, followed by a restore of `base/child`;
  the host path has no screen and reports the same logical verdict.
  `scripts/editor_scroll_qmp.py` proves the budget rule on the real pointer
  path: a burst of no-button moves plus a held-button drag ends `exit Ok` with
  no `BudgetExceeded`, while the pre-M9.2 editor faults on the same burst.
  `scripts/file_dialog_qmp.py` proves the user-facing M9.4 case: the dialog
  loads `lib/math.ig`, then `base/calculator`, with the text region changing
  and the path line showing the longer chosen path, before Escape cancels and
  `Ctrl+Q` exits `Ok`.

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
| 2026-11-19 | M8.8: the file browser gains a visible `Up` button in the path line (44×18, left of the writability label) that goes to the parent through the real pointer path; `Backspace`/`Ctrl+Up` remain, and the status hint names `Bksp up`. `browser check` clicks the button, and `input_routing_qmp.py` requires the path to return to the pre-navigation root. No API change. |
| 2026-11-19 | M8.9: the file browser's scrollbar thumb is draggable. A pointer-down on the thumb (hit class 3) records the grab offset; pointer moves with the primary button held map the thumb top onto `top` across the travel, clamp the selection visible, and release ends the drag; track clicks still page. `scrollbar_thumb` matches `stdlib.ui_scrollbar` (track `BodyH-4`, 8-pixel floor); `browser check` drags the thumb to the bottom and `input_routing_qmp.py` proves it with a real mouse drag. No API change. |
| 2026-11-19 | M9.1: the text editor highlights source files lexically. `base/`, `lib/`, `system/` paths and `.ig` names start highlighted; other files stay plain. The File menu gains a checked `Highlight` row (five rows total) that toggles the view. Keywords are blue, type/predeclared names dark red, strings dark green, comments gray and numbers dark brown; selected text stays white on the accent band. `//` ends at its line, `{ ... }` spans lines without nesting, and strings do not span lines. The block-comment state is scanned line by line once per frame and carried through the visible rows; token runs use whole code points and the existing checked slices, and unhighlighted drawing is unchanged. No file API, opcode, builtin or `stdlib` change. |
| 2026-11-19 | M9.2: the editor repaints a pointer event only when it changes visible state, and its scrollbar thumb drags. The block-comment state at every line start is cached in a `bytes[8193]` table refreshed once per document publication, so a frame scans only the visible lines; the drag matches `stdlib.ui_scrollbar` and track clicks still page. The reported `Faulted(BudgetExceeded)` after 2,893 claims was the normal 10 s CPU watchdog, caused by a full repaint (and O(document) block scan) per pointer move; the fix removes the waste and leaves the budget, scheduler and realtime policy unchanged. `editor check` drags the thumb through the injected pointer path and `scripts/editor_scroll_qmp.py` proves the burst ends `Ok` with no budget fault. No file API, opcode, builtin, `Host` or `stdlib` change. |
| 2026-11-19 | M9.3: File > Open and `Ctrl+O` run the `/lib` modal file-open dialog (`lib.file_dialog.open_file`) and load its result through the existing `open_path`; a cancel (`Invalid`) leaves every editor state untouched. The dialog starts in the current directory with the current name selected, lists up to 96 entries (directories/files selectable, kind-2 gray and refused), navigates with keys, row click/double-click, `Up`/`Open`/`Cancel`, scrollbar paging and thumb dragging, and returns a regular-file path or a cancel. The editor's `PathCapacity` becomes 256 (`PATH_MAX`) and `TempCapacity` 260; the destination is published only on `Ok`. No file API, opcode, builtin, `Host` or `stdlib` change. |
| 2026-11-19 | M9.4: the editor's document buffer is 131072 bytes (128 KiB, the spec 0008 per-unit source cap), raised from the M2.5.3 8 KiB. The file-open dialog was already correct: it returned `base/calculator` (32,516 B), but `open_path` refused it with a `too large (8192 max)` status line, while the tiny `beat` (554 B) and `deep` (874 B) samples opened. Every current sample now fits (the largest is `editor` at 81,509 B); a file over 131072 bytes is still refused by size before it is read with `too large (131072 max)`, and an over-capacity edit still returns `CapacityExceeded`. The state grows to 530,392 B and the task stack to 174,328 B (the `ed_replace` staging buffer is now 128 KiB), both under the 1 MiB region caps; `editor check` proves `base/calculator` opens with the full size and valid UTF-8 and restores `base/child`, while `scripts/file_dialog_qmp.py` opens it through the dialog. No file API, opcode, builtin, `Host`, `stdlib` or kernel change. |

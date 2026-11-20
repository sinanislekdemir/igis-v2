# 0018: The image filesystem and importable library modules

- **Status:** Implemented
- **Scope:** abi, compiler, interpreter, kernel ext2/repl/sched, langhost, xtask,
  samples/apps, stdlib (unchanged)
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0038
- **Updated:** 2026-11-18

## Context

The M2 image is flat: every staged source sits directly in the ext2 root, and the
language has exactly two units — the application and the embedded `stdlib`
(spec 0008, "Multi-unit compilation"). Sample applications now repeat helper code
that `stdlib` does not own, and there is no place for user data or for
kernel/boot-owned applications.

This spec fixes the image layout and the module system before implementation:

- `/lib` — importable module sources,
- `/base` — the base applications a user launches,
- `/home` — user files,
- `/system` — kernel and boot-owned applications.

**Non-goals.** No writable filesystem (spec 0015 stands; `/home` is read-only in
this slice), no dynamic linking or runtime module loading (ADR 0002 and 0019
stand: a module is source compiled into the application's package), no package
versioning, no permissions, no symlinks, no per-user home directories, and no
mount table. `stdlib` stays embedded and is not moved into the image.

## Requirements

### Layout and path syntax

- **L1 (MUST):** The application image is one read-only ext2 tree (spec 0015) with
  the top-level directories `lib`, `base`, `apps`, `home` and `system`. The only
  other root entry is `mke2fs`'s `lost+found`.
- **L2 (MUST):** `lib` holds importable module sources named `<name>.ig`; nesting is
  allowed (`lib/ui/menu.ig`).
- **L3 (MUST):** `base` holds the command-line and session applications a user
  launches, and `apps` holds launchable GUI applications under a category
  directory (`apps/<category>/<name>`). A source file is
  `samples/apps/base/<name>.ig` or `samples/apps/apps/<category>/<name>.ig` (or
  the matching `samples/tests/apps/...` path) and the stager drops the `.ig`
  suffix, so the image name has no extension (`base/shell`,
  `apps/utilities/terminal`, `apps/productivity/editor`). `lib` sources keep
  `<name>.ig`.
- **L4 (MUST):** `home` holds user files and is read-only in this slice.
- **L5 (MUST):** `system` holds kernel and boot-owned applications
  (`system/entrypoint`, `system/system_probe`, `system/terminal_probe`).
- **L6 (MUST):** Every path in the file API, `core.spawn`/`core.run` and `uses` is
  relative: no leading `/`, no trailing `/`, no empty component, no `.` or `..`
  component, at most 256 bytes (`abi::fs::PATH_MAX`), at most 8 components, and
  each component at most 64 bytes (`abi::fs::NAME_CAP`).
- **L7 (MUST):** Directory listing returns direct children only; `.` and `..` are
  never returned. Entry names are at most 64 bytes.
- **L8 (MUST):** Symbolic links are never followed. A symlink or other non-regular,
  non-directory inode is listed with kind 2 and cannot be read through the API.
- **L9 (MUST):** The reader validates every directory entry and inode it walks
  (name length, inode bounds, size, block pointers) as today; an invalid tree is a
  mount or lookup failure, never a fault.
- **L10 (MUST):** Test-only sources live under `samples/tests/apps/{base,system}` as
  `<name>.ig` sources in the repository and are staged to the same image paths only
  for the test image (`xtask image --shell`). The product image carries only
  `samples/apps`. A test file name that collides with a shipped name is a staging
  error, not a silent shadow.

### Modules and `uses`

- **M1 (MUST):** A `uses` clause accepts the built-in unit names `core`, `syslib`
  and `stdlib`, and any number of dotted names whose first component is `lib`
  (`lib.math`, `lib.ui.menu`).
- **M2 (MUST):** `lib.a` resolves to the image path `lib/a.ig`; the dotted name is
  the path with `.` replaced by `/` and `.ig` appended. A name that resolves to a
  directory, a missing file, a kind-2 inode or a non-UTF-8 file is a compile
  diagnostic that names the application, the `uses` line and the resolved path.
- **M3 (MUST):** An imported source declares `unit <last dotted component>;`
  (`lib.ui.menu` → `unit menu;`). A mismatch, or a unit name that is not a valid
  identifier, is a compile diagnostic.
- **M4 (MUST):** Unit names in one package are unique. Two `uses` entries that
  resolve to the same image path are one unit; two different paths that declare the
  same unit name are a compile diagnostic naming both paths.
- **M5 (MUST):** A module may `uses core`, `syslib`, `stdlib` and `lib.*`. An
  import cycle, including a self-import, is a compile diagnostic that names the
  chain. A module may not import an application, and `stdlib` may not import
  `lib.*` (the embedded unit's `uses` rule is unchanged).
- **M6 (MUST):** A module may declare types, constants, procedures and functions.
  It MUST NOT declare top-level `var` state, stream endpoints, or `main`. Each is a
  compile diagnostic naming the module.
- **M7 (MUST):** A module's declarations are called with its unit name as the
  qualifier (`math.clamp(...)`, `menu.draw(...)`), exactly as `stdlib.*` works
  today. Unqualified calls resolve only to the current unit.
- **M8 (MUST):** Unit ids are fixed: the application is 0; the embedded `stdlib`
  is 1 whenever it is in the package (even if only a module imports it); module
  units are 2 up to `abi`'s unit cap of 16 in discovery order. Discovery is
  depth-first over `uses` clauses in declaration order, starting at the
  application, so ids depend only on source text. The verifier accepts unit ids
  `0..=16`.
- **M9 (MUST):** Type rules across units follow spec 0008 and ADR 0039: the
  predeclared `Status`, `FaultCode`, `EventKind`, `Diagnostic`, `Event`, `Rect` and
  `ExitReason` cross every boundary (M4.1); a module's own enums/records/sums cross as
  package-global nominal types named `<unit>.<Type>` (M4.2). The application's types
  stay application-local, and structural `array`/`ring`/`map` types remain shared.
- **M10 (MUST):** A module may be declared in any directory under `lib`, but its
  unit name (the last dotted component) is what qualifies its calls, so two modules
  in different directories cannot share a unit name (M4).
- **M11 (MUST):** Compile caps become: 128 KiB of source per unit, 384 KiB of
  source per application (application + `stdlib` + every module), and a 16 MiB
  scratch arena. Exceeding any cap is `CapacityExceeded` with a diagnostic naming
  the unit and the cap.
- **M12 (MUST):** `stdlib` remains `include_str!`-embedded source at unit id 1 and
  keeps `uses stdlib;`. It is not read from the image and does not move to `/lib`.
- **M13 (MUST):** Compilation stays deterministic: the same application source and
  the same module sources produce byte-identical packages, independent of directory
  iteration order.

### Applications and the file API

- **A1 (MUST):** A launch path with no `/` is searched in this order:
  `base/<name>`, `apps/accessories/<name>`, `apps/games/<name>`,
  `apps/productivity/<name>`, `apps/utilities/<name>`, `system/<name>`. The
  first regular file wins. A path with `/` is used as written, relative to the
  image root. `run terminal`, `run apps/utilities/terminal` and
  `run base/shell` name their applications unambiguously; a duplicate bare name
  across categories resolves by the fixed order above.
- **A2 (MUST):** Any valid path may be launched. There is no capability model
  (vision R14), so restricting `home` or `system` would be cosmetic; a path that is
  not an application fails with the compiler's `missing main` diagnostic.
- **A3 (MUST):** The boot entry is `system/entrypoint`, falling back to
  `base/shell`. There is no root-level `entrypoint` alias any more.
- **A4 (MUST):** `core.fs_list(dir)` accepts `""`, `"."` (the root) or any
  directory path; `core.fs_read(path, ...)` and `core.fs_size(path)` accept any
  file path. The remaining rules are unchanged: 64-byte name destination,
  256-byte read chunk, out-parameters published only on `Ok`, `NotFound` for a
  missing entry, `Invalid` for a path that fails L6 or addresses a kind-2 inode.
- **A5 (MUST):** `core.spawn`, `core.run`, `core.fs_list`, `core.fs_read` and
  `core.fs_size` share one path validator (`abi::fs`). The M2.5.1 root-only
  validators are removed.

### Build and host

- **X1 (MUST):** `xtask image` stages the shipped corpus `samples/apps` to the same
  paths in the image, dropping the `.ig` suffix from `base`/`system` sources and
  keeping `lib` names, and fills `home/` from `samples/home`, creating real
  directories. No file is staged at the image root.
- **X2 (MUST):** `xtask image --shell` stages the test corpus `samples/tests/apps`
  over the shipped one (colliding names are an error) and omits `system/entrypoint`,
  so the fallback boot (`base/shell`) is exercised and every proof app is reachable.
  The product image never carries the test corpus.
- **X3 (MUST):** `home` must exist in the image even when the seed is empty;
  `samples/home/readme.txt` is the single seed for both the app image's `home/` and
  the writable disk.
- **X4 (MUST):** The desktop source `samples/apps/base/desktop_shell.ig` stays an
  ordinary base application, staged as `base/desktop_shell` and once more as
  `system/entrypoint` for the boot path; its unit name stays `desktop_shell`.
- **X5 (MUST):** `langhost` resolves `lib.*` through the same reader interface over
  the `samples/apps` tree, and its tests use an in-memory reader or a fixture tree.

## Design

### Image tree

```
/
├── lib/            importable modules   lib/math.ig, lib/ui/menu.ig
├── base/           base applications    base/shell, base/files, ...
├── apps/           GUI applications     apps/productivity/editor, ...
│   ├── accessories/
│   ├── games/
│   ├── productivity/
│   └── utilities/  apps/utilities/terminal
├── home/           user files           home/readme.txt (read-only)
├── system/         kernel/boot apps     system/entrypoint, system/system_probe
└── lost+found/     mke2fs (kind 1, unreadable, unchanged)
```

The ext2 subset (1 or 4 KiB blocks, direct/single/double indirect, validated
metadata) is unchanged; only the "root only" restriction goes away. The reader
gains a path walk with an 8-component depth cap and a per-directory entry scan
that already exists for the root.

The source side splits into the shipped corpus
(`samples/apps/{lib,base,apps,system}`) and the test-only corpus
(`samples/tests/apps/{base,system}`); `samples/home` is
the single seed for the image's `home` directory and the writable disk. Only the
`--shell` test image overlays `samples/tests/apps`, so the product image contains
just the shipped applications.

### Resolution and validation

One validator implements L6 for all call sites. A resolver then:

1. rejects an empty path, a leading/trailing `/`, an empty `.`/`..` component,
   more than 8 components, or a component over 64 bytes;
2. walks `lib`/`base`/`home`/`system` from the root, one directory at a time;
3. returns `NotFound` for a missing component, `Invalid` for a kind-2 inode.

A launch path with no `/` is prefixed with `base/` before resolution. A `uses`
name is resolved from `lib.<dots>` to `lib/<dots>.ig` at compile time.

### Compiler interface (no I/O)

The compiler stays `no_std`, allocation-free and free of I/O. The caller supplies
sources through a trait; each unit's bytes are copied into the scratch arena once,
so the AST's source borrows stay valid for the whole compilation.

```rust
pub trait SourceReader {
    /// Size of `path` in bytes, before the compiler reserves arena space.
    fn size(&self, path: &str) -> Result<usize, Status>;
    /// Reads all of `path` into `out`; the length must equal `size`.
    fn read(&self, path: &str, out: &mut [u8]) -> Result<usize, Status>;
}

pub fn compile_with_imports(
    source: &str, path: &str,
    reader: &mut dyn SourceReader,
    arena: &mut [u8], code: &mut [u8], manifest: &mut [u8],
) -> Result<Package<'_>, Diagnostic>;
```

`compile_into` keeps its current signature and behavior: it accepts no `lib.*`
import (a diagnostic), so the existing no-import call sites and tests are
unchanged. The kernel implements `SourceReader` over the mounted `Ext2`; `langhost`
implements it over a directory; tests implement it over an in-memory table.

### Unit table and function order

`stdlib` (if present) keeps its position as id 1 and its functions come first, then
module functions in discovery order, then the application, so intra-library and
cross-unit calls resolve in one index space as today. Line-table entries keep the
existing `[code_off][unit][line][column]` shape; only the verifier bound changes.
The manifest keeps the application's unit name (runtime diagnostics are compile
time only).

### Caps

| Cap | Old | New |
|---|---|---|
| Source per unit | 128 KiB | 128 KiB |
| Source per application (app + stdlib + modules) | 256 KiB | 384 KiB |
| Scratch arena | 8 MiB | 16 MiB |
| Units per package (ids) | 2 (`0..=1`) | 17 (`0..=16`) |

The kernel keeps its 128 KiB application-source buffer and adds the 16 MiB arena in
`.bss`; module sources are copied into the arena, so both buffers are static and
preallocated (ADR 0007).

### The `/lib` file-open dialog (M9.3)

`samples/apps/lib/file_dialog.ig` is an ordinary module under M5/M6: it says
`uses core, stdlib, syslib;`, declares no `main`, no top-level `var` state and no
stream endpoint, and is compiled into every application that imports it with
`uses lib.file_dialog;`. Its one public function is:

```text
function file_dialog.open_file(initial: const string[0];
                               chosen: var string[256]) -> Status;
```

`open_file` opens a normal task-owned window (title `Open File`, content
520 × 380, centred on the reported screen with the service chrome included) and
runs the modal loop over events addressed to that window. The start directory is
the directory part of `initial` when that directory is listable (`base/child`
starts in `base` with `child` selected); otherwise it is the root. It reads up
to 96 entries with `core.fs_list` from index 0 until `NotFound`, shows kind 1
(directory), kind 0 (regular file) and kind 2 (other, gray and refused), and
draws with `stdlib.ui_*` and the `syslib` graphics primitives only. Up/Down,
PageUp/PageDown, Home/End, Enter, Backspace/Ctrl+Up and Escape work; a row click
selects and a double-click opens, the visible `Up`/`Open`/`Cancel` buttons work,
the scrollbar track pages and the thumb drags, and the close box cancels through
the owner `EventKind.Close` request. An owner `Expose` re-reads
`window_content_rect` so a dragged dialog redraws at its new origin, and only a
visible change repaints.

`Ok` publishes the whole regular-file path into `chosen` with one
`core.substr_into`; a cancel is `Invalid` and a path that does not fit is
`CapacityExceeded`, and in both cases `chosen` is untouched. A disabled screen
(`langhost`) is `Invalid` without opening a window. The module adds no opcode,
builtin, `Host` method, file API or `stdlib` function; it is the caller's
window and task that host it, and the caller's own window stays behind.

## Interfaces

- `abi::fs::validate_path` (L6) is the single path rule; the root-only variants
  are removed.
- `abi::fs::validate_dir(path)` accepts `""`/`"."` or a valid directory path.
- `compiler::SourceReader` and `compiler::compile_with_imports` (above).
- `interpreter::MAX_UNITS` becomes 16.
- `Host` gains no method: the file API signatures are unchanged.

## Failure modes

| Failure | Detected | Result |
|---|---|---|
| Module missing / directory / kind 2 | compile | diagnostic naming the `uses` line and resolved path |
| Unit header mismatch or duplicate unit name | compile | diagnostic naming the module(s) |
| Import cycle / self-import | compile | diagnostic naming the chain |
| Module declares `main`, top-level `var` or a stream | compile | diagnostic naming the module |
| Unit over 128 KiB or package over 384 KiB | compile | `CapacityExceeded` diagnostic |
| Arena over 16 MiB | compile | `CapacityExceeded` diagnostic |
| More than 15 modules | compile | diagnostic at the cap |
| Path too long / deep / empty component / `..` | call time | `Invalid` (`CapacityExceeded` over 256 bytes) |
| Missing path component | call time | `NotFound` |
| Corrupt tree | mount / lookup | `Status::Invalid`, never a fault |

## Alternatives considered

| Alternative | Why not |
|---|---|
| Runtime modules or dynamic linking | Violates spec 0001/ADR 0002 and ADR 0019 (source only, one package per application). |
| Move `stdlib` into `/lib` | Loses the self-contained compiler and forces file I/O for the common case; kept embedded (M12). |
| Flat image with name prefixes (`lib_math`) | No directories, no `/home`; the user asked for a real hierarchy. |
| Writable `/home` | Needs an ext2 writer or a RAM overlay; a separate milestone. |
| Full dotted names as qualifiers (`lib.math.clamp`) | The unit header already names the module and matches the existing `stdlib.*` pattern (decision 2026-11-17). |
| Separate `/system` image | A second mount and a second image build for no benefit at this size. |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Records, sums and user enums across the unit boundary | when a `/lib` API needs them (spec 0008 defers) |
| 2 | Module-level state and stream endpoints | when a module must own state |
| 3 | Writable `/home` (RAM overlay vs ext2 writer) | when the editor must save (Q51) |
| 4 | Module versioning | never, unless packages become shareable |
| 5 | Browser navigation into subdirectories | M3 ticket, spec 0013 update |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial draft, decided with the project owner: `uses lib.x` dotted imports, header-name qualification, read-only `/home`, embedded `stdlib`, same-image `/system`, short names resolving under `base`, caps 384 KiB / 16 MiB. |
| 2026-11-18 | Implemented: M3.1 (tree, ext2 paths, boot, short names), M3.2 (compiler `SourceReader`/units/caps), M3.3 (kernel `ImageReader`, `module_demo`) and M3.4 (browser navigation). The kernel keeps a 128 KiB application-source buffer and copies module sources into the 16 MiB arena; `abi::MAX_UNIT_ID` is 16. |
| 2026-11-19 | M9.3 adds `lib/file_dialog.ig`: a module with one public function, `file_dialog.open_file(initial, chosen) -> Status`, that opens a task-owned, screen-centred `Open File` window, lists up to 96 image entries, navigates with keys/pointer/scrollbar and writes a regular-file path only on `Ok` (`Invalid` cancel, `CapacityExceeded` when the caller's 256-byte destination cannot hold it; both leave `chosen` untouched). It follows the M5 (`stdlib` import) and M6 (no `main`/state/stream) module rules and adds no opcode, builtin, file API or `stdlib` function. |
| 2026-11-20 | L10/X1–X3 separate the shipped and test corpora: `samples/apps` holds only applications a user launches, `samples/tests/apps` holds proofs, helper children and negative fixtures, and `samples/home` is the single `home/` seed. `xtask image` builds the product image from the shipped corpus; `xtask image --shell` overlays the test corpus (collisions are errors) and remains the boot for the QMP regression suite. |
| 2026-11-20 | L3 adds the source extension: `samples/apps/{base,system}` and `samples/tests/apps/{base,system}` sources are `<name>.ig`, and the stager drops `.ig` so image names stay extensionless (`base/shell`). `lib` keeps `<name>.ig`. `langhost` maps image paths to `.ig` sources, lists bare command names, and reports a path that descends through an application file (`base/child/x`) as `Invalid`. |
| 2026-11-20 | M11 adds the top-level `apps` directory and its category tree (`apps/accessories`, `apps/games`, `apps/productivity`, `apps/utilities`). Bare launch names are searched in `base`, those four categories, then `system`; explicit paths are unchanged. The shipped GUI sources move under `samples/apps/apps/<category>/`. |

# 0038: A hierarchical read-only image and compile-time `/lib` modules

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** project owner
- **Related specs:** 0018 (normative), amends 0003, 0008, 0012, 0013

## Context

Every behaviour this decides was already constrained by earlier accepted ADRs:

- applications are interpreted source compiled at load (ADR 0002) and never
  precompiled or dynamically linked (ADR 0019);
- the image is a read-only ext2 filesystem over an in-memory block device
  (ADR 0015);
- memory is fully static and preallocated, with no runtime allocation
  (ADR 0007);
- every application may use every primitive, with no capability model
  (vision R14);
- the language already has multi-unit compilation for one embedded library,
  `stdlib`, with fixed unit ids and a shared function table (spec 0008).

The flat image and the single embedded unit now produce duplicated helper code
between sample applications, and there is no place for user data or for
kernel/boot-owned applications. The owner asked for `/lib` (importable modules),
`/base` (base applications), `/home` (user files) and `/system` (kernel side),
with dot-notation imports.

The open questions were: whether imports are compile-time or runtime, how
`uses` maps to paths, how modules are called, whether `stdlib` moves, whether
`/home` is writable, and how applications are named on the command line.

## Decision

We will:

1. **Make the image a real directory tree.** The existing 4 MiB read-only ext2
   image gains `lib`, `base`, `home` and `system`; the ext2 subset and the
   in-memory block device are unchanged. `/system` is the same image, not a
   second mount.
2. **Keep imports compile-time.** `uses lib.ui.menu;` resolves to
   `lib/ui/menu.ig`, the source is read through a new `SourceReader` and compiled
   into the application's package as an additional unit. Dots map to path
   segments; the `.ig` extension is implicit.
3. **Qualify module calls by unit name.** The imported file declares
   `unit menu;` and its declarations are called as `menu.draw(...)`, matching the
   existing `stdlib.*` pattern. Unit names are unique per package, so the last
   path component is the identity.
4. **Keep `stdlib` embedded** at unit id 1. `/lib` is a second, file-backed
   mechanism for application-level helpers; moving `stdlib` would force file I/O
   into the compiler's common path for no gain at this size.
5. **Keep `/home` read-only.** No writer exists and a writable filesystem is a
   separate milestone; the editor still cannot save.
6. **Resolve short launch names under `/base`.** `run clock` means
   `base/clock`; a path containing `/` is used as written. Any valid path may be
   launched, because there is no capability model to enforce and a restriction
   would be cosmetic.
7. **Raise the compile caps** to 384 KiB of source per application and a 16 MiB
   scratch arena (one static `.bss` buffer each), because `/lib` adds source and
   ASTs to a package that already used 6.55 MiB of an 8 MiB arena.
8. **Boot from `system/entrypoint`,** falling back to `base/shell`; the
   root-level `entrypoint` alias is removed.

Module sources are read into the caller's arena once, so the AST's source
borrows stay valid and the compiler still allocates nothing and performs no I/O
itself. Unit ids are assigned deterministically (application 0, `stdlib` 1,
modules 2.. in depth-first `uses` order), so packages stay byte-identical for
the same sources.

## Consequences

### Positive

- Helper code lives once in `/lib` and is shared by source, which matches the
  source-only model exactly.
- The compiler remains testable and portable: the kernel, `langhost` and unit
  tests supply different `SourceReader`s over one interface.
- `/home` and `/system` give the desktop a meaningful top level without a
  writable filesystem or a second mount.
- No new opcode, builtin or `Host` method: the existing file API and
  `core.spawn` keep their shapes and gain path traversal.

### Negative

- Every application that imports a module gets a private copy of its code in its
  own package (duplicated at runtime). This is inherent to one-package-per-
  application and accepted; the packages are small.
- The kernel's static `.bss` grows by about 8.4 MiB (arena 8 → 16 MiB, source
  buffer 128 → 384 KiB). Preallocated memory is the accepted cost (ADR 0007).
- Module unit names are package-global, so `lib/ui/menu.ig` and
  `lib/util/menu.ig` cannot coexist. Directory structure is organisational, not
  a namespace.
- A tree walk adds per-component directory scans to every file API call; the
  image is tiny and the depth cap is 8, so this is not measured as a problem.
- The 8-component depth cap and 64-byte components are new hard limits.

### Neutral / follow-up work

- Records, sums and user enums still cannot cross a unit boundary (spec 0008);
  a `/lib` API needing them waits for that work.
- Modules cannot own state or stream endpoints in this slice.
- The browser should learn to descend into directories (spec 0013, Q51).
- Image staging, the boot path and every screenshot/regression script that
  assumed a flat root are updated in the same slice.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Dynamic/runtime module loading | Contradicts ADR 0002 and 0019; there is no loader, no linking and no shared code model. |
| Move `stdlib` to `/lib/stdlib.ig` | Makes every compilation depend on the filesystem, grows the import surface, and breaks the self-contained compiler for no benefit. |
| Runtime path lookup for `uses` (read at first call) | The interpreter has no dynamic loader; packages are fully verified before a task starts. |
| Full dotted path as the call qualifier | The unit header is already the module's identity and matches `stdlib.*`; duplicate last components are rejected instead. |
| Writable `/home` now | Requires an ext2 writer or an overlay layer, both larger than this slice; the owner chose read-only. |
| Separate `/system` image or mount | A second image build and a second mount for a handful of files. |
| Flat image with prefixed names | No real hierarchy for `/home`, `/system`, or nested modules. |
| Keep 256 KiB / 8 MiB caps | The first `/lib` user would likely hit `CapacityExceeded`; the measurement exists (`widgets` at 6.55 MiB) and the raise is two constants plus `.bss`. |

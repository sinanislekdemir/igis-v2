# 0019: Applications are always compiled from source at load; no precompiled packages, ever

- **Status:** Accepted
- **Date:** 2026-11-13
- **Deciders:** project owner
- **Related specs:** 0002 (architecture), 0003 (language), 0007 (scheduler), 0008
  (bytecode format)
- **Extends:** ADR 0002 (applications are interpreted source), ADR 0005 (internal
  bytecode), ADR 0015 (ext2 delivery)

## Context

ADR 0002 decided that applications are source text, not native binaries, and left the
evaluation strategy open. ADR 0005 chose an internal bytecode representation and rejected
bytecode as the *delivery* artifact, but the question of a compiled cache or a precompiled
package in the image was never closed. Spec 0008 then carried an open question about
whether a future image builder might ship precompiled packages.

The project owner has now closed it permanently: applications are always compiled from
source at load, and precompiled packages are never permitted. The same statement also
removes compile latency as a design constraint — a slow compile is acceptable, and no
mechanism exists to skip it.

This matters because "compiled bytecode" is an attractive optimisation that quietly
changes the system's identity: a shipped artifact has a version, a cache has an
invalidation problem, and the image stops being inspectable text. Once an artifact can be
shipped, the toolchain grows a build step, the image builder grows a compiler, and the
kernel's compiler can drift from the one that produced the artifact. Refusing the
optimisation outright is cheaper than defending a boundary around it.

## Decision

### Source is the only delivered form

- The ext2 image contains source text and nothing executable. There is no bytecode on
  disk, no package file, no object file, no cache, and no side channel that carries
  compiled output into the image.
- Every application start — including a second start of the same application — compiles
  its source in the kernel, through the REPL service, before the task exists.
- The package (manifest plus code blob) is an **in-memory** representation with the
  lifetime of the instance. It is never written back to any device, never transferred
  between tasks, and never persisted across boots.

### Compile cost is accepted

- Compile time is not optimised for. There is no incremental compilation, no bytecode
  cache, no two-stage build, and no "compile ahead" path in the toolchain that produces
  shippable output.
- Boot time includes compiling the shell. If a large application takes noticeable time to
  compile, the correct response is to make the compiler better or the application smaller,
  not to introduce an artifact.
- Host tooling may be as slow or as clever as it likes internally; its output is source.

### What remains allowed

- The bytecode format itself still exists (ADR 0005, spec 0008). It is an implementation
  detail of the interpreter, versioned by an internal ABI number so a host toolchain and a
  kernel cannot silently disagree.
- Reusing one already-compiled in-memory blob for several instances of the same
  application would still be compilation at load, not a precompiled artifact. It is not
  planned, because compile time is not a problem to solve (spec 0004 D4).

## Consequences

### Positive

- The image is exactly what it claims to be: source, readable and diffable. An image
  review is a source review.
- There is no artifact version matrix. The compiler that produced a program is always the
  compiler in the running kernel, so bytecode evolution is never a data migration.
- No stale, mismatched or tampered compiled output can exist, because compiled output
  never outlives the process that produced it.
- The verification step in spec 0008 is a compiler-bug check, not a trust boundary for
  downloaded artifacts.
- The system keeps one execution path: boot, shell, and every spawned child all take the
  same route through the compiler.

### Negative

- Every start pays the full compile cost, including repeated starts of the same program.
  Boot latency is compile latency, and a pathological source can delay the shell.
- A machine cannot run an application whose source it does not have; there is no way to
  ship a build for a smaller system.
- Compiler bugs are always live: there is no "known-good artifact" to fall back to.

### Neutral / follow-up work

- Memory caps on source and compiler scratch stay (spec 0008 B11). They bound *memory*,
  which is statically placed, not time, which is not.
- The internals of the compiler may change freely without an artifact-compatibility
  promise; only the source language and the interpreter's ABI are contracts.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Ship precompiled packages in the image | Rejected permanently by the owner. It turns the image into an artifact store, adds a version matrix, and makes the compiler's output a compatibility surface. |
| Compile on the host, cache the package in the image | The same artifact with an extra step; also lets host and kernel compilers diverge. |
| In-memory compile cache shared by instances of one application | Still compilation at load, so it does not violate the decision; not planned, because compile time is not a constraint and per-instance code is simpler (spec 0004 D4). |
| Make compilation lazy to shorten boot | Moves the cost into the first request and complicates the loader; and the cost is accepted. |
| A "fast path" that skips type checking for trusted sources | Optimises the thing the owner explicitly does not care about, while weakening the compiler. |

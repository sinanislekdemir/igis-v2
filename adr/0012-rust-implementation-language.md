# 0012: Implement the kernel and runtime in Rust

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture)

## Context

The kernel is not a process manager: it is a language runtime with a standard library
that owns the machine (vision). It must run freestanding, without an operating system
underneath, without a garbage collector, and without a runtime that allocates behind the
programmer's back. It contains the interpreter, the memory manager, the scheduler and the
device drivers — code where a single wrong pointer is a total loss (ADR 0001), because
there is no second line of defence.

The implementation language therefore has to give three things at once: precise control
over memory layout and machine access, enough static checking to keep the `unsafe`
surface small and auditable, and a toolchain that can produce a freestanding binary for
`x86_64-unknown-none` without a heap or an operating system.

The project owner's toolchain is already installed and pinned in `AGENTS.md`
(stable 1.99, nightly 1.101 with `rust-src` for `-Z build-std`), which reflects the
intended choice rather than a new evaluation.

## Decision

Rust is the implementation language for the kernel, the interpreter, the runtime, the
scheduler, the memory manager, the drivers and `syslib`. Assembly is confined to the
places where the ISA requires it: boot entry, the long-mode transition, interrupt and
exception entry stubs, and the context switch.

- All crates are `no_std` and freestanding; there is no `std`, no allocator in the core
  path, and no hidden runtime.
- `unsafe` is allowed but must be local, documented, and justified by a stated invariant;
  the trusted computing base (`interpreter` and `syslib`) keeps its `unsafe` surface
  small and enumerated (`0002:149-151`).
- The kernel is written in Rust, not in the project's application language. The
  application language is not required to be systems-capable (vision OQ 6).
- Rust is an implementation detail of the system track. It does not appear in the
  application ABI, and applications never see Rust types or layout.

## Consequences

### Positive

- Memory-safety checking applies to the code that has no hardware protection, which is
  exactly where it matters most.
- `no_std` plus `-Z build-std` produces a self-contained freestanding image with no
  hidden runtime or allocator.
- The tooling (cargo, rustfmt, clippy, rust-analyzer, `cargo xtask` later) is uniform for
  kernel, runtime and host-side language tooling.
- Enums, pattern matching and exhaustive checks fit the interpreter's instruction dispatch
  and the fault-code model well.
- The `unsafe` boundary can be documented and reviewed as a finite list, which is the
  only defence this system has.

### Negative

- `unsafe` is still required for paging, MMIO, port I/O and the interpreter's raw memory
  access; the language does not remove the class of bugs the system is most exposed to.
- Nightly features (`build-std`, `naked` functions, allocator hooks) create toolchain
  churn and occasional breakage.
- Compile times and binary size are larger than C for the same freestanding work.
- Rust has no stable ABI; anything that must outlive a build (the loader contract, the
  bytecode format) has to be defined explicitly rather than reusing Rust types.

### Neutral / follow-up work

- A panic handler, a global allocator stub for the kernel heap, and a linker script are
  required from the first bootable commit.
- The language track is written in Rust on the host first (vision R7); that code is
  ordinary `std` Rust and is not part of the kernel image.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| C | Full control and universal tooling, but no static safety checking; in a ring-0-only system every pointer bug is fatal, and the interpreter's memory checks are the only defence. |
| C++ | Same as C plus a large language surface, exceptions and RTTI that the design explicitly removes. |
| Zig | Attractive control and comptime, but a smaller ecosystem and less mature freestanding tooling for this target; would trade one set of unknowns for another. |
| Go | Runtime, GC and goroutine scheduler are directly contrary to the no-allocation, no-GC model. |
| The project's own language | Circular: the runtime is needed to run the language, and the language is not intended to be systems-capable (vision OQ 6). |

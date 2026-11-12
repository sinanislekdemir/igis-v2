# 0002: Applications are interpreted source, not native binaries

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0001 (vision)

> **Note (2026-11-12).** One consequence below — that the kernel must carry a garbage
> collector — predates ADR 0004 and ADR 0007. The application memory model is static and
> has no collector. The decision in this ADR is unaffected; only that estimate was wrong.

## Context

If applications were native binaries, the system would need a loader, an executable
format, an ABI, and a toolchain per architecture — and it would inherit the conventional
notion of a program as an opaque artifact. The project's thesis is the opposite: the
kernel is a programmable environment with a standard library, and programs are text that
the system evaluates.

Two facts make this viable here: there is no security requirement (so running app code
inside the kernel is acceptable, see ADR 0001), and applications are expected to be small
and exploratory rather than large and compute-bound.

## Decision

Applications will be written in the project's own language and delivered as source
text. The kernel embeds a language runtime and a standard library; loading an application
means evaluating its source. No native application binaries will be loaded or executed.

The concrete evaluation strategy — direct interpretation, an intermediate bytecode, or
compilation — is deliberately left open (see Open Question 3 in the vision spec) and
will be decided in a separate ADR.

## Consequences

### Positive

- Applications are portable across every architecture the kernel supports, for free.
- Applications are inspectable data: trivially embedded, diffed, generated, tested, and
  hot-reloaded.
- No executable format, no dynamic linker, no relocation handling, no per-arch app
  toolchains.
- A REPL and an in-system development workflow are natural, not bolted on.

### Negative

- The language and its semantics become the system's real ABI. Changing them later is
  expensive and breaks every application.
- Compute-heavy workloads are slow; the interpreter's performance is a first-order
  design constraint.
- The kernel must carry a garbage collector or whatever memory model the language
  chooses, including its pauses and fragmentation behavior.
- Bootstrapping requires a working runtime before any meaningful application can run,
  which couples early milestones to the language track.

### Neutral

- Source must be stored somewhere: an initrd/module at minimum, possibly a filesystem.
- Source size limits and load times become boot-time concerns.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Native ELF applications with a syscall ABI | Reintroduces the conventional OS model the project rejects. |
| Bytecode distributed as the application artifact | Close to the thesis, but stdlib and kernel would need a compiler; kept as a possible internal representation rather than the delivery format. |
| WebAssembly as the application format | Mature and sandboxed, but sandboxing is not a goal, and it would make the system a WASM host rather than a language project. |
| eBPF-style restricted native code | Requires verifier machinery and per-app compilation; contradicts "applications are source". |

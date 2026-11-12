# 0005: Internal bytecode representation is permitted

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture), 0003 (architecture)

## Context

ADR 0002 established that applications are delivered as source text and evaluated by a
kernel-embedded runtime, but deliberately left the evaluation strategy open. The two
obvious choices are walking the parsed syntax tree directly, or compiling to an internal
intermediate representation first.

Direct tree-walking is simpler to start, but it re-interprets structure on every
execution, makes safe-point placement awkward, and offers no clean place to cache work.
An intermediate representation is portable across architectures (it contains no machine
code), gives a compact instruction loop with natural safe points, and is not a "machine
runnable binary" in the sense rejected by the vision.

## Decision

Source text remains the **only distributed application artifact**. The runtime may
compile source to an internal bytecode before execution, and that bytecode is:

- never the delivery format;
- never stable across runtime versions;
- not persisted to disk in the first milestone;
- not executable by hardware.

Native code generation (JIT or AOT) is explicitly out of scope. If this changes, it
requires a new ADR superseding this one.

## Consequences

### Positive

- One flat instruction loop with well-defined safe points simplifies budget and yield
  checks (ADR 0003).
- Better performance than tree-walking without any machine-code generation.
- Architecture-independent: the same bytecode executes on every supported ISA.
- Compilation errors surface before execution, which is valuable for a typed language.

### Negative

- The runtime carries a compiler as well as an executor: parsing, checking, lowering, and
  a bytecode format to keep coherent.
- Debugging and error reporting must map bytecode positions back to source positions.
- A second representation invites premature optimisation; discipline is needed to keep
  the bytecode simple.
- Compilation cost is paid at load time; for a REPL, that cost is per input.

### Neutral

- Bytecode caching on disk is a possible later optimisation, but it would be a derived
  artifact, not a distributed one; it needs its own decision.
- The bytecode format is internal and may change freely.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Direct tree-walking only | Simplest initial implementation, but slower, harder to place safe points, and no caching path. |
| AOT-compile bytecode on the host and ship it | Makes bytecode the artifact, contradicting ADR 0002 and the vision. |
| Ship native binaries | Contradicts the core thesis. |
| JIT to machine code | Native code generation conflicts with "applications are source" and adds an enormous correctness surface. |

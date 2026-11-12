# 0004: Static, preallocated application memory

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture)

## Context

Applications are interpreted, run at ring 0 in a single address space, and have no
hardware protection from one another (ADR 0001). Without a privilege boundary, the only
way to keep one application from corrupting another is to make ownership a property the
interpreter can check cheaply and completely. A runtime allocator works against that:
ownership changes dynamically, applications hold pointers, and free lists become shared
mutable state that a bug can corrupt.

The project owner specified a static model: the whole memory layout an application needs
is visible up front, the kernel allocates it, and there is no `malloc` and no dynamic
expansion. Communication uses kernel-owned shared-memory streams with fixed capacity.

## Decision

- An application declares its complete memory footprint in its manifest: arena size,
  stack size, and the stream endpoints it needs.
- The kernel allocates and zeroes the arena **before** the task starts, and owns it for
  the task's lifetime.
- There is no runtime allocation: no `malloc`, no realloc, no growth, no GC.
- Application-visible references are **handles and arena-relative offsets**, never raw
  addresses. The interpreter validates every access against the owning arena.
- Streams are created with declared element size and capacity and never grow.
- On task termination (normal or watchdog), the arena and all stream references are
  reclaimed deterministically as a unit.

Whether an application may run a bump/slab allocator *inside* its fixed arena is left
open (vision OQ 13); either way there is no growth.

## Consequences

### Positive

- Ownership is a static fact, so bounds checking is simple and total.
- No allocator metadata, no free lists, no fragmentation across applications.
- Deterministic accounting and deterministic reclamation; no GC pauses.
- A killed task leaves nothing behind.
- The interpreter's memory checks become a small, auditable surface.

### Negative

- Variable-size data (text, files, queues) must be bounded explicitly; the language must
  make bounded collections ergonomic or applications become painful.
- Over-declaring wastes memory; under-declaring is a load-time failure with no fallback.
- Streaming large data requires windows and backpressure rather than "just allocate more".
- The manifest becomes a load-bearing part of the application contract.

### Neutral

- The kernel still allocates memory arbitrarily for its own use (heap, pools, caches);
  the restriction applies to applications.
- Paging is still used to map arenas and guard pages.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Garbage-collected heap per application | GC roots, pauses, and cross-task collector state reintroduce exactly the complexity the thesis avoids. |
| Per-application bump allocator with growth | Growth requires dynamic mapping and ownership changes; and a growing heap is a moving target for bounds checks. |
| Reference counting | Adds atomic or non-atomic refcount traffic everywhere, plus cycle problems; still dynamic ownership. |
| WebAssembly-style linear memory that can grow | Growth semantics and host-side bookkeeping; also drags in a sandbox model that is not the goal. |
| Kernel-managed `malloc` exposed as a builtin | Makes the kernel the allocator for arbitrary app requests, reintroducing fragmentation and failure paths at runtime. |

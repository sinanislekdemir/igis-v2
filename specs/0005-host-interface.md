# 0005: Host interface

- **Status:** Draft
- **Scope:** `core`, `syslib`, interpreter, runtime, host-side language tooling
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0011, 0014, 0015
- **Updated:** 2026-11-12

## Context

The runtime never calls the kernel directly. Applications, `stdlib` and `core` all run
against an enumerated set of operations called the **host interface**. In a conventional
system this would be the syscall ABI; here there is no privilege transition, so it is an
in-process contract — but it is still the boundary that defines what a program can ask the
machine to do.

The interface has three jobs:

1. **Be the ABI.** Every privileged or non-portable operation is named here once, so the
   interpreter, `core`, `syslib` and the language agree on the same set.
2. **Be mockable.** The language track runs on the host before the kernel boots (vision
   R7). The interface must be implementable by a mock without a machine underneath.
3. **Stay small.** It is part of the trusted computing base. Every primitive is a place a
   bug can reach around the interpreter's memory checks.

## Requirements

- **H1 (MUST):** The set of primitives is closed and enumerated. There is no "call an
  address" primitive and no way to add one from an application.
- **H2 (MUST):** Every primitive completes in bounded time or transitions the task to
  `Blocked`. No primitive spins or sleeps uninterruptibly (`0002` blocking-builtin rule).
- **H3 (MUST):** No raw address crosses the interface. Hardware is named by `syslib`
  handles; application memory is named by handles and offsets.
- **H4 (MUST):** The interface is a trait (or equivalent), with a host implementation for
  tests and a kernel implementation for the target.
- **H5 (MUST):** There is no capability check. Every application may call every
  primitive (vision R14).
- **H6 (MUST):** Failures that are not fatal return `Status`; fatal conditions become
  interpreter faults, not host-interface errors.
- **H7 (MUST):** No primitive allocates on behalf of the caller. Buffers are caller-owned
  and capacity-bounded.

## Design

The interface is split into two namespaces:

| Namespace | Implementation | Contents |
|-----------|----------------|----------|
| `core` | Rust, kernel or host mock | console, yield, streams, clock, sleep, byte/string primitives |
| `syslib` | Rust, kernel only | port I/O, MMIO, DMA and block access (the last two later) |

### `core` primitives

| Primitive | Signature (illustrative) | Blocking? |
|-----------|--------------------------|-----------|
| `console_write` | `fn write(bytes: &[u8])` | no |
| `console_read` | `fn read(buf: &mut [u8]) -> usize` | yes, until at least one byte |
| `yield` | `fn yield_now()` | no; releases the quantum |
| `send` | `fn send(ep: Endpoint, bytes: &[u8]) -> Status` | yes if full |
| `try_send` | `fn try_send(ep: Endpoint, bytes: &[u8]) -> Status` | no; `WouldBlock` |
| `receive` | `fn receive(ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>` | yes if empty |
| `try_receive` | `fn try_receive(ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>` | no |
| `clock` | `fn monotonic_ns() -> u64` | no |
| `sleep_until` | `fn sleep_until(deadline_ns: u64)` | yes, until deadline |

- Time is monotonic nanoseconds since boot. It does not wrap in any realistic uptime;
  the comparison is unsigned and the deadline is absolute.
- `console_read` returns a byte count and never blocks past the next input event; line
  editing and echo belong to `stdlib`.
- `send` is atomic (`0006-streams.md`): it either enqueues the whole slice or returns an
  error. A slice larger than the endpoint capacity can never succeed and is a
  `CapacityExceeded` status.
- `receive` copies what is available, up to the destination length; a byte stream has no
  message boundaries. Framing is a `stdlib` concern.

### `syslib` primitives

| Primitive | Signature (illustrative) | Notes |
|-----------|--------------------------|-------|
| `port_in8` / `port_out8` | `fn port_in8(port: u16) -> u8` | x86 port space, through `arch` |
| `mmio_map` | `fn mmio_map(phys: u64, len: usize, out: &mut MmioRegion) -> Status` | kernel-owned mapping |
| `mmio_unmap` | `fn mmio_unmap(region: MmioRegion)` | releases the mapping |
| `mmio_read8/16/32` | `fn mmio_read32(region: MmioRegion, off: usize) -> u32` | bounds-checked |
| `mmio_write8/16/32` | `fn mmio_write32(region: MmioRegion, off: usize, v: u32)` | bounds-checked |
| `dma_alloc` / `dma_free` | later | kernel-owned buffer, typed handle |

- MMIO regions are mapped into the shared address space but are not part of the
  application's nameable regions. The handle is the only way to reach them.
- Bounds and alignment checks are part of the interface, not the caller's duty.
- DMA and block access are specified when a device needs them; they follow the same
  handle model (`0004-memory-model.md`).

### Host implementation

The language track links the runtime against a host implementation that provides console
I/O through stdio, streams as in-process queues, a monotonic clock and a trivial `syslib`
that refuses hardware calls. The same runtime code runs on the host and in the kernel;
only the implementation behind the trait changes. This is what makes R7 real rather than
aspirational.

### What is deliberately absent

- No `exit`/`fault` primitive: those are interpreter control flow, not host calls.
- No allocation primitive: the loader allocates before the task starts; the interface has
  nothing to give at runtime.
- No process, thread or spawn primitive: an application is one task.
- No debug or introspection primitive in the first milestone (open question 3).

## Interfaces

```rust
// Illustrative. The concrete shape is fixed when the workspace is created.
trait Host {
    fn console_write(&self, bytes: &[u8]);
    fn console_read(&mut self, buf: &mut [u8]) -> usize;
    fn yield_now(&mut self);
    fn send(&mut self, ep: Endpoint, bytes: &[u8]) -> Status;
    fn try_send(&mut self, ep: Endpoint, bytes: &[u8]) -> Status;
    fn receive(&mut self, ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>;
    fn try_receive(&mut self, ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>;
    fn monotonic_ns(&self) -> u64;
    fn sleep_until(&mut self, deadline_ns: u64);
}

trait Syslib {
    fn port_in8(&self, port: u16) -> u8;
    fn port_out8(&self, port: u16, value: u8);
    fn mmio_map(&mut self, phys: u64, len: usize) -> Result<MmioRegion, Status>;
    fn mmio_read32(&self, region: MmioRegion, offset: usize) -> u32;
    fn mmio_write32(&mut self, region: MmioRegion, offset: usize, value: u32);
}
```

The language sees these as `core.*` and `syslib.*`. The namespace split is the
developer-visible signal for origin and cost (`0003-language.md`).

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| `console_read` with no input | Task blocks; no CPU consumed |
| `send` with a full queue | Blocks by default; `try_send` returns `WouldBlock` |
| `send` larger than capacity | `CapacityExceeded`; cannot ever succeed |
| `receive` from an empty queue | Blocks; `try_receive` returns `WouldBlock` |
| Endpoint peer dead | `Closed`; blocked peer wakes with `Closed` |
| `sleep_until` in the past | Returns immediately |
| `mmio_map` of an invalid or conflicting range | `Status` error; no partial mapping |
| `mmio_read/write` out of range | Fault (`OutOfBounds`); the handle is bounds-checked |
| Host mock asked for hardware | `Status` error, never a panic |
| Primitive called after task termination | Cannot happen; the task is not runnable |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | `console_read` granularity: bytes, lines, or key events | Console driver and REPL; a byte stream is the current assumption. |
| 2 | Clock source and unit (TSC, PIT, APIC timer) and the `sleep_until` granularity | `arch/x86_64` timer work. |
| 3 | Whether a minimal debug/introspection primitive is worth adding to the first milestone | After the REPL exists; not assumed. |
| 4 | Whether `syslib` needs its own trait or shares the host trait | Implementation layout; no semantic impact. |
| 5 | DMA and block primitives | When the first real device is added. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-12 | Initial draft: core and syslib namespaces, blocking semantics, mockability, absence of allocation and spawn. |

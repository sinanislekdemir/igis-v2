# 0006: Streams and inter-application IPC

- **Status:** Draft
- **Scope:** `mm` (buffers), `sched` (blocking), `core`/`stdlib` (endpoints), loader
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0004, 0011, 0015
- **Updated:** 2026-11-12

## Context

Applications share one address space but cannot name each other's memory. They still need
to communicate. The primitive is a **kernel-owned byte FIFO with a fixed capacity**: two
endpoints, one queue, no growth, no shared mapping.

The model is deliberately narrow. There is no message bus, no publish/subscribe, no
shared ring buffer and no zero-copy path in the first milestone. A stream is a pipe with
a capacity, and everything richer — typed messages, request/response, framing — is built
in `stdlib` on top of bytes.

The reason for bytes rather than elements is that the kernel should not know application
types. A byte FIFO is one implementation for every payload; the type system and the
`stdlib` typed views recover safety at the language layer, where it belongs.

## Requirements

- **S1 (MUST):** A stream is a FIFO of bytes with a fixed capacity in bytes, fixed when
  the stream is created. It never grows.
- **S2 (MUST):** The buffer is kernel-owned and is never mapped into any application's
  nameable regions. The application sees an endpoint handle only.
- **S3 (MUST):** A stream has exactly two endpoints, one sender and one receiver, each
  owned by at most one task at a time.
- **S4 (MUST):** `send` is atomic: it enqueues all N bytes or none. A send with N greater
  than the capacity can never succeed and returns `CapacityExceeded`.
- **S5 (MUST):** `send`/`receive` block by default when the queue is full/empty;
  `try_send`/`try_receive` return `WouldBlock` immediately.
- **S6 (MUST):** Data is copied through the kernel. No application-visible pointer into
  the queue exists, and no atomics are required in application code.
- **S7 (MUST):** When a task terminates, its endpoints close. The peer observes `Closed`;
  queued data is discarded immediately, not drained.
- **S8 (MUST):** Endpoints are declared in the manifest by name and direction. The loader
  creates and pairs them before any task starts; an unmatched or conflicting declaration
  is a load error.
- **S9 (MUST):** A task blocked on a stream is woken when data or space becomes available,
  or when the peer closes.
- **S10 (MUST):** The kernel has no notion of message boundaries, types or framing. Typed
  views are `stdlib` code.

## Design

### Endpoint model

```
out_stream[N]  ──┐                 ┌── in_stream[N]
                 ├── [ byte FIFO ] ─┤
   sender        │   capacity N     │   receiver
                 └─────────────────┘
```

- `out_stream[N]` and `in_stream[N]` are distinct language types; sending on a receive
  endpoint is a compile error.
- The endpoint handle carries direction, the queue reference and an open/closed state.
- The queue holds a head, a tail, a count and N bytes of storage. No metadata is visible
  to the application.

### Send

```
send(ep, bytes[0..n]):
    if ep is closed            -> Status.Closed
    if n > capacity            -> Status.CapacityExceeded
    if free < n                -> block (default) or Status.WouldBlock (try)
    copy bytes into the queue at tail (wrapping)
    count += n; tail = (tail + n) mod capacity
    wake a blocked receiver, if any
    -> Status.Ok
```

Atomicity is what makes peer death simple: there is never a half-written record in the
queue, so discarding the queue cannot tear a message.

### Receive

```
receive(ep, buf[0..m]):
    if ep is closed and count == 0 -> Status.Closed
    if count == 0                  -> block (default) or Status.WouldBlock (try)
    k = min(count, m)
    copy k bytes from head (wrapping)
    count -= k; head = (head + k) mod capacity
    wake a blocked sender, if any
    -> k
```

A byte stream has no message boundaries, so a receive returns what is available, up to
the destination length. Typed views that need exactly N bytes loop:

```
recv_exact(ep, var dst: bytes[N], n: int):
    var got := 0
    var chunk: bytes[N]
    while got < n do
        var k := receive(ep, var chunk)
        if k == 0 then return Status.Closed
        var i := 0
        while (i < k) and (got < n) do
            dst[got] := chunk[i]
            got := got + 1
            i := i + 1
```

There is no sub-range expression: `dst` and `chunk` are whole buffers, indexed with
runtime bounds checks. A `stdlib` helper that wants to fill a buffer at an offset takes
the buffer and the offset as separate parameters, not a view.

`stdlib` provides `recv_u64`, `recv_string_into`, and length-prefixed framing helpers on
top of this loop.

### Close and peer death

- An endpoint closes when its owning task terminates, normally or by fault.
- Closing discards queued bytes immediately. The peer's next `send`/`receive` returns
  `Closed`; a peer already blocked wakes with `Closed`.
- Because sends are atomic, `Closed` never means "a partial record was delivered".
- There is **no explicit close** primitive in the first milestone. A task can stop using
  an endpoint, but the queue stays open until the task ends. This is a deliberate
  simplification; explicit close is an open question.

### Pairing

- A unit declares top-level endpoints; the variable name is the stream name recorded in
  the manifest:

```
var
  events_out: out_stream[4096];   // stream "events_out", send side
  config_in:  in_stream[256];      // stream "config_in", receive side
```

- The loader reads every manifest, then pairs one `out` with one `in` of the same name.
  The two capacities must match; a mismatch is a load error.
- An unmatched declaration, a name with two senders, or a name with two receivers is a
  load error. There is no "maybe connected" state.
- Pairing happens before the first task runs. There is no runtime connect/disconnect.

### Capacity and backpressure

- Capacity is in bytes and is chosen by the application author (declared in the
  manifest, checked by the compiler).
- A full queue blocks the sender; it never expands, drops or overwrites.
- A slow receiver therefore throttles a fast sender through blocking, which is the
  intended backpressure mechanism.
- A send larger than the capacity is a permanent error, not a block: no amount of waiting
  can make it fit. This is the one place where the atomic model constrains the API.

### Wake-up

Blocking transitions the task to `Blocked(stream)` and removes it from the runnable set.
When data or space becomes available, or the peer closes, the task is re-enqueued; if all
cores are halted, an IPI wakes one (`0007-scheduler.md`).

### What is not a stream

- The console is a host-interface primitive (`0005-host-interface.md`), not a stream. An
  application's stdin/stdout equivalent is a pair of streams the loader can wire to a
  console application, if one exists.
- MMIO and DMA are `syslib` handles, not streams.

## Interfaces

```rust
// Illustrative kernel-side shape.
enum Status { Ok, WouldBlock, Closed, NotFound, Invalid, Full, CapacityExceeded, IoError }

struct Stream { head: usize, tail: usize, count: usize, cap: usize, closed: bool }

fn send(ep: Endpoint, bytes: &[u8]) -> Status;
fn try_send(ep: Endpoint, bytes: &[u8]) -> Status;
fn receive(ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>;
fn try_receive(ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>;
```

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Send larger than capacity | `CapacityExceeded`; never blocks, never truncates |
| Send on a full queue | Blocks; `try_send` returns `WouldBlock` |
| Receive on an empty queue | Blocks; `try_receive` returns `WouldBlock` |
| Peer task terminates | Queue discarded; surviving endpoint returns `Closed` |
| Both endpoints closed | Release the buffer at the last close |
| Blocked peer at close | Wakes with `Closed` |
| Unmatched endpoint in a manifest | Load error, before any task starts |
| Two senders for one name | Load error |
| Capacity mismatch between peers | Load error |
| Send on a receive endpoint | Compile error (distinct types) |
| Task blocked forever on a live peer | Remains `Blocked`; no CPU consumed |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Explicit close: should an application be able to close one endpoint while continuing to run? | Needed for long-lived services; not in the first milestone. |
| 2 | Multiple senders or multiple receivers on one stream | Would turn the FIFO into an MPMC queue; not needed yet. |
| 3 | Stream name collisions across units in one application | Pairing is per manifest name; a unit prefix may be required. |
| 4 | Whether the loader should auto-wire a console stream for REPL-style applications | After the console application exists. |
| 5 | Framing helpers: length prefix, delimiter, or both | `stdlib` design; does not affect the kernel. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-12 | Initial draft: byte FIFO, kernel-owned and copied, atomic sends, blocking plus try variants, immediate discard on peer death, manifest-based pairing, typed views in `stdlib`. |

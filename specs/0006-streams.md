# 0006: Streams and inter-application IPC

- **Status:** Accepted
- **Scope:** `mm` (buffers), `sched` (blocking), `core`/`stdlib` (endpoints), loader
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0004, 0011, 0015
- **Updated:** 2026-11-15

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
  creates an endpoint when its task loads, pairing it immediately with a live peer of the
  opposite direction or leaving it waiting for a peer that appears later. A second
  endpoint with the same name and direction, a capacity mismatch between peers, or a
  declaration against a closed stream is a load error (see "Dynamic loading").
- **S9 (MUST):** A task blocked on a stream is woken when data or space becomes available,
  or when the peer closes.
- **S10 (MUST):** The kernel has no notion of message boundaries, types or framing. Typed
  views are `stdlib` code.
- **S11 (MUST):** A stream name is unique across the whole application. Two units may not
  declare the same name, even with different directions; the compiler rejects it.
- **S12 (MUST):** A stream name is at most 31 bytes matching `[a-z][a-z0-9_]*`; a capacity
  is between 1 and 65536 bytes. Both are compile-time facts recorded in the manifest.

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
- A stream has exactly one sender and one receiver. A many-to-many stream would need a
  different structure (an MPMC queue with several waiters), not a change to this FIFO; if
  it is ever needed it arrives as a distinct shared endpoint type (open question 2).

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
    var one: bytes[1]
    var staged: bytes[N]
    while got < n do
        k := receive(ep, var one)          // exactly one byte, see below
        if k == 0 then return Status.Closed
        staged[got] := one[0]
        got := got + 1
    dst := staged
```

**A receive must never ask for more bytes than the view still needs.** `receive` copies
`min(count, destination capacity)` bytes, so a chunk wider than `remaining` takes bytes
that belong to the peer's next message, and there is no pushback: the surplus is lost and
every later read on that stream is misparsed. A `bytes[256]` chunk is therefore wrong for
any view smaller than 256 bytes. The receive bound has to be the *remaining* count, which
a buffer parameter cannot express today (the language has no runtime-capacity buffers, see
`OPEN-QUESTIONS.md` Q46), so a view reads **one byte per receive**: exact by construction
at the cost of one stream operation per byte. A `stdlib` helper that wants to fill a
buffer at an offset also takes the buffer and the offset as separate parameters, not a
view.

`stdlib` provides `recv_exact`, `recv_frame_into`, `recv_line_into`, `recv_u64`,
`send_frame`, `send_line`, `send_u64` and `send_bytes` on top of this loop. The default
frame is length-prefixed: a little-endian `u32` length followed by that many bytes, with
the length required to fit both the destination and the endpoint capacity; a length that
cannot be satisfied is an error, never a partial read. Text protocols use the delimiter
helpers (`send_line`, `recv_line_into`, `\n`-terminated) instead. The kernel has no
opinion: framing is a convention between the two peers.

### Close and peer death

- An endpoint closes when its owning task terminates, normally or by fault.
- Closing discards queued bytes immediately. The peer's next `send`/`receive` returns
  `Closed`; a peer already blocked wakes with `Closed`.
- Because sends are atomic, `Closed` never means "a partial record was delivered".
- There is **no explicit close** primitive in the first milestone. A task can stop using
  an endpoint, but the queue stays open until the task ends.
- Explicit close was deferred, not rejected. The constraint it leaves behind: an
  endpoint's closed state is modelled independently of its owner's lifetime, so adding
  `close` later does not rework the queue. When it is added, `close` means "drain what is
  queued, then report `Closed`"; task death keeps its current meaning, "discard
  immediately" (open question 1).

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
- A name is at most 31 bytes of `[a-z][a-z0-9_]*`; anything else is a compile error.
  Capacities are `1..=65536` bytes, and the loader refuses an application whose declared
  streams the kernel's stream-buffer budget cannot cover.
- A stream name is an application-wide identifier. Two units may not declare the same
  name, even with different directions: the compiler sees every unit of the application
  and rejects the duplicate, so the loader never has to guess which declaration was meant.
- A name with two senders, or a name with two receivers, is a load error. Pairing happens
  when a task loads; there is no runtime connect/disconnect.
- A capacity is fixed when the first endpoint of a name is created. A peer that arrives
  later must declare the same capacity; otherwise its load is refused and the existing
  stream is untouched.

#### Dynamic loading

Tasks are created at run time, so two peers can never be loaded in the same operation. The
loader therefore creates each declared endpoint when its task loads, before the task is
placed:

- If no stream with that name exists, the endpoint is created and the stream is **waiting**
  for its peer. Its buffer exists from this moment.
- If a live peer of the opposite direction exists, the endpoint pairs with it immediately.
  The capacities must match.
- If an endpoint with the same name and the same direction is already live, the arriving
  task's load is an error ("a sender named `x` already exists" / "a receiver named
  `x` already exists"). The existing stream is untouched.
- A capacity mismatch is a load error for the arriving task, naming both capacities; the
  existing stream is untouched.
- Sending into a waiting stream is allowed up to its capacity: the buffer exists. Receiving
  from an empty waiting stream blocks exactly as it does from an empty paired stream.
  Blocking on a peer that never arrives is the documented "blocked forever on a live peer"
  case; the task consumes no CPU.
- A stream whose owner terminates becomes closed and never pairs again. Its surviving peer
  observes `Closed`; a new declaration of that name is a load error until the surviving
  endpoint closes too.
- If placement or spawning fails after endpoints were created, the loader closes them and
  frees their buffers, exactly as task termination would.

The compiler still enforces application-wide unique names, the name grammar and the
capacity range (S11/S12), so the loader never sees two declarations in one package.

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

#### Multi-source read readiness (Q39)

A stream endpoint handle may be one entry in a `core.next_any` wait set. That entry is a
**read** select: `stream::readable(task, handle)` is true when data is queued, the record
is closed, or the handle is not a live receive endpoint for the caller (so the following
`core.recv` reports an error rather than blocking). Send-space readiness is deliberately
not part of the primitive; a task that needs to send waits on `core.send` alone.

- A send, a receive that frees space, and a close all call `wake_stream(handle)`, which
  now wakes a `WaitFor::Any` set containing that handle in addition to the single
  `WaitFor::Stream(handle)` waiter. The lock rule is unchanged: the stream lock is
  dropped before any task slot lock is taken.
- Closing a stream wakes a waiting select, which then reports the handle ready and lets
  `core.recv` observe `Closed`.
- Readiness is a snapshot: another task may consume the queued bytes between the ready
  report and the caller's receive. The receive's own `WouldBlock`/`Closed`/count is
  authoritative.

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
| A stream closes (one owner terminates) | Closing is terminal for both logical sides: discard queued bytes, release the buffer, wake the peer with `Closed`. The table record stays until the surviving endpoint's owner ends |
| Blocked peer at close | Wakes with `Closed` |
| Stream in a multi-source wait | A send, receive or close wakes the set; data or close is "read ready", space is not |
| Endpoint with no live peer yet | Created and waiting; sends fill up to capacity, receives block on empty |
| Two senders (or two receivers) for one name | Load error for the arriving task; the existing stream is untouched |
| Capacity mismatch between peers | Load error for the arriving task, naming both capacities; the existing stream is untouched |
| Declaration against a closed stream | Load error until the surviving endpoint closes |
| Placement or spawn fails after endpoints exist | Endpoints closed and buffers freed by the loader |
| Send on a receive endpoint | Compile error (distinct types) |
| Task blocked forever on a live peer | Remains `Blocked`; no CPU consumed |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Explicit close: an application closes one endpoint and keeps running | Deferred to streams v2. The closed state is already independent of owner lifetime; `close` will drain before reporting `Closed`. |
| 2 | ~~Multiple senders or multiple receivers on one stream~~ | **Resolved** — one sender, one receiver; a many-to-many need arrives as a distinct shared endpoint type, never as a change to the FIFO. |
| 3 | ~~Stream name collisions across units in one application~~ | **Resolved** — names are unique application-wide; a duplicate is a compile error (S11). |
| 4 | Whether the loader should auto-wire a console stream for REPL-style applications | After a console application exists; M1 applications use `core.console_*`. |
| 5 | ~~Framing helpers: length prefix, delimiter, or both~~ | **Resolved** — both in `stdlib`; length-prefixed (`u32` LE) by default, newline-delimited helpers for text. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-12 | Initial draft: byte FIFO, kernel-owned and copied, atomic sends, blocking plus try variants, immediate discard on peer death, manifest-based pairing, typed views in `stdlib`. |
| 2026-11-13 | Stream names are unique application-wide (S11); one sender and one receiver are fixed, a shared type would be a separate addition; `stdlib` framing uses length prefixes by default and newline delimiters for text; explicit close is deferred to v2 with the constraint that closed state is independent of owner lifetime. |
| 2026-11-13 | Stream names are `string[32]` matching `[a-z][a-z0-9_]*` and capacities are `1..=65536` bytes (S12). |
| 2026-11-15 | Implemented. Dynamic loading replaces the static "unmatched is a load error" rule (S8): an endpoint is created when its task loads and waits for a later peer, so a sender may fill a waiting stream and a receiver blocks on it. A same-direction duplicate, a capacity mismatch, or a declaration against a closed stream is the load error. Handles are `(generation, index)`; termination closes the owner's sides, discards queued bytes and frees the buffer when no side remains. Explicit close is still v2. |
| 2026-11-15 | The `stdlib` framing layer is implemented as ordinary language code over `const in_stream[1]`/`const out_stream[1]` parameters (the declared capacity is a lower bound the compiler does not check, so one helper serves every capacity): `recv_exact`, `send_frame`, `recv_frame_into`, `send_line`, `recv_line_into`, `send_u64`, `recv_u64` and `send_bytes`. Each is atomic at its boundary — a destination is written only by the one copy that publishes a whole value — so a failure never exposes a partial value. The "Receive" pseudocode is corrected: a receive returns up to the destination capacity, so a view must never ask for more than the bytes it still needs; without runtime-capacity buffer parameters a view reads one byte per receive. `samples/apps/streams` now runs through `send_line`/`recv_line_into`, and `samples/apps/frames` (`frame_in` + `frame_out`) proves the length prefix, the typed `u64` view, the `CapacityExceeded` paths and the close. |
| 2026-11-17 | Multi-source read readiness (Q39, ADR 0036): a stream handle can be a source of `core.poll_any`/`core.next_any`. Readiness is data-or-close on the caller's receive side (a stale or foreign handle is ready-as-error); send-space readiness is not part of it. `wake_stream` also wakes a `WaitFor::Any` set containing the handle, so a send, a receive that frees space and a close reach a waiting select; the stream lock is still dropped before any slot lock. Readiness is a snapshot and the following receive is authoritative. |

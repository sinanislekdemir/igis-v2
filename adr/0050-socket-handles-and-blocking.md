# 0050: Sockets are owned opaque handles with blocking waits and deadlines

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** igis-v2 runtime/scheduler
- **Related specs:** 0020 (networking), 0003 (language), 0005 (host interface), 0006
  (streams), 0007 (scheduler)
- **Related ADRs:** 0011 (claim/release, blocking), 0034 (id-tagged records), 0036
  (multi-source waits), 0037 (console byte stream), 0047 (window-addressed input),
  0048 (kernel wire stack)

## Context

A network socket has four properties the existing interfaces must accommodate:

1. **Identity.** An application must name a socket without naming a kernel address.
   A stale name must not reach a recycled connection.
2. **Ownership.** A task opens a socket; another task must not read or close it.
   When the owner dies, the socket must close and wake its peer, on every
   destruction path.
3. **Waiting.** `connect`, `send` and `recv` can block for an unbounded time. The
   scheduler already blocks tasks (`WaitFor::Child`, `Stream`, `Input`, `Any`) and
   the interpreter retries the same instruction on wake. Networking adds a new
   reason and a **timeout** reason; neither exists today.
4. **Readiness.** `core.next_any`/`poll_any` select over console bytes, input events
   and stream handles. A socket must be nameable in the same set without colliding
   with a stream handle.

The existing models answer most of this. Streams use `(generation << 8) | index`
handles, `0` invalid, with owner/liveness in a static table; windows use monotonic
ids with owner checks; `WaitSet` is a plain `[u64; 8]`; `Status` already has
`WouldBlock`, `Closed`, `Full`, `Invalid`, `NotFound`, `CapacityExceeded` and
`IoError`. What is missing is the socket table, the two new wait conditions and a
source tag.

## Decision

We will model a socket as an **owner-checked opaque handle** in a fixed kernel table,
expose it through `core.socket_*`/`core.net_*` host builtins (no opcode), block with
`WaitFor::Net(handle)` and `WaitFor::Deadline(ns)`, and tag socket sources in a
`WaitSet` with `1 << 63 | handle`.

### Identity and ownership

- A handle is a `uint`/`int` with `(generation << 8) | index`; generation starts at
  one and increments when a slot is reused, so `0` is never valid and a stale handle
  is `NotFound`.
- The slot stores the owner task id. Every operation checks it: a foreign live
  socket is `Invalid` (the same rule as `window_close`/`window_move`).
- `Task::reclaim` closes every owned socket before the task's regions are freed. A
  blocked peer wakes with `Closed`; queued bytes are discarded. A listener's pending
  connections are closed too.
- A socket does not outlive its task and is not transferable. There is no `dup`,
  `fork` or inheritance.

### The ABI

`socket_open(kind, ip, port, local, var id)` uses kind `1` TCP client, `2` UDP and
`3` listener; `local` is the local port to bind (`0` = ephemeral) and a listener
binds `local`. `socket_accept` returns a connected child (M10.7). Addresses are
`uint` in host order (first octet in bits 24..31); ports are `uint` in
`0..=65535`. No new predeclared record is introduced: scalar address/port
operands and one handle out-parameter are easier to keep untouched on failure than
a layout commitment.

The blocking variants (`socket_send`, `socket_recv`, `socket_open` for TCP) may
transition the task to `Blocked`; the try variants never block and return
`WouldBlock`. `socket_close` is idempotent for the owner (`Ok` on an already-closed
handle) and `Invalid` for a foreign one. `socket_timeout(id, ms)` sets the deadline
for subsequent blocking calls on that socket; `0` means no timeout. Defaults:
connect 5 s, send/recv 30 s once M10.4 adds the deadline arm.

### Blocking and deadlines

- `WaitFor::Net(handle)` is recorded by the interpreter when a blocking operation
  returns `WouldBlock`; `task::wake_net(handle)` wakes every task waiting on that
  handle (data, EOF, error or close). The scheduler's post-block re-check calls the
  same readiness probe, so a lost wake is not possible.
- `WaitFor::Deadline(ns)` is a new generic arm. The 1 kHz timer tick records expiry
  (or the service opportunity compares `clock::now_ns()`); it performs no socket
  work itself. On wake the interpreter retries the builtin, which observes the
  expired deadline and returns `Closed` (connection never opened) or `WouldBlock`
  (retryable send/recv). `ms = 0` never arms a deadline.
- A blocking builtin remains a bounded operation from the scheduler's point of view:
  it runs, returns `WouldBlock`, and the task is blocked. No spin is added.

### Readiness and `next_any`

`core.socket_source(id)` is compiler-lowered (no builtin), like
`core.stream_source`, and returns `abi::wait::SOURCE_SOCKET_BASE | handle` with
`SOURCE_SOCKET_BASE = 1 << 63`. Stream handles are small (`generation << 8 | index`),
so the tag is unambiguous. A socket is readable when it has data, EOF, an error, a
completed connect or (listener) a pending connection; `Host::socket_readable` defaults
to `false`, so a bare host and a disabled NIC are deterministic. Readiness is a
snapshot; the following `socket_recv`/`accept` is authoritative. Send-space readiness
is not a source in M10; a proof that needs it adds a second tag later.

### Status mapping

`Ok` success; `WouldBlock` no progress; `Closed` FIN/RST/refused/timeout;
`Invalid` foreign or malformed; `NotFound` stale/unknown; `Full` no slot;
`IoError` no device or link down; `CapacityExceeded` a datagram or send larger than
the slot arena. Every out-parameter is left untouched on failure, matching the file
and window rules.

### Language surface

The `/lib/socket` unit fixes the ergonomic capacities (`recv` 1024, `recv_line` 256)
because the language has no runtime-capacity buffer parameters (Q46). It stages text
sends into a local `bytes[1024]`; larger buffers call `core.socket_recv`/`send`
directly. The unit keeps no state, has no `main` and no top-level `var`.

## Consequences

### Positive

- Sockets follow the same handle/owner/close-on-death rules as streams and windows,
  so there is one mental model and one class of stale-handle bugs eliminated by
  generation checks.
- `next_any` works without a new array element type or a second selector: the tag is
  a plain `u64`.
- The deadline arm is generic and useful beyond networking (a future `core.sleep`
  or a bounded file operation can use it).
- No opcode, no new predeclared type, and no change to the `Event`/`Rect` layouts;
  ABI v1 stays at 75.

### Negative

- Handles are opaque integers, so an application cannot print a meaningful fd or
  pass one to a conventional API; there is no POSIX compatibility layer.
- The socket table is fixed (8 slots); a program that opens more gets `Full`. The
  limit is a build-time constant, not a tunable.
- `WaitFor::Deadline` adds a scheduler wake path and a timer interaction; a bug in it
  can leave a task blocked past its deadline or wake it spuriously. The post-block
  re-check and the 1 kHz tick bound the damage.
- Every blocking builtin needs a retry-safe state machine (the interpreter already
  has one), and the socket code must not consume data before the caller's buffer is
  validated; otherwise a retry loses a byte.
- The language's fixed capacities leak into the unit's API; a larger receive needs a
  raw builtin call, and the unit cannot hide that.

### Neutral / follow-up work

- M10.2 adds builtins 62–64, M10.3 adds 65–70 and the `socket` unit, M10.4 adds 71
  (`socket_timeout`) and the `Deadline` arm, M10.6 may add 73 (`net_stats`), and
  M10.7 adds 72 (`accept`).
- `abi::wait` gains `SOURCE_SOCKET_BASE`; `WaitSet`'s representation is unchanged.
- `Host` gains `socket_open/send/recv/close/readable` defaults that return
  `Invalid`/`false`, so `langhost` stays deterministic; a loopback host is added for
  the host tests.
- The builtins are documented in spec 0005 when they land, not before.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Small per-task file-descriptor integers | Requires a per-task table, an allocation or a fixed owner map, and makes a stale fd after close indistinguishable from a new one unless generations are added anyway. Windows and streams already use generation handles. |
| Global integer ids without ownership | Any task could read or close another task's socket; the window service already rejects that for windows. |
| A heap-allocated `Socket` object | Violates ADR 0004/0007; there is no allocator. |
| Callback/async API | The interpreter has no continuations; a callback would need a task and a stack, and the scheduler's blocking model is already the contract. |
| A per-socket kernel task | One task per connection is expensive, makes the socket table a scheduler concern, and complicates termination; the protocol timers already run in the service opportunity. |
| Reuse stream handles for sockets | A stream is a paired FIFO with a peer endpoint and a capacity; a socket has a remote address, connection state and a different close/EOF rule. Sharing the encoding would invite a cross-type bug. |
| Block with a spin/poll loop | Violates the bounded-builtin rule (spec 0005 H2) and spends the task's CPU budget. |

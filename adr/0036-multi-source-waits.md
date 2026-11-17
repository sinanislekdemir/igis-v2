# 0036: Multi-source waits are one `WaitSet` condition with readiness probes

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 language/runtime
- **Related specs:** 0003 ("Multi-source waits"), 0005 ("Multi-source waits"), 0006 ("Multi-source read readiness"), 0007 ("Blocking and wake-up")

## Context

Q39 asked how one task can serve more than one input source. Until now a task
blocked on exactly one condition: `core.wait(child)`, `core.read_line`
(`WaitFor::Console`), a stream send/receive (`WaitFor::Stream(handle)`) or
`syslib.next_event` (`WaitFor::Input`). An application that needed both a
console command and a stream message had to pick one, poll the other in a spin,
or split the work across tasks with no arbitration.

The language has no threads, callbacks, closures or shared memory, and the
kernel has no allocator. A solution therefore has to be a fixed-size value that
both the interpreter and the scheduler can reason about, and it must not turn a
readiness check into a reservation: two tasks may wait on the same console
line, and whoever reads first consumes it.

Two shapes were possible:

1. An event-loop/selector object with registration and callbacks. Rejected: the
   language has no callback, the object would need heap storage or a fixed
   handle table, and registration duplicates the wait condition.
2. A bounded source set plus a non-blocking probe and a blocking form. The
   probe is a pure query; the blocking form reuses the existing block/wake
   machinery with a new `WaitFor` payload.

The second shape also composes with the existing single-source primitives: the
application reports the ready index, then calls the ordinary
`read_line`/`next_event`/`recv`, which keeps ownership, `Closed` and
`WouldBlock` semantics in exactly one place.

## Decision

We will add two host builtins and one compiler-lowered primitive, all with no
bytecode opcode (ABI v1 stays at 75):

- `core.poll_any(sources: const array[uint, 8]; count: int; var ready: int)
  -> Status` (builtin 51). Non-blocking. It probes the first `count` sources in
  caller order and writes the zero-based index of the first ready one, returning
  `Ok`; with nothing ready it returns `WouldBlock` and leaves `ready` untouched.
  A `count` outside `1..=8` is `Invalid` and also leaves `ready` untouched.
- `core.next_any(...)` (builtin 52). The same probe; when nothing is ready it
  blocks on the whole set and retries the instruction after a wake-up.
- `core.stream_source(ep) -> uint`. Compiler-lowered to a value reference plus
  `load_ref`, so there is no builtin, opcode or `Host` method. It is the only
  way to name a stream endpoint in the source array; every other use of an
  endpoint as a value stays a compile error.

The source encoding is fixed and numeric:

| Value | Meaning |
|-------|---------|
| `0` | one completed console line (`core.read_line` would not block) |
| `1` | one machine-wide input event (`syslib.next_event` would not block, including a pending window `Expose`) |
| `>= 2` | a stream endpoint handle whose **data or close** is ready (a read select) |

`0` and `1` cannot collide with a stream handle: the kernel encodes a handle as
`(generation << 8) | index` with a non-zero generation. Send-space readiness is
deliberately not a source; a sender waits on `core.send` alone.

The shared value is `abi::wait::WaitSet` (`Copy + Eq`, `count: u8` plus
`[u64; 8]`). The interpreter's `WaitKind::Any(WaitSet)` and the kernel's
`WaitFor::Any(WaitSet)` carry the same bytes, so neither side depends on the
other.

Readiness is a **snapshot**, not a reservation. The ready report does not
consume the source and does not lock it. Another task may take the console line,
the event or the stream bytes before the caller reads, so the following read's
status is authoritative (`WouldBlock`, `Closed`, a short count, or a fault).
This is the only semantics that keeps the console unowned (spec 0005) and keeps
two selectors from stealing each other's data.

The wake path is unchanged in shape:

- `console::has_line`, `event::has_event`/`window::has_expose` and
  `stream::readable` are the probes. The kernel host maps the three `Host`
  methods to them; the scheduler's post-block recheck calls
  `task::wait_any_ready`, which uses the same functions.
- `wake_console`/`wake_input`/`wake_stream(handle)` wake a matching
  `WaitFor::Any` set in addition to their single-source waiter. A close wakes a
  waiting read select, which then observes `Closed` through `core.recv`.
- A source can become ready between the failed probe and `task::block`. The
  scheduler rechecks after the block and calls `task::wake_any_sources(set)`,
  closing the lost-wake window for every source kind. No stream/console/event
  lock is held while a slot lock is taken.
- `stream::readable(task, handle)` is data-or-close on the caller's receive
  side. A stale or foreign handle reports ready so the following receive
  returns an error instead of sleeping forever.

`Host` gains `console_line_ready`, `input_ready` and `stream_readable`, all
defaulting to `false`. A host with no sources therefore reports `WouldBlock`
and `next_any` blocks, which is also the bare `langhost` behaviour; `langhost`
overrides them for its scripted line, its small event queue and its loopback
FIFO. The existing `read_line` and `next_event` signatures, single-source waits
and wake paths are unchanged.

## Consequences

- The application owns the loop: choose a ready index, then perform the
  ordinary read. There is no selector object, no registration, no callback and
  no hidden buffer.
- The console stays unowned. Two tasks may select on the same console line and
  the first `read_line` wins; the loser's next probe reflects the new state.
- A ready report can be stale by design. Applications must treat the following
  read as authoritative; the proof app documents this in its header.
- The source array is exactly `array[uint, 8]`. The nominal count gives the
  machine a bounded read without a second immediate, and keeps the compiler
  check structural. A future larger set needs a language change, not a silent
  overflow.
- `next_any` re-reads the caller's array and count on every retry. Changing the
  array while blocked is well-defined at the next retry, which is what lets an
  application narrow its set after one arm completes.
- Wake-ups are slightly broader: an `Any` set can be woken by a source it no
  longer cares about if the caller mutated its array while blocked. The retry
  re-probes the new array and blocks again if nothing is ready, so a spurious
  wake costs at most one quantum slice, never correctness.
- No new opcode, no cap change, no `stdlib` helper. The image gains the
  `multiplex` and `multiplex_sender` samples; `stdlib` stays at 125 functions.

## Alternatives considered

- **Block on a fixed union of all three sources implicitly.** Rejected: it
  changes every existing wait and makes a task that only wants the console
  sensitive to unrelated events.
- **A selector handle with `add`/`remove`.** Rejected: fixed handle tables and
  lifetime rules for no gain over a bounded array passed per call.
- **Return ready booleans for every source.** Rejected: it encourages polling
  loops and grows the result shape; the first-index form composes with a single
  read and keeps the ABI small.
- **Consume the source while probing.** Rejected: it would make the console
  owned, require a buffered hand-off per source and break `try_*` semantics.

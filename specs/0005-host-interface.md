# 0005: Host interface

- **Status:** Accepted
- **Scope:** `core`, `syslib`, interpreter, runtime, host-side language tooling
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0011, 0014, 0015
- **Updated:** 2026-11-14

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
| `realtime_on` | `fn realtime_on() -> Status` | no; `Full` if no core is available |
| `heartbeat` | `fn heartbeat()` | no; resets the realtime deadline |
| `run` | `fn run(path: &str, args: &str, diag: &mut Diagnostic, reason: &mut ExitReason) -> Status` | yes, until the child terminates |
| `spawn` | `fn spawn(path: &str, args: &str, diag: &mut Diagnostic, id: &mut TaskId) -> Status` | no; reserves an id and returns |
| `wait` | `fn wait(id: TaskId, reason: &mut ExitReason) -> Status` | yes if the task is live |
| `kill` | `fn kill(id: TaskId) -> Status` | no; requests termination |

- `realtime_on` promotes the calling task, one way, to the realtime class: it is claimed
  before normal tasks and is not released at quantum expiry. It returns `Status.Full` when
  the number of realtime tasks would exceed the number of online cores. A promoted task
  must call `heartbeat` at least once per second of non-blocked time; otherwise it is
  terminated with `fault(WatchdogTimeout)` (`0007-scheduler.md`, ADR 0016).
- `heartbeat` is a progress claim, not a liveness check: the interpreter already shows
  that a task is executing at every safe point. The application is saying "one control
  cycle completed", which is what catches a livelocked controller.
- `run` and `spawn` enqueue a compile-and-run request for the kernel REPL service
  (`0002-architecture.md`, ADR 0017). `run` is `spawn` + `wait`. The request queue holds
  1024 pending requests by default; it is a compile-time constant, overridable at build
  time. A full queue returns `Status.Full` or blocks.
- `TaskId` is a global, monotonically increasing `u64`, never reused; zero is invalid.
  `spawn` reserves one at request time. There is no per-task handle table.
- The argument is a fixed `string[1024]`. The kernel copies it into the request and
  passes it to the child's `main(args: const string[1024])`; it never parses it. A longer
  argument is `Status.CapacityExceeded`.
- `path` is relative to the image root and limited to 256 bytes; a longer value returns
  `Status.CapacityExceeded`. A path with a leading `/`, an empty component, a trailing `/`
  or a `..` component is `Status.Invalid`, as is a directory or other non-regular entry;
  a well-formed path that names nothing is `Status.NotFound`. Matching is exact and
  case-sensitive.
- The caller supplies the diagnostic buffer. `run` and `spawn` fill `diag` on
  `Status.Invalid` with the failing unit, line, column and a truncated message, and clear
  it on every other status. There is no global "last error" slot — it would race between
  concurrent compiles — and no diagnostic stream.
- `wait` is `wait(id)`. If the task is live it blocks; if it has terminated, the exit
  reason is served from a small global ring (64 entries by default); if the record has
  been evicted or the id is unknown, it returns `Status.NotFound`.
- Any application may wait on or kill any id; there is no capability check (vision R14).
- Time is monotonic nanoseconds since boot, read from one global 64-bit counter. The boot
  core's periodic timer tick (1 kHz by default) advances it; every core reads the same
  value, so a task observes the clock consistently across migration. It does not wrap in
  any realistic uptime; the comparison is unsigned and the deadline is absolute.
- Resolution is the tick period, one millisecond by default. That bounds `sleep_until`
  precision and budget accounting error, and is well below the 10 s budget and 1 s
  heartbeat. The TSC is deliberately not used: in QEMU TCG it is per-vCPU and can drift,
  which would make two cores disagree about a task's budget or heartbeat. The APIC timer
  frequency is calibrated against the PIT or HPET at boot (`0007-scheduler.md`).
- `console_read` is a byte stream. It blocks until at least one byte is available,
  returns as many bytes as are available up to the destination length, and never waits for
  a line or a full key sequence.
- The kernel does not echo, does not assemble lines, does no editing and exposes no key
  events; prompt, echo, history and line editing are `stdlib`/shell code. The staged
  `core.read_line` below is a deliberate first-milestone exception until application-side
  editing exists (Q44).
- Console bytes pass through unvalidated: the kernel never checks UTF-8 and never
  interprets escape sequences. In the first milestone the keyboard driver emits US ASCII,
  with `Enter` as `\n` (LF only), `Backspace` as `0x7F`, `Tab` as `\t` and `Ctrl+C` as
  `0x03`. Every other non-text key is discarded.
- `Ctrl+C` is not interpreted by the kernel. It is a byte an application may read, for
  example to kill or interrupt the child it is waiting on.
- The console has no owner. Any task may read, and a byte is consumed by the first task
  that reads it. The first milestone assumes one interactive application.
- There is no `poll` or `select`: a task cannot wait on the console and a stream at the
  same time. It blocks on one source at a time (open question 12).
- `send` is atomic (`0006-streams.md`): it either enqueues the whole slice or returns an
  error. A slice larger than the endpoint capacity can never succeed and is a
  `CapacityExceeded` status.
- `receive` copies what is available, up to the destination length; a byte stream has no
  message boundaries. Framing is a `stdlib` concern.

#### Implemented ABI: `spawn`/`wait`/`kill` return `Status`

`Status` is a predeclared enum, `Diagnostic` is a predeclared record and `ExitReason` is
the predeclared sum type `Ok | Killed | Faulted(code: FaultCode)` (spec 0003), so all three
primitives use the target shapes: the return is a real `Status`, and the out-parameters are
a record and a sum value at the fixed `abi::layout` offsets.

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.spawn` | `(path: const string[0], args: const string[0], var diag: Diagnostic, var id: int) -> Status` | `Ok` with the reserved global id written through `id` and `diag` cleared; `NotFound`, `Invalid`, `Full` or `CapacityExceeded` with `id` untouched, `diag` filled and no task created |
| `core.wait` | `(id: int, var reason: ExitReason) -> Status` | `Ok` with `reason`'s tag/payload written (`Ok`, `Killed` or `Faulted(FaultCode)`); `NotFound` for an unknown or evicted id; `Invalid` for id zero or the caller itself |
| `core.kill` | `(id: int) -> Status` | `Ok`, `NotFound` (unknown or finished), or `Invalid` (id zero or the caller itself) |

- `Host::spawn(path, args, diag: &mut [u8])` receives a slice shaped like the record
  (`layout::diagnostic::SIZE` bytes). The interpreter hands the host a local
  record-shaped buffer, so the path/argument borrows and the destination record can never
  alias; the bytes are copied into the caller's `state`/`stack` reference afterwards. On
  success the host clears the slice; on failure it fills it. The kernel writes unit,
  message, line and column from the compiler's `Diagnostic`; the host toolchain does the
  same for a compile error it discovers synchronously.
- `Host::wait`'s `Exited(reason)` maps to the sum layout: the tag is `0` Ok, `1` Killed or
  `2` Faulted, and `Faulted`'s payload is the `FaultCode` ordinal (`0` otherwise).
  `WaitOutcome::Status(s)` pushes `s` and leaves `reason` untouched; `Blocked` keeps both
  operands on the stack and retries the instruction after a wake-up. `Host::kill` pushes
  its returned `Status`. `abi::Status` and the language `Status` share ordinals, so neither
  side translates.
- `Diagnostic` is cleared as empty text, `0` line/column/task id and `-1` for
  `handle`/`offset`/`length`.
- `TaskId` is an `int` (`0` is invalid) until a distinct type exists.
- **Still staged:** a failure the service discovers *after* `spawn` returned (a compile
  error, a read failure) is printed to the kernel console and recorded as
  `Faulted(IoError)`; the parent's `wait` observes `Faulted` with payload `IoError`.
  Reporting it through `diag` needs a request-completion path and is out of scope.
  `core.run` is not exposed until `stdlib` exists (Q43).

#### Staged line discipline: `core.read_line`

The target rule above stands: `console_read` is a byte stream, the kernel does not echo
or assemble lines, and prompt/history/editing are application code. The first milestone
cannot follow it yet because the language can read and write single bytes but cannot
delete or insert one inside a buffer, so this slice stages a line reader in the kernel
console layer:

| Primitive | Staged signature | Result |
|-----------|------------------|--------|
| `core.read_line` | `(var buf: string[N]) -> int` | copied length (`0..=N`), or blocks until a line is complete |

- The service opportunity polls COM1 and the PS/2 keyboard into one static FIFO.
  Printable bytes append to a 256-byte pending line and are echoed; Backspace erases
  (`\b \b`); Enter completes the line into a four-line ring and echoes `\n`; Ctrl+C
  clears the pending line and echoes `^C\n`. Other keys are dropped.
- A byte past the pending capacity is refused: it is neither stored nor echoed. A
  finished line longer than `N` is truncated to `N`; the rest is discarded and the
  copied length is returned. There is no `CapacityExceeded` result in this staged form.
- `read_line` returns `None` when no line is complete; the interpreter then returns
  `Blocked(Console)` without advancing and retries the instruction after the scheduler
  wakes the task.
- The host mock has no line ring: `langhost` returns one fixed line (`host`) per call so
  host-side programs exercise the same code path without blocking.
- This is temporary. Replace it with application-side editing as soon as indexing and
  string operations exist (Q44).

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

#### Staged syslib: ports as scalars (Q45)

The first `syslib` slice implements only 8-bit port I/O, and `Port` is staged as a plain
`uint` instead of an opaque handle (spec 0003, "Staged port handle"):

| Primitive | Staged signature | Result |
|-----------|------------------|--------|
| `syslib.port_in8` | `(port: uint) -> uint` | the byte read, zero-extended; the low 16 bits of `port` are used |
| `syslib.port_out8` | `(port: uint, value: uint) -> int` | `0` on success, `-4` (`Status.Invalid`) when `value > 255` (the port is not touched) |

The interpreter routes these through `Host::port_in8(u16) -> u8` and
`Host::port_out8(u16, u8)`. Both have defaults (zero and a no-op) so the host mock and
tests need no hardware; the kernel implements them over `arch_x86_64::port` (`inb`/`outb`).
This is a staging shape, not the ABI: the target API has opaque `Port`/`MmioRegion`
handles, `mmio_map/read/write` bounds-checked by the interface, and the DMA model above.
`syslib` is still a distinct namespace with no capability check (H4).

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
- No process or thread primitive: an application is one task. `spawn` starts another
  application instance, not a thread in the caller's task.
- No debug or introspection primitive in the first milestone; counters and task listing are
  designed after the shell exists.

## Interfaces

```rust
// Illustrative. The concrete shape is fixed when the workspace is created.
trait Host {
    fn console_write(&self, bytes: &[u8]);
    fn console_read(&mut self, buf: &mut [u8]) -> usize;
    fn read_line(&mut self, buf: &mut [u8]) -> Option<usize>;
    fn yield_now(&mut self);
    fn send(&mut self, ep: Endpoint, bytes: &[u8]) -> Status;
    fn try_send(&mut self, ep: Endpoint, bytes: &[u8]) -> Status;
    fn receive(&mut self, ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>;
    fn try_receive(&mut self, ep: Endpoint, buf: &mut [u8]) -> Result<usize, Status>;
    fn monotonic_ns(&self) -> u64;
    fn sleep_until(&mut self, deadline_ns: u64);
    fn realtime_on(&mut self) -> Status;
    fn heartbeat(&mut self);
    fn run(&mut self, path: &str, args: &str, diag: &mut Diagnostic, reason: &mut ExitReason) -> Status;
    fn spawn(&mut self, path: &[u8], args: &[u8], diag: &mut [u8]) -> Result<u64, Status>;
    fn wait(&mut self, id: TaskId, reason: &mut ExitReason) -> Status;
    fn kill(&mut self, id: TaskId) -> Status;
}

trait Syslib {
    fn port_in8(&self, port: u16) -> u8;
    fn port_out8(&self, port: u16, value: u8);
    fn mmio_map(&mut self, phys: u64, len: usize) -> Result<MmioRegion, Status>;
    fn mmio_read32(&self, region: MmioRegion, offset: usize) -> u32;
    fn mmio_write32(&mut self, region: MmioRegion, offset: usize, value: u32);
}
```

The language sees these as `core.*` and `syslib.*`: `console_write` is `core.write`,
`monotonic_ns` is `core.clock`, and so on. The namespace split is the developer-visible
signal for origin and cost (`0003-language.md`).

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| `console_read` with no input | Task blocks; no CPU consumed |
| `core.read_line` with no finished line | Task blocks; no CPU consumed (staged line discipline, above) |
| `console_read` sees a non-text key | Key is discarded; no byte is produced |
| Two tasks read the console | Bytes are split between them; no ownership, no arbitration |
| Task needs the console and a stream at once | Not expressible; one blocking wait at a time (open question 12) |
| `send` with a full queue | Blocks by default; `try_send` returns `WouldBlock` |
| `send` larger than capacity | `CapacityExceeded`; cannot ever succeed |
| `receive` from an empty queue | Blocks; `try_receive` returns `WouldBlock` |
| Endpoint peer dead | `Closed`; blocked peer wakes with `Closed` |
| `sleep_until` in the past | Returns immediately |
| Realtime requested with no free core | `Status.Full`; the task stays normal |
| Realtime task misses its heartbeat | Terminate at the next safe point; `fault(WatchdogTimeout)`; recovery rejected; `exit`; reclaim |
| `run`/`spawn` path not found | `Status.NotFound`; `diag` filled; no task created |
| Argument longer than 1024 bytes | `Status.CapacityExceeded`; `diag` filled; no request enqueued |
| Path longer than 256 bytes | `Status.CapacityExceeded`; `diag` filled; no request enqueued |
| Request path malformed or not a regular file | `Status.Invalid`; `diag` filled; no task created |
| `run`/`spawn` compile error | `Status.Invalid`; `diag` filled; no task created |
| Request queue full | `Status.Full`; `diag` filled; `run` blocks only on the child |
| `wait` on an unknown or evicted id | `Status.NotFound` |
| `kill` on a terminated or unknown id | `Status.NotFound` |
| `kill` on a queued request | Cancelled; `ExitReason.Killed` recorded; no task created |
| `kill` on a live task | Requests termination at the next safe point; `exit(Killed)` runs |
| Child terminates while the parent is blocked in `wait` | Parent wakes with the child's `ExitReason` |
| Parent terminates with live children | Children continue (ADR 0017 addendum 2) |
| `mmio_map` of an invalid or conflicting range | `Status` error; no partial mapping |
| `mmio_read/write` out of range | Fault (`OutOfBounds`); the handle is bounds-checked |
| Host mock asked for hardware | `Status` error, never a panic |
| Primitive called after task termination | Cannot happen; the task is not runnable |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | ~~`console_read` granularity: bytes, lines, or key events~~ | **Resolved** — byte stream; no echo, no line assembly, no key events; US ASCII with `\n`, `0x7F`, `\t`, `0x03`. |
| 2 | ~~Clock source and unit (TSC, PIT, APIC timer) and the `sleep_until` granularity~~ | **Resolved** — one global nanosecond counter advanced by the boot core's 1 kHz timer tick; no TSC; 1 ms resolution. |
| 3 | ~~Whether a minimal debug/introspection primitive is worth adding to the first milestone~~ | **Resolved** — none in M1; task listing and counters are designed after the shell exists. |
| 4 | Whether `syslib` needs its own trait or shares the host trait | Implementation layout; no semantic impact. |
| 5 | DMA and block primitives | When the first real device is added; M1 reads ext2 through the in-memory module. |
| 6 | ~~Whether one core is reserved for the REPL/system so realtime tasks cannot starve it~~ | **Resolved** — no reservation; realtime may occupy every core (ADR 0016 addendum). |
| 7 | ~~Compile-error diagnostics: a fixed buffer, a stream, or a status only~~ | **Resolved** — caller-owned `var diag: Diagnostic` out-parameter on `run`/`spawn`; no global slot. |
| 8 | ~~`run`/`spawn` arguments, working directory and environment~~ | **Resolved** — fixed `string[1024]`; the child parses it (ADR 0017 addendum). |
| 9 | ~~Parent death with live children~~ | **Resolved** — children continue (ADR 0017 addendum). |
| 10 | ~~Exit-reason ring size (64?) and whether `run` blocks on a full queue~~ | **Resolved** — ring default 64; a full request queue returns `Status.Full`. |
| 11 | ~~Whether the argument string is `core.arguments` or a `main` parameter~~ | **Resolved** — `main(args: const string[1024])`. |
| 12 | Multiplexing: a task cannot wait on the console and a stream at the same time | When an application must serve input from more than one source; a `poll`-style primitive would widen the ABI. |
| 13 | ~~Path validation: may a request name a directory, start with `/`, or contain `..`?~~ | **Resolved** — relative to the image root, no leading/trailing `/`, no empty or `..` component, regular files only; malformed is `Invalid`, absent is `NotFound`. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-12 | Initial draft: core and syslib namespaces, blocking semantics, mockability, absence of allocation and spawn. |
| 2026-11-12 | Added realtime promotion and heartbeat. |
| 2026-11-12 | Added `run`/`spawn`/`wait`/`kill`, fixed `string[1024]` arguments delivered as `main(args)`, global task ids and the exit-reason ring; removed per-task handles (ADR 0017 and addenda 1–2). |
| 2026-11-13 | Console input fixed as a byte stream (no echo, no line assembly, no key events, `\n`/`0x7F`/`\t`/`0x03`); clock fixed as one global nanosecond counter at 1 ms resolution (no TSC); `run`/`spawn` take a caller-owned `Diagnostic` out-parameter; no debug or DMA primitive in M1; consoles are unowned and there is no multiplexing (open question 12). |
| 2026-11-13 | `run`/`spawn` paths are limited to 256 bytes and are relative to the image root; path validation stays open (question 13). |
| 2026-11-13 | Path validation settled (question 13): relative, no leading/trailing `/`, no empty or `..` component, regular files only; malformed is `Invalid`, absent is `NotFound`. `kill` semantics: queued requests are cancelled with `Killed`, live tasks terminate at the next safe point. |
| 2026-11-13 | Staged scalar `spawn`/`wait`/`kill` ABI added for the first milestone (`int` task id, negated `Status`, `0`/`1`/`2 + fault_code` wait results); the record-based `Diagnostic`/`Status`/`ExitReason` form remains the target API (see "Staged ABI"). |
| 2026-11-13 | Staged `core.read_line` line discipline added: polled COM1/PS-2 bytes, kernel echo and a four-line ring, blocking on an unfinished line. Temporary until indexing exists (Q44). |
| 2026-11-14 | First `syslib` slice: `port_in8`/`port_out8` as `uint` scalars through `Host::port_in8`/`port_out8` defaults, kernel implementation over `arch_x86_64::port`; opaque `Port`/MMIO/DMA remain the target (Q45). |
| 2026-11-14 | REPL ABI now uses the predeclared `Status` enum: `core.spawn(path, args, var id)`, `core.wait(id, var reason)` and `core.kill(id)` return `Status`; only the `Diagnostic` and `ExitReason` records remain staged (Q43 narrowed). `Host` is unchanged (`Result<u64, Status>`, `WaitOutcome`, `Status`); the kernel maps `abi::Status` directly. |
| 2026-11-14 | Record-based REPL ABI: `core.spawn(path, args, var diag: Diagnostic, var id: int)` and `core.wait(id, var reason: ExitReason)` replace the staged out-parameters. `Host::spawn` gains a record-shaped `&mut [u8]` `diag` slice (filled on a synchronous failure, cleared on success); `wait` writes `kind`/`code` through the record. A compile failure the service discovers after `spawn` returned still prints to the kernel console and records `Faulted(IoError)`; `TaskId` stays an `int` and `core.run` is not exposed (Q43 narrowed to sum types and `core.run`). |
| 2026-11-14 | `ExitReason` is now the predeclared sum type `Ok | Killed | Faulted(code: FaultCode)`, so `core.wait` writes a tag (0/1/2) at offset 0 and the `FaultCode` ordinal at offset 8, exactly the previous 16-byte image under new names (`TAG_OFFSET`/`PAYLOAD_OFFSET`). Applications match the reason with an exhaustive `case`; `reason.kind`/`reason.code` no longer exist. `FaultCode` is a predeclared enum with qualified variant names. Q43 is now `core.run`, `TaskId` and the request-completion diagnostic path. |

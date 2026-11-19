# 0005: Host interface

- **Status:** Accepted
- **Scope:** `core`, `syslib`, interpreter, runtime, host-side language tooling
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0011, 0014, 0015, 0021, 0048, 0050
- **Updated:** 2026-11-19

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
| `fs_list` | `fn fs_list(path: &[u8], index: u64, name: &mut [u8]) -> Result<FsEntry, Status>` | no; root-only, directory order |
| `fs_read` | `fn fs_read(path: &[u8], offset: u64, dst: &mut [u8]) -> Result<usize, Status>` | no; at most 256 bytes, 0 at EOF |
| `fs_size` | `fn fs_size(path: &[u8]) -> Result<u64, Status>` | no; regular root files |
| `terminal_attach` | `fn terminal_attach(window: u64) -> Status` | no; one machine-wide terminal, owner-checked by the kernel host |
| `halt` | `fn halt() -> Status` | no; stops the machine (never returns on the kernel), `Invalid` on a bare host |
| `reboot` | `fn reboot() -> Status` | no; resets the machine, `Invalid` on a bare host |
| `net_config` | `fn net_config(ip: u32, mask: u32, gateway: u32, dns: u32) -> Status` | no; machine-wide static IPv4 configuration (`core.net_config`, builtin 62) |
| `net_info` | `fn net_info() -> Result<NetInfo, Status>` | no; four `u32` values (`core.net_info`, builtin 63) |
| `net_mac` | `fn net_mac() -> u64` | no; packed six-byte MAC, `0` when absent (`core.net_mac`, builtin 64) |
| `socket_open` | `fn socket_open(kind: u64, ip: u32, port: u16, local: u16) -> Result<u64, Status>` | no in this slice; `1` TCP (handshake in progress), `2` UDP (`core.socket_open`, builtin 65) |
| `socket_send` | `fn socket_send(handle: u64, data: &[u8]) -> Result<usize, Status>` | no in this slice; `WouldBlock` until connected/TX space (`core.socket_send`, builtin 66) |
| `socket_recv` | `fn socket_recv(handle: u64, dst: &mut [u8]) -> Result<usize, Status>` | no in this slice; `WouldBlock` when empty, `Ok(0)` EOF (`core.socket_recv`, builtin 67) |
| `socket_close` | `fn socket_close(handle: u64) -> Status` | no; owner-checked (`core.socket_close`, builtin 68) |

- **Networking (spec 0020, M10.1–M10.4).** `NetInfo` is `{ ip, mask, gateway, dns: u32 }`
  in host order (first octet in bits 24..31). A socket is an opaque
  `(generation << 8) | index` handle; `0` is invalid, a foreign live handle is
  `Invalid`, a stale one `NotFound`, and a full table `Full`. The first slice is
  **non-blocking**: `socket_send`/`socket_recv` return `WouldBlock` and the caller
  retries with `core.clock()`; `socket_open` returns immediately and the kernel's
  service opportunity drives the TCP handshake. `core.socket_recv` copies at most
  `abi::net::RECV_CHUNK` (2048) bytes and the compiler stages the destination's
  declared `bytes[N]` capacity as a hidden scalar operand, so a short buffer can
  never be overrun. `Host::socket_readable` exists with a `false` default for the
  future blocking/multi-source arm; `WaitFor::Net`/`Deadline` and
  `core.socket_source` arrive in the follow-up. `Task::reclaim` calls
  `Host`-independent `net::close_task(task)` for every owned socket.

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
- `console_read` is a byte stream. It returns as many bytes as are available up to the
  destination length and never waits for a line or a full key sequence. The kernel does
  not echo, does not assemble lines, does no editing and exposes no key events; prompt,
  echo, history and line editing are application code (Q44 resolved, ADR 0037).
- The service opportunity drains **COM1** into a fixed raw byte ring and feeds **PS/2 key
  presses** only into the machine-wide event queue. Serial is the byte stream; the
  keyboard is events. A key is therefore delivered exactly once, and a console reader
  and an event reader see different bytes rather than duplicates (spec 0010).
- Console bytes pass through unvalidated: the kernel never checks UTF-8 and never
  interprets escape sequences. A serial terminal delivers US ASCII and control bytes,
  with `Enter` as CR or LF, `Backspace` as `0x7F` or `0x08`, `Tab` as `\t` and `Ctrl+C` as
  `0x03`; control-byte policy belongs to the application. PS/2 key events carry the text
  byte and modifiers in `Event.code` (`abi::key`).
- `Ctrl+C` is not interpreted by the kernel. It is a byte an application may read, for
  example to kill or interrupt the child it is waiting on.
- The console has no owner. Any task may read, and a byte is consumed by the first task
  that reads it. The first milestone assumes one interactive application.
- Console output is still one byte stream to the host, but since M2.3.4 (spec 0012) the
  kernel paints it into a kernel-owned terminal window instead of the whole screen;
  kernel `println!` diagnostics go to a separate system-log window. The serial mirror is
  unchanged and both windows are text-only targets, not a change to this host contract.
- Since Q39 a task may wait on several input sources at once. `core.poll_any` is a
  non-blocking readiness probe and `core.next_any` blocks on the whole set; both inspect
  a `const array[uint, 8]` whose entries are `0` (console input byte available), `1`
  (input event) or a stream handle (`core.stream_source`). Readiness is a snapshot, not a
  reservation: the following `read_input`/`next_event`/`recv` is authoritative if another
  task consumed the source in between (see "Multi-source waits").
- `send` is atomic (`0006-streams.md`): it either enqueues the whole slice or returns an
  error. A slice larger than the endpoint capacity can never succeed and is a
  `CapacityExceeded` status.
- `receive` copies what is available, up to the destination length; a byte stream has no
  message boundaries. Framing is a `stdlib` concern.

#### Implemented ABI: `spawn`/`wait`/`kill`/`run` return `Status`

`Status` is a predeclared enum, `Diagnostic` is a predeclared record and `ExitReason` is
the predeclared sum type `Ok | Killed | Faulted(code: FaultCode)` (spec 0003), so every
primitive uses the target shapes: the return is a real `Status`, and the out-parameters are
a record and a sum value at the fixed `abi::layout` offsets.

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.spawn` | `(path: const string[0], args: const string[0], var diag: Diagnostic, var id: int) -> Status` | `Ok` with the reserved global id written through `id` and `diag` cleared; `NotFound`, `Invalid`, `Full` or `CapacityExceeded` with `id` untouched, `diag` filled and no task created |
| `core.run` | `(path: const string[0], args: const string[0], var diag: Diagnostic, var reason: ExitReason) -> Status` | spawn + wait; `Ok` with `reason`'s tag/payload written, a synchronous spawn failure exactly as `spawn`, and a load failure the service discovers after the spawn returned with `reason = Faulted(IoError)` plus the compiler's diagnostic copied into `diag` |
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
- `core.run` is `spawn` + `wait` with no extra kernel state: the interpreter
  spawns once, keeps the pending id in the `Machine`, writes `diag` from the
  spawn result, and retries only the wait after a `Blocked`. The id is cleared
  when the run completes or returns a `Status`.
- **Request-completion diagnostic:** a failure the service discovers *after*
  `spawn` returned (a compile error, a read failure) is printed to the kernel
  console, recorded as `Faulted(IoError)`, and its exit-ring entry also carries
  the compiler's `Diagnostic` image (192 bytes). `Host::diagnostic_for(id, diag)`
  (default `false`) lets the interpreter copy it into `run`'s caller-owned
  `diag`. `core.wait` keeps its spec signature: it observes `Faulted(IoError)`
  with no diagnostic, while `run` callers see both.

#### Implemented ABI: realtime promotion and heartbeat

`core.realtime_on()` (builtin 11) and `core.heartbeat()` (builtin 12) are
implemented with the shapes above:

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.realtime_on` | `() -> Status` | `Ok` when the task is or becomes realtime; `Full` when every online core already holds a realtime task. A repeat call on a realtime task is `Ok` and takes no second slot |
| `core.heartbeat` | `()` | no result; resets the caller's one-second deadline. A no-op on a normal task |

- `Host::realtime_on(&mut self) -> u64` defaults to `Status::Ok` and
  `Host::heartbeat(&mut self)` defaults to a no-op, so host mocks and tests need
  no scheduler. The kernel's host calls `sched::admit_realtime()` — a CAS loop
  against `smp::online_count()` — and records the promotion and heartbeat count
  for the loop to apply at the next quantum boundary.
- The heartbeat deadline is one second of non-blocked time (`HEARTBEAT_NS`, a
  compile-time constant; OPEN-QUESTIONS Q4 closed). The kernel accumulates
  claim-to-release time in `Task::budget_ns`, and a block adds nothing. A missed
  deadline terminates with `Faulted(WatchdogTimeout)`; the admission slot is
  released only at termination, so a blocked controller keeps its place. See
  `0007-scheduler.md`, "Realtime tasks".

#### Implemented ABI: stream send/receive

The four stream operations are implemented with the `Status`/staged-result shapes
above (spec 0006, spec 0008):

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.send` | `(ep: out_stream[N], data: const string/bytes) -> Status` | `Ok` after copying the whole buffer atomically; `CapacityExceeded` when `len(data) > N`; `Closed`; blocks while the free space is smaller than the buffer |
| `core.try_send` | same | `WouldBlock` instead of blocking |
| `core.recv` | `(ep: in_stream[N], var dst: bytes[M]) -> int` | copied count (`>= 1`), or `-1` `WouldBlock` (try) / `-2` `Closed`; blocks on an empty queue. A `string` destination is a compile error |
| `core.try_recv` | same | `-1` instead of blocking |

- `Host::stream_send(handle: u64, bytes: &[u8]) -> Result<(), StreamError>` and
  `Host::stream_recv(handle: u64, dst: &mut [u8]) -> Result<usize, StreamError>` are the
  non-blocking host hooks. `StreamError` is `WouldBlock`, `Closed`, `Invalid` or
  `CapacityExceeded`; both default to `Invalid`, so a host that does not implement streams
  still compiles. The interpreter turns `WouldBlock` into
  `Outcome::Blocked(WaitKind::Stream(handle))` for `core.send`/`core.recv`, and into the
  staged status for the `try_*` variants. On a block it restores the operands and `pc`, so
  the retry sees the same instruction and no bytes are copied twice.
- The kernel's host calls the stream table; every operation validates the 8-byte handle
  (generation and index) and that the calling task owns that endpoint direction. The
  table, its arena and its lock ordering are spec 0006, "Dynamic loading".
- The loader, not the host, creates or pairs the endpoints before the task is placed, so
  `stream_send`/`stream_recv` never see an unopened endpoint.

#### Console input: `core.read_input` (Q44, ADR 0037)

The console is a raw byte stream and the kernel is out of line editing.

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.read_input` | `(var dst: bytes[N]) -> int` | copies the bytes that are waiting (`0..=N`) and returns the count; `0` when none. Never blocks |

- The service opportunity drains COM1 into a fixed **512-byte ring**. Bytes are stored
  verbatim: no echo, no editing, no line completion, no byte interpretation.
- If the ring is full, the **newest** byte is dropped and counted (the oldest bytes
  survive); the ring never rewinds and the drop is not visible because the kernel does
  not echo. This policy is deterministic and is proved by the boot self-check
  `console: input ok`.
- PS/2 key presses do **not** enter the ring; they become events in the machine-wide
  queue (spec 0010). Serial is the byte stream; the keyboard is events.
- A non-zero return publishes the destination length word together with the bytes; a
  zero return publishes length `0`. There is no `WouldBlock` status: an empty read is a
  result, and an application that wants to sleep first waits on source `0` with
  `core.poll_any`/`core.next_any`.
- The interpreter's `READ_INPUT` opcode carries the destination capacity as its
  immediate and writes a raw `bytes` buffer. It reuses the retired `READ_LINE` slot
  (`0x42`), so ABI v1 still has 75 opcodes (spec 0008).
- `langhost` drains a scripted byte queue (`HOST_INPUT`, or `run_with_input`) so the same
  application-side editing runs on the host without blocking.
- Echo, prompts, history, cursor movement and line assembly are application code. The
  shell echoes each accepted byte through `core.write` (Backspace as `\x08 \x08`,
  Enter as `\n`, Ctrl+C as `^C\n`), and edits its `string[256]` with the M2.4.1
  `stdlib.str_*` helpers (spec 0003, "String editing").

#### Multi-source waits (Q39, implemented)

The language surface is spec 0003, "Multi-source waits"; this is the host contract.

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.poll_any` | `(sources: const array[uint, 8], count: int, var ready: int) -> Status` | `Ok` with the zero-based index of the first ready source written through `ready`; `WouldBlock` when none is ready; `Invalid` when `count` is outside `1..=8`. `ready` is untouched on a non-`Ok` result |
| `core.next_any` | same | identical readiness report; when none is ready the task blocks on the set and retries the instruction after a wake-up |

- Source `0` is a console input byte being available (`core.read_input` would return at
  least one byte), `1` is one input event (`syslib.next_event`, including a pending
  window `Expose`), and any other value is a stream handle whose data or close is ready.
  This is a read select; send-space readiness is not included.
- The interpreter validates `count` and the array region, then calls one `Host` probe per
  source in caller order: `console_byte_ready()`, `input_ready()` and
  `stream_readable(handle)`. All three default to `false`, so a host with no sources
  reports `WouldBlock` and `next_any` blocks exactly like an absent source.
- The kernel host maps them to `console::has_byte`, `event::has_event() ||
  window::has_expose(current)` and `stream::readable(task, handle)`. `stream::readable`
  is data-or-close on the caller's receive side; a stale or foreign handle reports ready
  so the following receive errors instead of sleeping.
- `core.stream_source(ep) -> uint` is compiler-lowered (a value reference plus
  `load_ref`, no builtin) and is the only way to name a stream in the array. It does not
  change the endpoint rules: every other use as a value is still a compile error.
- `syslib.next_event` keeps its signature, single-source wait and wake path. A console
  byte, an event insertion and a stream send/receive/close wake a matching
  `WaitFor::Any` set (spec 0006, spec 0007); `core.read_input` itself never sleeps.
- Readiness is not consumption. Another task may take the source before the caller's
  read; that read's `WouldBlock`, `Closed` or short count is the authoritative result.



The read-only image is reachable through three host builtins (41–43; spec 0013, ADR
0027). There is no handle, no per-task file state and no writable operation: every call
names a path, and the kernel host resolves it against the same ext2 mount the REPL
service compiles from.

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.fs_list` | `(path: const string[0], index: uint, var name: string[64], var kind: uint, var size: uint) -> Status` | `Ok` with the `index`-th root entry (name, kind, inode size); `NotFound` past the end; `Invalid` for any path other than `""`/`"."`; `CapacityExceeded` for a name over 64 bytes |
| `core.fs_read` | `(path: const string[0], offset: uint, var dst: bytes[256], var read: int) -> Status` | `Ok` with up to 256 bytes at `offset`, setting the destination length and `read` (`Ok`/0 at or past EOF); `NotFound` for a missing name; `Invalid` for a directory, another non-regular kind or a path with `/`; `CapacityExceeded` for a path over 256 bytes; `IoError` for a corrupt image |
| `core.fs_size` | `(path: const string[0], var size: uint) -> Status` | `Ok` with a regular root file's byte size; the same errors as `fs_read` |

- The machine validates the path through `abi::fs` (the same function `core.spawn` uses)
  and the destination regions before calling the host, then publishes the name/chunk
  with one copy only on `Ok`. A failure leaves every out-parameter untouched.
- `Host::fs_list` is `(path: &[u8], index: u64, name: &mut [u8]) -> Result<FsEntry,
  Status>`, `Host::fs_read` is `(path: &[u8], offset: u64, dst: &mut [u8]) ->
  Result<usize, Status>` and `Host::fs_size` is `(path: &[u8]) -> Result<u64, Status>`.
  All three default to `NotFound`, so a mock with no filesystem needs no override.
- The kernel host uses `Ext2::list_entry`/`read_at`/`file_size`. `langhost` implements the
  same root-only view over `samples/apps` (sorted names, the same 256-byte chunk).
  M2.5.1 is root-only by design: a separator is `Invalid`, not a traversal.

#### Implemented ABI: terminal window (M2.5.5)

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.terminal_attach` | `(window_id: int) -> Status` | `Ok` binds the one console terminal grid to the caller's window; `NotFound` for an unknown id; `Invalid` for a foreign window, disabled graphics or a content rectangle below one cell; `Full` when another live task holds the terminal |

- The kernel host supplies the caller's task id (`KernelHost::terminal_attach`), so the
  language has no task id and no window-content primitive. The kernel checks ownership
  and reads the content rectangle in one table lock (`CONSOLE → WINDOW`), releases the
  owner-0 boot terminal on the first task attach, and repaints the new grid.
- `Host::terminal_attach(window: u64) -> Status` defaults to `Invalid`: `langhost` and a
  bare mock have no terminal window, so the shell falls back to serial there.
- `console_write` keeps its signature and semantics; `core.read_input` reads the same
  byte ring wherever output is painted. The attached terminal receives `core.write` only
  (the application echoes its own input); `println!` stays in the System Log; serial
  mirroring is unchanged. A terminal whose window disappears is detached and output
  becomes serial-only until the next attach.

#### Implemented ABI: system control (M2.6.4)

| Primitive | Implemented signature | Result |
|-----------|-----------------------|--------|
| `core.halt` | `() -> Status` | stops every other online core on the wake vector, writes `system: N of M APs stopped` to serial, and halts the caller; never returns on a live kernel |
| `core.reboot` | `() -> Status` | stops the other cores, drives `0xCF9` (`0x02`/`0x06`) and the 8042 pulse reset (`0x64 <- 0xFE`), and halts the caller if the firmware ignores them |

- `Host::halt`/`Host::reboot` take no operands and return `Status`; both default to
  `Invalid`, so a bare mock and `langhost` simply report that the host has no
  system-control authority and the application keeps running.
- The kernel host calls `sched::halt_system`/`sched::reboot`. The shutdown flag and the
  existing `0x40` wake IPI are the stop path: `kernel::wake_ipi` acknowledges
  (`smp::note_stopped`) and halts even a core that is mid-task, so a halt does not wait
  for every core to reach its idle loop. The wait is bounded by the clock and a spin
  ceiling because the boot core's timer can stop before the acknowledgements arrive.
- There is no capability check and no confirmation (vision R14, ADR 0033): every task
  may stop or reset the machine. The desktop shell is the only M2 caller. A reset that
  the firmware ignores ends in a halt instead of returning to the interpreter.

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

#### Staged syslib: graphics (spec 0009)

The shadow-screen primitives are host builtins with a disabled-screen default, so the
host mock and `langhost` need no framebuffer:

| Primitive | Host method | Default |
|-----------|-------------|---------|
| `syslib.screen_width` / `screen_height` | `screen_width(&mut self) -> u64`, `screen_height(&mut self) -> u64` | `0`, `0` |
| `syslib.fill_rect` | `fill_rect(x, y, w, h: i64, color: u32) -> Status` | `Invalid` |
| `syslib.fill_dither` | `fill_dither(...) -> Status` | `Invalid` |
| `syslib.draw_rect` | `draw_rect(...) -> Status` | `Invalid` |
| `syslib.draw_text` | `draw_text(x, y: i64, text: &[u8], color: u32) -> Status` | `Invalid` |
| `syslib.get_pixel` | `get_pixel(x, y: i64) -> u32` | `0` |
| `syslib.present` | `present(x, y, w, h: i64) -> Status` | `Invalid` |
| `syslib.clear` | `clear(color: u32) -> Status` | `Invalid` |

The interpreter owns the error rules (`w <= 0`, `h <= 0`, a colour above `0x00FFFFFF` is
`Status.Invalid` before the host is called); the host clips to the screen and its clip
region. The kernel implements these over `kernel/src/gfx.rs`; the defaults are exactly the
kernel's disabled state, so both paths agree (spec 0009, G15/G16). `draw_text` receives the
resolved `string` bytes, so the host never sees a language reference.

#### Staged syslib: input (spec 0010)

The pointer is also a host builtin, with defaults that describe a machine with no pointer
device:

| Primitive | Host method | Default |
|-----------|-------------|---------|
| `syslib.pointer_x` / `pointer_y` | `pointer_x(&mut self) -> i64`, `pointer_y(&mut self) -> i64` | `0`, `0` |
| `syslib.pointer_buttons` | `pointer_buttons(&mut self) -> u64` | `0` |
| `syslib.pointer_source` | `pointer_source(&mut self) -> u64` | `0` |
| `syslib.inject_pointer` | `inject_pointer(dx, dy: i64, buttons: u32) -> Status` | `Invalid` |

The pointer state itself is machine-wide and kernel-owned (`kernel/src/gfx.rs`, fed by
`kernel/src/mouse.rs` and `kernel/src/usb.rs`); the host methods are read-only views plus
the debug-only injection hook. `pointer_source` names the active device (0 none, 1 PS/2,
2 tablet). The interpreter rejects `buttons > 0b111` with `Status.Invalid` before the host
is called, mirroring the graphics error rules. `inject_pointer` is documented as a test
hook, not a device abstraction (spec 0010, I12).

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
    // Raw console read behind `core.read_input`: non-blocking, 0 when empty.
    fn read_input(&mut self, dst: &mut [u8]) -> usize;
    fn yield_now(&mut self);
    // Non-blocking in the host; the interpreter blocks on WouldBlock for the
    // blocking opcodes and maps it to the staged status for try_*.
    fn stream_send(&mut self, handle: u64, bytes: &[u8]) -> Result<(), StreamError>;
    fn stream_recv(&mut self, handle: u64, dst: &mut [u8]) -> Result<usize, StreamError>;
    // Readiness probes for `core.poll_any`/`core.next_any` (Q39); all default
    // to `false` on a host with no sources.
    fn console_byte_ready(&mut self) -> bool;
    fn input_ready(&mut self) -> bool;
    fn stream_readable(&mut self, handle: u64) -> bool;
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
| `core.read_input` with no bytes waiting | Returns `0` immediately; the task blocks only by calling `core.next_any` first |
| `syslib.next_event` with no event | Task blocks; no CPU consumed |
| A non-text key | It is a key event (code, no text), never a console byte |
| Two tasks read the console | Bytes are split between them; no ownership, no arbitration |
| Task needs the console and a stream at once | Expressed with `core.next_any` over both sources (Q39) |
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
| File path malformed or a subdirectory path | `Status.Invalid`; no out-parameter written |
| File name not found | `Status.NotFound`; no out-parameter written |
| File entry is a directory or another non-regular kind | `Status.Invalid`; nothing read |
| Listing index past the last entry | `Status.NotFound`; nothing written |
| File name longer than the 64-byte listing destination | `Status.CapacityExceeded`; nothing written |
| File read destination below 256 bytes | Compile error (typed code) or `OutOfBounds` fault (hand-made bytecode); the host is not called |
| `run`/`spawn` compile error | `Status.Invalid`; `diag` filled; no task created |
| `run` load error after `spawn` returned | The service records `Faulted(IoError)`; `run` returns `Ok` with that reason and fills `diag` from the exit record; `wait` sees `Faulted(IoError)` without a diagnostic |
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
| 12 | ~~Multiplexing: a task cannot wait on the console and a stream at the same time~~ | **Resolved (Q39)** — `core.poll_any` (non-blocking) and `core.next_any` (blocking) inspect up to eight sources: console line, input event and stream read readiness. One `WaitFor::Any` set, no callback or shared memory; readiness is a snapshot and the following read is authoritative. |
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
| 2026-11-14 | `core.run` implemented as spawn + wait (pending id in the `Machine`, no second spawn on a blocked retry). The request-completion path landed: the exit ring carries a 192-byte `Diagnostic` image for a load failure, `Host::diagnostic_for` (default `false`) copies it, and `run` fills its `diag` while `wait` still reports only `Faulted(IoError)`. Q43 now covers only a distinct `TaskId` type. |
| 2026-11-15 | Realtime promotion and heartbeat implemented: `Host::realtime_on` (default `Ok`) and `Host::heartbeat` (default no-op), builtins 11 and 12, admission against the online core count, and `Faulted(WatchdogTimeout)` from a one-second non-blocked deadline. |
| 2026-11-15 | Streams implemented: `Host::stream_send`/`Host::stream_recv` are non-blocking `Result<_, StreamError>` hooks with `Invalid` defaults. The interpreter turns `WouldBlock` into `Blocked(WaitKind::Stream(handle))` for `core.send`/`core.recv` and into staged `WouldBlock`/`Closed` results for `try_send`/`try_recv`. Endpoints are created and paired by the loader before the task is placed, so the host never sees an unopened handle. |
| 2026-11-16 | Graphics host methods (spec 0009): `screen_width`/`screen_height` (0), `fill_rect`, `fill_dither`, `draw_rect`, `draw_text`, `present`, `clear` (`Invalid`), and `get_pixel` (0), all with disabled-screen defaults. The interpreter validates extents and colours, the host clips and rasterises; the kernel implements them over the shadow screen. |
| 2026-11-16 | Input host methods (spec 0010): `pointer_x`/`pointer_y` (`i64`, 0), `pointer_buttons` (`u64`, 0) and the debug-only `inject_pointer(dx, dy, buttons)` (`Invalid`). The pointer is one machine-wide value owned by the kernel graphics module; the interpreter rejects `buttons > 0b111` before the host is called, and `langhost`/host mocks see the no-device defaults. |
| 2026-11-16 | `pointer_source(&mut self) -> u64` added (spec 0010, I27), default 0: the host reports which device drives the pointer (0 none, 1 PS/2, 2 tablet). It is a read-only view over the same machine-wide state; no other host method changes, so `langhost` and mocks keep the no-device default. |
| 2026-11-16 | Window host methods (spec 0011, M2.3.1): `window_open(x, y, w, h, title) -> Result<u64, Status>`, `window_close`/`window_raise`/`window_move` (`Status`), `window_frame_rect`/`window_content_rect` (`Result<Rect, Status>`) and `window_focus() -> u64`. The defaults are a disabled service (`Invalid` at `open` and on the rect reads, `NotFound` at `close`/`raise`/`move`, focus 0), matching the kernel's empty table. The interpreter decodes the operands and writes a 32-byte `abi::layout::rect` image only on `Ok`; builtins 32–38, no new opcode. |
| 2026-11-16 | M2.3.4 (spec 0012): console output routing is unchanged at this interface (`console_write` bytes, `read_line` lines), but the kernel now paints the terminal into a kernel-owned window and kernel diagnostics into a system-log window; the serial mirror continues for both. A new read-only `window_info` host method is the disabled-service `None` default. |
| 2026-11-17 | Q44 narrows (M2.4.1): the language now has code-point-safe insert/delete/backspace/cursor/selection helpers (`stdlib.str_*`, spec 0003), so the reason the kernel kept a staged line discipline no longer holds at the language level. The staged `core.read_line` and its four-line ring stay for the shell until the terminal window can run the shell in a window with application-side editing (M2.5.5); the host interface itself is unchanged. |
| 2026-11-17 | Application files (spec 0013, M2.5.1): `Host::fs_list` (`Result<FsEntry, Status>`), `Host::fs_read` (`Result<usize, Status>`) and `Host::fs_size` (`Result<u64, Status>`), all defaulting to `NotFound`, and builtins 41–43. The interpreter validates the shared `core.spawn` path syntax and the root-only M2.5.1 subset, stages a 64-byte name and a 256-byte chunk and publishes only on `Ok`; the kernel host uses the existing ext2 mount, `langhost` the sample directory. No opcode, no handle, no writable path. |
| 2026-11-17 | Terminal window (spec 0014, M2.5.5): `Host::terminal_attach(window: u64) -> Status` (builtin 44, default `Invalid`) binds the one machine-wide console terminal to a window owned by the calling task. The kernel host passes its task id, checks ownership and geometry under `CONSOLE → WINDOW`, releases the owner-0 boot terminal on first attach and detaches when the owner's window disappears. `console_write`/`read_line` signatures and semantics are unchanged; the line discipline is not replaced. No opcode. |
| 2026-11-17 | System control (spec 0017, M2.6.4): `Host::halt`/`Host::reboot` (0 → 1, builtins 48/49, default `Invalid`) drive `sched::halt_system`/`sched::reboot`. The shutdown flag plus the existing `0x40` wake IPI stop every other core in the handler (acknowledge then halt), the wait is bounded by the clock and a spin ceiling, and `reboot` resets through `0xCF9` and the 8042 pulse command, halting if the firmware ignores both. No opcode. |
| 2026-11-17 | Multi-source waits (Q39, ADR 0036): `Host::console_line_ready`/`input_ready`/`stream_readable` (all default `false`) back `core.poll_any`/`core.next_any` (builtins 51/52, no opcode). Sources are `0` console line, `1` input event and `>= 2` stream read readiness; `core.stream_source` is compiler-lowered and adds no host method. The kernel maps the probes to `console::has_line`, `event::has_event`/`window::has_expose` and `stream::readable`, and a single `WaitFor::Any` set carries the block; `read_line`/`next_event` are unchanged. |
| 2026-11-19 | Networking (spec 0020, M10.1–M10.4): `Host::net_config`/`net_info`/`net_mac` (builtins 62–64, `Invalid`/`0` defaults) and `Host::socket_open`/`socket_send`/`socket_recv`/`socket_close` (65–68, `Invalid` defaults), plus `Host::socket_readable` (`false`) reserved for the blocking arm. Sockets are owner-checked generation handles; `socket_recv` stages the destination capacity as a hidden compiler operand and copies at most 2048 bytes. The first slice is non-blocking (`WouldBlock` + caller retry); no opcode, ABI v1 stays 75. |
| 2026-11-17 | Q44 resolved (ADR 0037): the kernel line discipline is gone. COM1 drains into a 512-byte raw ring (newest byte dropped and counted when full, no echo); PS/2 key presses go only to the event queue. `Host::read_input(dst) -> usize` (non-blocking, `0` when empty) behind `core.read_input(var bytes[N]) -> int` replaces `Host::read_line`; it reuses the `READ_LINE` opcode slot as `READ_INPUT` (`0x42`), so ABI v1 stays at 75. Source `0` is now "a console byte is available" and the probe is `Host::console_byte_ready`. The shell echoes and edits its own `string[256]` with the `stdlib.str_*` helpers and blocks in `core.next_any`; `langhost` drains a scripted byte queue. |

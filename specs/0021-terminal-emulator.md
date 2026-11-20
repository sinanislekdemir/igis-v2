# 0021: The terminal emulator and console sessions

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`console`, `session`,
  `sched`, `task`, `repl`), `langhost`, `samples/apps`; M11
- **Supersedes:** 0014 (the machine-wide terminal window and `core.terminal_attach`)
- **Superseded by:** none
- **ADRs:** 0053 (a terminal emulator over kernel console sessions)
- **Updated:** 2026-11-20

## Context

The M2.5 terminal was a kernel console: one machine-wide character grid in
`kernel/src/ui/console.rs`, bound to one task-owned window through
`core.terminal_attach` (spec 0014, ADR 0028). That model has one grid, one
input ring and one attachment, so it cannot provide independent terminal
instances, and the kernel grows a terminal UI it does not need.

A terminal is an application, not a kernel text device. The kernel's job is to
move bytes between a terminal emulator and the shell it runs. The emulator owns
the visible grid, the cursor, scrolling and input translation; the shell owns
its command line and working directory. The kernel keeps only byte rings and
task routing, exactly as it does for streams.

The kernel System Log stays a kernel-owned text window for `println!`/`print!`.
There is no kernel terminal window.

## Requirements

### Console sessions

- **S1 (MUST):** The application surface adds four stateful host builtins and
  one compiler-lowered source helper. No bytecode opcode is added; ABI v1 stays
  at **75 opcodes**.

  ```text
  core.session_open(var id: int) -> Status
  core.session_close(id: int) -> Status
  core.session_send(id: int; data: const string[0] or const bytes[N];
                    var sent: int) -> Status
  core.session_recv(id: int; var dst: bytes[N]; var read: int) -> Status
  core.session_source(id: int) -> uint
  ```

  `session_open` is builtin **74**, `session_close` **75**, `session_send`
  **76** and `session_recv` **77**. `session_source` is compiler-lowered: it
  returns `abi::wait::SOURCE_SESSION_BASE + id` and adds no host method.
  `core.terminal_attach` (builtin 44) is retired.
- **S2 (MUST):** A session is a kernel-owned pair of byte rings with a fixed
  capacity: an **input** ring (master to slave) and an **output** ring (slave
  to master). It has one owner task and any number of bound slave tasks. The
  session id is opaque, positive and never reused; a stale id is `NotFound`.
  The session table has one slot per task slot (`MAX_TASKS`), so the number of
  emulators is bounded by the kernel's existing task and window limits, not by
  a separate terminal limit.
- **S3 (MUST):** `session_open` makes the calling task the owner and returns a
  fresh id. A task owns at most one live session; a second open is `Full`.
  `session_close` is owner-only and marks the session closed, discards both
  rings and wakes every waiter. Closing is idempotent for the owner; a foreign
  owner is `Invalid`; a stale id is `NotFound`.
- **S4 (MUST):** `session_send` copies `data` into the input ring and reports
  the copied count through `sent`. It is non-blocking: `WouldBlock` when the
  ring has no room and nothing was copied. A closed or finished session is
  `Closed`; a foreign or stale id follows S2/S3.
- **S5 (MUST):** `session_recv` copies up to the destination capacity from the
  output ring and reports the count through `read`. It is non-blocking:
  `WouldBlock` when the ring is empty. A finished or closed session whose ring
  is empty is `Closed`; the caller drains the ring first. A failure writes
  nothing through an out-parameter.
- **S6 (MUST):** Output-ring overflow is bounded. The newest bytes are kept and
  the oldest undelivered bytes are dropped with a counter; a terminal therefore
  shows the most recent screenful, never an arbitrarily old prefix.
- **S7 (MUST):** Every task has one session binding, inherited from its
  requester at `core.spawn`/`core.run`. A task bound to a session sends its
  `core.write` bytes to that session's output ring instead of a kernel terminal,
  and its `core.read_input`/source `0` reads that session's input ring. The
  bytes still mirror to COM1. A task with no session keeps the pre-M11 console
  behaviour: output is serial-only (no kernel terminal window) and input comes
  from the global serial ring.
- **S8 (MUST):** A session becomes **finished** when its last bound slave task
  terminates. A finished session keeps its output ring for draining; once empty,
  `session_recv` returns `Closed` and `session_source` is permanently ready.
  The owner then closes the session. An owner that terminates closes its
  session; bound slaves fall back to serial on their next write.
- **S9 (MUST):** `core.session_source(id)` is a `core.next_any`/`core.poll_any`
  source that is ready when the output ring has bytes or the session is
  finished/closed. It is the only wait a terminal emulator needs for output.
  The source value uses `abi::wait::SOURCE_SESSION_BASE` (`1 << 62`) and cannot
  alias a stream handle, console source or socket source.
- **S10 (MUST):** Serial input is routed to the focused window's task session
  when that task has one; otherwise it enters the global input ring. Keyboard
  input stays a window-addressed event (spec 0010, I34): the terminal emulator
  converts a `KeyDown` into bytes and calls `session_send`. The two paths never
  double a byte.

### The terminal emulator

- **T1 (MUST):** `apps/utilities/terminal.ig` is an ordinary GUI application.
  It opens its own window, opens a session, spawns `base/shell` into it and
  pumps window events and the session output source.
- **T2 (MUST):** The emulator owns one fixed **screen grid**, derived from its
  window's content rectangle and capped by a static maximum. It has no
  scrollback: a full row wraps, a newline at the last row scrolls the grid one
  line and the oldest line is discarded. Output beyond the screen is not
  buffered.
- **T3 (MUST):** The emulator interprets at least CR, LF, BS, TAB and printable
  ASCII. It ignores other control bytes and leaves non-ASCII bytes blank.
  ANSI escape parsing is out of scope. It paints with `syslib` inside its own
  content rectangle, re-reads the content rectangle on every owner `Expose`,
  and draws a cursor marker.
- **T4 (MUST):** A window close request closes the emulator: it closes its
  session, kills its shell task, closes its window and exits. The system stays
  alive. If the shell exits first, the emulator drains the remaining output,
  closes the window and exits.
- **T5 (MUST):** Every `Apps` Terminal activation spawns a new emulator. There
  is no raise-the-existing-terminal path. Each instance has its own session,
  shell, working directory, grid and cursor.
- **T6 (MUST):** New emulator windows cascade from the first at `(24, 64)` by a
  fixed offset, measured from the live windows titled `Terminal`, so several
  instances do not hide each other exactly.
- **T7 (MUST):** If `session_open`, `window_open` or `core.spawn` fails, the
  emulator reports the kernel status on serial and exits without leaving a
  window or a session behind.

### The shell

- **H1 (MUST):** `base/shell` is a session shell: it does not open a window, it
  does not call `terminal_attach` and it does not process window events. It
  reads bytes from source `0` (`core.read_input`) and writes with `core.write`.
- **H2 (MUST):** In a session, the prompt, echo, line editing, `cd`, builtins
  and `core.run`/`core.spawn` are unchanged; a child launched by the shell
  inherits the session, so its output appears in the same emulator.
- **H3 (MUST):** With no session (headless boot, `langhost`), the shell keeps
  the serial path: source `0` is the global serial ring and output is mirrored
  to COM1. It does not draw a window it does not own.
- **H4 (MUST):** The `terminal` shell command proves the session surface: it
  reports its session id, checks a stale id is `NotFound`, checks a foreign id
  is `Invalid`, spawns `system/terminal_probe` (which opens its own session and
  must succeed without disturbing the shell's) and prints `terminal: ok`.
- **H5 (MUST):** The startup demo child is removed: a session shell opens at a
  clean prompt (`igis:/> `) with no `shell: spawning child` transcript.

### System Log and boot

- **B1 (MUST):** `console::init` opens exactly one kernel text window, the
  System Log, owned by task `0`. There is no kernel Terminal window.
- **B2 (MUST):** `println!`/`print!` continue to feed the System Log and COM1.
  `core.write` never feeds the System Log.
- **B3 (MUST):** The boot entry is still `system/entrypoint` (the desktop). The
  desktop launches `apps/utilities/terminal` from the Apps menu. A graphics-less
  boot still falls back to `base/shell` on serial.

### Menu and image tree

- **M1 (MUST):** The image gains a top-level `apps` directory. Launchable GUI
  applications are staged under `apps/<category>/<name>`; categories are part
  of the image path, not only menu labels.
- **M2 (MUST):** The Apps pull-down has non-selectable section-header rows.
  Headers, separators and disabled rows are never selected, highlighted or
  activated by pointer or key traversal.
- **M3 (MUST):** The direct `Terminal` menu-bar item is removed. Terminal is an
  Apps item under `Utilities`; `Windows` still raises the topmost `Terminal`.
- **M4 (MUST):** `File` no longer depends on a kernel terminal: its window list
  is shown in a desktop-owned window.

## Design

### Kernel session table

```text
Session
  id: u64                 // 0 = free, monotonic, never reused
  owner: u64              // the emulator task
  bound: usize            // live slave tasks
  closed: bool
  finished: bool
  input: Ring<IN_BYTES>   // emulator -> shell
  output: Ring<OUT_BYTES> // shell -> emulator
```

`IN_BYTES` is 512 and `OUT_BYTES` is 4096. The table is
`[SpinLock<Option<Session>>; MAX_TASKS]`, `const`-initialised in `.bss`. At
64 slots the rings cost about 288 KiB, in line with the existing AP stacks.
Output pushes overwrite the oldest undelivered byte when full and count the
drop; input pushes refuse the newest byte and count it.

`console::write_bytes(session, bytes)` and `console::read_input(session, dst)`
route by the task's binding. The session lock is never held while taking the
window or console locks; a session push drops its lock before waking waiters.

`Task` gains `session: u64`. `Task::place` starts it at 0; the REPL loader sets
the child's field to the requester's session, and `Task::reclaim` calls
`session::task_ended(id, session)` so a slave decrements `bound` and an owner
closes its session.

### Source tag

`abi::wait` adds:

```text
SOURCE_SESSION_BASE = 1 << 62
session_source(id) = SOURCE_SESSION_BASE | id
is_session_source(source)
session_id(source)
```

A stream handle is far below `1 << 62`, and a socket source sets bit 63, so the
three encodings cannot alias. `task::source_ready` checks a session source
before the stream fallback, and the interpreter's `poll_any`/`next_any` probe
calls `Host::session_readable`.

### Routing

```text
terminal emulator (owner)
  session_send  -> input ring  -> shell core.read_input / source 0
  session_recv  <- output ring <- shell core.write / child core.write
  session_source in next_any   <- wake on output push, finish or close

serial byte -> focused window owner -> its session input ring
            -> global ring when the owner has no session
```

The terminal emulator does not read the global serial ring; the kernel routes
serial bytes to the focused session. Keyboard events are converted by the
emulator and sent through `session_send`.

## Interfaces

| Primitive | Result |
|-----------|--------|
| `core.session_open(var id)` | `Ok` with a fresh positive id; `Full` when the table or the owner's one session is used; `Invalid` for a host without sessions |
| `core.session_close(id)` | `Ok` for the owner (idempotent); `NotFound` stale; `Invalid` foreign |
| `core.session_send(id, data, var sent)` | `Ok` with the copied count; `WouldBlock` when full (nothing copied); `Closed` after finish/close; `NotFound`/`Invalid` per S2/S3 |
| `core.session_recv(id, var dst, var read)` | `Ok` with the copied count; `WouldBlock` when empty; `Closed` when finished/closed and empty; `NotFound`/`Invalid` per S2/S3 |
| `core.session_source(id)` | `SOURCE_SESSION_BASE + id`; readiness is output data or finished/closed |

Host methods:

```text
fn session_open(&mut self) -> Result<u64, Status>;
fn session_close(&mut self, id: u64) -> Status;
fn session_send(&mut self, id: u64, data: &[u8]) -> Result<usize, Status>;
fn session_recv(&mut self, id: u64, dst: &mut [u8]) -> Result<usize, Status>;
fn session_readable(&mut self, id: u64) -> bool;
```

All default to a disabled service (`Invalid`/`false`) so `langhost` and a bare
mock stay deterministic.

## Failure modes

| Situation | Result |
|-----------|--------|
| No free session slot | `session_open` `Full`; the emulator reports and exits |
| Emulator window cannot open | emulator closes the session and exits |
| Shell cannot be spawned | emulator closes the session and window and exits |
| Emulator window closed | session closed, shell killed, emulator exits |
| Shell exits | session finished; emulator drains output, closes, exits |
| Owner task terminates unexpectedly | session closes; bound slaves fall back to serial |
| Output ring full | oldest undelivered bytes dropped and counted; recent screenful survives |
| Input ring full | newest byte dropped and counted |
| Stale session id | `NotFound`; no out-parameter is written |

## Open questions

- ANSI/VT100 escape parsing and scrollback are future work; neither is required
  for the shell.
- Blocking `core.write` backpressure is not introduced: the output ring is
  bounded and drops the oldest bytes.

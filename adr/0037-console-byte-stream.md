# 0037: The console is a raw byte stream; the application owns echo and line editing

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 kernel/language
- **Related specs:** 0003 ("Console input", "Multi-source waits"), 0005 ("Console input: `core.read_input`"), 0007 ("Blocking and wake-up"), 0008 (opcode table), 0010 (event queue), 0012/0014 (console ownership)

## Context

Since 2026-11-13 the kernel kept a staged line discipline as a temporary
shortcut: the service opportunity polled COM1 and the PS/2 keyboard into one
byte FIFO, echoed printable bytes into the terminal window, interpreted
Backspace, Ctrl+C and Enter, completed lines into a four-line ring, and
`core.read_line` blocked until one was ready. Q44 recorded it as temporary
because the language could not yet edit a buffer.

That reason is gone. The M2.4.1 `stdlib.str_*` helpers insert, delete, backspace
and move a cursor over a `string[256]` atomically and code-point-safely, the
terminal window has been running the shell in a task-owned window since M2.5.5,
and M2.5/M2.6 are complete. The staged discipline now only duplicates what an
application can do, hides input from the event layer, and makes the kernel
responsible for a policy (echo, line boundaries, truncation) that belongs to the
application.

Two further problems came with the staged design:

1. **Two consumers of one key.** A key byte was delivered into the line FIFO
   *and* as a key event. A shell that reads both would see every key twice; the
   event slice could not use the console path without duplication.
2. **A blocking primitive that is not in the wait model.** `core.read_line`
   blocked on `WaitFor::Console`, a single-source wait outside the Q39 set, and
   Q39 source `0` was defined as "one completed line". Once line assembly moves
   out, there is no completed line to wait for.

## Decision

The console is a **raw byte stream**, and the kernel does not echo, edit,
assemble or interpret input.

- **Ring.** The service opportunity drains COM1 into a fixed 512-byte ring. A
  byte is stored verbatim. When the ring is full the **newest** byte is dropped
  and counted; the oldest 512 bytes survive, the sequence never rewinds, and the
  drop is invisible because the kernel does not echo. The boot self-check
  `console: input ok` pins FIFO order, partial pops, empty reads and this
  overflow policy on a scratch ring.
- **Device split.** Serial is the byte stream; the PS/2 keyboard is events.
  Keyboard bytes go only to the machine-wide event queue (spec 0010), so a key
  is delivered exactly once. A shell that selects on both sources sees a serial
  byte in one arm and a key event (with its text byte) in the other; there is no
  duplication.
- **Read primitive.** `core.read_input(dst: var bytes[N]) -> int` copies the
  bytes that are waiting, publishes the length and returns the count (`0` when
  empty). It never blocks. `bytes` is required: a partial read could split a UTF-8
  code point, so a `string` destination is a compile error and conversion uses
  the checked `core.substr_into` (T3).
- **The opcode slot is reused.** `READ_INPUT` occupies the old `READ_LINE`
  value (`0x42`). The encoding is unchanged — a destination reference on the
  stack plus a capacity immediate, a copied count pushed, the length word
  published — so ABI v1 stays at **75** opcodes. What changes is the contract:
  no block, no line, no truncation of an over-long line (the caller decides how
  much to read), and a `bytes` destination. Reusing the slot is safe because
  bytecode is internal, regenerated from source at every load (ADR 0019), and
  there is no external consumer to keep compatible.
- **Q39 source `0` becomes "a console byte is available".** The probe is
  `Host::console_byte_ready` (`console::has_byte`); the block is the ordinary
  `WaitFor::Any` set and `task::wake_console_input` wakes matching sets. A
  non-zero return from `poll_any`/`next_any` is a snapshot; the following
  `core.read_input` is authoritative. `WaitKind::Console` and
  `WaitFor::Console` are deleted: nothing in the language blocks on the console
  directly.
- **The shell owns the line.** `samples/apps/shell` keeps `string[256] line` and
  an `int cursor`, selects on source `0` and source `1`, and applies one byte at
  a time through one editor:
  - printable ASCII (`0x20..=0x7E`) inserts at the cursor via
    `stdlib.str_insert_into` and is echoed with `core.write`;
  - Backspace / `0x7F` deletes the last code point via `stdlib.str_backspace`
    and echoes `\x08 \x08`;
  - `\r`/`\n` (or the Enter key) submits and echoes `\n`;
  - Ctrl+C (or the key event whose text is `0x03`) clears the line and echoes
    `^C\n`;
  - Tab inserts one space; the shell does not expand tab stops.
  A PS/2 key event's text byte goes through the same path, so the keyboard edits
  the same line. Other keys (arrows, Home/End, Delete, function keys) are
  ignored; there is no history and no ANSI handling. If the terminal window is
  not attached, the echo reaches the serial mirror and the shell still works.
- **Other users migrate.** `multiplex` reads its console arm with
  `core.read_input`; `kill_victim line` blocks in `core.next_any` on source `0`;
  `langhost` drains a scripted byte queue (including newlines) so the same
  application-side editing runs on the host.

## Consequences

- One read path, one wake model, one owner of input policy. The kernel keeps no
  partial line, no echo buffer and no control-byte semantics.
- The shell's echo is visible output, so a transcript contains the typed text.
  This is what a real terminal does; it also means the shell must track what it
  echoed (it only ever appends, so one cell per printable byte, and it never
  erases the prompt).
- Editing is append-only at the end of the line. Cursor movement, history and
  word editing are application features that do not exist yet; adding them
  requires erasing and rewriting the visible tail, and the terminal has no ANSI
  support, so a line that wraps past the window width is edited in memory but
  its display can be imperfect. This is recorded as an M2 limitation, not a
  kernel defect.
- A burst larger than the ring loses its newest bytes. The kernel counts them;
  the shell cannot recover them. 512 bytes is far larger than a typed line or a
  harness command.
- `langhost` no longer has a per-call line stub; a program that reads after its
  script is exhausted gets `0` and then blocks if it selects, which reports a
  host-side `IllegalOperation`. No host test depends on the old endless empty
  line.
- ABI v1 stays at 75 opcodes; `stdlib` stays at 125 functions; no cap changes.

## Alternatives considered

- **Keep `core.read_line` and add a separate byte primitive.** Rejected: two
  read paths, two block/wake stories, and a second opcode or builtin for a
  buffer write. The staged path would keep duplicating the shell's job.
- **Retire the opcode slot and add builtin 53.** Coherent but worse: a hole in
  the opcode table, a new builtin for the same stack shape, and a larger
  surface. A documented semantic change within ABI v1 is smaller and honest.
- **Make `core.read_line` non-blocking but keep the name.** Rejected: a name
  that promises a line while returning raw bytes is the kind of silent change
  the repository forbids. The primitive is renamed and the old name is gone.
- **Feed PS/2 bytes into the ring and the event queue.** Rejected: the shell
  would see every typed key twice, and an event-reading application would race
  a console reader for the same byte.
- **Let the kernel keep echoing but not assemble lines.** Rejected: echo is
  output policy and must be ordered with the application's own writes; a kernel
  echo would race the prompt and make a detached terminal behave differently.
- **Drop the oldest byte on overflow.** Rejected: it silently loses the start
  of a command the user already saw typed. Dropping the newest keeps the
  delivered prefix consistent with what was echoed.

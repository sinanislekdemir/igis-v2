# AGENTS.md

Operational guide for humans and coding agents working in this repository.
Keep this file short and current. It points at documents; it does not duplicate them.

**Last updated:** 2026-11-17

## What this is

`igis-v2` is a from-scratch second version of a personal operating system whose first
version was written in 2005 as a front-end for a quantum-cryptography modem. It began as
a Linux driver, the user/kernel ring boundary got in the way, and it grew into a kernel
with its own boot path. No code survives; the continuity is design intent and the
constraints learned there.

- **Primary language:** Rust (`no_std`, freestanding).
- **First architecture:** x86_64. All ISA-specific code is isolated so additional
  architectures can be added without touching the kernel core (ADR 0013).
- **Central thesis:** the kernel is a programmable system — a language runtime plus
  standard library owning the machine. Applications are source code, not native
  binaries, compiled in the kernel at load and never precompiled (ADR 0019). Everything
  runs at ring 0 in one address space.
- **Execution model:** one interpreter loop per core; tasks are claimed from a global
  runnable set and are never pinned to a core (ADR 0011).
- **Long-term goal:** a general personal system. The first milestone proves the thesis
  with a booting kernel, source applications, read-only ext2 delivery, and a REPL that
  can drive a device.
- **Status:** implementation. Specs 0001–0011 are accepted. The kernel boots under
  GRUB/multiboot2 into long mode, prints through a framebuffer console mirrored to serial,
  has a GDT/TSS and an exception path with a double-fault IST, runs a 1 kHz APIC timer
  driving a global nanosecond clock, and owns a bitmap frame allocator. The full vertical
  slice now runs: `xtask image` builds a 4 MiB ext2 image from `samples/apps/` and attaches
  it as a GRUB module, the kernel mounts it read-only, reads `shell` (falling back to
  `entrypoint`), compiles the source in the kernel with `compiler::compile_into`, places
  `code`/`state`/`stack` in contiguous frames, verifies the package and enqueues the task
  in a 64-slot global runnable set. The boot core then runs the scheduler loop: claim by
  value move under a per-slot spin lock, a 10 000-bytecode quantum, per-task CPU accounting
  with a 10 s / 200 000 000-step watchdog, and release or deterministic termination. The
  boot application crosses many quanta, prints its output, exits `Ok`, and the system
  prints the claim/yield/exit summary and halts. The compiler is `no_std` and
  allocation-free; the scheduler path adds no heap. The REPL request service now runs: a
  1024-slot static queue is serviced before every claim, `core.spawn` reserves a global
  task id, `core.wait` blocks the parent (a `Blocked` slot skipped by `claim`) until the
  child's exit record wakes it, and a 64-entry exit ring serves `wait`/`run`. These
  primitives use the predeclared `Status`/`Diagnostic`/`ExitReason` shapes (spec 0005,
  "Implemented ABI"). Console input now runs too: COM1
  and the PS/2 keyboard are polled in the service opportunity, the kernel stages
  echo/line assembly, and `core.read_line` blocks the shell until a line is ready (Q44;
  spec 0005). Buffers can be indexed and compared: `buf[i]` reads a raw `byte` (checked
  against the runtime length), `bytes[i] := b` writes one byte (checked against the
  capacity, extending the length and zero-filling the gap), `=`/`<>` compare two buffers
  of the same kind across capacities, and `core.len` returns the runtime length. The shell
  is now a real command loop (`help`, `echo <text>`, `run <app> [args]`, `exit`): `core.find`
  locates the first space, `core.substr_into` splits the line and the `run` tail, and the
  child prints the argument it received, so `run child world` proves argument carriage. The
  host `core.read_line` stub scripts those four commands so `langhost` runs the same shell
  deterministically. Integer formatting now exists as the staged `core.format_int_into`
  (decimal/hex, sign, zero padding, exact-fit `-6`), and the first `syslib` primitives —
  `port_in8`/`port_out8` with `Port` staged as a scalar `uint` — let an application drive
  hardware: `samples/apps/clock` polls the CMOS RTC update flag, reads seconds/minutes/hours
  and prints `HH:MM:SS` (`run clock` in the shell). Compilation is now bounded and runs on a
  guarded stack: the parser and the recursive codegen walks reject deeper than 32 counted
  levels with a `Diagnostic`, and after early boot `kmain` switches from the `.boot` stack
  to a 256 KiB stack with a not-present guard frame below it (the guard's 2 MiB identity
  page is split to 4 KiB PTEs; `#PF` uses its own IST). A stack overflow now faults cleanly
  at the guard instead of writing into the boot page tables (Q42). Enums are implemented
  (`type Name = (Variant, ...)`, nominal, one 8-byte ordinal, checked `EnumName(int)`);
  `Status` and `FaultCode` are predeclared; records are implemented
  (`type Name = record field: T; ... end;`: nominal, declaration-order 8-byte-aligned
  layout, nested records, buffer fields, whole-record copies, `value`/`const`/`var` record
  parameters). Sum types and pattern matching are implemented
  (`type Name = Variant | Variant(payload: T);`: nominal, 8-byte tag at 0 plus the largest
  payload at 8, destination-driven construction, exhaustive `case` with an arm-local
  by-value payload binding). `Diagnostic` is a predeclared record and `ExitReason` is the
  predeclared sum type `Ok | Killed | Faulted(code: FaultCode)` at the fixed `abi::layout`
  offsets: `core.spawn(path, args, var diag, var id)` fills `diag` on a synchronous
  failure, `core.wait(id, var reason)` writes the reason's tag/payload, and the shell
  prints `diag.message` and reports the reason with a `case`. `core.run(path, args,
  var diag, var reason)` now composes spawn + wait with the pending id in the machine, and
  the request-completion path carries the compiler's `Diagnostic` in the exit ring, so a
  compile error the service finds after `spawn` returned reaches `run`'s `diag` (`wait`
  still sees `Faulted(IoError)`). The `stdlib` is now real source: `stdlib/stdlib.ig` is
  embedded with `include_str!` and compiled into the same package as any application that
  says `uses stdlib;`, with library functions first in one function table, a shared
  constant pool, and line-table unit ids (0 app, 1 stdlib) so a broken library is a normal
  diagnostic naming `stdlib`. `function Name(...): T;` with `return` works for scalar
  results. The first helpers are `trim_into`, `split_once`, `parse_int_into`,
  `starts_with` and `contains_byte`; `samples/apps/text` exercises them on `main(args)`.
  Fixed-size arrays are implemented: `array[T, N]` (N a positive const) in state, locals,
  records, sum payloads and `value`/`const`/`var` parameters, with nominal
  element/count identity, `a[i]` read/write checked `index < N`
  (`fault(OutOfBounds)`), whole-array `copy_refs`, nested `a[i][j]`/`a[i].field`, and one
  new opcode `ref_element` (ABI v1 61 → 62, immediate `stride` and `count`). Layout is `N`
  elements at `align8(element size)`, array size `align8(N * stride)`; one structural
  array table is shared by the app and `stdlib`, so `stdlib.split_all` takes an
  `array[string[64], 8]` parameter. `samples/lang/array.ig` exercises the whole slice.
  Fixed-capacity rings are implemented too: `ring[T, N]` (N a positive const) in state,
  locals, records, sum payloads, arrays and `value`/`const`/`var` parameters, nominal in
  element and count. Layout is the `N` elements at `align8(element size)` followed by
  `head: u64` and `count: u64`; the built-in `core.ring_push`/`pop`/`peek`/`full`/`empty`
  are compiler-lowered over `ref_offset`/`ref_element`/`copy_refs`, `add_u`/`mod_u` and
  `eq`+`jz`, so no opcode was added (the ABI v1 opcode table is unchanged). `push` returns `Full` on a full
  ring; `pop`/`peek` return `NotFound` and leave the out-parameter untouched on an empty
  one; `push`/`pop` reject `const` receivers (a `value` copy is writable and private to
  the callee). One structural ring table is
  shared with `stdlib`, whose `drain_into` takes a `ring[int, 8]`; `samples/lang/ring.ig`
  covers FIFO order, wrap-around, copies and every element kind. Fixed-capacity maps are
  implemented too: `map[K, V, N]` (`N` a positive const, `K` one of `int`, `string[M]`,
  `bytes[M]`) is nominal in `(K, V, N)`, lays out a `count` header then `capacity` (the
  next power of two at least `2N`) slots of state/hash/key/value, and hashes with FNV-1a,
  probes linearly and deletes with backward shift. `core.map_put`/`get`/`has`/`remove`/
  `count` are five new opcodes (`MAP_PUT` … `MAP_COUNT`) so the algorithm lives once in
  the interpreter; `put` at `N` entries returns `Full`, a missing key is `NotFound`, and
  `get` leaves its out-parameter untouched. One structural map table is shared with
  `stdlib`, whose `map_get_or` takes a concrete map; `samples/lang/map.ig` covers
  collisions, backward shift, key 0, the empty buffer key, aggregate values, copies and
  the three parameter modes. UTF-8 is implemented: `core.substr_into` now accepts all four
  destination/source kind combinations, with the destination kind deciding the check —
  a `string` source keeps T4's code-point-boundary rule, a `bytes` source is validated as
  strict UTF-8 (no overlongs, surrogates, values above `U+10FFFF`, truncations or stray
  continuation bytes), and a `bytes` destination copies raw data. The source kind rides in
  the `SUBSTR` `b` field, so no opcode was added. `stdlib` gained `utf8_decode_into`
  (T3, the only checked bytes→string conversion), `utf8_len`, `utf8_decode` and
  `utf8_valid` as ordinary language code; `samples/lang/utf8.ig` covers every code-point
  length and malformed class and `run utf8` runs in the kernel. The checked slice stays in
  `core` permanently: the destination capacity must be a call-site immediate, and without
  generics a `stdlib` helper could only serve one capacity (the `stdlib.substr_into` move
  is withdrawn). Every online core now runs the scheduler: `smp::start()` brings the APs
  up with INIT-SIPI-SIPI through a trampoline copied to physical `0x8000` beside a
  parameter page at `0x9000`, one per-core TSS/IST set, one static 256 KiB kernel stack per
  AP (matching the boot core's guarded 256 KiB one, because any core may compile a
  request), and a bounded 100 ms wait, so `-smp 1`
  and `-smp 2` boot with fewer cores instead of hanging. Discovery is by attempt over APIC
  ids `1..MAX_CORES` with `MAX_CORES` = 8; the core index is internal and the APIC id is
  the identity software sees through `syslib.cpu_id()` (builtin 10). All cores run the
  same claim → quantum → release loop over the global runnable set, with per-core verifier
  scratch and per-core claim/yield/exit counters. An idle core does one `sti; hlt` step
  after checking the set; producers (`task::spawn`, `release`, `wake`, the REPL enqueue)
  set `need_resched` on one idle core and send the `0x40` IPI, whose handler only EOIs. A
  claimed task's slot stays reserved by task id and a reserved request id stays live while
  it compiles, so `core.wait` cannot race a spawn or a compile. The boot core's idle path
  prints the per-core summary, sets a stop flag, IPIs the APs, waits bounded for their
  acknowledgements and halts. `samples/apps/worker` and `cores` show six workers on
  distinct APIC ids in a `-smp 4` run; `xtask run` passes `-smp 4` by default (`--cores N`).
  The realtime class runs: a task promotes itself once with `core.realtime_on()` (builtin
  11; `Ok`, or `Full` when every online core already holds one), is claimed before normal
  tasks, is not released at quantum expiry, and is exempt from the total budget. It must
  call `core.heartbeat()` (builtin 12) at least once per second of non-blocked time; the
  deadline is `budget_ns + 1 s` (a compile-time constant) and a miss terminates it at the
  next quantum boundary with `Faulted(WatchdogTimeout)`. Admission is a CAS counter
  against `smp::online_count()`; a task releases its slot only at termination, so a blocked
  controller keeps it. Starvation is accepted (no reserved core). `samples/apps/realtime`
  promotes, beats through 200 control cycles and runs a `beat` child that reports `Full`
  on `-smp 1`; `samples/apps/spin` loops without a heartbeat and is killed after about a
  second. Streams now run: `in_stream[N]`/`out_stream[N]` are top-level state variables
  whose names are the stream identifiers, the compiler puts their 8-byte handles in the
  `state` prefix and lists the endpoints in the manifest, and the loader creates or pairs
  each endpoint before placing the task. A new endpoint whose peer is not live *waits*:
  a sender may fill it to capacity and a receiver blocks on it. A same-direction
  duplicate, a capacity mismatch against a live peer, and a declaration against a closed
  stream are load errors reported through `core.run`'s `diag`. `core.send`/`try_send`
  copy a whole buffer atomically (blocking, `WouldBlock`, `CapacityExceeded`, `Closed`);
  `core.recv`/`try_recv` fill a writable `bytes[N]` and return the count or a staged
  `-1`/`-2` (`string` destinations are rejected because a partial read could split a code
  point). Blocking uses `WaitFor::Stream(handle)`, a send wakes a receiver, a receive that
  frees space wakes a sender, and termination closes the owner's sides, discards queued
  bytes and frees the buffer at the close (the record stays until the surviving endpoint
  ends). `samples/apps/streams` (consumer
  `pipe_in`, producer `pipe_out`) proves in-order delivery through a blocking sender and
  the `Closed` exit; `streams_bad` proves both load errors. The `stdlib` framing layer
  now sits on top, as ordinary language code taking `const in_stream[1]`/`const
  out_stream[1]` parameters (a stream parameter's declared capacity is not checked, so one
  helper serves every capacity): `recv_exact`, `send_frame`/`recv_frame_into` (a `u32`
  little-endian length prefix), `send_line`/`recv_line_into` (`\n`, CRLF stripped, an
  overlong line keeps consuming to stay in sync), `send_u64`/`recv_u64` and `send_bytes`.
  Each helper publishes a whole value with one copy, so an error leaves the destination
  untouched; a receive is one byte wide because `core.recv` returns up to the destination
  capacity and there is no pushback. `pipe_out`/`pipe_in` now run through `send_line`/
  `recv_line_into`, and `samples/apps/frames` (`frame_in` + `frame_out`, with an ack
  stream so the producer exits only after the consumer drained) proves the length prefix,
  the typed `u64` view, both `CapacityExceeded` paths and the close. Every core's kernel
  stack is now 256 KiB: a compile on an AP overflowed the old 64 KiB stack into the
  neighbouring `.bss`. Stack guards are now complete (`kernel/src/guard.rs` over
  `arch/x86_64/src/paging.rs`): the boot stack, all seven AP `.bss` stacks and every task
  stack have a not-present 4 KiB frame. A cache of 32 4 KiB tables splits each 2 MiB page
  once and keeps the neighbouring pages present, so several guards share one table; a
  96-entry registry lets `#PF` print `stack guard: task N` / `ap core N` / `boot core`.
  Every PTE change is followed by a local CR3 reload and an IPI TLB shootdown to all online
  cores, because a frame may carry a stale translation from an earlier life on another
  core. A task stack is `stack_len + 4096` contiguous frames: the lowest frame is the
  guard, `regions.stack` starts one frame above it with exactly the manifest length, so the
  interpreter's `StackOverflow` check is unchanged; `reclaim` restores the guard and
  returns guard + stack as one unit on every destruction path. `samples/apps/deep` proves
  `run deep` reports `child: faulted StackOverflow`; a hardware guard hit is a hard backstop
  that halts with the guard named. Fixed-point float text now runs:
  `core.format_f64_into(dst, value, frac_digits) -> int` (9 fraction digits at most,
  half-away-from-zero rounding with carry, `nan`/`inf`/`-inf` specials, `-4` for a wider
  field or a magnitude at or above `2^63`, `-6` when the result does not fit) is one new
  `FORMAT_F64` opcode implemented as a pure buffer operation in the interpreter, and
  `stdlib.parse_f64_into(text, var value) -> Status` is ordinary language code accepting
  decimal text with an optional exponent plus the formatter's `inf`/`-inf`/`nan`, rejecting
  everything else with `Invalid` and saturating overflow/underflow to `±Inf`/`±0` with
  `Ok` while preserving `-0.0`. `samples/lang/f64text.ig` covers the whole surface on the
  host and `samples/apps/floats` proves `run floats 3.25` → `value=3.250` and
  `run floats nope` → `Invalid` in the kernel. Shortest round-trip float text now runs
  too: `core.format_f64_short_into(dst, value) -> int` writes the fewest significant
  digits that parse back to the same bits — positional while `-6 < e10 + 1 <= 21`,
  scientific otherwise (`1e20` vs `1e21`, `1e-6` vs `1e-7`), `0`/`-0`, `nan`, `inf`/`-inf`
  literal, an exact decimal tie away from zero, `-6` when the destination is too small —
  as one `FORMAT_F64_SHORT` opcode. It is an exact search, not an approximation: the value
  and both ends of its round-trip interval are expanded to base-`10^9` integers (at most
  769 digits, 88 limbs), the two `n`-digit decimals that bracket the value are tested in
  turn, and the first `n` that lands inside wins (17 digits always do). No allocation, no
  host call, no data tables beyond 72 bytes of `10^0..=10^8`. `samples/lang/f64short.ig`
  covers the shape boundaries and the subnormal tail on the host, and `samples/apps/floats`
  prints `s:1e21=1e21` and `s:tinysub=5e-324` in the kernel. The desktop milestone
  (M2) has started with its graphics foundation: the kernel keeps the multiboot2 mode in
  `kernel/src/gfx.rs` and allocates a full-screen **shadow screen** from the frame
  allocator (`pitch * height` bytes, 3 MiB / 768 frames at the 1024×768×32 mode GRUB
  returns) instead of drawing into the framebuffer directly. The shadow is the single
  source of truth; the text console paints its glyphs through it (`console_glyph` presents
  each cell) and `syslib` exposes nine primitives — `screen_width`/`screen_height`,
  `fill_rect`, `fill_dither` (50% `(X + Y)` checkerboard), `draw_rect` (1-px outline),
  `draw_text` (8×8 font, transparent background, no newline), `get_pixel`,
  `present` (pitch-aware row copy) and `clear` — as host builtins with `Status` results
  (`Invalid` for `w <= 0`, `h <= 0` or a colour above `0x00FFFFFF`; drawing is clipped,
  never faulted). A rectangular clip region (currently the whole screen) is the seam for
  the window service. `samples/apps/desktop` paints a System 7.5-style dither desktop, a
  menu bar with a right-aligned `core.clock()` HH:MM and a striped title-bar window, then
  checks nine pixels with `get_pixel` and prints `desktop: ok` in one claim. Graphics
  disable cleanly (no tag, non-32-bpp, bad geometry or no shadow memory: 0×0 screen,
  `Invalid`, console on serial). Spec 0009 and ADR 0020 record the decision; M2.2 is the
  mouse and events, M2.3 windows and `expose`. The desktop pointer now runs too: the
  i8042 auxiliary device is initialised at boot (`0xA8`, config aux interrupt cleared,
  `0xF6`, `0xF3 100`, `0xE8 3`, `0xE6`, `0xF4`; bounded waits) and polled in the service
  opportunity beside the keyboard, which now leaves aux bytes for it instead of
  discarding them. A pure 3-byte decoder requires bit 3 on the first byte (resync
  otherwise), drops an overflow packet whole and sign-extends the 9-bit deltas with the
  Y axis inverted; one machine-wide pointer state (`x`, `y`, buttons; clamped to the
  screen; 1:1 motion) lives in `kernel/src/gfx.rs`, and a boot self-check runs canned
  packets (including a negative delta, an overflow and a desynced stream) through the
  same decoder and clamp arithmetic on scratch state (`mouse: decoder ok`, halt on a
  mismatch). The 16×16 arrow (black outline, white fill, hotspot 0,0) is composited into
  the framebuffer by `present`, never stored in the shadow, so `get_pixel` stays
  uncomposited and headless checks stay deterministic; a move re-presents only the old
  and new cursor rectangles. `syslib` gained `pointer_x`/`pointer_y` (`int`),
  `pointer_buttons` (`uint`) and the debug-only `inject_pointer(dx, dy, buttons)`
  (builtins 22–25; host defaults 0/0/0 and `Invalid`), and `samples/apps/mouse` proves
  clamping at both edges, buttons and the "cursor is not in the shadow" rule (`mouse:
  ok`). Spec 0010 and ADR 0021 record the decision; M2.2.3 adds key events and M2.2.4 the
  event queue. The pointer is now **absolute** as well: `kernel/src/pci.rs` scans bus 0
  for the PIIX3 UHCI controller (`8086:7020` at 0:1.2, I/O BAR4), `kernel/src/usb.rs`
  resets the controller, installs a static 4 KiB frame list plus static QH/TDs and
  enumerates QEMU's `usb-tablet` (`GET_DESCRIPTOR`/`SET_ADDRESS`/`SET_CONFIGURATION`/
  `SET_IDLE`), verifying the HID report descriptor's absolute 16-bit X/Y shape. One
  self-linked interrupt-IN TD is polled from the service opportunity; the measured report
  is one buttons byte plus little-endian X and Y (`0..=32767`, top-left origin) and a
  wheel byte, scaled to `0..=w-1`/`0..=h-1` with round-to-nearest and applied through
  `gfx.set_pointer_absolute`. The tablet wins over PS/2 while ready; PS/2 motion is
  dropped (bytes still drained) and remains the fallback on a machine without the tablet.
  `syslib.pointer_source()` (builtin 26) reports 0 none / 1 PS/2 / 2 tablet. This fixes
  Q49's burst loss and host-window scale mismatch because an absolute report restates the
  position. `xtask run` passes `-usb -device usb-tablet` (`--no-tablet` omits the device);
  `scripts/tablet_qmp.py` drives QMP absolute injection and compares guest pixels. Spec
  0010 and ADR 0022 record the decision. The keyboard now
  produces events as well as console bytes (M2.2.3–M2.2.5): `arch/x86_64/src/keyboard.rs`
  decodes set 1 into a pure `KeyEvent` (stable key code, text, Shift/Ctrl/Alt/Caps,
  arrows, F1–F12, the E1 Pause sequence skipped whole) and a boot `keyboard: decoder ok`
  self-check; `kernel/src/event.rs` owns one 64-event machine-wide queue that drops the
  newest event on overflow and counts it, with pointer transitions enqueued from `gfx`.
  `EventKind` and `Event` (`kind, x, y, code, window`) are predeclared at
  `abi::layout::event`, key events pack `text | modifiers << 8 | key code << 16`
  (`abi::key`), and `syslib.next_event` (blocking on `WaitFor::Input`, SMP-safe wake),
  `poll_event`, `inject_event` (debug-only), `event_drops` and `event_flush` are builtins
  27–31 with no new opcode. `samples/apps/events` with `events_poke` proves the surface,
  the overflow policy and a genuinely blocked wake (`events: ok` on `-smp 4` and
  `-smp 1`). IRQ12 now drives the relative fallback too (M2.2.7): `arch/x86_64/src/ioapic.rs`
  routes IRQ12 through the I/O APIC at `0xFEC00000` (the page is split and marked
  `PWT | PCD`) to the boot core as a level-triggered vector `0x2C`; the `irq_aux` stub
  calls `mouse::on_irq`, which drains at most 64 aux bytes into a 1024-byte
  single-producer/single-consumer atomic ring and EOIs only the local APIC. `mouse::poll`
  decodes the ring in the service opportunity, and the direct controller drain remains
  only as the backstop when the route is unavailable; the two modes are exclusive, so an
  aux byte has one reader. `mouse::enable_irq` stops reporting, re-reads the
  configuration, re-enables reporting and writes `config | 0x02` last (once bit 1 is set
  the handler would steal the `0xF4` ACK); the configuration value is now written to the
  **data** port, fixing the old command-port write that left bit 1 clear. Boot lines
  `ioapic: version ...`, `mouse: aux IRQ12 enabled ...` and a one-time
  `mouse: aux IRQ12 delivered ...`; an absent IOAPIC/APIC or a failed write falls back to
  polling. `scripts/mouse_burst_qmp.py` clamps at x=0 and injects 300×(+2), requiring
  +600 on `-smp 4` and `-smp 1`; QEMU 10.2.2 retains PS/2 deltas, so the polled build is
  also 600/600 headlessly (the historical 346/600 was a host-window measurement). Spec
  0010 and ADR 0023 record the decision. Spec 0011 (Windows) is accepted and fixes the
  window model before implementation (ADR 0024): a 16-entry kernel table of records with
  monotonic `int` ids (`0` invalid); `window_open(x, y, w, h, title, var id)` takes the
  frame origin and the **content** size (frame = content + a 1-px border and a 16-px title
  bar, so `frame = (w + 2, h + 17)`); one bottom-to-top z-order with exactly one focus
  (open/raise focus, closing the focused window falls back to the topmost live one, task
  death closes its windows); owner-only `close`/`move` with `raise` allowed to any task;
  a 64-byte title, an 8 × 8 minimum content and an origin clamped on screen; a
  service-owned shadow clip with save/narrow/restore (no application primitive); and a
  whole-content `Expose` repair protocol for M2.3.2/M2.3.3. Geometry is read through the
  predeclared `Rect` record (`x, y, w, h: int`, size 32). M2.3.1 implements it:
  `kernel/src/window.rs` holds the 16-entry table behind one lock (`open` clamps and
  assigns monotonic ids, `close`/`move` check the owner, `raise` refocuses, `close_task`
  runs from `Task::reclaim`, the rect/focus reads are pure); `syslib.window_*` are host
  builtins 32–38 with disabled-service defaults; `gfx` gained the internal
  `set_clip`/`reset_clip` (no application primitive) with a boot self-check
  (`graphics: clip ok`) and a scratch-table self-check prints `window: rules ok`.
  `samples/apps/windows` (with `windows_child`) proves geometry, raise/focus, move,
  non-owner refusal, close-on-exit, focus fallback and the error paths (`windows: ok` on
  `-smp 4` and `-smp 1`). M2.3.2 implements composition and repair: every mutation records
  bounded damage (32-rect desktop and chrome lists) and one synchronous repair under the
  table lock (window → graphics): the desktop returns as white + the `(X + Y)` black dither
  in the parts no live window covers, chrome is redrawn bottom-to-top in each window's
  visible parts (frame minus higher frames) intersected with the damage, and the union is
  presented once. The service draws the chrome (1-px black border, 16-px title bar — white
  with black stripes when focused, plain `0xCCCCCC` when not — an 11×11 close box and a
  centred 8×8 title) and never writes the content rectangle. `window: paint ok` checks the
  damage/union arithmetic on scratch state. `samples/apps/windows_demo` composes three
  overlapping windows, moves and raises them, holds 15 s as a realtime task and prints
  `windows_demo: ok`; `scripts/windows_screenshot_qmp.py` captures the frame via QMP
  `screendump`. M2.3.3 implements routing and `Expose`: producers leave `Event.window = 0`,
  and the kernel host routes at poll time — a pending owner-only `Expose` first
  (`EventKind.Expose`, ordinal 5, `window = id`, `x = y = code = 0`, one coalesced flag per
  damaged window), then a queued event, where a `PointerDown` hit-tests the topmost frame,
  raises/focuses it and names it, other pointer/key events name the focused window, and an
  injected non-zero `window` is passed through. Routing never holds the graphics lock
  before the window lock (the pointer producer runs under graphics and only enqueues); a
  repair marks exposes and the wrappers wake `WaitFor::Input` after dropping the window
  lock, with the scheduler's input-block re-check testing `window::has_expose`.
  `syslib.window_present` (builtin 39) presents a window's content clipped to its visible
  parts (content minus higher frames), owner-only; `syslib.present` stays a generic
  primitive. `window: route ok` checks the bookkeeping and `samples/apps/windows_events`
  proves routing, expose fields, foreign isolation, `window_present` and
  close-on-termination on `-smp 4` and `-smp 1`. A client that ignores an expose leaves
  stale content and is not closed in M2. M2.3.4 separates the console and adds the first
  shell (spec 0012, ADR 0025): `console::init` opens two kernel-owned text windows
  (owner `0`) after `gfx::init` — a **terminal** for `core.write` and the line-discipline
  echo and a **system log** for `println!` — each with its own 62×89 (mode-derived) grid,
  cursor and per-window scroll, drawn at scale 1 inside the window's content and visible
  parts with an **explicit clip** (`gfx::draw_cell`/`present_within`), never the shadow's
  global clip, and presented cell by cell or part by part. `console::route_self_check`
  writes one probe through each real path, checks both grids and a foreground/background
  pixel in each with `get_pixel`, clears them and leaves `console: route ok`; every byte
  is still mirrored to serial and `core.read_line` is unchanged. A pending owner-0
  `Expose` is pulled by `console::pump` at the end of `console::poll_input` (service
  opportunity and idle loop), so the lock order stays `CONSOLE → WINDOW → SCREEN` and the
  repair never calls the console. `window::open` now admits owner `0` (task ids start at
  1) and kernel windows are ordinary windows: clickable, raiseable and enumerated. A
  boot-time `console: terminal ok` / `console: log ok` pair precedes the result line.
  `syslib.window_info(index, var id, var rect, var title[64])` is builtin 40 (no new
  opcode): index 0 is the topmost live window, `NotFound` past the end or with graphics
  disabled, and the compiler requires the title destination to hold 64 bytes.
  `samples/apps/desktop_shell` draws a System 7.5 menu bar (Apple square, File/Apps/Windows,
  right-aligned `HH:MM`, 22 rows) and a "Desktop" window; `File` lists windows through
  `window_info` into the terminal, `Apps` spawns `clock` with `core.spawn` (never
  `core.run`), `Windows` raises the terminal, and the Apple raises the shell. Check mode
  injects one pointer-down per item and prints `desktop_shell: ok`; normal mode promotes
  to realtime with heartbeats, paints, prints `desktop_shell: ready` and pumps
  `next_event`, and `scripts/desktop_screenshot_qmp.py` captures
  `/home/sinan/Desktop/igis-desktop.png` via QMP `screendump`. There is no global work
  area yet: kernel windows start at `y = 26` and the shell window at `y = 72`; an app
  opening at `y = 0` covers the bar until the next shell event. A full-screen
  `syslib.clear` still erases the kernel windows' pixels (their grids survive and the
  next expose/output repaints them). The M2.3.5 two-application proof now runs:
  `samples/apps/windows_proof` plus `windows_proof_a`/`windows_proof_b` place two
  task-owned windows side by side, switch focus and raise by real injected
  `syslib.inject_pointer` clicks (the parent asserts focus and z-order because the
  machine-wide queue is shared), drive a four-step scripted owner `window_move` drag
  (old area restored to the underlying terminal, new content painted), then move B
  over A and back so A takes its owner-only whole-content `Expose`, repaints only its
  visible strip (no per-task clip) and repaints the uncovered content through
  `window_present` (the covered part shows B while B is on top), and finally close
  both windows and exit `Ok` on a stage-4 command acknowledged by `core.wait`. Two
  prerequisites the proof exposed are fixed: the claim scan starts at a rotating
  cursor, so a busy low-slot task no longer starves higher slots on `-smp 1` (the old
  scan always began at slot 0); and console cell/grid paints compare a window-service
  visible-geometry generation before and after and retry with fresh visible parts, so
  a window that moves above the console during a repaint is not overwritten. `run
  windows_proof` prints `windows_proof: ok` on `-smp 4` and `-smp 1`; `windows_proof
  shot` holds the composed frame for `scripts/windows_proof_screenshot_qmp.py`
  (`/home/sinan/Desktop/igis-m23-proof.png`). Pointer-driven continuous dragging
  remains M2.4. The M2.4 language-level UI toolkit has started with its layout layer:
  `stdlib` now imports `core` and `syslib` (ADR 0026) and adds 18 ordinary-language
  helpers — 8×8 metrics (`ui_text_width` = code points × 8, `ui_text_height`,
  `ui_line_height`, `ui_content_inset`, `ui_title_bar_height`, `ui_close_box_size`,
  `ui_text_center_x`/`_y`), the System 7.5 palette as functions (`ui_black` 0x000000,
  `ui_white` 0xFFFFFF, `ui_gray` 0xCCCCCC, `ui_focus` 0xFFFFFF, `ui_accent` 0x000080),
  and client chrome (`ui_frame`, `ui_title_bar` with focused stripes/plain unfocused
  fill, an 11×11 close box and a code-point-safe 64-byte centred title, `ui_close_box`,
  `ui_separator`, `ui_center_text`). They draw only through the nine `syslib`
  primitives and never touch the window service's own chrome; there is no new opcode,
  builtin or `state`. `samples/apps/ui_layout` opens a real window, draws a synthetic
  dialog inside its content, presents it and checks stripes, borders, the close box, a
  button glyph and the accent swatch with `get_pixel` (`ui_layout: ok` on `-smp 4` and
  `-smp 1`); without graphics it checks the metrics and the disabled-screen `Invalid`
  path and prints `ui_layout: host ok` on `langhost`. The embedded library needed
  1,157,496 B of compile scratch at M2.4.0, so `MAX_ARENA_BYTES` rose from 1 to 2 MiB (the
  cap was provisional; spec 0008). String editing (M2.4.1) is implemented in `stdlib` as 13
  ordinary-language helpers with no new opcode, builtin or `state`: `str_boundary`,
  `str_cursor_clamp`/`left`/`right`/`home`/`end`, `str_insert_into`, `str_delete_into`,
  `str_backspace`, and `str_selection_normalize`/`all`/`collapse`/`replace`. Positions are
  byte offsets; insertion and deletion refuse a non-boundary with `Invalid`; a mutator
  stages into a local `bytes[256]`, converts through `core.substr_into` and publishes one
  whole-buffer copy, so a failure leaves the text untouched and the result is always valid
  UTF-8; the working capacity is 256 bytes (`CapacityExceeded` without a partial write).
  Cursors clamp instead of failing (negative → 0, past the end → the length, mid-code-point
  → that code point's first byte) and movement crosses whole code points; backspace at 0 is
  an `Ok` no-op; the selection is the scalar pair `start`/`stop`. `samples/apps/text_edit`
  exercises ASCII and 1-/2-/3-/4-byte code points, every rejection path, capacity and the
  selection helpers, prints `text_edit: ok` on `-smp 4` and `-smp 1`, and produces the same
  transcript on `langhost`. The library source is now 46,933 B, 32,600 B of code, and
  1,319,272 B of compile scratch. Controls (M2.4.2), menus (M2.4.3), dialogs (M2.4.4),
  icons (M2.4.5) and the widgets demo (M2.4.6) are next.

## Ground rules

1. **Spec before code.** Behavior changes are described in `specs/` and accepted before
   implementation. If reality and an accepted spec disagree, fix one of them explicitly.
2. **ADRs for decisions, specs for behavior.** A lasting constraint or technology choice
   gets an ADR in `adr/`. See `adr/README.md` for the immutability rule.
3. **Ideas are free.** Capture them in `IDEAS.md`; promote them when they matter.
4. **Don't invent requirements.** Ambiguity is resolved by asking, not by assumption.
   Record the answer in the relevant spec.
5. **Keep this file true.** Update the phase, status table, and toolchain notes when they
   change. Anything that grows beyond a few lines belongs in a spec or ADR.
6. **Work is broken into tickets in `PLAN.md`.** One ticket ≈ one session and one commit;
   its acceptance criteria and the deferred backlog live there, not in chat.

### Delegating tickets to subagents

Delegate each ticket to a **fresh subagent session**. Do not continue an old session across
tickets: its context grows with every slice and the model becomes less reliable (observed
after three slices). For every delegation the parent supplies the full context in the new
prompt — spec/ADR, current commit, exact files and symbols, lock order, acceptance
criteria, verification commands and boundaries — and independently verifies the result
(host tests, kernel transcript on `-smp 4` and `-smp 1`, diff, regressions) before
accepting it. A running session may finish the ticket it was given; the next ticket starts
fresh.

## Repository layout

```
.
├── AGENTS.md          # this file: operational facts and conventions
├── IDEAS.md           # unstructured inbox
├── OPEN-QUESTIONS.md  # parked decisions and unresolved questions
├── PLAN.md            # milestone plan, ticket backlog, deferred items
├── adr/               # decision log (immutable once accepted)
├── specs/             # normative specifications (living)
├── arch/x86_64/       # boot stub, page tables, long-mode transition, serial, APIC
├── abi/               # opcodes, headers, Status/FaultCode, reference encoding
├── compiler/          # lexer, parser, type check, codegen, manifest
├── interpreter/       # package verifier, machine, Host trait
├── langhost/          # host runner: compile, verify, run
├── kernel/            # entry, multiboot2, console, frame allocator, clock, ext2, REPL
├── stdlib/            # standard library source, embedded into the compiler
├── samples/lang/      # example programs
├── samples/apps/      # source applications staged into the ext2 image
├── xtask/             # build, image and QEMU orchestration
├── Cargo.toml         # workspace
├── rust-toolchain.toml
└── .cargo/config.toml
```

Planned top-level shape as code grows — architecture-specific code must not leak into
the core. `arch/x86_64/`, `abi/`, `compiler/`, `interpreter/`, `kernel/`, `langhost/` and
`xtask/` exist so far:

```
arch/<isa>/         # ISA-specific: boot, long mode, paging, stack switch, IDT, APIC, ...
platform/<machine>/ # machine-specific: memory map, device layout, boot protocol
kernel/             # arch-agnostic core
compiler/           # lex, parse, type/capacity check, bytecode and manifest emission
interpreter/        # bytecode execution, safe points, builtin dispatch
core/               # native primitives (Rust): console, streams, time, text
stdlib/             # standard library written in the application language
syslib/             # privileged hardware primitives (ports, MMIO, DMA)
drivers/            # framebuffer console, keyboard, timer, block
mm/                 # frame allocator, task regions, stream buffers
sched/              # per-core loops, runnable set, claim/release, budgets
vfs/                # read-only ext2 over BlockDevice, later writable filesystems
abi/                # shared kernel/runtime ABI definitions
libs/               # no_std support crates
xtask/              # build, image, and QEMU orchestration
```

## Toolchain

| Item | Version / value |
|------|-----------------|
| stable (default) | rustc 1.99.0 |
| nightly | rustc 1.101.0-nightly (2026-11-02) |
| targets installed | `x86_64-unknown-linux-gnu`, `x86_64-unknown-none` |
| extra components | `rust-src` (nightly), required for `-Z build-std` |
| QEMU | qemu-system-x86_64 10.2.2 |
| boot tooling | GRUB 2.12 (`grub2-mkrescue`), xorriso |

## Build commands

The kernel is cross-built for `x86_64-unknown-none`; `xtask` itself builds for the host.

| Command | Purpose |
|---------|---------|
| `cargo xtask build` | build the kernel ELF (release) |
| `cargo xtask image` | build and assemble `build/igis.iso` with GRUB |
| `cargo xtask run` | image + QEMU with a display, serial on stdio |
| `cargo xtask run --headless` | image + QEMU with no display (serial only) |
| `cargo xtask clean` | remove build artifacts |
| `cargo test -p abi -p compiler -p interpreter -p langhost` | host language tests (the kernel crates have no test harness) |
| `cargo run -p langhost -- samples/lang/hello.ig` | compile and run one unit on the host |

`.cargo/config.toml` pins the linker script, `code-model=kernel`, and the static
relocation model for the bare-metal target. The workspace has no default target, so
host tools are unaffected.

## Document conventions

- Specs: `specs/NNNN-kebab-title.md`, template `specs/0000-template.md`, index in
  `specs/README.md`. Status must be one of Draft / Accepted / Implemented / Withdrawn /
  Superseded.
- ADRs: `adr/NNNN-kebab-title.md`, template `adr/0000-template.md`, index in
  `adr/README.md`.
- Requirements are numbered and use MUST / SHOULD / MAY (RFC 2119).
- Dates are ISO 8601 (`YYYY-MM-DD`).
- Work in small commits; one logical change per commit. Do not commit unrelated edits.

## Decisions so far

| Decision | Record |
|----------|--------|
| Rust as the implementation language | ADR 0012 (Accepted) |
| x86_64 first, multi-arch layout from day one | ADR 0013 (Accepted) |
| All code runs at ring 0 in a single address space | ADR 0001 (Accepted) |
| Applications are interpreted source, not native binaries | ADR 0002 (Accepted) |
| Per-core interpreter loops; tasks claimed from a global set, never pinned | ADR 0011 (Accepted; supersedes pinning in 0003) |
| Cooperative safe points, quantum, total CPU budget | ADR 0003 + ADR 0011 |
| Static preallocated application memory; no malloc, no GC | ADR 0004 (Accepted) |
| Internal bytecode permitted; no JIT | ADR 0005 (Accepted) |
| MMU hardening: RO kernel text, guard pages, NX | ADR 0006 (Accepted, provisional) |
| Fully static placement; no runtime allocation of any kind | ADR 0007 (Accepted) |
| `fault(code, diag)` decides, `exit(reason, diag)` always tears down; one recovery, no retry | ADR 0010 (current; supersedes 0009) |
| Boot via GRUB + multiboot2 under QEMU, with own long-mode transition | ADR 0014 (Accepted) |
| Applications delivered on a read-only ext2 image over an in-memory block device | ADR 0015 (Accepted) |
| No capability model; every application may use every `syslib` primitive | vision R14 / spec 0001 |
| Framebuffer console with a bitmap font; no windowing | vision OQ 8 |
| Byte-FIFO streams, copied through the kernel, atomic sends | spec 0006 (Draft) |
| Pascal-like language, nominal types, no user generics | spec 0003 (Draft) |
| `core` (Rust) / `stdlib` (language) / `syslib` (hardware) split | specs 0002, 0003 |
| `bytes` (raw) and `string` (UTF-8) as distinct types | spec 0003 |
| One task per application instance; REPL-only in the first milestone | specs 0001, 0007 |
| Realtime class: no quantum preemption, one-second heartbeat, admission by core count, no reserved core | ADR 0016 (Accepted) |
| REPL is a kernel service; boot tries `shell` then `entrypoint`; `run`/`spawn` carry `string[1024]` args through a 1024-slot queue and return global task ids | ADR 0017 (Accepted + addenda) |
| Console is a byte stream (no echo, no key events); one global nanosecond clock at 1 ms resolution, no TSC; compile diagnostics via a caller-owned `Diagnostic` out-parameter | specs 0003, 0005, 0007 |
| Console input: polled COM1 + PS/2, kernel-staged line discipline, blocking `core.read_line` | spec 0005, Q44 |
| Streams are single-producer/single-consumer; names are unique per application; framing helpers live in `stdlib` | spec 0006 (Draft) |
| Floating point in the first milestone: one `f64` type, IEEE-754, with a kernel-owned per-task `fpu` region | ADR 0018 (Accepted) |
| No visibility modifiers: every top-level declaration is public; conversions are `T(x)` casts; maps use FNV-1a with linear probing | specs 0003, 0004 |
| Boot: INIT-SIPI-SIPI through a sub-1 MiB trampoline, `MAX_CORES` static state; the framebuffer tag is optional (a missing or unsupported mode leaves the console on serial); one IPI vector | specs 0002, 0007, 0009 |
| ext2 subset: 1/4 KiB blocks, double indirect, regular root files, metadata validated; unsupported images refuse the mount | specs 0002, 0007 |
| `ExitReason.Killed`; dispatcher before every claim; admission counts cores online at call time; relative paths without `..` | specs 0003, 0005, 0007 |
| Bytecode: one package per application, 4-byte-word stack VM, split `compiler`/`interpreter` crates over `abi`, structural verification | spec 0008 (Draft) |
| Scheduler: slot table of 64 tasks, claim takes the value under a per-slot spin lock, 10 000-step quantum, 10 s / 200M-step budget | spec 0007, ADR 0011 |
| Delivery is source-only, permanently: the image never carries bytecode and packages live only in memory | ADR 0019 (Accepted) |
| Integer formatting is a staged `core` opcode; `syslib` ports are staged as scalar `uint`s | specs 0003, 0008; Q45 |
| Enums: nominal `type Name = (Variant, ...)`, variants are global ordinal constants in one 8-byte slot; `Status` predeclared; `EnumName(int)` faults `Overflow` out of range | spec 0003, spec 0008 (`enum_cast`) |
| `core.spawn`/`wait`/`kill` return `Status`; `spawn` takes `var diag: Diagnostic` and `wait` takes `var reason: ExitReason` | spec 0005, Q43 |
| `core.run` is spawn + wait with the pending id in the `Machine`; a failed request carries its `Diagnostic` in the exit ring for `run`, while `wait` sees `Faulted(IoError)` | specs 0003, 0005, 0007 |
| Compiler nesting limit 32; post-boot kernel stack is 256 KiB with a guard frame | Q42, spec 0002, `arch/x86_64/src/stack.rs` |
| Stack guards for boot, AP and task stacks: 32-table cache, 96-frame registry, CR3 + IPI TLB shootdown after every PTE change | ADR 0006, spec 0002 H2, spec 0004 C6, `kernel/src/guard.rs`, `arch/x86_64/src/paging.rs` |
| Task stack allocation is `stack_len + 1` frame: guard excluded from the region, present bit restored before free, reclaim returns guard + stack as one unit | specs 0004, 0007 |
| Records: nominal, declaration-order layout with every field aligned to 8; `Diagnostic` predeclared at fixed `abi::layout` offsets | spec 0003, spec 0008 |
| Sum types: nominal, 8-byte tag at 0 plus the largest payload at 8 (rounded); destination-driven construction; exhaustive `case` with by-value arm-local payload bindings; `ExitReason` is `Ok | Killed | Faulted(code: FaultCode)`; `FaultCode` variants are qualified | spec 0003, spec 0008 |
| `stdlib` is embedded source (`stdlib/stdlib.ig`), compiled into the application's package; `stdlib.*` is qualified, the function table is library-first, line-table unit ids are 0 (app) / 1 (stdlib) | spec 0003, spec 0008 |
| Functions: `function Name(...): T;` with `return value;`, scalar results only, `return` on every path | spec 0003, spec 0008 |
| Arrays: `array[T, N]` is nominal in element type and count, `N` a positive const, index `0..N-1` faults `OutOfBounds`; element refs via `ref_element`, whole copies via `copy_refs` | spec 0003, spec 0008 |
| Rings: `ring[T, N]` is nominal in element and count, elements then `head`/`count`, `core.ring_push`/`pop`/`peek`/`full`/`empty` lowered by the compiler with no opcode; `Full`/`NotFound` leave the ring and out-parameter untouched | spec 0003, spec 0008 |
| Maps: `map[K, V, N]` is nominal in key, value and count; `count` then `capacity` slots of state/hash/key/value; FNV-1a, linear probing, backward-shift deletion; `core.map_put`/`get`/`has`/`remove`/`count` are five `map_*` opcodes and `put` at `N` is `Full` | spec 0003, spec 0008 |
| UTF-8: `core.substr_into` has four kind combinations and the destination kind decides (boundaries for `string`, strict validation for `bytes`→`string`, raw for a `bytes` destination); the source kind is the `SUBSTR` `b` field, no new opcode; the checked slice stays in `core` forever (destination capacity is a call-site immediate) | spec 0003, spec 0008 |
| APs come up by attempt: INIT-SIPI-SIPI, trampoline at `0x8000` with a parameter page at `0x9000`, `MAX_CORES` = 8, per-core TSS/IST and 256 KiB stacks (the compiler's bound, since any core may compile), bounded 100 ms wait; absent ids are not online | spec 0002 "SMP", spec 0007 |
| Every online core runs the scheduler loop over the global set; an idle core does one `sti; hlt` step and producers set `need_resched` before the `0x40` IPI, whose handler only EOIs | spec 0007, ADR 0011 |
| A claimed task's slot stays reserved by task id until reclaim; a request id stays live while it compiles; the boot core stops the APs and prints per-core totals | spec 0007 |
| `syslib.cpu_id()` (builtin 10) reports the APIC id of the executing core; the core index is internal | spec 0003, spec 0008 |
| Realtime: one-way `core.realtime_on()` (builtin 11) admitted by a CAS counter against online cores, `core.heartbeat()` (builtin 12) with a fixed 1 s non-blocked deadline, realtime-first claim, no quantum preemption, no total budget, `WatchdogTimeout`, admission released only at termination | spec 0007, ADR 0016 |
| Streams: top-level `in_stream[N]`/`out_stream[N]` state variables, handles in the `state` prefix in manifest order, dynamic load-time pairing with a waiting peer, same-direction/mismatch/closed load errors, four `stream_*` opcodes, `WaitFor::Stream` block/wake, kernel arena with first-fit free list, close-on-termination discards queued bytes | spec 0006 (amended), spec 0008 |
| Stream framing is `stdlib` code over `const ..._stream[1]` parameters: length-prefixed `u32` LE frames, `\n`-delimited lines with CRLF stripping, fixed-width `u64`, one-byte receives (a receive returns up to the destination capacity and has no pushback), one copy publishes a whole value so errors never leave a partial destination | spec 0003, spec 0006 |
| Float text is fixed-point plus shortest round-trip in the milestone: `format_f64` is one opcode (destination kind + capacity immediate), 0–9 fraction digits, half away from zero with carry, `nan`/`inf`/`-inf` literals, `-4` for a wider field or `\|value\| >= 2^63`, `-6` for a short destination; `format_f64_short` is one opcode with no width and no magnitude bound, the shortest digits that round-trip, positional for `-6 < e10 + 1 <= 21` and scientific otherwise, ties away from zero; `stdlib.parse_f64_into` is language code over a decimal grammar that saturates overflow/underflow to `±Inf`/`±0` | spec 0003, spec 0008; Q40/Q41 |
| Graphics: one full-screen shadow from the frame allocator, sized by the returned mode (32 bpp only); drawing goes to it and `present` blits pitch-aware rows; the console draws through it; a rectangular clip is the window-manager seam | spec 0009, ADR 0020 |
| The nine `syslib` graphics primitives are host builtins (no new opcode): `0x00RRGGBB`, top-left pixel origin, `Invalid` for `w <= 0`/`h <= 0`/colour above `0x00FFFFFF`, clipped never faulted, 8×8 font, `fill_dither` is `(X + Y)` even | spec 0009, spec 0003, spec 0005 |
| The window server is a kernel `syslib` service over the shadow, with `expose` repaint and no per-window canvas or user-space stream protocol | ADR 0020; M2.3 |
| Input is one machine-wide pointer (and later one event queue) owned by the kernel; the PS/2 aux device is polled beside the keyboard, the cursor is a present-time compositor overlay and never in the shadow, and injection is the debug-only deterministic test hook | spec 0010, ADR 0021 |
| `syslib.pointer_x`/`pointer_y` are `int`, `pointer_buttons` is a `uint` mask, and `inject_pointer(dx, dy, buttons)` returns `Invalid` above `0b111`; the four are host builtins with 0/0/0 and `Invalid` defaults (builtins 22–25) | spec 0010, spec 0003, spec 0005, spec 0008 |
| The software cursor is a 16×16 black-outline/white-fill arrow with hotspot `(0, 0)`, composited into the framebuffer by `present`, with a move re-presenting only the old and new cursor rectangles | spec 0010, spec 0009 |
| The primary pointer is an absolute USB HID tablet behind the PIIX3 UHCI controller (`kernel/src/pci.rs`, `kernel/src/usb.rs`): static frame list/QH/TDs, enumeration over control transfers, one self-linked polled interrupt-IN TD, HID `0..=32767` scaled to the screen with round-to-nearest, tablet-over-PS/2 precedence, `syslib.pointer_source()` (builtin 26, no new opcode); PS/2 remains the fallback and the driver moves behind `arch`/`platform` later | spec 0010, ADR 0022, ADR 0013 |
| Input events: predeclared `EventKind`/`Event` (40 bytes at `abi::layout::event`), one 64-event machine-wide FIFO with the newest event dropped on overflow and a monotonic drop counter, key events packing `text \| modifiers << 8 \| key code << 16` (`abi::key`), `syslib.next_event` blocking on `WaitFor::Input` with SMP-safe wake, and `inject_event`/`event_drops`/`event_flush` as builtins 27–31 (no new opcode); the console byte stream and `core.read_line` are unchanged | spec 0010, spec 0003, spec 0008 |
| PS/2 aux IRQ12: the I/O APIC at `0xFEC00000` (page split, `PWT \| PCD`) routes IRQ12 to the boot core as a level-triggered vector `0x2C`; the handler only drains ≤ 64 aux bytes into a 1024-byte SPSC atomic ring and EOIs the local APIC; `mouse::poll` decodes the ring in the service opportunity, with the direct controller drain as the fallback when the route is unavailable (modes exclusive) | spec 0010, spec 0002, ADR 0023 |
| Windows are records in a 16-entry kernel table: monotonic `int` ids (`0` invalid), `window_open` takes the frame origin and content size (1-px border, 16-px title bar), one bottom-to-top z-order with one focus and topmost fallback, owner-only `close`/`move` with any-task `raise`, close-on-task-termination through `Task::reclaim`, a service-owned shadow clip with save/narrow/restore, and whole-content `Expose` repair; implemented in M2.3.1 as builtins 32–38 with the predeclared `Rect` and no new opcode | spec 0011, ADR 0024 |
| Window composition and repair: bounded 32-rect desktop/chrome damage, visible-part subtraction (frame minus higher frames), one `present` of the damage union, white + `(X + Y)` dither desktop, service-drawn 1-px border / 16-px title bar / close box, synchronous repair under the window lock (window → graphics), content never kept by the service | spec 0011 |
| Event routing at poll time: producers never take the window lock; `Expose` is a per-window, owner-only coalesced flag (ordinal 5, whole content) returned before the queue; a pointer-down hit-tests and raises/focuses before delivery; `window_present` presents content ∩ visible parts; `syslib.present` stays generic; a client that never repaints is left with stale content in M2 | spec 0011, spec 0010 |
| Kernel text windows: `console::init` opens a terminal (`core.write` + line echo) and a system log (`println!`) as owner-`0` windows, each with its own grid/cursor/scroll; drawing uses an explicit clip (`gfx::draw_cell`/`present_within`), never the shadow's global clip; pending owner-0 exposes are pulled by `console::pump` in `console::poll_input`; lock order `CONSOLE → WINDOW → SCREEN`; serial mirror unchanged | spec 0012, spec 0009, ADR 0025 |
| Owner `0` is the kernel (task ids start at 1); kernel windows are ordinary windows (focus, hit-test, `Expose`, `window_present`) and the kernel calls the same public window functions | spec 0012, spec 0011 (W20) |
| The desktop shell is an application, not the boot entry (M2.5.6): a 22-row app-drawn System 7.5 menu bar with direct items (no dropdowns), `core.spawn` launches (never `core.run`), the File item lists windows and the Windows item raises the terminal; no global work area | spec 0012, ADR 0025 |
| Read-only window enumeration: `syslib.window_info(index, var id, var rect, var title[64])` (builtin 40, no new opcode), index 0 = topmost, `NotFound` past the end or disabled graphics, title capacity 64 compile-checked | spec 0011 (W19), spec 0012, spec 0008 |
| Fair scheduling: the claim scan starts at the slot after the last claimed one (round-robin over the runnable slots), so a busy low-slot task cannot starve higher slots on `-smp 1`; realtime-first claim is unchanged | spec 0007 (Sch12), M2.3.5 |
| Console stale-parts retry: every window mutation bumps a visible-geometry generation; a cell/grid paint reads it before computing visible parts and compares after painting, retrying with fresh parts (bounded) so a window that moves above the console mid-repaint is not overwritten | spec 0012 (D3), M2.3.5 |
| M2.3.5 proof: two task-owned windows; the parent-owned `Proof Control` frame origin is a persistent cross-task command stage (only `window_raise` is cross-task writable); injected pointer clicks are asserted by focus/z-order, not by which task receives the event; the drag is four scripted owner `window_move` calls; teardown is a stage-4 close/exit acknowledged by `core.wait`; no new service API or opcode | spec 0011 (M2.3.5), M2.3.5 |
| `stdlib` may import `syslib`: the embedded library is trusted source compiled into the package and an application already has every `syslib` primitive, so the UI layout layer draws through the nine graphics primitives with no new authority, opcode, builtin or cross-unit type | spec 0003 ("UI layout and chrome"), spec 0008, spec 0009 (G19), ADR 0026 |
| The UI layout layer is `stdlib` source: metrics as code points × 8, line height 16, inset 8, title bar 16, close box 11; palette `0x000000`/`0xFFFFFF`/`0xCCCCCC`/`0x000080`; `ui_frame`/`ui_title_bar`/`ui_close_box`/`ui_separator`/`ui_center_text` draw client chrome only, never the window service's frames | spec 0003, ADR 0026, M2.4.0 |
| Compile scratch is 2 MiB: the M2.4.0 UI layer took the embedded library to 1,157,496 B, over the old 1 MiB cap; the cap was always provisional | spec 0008, M2.4.0 |
| String editing is 13 `stdlib` helpers over the checked slice and byte indexing: byte-offset positions, boundary-checked insert/delete, a `bytes[256]` stage plus one whole-buffer publish (atomic and valid UTF-8), clamping cursors, `Ok` backspace at 0, and a scalar `start`/`stop` selection; no new opcode, builtin or `state` | spec 0003 (SE1–SE7), M2.4.1 |

## Open questions

See [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) for the remaining parked items: tuning
constants, the deferred debug/DMA/streams work, the float follow-ups, and the
`core.kill`/claim liveness question (Q50). Boot, platform, scheduler, REPL and language
decisions are settled.
When an item is resolved, record the answer in its spec or a new ADR and remove it there.

## Status log

| Date | Change |
|------|--------|
| 2026-11-03 | Repository created; documentation structure established. |
| 2026-11-03 | Vision captured in spec 0001; ADR 0001 (ring 0 only) and ADR 0002 (interpreted apps) accepted. |
| 2026-11-03 | Architecture spec 0002, language seed 0003; ADR 0003 (pinned cooperative tasks), 0004 (static memory), 0005 (bytecode), 0006 (MMU hardening, provisional). |
| 2026-11-03 | Memory model draft 0004; placement fork (static vs monotonic regions) left open. |
| 2026-11-03 | Option A selected: fully static placement, no runtime allocation (ADR 0007). Memory spec updated with rules, examples, and accepted costs. |
| 2026-11-03 | Destination-form construction recorded: `dst = a + b` writes into a declared buffer; no temporaries, no GC. |
| 2026-11-03 | `exit(reason)` as top-level catch: frames abandon to main level, handler may reclaim or transfer to a validated recovery label (ADR 0008 amended by 0009); aliasing forbidden, no views, capacity-typed strings. |
| 2026-11-03 | Handlers split: `fault(code)` decides terminate/recover, `exit(reason)` always tears down; one recovery per task, no retry (ADR 0010, supersedes 0009). |
| 2026-11-12 | Origin recorded: 2005 front-end for a quantum-cryptography modem; the ring boundary was the friction that produced ring 0. Execution changed to per-core interpreter loops with work stealing (ADR 0011). Rust, x86_64-first, GRUB/multiboot2 and read-only ext2 recorded (ADR 0012–0015). Specs 0001–0004 updated; host interface (0005), streams (0006) and scheduler (0007) added. |
| 2026-11-12 | Realtime class added: one-way promotion, no quantum preemption, admission by core count, one-second heartbeat with `WatchdogTimeout` (ADR 0016). |
| 2026-11-12 | Realtime admission deliberately allows occupying every core; no core is reserved for the REPL. Tradeoff recorded in the ADR 0016 addendum. |
| 2026-11-12 | REPL model settled: a kernel service compiles and starts applications from a request queue; the shell is an ordinary application; `run`/`spawn`/`wait`/`kill` with global task ids (ADR 0017). |
| 2026-11-12 | REPL details settled: `shell` then `entrypoint`, `no entry` halt, fixed `string[1024]` arguments, queue capacity 1024, orphan children continue (ADR 0017 addendum). |
| 2026-11-12 | Per-task handles replaced by global `TaskId`s and a 64-entry exit-reason ring; no per-task handle table (ADR 0017 addendum 2). |
| 2026-11-12 | Argument delivery settled as `main(args: const string[1024])`; a full request queue returns `Status.Full`, and `run` blocks only on the child. |
| 2026-11-12 | Remaining unresolved items parked in `OPEN-QUESTIONS.md`; session paused before code. |
| 2026-11-13 | Host interface settled: console is a byte stream, one global nanosecond clock (no TSC), caller-owned compile diagnostics; Q5–Q7 closed (specs 0003, 0005, 0007). |
| 2026-11-13 | Streams settled: unique application-wide names, one sender and one receiver, `stdlib` framing helpers; explicit close deferred to v2; Q11–Q13 closed (spec 0006). |
| 2026-11-13 | Floating point added to the first milestone: `f64` with a per-task FXSAVE region (ADR 0018). Language settled: no visibility modifiers, `T(x)` casts, FNV-1a maps, `core.bit_*`, `Diagnostic` only; Q15–Q23 closed (specs 0002–0004, 0006, 0007). |
| 2026-11-13 | Boot, loader and REPL settled: sub-1 MiB trampoline, mandatory framebuffer tag, one IPI vector, ext2 subset, dispatcher timing, `ExitReason.Killed`, path validation; Q24–Q38 closed (specs 0002, 0003, 0005, 0007). |
| 2026-11-13 | Spec 0008 drafted: one bytecode package per application, fixed 4-byte-word stack VM, `compiler`/`interpreter` split over `abi`, structural verification at load. |
| 2026-11-13 | Source-only delivery made permanent: no precompiled packages ever, compile latency is not a constraint (ADR 0019; specs 0002, 0008). |
| 2026-11-13 | Specs 0001–0008 accepted. Workspace created (`arch/x86_64`, `kernel`, `xtask`): GRUB/multiboot2 boots into long mode, serial works, the framebuffer tag is parsed and the screen cleared. |
| 2026-11-13 | Kernel diagnostics: 8x8 bitmap-font console on the framebuffer (mirrored to serial), IDT with 32 exception gates and a register dump (verified with a deliberate `int3`), multiboot2 memory-map parsing. |
| 2026-11-13 | Frame allocator: 4 KiB bitmap over the first 4 GiB, initialised from the multiboot2 map and reserving low memory, the kernel image, the multiboot2 structure and the framebuffer; verified by allocating, writing, reading back and freeing two frames (254 MiB usable under `-m 256M`). |
| 2026-11-13 | GDT/TSS with a double-fault IST (verified with a real non-canonical-stack fault), local APIC enabled with the legacy PIC masked, timer calibrated against the PIT (~62.5k ticks/ms in QEMU), and a 1 kHz tick advancing the global nanosecond clock (Q6, spec 0007). |
| 2026-11-13 | Host language slice: `abi` (ABI v1 opcodes/headers), `compiler` (lexer → parser → type/capacity check → deterministic package), `interpreter` (structural verifier, allocation-free machine, `Host` trait), `langhost` runner; 50 tests, kernel untouched. Spec 0008 open questions 1 and 3 resolved. |
| 2026-11-13 | Compiler made `no_std` and allocation-free (B11): a bump arena over the caller's `&mut [u8]`, source-borrowing tokens and AST, arena-backed lists instead of `Vec`/`HashMap`, fixed-buffer diagnostics, and `compile_into(source, path, arena, code, manifest) -> Package<'a>` writing into caller buffers. Packages are byte-identical to the previous build; 54 tests; `langhost` keeps a std helper that allocates the caps. |
| 2026-11-13 | First in-kernel vertical slice: `xtask image` stages `samples/apps/` into a 4 MiB ext2 image attached as the `apps` GRUB module; the kernel finds the module tag, mounts the image read-only (validated superblock, group descriptors, inodes, directories, direct/single/double indirect), reads `shell`, compiles it with `compile_into`, places `code`/`state`/`stack` in contiguous frames, verifies and runs it (`exit: Ok`), reclaims the frames and halts. Boot stack raised 16 KiB → 64 KiB after the in-kernel compiler overflowed into the boot page tables. |
| 2026-11-13 | Scheduler loop: `task.rs` owns the record and a 64-slot runnable table (claim/release by value move), `sched.rs` runs claim → quantum → release/terminate on the boot core. The REPL only compiles and enqueues; the boot application crosses 181 claims over a 10 000-step quantum, then `exit Ok` and an idle summary. Budgets are provisional: 10 s of CPU time plus a 200M-step cap because the 1 ms clock cannot resolve a quantum. |
| 2026-11-13 | REPL request service: `core.spawn`/`wait`/`kill` with a staged scalar `int` ABI, a 1024-slot static request queue serviced before every claim, a 64-entry exit ring, `Blocked(task exit)` with wake-on-termination, and a `shell` → `child` sample that blocks, wakes and reports `child: ok`. A load failure prints the diagnostic and records `Faulted(IoError)` under the reserved id. |
| 2026-11-13 | Console input: COM1 and the PS/2 keyboard are polled in the service opportunity into one static FIFO, a staged line ring echoes/edits/completes lines, and `core.read_line` blocks the shell (`Blocked(Console)`) until a line exists; `shell` reads three lines and exits. Temporary until indexing lands (Q44). |
| 2026-11-13 | Buffer indexing, comparison and length: `buf[i]` is a raw `byte` checked against the runtime length; `bytes[i] := b` is checked against the capacity and extends the length (zero-filling the gap), `string` stays read-only by index; `=`/`<>` compare buffers of the same kind across capacities; `core.len` returns `uint`. New ABI v1 opcodes `load_u8`, `store_u8`, `cmp_buffers` (54 → 57) and builtin id 6. The shell is a real `help`/`hello`/`exit` loop; the host `read_line` stub scripts those lines. |
| 2026-11-14 | String search and slicing: `core.find` (first index of a `byte` or `-1`) and `core.substr_into` (same-kind checked slice, negative → `-4`, past-end → 0, clamped copy, UTF-8 boundary check for `string`) as ABI v1 opcodes `find`/`substr` (57 → 59). The shell splits `cmd`/`rest` and the `run` tail into path/args; `child` prints its `main(args)`, `echo` writes the tail, and `run missing` exercises the `-3` spawn failure. |
| 2026-11-14 | Integer formatting and the first `syslib` primitives: `core.format_int_into` (opcode `format_int`, 59 → 60; decimal/hex, sign, zero padding, exact fit or `-6`, bad base `-4`) and `syslib.port_in8`/`port_out8` (builtin ids 7/8 through `Host::port_in8`/`port_out8`, kernel over `arch_x86_64::port`). `Port` is staged as a scalar `uint` (Q45). `samples/apps/clock` polls the CMOS RTC and prints a real `HH:MM:SS`; the host mock prints `00:00:00`. The boot stack grew 64 → 128 KiB because the added compiler paths tipped the in-kernel compile of the shell's child over the old limit and corrupted the boot page tables. |
| 2026-11-14 | Q42 closed: the compiler has a fixed 32-level nesting limit (parser expression/statement/unary recursion and independent codegen walks; a `Diagnostic` beyond it), and everything after early boot runs on a 256 KiB kernel stack with a not-present guard frame. The guard's 2 MiB identity page is split into 4 KiB PTEs in a static `.bss` table, CR3 is reloaded and the higher-half alias is shared. `#PF` got its own IST so an exhausted stack still reports a clean fault; the `.boot` stack is early-boot only. Deep-source test: clean `#PF` with CR2 in the guard frame. Per-task stack guards remain future work. |
| 2026-11-14 | Enums and the `Status` ABI: `type Name = (Variant, ...)` declares a nominal enum whose variants are global ordinal constants (`0..n-1`) in one 8-byte slot; assignment/comparison only within the same enum (ordering by ordinal), `int(enum)` yields the ordinal, `EnumName(int)` is range-checked and faults `Overflow` (opcode `enum_cast`, 60 → 61). `Status` is predeclared with `Ok` = 0; a user `type Status` and a duplicate type/variant are compile errors. `core.spawn(path, args, var id)`, `core.wait(id, var reason)` and `core.kill(id)` now return `Status`; the reason stays a staged `int` and `Diagnostic`/`ExitReason` records remain missing (Q43 narrowed). The shell checks each `Status` and prints `spawn failed`/`wait failed` plus ok/killed/faulted. |
| 2026-11-14 | Records and the record-based REPL ABI: `type Name = record field: T; ... end;` with nominal types, declaration-order 8-byte-aligned layout, nested records, buffer fields, whole-record copies (`copy_refs`), field references (`ref_offset`) and `value`/`const`/`var` record parameters. `Diagnostic` and `ExitReason` are predeclared at the fixed `abi::layout` offsets; `core.spawn(path, args, var diag, var id)` fills `diag` from the kernel compiler on a synchronous failure and clears it otherwise, and `core.wait(id, var reason)` writes `kind`/`code`. `run missing` prints `spawn failed: No such application` from `diag.message`, `run faulty` reports `child: faulted` from `reason.kind`, and the staged reason `int` is gone (Q43 narrowed to sum types and `core.run`). |
| 2026-11-14 | Sum types, `case` and the real `ExitReason`: `type Name = Variant | Variant(payload: T);` is nominal, an 8-byte tag at offset 0 plus the largest payload at offset 8, constructed into a destination (`dst := Variant(payload)`; variant names are scoped to the sum, so `Ok` is `Status.Ok` or `ExitReason.Ok` by destination). `case value of Variant: ... Variant(binding): ... end;` is exhaustive (one arm per variant, no `else`), has no fallthrough, and copies a payload by value into an arm-local binding; missing, duplicate and unknown arms, payload mismatches and cross-sum variants are compile errors. No new opcodes: construction is `store_ref`/`clear`/`append`/`copy_refs`, `case` is `load_ref`/`eq`/`jz`. `ExitReason` becomes `Ok | Killed | Faulted(code: FaultCode)` at the same 16-byte layout; `FaultCode` is a predeclared enum with qualified variant names (`FaultCode.DivideByZero`) because two names collide with `Status`. The shell's `report` is an exhaustive `case`; `run faulty` prints `child: faulted DivideByZero`, `run child world` prints `child: ok`, `run missing` keeps its `diag.message`, and the kernel run ends `task 1: exit Ok`. Q43 narrows to `core.run`, `TaskId` and the request-completion diagnostic path. |
| 2026-11-14 | `core.run` and the request-completion diagnostic: `core.run(path, args, var diag, var reason)` is spawn + wait in the interpreter, with the pending child id in the `Machine`, so a blocked retry waits without spawning twice (builtin id 9, `(4, 1)`). The exit ring now carries a 192-byte `Diagnostic` image for a load failure the service discovers after `spawn` returned; `Host::diagnostic_for` (default `false`) lets `run` copy it into its caller-owned `diag`, while `core.wait` keeps its signature and observes only `Faulted(IoError)`. The shell's `run` command uses `core.run`; the boot child still uses spawn + wait. `run broken` prints the compiler's `unknown identifier` message, `run ../shell` prints the path rule. Q43 is now only the distinct `TaskId` type. |
| 2026-11-14 | Multi-unit `stdlib`: `stdlib/stdlib.ig` is real source embedded with `include_str!`; an application with `uses stdlib;` compiles it into the same package (library functions first, shared constant pool, line-table unit ids 0 app / 1 stdlib, manifest keeps the app unit name, diagnostics name the failing unit). `function ... : T` with `return` (scalar results) and the first helpers `trim_into`, `split_once`, `parse_int_into`, `starts_with`, `contains_byte` landed; `samples/apps/text` runs `run text 42 hi` in the kernel. The stdlib adds ~5.3 KiB of source, ~4.1 KiB of code blob and ~154 KiB of compile scratch to the sample app. |
| 2026-11-14 | Fixed-size arrays: `array[T, N]` (positive const N) in state, locals, records, sum payloads and `value`/`const`/`var` parameters. Layout is N elements at `align8(element size)` with the array rounded to 8; arrays are nominal in element type and count. `a[i]` reads/writes the element (`index < N` at run time, `fault(OutOfBounds)`, never a clamp), nested `a[i][j]`/`a[i].field` composes, whole-array assignment and `value` parameters copy `N * stride` bytes, and `const`/`var` parameters are call-scoped references. One new opcode `ref_element` (immediates stride, count; ABI v1 61 → 62) forms an element reference; the verifier requires an 8-byte-multiple stride and a non-zero count whose product fits a section. One structural array table is shared by the app and `stdlib`, so arrays cross the unit boundary; `stdlib.split_all` fills an `array[string[64], 8]`. `samples/lang/array.ig` and `array_bounds.ig` cover reads, writes, copies, records, nested arrays, buffers, parameters, the stdlib helper and the bounds fault. |
| 2026-11-14 | Fixed-capacity rings: `ring[T, N]` (positive const N) in state, locals, records, sum payloads, arrays and `value`/`const`/`var` parameters; nominal in element and count; layout is the `N` elements at `align8(element size)` followed by `head: u64` and `count: u64`; whole-ring copies include the header. `core.ring_push`/`pop`/`peek`/`full`/`empty` are compiler-lowered over `ref_offset`/`load_ref`/`store_ref`/`copy_refs`/`ref_element`, `add_u`/`mod_u` and `eq`+`jz`, so the ABI v1 opcode table is unchanged. `push` on full returns `Full` and writes nothing; `pop`/`peek` on empty return `NotFound` and leave the out-parameter untouched; `push`/`pop` require a writable place, so `const` is rejected while a `value` copy is writable and private to the callee. The structural ring table is shared with `stdlib` (`drain_into` takes a `ring[int, 8]`); `samples/lang/ring.ig` covers FIFO order, peek, full/empty, wrap-around, whole copies and buffer/record/array/nested-ring elements. |
| 2026-11-15 | Fixed-capacity maps: `map[K, V, N]` (`N` a positive const, `K` one of `int`, `string[M]`, `bytes[M]`) in state, locals, records, sum payloads, array and ring elements, and `value`/`const`/`var` parameters; nominal in `(K, V, N)`, no at-least-N, no comparison. Layout is `count: u64` then `capacity` (smallest power of two at least `2N`) slots of `state`/`hash`/key/value; the hash is FNV-1a over the eight little-endian bytes of an `int` key or the `[0..len)` bytes of a buffer key, probing is linear and deletion is backward-shift with no tombstones. `core.map_put`/`get`/`has`/`remove`/`count` are five new opcodes (`MAP_PUT` … `MAP_COUNT`, key and value kinds in the opcode word, `capacity`/`count`/`key_size`/`value_size` immediates), so the algorithm lives once in the interpreter; `put` at `N` entries returns `Full` and writes nothing, a missing key is `NotFound`/`false`, and `get` leaves its out-parameter untouched. One structural map table is shared with `stdlib` (`map_get_or` takes a `map[int, int, 8]`); `samples/lang/map.ig` covers collisions, backward shift, key 0, the empty buffer key, aggregate values, whole copies and the three parameter modes. |
| 2026-11-15 | UTF-8: `core.substr_into` accepts all four destination/source kind combinations with the destination kind deciding (a `string` source keeps T4's boundary rule, a `bytes` source is validated as strict UTF-8, a `bytes` destination copies raw); the source kind is the `SUBSTR` `b` field, so no opcode was added and the ABI v1 table is unchanged. `stdlib` gains `utf8_decode_into` (T3), `utf8_len`, `utf8_decode` and `utf8_valid` as ordinary language code, `samples/lang/utf8.ig` covers 1-/2-/3-/4-byte code points and every malformed class, and `samples/apps/utf8` runs as `run utf8` in the shell. The `stdlib.substr_into` move is withdrawn: the destination capacity must be a call-site immediate and there are no generics. Also fixed a latent codegen bug where `dst := ""` left a destination reference on the operand stack. |
| 2026-11-15 | SMP: `smp::start` brings the APs up by attempt (INIT-SIPI-SIPI through the `0x8000` trampoline and `0x9000` parameter page, `MAX_CORES` 8, per-core TSS/IST and 64 KiB stacks, bounded 100 ms), every online core runs the claim/quantum/release loop with per-core verifier scratch and counters, idle cores do one `sti; hlt` step woken by the `0x40` IPI, the boot core stops the APs and prints per-core totals, and `syslib.cpu_id()` (builtin 10) reports the APIC id. Fixed two SMP races: a claimed task's slot stays reserved by id, and a reserved request id stays live while it compiles. `samples/apps/worker` + `cores` prove distribution; `xtask run` defaults to `-smp 4` (`--cores N`). |
| 2026-11-15 | Realtime class implemented (ADR 0016): `core.realtime_on()`/`core.heartbeat()` (builtins 11/12) through `Host::realtime_on`/`Host::heartbeat` defaults, admission by a CAS counter against `smp::online_count()`, realtime-first claim, no quantum preemption (an inner per-core loop keeps the claimed value), no total budget, a one-second deadline in non-blocked `budget_ns` (`HEARTBEAT_NS`) terminated as `Faulted(WatchdogTimeout)`, and admission released only in `terminate`. `samples/apps/{realtime,beat,spin}` prove promotion, `Full` on `-smp 1`, and the watchdog; the shell reports `WatchdogTimeout`. |
| 2026-11-15 | Streams implemented (spec 0006, amended): `in_stream[N]`/`out_stream[N]` top-level state variables whose name is the stream name; handles occupy the first `8 * endpoint_count` state bytes and the manifest lists descriptors in declaration order. The loader opens or pairs endpoints before placement (waiting semantics for an absent peer; same-direction duplicate, capacity mismatch and closed-stream declarations are load errors), writes handles into `state`, and rolls back on failure. A kernel table of 32 records over a 256 KiB static arena with a first-fit free list holds the buffers; handles are `(generation, index)`. `core.send`/`try_send` and `core.recv`/`try_recv` are four new opcodes; blocking is `WaitFor::Stream(handle)` with sends waking receivers, receives waking senders and close waking both. `samples/apps/{streams,pipe_in,pipe_out}` prove in-order delivery and `Closed`; `streams_bad` proves the two load errors; `bad_send`/`bad_recv`/`bad_name` cover the compile errors. |
| 2026-11-15 | `stdlib` framing layer (spec 0006, "Receive" and "Interfaces"): `recv_exact`, `send_frame`, `recv_frame_into`, `send_line`, `recv_line_into`, `send_u64`, `recv_u64` and `send_bytes` as ordinary language code over `const in_stream[1]`/`const out_stream[1]` parameters (the declared capacity of a stream parameter is not checked, so one helper serves every capacity). Each helper stages into a local `bytes[256]` and publishes the value with one copy, so a failure leaves the destination untouched; `send_frame`/`send_line`/`send_u64` build the whole message and send it atomically; `recv_line_into` strips CRLF and keeps consuming an overlong line to stay synchronised. Receives are one byte wide: `core.recv` returns up to the destination capacity and there is no pushback, so a wider chunk would steal the peer's next message (the spec's original loop was corrected). `pipe_out`/`pipe_in` now use `send_line`/`recv_line_into`; `samples/apps/frames` (`frame_in` + `frame_out` on `frames`/`frames_ack`) proves the length prefix, the `u64` view, both `CapacityExceeded` paths and the close, and the host `ConsoleHost` has a loopback FIFO so `langhost` tests the views deterministically. Also fixed an SMP bug this exposed: AP kernel stacks were 64 KiB, but any core may compile a request and a `stdlib`-using compile needs ~60 KiB, so the compiler overflowed an AP stack into the neighbouring `.bss` (random `#UD`/`#PF`). `AP_STACK_BYTES` is now 256 KiB, matching the boot core. |
| 2026-11-16 | Stack guards implemented for boot, AP and task stacks (spec 0002 H2, spec 0004 C6, ADR 0006). `arch/x86_64/src/paging.rs` gains `split_huge_page`, `set_pte_present`, `pte_present`, `flush_page` and `reload_cr3`; `kernel/src/guard.rs` owns a 32-entry 4 KiB table cache keyed by 2 MiB base and a 96-frame registry, and `install_guarded` moved there from `arch` (still the boot stack's entry point). Each `ApStack` is now guard + 256 KiB body; all seven AP guards are installed and PTE-checked before the SIPIs, and `smp::start` prints `guards: boot 1, ap 7, tables M`, halting on a mismatch or a changed free-frame count. Every PTE change does a local CR3 reload plus a generation-tagged wake-IPI TLB shootdown and bounded wait. Task placement allocates `stack_len + 4096` contiguous frames, the lowest is the guard, `regions.stack` starts one frame above with the exact manifest length, and `Region::free` unguards before `free_contiguous` on every destruction path. `#PF` prints `stack guard: task N` / `ap core N` / `boot core` before the dump. `samples/apps/deep` proves `run deep` reports `child: faulted StackOverflow` (the interpreter path, not the hardware guard). |
| 2026-11-16 | Fixed-point `f64` text (spec 0003, "Floating-point text"; spec 0008). `core.format_f64_into(dst, value, frac_digits) -> int` is one new `FORMAT_F64` opcode (`a` = destination buffer kind, a non-zero capacity immediate; destination reference, `f64` bits and `frac_digits` on the stack) implemented as a pure buffer operation in the interpreter: it builds the digits in a local array and publishes them with one copy, so a rejected value never leaves a partial destination. At most 9 fraction digits (`-4` beyond), half-away-from-zero rounding with the carry applied to the integer part (`9.999` with 2 → `10.00`), `nan`/`inf`/`-inf` written literally, `-4` for `\|value\| >= 2^63` because the form has no exponent, and `-6` when the result does not fit (destination length untouched). The sign of `-0.0` survives (`-0.000`). `stdlib.parse_f64_into(src, var value) -> Status` is ordinary language code: `[+-]? ( digits [ '.' digits? ] \| '.' digits ) ( [eE] [+-]? digits )?` plus the formatter's `inf`/`+inf`/`-inf`/`nan`; everything else is `Invalid` and writes nothing. The fraction accumulates as an integer numerator over a power of ten (18 significant digits, so `0.375` is exact) and the exponent is applied by repeated ×/÷10 with an early exit at saturation, so `1e400` is `±Inf` and `1e-400` is `±0`, both with `Ok`. Tests: interpreter hex/float formatter cases (specials, carry, `-0.0`, `2^63`, capacity and kind errors), compiler arity/type/encoding cases, `langhost` valid/malformed/round-trip cases, the stdlib helper count (21), `samples/lang/f64text.ig` on the host and `samples/apps/floats` in the kernel (`run floats 3.25` → `value=3.250`, `run floats nope` → `Invalid`). ABI v1 grows from 71 to 72 opcodes; Q41 narrows to shortest round-trip formatting and exact IEEE parsing. |
| 2026-11-16 | Shortest round-trip `f64` text (spec 0003, "Floating-point text"; spec 0008; Q41). `core.format_f64_short_into(dst, value) -> int` is one new `FORMAT_F64_SHORT` opcode (`a` = destination buffer kind, a non-zero capacity immediate, no width operand) implemented as an exact search in `interpreter/src/f64short.rs`: the value and both ends of its round-trip interval are expanded to base-`10^9` integers (the value times `5^k` and `2^shift`; at most 769 digits, 88 limbs, `5^27` and `2^30` chunk multiplication), then for `n = 1..=17` the two `n`-digit decimals that bracket the value are compared against the interval endpoints, with endpoint inclusivity from the significand's low bit (round-half-to-even) and the lower half-gap halved at a power-of-two boundary. The first `n` that lands inside wins; when both candidates do, the nearer one is taken and an exact tie rounds away from zero, matching the host reference. Shape: positional while `-6 < e10 + 1 <= 21` (`1e20`, `1e-6`, `0.000001`), scientific otherwise (`1e21`, `1e-7`, `5e-324`, `1.7976931348623157e308`), no trailing `.0`, `0`/`-0`, `nan`, `inf`/`-inf` literal, `-6` with the length untouched when the text does not fit. The text is staged in a local array and published with one copy: no allocation, no host call, no floating-point arithmetic after the decode, and no data tables beyond a 72-byte `10^0..=10^8` array (~9.4 KiB of `.text` at `-O`, stack frame 5.7 KiB). Tests: `interpreter/tests/f64short.rs` compares the produced string against the shape rule applied to Rust's `{:e}` for 24 000+ xorshift-bit values (all finite ones) plus every power of ten and two with both neighbours, every biased exponent at eight mantissa boundaries in both signs, and a hard-value list, and independently re-parses every result; `verify.rs` covers the hand-assembled cases and the kind/capacity/reserved-field rejections; `compiler/tests/compile.rs` covers arity, destination kinds and encoding; `samples/lang/f64short.ig` runs on the host and `samples/apps/floats` prints `s:third`, `s:1e21`, `s:tinysub` and `s:max` in the kernel. The ABI v1 table holds **75** opcodes (counted in `abi/src/opcodes.rs`; the `71 to 72` figure in the row above was off by two, and spec 0008's `73 to 74` for `format_f64` was already correct). Q41 narrows to correctly rounded decimal-to-binary parsing and its subnormal edges. |
| 2026-11-16 | Graphics foundation for the M2 desktop (spec 0009, ADR 0020). `kernel/src/gfx.rs` keeps the multiboot2 mode and allocates a full-screen shadow of `pitch * height` contiguous frames after `frame::init` (1024×768×32, pitch 4096 → 3 MiB, 768 frames, `0x757000` under QEMU); `present` copies clipped rectangles row by row with the pitch. `kernel/src/console.rs` lost its direct framebuffer path: glyphs are painted into the shadow by `gfx::console_glyph` and each cell is presented; `fb.rs` is gone. Nine `syslib` graphics primitives are host builtins (ids 13–21, ABI v1 stays 75 opcodes): `screen_width`/`screen_height`, `fill_rect`, `fill_dither` (`(X + Y)` even), `draw_rect` (1-px outline), `draw_text` (8×8 font, transparent, one byte per cell, no newline), `get_pixel`, `present`, `clear`; the interpreter rejects non-positive extents and colours above `0x00FFFFFF` before the host call, and the host clips to the screen and a rectangular clip region. `samples/apps/desktop` draws a System 7.5-style dither desktop, menu bar with a right-aligned `core.clock()` HH:MM, and a striped title-bar window, checks nine pixels and prints `desktop: ok` in one claim; `run desktop` verified on `--cores 4` and `--cores 1`, and `run frames`/`run streams` still pass. Graphics disable cleanly (no tag, non-32-bpp, bad geometry, no shadow memory); the framebuffer tag is now optional and the console falls back to serial. |
| 2026-11-16 | Desktop input: spec 0010 and ADR 0021 accepted; PS/2 aux driver, pointer state and the composited cursor (M2.2.0–M2.2.2). `kernel/src/mouse.rs` initialises the i8042 aux device (`0xA8`, config aux IRQ cleared, `0xF6`, `0xF3 100`, `0xE8 3`, `0xE6`, `0xF4`, bounded waits; `mouse: PS/2 aux ready` or `mouse: no aux device`), decodes 3-byte reports with a pure `Decoder` (bit-3 sync with resync, overflow dropped whole, 9-bit sign extension, Y inverted, 1:1), and is polled in the service opportunity and the idle loop next to the keyboard, which now leaves aux bytes for it. A boot self-check feeds canned packets (positive/negative deltas, an overflow, a desynced stream, both clamps, all three buttons) through the same decoder and clamp arithmetic on scratch state and prints `mouse: decoder ok`, halting on a mismatch. One machine-wide pointer state and a 16×16 black-outline/white-fill arrow live in `kernel/src/gfx.rs`; `present` copies the shadow and then composites the cursor into the framebuffer (never into the shadow), so `get_pixel` returns uncomposited pixels and headless checks stay deterministic; a move re-presents only the old and new cursor rectangles and the console's cell present carries the cursor. `syslib` gained `pointer_x`/`pointer_y` (`int`), `pointer_buttons` (`uint`) and debug-only `inject_pointer(dx, dy, buttons)` (builtins 22–25, ABI v1 stays 75 opcodes, host defaults 0/0/0 and `Invalid`; the interpreter rejects `buttons > 0b111`). `samples/apps/mouse` proves clamping at both edges, button set/clear and the "cursor is not in the shadow" rule. `run mouse`, `run desktop`, `run text 42 hi`, `run frames`, `run streams` and `run realtime` pass on `--cores 4`, `run mouse` on `--cores 1`; 290 host tests plus the doctest. |
| 2026-11-16 | Absolute pointer through QEMU's `usb-tablet` (M2.2.6; spec 0010, ADR 0022). `kernel/src/pci.rs` scans bus 0 through 0xCF8/0xCFC, logs one line per function, finds the PIIX3 UHCI controller (`8086:7020` at 0:1.2, class `0c.03.00`), enables I/O and bus mastering and takes BAR4 (`0xc040` under QEMU). `kernel/src/usb.rs` resets the controller (`HCRESET`, `FLBASEADD`, `FRNUM`, run/configured/maxp 64), resets and enables the port (QEMU needs the explicit `EN` on every PORTSC write), and enumerates at address 1 with control transfers over static QHs/TDs: device descriptor (`0627:0001`, bcdUSB 0200, maxp 8), `SET_ADDRESS(1)`, configuration (32 bytes: HID interface 3/0/0, endpoint `0x81` maxp 8 interval 10), `SET_CONFIGURATION(1)`, HID `SET_IDLE(0)`, and a 64-byte report-descriptor read that verifies the absolute 16-bit X/Y shape (descriptor declares 74 bytes). The measured report is buttons (bits 0–2), X LE16, Y LE16 (both `0..=32767`, top-left origin) and a wheel byte; it is scaled to `0..=w-1`/`0..=h-1` with `(2*v*(w-1)+32767)/65534` and applied with `gfx.set_pointer_absolute` (clamped, idempotent, old/new cursor rectangles only). One self-linked interrupt-IN TD is polled in the service opportunity and idle loop; NAK leaves it active, completion re-arms it with the next toggle, and a transfer error falls back to PS/2. PS/2 motion is ignored while the tablet is ready; `mouse::ready()` and `gfx::set_pointer_source` keep the fallback. `syslib.pointer_source()` is builtin 26 (0 none / 1 PS/2 / 2 tablet; ABI v1 stays 75 opcodes). Static `.bss` cost is 4096 bytes for the frame list plus 240 bytes of QH/TD/buffers. `xtask run` adds `-usb -device usb-tablet` (`--no-tablet` for the negative); `scripts/tablet_qmp.py` injects QMP absolute events and checks guest pixels. Boot: `pci:` lines, `usb: UHCI ...`, `usb: USBSTS ... PORTSC1 ... PORTSC2 ...`, `usb: tablet 0627:0001 addr 1 (abs X/Y)`, enumeration 76–77 ms under TCG; without the device `usb: no tablet (no device); PS/2 only` and `pointer_source` 1. QMP cases: (16384,16384)→512,384; (32767,32767)→1023,767; (0,0)→0,0; (0,32767)→0,767; (32767,0)→1023,0; (8192,16384,left)→256,384,1; all 7/7 on `--cores 4` and `--cores 1`. Regressions `mouse`, `desktop`, `text`, `frames`, `streams`, `realtime` pass in the same boot. Q49's burst loss and host scale mismatch are resolved for the tablet; M2.2.7 (aux IRQ12) remains for the relative fallback. |
| 2026-11-16 | Keyboard key events and the event queue (M2.2.3–M2.2.5; spec 0010 I18/I19/I29–I31, spec 0003, spec 0008). `arch/x86_64/src/keyboard.rs` now drains each controller byte once and reports a console byte and a `KeyEvent` together: a pure `decode` maps set 1 to a stable key code (unshifted ASCII, letters uppercase, or `abi::key` `0x100` codes for Escape/Backspace/Tab/Enter/arrows/Home/End/Page/Insert/Delete/F1–F12/modifiers), the text under Shift/Caps/Ctrl, and the Shift/Ctrl/Alt/Caps state; the `0xE1` Pause tail is skipped. `decoder_self_check` feeds canned scancodes through the same function at boot (`keyboard: decoder ok`). `kernel/src/event.rs` owns one 64-event machine-wide FIFO; a full queue drops the newest event and increments a monotonic counter, and an insertion drops the queue lock before waking every `WaitFor::Input` task. `gfx` enqueues `PointerMove`/`PointerDown`/`PointerUp` on pointer transitions (changed button bits only), so `inject_pointer` events are deterministic. `EventKind` (`PointerMove`/`PointerDown`/`PointerUp`/`KeyDown`/`KeyUp`) and `Event` (`kind,x,y,code,window`, 40 bytes) are predeclared at `abi::layout::event`; key events pack `text \| modifiers << 8 \| key code << 16` (`abi::key`). Builtins 27–31 (`next_event`, `poll_event`, `inject_event`, `event_drops`, `event_flush`) add no opcode; `next_event` blocks with `WaitKind::Input`, and `sched` re-checks the queue after the block. `samples/apps/events` + `events_poke` prove the surface, the newest-six-of-seventy overflow and a genuinely blocked wake (`events: ok` on `-smp 4` and `-smp 1`); `mouse`, `desktop`, `text`, `frames`, `streams`, `deep`, `utf8`, `realtime` and `cores` still pass, and `scripts/tablet_qmp.py` stays 7/7. Host tests: 301 (abi 14, compiler 134, interpreter 94, langhost 5 plus doctest). |
| 2026-11-16 | PS/2 aux IRQ12 (M2.2.7; spec 0010 I1/I14/I15/I32/I33, ADR 0023). `arch/x86_64/src/ioapic.rs` splits the `0xFEC00000` 2 MiB page once, marks the register page `PWT \| PCD`, reads `IOAPICVER` (version `0x20`, 24 entries) and programs redirection entry 12 as fixed/physical/active-high/**level**/unmasked to the boot core's APIC id on vector `0x2C`; `idt::init` installs the `irq_aux` gate and the stub calls `aux_irq` → `mouse::on_irq`. The handler drains at most 64 aux bytes, pushes them into a 1024-byte SPSC atomic ring (`AtomicU8` slots, producer head, consumer tail under the decoder lock), counts interrupts/bytes and EOIs only the local APIC; it takes no kernel lock and prints nothing. `mouse::poll` now drains the ring and decodes it in the service opportunity; the direct controller drain is the bounded backstop and is not executed while IRQ12 is active, so an aux byte has exactly one reader (this also removes the status/data read race across cores). `mouse::enable_irq` stops reporting (`0xF5`), re-reads the configuration, re-enables reporting (`0xF4`) and writes `config \| 0x02` **last**, because once bit 1 is set the handler would steal the `0xF4` ACK; the configuration value is written to the data port (the previous code wrote it to the command port, so bit 1 never persisted). Boot: `ioapic: version 0x20, IRQ12 -> vector 0x2c on apic 0 (level)`, `mouse: aux IRQ12 enabled (config 0x41 + aux irq, ...)`, one-time `mouse: aux IRQ12 delivered N interrupt(s), M byte(s)`; absent/unusable IOAPIC or a failed write logs `... polling` and keeps the old path. `scripts/mouse_burst_qmp.py` (with `--no-tablet`) clamps at x=0 and injects 300×(+2) as one QMP batch: 600/600 px on `-smp 4` and `-smp 1`; the polled HEAD is also 600/600 headlessly because QEMU 10.2.2 accumulates PS/2 deltas in `mouse_dx` and defers packets, so the historical host-window 346/600 is not reproducible through QMP (recorded in Q49). Regressions `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores` pass on `-smp 4` and `-smp 1`; `scripts/tablet_qmp.py` is 7/7 and `--no-tablet` reports `mouse: ok`; 301 host tests. |
| 2026-11-16 | Spec 0011 "Windows" accepted (M2.3.0; ADR 0024). The window model is fixed before implementation: a window is a record in a 16-entry kernel table with monotonic `int` ids (`0` invalid, never reused); `window_open(x, y, w, h, title, var id)` takes the frame origin and the content size, so the frame is `(w + 2, h + 17)` with a 1-px border and a 16-px title bar; the z-order is bottom-to-top with exactly one focus, `open`/`raise` focus, closing the focused window falls back to the topmost live one, and `Task::reclaim` closes a task's windows on every destruction path; `close`/`move` are owner-only (`Invalid` for a live foreign window) while `raise` is allowed to any task; limits are 16 windows, a 64-byte title truncated at a code-point boundary, and an 8×8 minimum content, with the frame origin clamped on screen; geometry is read through the predeclared `Rect` record (`x, y, w, h: int`, 32 bytes, `abi::layout::rect`); the shadow clip stays service-owned (internal save/narrow/restore, no application primitive) and `Expose` asks for the whole content rectangle (M2.3.2/M2.3.3). Disabled graphics make `open`/reads `Invalid`, `close`/`raise`/`move` `NotFound` and `focus` 0. Spec 0011, ADR 0024. |
| 2026-11-16 | M2.3.1 window records and lifecycle (spec 0011, ADR 0024). `kernel/src/window.rs` owns a 16-entry table (`MAX_WINDOWS`) with monotonic `u64` ids behind one `SpinLock`: `open` validates the 8×8 content minimum and screen fit, clamps the frame origin, truncates the 64-byte title at a code point, appends to the top of the z-order and focuses; `close`/`move_to` are owner-only (`Invalid` for a live foreign window), `raise` is any task's and focuses, `close_task` removes a terminating task's windows with the topmost fallback; the frame is `(w + 2, h + 17)` and the content `(x + 1, y + 16)`. `Task::reclaim` calls `window::close_task` on every destruction path, before the regions are freed. Builtins 32–38 (`window_open` 6→1, `close` 1→1, `raise` 1→1, `move` 3→1, `frame_rect`/`content_rect` 2→1 writing a 32-byte `Rect`, `focus` 0→1) with disabled-service `Host` defaults, compiler signature + `Rect` checks, and interpreter arm tests; `Rect` is predeclared (record id 2, `abi::layout::rect`, size 32) and cannot be redeclared. `gfx` gained the kernel-internal `set_clip`/`reset_clip` (no application binding) and a pre-console clip self-check; `window::self_check` runs the policy on a scratch table at boot. Proof: `samples/apps/windows` + `windows_child` (a child opens its own window, is refused `close`/`move` on the parent's, then exits and its window disappears with the focus fallback) prints `windows: ok` on `-smp 4` and `-smp 1`; boot prints `graphics: clip ok`, `window: rules ok`. 312 host tests (abi 15, compiler 139, interpreter 113, langhost 45, including doctests). Regressions `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores` pass on both core counts; `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`, `scripts/mouse_burst_qmp.py --cores 1 --batch 300` 600/600. Chrome, damage, damage-union `present` (M2.3.2) and `Expose` routing (M2.3.3) are not implemented. |
| 2026-11-16 | M2.3.2 window composition and repair (spec 0011, W11; ADR 0020/0024). `kernel/src/window.rs` records damage on every mutation in two bounded 32-rect lists: `Erase { rect, covers_from }` (desktop restore, with a per-entry z-order coverage limit so an opened window's whole frame starts as desktop, a moved window's new frame subtracts only windows above it, and a closed/moved-away frame subtracts every live window) and a chrome rectangle list. One synchronous `repair` under the table lock (window → graphics, no path takes graphics then the table) resets the clip, paints the base white and the `(X + Y)` black dither in each erase's visible parts, then redraws the chrome of every window whose frame intersects the damage, bottom-to-top, clipped to the exact visible parts (frame minus higher frames) intersected with the damage, and presents the bounding-box union once (the cursor is composited as before). The chrome is service-drawn: a 1-px black outer border, a 16-px title bar (white with 1-px black stripes when focused, plain `0xCCCCCC` when not), an 11×11 close box at `(x+4, y+3)`, an 8×8 centred title truncated to the band and its own fill, a separator at the last title row; the content rectangle is never touched. `move` to the same origin and every refused call record nothing; `close_task` accumulates all removed frames and repairs once. Boot adds `window: paint ok`: `paint_self_check` proves list merging, visible-part subtraction (area-exact, never touching the subtracted frame) and the damage each lifecycle operation records, all on scratch state. Proof: `samples/apps/windows_demo` composes three overlapping windows (Alpha/Beta/Gamma) with distinct content colours, moves Beta and raises Alpha, repaints contents and rotates the z-order so every chrome is repainted in its visible parts, checks eight chrome/content pixels, prints `windows_demo: checks ok` / `windows_demo: ready`, repaints after the console scroll and holds 15 s as a realtime task (heartbeats; fallback short hold if admission is full) before `windows_demo: ok`. `scripts/windows_screenshot_qmp.py` boots headless without a tablet, runs the demo, waits for ready, and captures the framebuffer with QMP `screendump` (PNG, PPM fallback) to `/home/sinan/Desktop/igis-windows.png` (1024×768, 11078 bytes). 313 host tests (abi 15, compiler 139, interpreter 113, langhost 46, including doctests). Regressions `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores`, `windows` pass on `-smp 4` and `-smp 1`; `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`, `scripts/mouse_burst_qmp.py --cores 1 --batch 300` 600/600. Expose, event routing and `window_present` remain M2.3.3. |
| 2026-11-16 | M2.3.3 event routing, `Expose` and `window_present` (spec 0011, W12/W16/W17; spec 0010). `abi::EventKind` gains `Expose = 5` (compiler predeclared variants, `from_ordinal`, layout tests). Producers (`event::push_pointer`, `push_key`) still write `window = 0` because the pointer path runs under the graphics lock and the repair holds window → graphics. The kernel host routes on `poll_event` with no graphics lock: `window::take_expose(current)` first (topmost pending window of the caller, flag cleared, 40-byte Expose with `window = id`, `x = y = code = 0`), otherwise `event::poll`; a `PointerDown` is hit-tested against the topmost frame, `window::route_click` raises/focuses and repairs it (window → graphics) and the event names it; other pointer/key events name `window::focus()`; an event with a non-zero `window` (an injected `Expose` or addressed injection) is passed through. `Table::repair` marks one coalesced expose on every window whose frame intersects the chrome damage before painting and returns whether it did; wrappers drop the window lock and then call `task::wake_input`, and the scheduler's `WaitKind::Input` block re-check also tests `window::has_expose(task_id)`, closing the empty-read/block race. `close_task` only records damage and the final focus title when it actually removed a window, so a task that owns none cannot spuriously expose the focused window. `syslib.window_present` is builtin 39 (1 → 1, owner-only; `NotFound` unknown, `Invalid` foreign/disabled) and presents the content minus higher frames part by part with `set_clip`/`present`/`reset_clip`; `syslib.present` remains the generic primitive. Compiler signature + tests, interpreter arm + tests, disabled-service `Host` default `Invalid`. `window: route ok` (`route_self_check`) checks expose marking, owner isolation, topmost-first take order, hit-testing and expose loss on close on scratch state. Proof `samples/apps/windows_events` (+ `windows_events_child` for foreign expose/present refusal and `windows_events_open` for close-on-termination, using `stdlib.parse_int_into` for the parent id): pointer move to the focus, click-to-raise-before-delivery, key routing, Expose fields and a repaint with `window_present`, C-above-A visible-part repaint with chrome/content checks, foreign isolation, child close with focus fallback and expose; prints `windows_events: ok` on `-smp 4` and `-smp 1`. Boot lines `graphics: clip ok`, `window: rules ok`, `window: paint ok`, `window: route ok`; `windows`, `windows_demo` still pass. 316 host tests (abi 15, compiler 140, interpreter 114, langhost 47, including doctests). Regressions `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores`, `windows`, `windows_demo` pass on both; `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`, `scripts/mouse_burst_qmp.py --cores 1 --batch 300` 600/600. M2 policy: a client that ignores an `Expose` leaves stale content and is not closed (W18); M2.3.4 is the desktop shell, M2.3.5 the two-app/drag proof. |
| 2026-11-16 | M2.3.4 desktop shell and kernel text windows (spec 0012, ADR 0025; spec 0011 W19/W20). `kernel/src/console.rs` is rewritten: after `gfx::init` it paints the white + `(X + Y)` dither desktop and opens two owner-`0` windows — `Terminal` at `(8, 26)` and `System Log` beside it — with content derived from the reported screen (62×89 cells at 1024×768, 16 KiB grid cap each, scale-1 glyphs, black on white / light grey). `println!`/`print!` feed the log; `core.write` and the line-discipline echo feed the terminal; both still mirror every byte to COM1 and `core.read_line` is unchanged. Each grid has its own cursor and per-window scroll (`copy_within`, clear the bottom row, repaint); a cell is drawn and presented only in the intersection of the cell, the content rectangle and the window's visible parts, using the new **explicit-clip** helpers `gfx::draw_cell`/`gfx::present_within` (`gfx::Clip`, `Screen::clipped_by`/`put_in`/`fill_in`/`glyph_in`/`present_in`) and never the shadow's global clip, so a long grid repaint cannot clip another task's drawing. `console::pump` drains `window::take_expose(0)` at the end of `console::poll_input` and repaints the matching grid; order stays `CONSOLE → WINDOW → SCREEN` and the repair never calls the console. `console::route_self_check` writes `console: terminal ok` through `write_bytes` and `console: log ok` through `_print`, checks both grids and a foreground/background glyph pixel with `get_pixel`, clears them, and `kmain` prints `console: route ok` (or `console: serial only`). The whole-screen `gfx::console_clear`/`console_glyph`/`console_scroll` and `Screen::scroll` are removed. `window::open` admits owner `0` (task ids start at 1), `window::info`/`Table::info` return a topmost-first snapshot, `window::visible_content_parts` serves the console, and `self_check`/`route_self_check` cover owner 0 and enumeration order. `syslib.window_info(index, var id, var rect, var title[64])` is builtin 40 (`(4,1)`, no opcode): the interpreter validates the title destination (cap 64 compile-checked in `compiler/src/codegen.rs`, region-bounds-checked in `interpreter/src/machine.rs`), writes id/rect/length+title only on `Ok`, and the disabled `Host` default is `None` → `NotFound`. `samples/apps/desktop_shell` (menu bar: Apple square, File/Apps/Windows, right-aligned clock, 22 rows; one `Desktop` window at `(600, 72)`; File lists windows into the terminal, Apps spawns `clock` with `core.spawn`, Windows raises the terminal, Apple raises the shell; check mode injects a pointer-down per item; normal mode promotes realtime, prints `desktop_shell: ready`, pumps `next_event` and heartbeats; it never calls `syslib.clear`). `samples/apps/{events,windows,windows_events}` record the boot focus baseline (the kernel windows exist) and `events` uses the menu-bar row `(40, 10)` so a click hits no window; `scripts/desktop_screenshot_qmp.py` captures `/home/sinan/Desktop/igis-desktop.png` (1024×768, ~31 KB) through QMP `screendump`, and the harness samples nothing. 320 host tests (abi 15, compiler 141, interpreter 117, langhost 48, including doctests); `cargo xtask build`/`image` clean. Boot on `-smp 4` and `-smp 1`: `console: terminal ok`, `console: log ok`, `console: route ok`, `graphics: clip ok`, `window: rules ok`, `window: paint ok`, `window: route ok`; `run desktop_shell check` → `desktop_shell: ok`, `run desktop_shell` → `desktop_shell: ready`. Regressions `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores`, `windows`, `windows_demo`, `windows_events` pass on both; `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`, `scripts/mouse_burst_qmp.py --cores 1 --batch 300` 600/600. Remaining: M2.3.5 (two apps side by side, focus switch, raise, drag, expose redraw headless). |
| 2026-11-17 | M2.3.5 two-application proof (spec 0011; spec 0007 Sch12; spec 0012 D3). `samples/apps/windows_proof` spawns `windows_proof_a` and `windows_proof_b` (separate tasks, one window each) and owns a third `Proof Control` window; a child reads the control window's frame `y` through `window_info` as a persistent command stage, because only `window_raise` is cross-task writable and the shared event queue cannot carry an addressed command reliably. The parent raises the control (any-task raise) to establish a non-app focus, then injects real pointer clicks over A and B (`inject_pointer` clamp→move→press→release); the poll-time routing raises/focuses before delivery, and the proof asserts `window_focus()` and the topmost index plus the striped/grey title bar, not which task received the event. Stage 1: A performs four owner `window_move` calls; the proof waits for the final frame and checks the new content/border and that the old area is white again (the terminal below is repainted by `console::pump`). Stage 2: B (topmost) moves over A; A takes its owner-only whole-content `Expose`, repaints only the part left of B's frame (M2 has no per-task clip) and presents; the overlap shows B, A's visible strip and border stay A. Stage 3: B moves back; A takes the new expose and repaints the previously covered content (checked at a pixel that was B's), and the vacated log/terminal area is restored. Stage 4: both children close their windows and exit `Ok`, acknowledged by `core.wait`; the control closes and the focus falls back to the entry baseline. Two prerequisites fixed: `task::claim_class` now scans from a global `NEXT_CLAIM` cursor and stores the claimed slot + 1 (the old always-from-slot-0 scan starved the higher-slot children on `-smp 1`, leaving the proof stuck at "both app windows exist"); and `window` carries a visible-geometry `generation` bumped by open/close/raise/move/close_task, which the console's `paint_cell`/`repaint` read before `visible_content_parts` and compare after painting, retrying (4×) with fresh parts—the proof reproducibly caught the old log-repaint overwriting B after B had moved above it. `run windows_proof` → `windows_proof: ok` on `-smp 4` and `-smp 1`; `windows_proof shot` prints `ready` and holds 3 s; `scripts/windows_proof_screenshot_qmp.py` writes `/home/sinan/Desktop/igis-m23-proof.png` (1024×768, 30901 bytes). 323 host tests (abi 15, compiler 141, interpreter 117, langhost 51, including doctests); `cargo xtask build`/`image` clean; boot self-checks unchanged; regressions `desktop_shell` (check), `windows`, `windows_events`, `windows_demo`, `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores` pass on both; `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`, `scripts/mouse_burst_qmp.py --cores 1 --batch 300` 600/600; desktop/windows screenshots still 1024×768. Pointer-driven continuous drag remains M2.4. |
| 2026-11-17 | M2.4.0 `stdlib` UI layout and chrome layer (spec 0003 "UI layout and chrome"; spec 0008; spec 0009 G19; ADR 0026). The embedded library now says `uses core, syslib;` and adds 18 ordinary-language helpers with no new opcode, builtin, `state`, record, sum or user enum: metrics `ui_text_width` (code points × 8 via `utf8_len`), `ui_text_height` 8, `ui_line_height` 16, `ui_content_inset` 8, `ui_title_bar_height` 16, `ui_close_box_size` 11, `ui_text_center_x`/`_y`; palette `ui_black` 0x000000, `ui_white` 0xFFFFFF, `ui_gray` 0xCCCCCC, `ui_focus` 0xFFFFFF, `ui_accent` 0x000080 (functions because `stdlib` constants are not exported); client chrome `ui_frame` (1-px black outline), `ui_title_bar` (frame-origin geometry, white/striped focused or plain 0xCCCCCC interior, 11×11 close box at `(x+4, y+3)` when the interior is at least 14 px, code-point-safe 64-byte centred title with its own band fill, separator at `y+15`, `w < 3` → `Invalid`), `ui_close_box` (outline only), `ui_separator`, `ui_center_text` (left/top aligned when it does not fit). The title uses a local `string[64]`; the helpers never call the window service, so the service's own chrome is unchanged. The library source is 35,647 B and needs 1,157,496 B of compile scratch, so `MAX_ARENA_BYTES` rose from 1 to 2 MiB (provisional cap; `compile_into_reports_its_caps` and the cost test updated). `samples/apps/ui_layout` opens a window at `(120, 120)` with content 360×220, fills it, draws a framed dialog with a focused title bar, a centred "OK" button, an unfocused bar and an accent swatch, calls `window_present` and checks 15 shadow pixels (content white, top/bottom borders, stripe on/off, separator, close box outline/interior, unfocused fill/no-stripe/box, button border/interior/glyph `0x1C` bit 2, accent) before closing; it prints `ui_layout: ok` on `-smp 4` and `-smp 1`. Without graphics it checks the metrics/palette and that all five drawing helpers return `Invalid`, printing `ui_layout: host ok` (the `langhost` test asserts it). Tests: compiler pins the 39 library functions and an app's use of every helper, and checks a record/sum argument is still rejected across the unit; a langhost fake host records the exact title-bar call sequence (background, seven stripes at rows 22..34, box, separator, band, text at `(62, 24)`) and the metrics output. **328 host tests** (abi 15, compiler 143, interpreter 117, langhost 53, including doctests), `cargo xtask build`/`image` clean; boot self-checks unchanged; `ui_layout: ok`, `windows_proof: ok`, `desktop_shell: ok`, `windows: ok`, `windows_events: ok`, `windows_demo: checks ok/ready/ok`, `desktop`, `events`, `text 42 hi`, `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores` pass on `-smp 4` and `-smp 1`; `mouse: ok` in its own boot and through `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`, `scripts/mouse_burst_qmp.py` 600/600. Note: running `mouse` after a long window-app sequence can fail its background-pixel check because its whole-screen `syslib.clear` races a console `Expose` repaint on another core; that is the pre-existing full-screen-clear limitation (spec 0012), not an M2.4.0 regression, and the QMP harness runs it in a fresh boot. No screenshot for this slice: the host and kernel pixel checks are the proof. M2.4.1 string editing is next. |
| 2026-11-17 | M2.4.1 `stdlib` string editing and selection (spec 0003 "String editing", SE1–SE7; spec 0005 Q44). 13 ordinary-language helpers with no new opcode, builtin or `state`: `str_boundary`, `str_cursor_clamp`/`left`/`right`/`home`/`end`, `str_insert_into`, `str_delete_into`, `str_backspace`, `str_selection_normalize`/`all`/`collapse`/`replace`. Positions are byte offsets and both ends of an insert/delete range must be code-point boundaries (`Invalid`, text untouched); a mutator stages the result in a local `bytes[256]`, converts through `core.substr_into` (strict UTF-8) and publishes one whole-buffer copy, so no partial buffer is ever visible. The 256-byte working capacity returns `CapacityExceeded` without a write (delete never grows, so there it only reports an incoming length above 256). Cursors clamp (negative → 0, past the end → the length, mid-code-point → that code point's first byte) and move whole code points; backspace at 0 is an `Ok` no-op; the selection is the scalar pair `start`/`stop` (normalize clamps and orders, select-all, collapse, replace). `samples/apps/text_edit` proves ASCII and 1-/2-/3-/4-byte code points, every rejection and capacity path and the selection helpers, printing `text_edit: ok` on `-smp 4` and `-smp 1` and the same transcript on `langhost`; the library is 46,933 B source / 32,600 B code / 1,319,272 B arena (52 functions, 2 MiB cap). 330 host tests (abi 15, compiler 144, interpreter 117, langhost 54). Q44 narrows: the language side exists; the kernel keeps `core.read_line` and its 4-line ring until the terminal window (M2.5.5). |

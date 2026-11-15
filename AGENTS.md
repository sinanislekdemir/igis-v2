# AGENTS.md

Operational guide for humans and coding agents working in this repository.
Keep this file short and current. It points at documents; it does not duplicate them.

**Last updated:** 2026-11-16

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
- **Status:** implementation. Specs 0001–0008 are accepted. The kernel boots under
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
  `run floats nope` → `Invalid` in the kernel. The next pieces are recovery from a hardware
  guard hit (Q47) and the remaining float-text refinements (Q41); explicit close is v2.

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

## Repository layout

```
.
├── AGENTS.md          # this file: operational facts and conventions
├── IDEAS.md           # unstructured inbox
├── OPEN-QUESTIONS.md  # parked decisions and unresolved questions
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
| Boot: INIT-SIPI-SIPI through a sub-1 MiB trampoline, `MAX_CORES` static state; framebuffer tag mandatory; one IPI vector | specs 0002, 0007 |
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
| Float text is fixed-point in the milestone: `format_f64` is one opcode (destination kind + capacity immediate), 0–9 fraction digits, half away from zero with carry, `nan`/`inf`/`-inf` literals, `-4` for a wider field or `\|value\| >= 2^63`, `-6` for a short destination; `stdlib.parse_f64_into` is language code over a decimal grammar that saturates overflow/underflow to `±Inf`/`±0` | spec 0003, spec 0008; Q40/Q41 |

## Open questions

See [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) for the remaining parked items: tuning
constants, the deferred debug/DMA/streams work, and the float follow-ups. Boot, platform,
scheduler, REPL and language decisions are settled.
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

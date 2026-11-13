# AGENTS.md

Operational guide for humans and coding agents working in this repository.
Keep this file short and current. It points at documents; it does not duplicate them.

**Last updated:** 2026-11-13

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
  1024-slot static queue is serviced before every claim, `core.spawn` returns a reserved
  `TaskId` immediately, `core.wait` blocks the parent (a `Blocked` slot skipped by
  `claim`) until the child's exit record wakes it, and a 64-entry exit ring serves `wait`.
  These three primitives use a staged scalar `int` ABI (`Status`/`ExitReason`/`Diagnostic`
  are not language values yet; spec 0005, "Staged ABI"). The next pieces are console input,
  the record-based spawn ABI, then APs, the realtime class and streams.

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
| Streams are single-producer/single-consumer; names are unique per application; framing helpers live in `stdlib` | spec 0006 (Draft) |
| Floating point in the first milestone: one `f64` type, IEEE-754, with a kernel-owned per-task `fpu` region | ADR 0018 (Accepted) |
| No visibility modifiers: every top-level declaration is public; conversions are `T(x)` casts; maps use FNV-1a with linear probing | specs 0003, 0004 |
| Boot: INIT-SIPI-SIPI through a sub-1 MiB trampoline, `MAX_CORES` static state; framebuffer tag mandatory; one IPI vector | specs 0002, 0007 |
| ext2 subset: 1/4 KiB blocks, double indirect, regular root files, metadata validated; unsupported images refuse the mount | specs 0002, 0007 |
| `ExitReason.Killed`; dispatcher before every claim; admission counts cores online at call time; relative paths without `..` | specs 0003, 0005, 0007 |
| Bytecode: one package per application, 4-byte-word stack VM, split `compiler`/`interpreter` crates over `abi`, structural verification | spec 0008 (Draft) |
| Scheduler: slot table of 64 tasks, claim takes the value under a per-slot spin lock, 10 000-step quantum, 10 s / 200M-step budget | spec 0007, ADR 0011 |
| Delivery is source-only, permanently: the image never carries bytecode and packages live only in memory | ADR 0019 (Accepted) |

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

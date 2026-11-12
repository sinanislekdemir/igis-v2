# 0001: Vision and guiding principles

- **Status:** Draft
- **Scope:** whole system (kernel, runtime, language, tooling)
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0002, 0003, 0004, 0005, 0006
- **Updated:** 2026-11-03

## Context

`igis-v2` is a from-scratch successor to an operating system written around 2005–2006.
No source survives, so this is a re-imagining rather than a port: the continuity is
design memory and intent, not code.

The system is **deliberately experimental**. It is not intended to serve users, host
untrusted workloads, achieve POSIX compatibility, or run on arbitrary hardware. Those
concerns are out of scope unless adopted explicitly. The design therefore optimizes for
conceptual clarity and a fast iteration loop (edit → build → boot in QEMU), not for
robustness, security, or throughput.

### Core thesis

Conventional kernels separate *mechanism* (privileged code) from *policy* (user
programs) with a hardware boundary. igis-v2 rejects that split and takes the opposite
position:

> The kernel is a **programmable system**: a language runtime with a standard library
> that owns the machine. Applications are source code evaluated inside it, and the
> kernel allocates every byte they will ever use.

Three commitments follow, and they reinforce each other:

1. **No privilege boundary.** Everything runs at ring 0 in one address space (ADR 0001).
2. **No binary applications.** The language runtime is the system's interface to
   programs (ADR 0002, ADR 0005).
3. **No dynamic application memory.** An application declares its complete memory
   footprint; the kernel allocates and owns it; nothing grows at runtime (ADR 0004).

The third commitment is what makes the first two defensible: with fixed, kernel-known
layouts and no allocator, the interpreter can enforce memory correctness without
hardware protection, and termination is deterministic.

### Trust model

There is no hardware isolation, so "isolation" here means something narrower: the
interpreter enforces that an application can only name memory inside its own fixed
arena. That contains *accidental* bugs in cooperative applications. It does not contain
hostile code, and it fails open — a bug in the interpreter, a raw-pointer escape in a
builtin, or an app-programmed DMA removes the protection entirely. The interpreter and
`syslib` are the trusted computing base. Details in `0002-architecture.md`.

### Interpretation note

The kernel is written in Rust and assembly, and **hosts** the language runtime. The
kernel is not written in the new language. Self-hosting the kernel is a separate, much
larger ambition — see Open Question 6.

## Requirements

- **R1 (MUST):** All kernel and application code executes at CPL 0. There is no ring 3
  code, no user mode, and no privilege-transition instruction in the execution path.
- **R2 (MUST):** The kernel contains the language runtime, a standard library, and — as a
  later addition — a system library. System services are reachable from applications as
  runtime primitives, not as syscalls.
- **R3 (MUST):** Applications are distributed and loaded as source text. No native
  application binaries are loaded or executed. An internal intermediate representation
  (bytecode) is permitted (ADR 0005); native code generation is not planned.
- **R4 (MUST):** Kernel code has unrestricted access to the address space. No per-app
  hardware memory protection and no separate address spaces.
- **R5 (MUST):** All instruction-set-specific code is confined to architecture crates.
  x86_64 is the first implementation; kernel core, runtime and language contain no
  ISA-specific logic.
- **R6 (MUST):** The system boots under QEMU via GRUB from a single documented command,
  and that loop is the primary development target.
- **R7 (MUST):** The programming language and its standard library are specified as an
  independent track, with a defined host interface to the kernel, so the runtime can be
  developed and tested on the host without booting the OS.
- **R8 (SHOULD):** Prefer the simplest design that demonstrates the thesis. Performance
  work happens only after behavior is settled and measured.
- **R9 (MUST):** Each CPU core runs one cooperative kernel scheduler thread. Applications
  are cooperative tasks pinned to exactly one core for their entire lifetime; migration
  is not permitted (ADR 0003).
- **R10 (MUST):** Applications declare their full memory requirement before execution.
  There is no `malloc`, no realloc, and no runtime memory expansion. The kernel allocates
  the footprint on the application's behalf (ADR 0004).
- **R11 (MUST):** Inter-application communication uses kernel-owned shared-memory
  streams with fixed capacity (FIFO queues or equivalent). No dynamic buffer growth.
- **R12 (MUST):** The kernel is monolithic and layered internally: architecture and
  platform support, drivers, memory management, scheduler, interpreter, standard
  library, and system library (`syslib`, later).
- **R13 (MUST):** Application loading is behind a provider interface. An initial
  ramdisk/module source is first; block devices and a filesystem follow without changing
  the application model or the tooling contract.

## Non-goals

Explicitly out of scope: security boundaries and hostile-code containment, multi-user
operation, POSIX or Linux compatibility, networking, demand paging, swap, binary
compatibility, and production-grade fault tolerance. SMP is in scope as a design
constraint (R9) but not necessarily enabled in the first milestone.

## Design sketch

```
layer 2   applications (source text, custom language)
              |  evaluated by
layer 1   +-------------------------------------------------------+
          | interpreter   values, frames, bytecode, safe points   |
          | stdlib        strings, lists, math, formatting        |
          | syslib        raw hardware, streams, disk (later)     |
          |-------------------------------------------------------|
          | sched         per-core cooperative scheduler, budgets |
          | mm            frame allocator, app arenas, streams    |
          | vfs / fs      ramdisk now, block device later         |
          | drivers       console, timer, block, input            |
          | platform      memory map, ACPI, device discovery      |
          | arch/x86_64   boot, GDT/IDT, paging, APIC, SMP        |
          +-------------------------------------------------------+
              |  multiboot2
          GRUB -> QEMU
```

Per core: one kernel scheduler thread runs a cooperative loop. Every application is a
task bound to one core, with a preallocated stack and arena, executed by the interpreter
until it yields, blocks on a stream, or exhausts its time budget. A watchdog detects
budget exhaustion at interpreter safe points and may terminate the task. Terminating a
task deterministically reclaims its arena and closes its stream endpoints.

Two parallel workstreams:

1. **System track** — boot, memory, streams, devices, loader, scheduler, runtime hosting.
2. **Language track** — syntax, type system, memory declarations, standard library,
   tooling. Runs on the host first, so it does not block on the kernel.

The contract between them is the *host interface*: the enumerated set of operations the
runtime may invoke (allocate arena, yield, send/receive on stream, write console, ...).
It is the closest thing this system has to a syscall ABI and is specified independently.

### Invariants

- No privilege-level change occurs after boot.
- No application allocation occurs at runtime; every arena is fixed at load time.
- Every application task is single-core for life; no migration.
- All application memory access is interpreter-mediated and bounds-checked.
- Application termination is deterministic: no garbage collector, no leaked arenas.

## Consequences

**Positive**

- No syscall ABI, no protection-domain switches, no user copies, no per-process page
  tables, no GC, no allocator in the hot path.
- Applications are data: inspectable, embeddable, reloadable, testable.
- Deterministic memory accounting and deterministic cleanup; a killed app leaves nothing
  behind.
- Cross-architecture applications come for free.
- A REPL is the native execution model, not a feature.

**Negative**

- Any application bug can corrupt kernel state; the interpreter's checks are the only
  defense, and a bug in *them* is fatal.
- The language becomes the system ABI. Semantic mistakes are expensive and propagate
  everywhere.
- Fixed footprints force awkward explicit bounds on variable-size data (text, files,
  queues); the language must make bounded data ergonomic or applications will be ugly.
- A cooperative task that fails to reach a safe point can stall its core; every blocking
  primitive must be written to yield.
- Interpreted execution is slow; compute-heavy applications will be painful.

**Neutral**

- Paging remains for mapping, MMIO and guard pages, but not for inter-app protection.
- Interrupts, GDT, IDT and TSS remain necessary.
- SMP means shared kernel state needs locking even though applications never migrate.

## Open questions

| # | Question | Status / why it matters |
|---|----------|-------------------------|
| 1 | Multitasking model | **Resolved** — per-core cooperative tasks, pinned (ADR 0003). Sub-questions in #12, #16, #17. |
| 2 | What did the original igis do, and which parts should v2 inherit? | Open. Defines what "second version" means. |
| 3 | Is an intermediate bytecode representation acceptable? | **Resolved** — yes; JIT excluded (ADR 0005). |
| 4 | How are applications delivered? | Partly resolved (R13); filesystem design still open. |
| 5 | Is the system self-hosting for development (in-OS editor/REPL) or cross-developed? | Open. Scope difference; affects UI and FS. |
| 6 | Should the kernel eventually be written in its own language? | Open. Determines whether the language must be systems-capable. |
| 7 | Is there logical isolation between applications (capabilities, builtin subsets)? | Open. Interpreter-level isolation is assumed; whether it is *enforced* as a permission model is undecided. |
| 8 | Text console only, or framebuffer graphics and a windowing model? | Open. Affects stdlib output primitives and driver scope. |
| 9 | x86_64 under QEMU only, or eventual real-hardware boot? | Open. Constrains drivers and firmware assumptions. |
| 10 | Language identity beyond "typed": syntax family, type system details, memory model details. | Partly resolved (typed, declared layout). See `specs/0003-language.md`. |
| 11 | BIOS + multiboot2, or UEFI under QEMU? | Open. Early boot code and long-mode transition differ. |
| 12 | Watchdog semantics: is termination unconditional on budget overrun, and how do blocking builtins interact with budgets? | Open. Determines whether builtins must be non-blocking. |
| 13 | Is sub-allocation inside a fixed arena allowed (bump/slab reset by the app), or is every object statically placed? | **Resolved** — fully static, no runtime placement of any kind (ADR 0007). |
| 14 | Is the standard library written in Rust or in the custom language? | Open. Affects portability, bootstrapping and speed. |
| 15 | Filesystem: custom minimal design, FAT32, or read-only images only? | Open. Needed once disk access starts. |
| 16 | Is SMP enabled in the first milestone or only designed for? | Open. Recommend design-for-N, boot-1. |
| 17 | Can one application have several tasks on the same core, or exactly one? How is a task's core chosen? | Open. Affects scheduler and app manifest. |
| 18 | Stack overflow handling: fixed stack with interpreter depth checks, or guard pages that fault? | **Resolved** — guard pages (ADR 0006, H2); depth checks only if guard pages prove costly. |
| 19 | Streams: copy through the kernel, or true shared ring buffers? Message- or byte-oriented? | Open. Dictates the IPC fast path and how much `syslib` must expose. |
| 20 | Can a task's time budget be configured, and by whom? | Open. Scheduler policy and manifest surface. |
| 21 | What happens to a task's streams when the peer dies mid-frame? | Open. Determines close/delivery semantics. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-03 | Initial draft from vision discussion. |
| 2026-11-03 | Added layer model, per-core cooperative concurrency, static memory model, stream IPC, loader abstraction. |

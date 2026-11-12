# 0001: Vision and guiding principles

- **Status:** Draft
- **Scope:** whole system (kernel, runtime, language, tooling)
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0002, 0003, 0004, 0005, 0006, 0007, 0011, 0012, 0013, 0014, 0015
- **Updated:** 2026-11-12

## Context

`igis-v2` is a from-scratch successor to an operating system written in 2005. The first
version was a front-end for a quantum-cryptography modem: it began as a Linux driver, but
the user/kernel ring boundary made the hardware-facing work awkward, so it grew into a
kernel with its own boot path. No source survives. The continuity is therefore not code
but the constraints learned while writing that system: deterministic memory, no allocator
in the hot path, direct device access, and a programming environment used interactively
against live hardware.

The second version is not a re-creation of the modem front-end. The modem is gone. What
remains is the question the first version raised: what does an operating system look like
when the privileged/unprivileged split — the thing that made the hardware awkward — is
removed entirely?

The long-term goal is a **general personal system**: a machine the author uses and
rewrites, where hardware and language carry equal weight. The first milestone is much
smaller: prove the thesis with a kernel that boots, runs applications written as source,
isolates them at the interpreter level, and lets a REPL drive a device.

The system is deliberately experimental. It is single-user, by the author and for the
author. It is not intended to host untrusted workloads, to serve other users, to achieve
POSIX compatibility, or to run on arbitrary hardware. It optimizes for conceptual clarity
and a fast iteration loop (edit → build → boot in QEMU), not for robustness, security, or
throughput.

### Core thesis

Conventional kernels separate *mechanism* (privileged code) from *policy* (user programs)
with a hardware boundary. igis-v2 rejects that split and takes the opposite position:

> The kernel is a **programmable system**: a language runtime with a standard library
> that owns the machine. Applications are source code evaluated inside it, and the
> kernel allocates every byte they will ever use.

Three commitments follow, and they reinforce each other:

1. **No privilege boundary.** Everything runs at ring 0 in one address space (ADR 0001).
2. **No binary applications.** The language runtime is the system's interface to
   programs (ADR 0002, ADR 0005).
3. **No dynamic application memory.** An application declares its complete memory
   footprint; the kernel allocates and owns it; nothing grows at runtime (ADR 0004,
   ADR 0007).

The third commitment is what makes the first two defensible: with fixed, kernel-known
layouts and no allocator, the interpreter can enforce memory correctness without hardware
protection, and termination is deterministic.

### Trust model

There is no hardware isolation, so "isolation" here means something narrower: the
interpreter enforces that an application can only name memory inside its own fixed
regions. That contains *accidental* bugs in cooperative applications. It does not contain
hostile code, and it fails open — a bug in the interpreter, a raw-pointer escape in a
builtin, or an app-programmed DMA removes the protection entirely. The interpreter and
`syslib` are the trusted computing base.

There is deliberately **no capability model**. Every application may call every
`syslib` primitive and touch every piece of hardware. The only boundary the system
maintains is memory ownership between applications, enforced by the interpreter. This is
a single-user machine; the author is the only adversary, and the author is trusted.

### Interpretation note

The kernel is written in Rust and assembly, and **hosts** the language runtime. The
kernel is not written in the new language. Self-hosting the kernel is out of scope
(vision OQ 6 resolved); the language is an application language, not a systems language.

## Requirements

- **R1 (MUST):** All kernel and application code executes at CPL 0. There is no ring 3
  code, no user mode, and no privilege-transition instruction in the execution path.
- **R2 (MUST):** The kernel contains the language runtime, a runtime `core` of native
  primitives, a `stdlib` written in the application language, and a `syslib` exposing
  hardware. System services are reachable from applications as runtime primitives, not
  as syscalls.
- **R3 (MUST):** Applications are distributed and loaded as source text. No native
  application binaries are loaded or executed. An internal intermediate representation
  (bytecode) is permitted (ADR 0005); native code generation is not planned.
- **R4 (MUST):** Kernel code has unrestricted access to the address space. No per-app
  hardware memory protection and no separate address spaces.
- **R5 (MUST):** All instruction-set-specific code is confined to architecture crates.
  x86_64 is the first implementation; kernel core, runtime and language contain no
  ISA-specific logic (ADR 0013).
- **R6 (MUST):** The system boots under QEMU via GRUB and multiboot2 from a single
  documented command, and that loop is the primary development target (ADR 0014).
- **R7 (MUST):** The programming language and its standard library are specified as an
  independent track, behind a host interface, so the runtime can be developed and tested
  on the host without booting the OS.
- **R8 (SHOULD):** Prefer the simplest design that demonstrates the thesis. Performance
  work happens only after behavior is settled and measured.
- **R9 (MUST):** Each CPU core runs one interpreter loop. Runnable applications are
  claimed from a global runnable set and may be advanced by any core; an application is
  never pinned to a core, and exactly one core advances a given application at a time
  (ADR 0011).
- **R10 (MUST):** Applications declare their full memory requirement before execution.
  There is no `malloc`, no realloc, and no runtime memory expansion. The kernel allocates
  the footprint on the application's behalf (ADR 0004, ADR 0007).
- **R11 (MUST):** Inter-application communication uses kernel-owned byte FIFOs with fixed
  capacity and typed views above them. No dynamic buffer growth.
- **R12 (MUST):** The kernel is monolithic and layered internally: architecture and
  platform support, drivers, memory management, scheduler, interpreter, `core`,
  `stdlib`, and `syslib`.
- **R13 (MUST):** Application loading is behind a provider interface. A read-only ext2
  image delivered as a GRUB module is first; block devices and a writable filesystem
  follow without changing the application model or the tooling contract (ADR 0015).
- **R14 (MUST):** There is no capability or permission model. Any application may use any
  `syslib` primitive. Isolation is limited to interpreter-enforced memory ownership.
- **R15 (SHOULD):** The system should remain usable interactively: a REPL is the native
  execution model, and a running application can be inspected and terminated without
  rebooting.

## Non-goals

Explicitly out of scope: security boundaries and hostile-code containment, multi-user
operation, POSIX or Linux compatibility, networking, demand paging, swap, binary
compatibility, production-grade fault tolerance, real-hardware bring-up, a windowing
system, and durable storage in the first milestone. SMP is in scope as a design
constraint (R9); the first milestone boots one core and brings up the others within the
same milestone.

## Design sketch

```
layer 2   applications (source text, custom language)
              |  evaluated by
layer 1   +-------------------------------------------------------+
          | interpreter   values, frames, bytecode, safe points   |
          | core          native primitives (Rust): text, math,   |
          |               streams, time, console                  |
          | stdlib        written in the language: collections,   |
          |               parsing, formatting, UTF-8              |
          | syslib        privileged: port I/O, MMIO, DMA, disk   |
          |-------------------------------------------------------|
          | sched         per-core interpreter loops, claim/      |
          |               release, quantum, budget, termination   |
          | mm            frame allocator, app regions, streams   |
          | vfs / fs      read-only ext2 over BlockDevice         |
          | drivers       framebuffer console, keyboard, timer    |
          | platform      memory map, ACPI, device discovery      |
          | arch/x86_64   boot, GDT/IDT, paging, APIC, SMP        |
          +-------------------------------------------------------+
              |  multiboot2
          GRUB -> QEMU
```

Per core: one interpreter loop claims a runnable application, runs a quantum of
bytecode, and releases it at a safe point. Every application is a task with a
preallocated stack, `state` region and code, advanced by whichever core is free, until it
yields, blocks on a stream or timer, exhausts its quantum, or exhausts its budget.
Terminating a task deterministically reclaims its regions and closes its stream
endpoints.

Two parallel workstreams:

1. **System track** — boot, memory, streams, devices, loader, scheduler, runtime hosting.
2. **Language track** — syntax, type system, memory declarations, standard library,
   tooling. Runs on the host first, behind the host interface, so it does not block on
   the kernel.

The contract between them is the **host interface**: the enumerated set of operations the
runtime may invoke (write console, yield, send/receive on a stream, read the clock,
sleep, map MMIO, ...). It is this system's syscall ABI in all but name, and it is
specified independently in `0005-host-interface.md`.

### Invariants

- No privilege-level change occurs after boot.
- No application allocation occurs at runtime; every region is fixed at load time.
- An application is never pinned to a core, and never runs on two cores at once.
- All application memory access is interpreter-mediated and bounds-checked.
- Application termination is deterministic: no garbage collector, no leaked regions.
- Applications cannot name memory outside their own regions through language features.

## Consequences

**Positive**

- No syscall ABI, no protection-domain switches, no user copies, no per-process page
  tables, no GC, no allocator in the hot path.
- Applications are data: inspectable, embeddable, reloadable, testable, and — because
  their state is in memory at every safe point — migratable between cores.
- Deterministic memory accounting and deterministic cleanup; a killed app leaves nothing
  behind.
- Cross-architecture applications come for free.
- A REPL is the native execution model, not a feature.
- Work stealing means no idle core while work is runnable, and no load-balancing policy
  to tune.

**Negative**

- Any application bug can corrupt kernel state; the interpreter's checks are the only
  defense, and a bug in *them* is fatal.
- The language becomes the system ABI. Semantic mistakes are expensive and propagate
  everywhere.
- Fixed footprints force awkward explicit bounds on variable-size data (text, files,
  queues); the language must make bounded data ergonomic or applications will be ugly.
- A cooperative task that fails to reach a safe point can stall its core beyond its
  quantum; every blocking primitive must be written to yield.
- Interpreted execution is slow; compute-heavy applications will be painful.
- Application state is shared across cores; cache-line bouncing and cross-core
  invalidation are real costs.
- The claim/release protocol is a new correctness surface in the trusted base: a mistake
  means silent corruption, not a crash.

**Neutral**

- Paging remains for mapping, MMIO and guard pages, but not for inter-app protection.
- Interrupts, GDT, IDT and TSS remain necessary.
- SMP means shared kernel state needs locking even though application code is
  single-threaded.
- The boot path is GRUB/multiboot2 with a hand-written long-mode transition; UEFI and
  real hardware are later, separate work.

## Open questions

| # | Question | Status / why it matters |
|---|----------|-------------------------|
| 1 | Multitasking model | **Resolved** — per-core interpreter loops, work-stealing tasks (ADR 0011). |
| 2 | What did the original igis do, and which parts should v2 inherit? | **Resolved** — a 2005 bare-metal front-end for a quantum-cryptography modem; v2 inherits the constraints (determinism, no allocator, direct hardware, REPL), not the modem or its UI. |
| 3 | Is an intermediate bytecode representation acceptable? | **Resolved** — yes; JIT excluded (ADR 0005). |
| 4 | How are applications delivered? | **Resolved** — read-only ext2 image as a GRUB module over an in-memory block device (ADR 0015). |
| 5 | Is the system self-hosting for development (in-OS editor/REPL) or cross-developed? | **Resolved** — cross-developed; the in-system workflow is a REPL with no durable storage. A writable filesystem and an editor are future work. |
| 6 | Should the kernel eventually be written in its own language? | **Resolved** — no; the language is an application language (ADR 0012). |
| 7 | Is there logical isolation between applications (capabilities, builtin subsets)? | **Resolved** — no capability model; interpreter-enforced memory ownership only (R14). |
| 8 | Text console only, or framebuffer graphics and a windowing model? | **Resolved** — linear framebuffer with a bitmap font; no windowing. |
| 9 | x86_64 under QEMU only, or eventual real-hardware boot? | **Resolved** — QEMU only for now; the platform layer keeps the door open. |
| 10 | Language identity beyond "typed": syntax family, type system details, memory model details. | **Resolved** — Pascal-like, nominal types, no user generics; see `0003-language.md`. |
| 11 | BIOS + multiboot2, or UEFI under QEMU? | **Resolved** — GRUB + multiboot2 on BIOS, hand-written long-mode transition (ADR 0014). |
| 12 | Watchdog semantics: is termination unconditional on budget overrun, and how do blocking builtins interact with budgets? | **Resolved** — quantum expiry releases, total CPU budget terminates unconditionally; blocked time does not count; blocking builtins transition to `Blocked` (`0007-scheduler.md`). |
| 13 | Is sub-allocation inside a fixed arena allowed? | **Resolved** — fully static, no runtime placement of any kind (ADR 0007). |
| 14 | Is the standard library written in Rust or in the custom language? | **Resolved** — native primitives in `core` (Rust), everything above them in `stdlib` (the language). |
| 15 | Filesystem: custom minimal design, FAT32, or read-only images only? | **Resolved** — read-only ext2 (ADR 0015). |
| 16 | Is SMP enabled in the first milestone or only designed for? | **Resolved** — one core first, AP bring-up later within the same milestone. |
| 17 | Can one application have several tasks on the same core, or exactly one? How is a task's core chosen? | **Resolved** — one task per application instance; no core is chosen, any free core claims it (ADR 0011). |
| 18 | Stack overflow handling: fixed stack with interpreter depth checks, or guard pages that fault? | **Resolved** — guard pages (ADR 0006, H2); depth checks only if guard pages prove costly. |
| 19 | Streams: copy through the kernel, or true shared ring buffers? Message- or byte-oriented? | **Resolved** — byte FIFO, copied through the kernel, typed views above it (`0006-streams.md`). |
| 20 | Can a task's time budget be configured, and by whom? | **Resolved** — a single kernel constant in the first milestone; applications cannot negotiate. |
| 21 | What happens to a task's streams when the peer dies mid-frame? | **Resolved** — sends are atomic, so there is no mid-frame; queued data is discarded immediately and the peer observes `Closed` (`0006-streams.md`). |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-03 | Initial draft from vision discussion. |
| 2026-11-03 | Added layer model, per-core cooperative concurrency, static memory model, stream IPC, loader abstraction. |
| 2026-11-12 | Recorded the 2005 origin (modem front-end, ring-boundary friction) and the personal-system goal. Replaced core pinning with work stealing (ADR 0011). Resolved delivery (ext2), output (framebuffer), isolation (no capability model), boot (GRUB/multiboot2), stdlib split (`core`/`stdlib`) and all remaining open questions. |

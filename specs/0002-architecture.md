# 0002: System architecture

- **Status:** Draft
- **Scope:** kernel internals, runtime hosting, execution and memory model
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0002, 0003, 0004, 0006
- **Updated:** 2026-11-03

## Context

This spec turns the vision (`0001-vision.md`) into a concrete structure: what the layers
are, how a task executes, how memory is owned, and how applications communicate. It is
deliberately about *mechanism*, not language syntax — the language is specified
separately in `0003-language.md`.

The constraints inherited from the vision that shape everything here: single privilege
level, single address space, no application allocator, no migration between cores, and
a language runtime embedded in the kernel.

## Layering

| Layer | Component | Written in | Responsibility |
|-------|-----------|------------|----------------|
| 1 | `arch/<isa>` | Rust + asm | Boot entry, GDT/IDT/TSS, paging, context switch, APIC/timer, SMP bring-up |
| 1 | `platform/<machine>` | Rust | Memory map, ACPI parsing, device discovery, machine quirks |
| 1 | `drivers` | Rust | Console, timer, input, block, bus enumeration — in-kernel, monolithic |
| 1 | `mm` | Rust | Frame allocator, app arenas, kernel heap, stream buffers, guard pages |
| 1 | `sched` | Rust | Per-core scheduler threads, run queues, task states, budgets, termination |
| 1 | `vfs` / `fs` | Rust | Loader providers: ramdisk first, block device and filesystem later |
| 1 | `interpreter` | Rust | Parse/compile, bytecode execution, safe points, builtin dispatch |
| 1 | `stdlib` | Rust *or* the language (OQ 14) | Values, strings, collections, math, formatting |
| 1 | `syslib` | Rust (later) | Privileged services: raw hardware access, disk, streams, time |
| 2 | applications | Custom language | Everything else |

`syslib` is deliberately a separate namespace rather than part of `stdlib`: it is the
surface that would eventually be permission-gated, and it is the only place where raw
hardware concepts (addresses, ports, MMIO, DMA) are exposed.

## Execution model

### Per-core scheduler thread

At boot, the kernel creates one scheduler thread per CPU core. That thread is a normal
kernel execution context; it is not an application task. It owns a run queue of tasks
pinned to its core and loops:

```
loop {
    task = pick_next(core.run_queue)        // cooperative, priority-aware
    if task.is_none() { idle() }            // halt until interrupt
    run_interpreter_until_safe_point(task)  // yields, blocks, or exhausts budget
}
```

Applications never migrate. Core assignment happens at task creation and is fixed
(assignment policy is OQ 17). This makes an application's execution deterministic with
respect to which core's state it touches, at the cost of load balancing.

### Task states

`Runnable` · `Running` · `Blocked(stream | timer)` · `Terminated`

A task yields explicitly, blocks on a stream or timer, or is preempted by the budget
mechanism. There is no involuntary context switch into a different task without the
interpreter's cooperation; the interpreter is the only place a task boundary is crossed.

### Safe points and budgets

The interpreter checks, at defined safe points (bytecode boundaries, backward branches,
calls, and builtin entry):

1. a cooperative yield request (kernel or scheduler initiated), and
2. the task's remaining time budget.

A per-core timer interrupt updates time and marks a budget as expired if the running
task exceeded it. The interrupt handler does not switch tasks; it only sets state. The
interpreter observes the flag at its next safe point.

**Termination rule (as specified by the project owner):** if a task exceeds its budget
*and* other tasks are waiting on the same core, the task is terminated rather than
suspended. If no task is waiting, it may continue. Termination is immediate at a safe
point.

Termination must be deterministic: the task's arena is released as a whole, its stream
endpoints are closed and their references invalidated, and its entry is removed from all
run queues. There are no zombies and no GC.

### Blocking builtins

A cooperative model cannot tolerate a builtin that spins or sleeps uninterruptibly: it
would stall the core and defeat the watchdog. Therefore every builtin that can take
unbounded time must either complete in bounded time or transition the task to `Blocked`
and return to the scheduler. This is a requirement, not a guideline, and it is the main
reason `syslib` must be designed against the scheduler rather than beside it.

### Interrupts

Hardware interrupts still preempt the interpreter. Handlers must be short and must not
mutate application-visible memory in ways the interpreter cannot tolerate; work that
needs more time is deferred to a bottom-half drained by the scheduler thread. This is
the standard split, but it matters more here because there is no separate kernel thread
that can be rescheduled independently.

## Memory model

### Ownership

The kernel owns all physical memory. An application never allocates; its compiled
footprint — computed from compile-time capacities — is recorded in the manifest, and the
loader asks `mm` for regions of those sizes before the task starts. The regions are the
application's entire writable world.

```
application manifest
  └─ arena size, stack size, stream endpoints, entry point
        │
        ▼
  mm: reserve frames → map into kernel address space → zero
        │
        ▼
  task: { arena, stack, streams, bytecode }
```

No runtime growth, no `malloc`, no realloc, no GC.

### Addresses and handles

Because everything shares one address space, an application must never be given a raw
address that would let it name another application's memory. The intended model is that
application-visible references are **handles or offsets relative to the task's arena**,
translated and bounds-checked by the interpreter on every access. Raw addresses are
confined to `syslib`, behind a capability.

This is the only mechanism providing "isolation" (vision OQ 7): logical, interpreter-
enforced, not hardware-enforced.

### Trust model, precisely

| Aspect | Reality |
|--------|---------|
| Hardware protection | None. All code is CPL 0 in one address space; a single wrong pointer can reach any byte. |
| Application memory access | Every load/store is a bytecode operation performed by the interpreter. The app holds handles and offsets, never raw addresses. The interpreter checks ownership and bounds before touching memory. |
| What it protects | Accidental bugs in cooperative applications: overruns, wild indices, use-after-close. One app cannot corrupt another's arena through normal language operations. |
| What it does not protect | Hostile code, the kernel from the interpreter, or anything from a bug in a bounds check. The guarantee fails **open**: one wrong check means no protection at all. |
| Escape hatches | `syslib` raw hardware access, DMA-capable devices, any interpreter builtin that takes a raw address, and every `unsafe` block in the kernel. |
| Why fixed arenas help | No allocator metadata, no realloc, no cross-task free lists. An app's memory bugs stay within its own arena even when the interpreter does not notice them. |

Consequence: the interpreter and `syslib` are the trusted computing base. Their `unsafe`
surface must stay small, enumerated, and auditable, because there is no second line of
defense.

### Why hardware cannot help here

x86_64 has no concept of an application. It has privilege levels (CPL), page permissions
(the U/S bit), and address-space roots (CR3). App-vs-app isolation is not a hardware
feature; it is a policy the kernel implements by giving each app its own page tables and
marking its pages user-accessible. What the CPU actually enforces is:

- CPL 3 may not touch supervisor pages, and
- an address absent from the current CR3 is not mapped.

It does not enforce "ring-0 context A may not touch ring-0 context B". CPL 0 can write
CR3, can access user pages, and would see every app's memory if they shared a table.
With one address space and every context at CPL 0, no page-table arrangement can separate
two applications. The only hardware layer that could is a hypervisor (VMX/SVM), which is
a more privileged ring under a different name — i.e. reintroducing the boundary this
project removes.

**What the MMU can still do at ring 0**, and should: read-only kernel text (`CR0.WP`),
non-executable data and arenas (NX is meaningless for interpreted code, but harmless),
and not-present guard pages that fault even at CPL 0. These harden the kernel against its
own bugs; they cannot distinguish "the interpreter writing app A's arena" from "the
interpreter writing app B's arena", because both are ordinary supervisor writes.

### Sub-allocation

Whether an application may run its own bump/slab allocator *inside* its fixed arena is
OQ 13. Either way there is no growth: at worst, an application manages a fixed pool and
resets it explicitly.

### Stacks

Each task has a preallocated stack. With no inter-app paging protection, a silent stack
overflow corrupts whatever follows, so one of two mechanisms is required (OQ 18): a
non-present guard page that raises a page fault even at CPL 0, or interpreter-side
call-depth and frame-size accounting against a declared maximum.

### Hardening (provisional, ADR 0006)

The MMU cannot separate applications at CPL 0, but it can still catch kernel bugs. Three
measures are adopted provisionally — accepted only for as long as they stay cheap:

- **H1 (MUST):** Kernel text and read-only data are mapped read-only, with `CR0.WP = 1`.
- **H2 (MUST):** Every task stack has a not-present guard page at its growing end. A fault
  inside a known guard range terminates that task; any other fault is a kernel panic.
- **H3 (MUST):** NX is enabled (`EFER.NXE`); kernel data, task stacks and application
  arenas are mapped non-executable.
- **H4 (MUST):** All of this lives in the single shared address space: no per-task page
  tables and no CR3 switching.
- **H5 (MUST):** If any measure requires per-task mappings, address-space switching, or a
  fault handler with more than trivial policy, the measure is dropped — not the
  architecture adjusted around it.

## Streams and IPC

Applications communicate through kernel-owned shared-memory streams with **fixed
capacity** (vision R11). A stream is created before use, with a declared element size
and count; it never grows.

Properties to be specified in a dedicated spec:

- Two endpoints, each owned by at most one task at a time.
- Blocking and non-blocking send/receive; blocking transitions the task to `Blocked`.
- Backpressure: a full queue blocks or fails the sender, never expands.
- Close semantics: endpoints close on task termination; the peer observes closure.
- Whether data is copied through the kernel or genuinely shared via a ring buffer with
  atomics is an open question (OQ 19). Copying is simpler and keeps handles opaque;
  a shared ring is faster and matches the "shared memory" wording.

## Application loading and delivery

Loading is behind a provider interface (vision R13), so delivery can evolve without
touching the runtime:

```rust
trait SourceProvider {
    fn list(&self) -> ...;
    fn load(&self, name: &str) -> Result<AppSource, LoadError>;
}
```

- **v1:** ramdisk or GRUB module containing application source plus manifests.
- **Next:** block device + filesystem provider, same interface.
- **Tooling:** one manifest format, two image builders (ramdisk, disk), so neither path
  becomes a dead end.

The manifest is part of the application contract: footprint, entry point, declared
streams, and (later) capability requests.

## Boot

1. GRUB loads a multiboot2 kernel ELF (and the initial payload) and enters 32-bit
   protected mode.
2. A 32-bit stub enables long mode and jumps to the 64-bit entry point.
3. Early kernel init: GDT/IDT, paging and direct map, serial/framebuffer console, frame
   allocator.
4. Platform discovery: memory map, ACPI, APIC, timers, console devices.
5. Scheduler thread for the boot core; optional AP bring-up (OQ 16).
6. Loader registers the ramdisk provider, runtime initializes, REPL or entry application
   starts.

## Fatal fault path

Two entry points, one terminal state:

```
main returns normally
        └─→ exit(Ok) → reclaim

fault detected (interpreter, guard page, budget)
        │
        ▼
abandon frames back to the application's main level
        │
        ▼
fault(code) on the scheduler's kernel stack, bounded budget
        │
        ├─ returns / faults / overruns → exit(code) → reclaim
        └─ jumps to a main-level recovery label (first fault only)
                 → reset frame stack, resume; budget is not reset
```

`fault` and `exit` run on the kernel stack because the task stack may be exhausted. A
handler is never re-entered while running; a fault inside it is abandoned rather than
escalated. A task gets at most one recovery: after a recovery, the next fault is terminal
and `fault` is not called again. `BudgetExceeded` may be logged by `fault`, but recovery is
rejected. `exit` is always attempted at termination, with a small emergency budget; it is
best-effort, not guaranteed (ADR 0010).

The limit is deliberate: a programmer's infinite-loop or repeated-fault bug must not be
able to force the kernel into repeated fault handling. The kernel processes one fault per
task; after that, shutdown.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Task exceeds budget with others waiting | Terminate at next safe point; `fault(BudgetExceeded)`; recovery rejected; `exit`; reclaim |
| Application fault detected by interpreter (out of bounds, capacity, illegal operation) | Abort operation; `fault(code)`; if it returns, `exit(code)` then reclaim; if it jumps, resume |
| Stack overflow | Guard-page fault; `fault(StackOverflow)` on kernel stack; same outcomes as above |
| `fault` returns | Terminal: `exit(code)` then reclaim |
| `fault` jumps to a recovery label | Reset frame stack to the label; resume; budget not reset; one recovery per task |
| Second fault after a recovery | Terminal; `fault` is not called again; `exit` then reclaim |
| `fault` faults or exceeds its budget | Abandon handler; `exit` is still attempted; reclaim |
| `exit` faults or exceeds its budget | Abandon handler immediately; reclaim task |
| No `fault` handler defined | Fault is terminal: `exit` then reclaim |
| No `exit` handler defined | Task ends silently; reclaim |
| Task blocks on a stream forever | Remains `Blocked`; no CPU consumed; a timeout API may be added later |
| Stream full | Sender blocks or receives an error; never grows |
| Arena request cannot be satisfied at load | Application refused; no runtime out-of-memory path |
| Interpreter bug (bad pointer, wrong bounds) | Undefined; may corrupt kernel. No recovery boundary exists |
| Kernel panic | Log to serial, halt (or reboot) |

## Open questions

| # | Question | Blocking what |
|---|----------|---------------|
| 12 | Watchdog semantics: unconditional kill vs. kill only when others wait; grace period; cleanup callback? | scheduler spec |
| 13 | Sub-allocation within a fixed arena, or fully static placement? | **Resolved** — fully static (ADR 0007). |
| 16 | SMP enabled in milestone 1 or only designed for? | boot and locking work |
| 17 | One task per application, or several tasks on the same core? Core assignment policy? | scheduler and manifest |
| 18 | Stack overflow: guard page or interpreter accounting? | mm and interpreter |
| 19 | Streams: copy-through-kernel or true shared ring buffers? Message vs byte-oriented? | IPC and syslib |
| 20 | Can a task's budget be configured, and by whom? | scheduler and manifests |
| 21 | What happens to a task's streams when the peer dies mid-frame? | IPC semantics |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-03 | Initial draft from vision discussion. |

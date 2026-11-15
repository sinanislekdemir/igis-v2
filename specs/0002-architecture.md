# 0002: System architecture

- **Status:** Accepted
- **Scope:** kernel internals, runtime hosting, execution and memory model
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0001, 0002, 0003, 0004, 0006, 0011, 0012, 0013, 0014, 0015, 0016, 0018,
  0020, 0021
- **Updated:** 2026-11-16

## Context

This spec turns the vision (`0001-vision.md`) into a concrete structure: what the layers
are, how a task executes, how memory is owned, and how applications communicate. It is
deliberately about *mechanism*, not language syntax — the language is specified
separately in `0003-language.md`.

The constraints inherited from the vision that shape everything here: single privilege
level, single address space, no application allocator, no core affinity, and a language
runtime embedded in the kernel.

## Layering

| Layer | Component | Written in | Responsibility |
|-------|-----------|------------|----------------|
| 1 | `arch/<isa>` | Rust + asm | Boot entry, long-mode transition, GDT/IDT/TSS, paging, stack switch, APIC/timer, SMP bring-up |
| 1 | `platform/<machine>` | Rust | Memory map, ACPI parsing, device discovery, machine quirks |
| 1 | `drivers` | Rust | Framebuffer console, keyboard, mouse, timer, block devices — in-kernel, monolithic |
| 1 | `mm` | Rust | Frame allocator, application regions, kernel heap, stream buffers, guard pages |
| 1 | `sched` | Rust | Per-core interpreter loops, global runnable set, claim/release, quantum, budgets, termination |
| 1 | `vfs` / `fs` | Rust | Loader providers: read-only ext2 over `BlockDevice` first; writable filesystems later |
| 1 | `compiler` | Rust | Lex, parse, type and capacity check, bytecode and manifest emission |
| 1 | `interpreter` | Rust | Bytecode execution, safe points, builtin dispatch, diagnostics |
| 1 | `repl` | Rust | Compile-and-run service: request queue, loader, task creation |
| 1 | `core` | Rust | Native primitives: text and bytes, math, formatting core, console binding, stream binding, clock |
| 1 | `stdlib` | The application language | Collections, parsing, formatting, UTF-8 helpers, typed stream views |
| 1 | `syslib` | Rust | Privileged services: port I/O, MMIO mapping, DMA buffers, block access |
| 2 | applications | The application language | Everything else |

The three namespaces are visible to applications and deliberately distinct:
`core.*` is a native builtin (fast, part of the trusted base), `stdlib.*` is written in
the language (portable, inspectable, slower), and `syslib.*` reaches hardware. The split
tells a developer what a call costs and where it comes from.

`syslib` is **not** permission-gated. Every application may call it (vision R14). It is
a separate namespace for clarity and performance, not for security.

## Execution model

### Per-core interpreter loop

At boot, the kernel starts one interpreter loop on each online core. The loop is a normal
kernel execution context; it is not an application task. It owns no application. It
loops:

```
loop {
    task = runnable_set.claim()          // atomic; exclusive ownership
    if task.is_none() { halt_until_ipi() }
    run_quantum(task)                    // N bytecodes or ~1 ms
    task.release()                       // at a safe point; state is in memory
}
```

A task is claimed by exactly one core at a time. The claim word is the only
synchronisation needed to execute application code. Release is a memory-ordering
boundary: everything the previous owner wrote is visible to the next owner.

Applications have no affinity. Any core may claim any runnable application. Migration is
invisible to the application: it sees no core identity, no thread ID and no affinity.
ADR 0011 records the reasoning and the rejected alternatives.

### Task states

`Runnable` · `Running` · `Blocked(stream | timer)` · `Terminated`

A task yields explicitly, blocks on a stream or timer, exhausts its quantum, or is
terminated by the budget. Only `Running` is owned by a core; `Runnable` sits in the
global set; `Blocked` consumes no CPU.

### Safe points, quanta and budgets

The interpreter checks, at defined safe points (bytecode boundaries, backward branches,
calls, and builtin entry):

1. whether the current quantum is exhausted, and
2. whether the task's total budget is exhausted.

**Quantum** is OS jargon for a scheduling slice (unrelated to physics); `0007-scheduler.md`
distinguishes it from the total budget and the handler budget.

Two limits exist and they mean different things:

- **Quantum** — fairness. When it expires the core releases the task at the next safe
  point and re-enqueues it. This is preemption by cooperation, not a kill.
- **Total budget** — watchdog. Cumulative CPU time consumed by the task, with blocked
  time excluded. It is a single kernel-wide constant in the first milestone; applications
  cannot negotiate it (vision OQ 20). Expiry terminates the task **unconditionally** at
  the next safe point: `fault(BudgetExceeded)`, recovery rejected, `exit`, reclaim.

The unconditional rule replaces the conditional "terminate if another task is waiting on
the same core" rule of ADR 0003. Under work stealing there is no "same core" to reason
about.

A per-core timer interrupt updates time and marks budgets as expired. The interrupt
handler does not switch tasks; it only sets state. The interpreter observes the flag at
its next safe point.

Termination is deterministic: the task's regions are released as a whole, its stream
endpoints are closed, its references invalidated, and its entry removed from all queues.
There are no zombies and no GC.

### Realtime tasks

A task that controls hardware may promote itself, once, to the realtime class with
`core.realtime_on()`. Realtime tasks are claimed before normal tasks, are not released at
quantum expiry, and are exempt from the total CPU budget. In exchange they must call
`core.heartbeat()` at least once per second of non-blocked time; a missed deadline is
terminal (`fault(WatchdogTimeout)`). Admission is limited to one realtime task per online
core, so they cannot oversubscribe the machine. No core is reserved: N realtime tasks on
N cores can starve normal tasks, including the REPL; the tradeoff is accepted (ADR 0016
addendum). The model is soft realtime: it removes time-slicing and adds priority, but does
not promise deadlines. The full contract is in `0007-scheduler.md`; the decision and its
alternatives are in ADR 0016.

### Blocking builtins

A cooperative model cannot tolerate a builtin that spins or sleeps uninterruptibly: it
would stall the core and defeat both the quantum and the watchdog. Every builtin that can
take unbounded time must either complete in bounded time or transition the task to
`Blocked` and return to the loop. This is a requirement, not a guideline, and it is the
main reason `syslib` is designed against the scheduler rather than beside it.

### Interrupts

Hardware interrupts still preempt the interpreter. Handlers must be short and must not
mutate application-visible memory in ways the interpreter cannot tolerate; work that
needs more time is deferred to a bottom half drained by the interpreter loop. This is the
standard split, but it matters more here because there is no separate kernel thread that
can be rescheduled independently.

### SMP

The model is designed for N cores and boots with one. Within the first milestone,
additional cores are brought up through the APIC path and join the same global runnable
set. The claim protocol is identical on one core and on many; a single-core run is the
degenerate case, not a different design.

`MAX_CORES` is one constant (8): core index 0 is the boot core, APs take `1..MAX_CORES`.
The index is internal; the **APIC id** is the identity software sees (`syslib.cpu_id`).

Discovery is by attempt, with no ACPI/MADT parsing. After the boot core has its APIC and
clock, `smp::start()` copies the trampoline blob to the fixed page `0x8000` (so the SIPI
vector is 8) beside a parameter page at `0x9000` (PML4 physical address, higher-half entry
point, per-core stack tops, an atomic next-index counter). It sends INIT to APIC ids
`1..MAX_CORES`, waits 10 ms, sends two SIPIs ~200 µs apart and then waits a bounded 100 ms
for each to report. An id that never reports is simply not online: `-smp 1` boots with no
APs and `-smp 2` boots with one, without hanging.

The trampoline runs 16-bit real mode → 32-bit protected mode → long mode with the shared
PML4 and a flat 64-bit GDT, claims a core index with `lock xadd`, loads that core's stack
and `call`s the kernel entry. Its GDT keeps 64-bit code at selector `0x08` so the AP is
already on the kernel's code selector when the kernel GDT is loaded.

Per-core state is static: one GDT holds one TSS per core (each with its own kernel `RSP0`,
double-fault IST and page-fault IST stacks), and every core has a 256 KiB kernel stack —
the boot core's guarded one in the frame allocator, one 256 KiB `.bss` stack per AP. The
size is not arbitrary: any core may compile a request (spec 0007, "Services and task
creation"), and the compiler's bounded recursion costs up to ~200 KiB at the 32-level
limit, with a `stdlib`-using application already needing ~60 KiB. Every core loads the
shared IDT; only the boot core programs the timer, and the clock stays one global counter.

## Memory model

### Ownership

The kernel owns all physical memory. An application never allocates; its compiled
footprint — computed from compile-time capacities — is recorded in the manifest, and the
loader asks `mm` for regions of those sizes before the task starts. The regions are the
application's entire writable world.

```
application manifest
  └─ region sizes, stack size, stream endpoints, entry point
        │
        ▼
  mm: reserve frames → map into the shared address space → zero
        │
        ▼
  task: { code, state, stack, fpu, streams, bytecode }
```

No runtime growth, no `malloc`, no realloc, no GC, no runtime placement.

Floating point adds a fifth region: a 512-byte `fpu` image holding the task's SSE state
(ADR 0018). The scheduler `FXSAVE`s into it on release and `FXRSTOR`s from it on claim, so
a migratable task carries its float registers in memory like everything else. The region is
kernel-owned and never nameable by the application, exactly like a stream buffer.

### Addresses and handles

Because everything shares one address space, an application must never be given a raw
address that would let it name another application's memory. Application-visible
references are **handles or offsets relative to the task's regions**, translated and
bounds-checked by the interpreter on every access. Raw addresses are confined to
`syslib`.

This is the only mechanism providing "isolation": logical, interpreter-enforced, not
hardware-enforced.

### Trust model, precisely

| Aspect | Reality |
|--------|---------|
| Hardware protection | None. All code is CPL 0 in one address space; a single wrong pointer can reach any byte. |
| Application memory access | Every load/store is a bytecode operation performed by the interpreter. The app holds handles and offsets, never raw addresses. The interpreter checks ownership and bounds before touching memory. |
| What it protects | Accidental bugs in cooperative applications: overruns, wild indices, use-after-close. One app cannot corrupt another's regions through normal language operations. |
| What it does not protect | Hostile code, the kernel from the interpreter, or anything from a bug in a bounds check. The guarantee fails **open**: one wrong check means no protection at all. |
| Escape hatches | `syslib` hardware access, DMA-capable devices, any interpreter builtin that takes a raw address, and every `unsafe` block in the kernel. |
| Why fixed regions help | No allocator metadata, no realloc, no cross-task free lists. An app's memory bugs stay within its own regions even when the interpreter does not notice them. |
| Capability model | None. Every application may call every `syslib` primitive (vision R14). |

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
non-executable data and regions (NX is meaningless for interpreted code, but harmless),
and not-present guard pages that fault even at CPL 0. These harden the kernel against its
own bugs; they cannot distinguish "the interpreter writing app A's region" from "the
interpreter writing app B's region", because both are ordinary supervisor writes.

### Sub-allocation

Resolved: none. There is no runtime placement of any kind (ADR 0007). Every offset is
chosen by the compiler; constructors write into caller-provided storage.

### Stacks

Each task has a preallocated native stack, sized in the manifest. The interpreter's own
frames live in this stack, so application recursion depth is bounded by the declared
stack size. A not-present guard page at the growing end raises a page fault even at CPL 0
(ADR 0006, H2). Large locals are allowed in frames; the manifest declares the stack that
must hold the worst case.

The guard is one extra frame immediately below the declared stack, excluded from the
`stack` region (spec 0004 C6). The interpreter checks every frame extent against the
declared length, so a legal overflow terminates the task with `Faulted(StackOverflow)`
before the hardware guard is reachable; the guard is a hard backstop for a kernel bug.
A guard hit names the stack (`stack guard: task N` / `ap core N` / `boot core`) and halts
the machine. Terminating only the task from the hardware fault is deferred
(OPEN-QUESTIONS Q47).

Every kernel stack is guarded too: after early boot the boot core switches to a 256 KiB
stack with one not-present guard frame below it, and each AP's static 256 KiB `.bss`
stack has its own guard frame installed before the SIPIs are sent. The shared machinery
lives in `kernel/src/guard.rs` over `arch/x86_64/src/paging.rs`:

- a cache of 32 4 KiB tables keyed by 2 MiB base; a table is created by splitting the
  covering huge page once, and all 512 entries stay present so several guards share it;
- a registry of up to 96 guarded frames that names the stack in the `#PF` report;
- a local CR3 reload plus an IPI shootdown to every online core after each PTE change,
  because a frame may still be translated by another core's TLB from an earlier life;
- a boot self-check that reads the PTEs back and proves the frame allocator's free count
  did not change, printing `guards: boot N, ap N, tables N` before the SIPIs.

### Hardening (provisional, ADR 0006)

The MMU cannot separate applications at CPL 0, but it can still catch kernel bugs. Three
measures are adopted provisionally — accepted only for as long as they stay cheap:

- **H1 (MUST):** Kernel text and read-only data are mapped read-only, with `CR0.WP = 1`.
- **H2 (MUST):** Every task stack has a not-present guard page at its growing end. The
  guard is one extra frame below the declared region, so the interpreter's frame-extent
  check is the first line of defence; a fault inside a known guard range names the guard
  and halts as a kernel-bug backstop, and per-task recovery from the hardware fault is
  deferred (OPEN-QUESTIONS Q47). Any other fault is a kernel panic.
- **H3 (MUST):** NX is enabled (`EFER.NXE`); kernel data, task stacks and application
  regions are mapped non-executable.
- **H4 (MUST):** All of this lives in the single shared address space: no per-task page
  tables and no CR3 switching.
- **H5 (MUST):** If any measure requires per-task mappings, address-space switching, or a
  fault handler with more than trivial policy, the measure is dropped — not the
  architecture adjusted around it.

## Streams and IPC

Applications communicate through kernel-owned byte FIFOs with fixed capacity. The buffers
are never mapped into an application's addressable regions; the application holds an
endpoint handle that permits only send/receive operations. Data is copied through the
kernel.

- Sends are **atomic**: a send of N bytes either enqueues all N bytes or none. The maximum
  record is therefore the queue capacity.
- Blocking send/receive transition the task to `Blocked`; `try_*` variants return
  `WouldBlock`.
- When a peer dies, queued data is discarded immediately and the surviving endpoint
  observes `Closed`. There is no partial record, because sends are atomic.
- Typed views (integers, strings) are built in `stdlib` on top of the byte FIFO.

The full contract is in `0006-streams.md`.

## Application loading and delivery

Loading is behind a provider interface, so delivery can evolve without touching the
runtime:

```rust
trait SourceProvider {
    fn list(&self) -> ...;
    fn load(&self, name: &str) -> Result<AppSource, LoadError>;
}
```

- **First:** read-only ext2 image delivered as a GRUB module and read through an in-memory
  `BlockDevice` (ADR 0015).
- **Next:** block devices (virtio-blk or ATA) implement the same `BlockDevice`; the
  filesystem driver does not change.
- **Tooling:** two image builders (module now, disk later) that place source files in the
  read-only image, so neither path becomes a dead end. The manifest is produced by the
  compiler at load and is never stored (ADR 0019).
- **Delivery invariant:** the image contains source text only. No bytecode, package,
  object file or compile cache is ever written to it; every start compiles its source in
  the kernel (ADR 0019).

### ext2 subset

The loader implements a deliberately small, read-only subset. Anything outside it is a
mount failure with a diagnostic, never a best-effort read:

| Aspect | Supported |
|--------|-----------|
| Block size | 1 KiB and 4 KiB, read from the superblock |
| Revision | 1 (dynamic); unsupported `INCOMPAT` features refuse the mount |
| Indirect blocks | direct, single and double; triple indirect is `IoError` |
| Entries | regular files only, in the image root; no directories below root, symlinks or devices |
| Matching | exact, case-sensitive byte match |
| Integrity | superblock magic, block size, inode size, bitmap and inode bounds, group descriptors and directory-entry lengths are validated before use |

Source files in the first milestone are small, so the double-indirect ceiling is not a
practical limit. A corrupt image is refused as a whole; there is no partial mount.

The manifest is part of the application contract: region sizes, stack size, entry point,
declared stream endpoints, and the capacity profile. It is generated by the compiler, not
written by hand (`0003-language.md`). It is a runtime value, not a file: it lives only as
long as the instance that was compiled from it.

## REPL service

The only permanent code is the kernel. The shell is an application, written in the
language, compiled at boot. The component that compiles and starts it — and that starts
every subsequent application — is the **REPL service**, a kernel service rather than a
task (ADR 0017).

- It is invoked by the interpreter loop at a service opportunity: before claiming the next
  task, or when the runnable set is empty. It processes at most one request per
  opportunity; a single compile is not interrupted and may hold the core for a long time,
  which is accepted (ADR 0019). Compilation cost is not a design constraint.
- At boot it tries `shell` in the ext2 root; if absent, it tries `entrypoint`. If neither
  exists, or the chosen one fails to compile, it writes `no entry` to the console and
  halts the system. `entrypoint` is for images that boot straight into an application
  instead of a shell. Once started, the chosen task is ordinary and migratable.
- Applications enqueue compile-and-run requests through `core.run` (blocking) or
  `core.spawn` (returns a `TaskId`); `core.wait` waits and returns the exit reason,
  `core.kill` requests termination by id. The queue holds 1024 pending requests by
  default — a compile-time constant, overridable at build time. Each request carries a
  path and a fixed `string[1024]` argument; the kernel does not parse it, and passes it to
  the child's `main(args: const string[1024])`.
- For each request the service resolves the path, reads the source, compiles it, computes
  the manifest, allocates `code`/`state`/`stack`/`fpu`, creates the task with the reserved
  id and enqueues it. The kernel allocates before the task starts, exactly as the memory
  model requires.
- A request path is relative to the image root, at most 256 bytes, with no leading `/`, no
  empty component and no `..`. A malformed path is `Status.Invalid`; a well-formed path
  that is not a regular file is `Invalid`; a path that names nothing is `NotFound`.
- A terminated child's regions are reclaimed immediately; its id and exit reason go to a
  small global ring (64 entries by default) until `wait` consumes them or they are
  evicted. There is no zombie task. Children are independent: if the parent terminates,
  the child continues.
- If every core is held by realtime tasks, the service gets no opportunity and no
  application starts. This follows from the no-reserved-core decision (ADR 0016
  addendum).

The shell is not special: it is a normal task with a normal footprint, and it can be
starved or killed like any other. It implements the prompt, history and line editing in
the language; none of that is kernel work.

## Host interface

The runtime never calls the kernel directly. It calls the **host interface**: the
enumerated set of primitives it may invoke. In the first milestone these are console
read/write, `yield`, stream send/receive/try, a monotonic clock and `sleep_until`.
`syslib` adds port I/O and MMIO handles. The interface is specified in
`0005-host-interface.md` and is mockable, so the language track runs on the host (vision
R7).

## Boot

1. GRUB loads a multiboot2 kernel ELF and the ext2 application image as a module, and
   enters 32-bit protected mode.
2. A 32-bit stub validates the multiboot2 information, builds identity and higher-half
   page tables, enables PAE, long mode and paging, and jumps to the 64-bit entry point in
   the higher half.
3. Early kernel init: GDT/IDT, paging and direct map, serial debug, frame allocator,
   framebuffer detection. The framebuffer tag is optional: with one the kernel allocates
   the full-screen shadow from the frame allocator and attaches the console to it; without
   one (or with an unsupported mode) graphics are disabled and the console stays on serial
   (spec 0009, G15).
4. Platform discovery: memory map, ACPI, APIC, timers, keyboard, and the PS/2 auxiliary
   (mouse) device.
5. Interpreter loop on the boot core; APs are brought up with INIT-SIPI-SIPI through a
   sub-1 MiB trampoline within the milestone (see "SMP": `MAX_CORES` 8, trampoline at
   `0x8000`, parameter page at `0x9000`, per-core TSS/IST/guarded stacks, bounded
   discovery).
6. Loader registers the ext2 provider; the REPL service compiles `shell` from the image
   root and starts it as a task on the boot core. The shell provides the prompt and
   requests further runs through the spawn queue.

Before step 6, the boot core leaves the `.boot` stack for a dedicated kernel stack with a
not-present guard frame below it (one guard frame plus 256 KiB of stack, from the frame
allocator). The guard turns a kernel stack overflow into a `#PF` instead of a write into the
boot page tables; the fault handler runs on its own IST. The in-kernel compiler additionally
has a fixed nesting limit (32 counted levels); exceeding it is a compile `Diagnostic`. The
limit is calibrated to the guarded stack size (Q42; `kernel/src/guard.rs`,
`arch/x86_64/src/stack.rs`).

The same machinery guards the AP stacks and the task stacks: `smp::start` installs the
guard below every static AP stack before the SIPIs, and the loader installs the extra guard
frame below a task's declared stack before the task enters the runnable set. The boot
self-check prints one `guards: boot 1, ap N, tables M` line and halts if a guard is not
actually not-present.

UEFI and real-hardware boot are out of scope (ADR 0014). The boot path lives entirely in
`arch/x86_64`.

## Graphics

The framebuffer is a device the kernel owns, not a drawing surface applications touch
directly (spec 0009, ADR 0020). At boot, after the frame allocator knows the memory map,
`kernel/src/gfx.rs` keeps the multiboot2 mode (address, pitch, width, height, bpp, type)
and allocates one **shadow screen** of `pitch * height` contiguous frames. The shadow is
the single source of truth: every `syslib` drawing primitive, the text console and the
future window service write there, and `present` copies a clipped rectangle to the real
framebuffer row by row with the pitch. Only 32 bpp RGB is supported; any other mode, a
missing tag, bad geometry or a failed allocation disables graphics and leaves the console
on serial.

The screen size is the bootloader's, never a compile-time constant. A rectangular clip
region, initially the whole screen, narrows every primitive and is the seam the window
service uses to repaint one window. The console loses its direct framebuffer path and
presents each glyph cell from the shadow, so there is one image at all times.

## Input

Input is a kernel device, not a per-application resource (spec 0010, ADR 0021). The
service opportunity that already polls COM1 and the PS/2 keyboard also drains the i8042
auxiliary (mouse) device, decodes its 3-byte reports and updates one machine-wide pointer
state (`x`, `y`, buttons). The cursor is a 16×16 monochrome arrow composited at present
time into the framebuffer, never stored in the shadow, so `get_pixel` keeps reading
uncomposited pixels. Applications see only `syslib.pointer_x`/`pointer_y`/
`pointer_buttons` and the debug-only `syslib.inject_pointer`; the console keeps its byte
stream and `core.read_line` for the shell. Keyboard key events and the event queue are
later M2.2 slices.

## Fatal fault path

Two entry points, one terminal state:

```
main returns normally
        └─→ exit(Ok, diag) → reclaim

fault detected (interpreter, guard page, budget)
        │
        ▼
abandon frames back to the application's main level
        │
        ▼
fault(code, diag) on the current core's kernel stack, bounded budget
        │
        ├─ returns / faults / overruns → exit(Faulted(code), diag) → reclaim
        └─ jumps to a main-level recovery label (first fault only)
                 → reset frame stack, resume; budget is not reset
```

`fault` and `exit` run on the core's kernel stack because the task stack may be exhausted.
A handler is never re-entered while running; a fault inside it is abandoned rather than
escalated. A task gets at most one recovery: after a recovery, the next fault is terminal
and `fault` is not called again. `BudgetExceeded` may be logged by `fault`, but recovery
is rejected. `exit` is always attempted at termination, with a small emergency budget; it
is best-effort, not guaranteed (ADR 0010).

The limit is deliberate: a programmer's infinite-loop or repeated-fault bug must not be
able to force the kernel into repeated fault handling. The kernel processes one fault per
task; after that, shutdown.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Task exceeds total budget | Terminate unconditionally at next safe point; `fault(BudgetExceeded)`; recovery rejected; `exit`; reclaim |
| Task exhausts its quantum | Release at next safe point; re-enqueue; no fault |
| Realtime promotion with no free core | `Status.Full`; task stays normal |
| Realtime task misses its heartbeat | Terminate at next safe point; `fault(WatchdogTimeout)`; recovery rejected; `exit`; reclaim |
| Application fault detected by interpreter (out of bounds, capacity, illegal operation) | Abort operation; `fault(code, diag)`; if it returns, `exit(Faulted(code), diag)` then reclaim; if it jumps, resume |
| Stack overflow | Interpreter frame-extent check: terminate with `Faulted(StackOverflow)` and reclaim; the not-present guard below the stack is a hardware backstop that names the guard and halts |
| `fault` returns | Terminal: `exit(Faulted(code), diag)` then reclaim |
| `fault` jumps to a recovery label | Reset frame stack to the label; resume; budget not reset; one recovery per task |
| Second fault after a recovery | Terminal; `fault` is not called again; `exit` then reclaim |
| `fault` faults or exceeds its budget | Abandon handler; `exit` is still attempted; reclaim |
| `exit` faults or exceeds its budget | Abandon handler immediately; reclaim task |
| No `fault` handler defined | Fault is terminal: `exit` then reclaim |
| No `exit` handler defined | Task ends silently; reclaim |
| Task blocks on a stream forever | Remains `Blocked`; no CPU consumed; a timeout may be added later |
| Stream full | Sender blocks or `try_send` returns `WouldBlock`; never grows |
| Stream empty | Receiver blocks or `try_receive` returns `WouldBlock` |
| Stream peer dies | Queued data discarded; surviving endpoint returns `Closed`; blocked peer wakes with `Closed` |
| Arena request cannot be satisfied at load | Application refused; no runtime out-of-memory path |
| ext2 image missing or corrupt | Mount refused; no applications load; kernel still boots to a diagnostic |
| ext2 image uses an unsupported feature or triple indirect | Mount refused with a diagnostic; no partial mount |
| Multiboot2 framebuffer tag absent | Graphics disabled; the console is serial-only; the kernel continues (spec 0009, G15) |
| Framebuffer is not 32 bpp RGB, has bad geometry, or the shadow cannot be allocated | Graphics disabled; the boot line names the mode or size; the console is serial-only |
| Request path malformed or not a regular file | `Status.Invalid`; no task created |
| `shell` missing or fails to compile | Try `entrypoint`; if that also fails, write `no entry` and halt |
| Spawn path not found | `Status.NotFound`; no task created |
| Spawn compile error | `Status.Invalid`; `diag` filled; no task created |
| Request queue full | `Status.Full`; `run` blocks only on the child, never on the queue |
| `wait` on an unknown or evicted id | `Status.NotFound` |
| `kill` on a terminated or unknown id | `Status.NotFound` |
| Parent terminates with live children | Children continue (ADR 0017 addendum 2) |
| Claim protocol violation | Two cores advance the same task; silent corruption. Prevented by the claim word; treated as a TCB defect |
| All cores idle | Each core halts until an IPI or interrupt |
| Interpreter bug (bad pointer, wrong bounds) | Undefined; may corrupt kernel. No recovery boundary exists |
| Kernel panic | Log to serial, halt (or reboot) |

## Open questions

| # | Question | Blocking what |
|---|----------|---------------|
| 1 | Quantum size and total budget value | Tuning, not structure; both are compile-time constants. |
| 2 | ~~Global run queue vs per-core deques with stealing~~ | **Resolved** — one global table of `MAX_TASKS` slots, each under a spin lock; the atomic take is the claim. Per-core deques are a performance option only. |
| 3 | Stream endpoint naming and pairing strictness | `0006-streams.md`; first choice is name-based pairing by the loader. |
| 4 | Console input semantics: byte stream, line buffering, or key events | `0005-host-interface.md`. |
| 5 | ~~AP bring-up details: trampoline vs INIT-SIPI, per-core stacks~~ | **Resolved** — INIT-SIPI-SIPI through the `0x8000` trampoline with a `0x9000` parameter page, observation by attempt, `MAX_CORES` 8, per-core TSS/IST/stacks (see "SMP"). |
| 6 | Debug/introspection surface (task listing, counters, trace) | Deferred; not part of the first host interface. |
| 7 | ~~Whether to reserve one core for the REPL/system when realtime tasks are admitted~~ | **Resolved** — no reservation; realtime may occupy every core (ADR 0016 addendum). |
| 8 | ~~Shell boot fallback: drop to a built-in prompt, or boot without a UI~~ | **Resolved** — try `shell`, then `entrypoint`; otherwise `no entry` and halt (ADR 0017 addendum). |
| 9 | ~~Exit-reason ring size and whether `run` may block on a full queue~~ | **Resolved** — ring default 64 (compile-time constant); a full request queue returns `Status.Full` (ADR 0017 addendum 2). |
| 10 | ~~Whether the argument string is exposed as `core.arguments` or as a `main` parameter~~ | **Resolved** — `main(args: const string[1024])` (ADR 0017 addendum 2). |

## Changelog

| Date | Changes |
|------|---------|
| 2026-11-03 | Initial draft. |
| 2026-11-12 | Replaced pinned tasks with work-stealing interpreter loops (ADR 0011); split the runtime into `core`/`stdlib`/`syslib`; byte-FIFO streams; read-only ext2 delivery; framebuffer console; GRUB/multiboot2 boot with own long-mode transition; unconditional budget kill; no capability model. |
| 2026-11-12 | Added the realtime class: no quantum preemption, admission by core count, one-second heartbeat and `WatchdogTimeout` (ADR 0016). |
| 2026-11-12 | Added the REPL service: kernel-side compile-and-run, `run`/`spawn`/`wait`/`kill`, task ids and the boot shell (ADR 0017). |
| 2026-11-12 | REPL service details: `shell` then `entrypoint` boot fallback, `no entry` halt, fixed `string[1024]` arguments, queue capacity 1024, independent children (ADR 0017 addendum). |
| 2026-11-13 | Floating point is part of the first milestone: `f64` with a kernel-owned per-task `fpu` region, saved on release and restored on claim (ADR 0018). |
| 2026-11-13 | Loader and boot fixed: read-only ext2 subset (1/4 KiB blocks, double indirect, regular root files, validated metadata); request paths validated as relative with no `..`; the multiboot2 framebuffer tag is mandatory; APs come up through a sub-1 MiB trampoline. |
| 2026-11-13 | The interpreter layer is split into a `compiler` and an `interpreter` crate over a shared `abi` (spec 0008). |
| 2026-11-13 | Delivery fixed as source-only: the image never contains bytecode, and the manifest is a runtime value (ADR 0019). |
| 2026-11-15 | APs are real: `MAX_CORES` 8, INIT-SIPI-SIPI through the `0x8000` trampoline, a `0x9000` parameter page, per-core TSS/IST/stacks, observation by attempt with a bounded wait, one IPI vector shared by every core. |
| 2026-11-16 | H2 implemented for boot, AP and task stacks: a shared split-table cache and guard registry (`kernel/src/guard.rs`, `arch/x86_64/src/paging.rs`), a CR3 + wake-IPI TLB shootdown after every PTE change, AP guards installed before the SIPIs, one extra guard frame below each task stack, a boot PTE/free-frame self-check (`guards: boot 1, ap N, tables M`) and a `#PF` line naming the guard. The interpreter's `StackOverflow` check stays the legal path; a hardware guard hit halts (per-task recovery deferred, Q47). |
| 2026-11-16 | Graphics moved to a kernel-owned shadow screen (spec 0009, ADR 0020): the multiboot2 mode lives in `kernel/src/gfx.rs`, the shadow is `pitch * height` contiguous frames allocated after the frame allocator, `present` blits clipped rows with the pitch, the console draws through the shadow, and a rectangular clip is the window-service seam. The framebuffer tag is now optional: a missing or unsupported mode disables graphics and leaves the console on serial instead of halting. |
| 2026-11-16 | Input added as a kernel device (spec 0010, ADR 0021): the i8042 auxiliary device is initialised at boot and polled in the service opportunity next to the keyboard, one machine-wide pointer state is owned by `kernel/src/gfx.rs`, the software cursor is composited at present time (never in the shadow), and applications reach input only through `syslib.pointer_x`/`pointer_y`/`pointer_buttons` and the debug-only `inject_pointer`. |

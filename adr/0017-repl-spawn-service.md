# 0017: The REPL is a kernel service; applications request runs through a spawn queue

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture), 0003 (language), 0005 (host
  interface), 0007 (scheduler)
- **Extends:** ADR 0002 (applications are source), ADR 0007 (static placement), ADR 0015
  (delivery)

> **Note.** The per-task `TaskHandle` model in the Decision section is superseded by
> Addendum 2: tasks are named by global `TaskId`s and there is no per-task handle table.
> Where this document says `join`, read `wait`.

## Context

The only permanently resident code in the system is the kernel. Everything else — the
shell included — is source compiled at load time. That creates a bootstrap problem and a
runtime problem:

- **Bootstrap:** the shell is the system's interface, but it is itself an application. Some
  kernel component must compile it and start it before any application exists.
- **Runtime:** the shell, or any application, needs to run another application. An
  interpreted application cannot allocate another application's footprint; the static
  model forbids runtime placement (ADR 0007). The kernel must do the allocation.
- **Naming:** the component that reads a request, compiles, allocates and starts a task is
  a read-eval-print loop in the broad sense. The interactive prompt belongs to the shell;
  the compile-and-run service belongs to the kernel.

Alternatives were a privileged `core.eval` primitive exposed to applications (moves the
same work into the application ABI and gives every application arbitrary code execution),
no dynamic run at all (the image must be rebuilt for every command), and a REPL
implemented as a task (it still needs the compiler and the ability to allocate another
task's footprint, so it would be a kernel task in disguise).

## Decision

### The REPL is a kernel service, not a task

- It is invoked by the interpreter loop at a **service opportunity**: before claiming the
  next task, or when the runnable set is empty.
- It processes **at most one pending request per opportunity**, so a large compile cannot
  hold a core for an unbounded time. Compilation time is bounded by source size.
- It has no application-visible identity: it is not a task, holds no application regions,
  and cannot be scheduled or killed like one.

### Boot

- After the loader mounts the read-only ext2 image, the REPL reads `shell` from the root,
  compiles it, allocates its footprint, and starts it as a normal task on the boot core.
- Once started, the shell is an ordinary task: it is migratable, preemptible and
  killable like any other.
- If `shell` is missing or fails to compile, the kernel logs the reason and continues
  without an interactive interface (see Addendum 1).

### Requests

Applications request runs through core primitives that enqueue a request in a kernel-owned
message queue:

| Primitive | Semantics |
|-----------|-----------|
| `core.run(path, var reason: ExitReason) -> Status` | Blocking. Equivalent to spawn + join. Returns when the child terminates. |
| `core.spawn(path, var handle: TaskHandle) -> Status` | Non-blocking. Enqueues the request and returns a handle immediately. |
| `core.join(handle, var reason: ExitReason) -> Status` | Blocks until the child terminates, then reaps the handle. |
| `core.kill(handle) -> Status` | Requests termination. `exit` still runs; `join` reaps. |

- A request carries the path, the requesting task and the handle slot to complete.
- The REPL resolves the path in ext2, reads the source, compiles it, computes the
  manifest, allocates `code`/`state`/`stack`, creates the task, and attaches it to the
  requester's handle slot. On failure it completes the request with a `Status` error.
- The queue has a fixed capacity; enqueueing blocks or returns `Status.Full` (see the
  addendum).

### Handles

- `TaskHandle` is an opaque value owned by the requesting task.
- Each task has a fixed-size handle table (first choice: 8 entries). `spawn` returns
  `Status.Full` when it is exhausted.
- A terminated child's regions are reclaimed immediately. Only a small slot survives —
  the child's exit reason — until `join` reaps it or the parent terminates. This is not a
  zombie process: no task state or memory remains.
- `join` on an already-reaped handle returns `Status.Invalid`.
- `kill` requests termination at the child's next safe point; it does not wait. `join`
  observes the result.
- Children are independent: parent termination does not kill them in the first milestone
  (see the addendum).

### Starvation

The REPL is a kernel service on a core. If every core is held by realtime tasks, it gets
no service opportunity and no application can be started. This follows from the
no-reserved-core decision (ADR 0016 addendum) and is accepted.

## Consequences

### Positive

- The shell is an ordinary application, written in the system's language, and the boot
  path proves the thesis: the first useful program is source text compiled by the kernel.
- No `eval` primitive is exposed to applications; arbitrary code execution stays inside
  the kernel service that already owns the compiler.
- Allocation for a new task happens in the kernel, before the task starts, exactly as the
  memory model requires.
- `run` and `spawn` share one queue and one loader path; `run` is a convenience over
  spawn + wait.
- The request queue decouples the caller from compilation time: the caller blocks on the
  child, not on the compiler.

### Negative

- The REPL compiles on a core, so that core is not running application code during a
  compile. Large sources create latency spikes.
- Task ids, the exit-reason ring and a request queue are new kernel state with new
  failure modes (full ring, full queue, orphan children).
- Parent death leaves children running; a spawned server can outlive the shell that
  started it, with no supervisor.
- The shell is not privileged; if it is starved (for example by realtime tasks), the
  interactive interface is gone until a core frees up.
- The boot path depends on a fixed file name (`shell`); a broken image boots to a kernel
  with no UI.

### Neutral / follow-up work

- `run` may be implemented as spawn + wait in `core` rather than as a separate queue
  operation.
- Arguments, working directory and environment are not passed in the first milestone
  (see the addendum).
- The shell can implement command history, completion and line editing itself; none of
  that is kernel work.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| `core.eval(source)` exposed to applications | Every application could compile and run arbitrary code; moves the compiler and allocator into the application ABI. |
| No dynamic run; rebuild the image for every command | Makes the shell a command dispatcher over precompiled commands, not an interactive system. |
| REPL as a task | It still needs the compiler and the ability to allocate another task's footprint; it would be a kernel task with a scheduler identity. |
| REPL executes the child inline on its own stack | No isolation, no scheduling, and the child would share the REPL's fate. |
| Kernel-thread REPL with its own context | Violates the one-interpreter-loop-per-core model (ADR 0011) for a service that is bounded work. |

## Addendum (2026-11-12): boot fallback, arguments, queue capacity, orphans

- **Boot fallback.** The REPL tries `shell` in the ext2 root. If it is absent, it tries
  `entrypoint`. If neither exists, or the chosen one fails to compile, it writes
  `no entry` to the console and halts the system. `entrypoint` is for custom images that
  want to boot straight into an application instead of a shell; it has no special
  semantics beyond being the second name tried.
- **Arguments.** `run` and `spawn` carry a fixed `string[1024]`. The child reads it with
  `core.arguments(var buf: string[1024])`. Splitting, quoting or interpreting the string
  is entirely the child's decision; the kernel never parses it. The 1024-byte capacity is
  a compile-time constant, not a per-call choice.
- **Queue capacity.** The request queue holds 1024 pending requests. It is a compile-time
  constant with a build-time override, in the spirit of `-DQUEUE_CAPACITY=1024`; the Rust
  form is a `build.rs`-generated constant or an `option_env!` parse, not a Cargo feature.
- **Orphan policy.** A spawned child is independent: if the parent terminates, the child
  continues. There is no reparenting and no parent-death kill in the first milestone. The
  parent's regions are reclaimed; the child keeps running.
- **Handle table.** The per-task handle table size remains open; eight is the working
  assumption. **Superseded by Addendum 2 below.**

## Addendum 2 (2026-11-12): task ids, not per-task handles

The per-task handle table and `TaskHandle` are removed. Every task has a global,
monotonically increasing `TaskId` (`u64`; zero is invalid; ids are never reused).

- `core.spawn(path, args, var id)` reserves an id at request time and returns it
  immediately. The application may store the id in its own `state` or forget it; the
  child runs independently either way.
- `core.wait(id, var reason)` blocks if the task is live and returns its exit reason.
  `core.kill(id)` requests termination and does not wait. `core.run` is `spawn` + `wait`.
- `wait` and `kill` are requests to the REPL queue; the service looks the id up in the
  global task table. There is no per-task table and no generation counter, because ids are
  never reused.
- When a task terminates, its id and `ExitReason` are written to a small global ring
  (64 entries by default, compile-time constant). `wait` consumes the record if present;
  otherwise it blocks if the task is live, and returns `NotFound` if it is neither live
  nor in the ring. The oldest record is evicted when the ring is full. This is bounded
  kernel state, not a zombie task: the regions are already reclaimed.
- Any application may wait on or kill any id. There is no capability model; the system is
  single-user and trusted. A stray id can stop another application's child, which is
  consistent with the trust model.

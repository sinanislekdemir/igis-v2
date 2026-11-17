# 0016: Performance measurement

- **Status:** Implemented
- **Scope:** `kernel` (`window`, `console`, `gfx`, `clock`), `samples/apps/perf`;
  M2.6.2 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0031
- **Updated:** 2026-11-17

## Context

M2.6.2 reduces the work of a repair pass and of console output: damage is coalesced
before it is painted and presented (spec 0011, W21), and console cells are batched into
one present per byte run instead of one per cell (spec 0012, D3). Both are bounded,
allocation-free changes to the kernel's own paint path; they do not change any window,
event or pixel contract.

What was missing was **evidence**: a repeatable measurement of the UI's cost on this
machine, and of what the realtime class does to it. The project has one coarse clock
(`core.clock()`, the 1 ms global tick of spec 0007), no performance counters (Q8 stays
deferred) and no host timing inside the kernel. The measurement therefore lives in an
ordinary application, uses only the public surface, and reports what the 1 ms clock can
resolve.

Non-goals:

- no pass/fail threshold: a threshold on a TCG-emulated machine would be noise, and the
  deliverable is the measurement plus the correctness proofs;
- no new opcode, builtin, `syslib` primitive or counter;
- no attempt to benchmark the interpreter, the compiler or block I/O;
- no realtime promotion by the measuring task (it stays a normal, preemptible task);
- no continuous benchmark daemon and no persisted history.

## Requirements

- **P1 (MUST):** `samples/apps/perf` measures two operations with `core.clock()`:
  **frame time** (draw a complete client frame into an owned window and
  `window_present` it, `N` times) and **event latency** (inject one key event, poll it
  back, repaint a marker and present, `N` times). `N` is a compile-time constant, all
  loops are bounded, and the task is ordinary and preemptible.
- **P2 (MUST):** Each phase and metric reports the sample count, the phase total in
  whole milliseconds, and the average, minimum and maximum in microseconds. A sample
  under the clock's 1 ms resolution reads as `0 us` at the minimum; the average over `N`
  samples carries the sub-millisecond signal. The report says so in the sample's comment
  and in this spec; no sub-millisecond claim is made from a single sample.
- **P3 (MUST):** The measurement runs twice: with no other application load, then with
  the realtime controller (`realtime`) and three `worker` tasks spawned with
  `core.spawn` (never `core.run`). The second phase starts immediately after the spawns,
  so the realtime class and the UI coexist during it. Every child is reaped with
  `core.wait` before the sample exits. A refused `realtime_on` inside the child (for
  example `Full` on `-smp 1`) is the child's own report and does not stop the phase.
- **P4 (MUST):** Event delivery is counted: the number of injected events that came
  back from `poll_event` is printed per phase. A lost event is reported as a lower count,
  not silently ignored. The poll loop is bounded, so a competing consumer cannot hang the
  sample.
- **P5 (MUST):** The coalescing/batching reduction is measured without a new counter:
  `window::damage_self_check` (boot line `window: damage ok (present ...)`) reports the
  present rectangles and pixels of a realistic repair before and after coalescing, and
  the console's present count follows from its code path (one per `write_bytes`/`_print`
  flush and one per input service opportunity, not one per cell). The perf sample itself
  measures the end-to-end cost, not the internal split.
- **P6 (MUST):** With graphics disabled, or on a screen too small for the window, the
  sample prints a one-line reason (`perf: no graphics`, `perf: screen too small`,
  `perf: no window`, `perf: no content rect`) and exits without measuring or spinning;
  `langhost` therefore prints `perf: no graphics` and `perf: host ok`. A complete run
  ends `perf: load children reaped` and `perf: ok`.

## Design

The sample owns one 320 × 220 window. Each frame is a white face, a black border, a
moving block, a red progress bar and a text label, all drawn with `syslib` primitives and
presented with one `window_present`. Each event round trip drains at most eight stale
events, injects one `KeyDown` (`EventKind.KeyDown`, the packed `abi::key` code of `A`),
polls until it returns (at most 64 tries), paints a marker and presents.

The load phase spawns `realtime` and three `worker` instances. `worker` crosses several
quanta on purpose, so on `-smp 1` it interleaves with the measuring task; `realtime`
promotes to the realtime class, which is claimed first and is not released at quantum
expiry, so on `-smp 1` it can delay a frame until it blocks, while on `-smp 4` it runs on
another core. Both effects are the load the phase is meant to show. The children print
their own lines through the terminal grid; the sample's own output stays compact.

The event-injecting suite runs on the shell image (`build/igis-shell.iso`), not under the
boot desktop, because the boot desktop is a permanent consumer of the one machine-wide
event queue (spec 0010, I31; spec 0012, D19).

The numbers are TCG/QEMU measurements of this repository's current build, kept in
`AGENTS.md`'s status log. They are evidence, not a contract.

## Interfaces

```text
run perf
```

```text
perf: idle frames 60 total 22 ms avg 366 us min 0 us max 2000 us
perf: idle events 200 total 33 ms avg 165 us min 0 us max 1000 us
perf: idle events delivered 200
perf: load realtime spawned
perf: load frames 60 total 109 ms avg 1816 us min 0 us max 37000 us
perf: load events 200 total 30 ms avg 150 us min 0 us max 5000 us
perf: load events delivered 200
perf: load children reaped
perf: ok
```

`run perf` needs no arguments. The boot line `window: damage ok (present 4 -> 2 rects,
510874 -> 55348 px)` is the coalescing measurement.

## Failure modes

| Case | Result |
|------|--------|
| Graphics disabled | `perf: no graphics`, `perf: host ok`; no window, no spin |
| Screen too small | `perf: screen too small`; no window |
| `window_open`/`window_content_rect` refused | one-line reason; no measurement |
| Child spawn refused | the diagnostic message is printed; the phase still runs |
| Realtime admission `Full` | the child reports it; the phase still runs |
| Injected event lost (a competing consumer) | lower `delivered` count in that phase |
| Clock unchanged during a sample | sample reads `0 us`; the aggregate total is the signal |

## Open questions

| # | Question | Trigger |
|---|----------|---------|
| 1 | A true A/B build toggle for coalescing/batching (a compile-time constant or debug builtin) instead of the arithmetic self-check baseline | if a regression must be bisected, or when Q8 introspection lands |
| 2 | Percentiles (p95/p99) instead of min/avg/max | when the clock is finer than 1 ms or a host provides a TSC-based counter |
| 3 | A sustained-load generator (a long-running CPU task with a stop command) so the load phase outlasts the measurement deterministically | if the build's workload grows enough for the current realtime phase to finish early |
| 4 | Frame-time measurement on real hardware | when the project runs on a physical machine |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial accepted/implemented version with M2.6.2: the perf sample, the two phases, the load recipe, the reporting format, the host path and the no-threshold rule. |

# 0034: Kill requests are id-tagged records resolved under the slot lock

- **Status:** Accepted
- **Date:** 2026-11-17
- **Supersedes:** none
- **Superseded by:** none

## Context

`core.kill` cannot reach a task another core has claimed: the task value lives in that
core's registers and kernel stack, not in the runnable table. The first implementation
parked a bare `AtomicBool` per slot and had the owner check it at its quantum boundary.
That lost requests in two independent ways (OPEN-QUESTIONS Q50):

1. **Lost wake.** The owner checked the flag at the boundary and then blocked. The block
   path printed a trace line first, so the window was milliseconds long on serial. A
   request published in that window found an empty slot, set the flag and returned `Ok`;
   the owner then put the task to sleep with the flag still set. Nothing was runnable, so
   `core.wait` hung forever. The M2.3.5 two-application proof hit this once on `-smp 4`.
2. **Slot reuse.** A bare boolean has no owner, so a publication that interleaved with
   `Task::reclaim` could be read by a later tenant of the same slot, or a stale `true`
   could outlive the task it named.

Both must be fixed together: re-checking only in the block path closes the hang but leaves
the bool unsafe across slot reuse.

## Decision

The per-slot kill record is an **`AtomicU64` holding the target task id** (`0` = none), and
every read, store and clear of it happens under that slot's spin lock:

- `request_kill(id)` scans `SLOT_ID`, takes the slot lock, re-reads the reservation, and
  then either applies the request to the task value in the slot (`kill_requested`, make a
  blocked task `Runnable`) or, when the value is claimed elsewhere, stores `id` in the
  record.
- `task::block` re-checks the record under the slot lock after putting the task back. A
  matching record is consumed with a compare-exchange, `kill_requested` is set and the task
  stays `Runnable`; the scheduler terminates it at the next boundary. All four wait kinds
  go through `block`, so one re-check covers them.
- The quantum boundary keeps its check: `task.kill_requested || task::take_kill(slot, id)`.
- `take_kill(slot, id)` and the block path clear the record only when it names `id`.
- `Task::reclaim` takes the slot lock and clears the record (same compare-exchange) and the
  reservation only when the reservation still names its own id.

Task ids are global, monotonic and never reused (ADR 0017), so a record can never match a
later tenant of its slot.

**Invariant (spec 0007 Sch17):** for a live id, a kill is either observed by the owner or
leaves the task runnable/terminated; a record never outlives its id. A request published
between the boundary check and `release` leaves the task runnable with a pending record,
and the next boundary (or the block path) consumes it; the worst case is one extra quantum.

The lock order is unchanged: the slot lock is the only lock held, no slot lock is held
across `Machine::run`, and kill never holds a stream, console or window lock.

## Consequences

- The lost wake and the stale-record race are both closed without disabling interrupts or
  stopping the world, and without a new opcode, builtin or language surface.
- A kill that races a normal exit may report `Killed` or the exit reason; both are honest.
- `block` and `reclaim` each take their slot lock once more. Neither is on the per-bytecode
  path, and the claim protocol is unchanged.
- The accepted cooperative hole remains: a task that never reaches a safe point cannot be
  killed (spec 0007, "Budget").
- Proof: the boot self-check `kill: records ok` drives the exact interleaving on a real
  slot and real task value before the APs start, and `samples/apps/kill_stress` runs 300
  spawn/kill/wait iterations over the three blocking waits on `-smp 4` and `-smp 1`. The
  same test hangs on its first iteration against the pre-fix kernel.

## Alternatives rejected

- **Block re-check only.** Closes the hang but leaves a boolean that can be misread by a
  later tenant; the task explicitly requires both races to be addressed.
- **Publish under the slot lock with a boolean.** Publishing under the lock makes the
  store/clear atomic but still cannot tell two tenants of one slot apart. The id tag is
  what makes the record tenant-safe, and the task id is already monotonic.
- **A separate slot generation counter.** Equivalent to id tagging but adds a second
  counter to keep in sync; the task id is exactly the generation the record needs.
- **Holding the slot lock across the quantum.** Forbidden: it would serialise every claim
  and block a killer for a whole quantum. The lock is taken only for the decision.
- **Making every kill synchronous** (waiting until the target is reclaimed). Rejected by
  spec 0005: `kill` requests termination and never waits.

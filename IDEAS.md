# Ideas

Unstructured inbox. Cheap to add, cheap to discard. Nothing here is a commitment.

## How this file is used

1. Add one bullet under **Unsorted**, prefixed with the date.
2. When an idea matters enough to design, create a spec in `specs/` and move the bullet
   to **Promoted**, linking the spec.
3. When an idea is ruled out, move it to **Rejected** with one line of reasoning —
   this prevents us from re-litigating it in six months.

Rules of thumb: an idea that implies a lasting constraint deserves an ADR; an idea that
implies behavior deserves a spec; neither is needed for a thought we may never revisit.

## Unsorted

*(none)*

## Promoted

- 2026-11-03 — Keep the repository multi-architecture from day one: all ISA-specific
  code isolated behind a thin contract, with x86_64 as the first implementation.
  → [ADR 0013](adr/0013-x86_64-first-multi-arch.md).
- 2026-11-03 — Hold a "compatibility with the original igis" discussion before deciding
  what v2 keeps; no source exists, so the only continuity is design memory.
  → Resolved in [`specs/0001-vision.md`](specs/0001-vision.md) Context and OQ 2:
  v1 was a 2005 bare-metal front-end for a quantum-cryptography modem; v2 inherits the
  constraints (determinism, no allocator, direct hardware, REPL), not the modem or its UI.

## Rejected

*(none yet)*

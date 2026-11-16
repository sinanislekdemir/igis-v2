# 0026: The standard library may import `syslib` for the UI layout layer

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** project owner
- **Spec:** 0003 (standard library), 0008 (multi-unit compilation), 0009 (graphics)
- **Supersedes:** nothing (narrows the first-slice rule in spec 0008)

## Context

M2.4 begins with a UI layout layer in `stdlib`: text metrics for the 8×8 font,
the System 7.5 palette, and client-drawn chrome (a 1-px frame, a 16-px title bar with
focused stripes, a close box, a separator, centred text). The helpers need the nine
`syslib` drawing primitives (`fill_rect`, `draw_rect`, `draw_text`, ...).

Spec 0008 states that the embedded library "may import only `core`". That rule described
the first slice (parsing, UTF-8, collections, streams), not a security boundary: `stdlib`
is trusted source embedded in the compiler with `include_str!` and compiled into the same
package as the application. An application already has every `syslib` primitive (spec
0003, H4: there is no capability check), so a library call adds no authority the caller
did not have. The compiler's `check_uses` already accepts `syslib` for any unit; only the
spec text and the library header forbade it.

The alternatives are to leave drawing to every application (duplicating the chrome rules
and drifting from the window service), or to add kernel `ui_*` builtins (a new service
surface and a new place for pixel policy; this ticket forbids a new builtin or opcode).

## Decision

We will let `stdlib` say `uses core, syslib;` and put the UI layout/chrome helpers in
`stdlib/stdlib.ig` as ordinary language code:

- only the nine drawing primitives and `core`/existing `stdlib` helpers are used;
- scalar, `bool` and buffer parameters with concrete capacities, plus `Status`/`int`/
  `uint` results — no records, sums, user enums, generics or `state`;
- no new opcode or builtin, and no kernel change;
- the helpers draw *client* chrome. The window service still owns and draws the real
  window frames (spec 0011, ADR 0024); a client that wants a dialog/panel inside its
  content can call the helpers.

Spec 0003 gains the exact metrics, palette and helper signatures; spec 0008 records the
import permission; spec 0009 notes that the layer is stdlib code over the primitives,
not a second drawing path.

## Consequences

### Positive

- One copy of the System 7.5 metrics and chrome rules, shared by every application and
  testable headlessly (`get_pixel`) and on `langhost` (the disabled-screen defaults).
- No ABI growth: the layer is source compiled with the application (ADR 0019), so
  changing a title bar does not change the bytecode contract.
- The library's trust model is unchanged: it is embedded source, not a downloaded unit,
  and the application already has `syslib`.

### Negative

- The library now depends on the `syslib` namespace, so any future port must provide the
  graphics primitives to use the UI layer. This was already true for UI applications.
- Every application that imports `stdlib` compiles the UI helpers too: source size, code
  blob and compile scratch grow even when no window is drawn. Measured and bounded by the
  compiler's cost test.
- The helpers are only as good as the primitives: the 8×8 font is ASCII-only (a
  non-ASCII byte draws a blank cell) and there is no per-task clip, so a covered client
  must still repaint only its visible part.

### Neutral / follow-up work

- `stdlib` constants are not exported across units, so the palette and metrics are
  functions (`ui_white()`, `ui_text_height()`), not constants.
- Records still may not cross the unit boundary. If a future API wants a metrics record,
  the shared type table is a separate decision.
- M2.4.1–M2.4.6 build controls, menus, dialogs and the widgets demo on this layer.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep `stdlib` core-only; every app duplicates the chrome helpers | Duplication and drift: the frame/title/close-box rules already exist once in the window service and would exist once per app here; a fix would land in one place only |
| Add kernel `ui_frame`/`ui_title_bar` builtins | A second pixel-policy surface, a new builtin table entry and a new ABI maintenance cost for code that is not performance-critical; the ticket forbids new builtins and opcodes |
| A separate embedded `uilab` unit | A third unit and a new line-table id for no isolation benefit; `stdlib` is already the language-level library |
| Allow records across units so a metrics record can be returned | There is no shared type table yet (spec 0008); scalar functions and existing out-parameters are sufficient for metrics and keep the slice small |

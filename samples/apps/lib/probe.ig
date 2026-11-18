unit probe;

// Importable module source (spec 0018, M2/M3/M10; ADR 0039, M4.1). It is not
// an application: it declares no `main`, no top-level `var` state and no
// stream endpoint, and its helpers are called as `probe.<name>` by the units
// that import it with `uses lib.probe;`.
//
// The predeclared nominal types `Rect`, `ExitReason`, `Diagnostic` and `Event`
// have fixed ids in every unit (`abi::layout`; spec 0008, "Types"), so they may
// cross this boundary as parameters. User-declared aggregates still cannot
// (M4.2). Ordinary language code only: no new syntax, opcode or builtin.

uses core;

// The area of a rectangle: width times height. Negative extents are not
// rejected; their meaning is the caller's.
function rect_area(r: const Rect): int;
begin
  return r.w * r.h;
end;

// True only for `ExitReason.Ok`. The `case` is exhaustive; `Faulted`'s payload
// binding is a local copy the body ignores.
function reason_ok(reason: const ExitReason): bool;
var
  ok: bool;
begin
  ok := false;
  case reason of
    Ok: ok := true
    Killed: ok := false
    Faulted(code): ok := false
  end;
  return ok;
end;

// The runtime length of a diagnostic's message, in bytes.
function diagnostic_len(d: const Diagnostic): int;
begin
  return int(core.len(d.message));
end;

// True for a key press (`KeyDown` or `KeyUp`), false for a pointer event.
function event_is_key(e: const Event): bool;
begin
  return e.kind = KeyDown or e.kind = KeyUp;
end;

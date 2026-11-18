unit outcome;

// Importable module source (spec 0018, M2/M3/M10; ADR 0039). It classifies a
// child's `ExitReason`, which the sample applications used to do with a local
// `procedure classify_reason` over their own `reason` variable.
//
// The parameter is named `reason` like the callers' variable; the unit
// qualifier in `reason.is_ok(reason)` is resolved through the import table, so
// the two names do not collide. Ordinary language code only: no new syntax,
// opcode or builtin.

uses core;

// True only for `ExitReason.Ok`; the `case` is exhaustive and `Faulted`'s
// payload binding is a local copy the body ignores (spec 0003, "Sum types").
function is_ok(reason: const ExitReason): bool;
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

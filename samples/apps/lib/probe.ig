unit probe;

// Importable module source (spec 0018, M2/M3/M10; ADR 0039, M4.1/M4.2). It is
// not an application: it declares no `main`, no top-level `var` state and no
// stream endpoint, and its helpers are called as `probe.<name>` by the units
// that import it with `uses lib.probe;`.
//
// The predeclared nominal types `Rect`, `ExitReason`, `Diagnostic` and `Event`
// have fixed ids in every unit (`abi::layout`; spec 0008, "Types"), so they may
// cross this boundary as parameters. Since M4.2 this module's own `Point`,
// `Shape` and `Color` are package-global nominal types too: another unit names
// them `probe.Point`, `probe.Shape` and `probe.Color`, and the enum constants
// `probe.Color.Red`. Ordinary language code only: no new syntax beyond the
// unit qualifier, no new opcode or builtin.

uses core;

// A module-defined record (ADR 0039, C2). Its layout is computed once in the
// package-wide type table, so `probe.Point` is the same type everywhere.
type Point = record x: int; y: int; end;

// A module-defined sum. `RectShape` carries a record payload, which is how a
// variant holds more than one field.
type Shape = Circle(radius: int) | RectShape(box: Rect);

// A module-defined enum. Its constants are `probe.Color.Red` outside this
// unit and `Red` inside it.
type Color = (Red, Green, Blue);

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

// Adds two points into `out`, a `var` record parameter. A record cannot be a
// function result (spec 0003), so an aggregate result travels through `var`.
function point_add(a: const Point; b: const Point; out: var Point): Status;
begin
  out.x := a.x + b.x;
  out.y := a.y + b.y;
  return Ok;
end;

// The area of a shape: an exhaustive `case` over the imported-by-the-caller
// sum type. Circle uses a coarse integer pi of 3.
function shape_area(s: const Shape): int;
var
  area: int;
begin
  area := 0;
  case s of
    Circle(radius): area := radius * radius * 3
    RectShape(box): area := box.w * box.h
  end;
  return area;
end;

// The next colour in red -> green -> blue -> red order. An enum is a scalar,
// so it may be a function result.
function next_color(c: Color): Color;
begin
  if c = Red then
    return Green;
  if c = Green then
    return Blue;
  return Red;
end;

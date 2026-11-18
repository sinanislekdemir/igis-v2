unit inject;

// Importable module source (spec 0018, M2/M3/M10; ADR 0039). It builds the
// deterministic test events the sample applications inject through the debug
// hooks (`syslib.inject_pointer`/`syslib.inject_event`), so each application no
// longer repeats the same pointer reset and `Event` field setup.
//
// The module returns the host `Status` and holds no state; draining the queued
// events stays the caller's job because the pump belongs to the application.
// Ordinary language code only: no new syntax, opcode or builtin.

uses core, syslib;

// Clamp the pointer to the origin, discard the queued move, then place it on an
// absolute screen point. The window service sees one move from a known state.
// The `event_flush` result is discarded; the returned status is the placement's.
function pointer_to(x: int; y: int): Status;
var
  st: Status;
begin
  st := syslib.inject_pointer(-100000, -100000, uint(0));
  syslib.event_flush();
  st := syslib.inject_pointer(x, y, uint(0));
  return st;
end;

// Press the primary button without moving the pointer (the caller has placed it
// with `pointer_to`).
function pointer_down(): Status;
var
  st: Status;
begin
  st := syslib.inject_pointer(0, 0, uint(1));
  return st;
end;

// Release the primary button without moving the pointer.
function pointer_up(): Status;
var
  st: Status;
begin
  st := syslib.inject_pointer(0, 0, uint(0));
  return st;
end;

// A `KeyDown` with the packed `Event.code` shape (`text | modifiers << 8 | key
// code << 16`, spec 0010).
function inject_key_mods(key_code: int; text_byte: int; mods: int): Status;
var
  ev: Event;
  st: Status;
begin
  ev.kind := KeyDown;
  ev.x := 0;
  ev.y := 0;
  ev.code := uint(key_code * 65536 + mods * 256 + text_byte);
  ev.window := 0;
  st := syslib.inject_event(ev);
  return st;
end;

// A `KeyDown` with no modifiers.
function inject_key(key_code: int; text_byte: int): Status;
begin
  return inject_key_mods(key_code, text_byte, 0);
end;

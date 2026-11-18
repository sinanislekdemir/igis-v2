unit module_demo;

// In-kernel proof of the `/lib` module path (spec 0018, M1–M13; M3.3), of
// predeclared nominal types crossing a unit boundary (ADR 0039, M4.1) and of
// package-global user types with unit-qualified names (ADR 0039, M4.2).
//
// `lib.math` is `samples/apps/lib/math.ig` and `lib.probe` is
// `samples/apps/lib/probe.ig`, resolved by the kernel's
// `compiler::SourceReader` over the mounted image at load time. The modules'
// helpers are called as `math.*` and `probe.*`; the application itself declares
// no stream endpoint and no new syntax.

uses core, lib.math, lib.probe;

// The module's record, sum and enum cross into the application's `state` (ADR
// 0039, C2). Their type names are qualified by the module's unit.
var
  p: probe.Point;
  s: probe.Shape;
  c: probe.Color;

procedure check(what: const string[0]; passed: bool; ok: var bool);
begin
  if not passed then
  begin
    core.write("module_demo: FAIL ");
    core.write(what);
    core.write("\n");
    ok := false;
  end;
end;

procedure main(args: const string[1024]);
var
  ok: bool;
  r: Rect;
  reason: ExitReason;
  ordinal: FaultCode;
  diag: Diagnostic;
  ev: Event;
  q: probe.Point;
  sum: probe.Point;
  box: Rect;
begin
  ok := true;

  // clamp: inside the band, below the low bound, above the high bound, and the
  // low-above-high rule.
  check("clamp in band", math.clamp(5, 0, 9) = 5, ok);
  check("clamp low edge", math.clamp(0, 0, 9) = 0, ok);
  check("clamp high edge", math.clamp(9, 0, 9) = 9, ok);
  check("clamp below low", math.clamp(0 - 3, 0, 9) = 0, ok);
  check("clamp above high", math.clamp(42, 0, 9) = 9, ok);
  check("clamp low above high", math.clamp(3, 9, 2) = 9, ok);

  // min_int / max_int, both argument orders and negatives.
  check("min_int left", math.min_int(2, 5) = 2, ok);
  check("min_int right", math.min_int(5, 2) = 2, ok);
  check("min_int negative", math.min_int(0 - 7, 0 - 2) = 0 - 7, ok);
  check("max_int left", math.max_int(5, 2) = 5, ok);
  check("max_int right", math.max_int(2, 5) = 5, ok);
  check("max_int negative", math.max_int(0 - 7, 0 - 2) = 0 - 2, ok);

  // abs_int: negative, zero and positive.
  check("abs_int negative", math.abs_int(0 - 7) = 7, ok);
  check("abs_int zero", math.abs_int(0) = 0, ok);
  check("abs_int positive", math.abs_int(7) = 7, ok);

  // sign: -1, 0 and 1.
  check("sign negative", math.sign(0 - 3) = 0 - 1, ok);
  check("sign zero", math.sign(0) = 0, ok);
  check("sign positive", math.sign(3) = 1, ok);

  // Predeclared nominal types cross into `lib.probe` (ADR 0039, C1): a record
  // by value (`const Rect`), a sum matched with an exhaustive `case`, a
  // record's buffer field, and the `EventKind` enum inside an `Event` record.
  r.x := 0;
  r.y := 0;
  r.w := 6;
  r.h := 7;
  check("probe rect_area", probe.rect_area(r) = 42, ok);

  reason := Ok;
  check("probe reason Ok", probe.reason_ok(reason), ok);
  reason := Killed;
  check("probe reason Killed", not probe.reason_ok(reason), ok);
  ordinal := FaultCode.DivideByZero;
  reason := Faulted(ordinal);
  check("probe reason Faulted", not probe.reason_ok(reason), ok);

  diag.message := "boom";
  check("probe diagnostic_len", probe.diagnostic_len(diag) = 4, ok);

  ev.kind := KeyDown;
  check("probe event KeyDown", probe.event_is_key(ev), ok);
  ev.kind := KeyUp;
  check("probe event KeyUp", probe.event_is_key(ev), ok);
  ev.kind := PointerMove;
  check("probe event PointerMove", not probe.event_is_key(ev), ok);

  // A module record crosses the boundary (M4.2): `probe.Point` in the
  // application's state and in a local, a `var` out-parameter, and a whole
  // record copy.
  p.x := 1;
  p.y := 2;
  q.x := 3;
  q.y := 4;
  check("probe point_add status", probe.point_add(p, q, sum) = Ok, ok);
  check("probe point_add x", sum.x = 4, ok);
  check("probe point_add y", sum.y = 6, ok);
  p := sum;
  check("probe point copy", p.x = 4 and p.y = 6, ok);

  // A module sum: construction stays unqualified because the destination names
  // the type, and `case` resolves the variants through the scrutinee.
  s := Circle(3);
  check("probe shape circle", probe.shape_area(s) = 27, ok);
  box.w := 4;
  box.h := 5;
  s := RectShape(box);
  check("probe shape rect", probe.shape_area(s) = 20, ok);

  // A module enum: `probe.Color.Red` outside the module, and the checked cast
  // `probe.Color(ordinal)`.
  c := probe.Color.Red;
  check("probe color red", int(c) = 0, ok);
  c := probe.Color(1);
  check("probe color cast", int(c) = 1, ok);
  check("probe color constant", c = probe.Color.Green, ok);
  c := probe.next_color(c);
  check("probe color next", c = probe.Color.Blue, ok);
  c := probe.next_color(c);
  check("probe color wrap", c = probe.Color.Red, ok);

  if ok then
  begin
    core.write("module_demo: types ok\n");
    core.write("module_demo: lib types ok\n");
    core.write("module_demo: ok\n");
  end;
end;

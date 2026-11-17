unit math;

// Importable module source (spec 0018, M2/M3/M10). It is not an application:
// it declares no `main`, no top-level `var` state and no stream endpoint, and
// its helpers are called as `math.<name>` by the units that import it with
// `uses lib.math;`. M3.3's `module_demo` application is the in-kernel proof.
//
// Ordinary language code only: no new syntax, opcode or builtin.

// Returns `value` limited to `low..=high`. `low` above `high` returns `low`.
function clamp(value: int; low: int; high: int): int;
begin
  if value < low then
    return low;
  if value > high then
    return high;
  return value;
end;

// The smaller of two integers.
function min_int(a: int; b: int): int;
begin
  if a < b then
    return a;
  return b;
end;

// The larger of two integers.
function max_int(a: int; b: int): int;
begin
  if a > b then
    return a;
  return b;
end;

// The magnitude of `v`; `abs_int(MinInt)` wraps like the machine's `0 - v`.
function abs_int(v: int): int;
begin
  if v < 0 then
    return 0 - v;
  return v;
end;

// -1, 0 or 1 according to the sign of `v`.
function sign(v: int): int;
begin
  if v < 0 then
    return -1;
  if v > 0 then
    return 1;
  return 0;
end;

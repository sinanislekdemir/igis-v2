unit ring;

// Fixed-capacity rings (spec 0003, "Rings"): push/pop/peek/full/empty in every
// allowed position, FIFO order, the full/empty failures, whole-ring copies,
// wrap-around and buffer/record/array/nested-ring elements.
//
// Layout: N elements at the element size rounded to 8 come first, then
// `head: u64` and `count: u64`. `head` is the oldest element's physical index,
// `count` the number of live elements; a push writes at `(head + count) % N`.
// The operations are ordinary `ref_element`/`copy_refs`/branch code, so a ring
// adds no opcode.

uses core, stdlib;

const
  Capacity = 3;

type
  Point = record
    x, y: int;
  end;

var
  numbers: ring[int, 4];
  letters: ring[string[8], 2];
  points: ring[Point, 2];
  cells: ring[array[int, 2], 2];
  nested: ring[ring[int, 2], 2];

procedure print_bool(value: bool);
begin
  if value then
    print_int(1)
  else
    print_int(0);
end;

procedure print_int(value: int);
var
  out: string[24];
begin
  if core.format_int_into(out, value, uint(10), uint(0)) >= 0 then
  begin
    core.write(out);
    core.write("\n");
  end;
end;

// A `var` ring parameter is a call-scoped reference: pushes reach the caller.
procedure fill(dst: var ring[int, 4]; base: int);
var
  i: int;
begin
  i := 0;
  while i < 4 do
  begin
    if core.ring_push(dst, base + i) <> Ok then
      i := 4
    else
      i := i + 1;
  end;
end;

// A `const` ring parameter is read-only, so only peek/full/empty are allowed.
function peek_const(src: const ring[int, 4]): int;
var
  value: int;
begin
  value := -1;
  if core.ring_peek(src, value) = Ok then
    return value;
  return -1;
end;

// A `value` ring parameter is a whole copy, header included.
function peek_value(src: ring[int, 4]): int;
var
  value: int;
begin
  value := -1;
  if core.ring_peek(src, value) = Ok then
    return value;
  return -1;
end;

procedure main(args: const string[1024]);
var
  local: ring[int, 4];
  copy: ring[int, Capacity];
  small: ring[int, Capacity];
  inner: ring[int, 2];
  row: array[int, 2];
  out_row: array[int, 2];
  point: Point;
  got: Point;
  drained: array[int, 8];
  drainable: ring[int, 8];
  word: string[8];
  status: Status;
  value: int;
  moved: int;
  i: int;
begin
  // FIFO order: push 10, 20, 30 and pop them in that order.
  status := core.ring_push(numbers, 10);
  status := core.ring_push(numbers, 20);
  status := core.ring_push(numbers, 30);
  status := core.ring_pop(numbers, value);
  print_int(value);
  status := core.ring_pop(numbers, value);
  print_int(value);
  status := core.ring_pop(numbers, value);
  print_int(value);

  // `peek` returns the oldest element without removing it.
  status := core.ring_push(numbers, 40);
  status := core.ring_peek(numbers, value);
  print_int(value);
  status := core.ring_pop(numbers, value);
  print_int(value);

  // Empty and full transitions, and the failures that leave a value alone.
  print_bool(core.ring_empty(numbers));
  print_bool(core.ring_full(numbers));
  value := 99;
  status := core.ring_pop(numbers, value);
  print_int(int(status));
  print_int(value);

  status := core.ring_push(small, 1);
  status := core.ring_push(small, 2);
  status := core.ring_push(small, 3);
  print_bool(core.ring_full(small));
  status := core.ring_push(small, 4);
  print_int(int(status));
  // The failed push wrote nothing: the oldest element is still 1.
  status := core.ring_peek(small, value);
  print_int(value);

  // Wrap around several times: pop the oldest, push one more so the storage
  // wraps (head moves past the end and back to the front), and repeat.
  status := core.ring_pop(small, value);
  print_int(value);
  status := core.ring_push(small, 4);
  status := core.ring_pop(small, value);
  print_int(value);
  status := core.ring_push(small, 5);
  i := 0;
  while i < 3 do
  begin
    status := core.ring_pop(small, value);
    print_int(value);
    i := i + 1;
  end;

  // A whole-ring copy duplicates the storage and the header.
  status := core.ring_push(small, 7);
  status := core.ring_push(small, 8);
  status := core.ring_push(small, 9);
  status := core.ring_pop(small, value);
  copy := small;
  status := core.ring_pop(copy, value);
  print_int(value);
  status := core.ring_peek(small, value);
  print_int(value);
  status := core.ring_push(copy, 55);
  print_bool(core.ring_full(small));
  print_bool(core.ring_full(copy));

  // Parameters: `var` writes through, `const` and `value` read a copy.
  fill(local, 100);
  print_int(peek_const(local));
  print_int(peek_value(local));
  status := core.ring_pop(local, value);
  print_int(value);

  // Buffer elements: the element copy keeps `[len][data]`; the out-parameter
  // keeps its old contents when the ring is empty.
  status := core.ring_push(letters, "alpha");
  status := core.ring_push(letters, "beta");
  status := core.ring_pop(letters, word);
  core.write(word);
  core.write("\n");
  status := core.ring_pop(letters, word);
  core.write(word);
  core.write("\n");
  word := "keep";
  status := core.ring_pop(letters, word);
  print_int(int(status));
  core.write(word);
  core.write("\n");

  // Record elements are copied whole.
  point.x := 3;
  point.y := 4;
  status := core.ring_push(points, point);
  status := core.ring_pop(points, got);
  print_int(got.x + got.y);

  // Array elements are copied whole.
  row[0] := 5;
  row[1] := 6;
  status := core.ring_push(cells, row);
  status := core.ring_pop(cells, out_row);
  print_int(out_row[0] + out_row[1]);

  // Nested rings: a ring of rings.
  status := core.ring_push(inner, 1);
  status := core.ring_push(inner, 2);
  status := core.ring_push(nested, inner);
  status := core.ring_pop(nested, inner);
  status := core.ring_pop(inner, value);
  print_int(value);

  // A stdlib helper takes a ring across the unit boundary.
  status := core.ring_push(drainable, 61);
  status := core.ring_push(drainable, 62);
  status := stdlib.drain_into(drained, moved, drainable);
  print_int(moved);
  print_int(drained[1]);
  print_bool(core.ring_empty(drainable));
end;

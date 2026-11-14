unit array;

// Fixed-size arrays (spec 0003, "Arrays"): declarations, indexing, whole-array
// copies, arrays in records, nested arrays and array parameters.
//
// Layout: N elements at the element size rounded to 8; every element is
// 8-byte aligned. The index check is `index < N` at run time and faults
// OutOfBounds, never a clamp.

uses core, stdlib;

const
  Width = 4;

type
  Point = record
    x, y: int;
  end;

  Row = record
    cells: array[int, 3];
    label: string[16];
  end;

var
  numbers: array[int, Width];
  grid: array[array[int, 2], 2];
  points: array[Point, 2];
  words: array[string[8], 3];
  pieces: array[string[64], 8];
  row: Row;

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

// A `var` array parameter is a call-scoped reference: element writes reach the
// caller's array.
procedure fill(dst: var array[int, Width]; base: int);
var
  i: int;
begin
  i := 0;
  while i < Width do
  begin
    dst[i] := base + i;
    i := i + 1;
  end;
end;

// A `const` array parameter is read-only and reads elements by index.
procedure sum(src: const array[int, Width]; total: var int);
var
  i: int;
begin
  total := 0;
  i := 0;
  while i < Width do
  begin
    total := total + src[i];
    i := i + 1;
  end;
end;

// A `value` array parameter is an ordinary whole-array copy.
function first_value(items: array[int, Width]): int;
begin
  return items[0];
end;

procedure main(args: const string[1024]);
var
  local: array[int, Width];
  copy: array[int, Width];
  i: int;
  total: int;
  parts: int;
begin
  // State array: write then read.
  i := 0;
  while i < Width do
  begin
    numbers[i] := i * 10;
    i := i + 1;
  end;
  print_int(numbers[2]);

  // Whole-array copy: state -> local, local -> local.
  local := numbers;
  print_int(local[3]);
  copy := local;
  print_int(copy[1]);

  // `var` and `const` parameters reach the same storage.
  fill(local, 100);
  sum(local, total);
  print_int(total);
  print_int(first_value(local));

  // Nested arrays compose: `grid[i][j]`.
  grid[0][0] := 1;
  grid[1][1] := 4;
  print_int(grid[0][0] + grid[1][1]);

  // Arrays in records, through field and element steps.
  row.cells[0] := 7;
  row.cells[2] := 9;
  row.label := "row";
  print_int(row.cells[0] + row.cells[2]);
  core.write(row.label);
  core.write("\n");

  // Array of records: field access through an element.
  points[0].x := 3;
  points[0].y := 4;
  points[1] := points[0];
  print_int(points[1].x + points[1].y);

  // Array of buffers: element copies and a byte read.
  words[0] := "alpha";
  words[1] := words[0];
  print_int(int(core.len(words[1])));
  print_int(int(words[1][0]));
  core.write(words[1]);
  core.write("\n");

  // An array parameter across the stdlib boundary: `split_all` fills at most
  // eight pieces and reports the count.
  if stdlib.split_all(pieces, parts, "a,bb,,ccc", byte(44)) = Ok then
  begin
    print_int(parts);
    core.write(pieces[1]);
    core.write("\n");
    print_int(int(core.len(pieces[2])));
  end;
end;

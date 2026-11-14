unit array_bounds;

// The array index check is `index < N` at run time and faults OutOfBounds;
// it is never a clamp (spec 0003, "Arrays"). This sample exists to prove the
// fault on a real compiled package.

uses core;

var
  cells: array[int, 2];
  index: int;

procedure main(args: const string[1024]);
begin
  cells[0] := 10;
  index := 2;
  cells[index] := 30;
  core.write("unreachable\n");
end;

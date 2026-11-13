unit float;

uses core;

var
  x: f64;
  n: int;

procedure main(args: const string[1024]);
begin
  x := 1.5 + 2.25;
  x := x * 2.0;
  n := int(x);
  if n = 7 then
    core.write("float ok\n");
  if core.clock() >= uint(0) then
    core.write("clock ok\n");
end;

unit logic;

uses core;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := 2;
  if n > 1 and n < 3 then
    core.write("and\n");
  if n = 9 or n = 2 then
    core.write("or\n");
  if not (n = 3) then
    core.write("not\n");
end;

unit params;

uses core;

var
  total: int;

procedure add_to(value: var int; amount: int);
begin
  value := value + amount;
end;

procedure main(args: const string[1024]);
var
  n: int;
begin
  total := 0;
  add_to(total, 5);
  n := 0;
  while n < 3 do
  begin
    add_to(n, 2);
    total := total + n;
  end;
  if -total = -11 then
    core.write("params ok\n");
end;

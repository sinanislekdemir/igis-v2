unit loop;

uses core;

const
  Limit = 5;

var
  total: int;
  text: string[32];
  raw: bytes[8];

procedure main(args: const string[1024]);
begin
  total := 0;
  while total < Limit do
  begin
    if total mod 2 = 0 then
      core.write("even\n")
    else
      core.write("odd\n");
    total := total + 1;
  end;
  text := "total=";
  core.write(text);
  raw := "abc";
  core.write(raw);
  core.write("\n");
end;

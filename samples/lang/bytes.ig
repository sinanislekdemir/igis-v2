unit bytesdemo;

uses core;

var
  raw: bytes[8];

procedure main(args: const string[1024]);
begin
  raw := "a\xffb";
  core.write(raw);
  core.write("\n");
end;

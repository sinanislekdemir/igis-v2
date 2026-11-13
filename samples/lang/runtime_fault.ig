unit runtime_fault;

uses core;

var
  a: string[4];
  b: string[4];
  c: string[4];

procedure main(args: const string[1024]);
begin
  core.write("before\n");
  a := "abcd";
  b := "abcd";
  c := a + b;
  core.write("after\n");
end;

unit concat;

uses core;

var
  greeting: string[16];

procedure emit(prefix: const string[0]; text: const string[0]);
begin
  core.write(prefix);
  core.write(text);
  core.write("\n");
end;

procedure set_name(dst: var string[16]; value: const string[0]);
begin
  dst := value;
end;

procedure main(args: const string[1024]);
var
  name: string[16];
begin
  set_name(name, "igis");
  greeting := "hello, " + name;
  emit("greeting: ", greeting);
end;

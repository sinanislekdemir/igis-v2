unit args;

uses core;

procedure main(args: const string[1024]);
begin
  core.write("args=[");
  core.write(args);
  core.write("]\n");
end;

unit bad_send;

// Negative sample: `input` is an `in_stream`, so `core.send` must be a compile
// error even though the handle is the same size as an `out_stream` handle.

uses core;

var
  input: in_stream[16];
  msg: string[8];

procedure main(args: const string[1024]);
begin
  msg := "x";
  core.send(input, msg);
end;

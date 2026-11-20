unit pipe_out128;

// Negative sample. The live `pipe_in` receiver declares `report` with 64
// bytes, so this declaration is a load error: the stream already exists and
// its capacity is fixed (spec 0006, "Dynamic loading").

uses core;

var
  report: out_stream[128];

procedure main(args: const string[1024]);
begin
end;

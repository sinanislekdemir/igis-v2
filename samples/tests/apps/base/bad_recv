unit bad_recv;

// Negative sample: `core.recv` writes raw bytes, so a `string` destination is
// a compile error; a partial read could split a UTF-8 code point.

uses core;

var
  input: in_stream[16];
  text: string[16];

procedure main(args: const string[1024]);
begin
  core.recv(input, text);
end;

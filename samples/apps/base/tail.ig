unit tail;

// `tail [N] path` — print the last N lines of a regular file (default 10).
//
// It is an ordinary application. N is the first word when it parses as a
// decimal integer in `1..=32`; otherwise the whole rest is the path. With no
// path word the cwd itself is the target.
//
// The reader makes two passes without staging the file: pass 1 streams the
// bytes and keeps the last N line-start offsets in a 32-entry array, then pass
// 2 copies from the oldest kept start to EOF. A file with fewer than N lines
// starts at 0, and an empty file prints nothing. A start exactly at EOF is
// skipped, so a trailing newline does not add a phantom empty line.
//
// Errors: `tail: usage: tail [N] <file>`, `tail: no such file or directory`,
// `tail: not a regular file`, `tail: path too long`, `tail: failed <Status>`.

uses core, stdlib, lib.path, lib.out, lib.cli, lib.file_view;

procedure usage();
begin
  out.say("tail: usage: tail [N] <file> (N 1..32)\n");
end;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  first: string[256];
  tail_text: string[256];
  chosen: string[256];
  target: string[256];
  st: Status;
  parsed: Status;
  lines: int;
  value: int;
  has_count: bool;
begin
  st := path.split_cwd(args, cwd, rest);
  if st <> Ok then
  begin
    out.say("tail: bad arguments\n");
    return;
  end;
  st := cli.split_words(rest, first, tail_text);
  if st <> Ok then
  begin
    usage();
    return;
  end;

  lines := 10;
  has_count := false;
  chosen := rest;
  if int(core.len(first)) > 0 then
  begin
    value := 0;
    parsed := stdlib.parse_int_into(first, value);
    if parsed = Ok then
    begin
      if value < 1 or value > 32 then
      begin
        usage();
        return;
      end;
      has_count := true;
      lines := value;
      chosen := tail_text;
    end;
  end;
  if has_count and int(core.len(chosen)) = 0 then
  begin
    usage();
    return;
  end;

  st := path.join(cwd, chosen, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("tail: path too long\n")
    else
      out.say("tail: invalid path\n");
    return;
  end;
  st := file_view.tail(target, lines, "tail");
end;

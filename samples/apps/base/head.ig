unit head;

// `head [N] path` — print the first N lines of a regular file (default 10).
//
// It is an ordinary application. N is the first word when it parses as a
// decimal integer in `1..=64`; otherwise the whole rest is the path, so
// `head /base/child` and the explicit form `run head 3 /base/child` both work.
// With no path word the cwd itself is the target. A final line without a
// newline counts as a line.
//
// Errors: `head: usage: head [N] <file>` for an out-of-range N or a numeric N
// with no path, `head: no such file or directory`, `head: not a regular file`,
// `head: path too long` and `head: failed <Status>`.

uses core, stdlib, lib.path, lib.out, lib.cli, lib.file_view;

procedure usage();
begin
  out.say("head: usage: head [N] <file> (N 1..64)\n");
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
    out.say("head: bad arguments\n");
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
      if value < 1 or value > 64 then
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
      out.say("head: path too long\n")
    else
      out.say("head: invalid path\n");
    return;
  end;
  st := file_view.head(target, lines, "head");
end;

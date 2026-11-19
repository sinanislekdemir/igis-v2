unit file_view;

// Shared streaming readers for the `cat`, `head` and `tail` commands.
//
// Ordinary `/lib` module source: no `main`, no top-level `var` state and no
// stream endpoint. Every reader asks `core.fs_size` first, then copies with
// 256-byte `core.fs_read` chunks into a local buffer and writes the chunk
// exactly as read; nothing is allocated and no whole file is staged.
//
//   file_view.cat(target, cmd)          // offset 0 to EOF
//   file_view.head(target, n, cmd)      // the first n lines
//   file_view.tail(target, n, cmd)      // the last n lines, two passes
//
// `head` counts newlines as it copies and stops the chunk at the n-th one, so
// the first line after the limit is not consumed; a final line without a
// newline is still copied when EOF arrives before the limit. `tail` keeps the
// last `n` line-start offsets in a 32-entry array; a start exactly at EOF is
// skipped, so a trailing newline does not create a phantom empty last line.

uses core, stdlib, lib.out, lib.cli;

// Writes nothing; reports a failed size with a command-specific line.
function size_of(target: const string[0]; cmd: const string[0];
                 total: var uint): Status;
var
  st: Status;
begin
  st := core.fs_size(target, total);
  if st = NotFound then
  begin
    out.say(cmd);
    out.say(": no such file or directory\n");
  end
  else if st = Invalid then
  begin
    out.say(cmd);
    out.say(": not a regular file\n");
  end
  else if st <> Ok then
    cli.fail(cmd, st);
  return st;
end;

// Copies `[start, total)` to the console in 256-byte chunks.
function copy_from(target: const string[0]; cmd: const string[0];
                   start: uint; total: uint): Status;
var
  offset: uint;
  got: int;
  st: Status;
  chunk: bytes[256];
begin
  offset := start;
  while offset < total do
  begin
    got := -1;
    st := core.fs_read(target, offset, chunk, got);
    if st <> Ok then
    begin
      cli.fail(cmd, st);
      return st;
    end;
    if got <= 0 then
    begin
      out.say(cmd);
      out.say(": read error\n");
      return IoError;
    end;
    core.write(chunk);
    offset := offset + uint(got);
  end;
  return Ok;
end;

function cat(target: const string[0]; cmd: const string[0]): Status;
var
  total: uint;
  st: Status;
begin
  st := size_of(target, cmd, total);
  if st <> Ok then
    return st;
  return copy_from(target, cmd, uint(0), total);
end;

function head(target: const string[0]; count: int; cmd: const string[0]): Status;
var
  total: uint;
  offset: uint;
  got: int;
  st: Status;
  chunk: bytes[256];
  stage: bytes[256];
  lines: int;
  i: int;
  take: int;
  copied: int;
begin
  st := size_of(target, cmd, total);
  if st <> Ok then
    return st;
  offset := uint(0);
  lines := 0;
  while offset < total and lines < count do
  begin
    got := -1;
    st := core.fs_read(target, offset, chunk, got);
    if st <> Ok then
    begin
      cli.fail(cmd, st);
      return st;
    end;
    if got <= 0 then
    begin
      out.say(cmd);
      out.say(": read error\n");
      return IoError;
    end;
    i := 0;
    take := 0;
    while i < got and lines < count do
    begin
      if chunk[i] = byte(10) then
        lines := lines + 1;
      i := i + 1;
      take := i;
    end;
    if take > 0 then
    begin
      copied := core.substr_into(stage, chunk, 0, take);
      if copied <> take then
      begin
        out.say(cmd);
        out.say(": read error\n");
        return IoError;
      end;
      core.write(stage);
    end;
    offset := offset + uint(got);
  end;
  return Ok;
end;

// Keeps the last `keep` line starts in `starts`; a full array shifts left.
procedure push_start(starts: var array[int, 32]; count: var int;
                     keep: int; value: int);
var
  i: int;
begin
  if count < keep then
  begin
    starts[count] := value;
    count := count + 1;
  end
  else
  begin
    i := 0;
    while i + 1 < keep do
    begin
      starts[i] := starts[i + 1];
      i := i + 1;
    end;
    starts[keep - 1] := value;
  end;
end;

function tail(target: const string[0]; count: int; cmd: const string[0]): Status;
var
  total: uint;
  offset: uint;
  got: int;
  st: Status;
  chunk: bytes[256];
  starts: array[int, 32];
  kept: int;
  i: int;
  next_start: int;
begin
  st := size_of(target, cmd, total);
  if st <> Ok then
    return st;
  if total = uint(0) then
    return Ok;
  kept := 0;
  push_start(starts, kept, count, 0);
  offset := uint(0);
  while offset < total do
  begin
    got := -1;
    st := core.fs_read(target, offset, chunk, got);
    if st <> Ok then
    begin
      cli.fail(cmd, st);
      return st;
    end;
    if got <= 0 then
    begin
      out.say(cmd);
      out.say(": read error\n");
      return IoError;
    end;
    i := 0;
    while i < got do
    begin
      if chunk[i] = byte(10) then
      begin
        next_start := int(offset) + i + 1;
        if uint(next_start) < total then
          push_start(starts, kept, count, next_start);
      end;
      i := i + 1;
    end;
    offset := offset + uint(got);
  end;
  return copy_from(target, cmd, uint(starts[0]), total);
end;

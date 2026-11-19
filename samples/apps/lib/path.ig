unit path;

// Importable path helpers for the base file commands (ls, cat, head, ...).
//
// It is ordinary `/lib` module source: no `main`, no top-level `var` state, no
// stream endpoint and no kernel service. The commands receive the shell's
// `<cwd> <rest>` argument string and use these helpers to split it and to turn
// a user path into an API path (`""` is the root, `base/child`,
// `home/sub/file`).
//
//   path.split_cwd(args, cwd, rest)   // "<cwd> <rest>" or the explicit run form
//   path.join(cwd, arg, api)          // normalize `.`/`..`/`~` and `/`
//   path.display(api, shown)          // `/`, `/base`, `/home/sub`
//   path.basename(api, name)          // `child` for `base/child`, `""` at root
//
// The API path is relative and never starts or ends with `/`; `.` and `..` are
// resolved here with shell semantics (`..` at the root clamps to the root).
// Every output is staged and published with one `core.substr_into`, so a
// failure leaves the caller's buffer untouched. Buffers stay at or below 256
// bytes: a raw combination longer than 256 is `CapacityExceeded`, even when a
// trailing `..` might have shortened it.
//
// No new syntax, opcode, builtin or `stdlib` function.

uses core, stdlib;

const
  PathCapacity = 256;
  NameCapacity = 64;
  MaxComponents = 8;

// Splits the command argument string into the shell's current display path and
// the user's tail (`rest`).
//
// A bare command is invoked with `args = "<cwd> <rest>"`, where `<cwd>` is a
// display path such as `/` or `/base`; the shell may append one trailing space
// after an empty tail. When `args` starts with something other than `/`, the
// explicit `run <app> ...` form is assumed: the current directory is the root
// and the whole `args` is the tail (so `run ls base` lists `base`).
//
// The cwd is not normalized here: `join` accepts a display or an API path.
function split_cwd(args: const string[0];
                   cwd: var string[256];
                   rest: var string[256]): Status;
var
  length: int;
  at: int;
  cwd_len: int;
  rest_start: int;
  rest_end: int;
  tail_len: int;
  copied: int;
begin
  length := int(core.len(args));
  if length = 0 then
  begin
    copied := core.substr_into(cwd, "/", 0, 1);
    copied := core.substr_into(rest, "", 0, 0);
    return Ok;
  end;

  if args[0] <> byte(47) then
  begin
    copied := core.substr_into(cwd, "/", 0, 1);
    rest_start := 0;
    rest_end := length;
    while rest_start < rest_end and args[rest_start] = byte(32) do
      rest_start := rest_start + 1;
    while rest_end > rest_start and args[rest_end - 1] = byte(32) do
      rest_end := rest_end - 1;
    tail_len := rest_end - rest_start;
    copied := core.substr_into(rest, args, rest_start, tail_len);
    if copied = tail_len then
      return Ok;
    if copied = -6 then
      return CapacityExceeded;
    return Invalid;
  end;

  at := core.find(args, byte(32));
  if at < 0 then
  begin
    cwd_len := length;
    rest_start := length;
  end
  else
  begin
    cwd_len := at;
    rest_start := at + 1;
  end;

  if cwd_len = 0 then
    return Invalid;
  if cwd_len > PathCapacity then
    return CapacityExceeded;
  copied := core.substr_into(cwd, args, 0, cwd_len);
  if copied <> cwd_len then
  begin
    if copied = -6 then
      return CapacityExceeded;
    return Invalid;
  end;

  rest_end := length;
  while rest_start < rest_end and args[rest_start] = byte(32) do
    rest_start := rest_start + 1;
  while rest_end > rest_start and args[rest_end - 1] = byte(32) do
    rest_end := rest_end - 1;
  tail_len := rest_end - rest_start;
  copied := core.substr_into(rest, args, rest_start, tail_len);
  if copied = tail_len then
    return Ok;
  if copied = -6 then
    return CapacityExceeded;
  return Invalid;
end;

// Resolves `arg` against `cwd` and normalizes the result to an API path.
//
// `cwd` may be a display path (`/base`) or an API path (`base`). An empty
// `arg` resolves to `cwd` itself. An `arg` starting with `/` is absolute; `~`
// becomes `home` and `~/x` becomes `home/x`; everything else is relative to
// `cwd`. Empty and `.` components are dropped and `..` pops one component,
// clamped at the root. The result is `""` for the root and never contains `.`,
// `..`, a leading `/`, a trailing `/` or an empty component.
function join(cwd: const string[0];
              arg: const string[0];
              out: var string[256]): Status;
var
  cwd_len: int;
  arg_len: int;
  cwd_start: int;
  cwd_body: int;
  needed: int;
  raw: bytes[256];
  raw_len: int;
  i: int;
  j: int;
  comp_end: int;
  comp_len: int;
  comp_start: int;
  used: int;
  count: int;
  is_dot: bool;
  is_dotdot: bool;
  stage: bytes[256];
  copied: int;
begin
  cwd_len := int(core.len(cwd));
  arg_len := int(core.len(arg));
  cwd_start := 0;
  if cwd_len > 0 and cwd[0] = byte(47) then
    cwd_start := 1;
  cwd_body := cwd_len - cwd_start;

  // Decide the length of the text to normalize before copying any byte, so an
  // oversized request never writes past the staging buffer.
  if arg_len = 0 then
    needed := cwd_body
  else if arg[0] = byte(47) then
    needed := arg_len - 1
  else if arg_len = 1 and arg[0] = byte(126) then
    needed := 4
  else if arg_len >= 2 and arg[0] = byte(126) and arg[1] = byte(47) then
    needed := 4 + (arg_len - 1)
  else if cwd_body > 0 then
    needed := cwd_body + 1 + arg_len
  else
    needed := arg_len;

  if needed > PathCapacity then
    return CapacityExceeded;

  raw_len := 0;
  if arg_len = 0 then
  begin
    i := 0;
    while i < cwd_body do
    begin
      raw[raw_len] := cwd[cwd_start + i];
      raw_len := raw_len + 1;
      i := i + 1;
    end;
  end
  else if arg[0] = byte(47) then
  begin
    i := 1;
    while i < arg_len do
    begin
      raw[raw_len] := arg[i];
      raw_len := raw_len + 1;
      i := i + 1;
    end;
  end
  else if arg_len = 1 and arg[0] = byte(126) then
  begin
    raw[0] := byte(104);  // h
    raw[1] := byte(111);  // o
    raw[2] := byte(109);  // m
    raw[3] := byte(101);  // e
    raw_len := 4;
  end
  else if arg_len >= 2 and arg[0] = byte(126) and arg[1] = byte(47) then
  begin
    raw[0] := byte(104);  // h
    raw[1] := byte(111);  // o
    raw[2] := byte(109);  // m
    raw[3] := byte(101);  // e
    raw_len := 4;
    i := 1;
    while i < arg_len do
    begin
      raw[raw_len] := arg[i];
      raw_len := raw_len + 1;
      i := i + 1;
    end;
  end
  else
  begin
    i := 0;
    while i < cwd_body do
    begin
      raw[raw_len] := cwd[cwd_start + i];
      raw_len := raw_len + 1;
      i := i + 1;
    end;
    if cwd_body > 0 then
    begin
      raw[raw_len] := byte(47);  // '/'
      raw_len := raw_len + 1;
    end;
    i := 0;
    while i < arg_len do
    begin
      raw[raw_len] := arg[i];
      raw_len := raw_len + 1;
      i := i + 1;
    end;
  end;

  used := 0;
  count := 0;
  i := 0;
  while i <= raw_len do
  begin
    comp_start := i;
    comp_end := i;
    while comp_end < raw_len and raw[comp_end] <> byte(47) do
      comp_end := comp_end + 1;
    comp_len := comp_end - comp_start;
    if comp_len > 0 then
    begin
      is_dot := comp_len = 1 and raw[comp_start] = byte(46);
      is_dotdot := comp_len = 2 and raw[comp_start] = byte(46)
        and raw[comp_start + 1] = byte(46);
      if not is_dot then
      begin
        if is_dotdot then
        begin
          if count > 0 then
          begin
            j := used - 1;
            while j >= 0 and stage[j] <> byte(47) do
              j := j - 1;
            if j >= 0 then
              used := j
            else
              used := 0;
            count := count - 1;
          end;
        end
        else
        begin
          if comp_len > NameCapacity then
            return Invalid;
          if count >= MaxComponents then
            return Invalid;
          if used > 0 then
          begin
            if used + 1 + comp_len > PathCapacity then
              return CapacityExceeded;
            stage[used] := byte(47);
            used := used + 1;
          end;
          j := 0;
          while j < comp_len do
          begin
            stage[used] := raw[comp_start + j];
            used := used + 1;
            j := j + 1;
          end;
          count := count + 1;
        end;
      end;
    end;
    i := comp_end + 1;
  end;

  copied := core.substr_into(out, stage, 0, used);
  if copied = used then
    return Ok;
  if copied = -6 then
    return CapacityExceeded;
  return Invalid;
end;

// Formats an API path for display: the root is `/` and every other path is
// `/` followed by the API path. A leading `/` on the input is accepted and
// left alone, so a display path can be passed back unchanged.
function display(api: const string[0]; out: var string[256]): Status;
var
  length: int;
  used: int;
  i: int;
  stage: bytes[256];
  copied: int;
begin
  length := int(core.len(api));
  if length > 0 and api[0] = byte(47) then
  begin
    copied := core.substr_into(out, api, 0, length);
    if copied = length then
      return Ok;
    if copied = -6 then
      return CapacityExceeded;
    return Invalid;
  end;
  if length + 1 > PathCapacity then
    return CapacityExceeded;
  stage[0] := byte(47);
  used := 1;
  i := 0;
  while i < length do
  begin
    stage[used] := api[i];
    used := used + 1;
    i := i + 1;
  end;
  copied := core.substr_into(out, stage, 0, used);
  if copied = used then
    return Ok;
  if copied = -6 then
    return CapacityExceeded;
  return Invalid;
end;

// The last component of an API path, or `""` for the root. A path component is
// at most 64 bytes (spec 0018, L6); a longer one is `CapacityExceeded`.
function basename(api: const string[0]; out: var string[64]): Status;
var
  length: int;
  at: int;
  start: int;
  name_len: int;
  copied: int;
begin
  length := int(core.len(api));
  if length = 0 then
  begin
    copied := core.substr_into(out, "", 0, 0);
    return Ok;
  end;
  at := length - 1;
  while at >= 0 and api[at] <> byte(47) do
    at := at - 1;
  start := at + 1;
  name_len := length - start;
  if name_len > NameCapacity then
    return CapacityExceeded;
  copied := core.substr_into(out, api, start, name_len);
  if copied = name_len then
    return Ok;
  if copied = -6 then
    return CapacityExceeded;
  return Invalid;
end;

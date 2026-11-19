unit cli;

// Small shared helpers for the base file commands.
//
// Ordinary `/lib` module source: no `main`, no top-level `var` state and no
// stream endpoint. It only formats status names and splits a command tail into
// its first word and the rest, so every command prints the same words for a
// `Status` value.

uses core, stdlib, lib.out;

// Writes the stable `Status` name (`NotFound`, `Invalid`, `CapacityExceeded`,
// ...) with no prefix, newline or padding. `Ok` prints as `Ok`.
procedure status_name(st: Status);
begin
  if st = WouldBlock then
    out.say("WouldBlock")
  else if st = Closed then
    out.say("Closed")
  else if st = NotFound then
    out.say("NotFound")
  else if st = Invalid then
    out.say("Invalid")
  else if st = Full then
    out.say("Full")
  else if st = CapacityExceeded then
    out.say("CapacityExceeded")
  else if st = IoError then
    out.say("IoError")
  else
    out.say("Ok");
end;

// `"<cmd>: failed <Status>\n"`.
procedure fail(cmd: const string[0]; st: Status);
begin
  out.say(cmd);
  out.say(": failed ");
  status_name(st);
  out.say("\n");
end;

// True when an API path is `home` or below it, the only writable tree.
function is_home(api: const string[0]): bool;
var
  length: int;
begin
  if api = "home" then
    return true;
  if stdlib.starts_with(api, "home") then
  begin
    length := int(core.len(api));
    if length > 4 and api[4] = byte(47) then
      return true;
  end;
  return false;
end;

// Splits `src` at its first space into `head` (without the space) and `tail`
// (after the space, with further leading spaces skipped). With no space the
// whole source is the head and the tail is empty. Both destinations are
// 256-byte strings; an oversized part is `CapacityExceeded` and leaves that
// destination untouched.
function split_words(src: const string[0];
                    head: var string[256];
                    tail: var string[256]): Status;
var
  length: int;
  at: int;
  start: int;
  tail_len: int;
  copied: int;
begin
  length := int(core.len(src));
  at := core.find(src, byte(32));
  if at < 0 then
  begin
    copied := core.substr_into(head, src, 0, length);
    if copied <> length then
    begin
      if copied = -6 then
        return CapacityExceeded;
      return Invalid;
    end;
    copied := core.substr_into(tail, src, length, 0);
    if copied = 0 then
      return Ok;
    return Invalid;
  end;

  copied := core.substr_into(head, src, 0, at);
  if copied <> at then
  begin
    if copied = -6 then
      return CapacityExceeded;
    return Invalid;
  end;

  start := at + 1;
  while start < length and src[start] = byte(32) do
    start := start + 1;
  tail_len := length - start;
  copied := core.substr_into(tail, src, start, tail_len);
  if copied = tail_len then
    return Ok;
  if copied = -6 then
    return CapacityExceeded;
  return Invalid;
end;

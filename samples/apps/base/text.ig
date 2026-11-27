unit text;

// Kernel sample for the standard library: `run text 42 hi` gives the library a
// real `main(args)` string. It trims the argument, splits it at the first
// space, parses the tail as an `int`, and prints one fixed line per result.

uses core, stdlib, lib.out, lib.path;

var
  trimmed: string[256];
  head: string[128];
  tail: string[256];
  number: int;
  st: Status;
  split: int;
  count: int;
  digits: string[32];

procedure show(value: int);
begin
  count := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
begin
  rest := "";
  st := path.split_cwd(args, cwd, rest);
  count := stdlib.trim_into(trimmed, rest);
  out.say("text: trimmed=[");
  out.say(trimmed);
  out.say("] len=");
  show(count);
  out.say("\n");

  split := stdlib.split_once(head, tail, trimmed, byte(32));
  out.say("text: split=");
  show(split);
  out.say(" head=[");
  out.say(head);
  out.say("] tail=[");
  out.say(tail);
  out.say("]\n");

  st := stdlib.parse_int_into(tail, number);
  if st = Ok then
  begin
    out.say("text: tail is ");
    show(number);
    out.say("\n");
  end
  else if st = Invalid then
    out.say("text: tail is not an integer (Invalid)\n")
  else
    out.say("text: tail is out of range (CapacityExceeded)\n");

  if stdlib.starts_with(head, "4") then
    out.say("text: head starts with 4\n")
  else
    out.say("text: head does not start with 4\n");
  if stdlib.contains_byte(trimmed, byte(32)) then
    out.say("text: trimmed has a space\n")
  else
    out.say("text: trimmed has no space\n");
end;

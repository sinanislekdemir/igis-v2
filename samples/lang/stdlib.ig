unit stdlib_demo;

// Host sample for the embedded standard library (spec 0003, "Standard
// library"). Covers trim, split, parse (including malformed and overflowing
// text) and the two predicates. `langhost` asserts this output byte for byte.

uses core, stdlib;

var
  text: string[256];
  head: string[128];
  tail: string[256];
  number: int;
  st: Status;
  ok: bool;
  count: int;
  digits: string[32];

procedure say(text: const string[0]);
begin
  core.write(text);
end;

procedure show(value: int);
begin
  count := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

procedure main(args: const string[1024]);
begin
  count := stdlib.trim_into(text, "  hi there \t\n");
  say("trim=[");
  say(text);
  say("] len=");
  show(count);
  say("\n");

  count := stdlib.split_once(head, tail, "left/right", byte(47));
  say("split=");
  show(count);
  say(" head=[");
  say(head);
  say("] tail=[");
  say(tail);
  say("]\n");

  count := stdlib.split_once(head, tail, "no-sep", byte(47));
  say("nosplit=");
  show(count);
  say(" head=[");
  say(head);
  say("] tail=[");
  say(tail);
  say("]\n");

  st := stdlib.parse_int_into("-42", number);
  if st = Ok then
  begin
    say("parse=-42 ok value=");
    show(number);
    say("\n");
  end
  else
    say("parse=-42 failed\n");

  st := stdlib.parse_int_into("-9223372036854775808", number);
  if st = Ok then
  begin
    say("parse=min ok value=");
    show(number);
    say("\n");
  end
  else
    say("parse=min failed\n");

  st := stdlib.parse_int_into("-x", number);
  if st = Invalid then
    say("parse=-x invalid\n")
  else
    say("parse=-x unexpected\n");

  st := stdlib.parse_int_into("9223372036854775808", number);
  if st = CapacityExceeded then
    say("parse=overflow capacity exceeded\n")
  else
    say("parse=overflow unexpected\n");

  ok := stdlib.starts_with("igis-v2", "igis");
  if ok then
    say("prefix=yes\n")
  else
    say("prefix=no\n");
  ok := stdlib.starts_with("igis", "igis-v2");
  if ok then
    say("long-prefix=yes\n")
  else
    say("long-prefix=no\n");
  ok := stdlib.contains_byte("hello", byte(108));
  if ok then
    say("contains=yes\n")
  else
    say("contains=no\n");
  ok := stdlib.contains_byte("hello", byte(122));
  if ok then
    say("missing=yes\n")
  else
    say("missing=no\n");
end;

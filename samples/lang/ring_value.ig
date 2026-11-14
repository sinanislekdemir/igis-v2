unit ring_value;

// A `value` ring parameter is an ordinary writable local copy (spec 0003,
// "Rings"): push/pop mutate the callee's copy only, and the caller's ring —
// storage and `head`/`count` — is unchanged. The same rule applies to a ring
// reached through a ring field of a `value` record parameter.

uses core;

type
  Box = record
    queue: ring[int, 2];
    tag: int;
  end;

var
  outer: ring[int, 3];
  outer_box: Box;

procedure print_int(value: int);
var
  out: string[24];
begin
  if core.format_int_into(out, value, uint(10), uint(0)) >= 0 then
  begin
    core.write(out);
    core.write("\n");
  end;
end;

procedure print_bool(value: bool);
begin
  if value then
    print_int(1)
  else
    print_int(0);
end;

// Mutates its own copy: pop 10, push 110, return the popped value.
function copy_roundtrip(src: ring[int, 3]): int;
var
  value: int;
begin
  if core.ring_pop(src, value) <> Ok then
    return -1;
  if core.ring_push(src, value + 100) <> Ok then
    return -2;
  return value;
end;

// The same through a ring field of a value record parameter.
function box_roundtrip(box: Box): int;
var
  value: int;
begin
  value := -1;
  if core.ring_push(box.queue, 7) <> Ok then
    return -1;
  if core.ring_pop(box.queue, value) <> Ok then
    return -2;
  return value;
end;

procedure main(args: const string[1024]);
var
  status: Status;
  value: int;
begin
  status := core.ring_push(outer, 10);
  status := core.ring_push(outer, 20);

  // The callee sees and mutates its copy, returning the popped value.
  print_int(copy_roundtrip(outer));

  // The caller is unchanged: oldest is still 10, then 20, then empty.
  status := core.ring_peek(outer, value);
  print_int(value);
  status := core.ring_pop(outer, value);
  print_int(value);
  status := core.ring_pop(outer, value);
  print_int(value);
  status := core.ring_pop(outer, value);
  print_int(int(status));

  // A ring field of a value record parameter is writable inside the callee.
  print_int(box_roundtrip(outer_box));
  print_bool(core.ring_empty(outer_box.queue));
end;

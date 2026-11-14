unit map;

// Fixed-capacity open-addressing maps (spec 0003, "Maps"): get/put/has/remove/
// count in every allowed position, replacement, the full failure, colliding
// keys that probe past an entry, backward-shift deletion (every survivor stays
// findable), key 0, the empty buffer key, buffer keys, aggregate values, whole
// map copies and const/var/value parameters.
//
// Layout: `count: u64` at offset 0, then a power-of-two table of slots at
// offset 8. A slot is `state` (0 empty, 1 occupied), the stored FNV-1a `hash`,
// the key image and the value image. Hash/probe/delete is one interpreter
// algorithm behind the `map_*` opcodes, so the source sees only the five
// built-in procedures.

uses core, stdlib;

const
  Slots = 3;
  Words = 4;

type
  Point = record
    x, y: int;
  end;

var
  numbers: map[int, int, Slots];
  shared: map[int, int, 8];
  zeros: map[int, int, 2];
  labels: map[int, string[8], 2];
  names: map[string[8], int, Words];
  points: map[int, Point, 2];
  copy: map[int, int, Slots];

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

// A `var` map parameter is a call-scoped reference: the write reaches the
// caller's map.
function put_number(target: var map[int, int, Slots]; key: int; value: int): Status;
begin
  return core.map_put(target, key, value);
end;

// A `const` map parameter is read-only, so only get/has/count are allowed.
function peek_number(source: const map[int, int, Slots]; key: int): int;
var
  value: int;
begin
  value := -1;
  if core.map_get(source, key, value) <> Ok then
    return -999;
  return value;
end;

// A `value` map parameter is a whole copy (header and slots): mutating it here
// never touches the caller's map.
function counted_copy(source: map[int, int, Slots]): int;
var
  status: Status;
begin
  status := core.map_put(source, 99, 99);
  return core.map_count(source);
end;

// The same three modes over the shared eight-entry map.
function put_shared(target: var map[int, int, 8]; key: int; value: int): Status;
begin
  return core.map_put(target, key, value);
end;

function peek_shared(source: const map[int, int, 8]; key: int): int;
var
  value: int;
begin
  value := -1;
  if core.map_get(source, key, value) <> Ok then
    return -999;
  return value;
end;

function copied_count(source: map[int, int, 8]): int;
var
  status: Status;
begin
  status := core.map_put(source, 99, 99);
  return core.map_count(source);
end;

procedure main(args: const string[1024]);
var
  status: Status;
  value: int;
  word: string[8];
  name: string[8];
  point: Point;
  got: Point;
  c: map[int, int, Slots];
begin
  // An empty map: count 0, no key, and a failed get leaves the out-parameter
  // untouched.
  print_int(core.map_count(numbers));
  print_bool(core.map_has(numbers, 5));
  value := 77;
  status := core.map_get(numbers, 5, value);
  print_int(int(status));
  print_int(value);

  // 5, 13 and 21 all hash to table index 0 (capacity 8), so the second and
  // third probe past an occupied slot. All three must be found again.
  status := put_number(numbers, 5, 50);
  status := put_number(numbers, 13, 130);
  status := put_number(numbers, 21, 210);
  print_int(core.map_count(numbers));
  print_int(peek_number(numbers, 5));
  status := core.map_get(numbers, 13, value);
  print_int(value);
  status := core.map_get(numbers, 21, value);
  print_int(value);
  print_bool(core.map_has(numbers, 13));

  // Replacing an existing key writes in place and does not grow the count.
  status := core.map_put(numbers, 13, 131);
  print_int(int(status));
  print_int(core.map_count(numbers));
  status := core.map_get(numbers, 13, value);
  print_int(value);

  // Removing the middle of the cluster shifts the later entry back; 5 and 21
  // must stay findable, and a second remove reports NotFound.
  status := core.map_remove(numbers, 13);
  print_int(int(status));
  print_int(core.map_count(numbers));
  value := 55;
  status := core.map_get(numbers, 13, value);
  print_int(int(status));
  print_int(value);
  status := core.map_get(numbers, 21, value);
  print_int(value);
  status := core.map_remove(numbers, 13);
  print_int(int(status));

  // Three entries are the limit: a fourth distinct key is Full and changes
  // nothing; an existing key still replaces.
  status := core.map_put(numbers, 29, 290);
  print_int(int(status));
  print_int(core.map_count(numbers));
  status := core.map_put(numbers, 37, 370);
  print_int(int(status));
  print_bool(core.map_has(numbers, 37));
  status := core.map_get(numbers, 29, value);
  print_int(value);
  status := core.map_put(numbers, 29, 291);
  status := core.map_get(numbers, 29, value);
  print_int(value);

  // Key 0 is an ordinary key, not a sentinel.
  status := core.map_put(zeros, 0, 7);
  status := core.map_get(zeros, 0, value);
  print_int(value);
  print_bool(core.map_has(zeros, 0));
  print_bool(core.map_has(zeros, 1));

  // Buffer values: a literal is typed by the value type, and `get` writes the
  // `[len][data]` image into the out-parameter.
  status := core.map_put(labels, 1, "one");
  status := core.map_put(labels, 2, "two");
  status := core.map_get(labels, 1, word);
  core.write(word);
  core.write("\n");
  status := core.map_put(labels, 1, "uno");
  status := core.map_get(labels, 1, word);
  core.write(word);
  core.write("\n");

  // Buffer keys: the empty buffer is a real key, and a missing key reports
  // NotFound without touching the destination.
  status := core.map_put(names, "", 11);
  status := core.map_put(names, "a", 22);
  status := core.map_put(names, "b", 33);
  status := core.map_get(names, "", value);
  print_int(value);
  status := core.map_get(names, "a", value);
  print_int(value);
  print_bool(core.map_has(names, "b"));
  name := "a";
  status := core.map_remove(names, name);
  print_int(int(status));
  status := core.map_get(names, "b", value);
  print_int(value);
  value := 66;
  status := core.map_get(names, "zz", value);
  print_int(int(status));
  print_int(value);

  // Aggregate values are copied whole.
  point.x := 3;
  point.y := 4;
  status := core.map_put(points, 7, point);
  status := core.map_get(points, 7, got);
  print_int(got.x + got.y);

  // Parameters: `var` writes through, `const` reads, `value` gets a copy.
  status := put_shared(shared, 1, 10);
  status := put_shared(shared, 2, 20);
  print_int(peek_shared(shared, 1));
  print_int(peek_shared(shared, 9));
  print_int(copied_count(shared));
  print_int(core.map_count(shared));
  print_bool(core.map_has(shared, 99));

  // A whole-map copy duplicates the header and every slot. A `value`
  // parameter works on its own copy first.
  print_int(counted_copy(numbers));
  print_int(core.map_count(numbers));
  c := numbers;
  status := core.map_remove(c, 5);
  print_int(core.map_count(c));
  print_int(core.map_count(numbers));
  status := core.map_get(numbers, 5, value);
  print_int(value);
  status := core.map_get(c, 5, value);
  print_int(int(status));

  // A stdlib helper over a concrete map crosses the unit boundary.
  print_int(stdlib.map_get_or(shared, 2, -1));
  print_int(stdlib.map_get_or(shared, 3, -1));
end;

unit stdlib;

// The standard library, written in the application language (spec 0003,
// "Standard library"; ADR 0019). The compiler embeds this file with
// `include_str!` and compiles it together with any application that says
// `uses stdlib;`. Its top-level names are reachable only as `stdlib.<name>`
// from the application; inside this unit they are unqualified.
//
// Only `core` primitives are used here. There are no new opcodes or builtins:
// every helper is ordinary language code. No `state` is declared, so importing
// the library adds no bytes to an application's `state` region.
//
// Capacities are concrete because the language has no generics. A `var string`
// parameter uses the at-least-N rule, so a destination must be at least as
// large as the declared capacity.

const
  TrimCapacity = 256;
  SplitHeadCapacity = 128;
  SplitTailCapacity = 256;
  Utf8Capacity = 256;
  MaxInt = 9223372036854775807;
  MinInt = -9223372036854775807 - 1;

// Copies `src` without leading or trailing spaces, tabs, CRs and LFs and
// returns the copied length. The destination must hold the whole result:
// a longer result returns -6 (Status.CapacityExceeded) and writes nothing.
function trim_into(dst: var string[256]; src: const string[0]): int;
var
  first: int;
  last: int;
  length: int;
  trimming: bool;
begin
  first := 0;
  last := int(core.len(src));
  trimming := true;
  while first < last and trimming do
  begin
    if src[first] = byte(32) or src[first] = byte(9) or src[first] = byte(10) or src[first] = byte(13) then
      first := first + 1
    else
      trimming := false;
  end;
  trimming := true;
  while last > first and trimming do
  begin
    if src[last - 1] = byte(32) or src[last - 1] = byte(9) or src[last - 1] = byte(10) or src[last - 1] = byte(13) then
      last := last - 1
    else
      trimming := false;
  end;
  length := last - first;
  if length > TrimCapacity then
    return -6;
  return core.substr_into(dst, src, first, length);
end;

// Splits `src` at the first `sep`. Returns 1 with both parts when the byte is
// present, or 0 with `dst_head` = the whole source and `dst_tail` empty when it
// is not. A part that does not fit its destination returns -6 and writes
// nothing further. Uses `core.find` and `core.substr_into` (spec 0003, T4).
function split_once(dst_head: var string[128]; dst_tail: var string[256]; src: const string[0]; sep: byte): int;
var
  at: int;
  length: int;
  copied: int;
begin
  length := int(core.len(src));
  at := core.find(src, sep);
  if at < 0 then
  begin
    if length > SplitHeadCapacity then
      return -6;
    copied := core.substr_into(dst_head, src, 0, length);
    if copied < 0 then
      return copied;
    copied := core.substr_into(dst_tail, src, length, 0);
    if copied < 0 then
      return copied;
    return 0;
  end;
  if at > SplitHeadCapacity then
    return -6;
  copied := core.substr_into(dst_head, src, 0, at);
  if copied < 0 then
    return copied;
  if length - at - 1 > SplitTailCapacity then
    return -6;
  copied := core.substr_into(dst_tail, src, at + 1, length - at - 1);
  if copied < 0 then
    return copied;
  return 1;
end;

// Parses optional leading `-` followed by decimal digits. Returns Ok and writes
// `value`, Invalid for malformed text (empty, `-` alone, any non-digit, a `+`
// sign), or CapacityExceeded when the magnitude does not fit an `int`. Never
// faults: malformed and out-of-range input are data, not errors (spec 0003,
// E1). `-9223372036854775808` is accepted.
function parse_int_into(src: const string[0]; value: var int): Status;
var
  at: int;
  length: int;
  negative: bool;
  digit: int;
  accumulator: int;
begin
  length := int(core.len(src));
  if length = 0 then
    return Invalid;
  at := 0;
  negative := false;
  if src[0] = byte(45) then
  begin
    negative := true;
    at := 1;
  end;
  if at = length then
    return Invalid;
  accumulator := 0;
  while at < length do
  begin
    if src[at] < byte(48) or src[at] > byte(57) then
      return Invalid;
    digit := int(src[at]) - 48;
    if negative then
    begin
      if accumulator < MinInt div 10 then
        return CapacityExceeded;
      if accumulator = MinInt div 10 and digit > 8 then
        return CapacityExceeded;
      accumulator := accumulator * 10 - digit;
    end
    else
    begin
      if accumulator > MaxInt div 10 then
        return CapacityExceeded;
      if accumulator = MaxInt div 10 and digit > 7 then
        return CapacityExceeded;
      accumulator := accumulator * 10 + digit;
    end;
    at := at + 1;
  end;
  value := accumulator;
  return Ok;
end;

// True when `src` begins with `prefix`. Both are read byte-wise, so this is a
// byte-prefix test, not a code-point test.
function starts_with(src: const string[0]; prefix: const string[0]): bool;
var
  at: int;
  length: int;
begin
  length := int(core.len(prefix));
  if length > int(core.len(src)) then
    return false;
  at := 0;
  while at < length do
  begin
    if src[at] <> prefix[at] then
      return false;
    at := at + 1;
  end;
  return true;
end;

// True when the byte appears anywhere in `src` (byte-wise, like `core.find`).
function contains_byte(src: const string[0]; value: byte): bool;
begin
  return core.find(src, value) >= 0;
end;

// Splits `src` at every `sep` into at most 8 destination elements, each a
// `string[64]`. Writes the piece count through `count` and returns Ok. A
// ninth piece returns Full before writing it; a piece longer than 64 bytes
// returns CapacityExceeded and writes nothing further. An empty source or a
// trailing separator yields a final empty piece. This is the first helper to
// exercise a fixed-size array parameter (spec 0003, "Arrays").
function split_all(dst: var array[string[64], 8]; count: var int; src: const string[0]; sep: byte): Status;
var
  at: int;
  start: int;
  length: int;
  piece: int;
begin
  count := 0;
  length := int(core.len(src));
  start := 0;
  while start <= length do
  begin
    if count >= 8 then
      return Full;
    at := start;
    while at < length and src[at] <> sep do
      at := at + 1;
    piece := at - start;
    if piece > 64 then
      return CapacityExceeded;
    if core.substr_into(dst[count], src, start, piece) < 0 then
      return CapacityExceeded;
    count := count + 1;
    if at >= length then
      return Ok;
    start := at + 1;
  end;
  return Ok;
end;

// Moves up to eight integers out of `src` into `dst`, oldest first, and writes
// how many were moved through `count`. Returns Ok, or NotFound when `src` is
// empty (then nothing is written). This is the first helper whose signature
// carries a `ring`, which crosses the unit boundary through the shared
// structural ring table (spec 0003, "Rings").
function drain_into(dst: var array[int, 8]; count: var int; src: var ring[int, 8]): Status;
var
  value: int;
  status: Status;
begin
  count := 0;
  while count < 8 and not core.ring_empty(src) do
  begin
    status := core.ring_pop(src, value);
    if status <> Ok then
      return status;
    dst[count] := value;
    count := count + 1;
  end;
  return Ok;
end;

// Reads `key` from a concrete map and returns `fallback` when it is absent.
// A `const` map parameter is read-only; this is the first helper whose
// signature carries a `map`, which crosses the unit boundary through the
// shared structural map table (spec 0003, "Maps").
function map_get_or(src: const map[int, int, 8]; key: int; fallback: int): int;
var
  value: int;
begin
  if core.map_get(src, key, value) <> Ok then
    return fallback;
  return value;
end;

// Converts the whole of `src` (raw bytes) into a valid UTF-8 `string`. T3: the
// conversion is explicit and checked, so an invalid `string` cannot be built.
// A source longer than the destination capacity returns CapacityExceeded and
// writes nothing; malformed UTF-8 returns Invalid and writes nothing. The
// check itself lives in `core.substr_into`, the only primitive that turns
// `bytes` into a `string`.
function utf8_decode_into(dst: var string[256]; src: const bytes[0]): Status;
var
  length: int;
  copied: int;
begin
  length := int(core.len(src));
  if length > Utf8Capacity then
    return CapacityExceeded;
  copied := core.substr_into(dst, src, 0, length);
  if copied = -4 then
    return Invalid;
  if copied < 0 then
    return CapacityExceeded;
  return Ok;
end;

// Number of code points in `src`: every byte that is not a UTF-8 continuation
// byte (`0x80..0xBF`) starts one code point. The source is a valid `string`,
// so a byte scan is enough (spec 0003, T5: capacities stay in bytes).
function utf8_len(src: const string[0]): int;
var
  at: int;
  count: int;
  length: int;
begin
  length := int(core.len(src));
  at := 0;
  count := 0;
  while at < length do
  begin
    if src[at] < byte(0x80) or src[at] > byte(0xBF) then
      count := count + 1;
    at := at + 1;
  end;
  return count;
end;

// Decodes the code point that starts at `index` in the valid `string` `src`
// and writes its scalar value and the index just past it. Returns Invalid,
// writing nothing, when `index` is negative, at or past the end, points at a
// continuation byte, or names a sequence that would run past the end. No full
// re-validation is needed because `src` is a `string`.
function utf8_decode(src: const string[0]; index: int; codepoint: var int; next: var int): Status;
var
  length: int;
  lead: int;
  width: int;
  value: int;
begin
  length := int(core.len(src));
  if index < 0 or index >= length then
    return Invalid;
  lead := int(src[index]);
  if lead >= 0x80 and lead <= 0xBF then
    return Invalid;
  if lead < 0x80 then
    width := 1
  else if lead < 0xE0 then
    width := 2
  else if lead < 0xF0 then
    width := 3
  else if lead < 0xF8 then
    width := 4
  else
    return Invalid;
  if index + width > length then
    return Invalid;
  if width = 1 then
    value := lead
  else if width = 2 then
    value := (lead - 0xC0) * 0x40 + (int(src[index + 1]) - 0x80)
  else if width = 3 then
    value := (lead - 0xE0) * 0x1000
      + (int(src[index + 1]) - 0x80) * 0x40
      + (int(src[index + 2]) - 0x80)
  else
    value := (lead - 0xF0) * 0x40000
      + (int(src[index + 1]) - 0x80) * 0x1000
      + (int(src[index + 2]) - 0x80) * 0x40
      + (int(src[index + 3]) - 0x80);
  codepoint := value;
  next := index + width;
  return Ok;
end;

// Strict UTF-8 validator for raw bytes, matching the interpreter's rules
// (spec 0003, T3): 1- to 4-byte sequences only, no overlong encodings
// (`C0`/`C1`, `E0 80..`, `F0 80..`), no surrogates (`ED A0..`), nothing above
// `U+10FFFF` (`F4 90..`, `F5..FF`), no truncated sequence and no stray
// continuation byte. It checks bytes without converting them and cross-checks
// `utf8_decode_into`'s primitive.
function utf8_valid(src: const bytes[0]): bool;
var
  at: int;
  length: int;
  lead: int;
  extra: int;
  low: int;
  high: int;
  step: int;
begin
  length := int(core.len(src));
  at := 0;
  while at < length do
  begin
    lead := int(src[at]);
    if lead < 0x80 then
      at := at + 1
    else
    begin
      if lead >= 0xC2 and lead <= 0xDF then
      begin
        extra := 1;
        low := 0x80;
        high := 0xBF;
      end
      else if lead >= 0xE0 and lead <= 0xEF then
      begin
        extra := 2;
        low := 0x80;
        high := 0xBF;
        if lead = 0xE0 then
          low := 0xA0
        else if lead = 0xED then
          high := 0x9F;
      end
      else if lead >= 0xF0 and lead <= 0xF4 then
      begin
        extra := 3;
        low := 0x80;
        high := 0xBF;
        if lead = 0xF0 then
          low := 0x90
        else if lead = 0xF4 then
          high := 0x8F;
      end
      else
        return false;
      if at + extra >= length then
        return false;
      if int(src[at + 1]) < low or int(src[at + 1]) > high then
        return false;
      step := 2;
      while step <= extra do
      begin
        if int(src[at + step]) < 0x80 or int(src[at + step]) > 0xBF then
          return false;
        step := step + 1;
      end;
      at := at + extra + 1;
    end;
  end;
  return true;
end;

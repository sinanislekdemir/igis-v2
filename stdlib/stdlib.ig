unit stdlib;

// The standard library, written in the application language (spec 0003,
// "Standard library"; ADR 0019). The compiler embeds this file with
// `include_str!` and compiles it together with any application that says
// `uses stdlib;`. Its top-level names are reachable only as `stdlib.<name>`
// from the application; inside this unit they are unqualified.
//
// `core` primitives are used for the language services, and `syslib` for the
// UI layout/chrome layer (ADR 0026): the library is trusted embedded source,
// and an application already has every `syslib` primitive, so the import grants
// nothing new. There are no new opcodes or builtins: every helper is ordinary
// language code. No `state` is declared, so importing the library adds no bytes
// to an application's `state` region.
//
// Capacities are concrete because the language has no generics. A `var string`
// parameter uses the at-least-N rule, so a destination must be at least as
// large as the declared capacity.

uses core, syslib;

const
  TrimCapacity = 256;
  SplitHeadCapacity = 128;
  SplitTailCapacity = 256;
  Utf8Capacity = 256;
  FrameCapacity = 256;
  FrameHeader = 4;
  FramePayloadCapacity = 252;
  LineCapacity = 255;
  MaxInt = 9223372036854775807;
  MinInt = -9223372036854775807 - 1;
  UiTitleCapacity = 64;
  UiGlyphWidth = 8;
  UiGlyphHeight = 8;
  UiLineHeight = 16;
  UiContentInset = 8;
  UiTitleBarHeight = 16;
  UiCloseBoxSize = 11;
  UiBlack = 0x000000;
  UiWhite = 0x00FFFFFF;
  UiGray = 0x00CCCCCC;
  UiAccent = 0x000080;
  StrEditCapacity = 256;

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

// Parses decimal text into an `f64` (spec 0003, "Floating point"; Q41).
//
// Grammar: `[+-]? ( digits [ '.' digits? ] | '.' digits ) ( [eE] [+-]? digits )?`
// with at least one mantissa digit. The exact strings `inf`, `+inf`, `-inf` and
// `nan` — the formatter's own output — are accepted too, so a format→parse
// round trip works. Anything else is Invalid and writes nothing.
//
// The mantissa is accumulated as an `f64` and the exponent applied by repeated
// multiplication or division by 10. That is the milestone's fixed-point
// precision, not correct rounding (Q41): shortest round-trip formatting and
// exact IEEE parsing are deferred. A magnitude that overflows `f64` becomes
// ±Inf and an underflow becomes ±0, both with Ok; the sign of `-0.0` is
// preserved. No input faults, and a failure never touches `value`.
function parse_f64_into(src: const string[0]; value: var f64): Status;
var
  at: int;
  length: int;
  negative: bool;
  negative_exponent: bool;
  seen_digit: bool;
  digit: int;
  exponent: int;
  step: int;
  mantissa: f64;
  fraction: f64;
  fraction_scale: f64;
  seen_significant: bool;
  taken: int;
  zero: f64;
begin
  length := int(core.len(src));
  if length = 0 then
    return Invalid;
  // The formatter's specials. There are no literals for `NaN` and the
  // infinities (spec 0003, "Floating point"), so IEEE arithmetic produces
  // them: `0.0 / 0.0` is `NaN`, `±1.0 / 0.0` is `±Inf`.
  if src = "nan" then
  begin
    zero := 0.0;
    value := zero / zero;
    return Ok;
  end;
  if src = "inf" or src = "+inf" then
  begin
    zero := 0.0;
    value := 1.0 / zero;
    return Ok;
  end;
  if src = "-inf" then
  begin
    zero := 0.0;
    value := -1.0 / zero;
    return Ok;
  end;

  at := 0;
  negative := false;
  if src[at] = byte(43) then
    at := at + 1
  else if src[at] = byte(45) then
  begin
    negative := true;
    at := at + 1;
  end;
  if at = length then
    return Invalid;

  mantissa := 0.0;
  seen_digit := false;
  while at < length and src[at] >= byte(48) and src[at] <= byte(57) do
  begin
    digit := int(src[at]) - 48;
    mantissa := mantissa * 10.0 + f64(digit);
    seen_digit := true;
    at := at + 1;
  end;
  if at < length and src[at] = byte(46) then
  begin
    at := at + 1;
    // Fraction digits are accumulated as an integer numerator over a power of
    // ten, so `0.375` is exact where summing `3*0.1 + 7*0.01 + 5*0.001` would
    // not be. Only 18 significant digits are kept: the rest are below `f64`
    // resolution. Leading zeros only move the scale, so a tiny value such as
    // `0.0000001` keeps its digits.
    fraction := 0.0;
    fraction_scale := 1.0;
    seen_significant := false;
    taken := 0;
    while at < length and src[at] >= byte(48) and src[at] <= byte(57) do
    begin
      digit := int(src[at]) - 48;
      if not seen_significant and digit = 0 then
        fraction_scale := fraction_scale * 10.0
      else
      begin
        seen_significant := true;
        if taken < 18 then
        begin
          fraction := fraction * 10.0 + f64(digit);
          fraction_scale := fraction_scale * 10.0;
          taken := taken + 1;
        end;
      end;
      seen_digit := true;
      at := at + 1;
    end;
    if taken > 0 then
      mantissa := mantissa + fraction / fraction_scale;
  end;
  if not seen_digit then
    return Invalid;

  exponent := 0;
  negative_exponent := false;
  if at < length and (src[at] = byte(101) or src[at] = byte(69)) then
  begin
    at := at + 1;
    if at = length then
      return Invalid;
    if src[at] = byte(43) then
      at := at + 1
    else if src[at] = byte(45) then
    begin
      negative_exponent := true;
      at := at + 1;
    end;
    if at = length then
      return Invalid;
    if src[at] < byte(48) or src[at] > byte(57) then
      return Invalid;
    while at < length and src[at] >= byte(48) and src[at] <= byte(57) do
    begin
      // The bound only keeps the `int` accumulator in range: the scaling
      // below saturates long before it, so a larger exponent cannot change a
      // result that is already ±Inf or ±0.
      if exponent < 1000000 then
        exponent := exponent * 10 + (int(src[at]) - 48);
      at := at + 1;
    end;
  end;
  if at <> length then
    return Invalid;

  // Apply the exponent. Both loops stop once the value has saturated, so the
  // cost does not grow with the exponent the text names.
  step := 0;
  if negative_exponent then
  begin
    while step < exponent and mantissa <> 0.0 do
    begin
      mantissa := mantissa / 10.0;
      step := step + 1;
    end;
  end
  else
  begin
    while step < exponent and mantissa <> 0.0 and mantissa < 1.0e308 do
    begin
      mantissa := mantissa * 10.0;
      step := step + 1;
    end;
    // The loop stopped above the largest finite value but with exponent left:
    // one more multiplication overflows to `Inf`, which is the true result.
    if step < exponent and mantissa <> 0.0 then
      mantissa := mantissa * 10.0;
  end;

  if negative then
    mantissa := -mantissa;
  value := mantissa;
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

// ---- string editing and selection (spec 0003, "String editing") ---------
//
// Code-point-safe editing for a 256-byte text buffer, as ordinary language
// code. `string` stays read-only by index (spec 0003, T4), so a mutator never
// writes into its destination in place: it copies the result into a local
// `bytes[256]`, converts that back to a `string` with `core.substr_into` (which
// validates strict UTF-8) and publishes it with one whole-buffer assignment.
// A rejected call therefore leaves the destination byte-identical and the text
// stays valid UTF-8.
//
// Positions are byte offsets into the `string`, never code-point ordinals
// (T5: capacities and lengths are bytes). "Boundary" means the offset is 0, the
// length, or the first byte of a code point. On a valid `string` that is
// exactly "the byte is not a continuation byte (0x80..0xBF)".
//
// The 256-byte working capacity is the declared parameter capacity; the
// language's at-least-N rule accepts a larger destination buffer, so a runtime
// length above 256 is refused with `CapacityExceeded` rather than risking a
// partial or out-of-bounds copy. Cursor helpers clamp and return a value; the
// mutators never fault.

// True when `index` is a code-point boundary of the valid `string` `text`: 0,
// the runtime length, or the first byte of a code point. Negative and
// past-the-end offsets are not boundaries.
function str_boundary(text: const string[0]; index: int): bool;
var
  length: int;
begin
  length := int(core.len(text));
  if index < 0 or index > length then
    return false;
  if index = 0 or index = length then
    return true;
  if text[index] >= byte(0x80) and text[index] <= byte(0xBF) then
    return false;
  return true;
end;

// Clamps `cursor` into 0..len and to a code-point boundary: a negative offset
// becomes 0, an offset past the end becomes the length, and an offset inside a
// multi-byte code point moves back to that code point's first byte. A position
// already on a boundary is returned unchanged. This is the rule every cursor
// helper below applies; none of them returns `Invalid` for a bad cursor.
function str_cursor_clamp(text: const string[0]; cursor: int): int;
var
  length: int;
  at: int;
begin
  length := int(core.len(text));
  if cursor <= 0 then
    return 0;
  if cursor >= length then
    return length;
  at := cursor;
  while at > 0 and text[at] >= byte(0x80) and text[at] <= byte(0xBF) do
    at := at - 1;
  return at;
end;

// Moves the clamped cursor back one whole code point and returns it. At 0 it
// stays 0. A cursor inside a code point is clamped first, then the code point
// before that boundary is crossed.
function str_cursor_left(text: const string[0]; cursor: var int): int;
var
  at: int;
  previous: int;
begin
  at := str_cursor_clamp(text, cursor);
  if at > 0 then
  begin
    previous := at - 1;
    while previous > 0 and text[previous] >= byte(0x80) and text[previous] <= byte(0xBF) do
      previous := previous - 1;
    at := previous;
  end;
  cursor := at;
  return at;
end;

// Moves the clamped cursor forward one whole code point and returns it. At the
// length it stays there; a cursor inside a code point is clamped first, so the
// next move crosses the code point that contains it.
function str_cursor_right(text: const string[0]; cursor: var int): int;
var
  at: int;
  length: int;
begin
  at := str_cursor_clamp(text, cursor);
  length := int(core.len(text));
  if at < length then
  begin
    at := at + 1;
    while at < length and text[at] >= byte(0x80) and text[at] <= byte(0xBF) do
      at := at + 1;
  end;
  cursor := at;
  return at;
end;

// Sets the cursor to 0 and returns it. The text is not needed.
function str_cursor_home(cursor: var int): int;
begin
  cursor := 0;
  return 0;
end;

// Sets the cursor to the runtime length and returns it.
function str_cursor_end(text: const string[0]; cursor: var int): int;
var
  length: int;
begin
  length := int(core.len(text));
  cursor := length;
  return length;
end;

// Inserts `insert` at byte offset `at`, which must be a code-point boundary.
// `at = len` appends, `at = 0` prepends. Returns Ok, or Invalid (nothing
// changed) for a negative offset, an offset past the end or a non-boundary, or
// CapacityExceeded (nothing changed) when the result would exceed 256 bytes.
// The inserted text is itself a valid `string`, so the result is valid UTF-8.
function str_insert_into(text: var string[256]; at: int; insert: const string[0]): Status;
var
  length: int;
  add: int;
  i: int;
  raw: bytes[256];
  staged: string[256];
  copied: int;
begin
  length := int(core.len(text));
  if length > StrEditCapacity then
    return CapacityExceeded;
  if at < 0 or at > length then
    return Invalid;
  if not str_boundary(text, at) then
    return Invalid;
  add := int(core.len(insert));
  if add > StrEditCapacity - length then
    return CapacityExceeded;

  i := 0;
  while i < at do
  begin
    raw[i] := text[i];
    i := i + 1;
  end;
  i := 0;
  while i < add do
  begin
    raw[at + i] := insert[i];
    i := i + 1;
  end;
  i := at;
  while i < length do
  begin
    raw[i + add] := text[i];
    i := i + 1;
  end;

  // The result is valid UTF-8 by construction (two valid strings joined on a
  // boundary); the conversion is still the only publish path and would refuse a
  // malformed result instead of building an invalid `string`.
  copied := core.substr_into(staged, raw, 0, length + add);
  if copied <> length + add then
    return Invalid;
  text := staged;
  return Ok;
end;

// Deletes `count` bytes at offset `at`. Both `at` and `at + count` must be
// code-point boundaries inside the text, so only whole code points are removed;
// `count = 0` is Ok and changes nothing. Returns Ok, or Invalid (nothing
// changed) for a negative offset or count, an offset past the end, a range that
// runs past the end, or either end inside a code point. Deletion never needs to
// grow the text, so its own result always fits; CapacityExceeded appears only
// when the incoming text already exceeds the helper's 256-byte working
// capacity (an at-least-N caller with a longer runtime length).
function str_delete_into(text: var string[256]; at: int; count: int): Status;
var
  length: int;
  i: int;
  raw: bytes[256];
  staged: string[256];
  copied: int;
begin
  length := int(core.len(text));
  if length > StrEditCapacity then
    return CapacityExceeded;
  if at < 0 or at > length then
    return Invalid;
  if count < 0 or count > length - at then
    return Invalid;
  if not str_boundary(text, at) or not str_boundary(text, at + count) then
    return Invalid;
  if count = 0 then
    return Ok;

  i := 0;
  while i < at do
  begin
    raw[i] := text[i];
    i := i + 1;
  end;
  i := at + count;
  while i < length do
  begin
    raw[i - count] := text[i];
    i := i + 1;
  end;

  copied := core.substr_into(staged, raw, 0, length - count);
  if copied <> length - count then
    return Invalid;
  text := staged;
  return Ok;
end;

// Deletes the code point immediately before the clamped cursor and moves the
// cursor to that code point's first byte. At 0 it is Ok and does nothing (the
// cursor stays 0), because a text field always receives the key and there is
// nothing to delete: that is an empty state, not an error. The text is
// untouched when the delete is rejected, and the cursor is only written on
// success (apart from the documented clamp).
function str_backspace(text: var string[256]; cursor: var int): Status;
var
  length: int;
  at: int;
  previous: int;
  st: Status;
begin
  length := int(core.len(text));
  if length > StrEditCapacity then
    return CapacityExceeded;
  at := str_cursor_clamp(text, cursor);
  if at = 0 then
  begin
    cursor := 0;
    return Ok;
  end;
  previous := at - 1;
  while previous > 0 and text[previous] >= byte(0x80) and text[previous] <= byte(0xBF) do
    previous := previous - 1;
  st := str_delete_into(text, previous, at - previous);
  if st <> Ok then
    return st;
  cursor := previous;
  return Ok;
end;

// Clamps both scalars to code-point boundaries, orders them (the result always
// satisfies `start <= stop`) and returns the selection length in bytes, 0 for a
// collapsed selection. The pair is the scalar selection representation: no
// record crosses the unit boundary.
function str_selection_normalize(text: const string[0]; start: var int; stop: var int): int;
var
  first: int;
  last: int;
  swap: int;
begin
  first := str_cursor_clamp(text, start);
  last := str_cursor_clamp(text, stop);
  if first > last then
  begin
    swap := first;
    first := last;
    last := swap;
  end;
  start := first;
  stop := last;
  return last - first;
end;

// Selects the whole text: `start = 0`, `stop = len`, returning the length.
function str_selection_all(text: const string[0]; start: var int; stop: var int): int;
var
  length: int;
begin
  length := int(core.len(text));
  start := 0;
  stop := length;
  return length;
end;

// Collapses the selection to one end: `to_stop` picks the `stop` end, otherwise
// the `start` end. The chosen scalar is clamped to a code-point boundary, both
// scalars are set to it, and the caret is returned. Calling this on an
// unnormalized pair is safe.
function str_selection_collapse(text: const string[0]; start: var int; stop: var int; to_stop: bool): int;
var
  caret: int;
begin
  if to_stop then
    caret := str_cursor_clamp(text, stop)
  else
    caret := str_cursor_clamp(text, start);
  start := caret;
  stop := caret;
  return caret;
end;

// Replaces the selection with `insert` in one publish. The pair is normalized
// first (clamped and ordered), so an out-of-range or in-code-point selection
// never faults. On success the selection collapses to the caret just past the
// inserted text (`start = stop = first + len(insert)`) and the result is valid
// UTF-8. A result over 256 bytes returns CapacityExceeded with the text and both
// scalars untouched. The whole result is staged and published once rather than
// calling `str_delete_into` then `str_insert_into`: two publishes would expose
// the deletion if the insertion then failed, so a single copy is what makes the
// operation all-or-nothing.
function str_selection_replace(text: var string[256]; start: var int; stop: var int; insert: const string[0]): Status;
var
  length: int;
  first: int;
  last: int;
  swap: int;
  add: int;
  new_length: int;
  i: int;
  raw: bytes[256];
  staged: string[256];
  copied: int;
begin
  length := int(core.len(text));
  if length > StrEditCapacity then
    return CapacityExceeded;
  first := str_cursor_clamp(text, start);
  last := str_cursor_clamp(text, stop);
  if first > last then
  begin
    swap := first;
    first := last;
    last := swap;
  end;
  add := int(core.len(insert));
  if add > StrEditCapacity - (length - (last - first)) then
    return CapacityExceeded;

  i := 0;
  while i < first do
  begin
    raw[i] := text[i];
    i := i + 1;
  end;
  i := 0;
  while i < add do
  begin
    raw[first + i] := insert[i];
    i := i + 1;
  end;
  i := last;
  while i < length do
  begin
    raw[first + add + (i - last)] := text[i];
    i := i + 1;
  end;

  new_length := length - (last - first) + add;
  copied := core.substr_into(staged, raw, 0, new_length);
  if copied <> new_length then
    return Invalid;
  text := staged;
  start := first + add;
  stop := first + add;
  return Ok;
end;

// ---- typed stream views (spec 0006, "Receive" and "Interfaces") --------
//
// The kernel has no message boundaries (S10): a `core.recv` returns whatever
// bytes are available, up to the destination capacity. These helpers recover
// typed messages on top of that byte FIFO, using only `core.send`,
// `core.recv`, `core.substr_into`, `core.len` and indexing.
//
// Every endpoint parameter is declared with capacity 1 on purpose: a `const`
// endpoint parameter accepts any capacity of its direction (the declared
// capacity is a lower bound that the compiler does not check for streams;
// spec 0003, "Streams"). Writing the real capacity here would suggest a
// guarantee the language does not make, and one helper would need a copy per
// capacity. The capacity that matters is checked where it can be: the size of
// the local work buffer, `CapacityExceeded` on an oversized payload, and the
// endpoint's own capacity in `core.send`.
//
// Each helper is atomic at its boundary. A destination is written only by the
// final copy that publishes a whole value, so a failure never exposes a
// half-filled buffer: the caller either gets `Ok` with a complete value or a
// status with the destination untouched. Blocking inside a helper is safe:
// the machine rewinds to the blocking `core.send`/`core.recv` instruction and
// retries it, so the callee's frame, locals included, is preserved.
//
// Work buffers are locals with concrete capacities, because the language has
// no generics. That is the whole cost of the layer: the frame of each helper
// below.

// Reads exactly `n` bytes from `ep` into `dst[0..n]` and sets the
// destination's runtime length to `n`. `n < 0` returns Invalid and `n > 256`
// returns CapacityExceeded without reading anything. A close before `n` bytes
// returns Closed with `dst` untouched (the length it had on entry), so the
// caller never sees a partial value. The bytes are staged in a local buffer
// and published with one copy at the end, which is what makes that promise
// hold even though the receive loop blocks.
//
// Every receive asks for exactly one byte, on purpose. A receive returns
// *up to* the destination capacity, not exactly the bytes the caller wants, so
// a 256-byte chunk would take bytes that belong to the peer's next message and
// silently drop them: the stream has no pushback, and the language has no
// buffer parameter that carries its capacity at run time (Q46), so
// `remaining` cannot become the receive bound. One byte per receive is exact
// by construction; the price is one stream operation per byte, which is the
// right trade for a reference implementation and is invisible at the sizes
// this layer frames.
function recv_exact(ep: const in_stream[1]; dst: var bytes[256]; n: int): Status;
var
  one: bytes[1];
  staged: bytes[256];
  got: int;
begin
  if n < 0 then
    return Invalid;
  if n > FrameCapacity then
    return CapacityExceeded;
  got := 0;
  while got < n do
  begin
    if core.recv(ep, one) <= 0 then
      return Closed;
    staged[got] := one[0];
    got := got + 1;
  end;
  dst := staged;
  return Ok;
end;

// Sends `[u32 little-endian length][payload]` as one atomic `core.send`, so a
// peer never observes a torn frame. The payload must fit the local frame
// buffer: 256 - 4 header bytes = 252 bytes. A longer payload returns
// CapacityExceeded and sends nothing. The peer's own capacity is the other
// bound: a frame larger than it is refused by `core.send` with the same
// status, again without sending anything.
function send_frame(ep: const out_stream[1]; text: const string[0]): Status;
var
  frame: bytes[256];
  length: int;
  i: int;
begin
  length := int(core.len(text));
  if length > FramePayloadCapacity then
    return CapacityExceeded;
  frame[0] := byte(length mod 256);
  frame[1] := byte((length div 256) mod 256);
  frame[2] := byte((length div 65536) mod 256);
  frame[3] := byte((length div 16777216) mod 256);
  i := 0;
  while i < length do
  begin
    frame[FrameHeader + i] := text[i];
    i := i + 1;
  end;
  return core.send(ep, frame);
end;

// Reads one length-prefixed frame into `dst`. The length is a little-endian
// `u32`; a payload was limited to 252 bytes by `send_frame`, so a length above
// 256 cannot be a frame this library produced (`recv_exact` would also refuse
// it). Such a length returns CapacityExceeded and leaves the payload bytes in
// the stream: the stream is then desynchronised, because the four length bytes
// are gone and the peer's message boundaries are lost. The caller must stop
// using that stream (spec 0006: a length that cannot be satisfied is an error,
// never a partial read). Closed and a short read propagate unchanged, with
// `dst` untouched.
function recv_frame_into(ep: const in_stream[1]; dst: var bytes[256]): Status;
var
  header: bytes[256];
  length: int;
  st: Status;
begin
  // `recv_exact` takes a `bytes[256]` destination (a `var` buffer parameter
  // must be at least that large), so the 4 header bytes get their own staging
  // buffer. Only `header[0..3]` is read below.
  st := recv_exact(ep, header, FrameHeader);
  if st <> Ok then
    return st;
  length := int(header[0])
    + int(header[1]) * 256
    + int(header[2]) * 65536
    + int(header[3]) * 16777216;
  if length > FrameCapacity then
    return CapacityExceeded;
  return recv_exact(ep, dst, length);
end;

// Sends `text` and one trailing newline as one atomic `core.send`, so a peer
// receives whole lines or nothing. Text longer than 255 bytes returns
// CapacityExceeded and sends nothing (the local line buffer is 256 bytes).
function send_line(ep: const out_stream[1]; text: const string[0]): Status;
var
  line: bytes[256];
  length: int;
  i: int;
begin
  length := int(core.len(text));
  if length > LineCapacity then
    return CapacityExceeded;
  i := 0;
  while i < length do
  begin
    line[i] := text[i];
    i := i + 1;
  end;
  line[length] := byte(10);
  return core.send(ep, line);
end;

// Reads one `\n`-terminated line into `dst`, without the terminator; a CR
// immediately before the LF is stripped too, so CRLF peers work. The line is
// accumulated in a local buffer and published with one copy, so a failure
// leaves `dst` untouched. A line longer than 256 bytes keeps consuming bytes
// up to its LF (discarding the excess, so the stream stays synchronised) and
// then returns CapacityExceeded. A close before the LF returns Closed.
//
// One byte per receive, for the same reason as `recv_exact`: a wider receive
// could swallow the first bytes of the next line and the stream cannot give
// them back.
function recv_line_into(ep: const in_stream[1]; dst: var bytes[256]): Status;
var
  one: bytes[1];
  staged: bytes[256];
  got: int;
  over: bool;
  done: bool;
  st: Status;
  b: byte;
begin
  got := 0;
  over := false;
  done := false;
  st := Ok;
  while not done do
  begin
    if core.recv(ep, one) <= 0 then
    begin
      st := Closed;
      done := true;
    end
    else
    begin
      b := one[0];
      if b = byte(10) then
      begin
        done := true;
        if over then
          st := CapacityExceeded
        else
        begin
          if got > 0 and staged[got - 1] = byte(13) then
            got := got - 1;
          // `staged` holds at least `got` bytes, so the explicit length
          // makes the copy exact even after a stripped CR. It cannot fail
          // (`got` is at most the destination capacity); the branch keeps
          // the result total instead of asserting.
          if core.substr_into(dst, staged, 0, got) < 0 then
            st := CapacityExceeded;
        end;
      end
      else if got < FrameCapacity then
      begin
        staged[got] := b;
        got := got + 1;
      end
      else
        over := true;
    end;
  end;
  return st;
end;

// Sends one `int` as exactly 8 little-endian bytes (two's complement, so a
// negative value round-trips) in one atomic `core.send`.
function send_u64(ep: const out_stream[1]; value: int): Status;
var
  raw: bytes[8];
  magnitude: int;
  top: int;
begin
  if value < 0 then
  begin
    // The 64-bit pattern is `value + 2^63` with the top bit set. `2^63` does
    // not fit an `int`, so it is built from two in-range addends; for
    // `MinInt` the sum is `0`.
    magnitude := value + MaxInt + 1;
    top := 128;
  end
  else
  begin
    magnitude := value;
    top := 0;
  end;
  raw[0] := byte(magnitude mod 256);
  raw[1] := byte((magnitude div 256) mod 256);
  raw[2] := byte((magnitude div 65536) mod 256);
  raw[3] := byte((magnitude div 16777216) mod 256);
  raw[4] := byte((magnitude div 4294967296) mod 256);
  raw[5] := byte((magnitude div 1099511627776) mod 256);
  raw[6] := byte((magnitude div 281474976710656) mod 256);
  raw[7] := byte((magnitude div 72057594037927936) mod 256 + top);
  return core.send(ep, raw);
end;

// Reads exactly 8 little-endian bytes and stores the signed value. Closed and
// CapacityExceeded propagate from `recv_exact` and leave `value` untouched.
function recv_u64(ep: const in_stream[1]; value: var int): Status;
var
  raw: bytes[256];
  st: Status;
  number: int;
  low: int;
  top: int;
begin
  st := recv_exact(ep, raw, 8);
  if st <> Ok then
    return st;
  low := int(raw[0])
    + int(raw[1]) * 256
    + int(raw[2]) * 65536
    + int(raw[3]) * 16777216
    + int(raw[4]) * 4294967296
    + int(raw[5]) * 1099511627776
    + int(raw[6]) * 281474976710656;
  top := int(raw[7]);
  if top < 128 then
    number := low + top * 72057594037927936
  else
    // The top bit means the pattern is negative: subtract 2^63, spelled as
    // `MinInt`, after adding the remaining 7 bits of the top byte.
    number := low + (top - 128) * 72057594037927936 + MinInt;
  value := number;
  return Ok;
end;

// Sends any `bytes` buffer as one atomic `core.send`. The payload capacity is
// 0, so a caller may pass a buffer of any capacity (the at-least-N rule); the
// endpoint's capacity still bounds the send. This is the raw counterpart of
// `send_frame`/`send_line` for a payload the application built itself.
function send_bytes(ep: const out_stream[1]; data: const bytes[0]): Status;
begin
  return core.send(ep, data);
end;

// ---- UI layout and chrome (spec 0003, "UI layout and chrome"; ADR 0026) --
//
// Client-side chrome for dialogs, panels and controls inside an application's
// own content. The window service still draws the real window frames (spec
// 0011, ADR 0024); these helpers never call it. Everything here is ordinary
// language code over the nine `syslib` drawing primitives, with scalar and
// buffer arguments only. The palette and the metrics are functions because a
// `stdlib` constant is not visible across the unit boundary.
//
// The font is 8x8 and `syslib.draw_text` advances one cell per byte, drawing a
// blank cell for a byte above 0x7F. `ui_text_width` counts UTF-8 code points,
// which agree with the cell count for the ASCII the font covers.

// One 8-pixel cell per code point in `text`.
function ui_text_width(text: const string[0]): int;
begin
  return utf8_len(text) * UiGlyphWidth;
end;

// Height of one glyph row.
function ui_text_height(): int;
begin
  return UiGlyphHeight;
end;

// Standard text line advance: one row plus leading, equal to the title bar.
function ui_line_height(): int;
begin
  return UiLineHeight;
end;

// Standard content/control margin from a frame, title bar or neighbour.
function ui_content_inset(): int;
begin
  return UiContentInset;
end;

// Height of the title bar drawn by `ui_title_bar`.
function ui_title_bar_height(): int;
begin
  return UiTitleBarHeight;
end;

// Side of the close box drawn by `ui_close_box`.
function ui_close_box_size(): int;
begin
  return UiCloseBoxSize;
end;

// System 7.5 palette. Text, borders, chrome ink and the desktop dither.
function ui_black(): uint;
begin
  return uint(UiBlack);
end;

// Content and desktop base, and the focused title background.
function ui_white(): uint;
begin
  return uint(UiWhite);
end;

// Unfocused title background and disabled control face.
function ui_gray(): uint;
begin
  return uint(UiGray);
end;

// The named focused title background (equal to white).
function ui_focus(): uint;
begin
  return uint(UiWhite);
end;

// Selection/highlight accent.
function ui_accent(): uint;
begin
  return uint(UiAccent);
end;

// `x + (w - text width) / 2`, or `x` when the text is at least as wide as the
// box, so the result is never left of `x`.
function ui_text_center_x(x: int; w: int; text: const string[0]): int;
var
  width: int;
begin
  width := ui_text_width(text);
  if width >= w then
    return x;
  return x + (w - width) div 2;
end;

// `y + (h - 8) / 2`, or `y` when the row is at most one glyph high.
function ui_text_center_y(y: int; h: int): int;
begin
  if UiGlyphHeight >= h then
    return y;
  return y + (h - UiGlyphHeight) div 2;
end;

// A one-pixel black outline inside `(x, y, w, h)`; the interior is untouched.
// `w <= 0`, `h <= 0` or disabled graphics return `Status.Invalid` and draw
// nothing (the `syslib.draw_rect` rule).
function ui_frame(x: int; y: int; w: int; h: int): Status;
begin
  return syslib.draw_rect(x, y, w, h, uint(UiBlack));
end;

// An 11x11 black outline at `(x, y)`. The background is not filled, so the
// caller picks it; `ui_title_bar` fills the box first with the bar colour.
function ui_close_box(x: int; y: int): Status;
begin
  return syslib.draw_rect(x, y, UiCloseBoxSize, UiCloseBoxSize, uint(UiBlack));
end;

// One black row `(x, y, w, 1)`.
function ui_separator(x: int; y: int; w: int): Status;
begin
  return syslib.fill_rect(x, y, w, 1, uint(UiBlack));
end;

// Draws `text` centred in `(x, y, w, h)`, left/top aligned when it does not
// fit, through `syslib.draw_text` (transparent background).
function ui_center_text(x: int; y: int; w: int; h: int; text: const string[0]; color: uint): Status;
begin
  return syslib.draw_text(
    ui_text_center_x(x, w, text),
    ui_text_center_y(y, h),
    text,
    color);
end;

// Paints a title bar inside a frame whose origin is `(x, y)` and whose width is
// `w`. The caller has drawn the one-pixel frame with `ui_frame`; this helper
// never paints the outer border. The interior is white with black stripes when
// focused, plain gray when not; the close box sits at `(x + 4, y + 3)` and the
// title is centred in the band right of it, truncated to at most 64 bytes at a
// code-point boundary. A separator at `y + 15` closes the bar.
//
// `w < 3` returns `Invalid` and draws nothing. A narrow bar (interior under 14
// pixels) omits the close box; a band under one glyph omits the title. The
// result is the first non-`Ok` status from a `syslib` call, so disabled
// graphics propagate as `Invalid` after the first paint.
function ui_title_bar(x: int; y: int; w: int; title: const string[0]; focused: bool): Status;
var
  st: Status;
  inner_x: int;
  inner_w: int;
  title_h: int;
  bg: uint;
  row: int;
  box_x: int;
  box_y: int;
  avail: int;
  fits: int;
  at: int;
  next: int;
  codepoint: int;
  shown_chars: int;
  shown_bytes: int;
  length: int;
  text_w: int;
  band_x: int;
  left: int;
  right: int;
  copied: int;
  staged: string[64];
  done: bool;
begin
  if w < 3 then
    return Invalid;
  inner_x := x + 1;
  inner_w := w - 2;
  title_h := UiTitleBarHeight - 1;
  if focused then
    bg := uint(UiWhite)
  else
    bg := uint(UiGray);

  st := syslib.fill_rect(inner_x, y + 1, inner_w, title_h, bg);
  if st <> Ok then
    return st;

  if focused then
  begin
    row := y + 2;
    while row < y + title_h do
    begin
      st := syslib.fill_rect(inner_x, row, inner_w, 1, uint(UiBlack));
      if st <> Ok then
        return st;
      row := row + 2;
    end;
  end;

  // The box ends at x + 15 (exclusive); the interior ends at x + w - 1.
  if inner_w >= 14 then
  begin
    box_x := x + 4;
    box_y := y + 3;
    st := syslib.fill_rect(box_x, box_y, UiCloseBoxSize, UiCloseBoxSize, bg);
    if st <> Ok then
      return st;
    st := ui_close_box(box_x, box_y);
    if st <> Ok then
      return st;
  end;

  st := ui_separator(inner_x, y + UiTitleBarHeight - 1, inner_w);
  if st <> Ok then
    return st;

  // Title text: the band right of the close box, at most 64 bytes and never
  // splitting a code point.
  avail := inner_w - 20;
  if avail >= UiGlyphWidth then
  begin
    fits := avail div UiGlyphWidth;
    length := int(core.len(title));
    at := 0;
    next := 0;
    shown_chars := 0;
    shown_bytes := 0;
    done := false;
    while not done and shown_chars < fits and at < length do
    begin
      st := utf8_decode(title, at, codepoint, next);
      if st <> Ok then
        done := true
      else
      begin
        at := next;
        if at <= UiTitleCapacity then
        begin
          shown_chars := shown_chars + 1;
          shown_bytes := at;
        end
        else
          done := true;
      end;
    end;
    if shown_bytes > 0 then
    begin
      copied := core.substr_into(staged, title, 0, shown_bytes);
      if copied < 0 then
        return Invalid;
      text_w := shown_chars * UiGlyphWidth;
      band_x := inner_x + 20 + (avail - text_w) div 2;
      // Repaint the band behind the text so stripes never cross a glyph, but
      // clamp it to the interior: a narrow bar must not touch the border.
      left := band_x - 2;
      if left < inner_x then
        left := inner_x;
      right := band_x + text_w + 2;
      if right > inner_x + inner_w then
        right := inner_x + inner_w;
      if right > left then
      begin
        st := syslib.fill_rect(left, y + 3, right - left, 10, bg);
        if st <> Ok then
          return st;
      end;
      st := syslib.draw_text(band_x, y + 4, staged, uint(UiBlack));
      if st <> Ok then
        return st;
    end;
  end;

  return Ok;
end;

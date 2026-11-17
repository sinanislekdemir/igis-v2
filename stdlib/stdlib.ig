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
  UiCheckBoxSize = 11;
  UiFieldCapacity = 64;
  UiMenuBarHeight = 22;
  UiMenuBarInset = 8;
  UiMenuBarPad = 4;
  UiMenuBarGap = 16;
  UiMenuRowHeight = 16;
  UiMenuCheckX = 8;
  UiMenuTextX = 22;
  UiMenuEdge = 8;
  UiMenuBarCount = 8;
  UiMenuItemCount = 16;
  UiMenuActionNone = 0;
  UiMenuActionUp = 1;
  UiMenuActionDown = 2;
  UiMenuActionLeft = 3;
  UiMenuActionRight = 4;
  UiMenuActionActivate = 5;
  UiMenuActionClose = 6;
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

// ---- UI controls (spec 0003, "UI controls") ----------------------------
//
// Drawing, hit-testing and small state-transition helpers for the controls an
// application composes inside its own window content (M2.4.2). The application
// owns the event loop and every state scalar; the library has no widget object,
// callback, internal event pump or `state`. Drawing uses the M2.4.0 metrics and
// palette (a `stdlib` constant is not exported, so the values are passed to
// `syslib` through the private constants below).
//
// Every drawing helper returns the first `syslib` status it receives: `Ok`, or
// `Invalid` with nothing drawn when graphics are disabled or the geometry is
// unusable. Hit tests and state helpers are pure and keep working with a
// disabled screen. Geometry follows the screen-relative `syslib` rule (drawing
// is clipped, never faulted); a control whose own extent is too small for its
// drawing returns `Invalid` rather than painting outside itself.
//
// Control conventions:
//   - The outer `(x, y, w, h)` is the control's full extent. The 1-px border is
//     inside it, so `ui_rect_hit` is inclusive of `(x, y)` and exclusive of
//     `(x + w, y + h)`.
//   - The focused state is a dotted 1-px ring inset inside the control's own
//     extent (`ui_focus_ring`), distinct from the solid border and never drawn
//     outside the control (there is no per-task clip in M2).
//   - `pressed` and `checked` are application state; the helpers draw it and
//     return a transition, they never store it.

// The shared hit-test rule: `(px, py)` is inside `(x, y, w, h)` when the
// extent is positive and the point is at or past the origin and strictly
// before the far edge. Off-screen and clipped controls use the same rule.
function ui_rect_hit(x: int; y: int; w: int; h: int; px: int; py: int): bool;
begin
  if w <= 0 or h <= 0 then
    return false;
  if px < x or px >= x + w then
    return false;
  if py < y or py >= y + h then
    return false;
  return true;
end;

// A dotted 1-px black ring along the perimeter of `(x, y, w, h)`: one pixel
// every two positions on each edge. It is the focused-state cue for controls;
// callers inset it inside the control so it never draws outside the control's
// extent. `w < 1` or `h < 1` returns `Invalid` and draws nothing.
function ui_focus_ring(x: int; y: int; w: int; h: int): Status;
var
  st: Status;
  i: int;
  black: uint;
begin
  if w < 1 or h < 1 then
    return Invalid;
  black := uint(UiBlack);
  i := 0;
  while i < w do
  begin
    st := syslib.fill_rect(x + i, y, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + i, y + h - 1, 1, 1, black);
    if st <> Ok then
      return st;
    i := i + 2;
  end;
  i := 0;
  while i < h do
  begin
    st := syslib.fill_rect(x, y + i, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + w - 1, y + i, 1, 1, black);
    if st <> Ok then
      return st;
    i := i + 2;
  end;
  return Ok;
end;

// The next focus index for `count` controls, wrapping in both directions.
// `count <= 0` returns 0. A `current` outside `0..count-1` is first wrapped, so
// a stale index cannot skip the wrap.
function ui_focus_next(current: int; count: int; delta: int): int;
var
  at: int;
  next: int;
begin
  if count <= 0 then
    return 0;
  at := current mod count;
  if at < 0 then
    at := at + count;
  next := (at + delta) mod count;
  if next < 0 then
    next := next + count;
  return next;
end;

// ---- key decoding (spec 0010, "Key events") ----------------------------
//
// `Event.code` packs `text | modifiers << 8 | key code << 16`; the language has
// no bitwise operators, so these helpers use `div`/`mod` over `uint`. They are
// pure and work with a disabled screen. The application maps keys to controls:
// Tab/Shift+Tab call `ui_focus_next`, arrows move the selected control's value,
// Backspace/Delete/Home/End drive a text field, and Enter activates a button.

// bits 0..=7: the text byte (0 for a non-text key or a release).
function ui_key_text(code: uint): byte;
begin
  return byte(code mod uint(256));
end;

// bits 8..=11: Shift 1, Ctrl 2, Alt 4, Caps 8.
function ui_key_modifiers(code: uint): int;
begin
  return int((code div uint(256)) mod uint(16));
end;

// bits 16..=31: the stable key code (unshifted ASCII or a 0x100-range code).
function ui_key_code(code: uint): int;
begin
  return int(code div uint(65536));
end;

function ui_key_is_backspace(code: uint): bool;
begin
  return ui_key_code(code) = 0x101;
end;

function ui_key_is_tab(code: uint): bool;
begin
  return ui_key_code(code) = 0x102;
end;

function ui_key_is_enter(code: uint): bool;
begin
  return ui_key_code(code) = 0x103;
end;

function ui_key_is_left(code: uint): bool;
begin
  return ui_key_code(code) = 0x110;
end;

function ui_key_is_right(code: uint): bool;
begin
  return ui_key_code(code) = 0x111;
end;

function ui_key_is_up(code: uint): bool;
begin
  return ui_key_code(code) = 0x112;
end;

function ui_key_is_down(code: uint): bool;
begin
  return ui_key_code(code) = 0x113;
end;

function ui_key_is_home(code: uint): bool;
begin
  return ui_key_code(code) = 0x114;
end;

function ui_key_is_end(code: uint): bool;
begin
  return ui_key_code(code) = 0x115;
end;

function ui_key_is_delete(code: uint): bool;
begin
  return ui_key_code(code) = 0x119;
end;

// ---- button ------------------------------------------------------------

// A rectangular button inside `(x, y, w, h)`: a white face (gray while pressed,
// so the press is visible), a 1-px black border, a centred label truncated to
// the interior width and, while focused, a dotted inset ring. A pressed button
// insets its label by one pixel when the face is tall enough. Labels are cut at
// a code-point boundary and at most 64 bytes.
//
// `w < 9` or `h < 10` returns `Invalid` and draws nothing: the face would not
// hold the border, padding and one glyph row.
function ui_button(x: int; y: int; w: int; h: int; label: const string[0]; pressed: bool; focused: bool): Status;
var
  st: Status;
  face: uint;
  avail: int;
  fits: int;
  at: int;
  next: int;
  codepoint: int;
  length: int;
  shown_chars: int;
  shown_bytes: int;
  staged: string[64];
  copied: int;
  label_x: int;
  label_y: int;
  shift: int;
  done: bool;
begin
  if w < 9 or h < 10 then
    return Invalid;
  if pressed then
    face := uint(UiGray)
  else
    face := uint(UiWhite);
  st := syslib.fill_rect(x + 1, y + 1, w - 2, h - 2, face);
  if st <> Ok then
    return st;
  st := ui_frame(x, y, w, h);
  if st <> Ok then
    return st;

  avail := w - 8;
  fits := avail div UiGlyphWidth;
  length := int(core.len(label));
  at := 0;
  shown_chars := 0;
  shown_bytes := 0;
  done := false;
  while not done and at < length and shown_chars < fits do
  begin
    st := utf8_decode(label, at, codepoint, next);
    if st <> Ok then
      done := true
    else if next > UiTitleCapacity then
      done := true
    else
    begin
      at := next;
      shown_chars := shown_chars + 1;
      shown_bytes := at;
    end;
  end;
  if shown_bytes > 0 then
  begin
    copied := core.substr_into(staged, label, 0, shown_bytes);
    if copied <> shown_bytes then
      return Invalid;
    shift := 0;
    if pressed and w > 12 and h > 12 then
      shift := 1;
    label_x := ui_text_center_x(x + 1, w - 2, staged) + shift;
    label_y := ui_text_center_y(y, h) + shift;
    st := syslib.draw_text(label_x, label_y, staged, uint(UiBlack));
    if st <> Ok then
      return st;
  end;

  if focused then
  begin
    st := ui_focus_ring(x + 3, y + 3, w - 6, h - 6);
    if st <> Ok then
      return st;
  end;
  return Ok;
end;

// The button's hit test; the same inclusive/exclusive rule as `ui_rect_hit`.
function ui_button_hit(x: int; y: int; w: int; h: int; px: int; py: int): bool;
begin
  return ui_rect_hit(x, y, w, h, px, py);
end;

// The press/release transition: true when a pointer release lands inside a
// button that was held down, i.e. the click activates it. The caller keeps
// `pressed` (set on a pointer-down inside the button) and clears it after the
// release. A release outside the button returns false.
function ui_button_pressed(pressed: bool; inside: bool): bool;
begin
  return pressed and inside;
end;

// ---- checkbox ----------------------------------------------------------

// A checkbox: an 11x11 white box with a black outline, a seven-pixel check
// mark when `checked`, and the label six pixels to its right, vertically
// centred on the box. While focused, a dotted ring surrounds the control's
// full extent (`11 + 6 + label width` wide), drawn before the box so the box
// border stays solid.
function ui_checkbox(x: int; y: int; label: const string[0]; checked: bool; focused: bool): Status;
var
  st: Status;
  white: uint;
  black: uint;
  label_x: int;
  label_y: int;
  width: int;
begin
  if focused then
  begin
    width := UiCheckBoxSize + 6 + ui_text_width(label);
    st := ui_focus_ring(x, y, width, UiCheckBoxSize);
    if st <> Ok then
      return st;
  end;
  white := uint(UiWhite);
  black := uint(UiBlack);
  st := syslib.fill_rect(x, y, UiCheckBoxSize, UiCheckBoxSize, white);
  if st <> Ok then
    return st;
  st := syslib.draw_rect(x, y, UiCheckBoxSize, UiCheckBoxSize, black);
  if st <> Ok then
    return st;
  if checked then
  begin
    // A check mark from (x+2, y+5) down to (x+4, y+7) and up to (x+8, y+3).
    st := syslib.fill_rect(x + 2, y + 5, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + 3, y + 6, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + 4, y + 7, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + 5, y + 6, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + 6, y + 5, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + 7, y + 4, 1, 1, black);
    if st <> Ok then
      return st;
    st := syslib.fill_rect(x + 8, y + 3, 1, 1, black);
    if st <> Ok then
      return st;
  end;
  label_x := x + UiCheckBoxSize + 6;
  label_y := ui_text_center_y(y, UiCheckBoxSize);
  st := syslib.draw_text(label_x, label_y, label, black);
  if st <> Ok then
    return st;
  return Ok;
end;

// The checkbox's hit test: the 11x11 box only. The label is not clickable
// because the signature carries no width for it.
function ui_checkbox_hit(x: int; y: int; px: int; py: int): bool;
begin
  return ui_rect_hit(x, y, UiCheckBoxSize, UiCheckBoxSize, px, py);
end;

// The toggle transition: the new `checked` value after a click.
function ui_checkbox_toggle(checked: bool): bool;
begin
  return not checked;
end;

// ---- text field --------------------------------------------------------

// Returns the scroll offset that keeps the clamped `cursor` visible in a field
// whose text area is `width_px` pixels wide (one 8-pixel cell per code point).
// It is the horizontal analogue of `ui_list_ensure_visible`: a caret left of
// `scroll` scrolls back to the caret; a caret past the right edge scrolls
// forward so it sits at the right edge. A width below one glyph is treated as
// one glyph. Pure, so a caller can run it with a disabled screen.
function ui_text_field_reveal(text: const string[0]; cursor: int; scroll: int; width_px: int): int;
var
  base: int;
  caret: int;
  at: int;
  cells: int;
  limit: int;
  i: int;
begin
  base := str_cursor_clamp(text, scroll);
  caret := str_cursor_clamp(text, cursor);
  if width_px < UiGlyphWidth then
    width_px := UiGlyphWidth;
  limit := width_px div UiGlyphWidth;
  if caret <= base then
    return caret;
  at := base;
  cells := 0;
  while at < caret and cells < limit do
  begin
    at := str_cursor_right(text, at);
    cells := cells + 1;
  end;
  if at >= caret then
    return base;
  at := caret;
  i := 0;
  while i < limit do
  begin
    at := str_cursor_left(text, at);
    i := i + 1;
  end;
  return at;
end;

// A text field inside `(x, y, w, h)`: a white interior, a 1-px black border,
// the visible slice of `text` from the `scroll` byte offset, an accent band
// with white text for the `sel_start..sel_stop` selection, a one-pixel caret
// when focused and the cursor is visible, and a dotted inset focus ring.
//
// `scroll`, `cursor` and both selection ends are clamped to code-point
// boundaries; the helper never mutates the text (a `string` is read-only by
// index anyway). At most 64 bytes and `(w - 6) / 8` code points are shown.
// `w < 9` or `h < 10` returns `Invalid` and draws nothing.
function ui_text_field(x: int; y: int; w: int; h: int; text: const string[0]; cursor: int; scroll: int; sel_start: int; sel_stop: int; focused: bool): Status;
var
  st: Status;
  inner_x: int;
  avail: int;
  fits: int;
  length: int;
  scroll_at: int;
  shown_end: int;
  at: int;
  next: int;
  codepoint: int;
  shown_chars: int;
  shown_bytes: int;
  staged: string[64];
  selected: string[64];
  copied: int;
  ty: int;
  first: int;
  last: int;
  sel_from: int;
  sel_to: int;
  sel_x: int;
  sel_end: int;
  cells: int;
  caret: int;
  caret_x: int;
  done: bool;
begin
  if w < 9 or h < 10 then
    return Invalid;
  inner_x := x + 3;
  avail := w - 6;
  fits := avail div UiGlyphWidth;
  ty := ui_text_center_y(y + 1, h - 2);

  st := syslib.fill_rect(x + 1, y + 1, w - 2, h - 2, uint(UiWhite));
  if st <> Ok then
    return st;
  st := ui_frame(x, y, w, h);
  if st <> Ok then
    return st;

  length := int(core.len(text));
  scroll_at := str_cursor_clamp(text, scroll);
  at := scroll_at;
  shown_chars := 0;
  shown_bytes := 0;
  done := false;
  while not done and at < length and shown_chars < fits do
  begin
    st := utf8_decode(text, at, codepoint, next);
    if st <> Ok then
      done := true
    else if next - scroll_at > UiFieldCapacity then
      done := true
    else
    begin
      at := next;
      shown_chars := shown_chars + 1;
      shown_bytes := at - scroll_at;
    end;
  end;
  shown_end := scroll_at + shown_bytes;

  if shown_bytes > 0 then
  begin
    copied := core.substr_into(staged, text, scroll_at, shown_bytes);
    if copied <> shown_bytes then
      return Invalid;
    st := syslib.draw_text(inner_x, ty, staged, uint(UiBlack));
    if st <> Ok then
      return st;
  end;

  // Selection: normalize the pair, intersect with the visible slice, convert
  // the byte offsets to cell offsets, then paint the accent band and redraw
  // the selected text white on top.
  first := sel_start;
  last := sel_stop;
  copied := str_selection_normalize(text, first, last);
  sel_from := first;
  if sel_from < scroll_at then
    sel_from := scroll_at;
  sel_to := last;
  if sel_to > shown_end then
    sel_to := shown_end;
  if sel_from < sel_to then
  begin
    at := scroll_at;
    cells := 0;
    while at < sel_from do
    begin
      at := str_cursor_right(text, at);
      cells := cells + 1;
    end;
    sel_x := inner_x + cells * UiGlyphWidth;
    at := scroll_at;
    cells := 0;
    while at < sel_to do
    begin
      at := str_cursor_right(text, at);
      cells := cells + 1;
    end;
    sel_end := inner_x + cells * UiGlyphWidth;
    st := syslib.fill_rect(sel_x, ty, sel_end - sel_x, UiGlyphHeight, uint(UiAccent));
    if st <> Ok then
      return st;
    copied := core.substr_into(selected, text, sel_from, sel_to - sel_from);
    if copied <> sel_to - sel_from then
      return Invalid;
    st := syslib.draw_text(sel_x, ty, selected, uint(UiWhite));
    if st <> Ok then
      return st;
  end;

  if focused then
  begin
    caret := str_cursor_clamp(text, cursor);
    if caret >= scroll_at and caret <= shown_end then
    begin
      at := scroll_at;
      cells := 0;
      while at < caret do
      begin
        at := str_cursor_right(text, at);
        cells := cells + 1;
      end;
      caret_x := inner_x + cells * UiGlyphWidth;
      st := syslib.fill_rect(caret_x, ty, 1, UiGlyphHeight, uint(UiBlack));
      if st <> Ok then
        return st;
    end;
    st := ui_focus_ring(x + 2, y + 2, w - 4, h - 4);
    if st <> Ok then
      return st;
  end;
  return Ok;
end;

// ---- scrollbar ---------------------------------------------------------

// Clamps `position` into `0..max`, where `max = total - visible` (at least 0).
// Negative `total` or `visible` are treated as 0. Pure.
function ui_scrollbar_clamp(position: int; visible: int; total: int): int;
var
  max: int;
begin
  if total < 0 then
    total := 0;
  if visible < 0 then
    visible := 0;
  max := total - visible;
  if max < 0 then
    max := 0;
  if position < 0 then
    return 0;
  if position > max then
    return max;
  return position;
end;

// A vertical scrollbar inside `(x, y, w, h)`: a white track with a black
// border and a gray thumb with a black outline. `visible` is the number of
// items on screen and `total` the whole count, so `position` runs `0..max`.
// The thumb is at least 8 pixels tall (clamped to the track) and moves over the
// travel `track - thumb`. Focused adds a dotted inset ring.
//
// `w < 5` or `h < 5` returns `Invalid` and draws nothing.
function ui_scrollbar(x: int; y: int; w: int; h: int; position: int; visible: int; total: int; focused: bool): Status;
var
  st: Status;
  track_y: int;
  track_h: int;
  max: int;
  pos: int;
  thumb_h: int;
  thumb_y: int;
  travel: int;
begin
  if w < 5 or h < 5 then
    return Invalid;
  st := syslib.fill_rect(x + 1, y + 1, w - 2, h - 2, uint(UiWhite));
  if st <> Ok then
    return st;
  st := ui_frame(x, y, w, h);
  if st <> Ok then
    return st;

  track_y := y + 2;
  track_h := h - 4;
  pos := ui_scrollbar_clamp(position, visible, total);
  if total <= 0 then
    thumb_h := track_h
  else
  begin
    if visible <= 0 then
      thumb_h := 8
    else
      thumb_h := (track_h * visible) div total;
    if thumb_h < 8 then
      thumb_h := 8;
    if thumb_h > track_h then
      thumb_h := track_h;
  end;
  max := total - visible;
  if max < 0 then
    max := 0;
  thumb_y := track_y;
  travel := track_h - thumb_h;
  if max > 0 and travel > 0 then
    thumb_y := track_y + (travel * pos) div max;
  st := syslib.fill_rect(x + 2, thumb_y, w - 4, thumb_h, uint(UiGray));
  if st <> Ok then
    return st;
  st := syslib.draw_rect(x + 2, thumb_y, w - 4, thumb_h, uint(UiBlack));
  if st <> Ok then
    return st;

  if focused then
  begin
    st := ui_focus_ring(x + 1, y + 1, w - 2, h - 2);
    if st <> Ok then
      return st;
  end;
  return Ok;
end;

// The scrollbar hit test. Returns `0` outside, `1` above the thumb (page up),
// `2` below it (page down) or `3` on the thumb. Uses the same clamped geometry
// as `ui_scrollbar`. The caller applies the command, e.g.
// `position := ui_scrollbar_clamp(position - visible, visible, total)` for a
// page up; thumb dragging belongs to the M2.4 pointer-drag slice.
function ui_scrollbar_hit(x: int; y: int; w: int; h: int; px: int; py: int; position: int; visible: int; total: int): int;
var
  track_y: int;
  track_h: int;
  max: int;
  pos: int;
  thumb_h: int;
  thumb_y: int;
  travel: int;
begin
  if w < 5 or h < 5 then
    return 0;
  if not ui_rect_hit(x, y, w, h, px, py) then
    return 0;
  track_y := y + 2;
  track_h := h - 4;
  pos := ui_scrollbar_clamp(position, visible, total);
  if total <= 0 then
    thumb_h := track_h
  else
  begin
    if visible <= 0 then
      thumb_h := 8
    else
      thumb_h := (track_h * visible) div total;
    if thumb_h < 8 then
      thumb_h := 8;
    if thumb_h > track_h then
      thumb_h := track_h;
  end;
  max := total - visible;
  if max < 0 then
    max := 0;
  thumb_y := track_y;
  travel := track_h - thumb_h;
  if max > 0 and travel > 0 then
    thumb_y := track_y + (travel * pos) div max;
  if py >= thumb_y and py < thumb_y + thumb_h then
    return 3;
  if py < thumb_y then
    return 1;
  return 2;
end;

// ---- list --------------------------------------------------------------

// The number of whole item rows inside a list of height `h`: the interior
// `h - 2` divided by the 16-pixel line height, or 0 when there is no room.
// Pure, so an application can use it for `ui_list_ensure_visible`.
function ui_list_rows(h: int): int;
begin
  if h < 2 then
    return 0;
  return (h - 2) div UiLineHeight;
end;

// A list inside `(x, y, w, h)` over `items[0..count)`: a white interior, a
// 1-px black border, one 16-pixel row per item from `top`, the selected row
// filled with the accent (white text) and an unfocused black-on-white row
// otherwise. `count` is clamped to `0..8` and `top` to
// `0..max(0, count - rows)`. Labels are truncated to the interior width and
// 64 bytes. Focused adds a dotted inset ring. `w < 4` or `h < 4` returns
// `Invalid`; a list with no whole row still draws its frame.
function ui_list(x: int; y: int; w: int; h: int; items: const array[string[64], 8]; count: int; selected: int; top: int; focused: bool): Status;
var
  st: Status;
  rows: int;
  total: int;
  first: int;
  max_top: int;
  row: int;
  index: int;
  item_y: int;
  text_y: int;
  avail: int;
  fits: int;
  at: int;
  next: int;
  codepoint: int;
  shown_chars: int;
  shown_bytes: int;
  staged: string[64];
  copied: int;
  done: bool;
begin
  if w < 4 or h < 4 then
    return Invalid;
  st := syslib.fill_rect(x + 1, y + 1, w - 2, h - 2, uint(UiWhite));
  if st <> Ok then
    return st;
  st := ui_frame(x, y, w, h);
  if st <> Ok then
    return st;

  total := count;
  if total < 0 then
    total := 0;
  if total > 8 then
    total := 8;
  rows := ui_list_rows(h);
  first := top;
  if first < 0 then
    first := 0;
  max_top := total - rows;
  if max_top < 0 then
    max_top := 0;
  if first > max_top then
    first := max_top;
  avail := w - 8;
  if avail < 0 then
    avail := 0;
  fits := avail div UiGlyphWidth;

  row := 0;
  while row < rows and first + row < total do
  begin
    index := first + row;
    item_y := y + 1 + row * UiLineHeight;
    text_y := ui_text_center_y(item_y, UiLineHeight);
    if index = selected then
    begin
      st := syslib.fill_rect(x + 1, item_y, w - 2, UiLineHeight, uint(UiAccent));
      if st <> Ok then
        return st;
    end;

    at := 0;
    shown_chars := 0;
    shown_bytes := 0;
    done := false;
    while not done and at < int(core.len(items[index])) and shown_chars < fits do
    begin
      st := utf8_decode(items[index], at, codepoint, next);
      if st <> Ok then
        done := true
      else if next > UiTitleCapacity then
        done := true
      else
      begin
        at := next;
        shown_chars := shown_chars + 1;
        shown_bytes := at;
      end;
    end;
    if shown_bytes > 0 then
    begin
      copied := core.substr_into(staged, items[index], 0, shown_bytes);
      if copied <> shown_bytes then
        return Invalid;
      if index = selected then
        st := syslib.draw_text(x + 4, text_y, staged, uint(UiWhite))
      else
        st := syslib.draw_text(x + 4, text_y, staged, uint(UiBlack));
      if st <> Ok then
        return st;
    end;
    row := row + 1;
  end;

  if focused then
  begin
    st := ui_focus_ring(x + 2, y + 2, w - 4, h - 4);
    if st <> Ok then
      return st;
  end;
  return Ok;
end;

// The list row under `(px, py)`, or `-1` outside the list, in the border or
// past the last item. `top` is clamped exactly as `ui_list` clamps it.
function ui_list_hit(x: int; y: int; w: int; h: int; px: int; py: int; count: int; top: int): int;
var
  rows: int;
  total: int;
  first: int;
  max_top: int;
  row: int;
  index: int;
begin
  if w <= 0 or h <= 0 then
    return -1;
  if not ui_rect_hit(x, y, w, h, px, py) then
    return -1;
  if py < y + 1 then
    return -1;
  total := count;
  if total < 0 then
    total := 0;
  if total > 8 then
    total := 8;
  rows := ui_list_rows(h);
  row := (py - (y + 1)) div UiLineHeight;
  if row < 0 or row >= rows then
    return -1;
  first := top;
  if first < 0 then
    first := 0;
  max_top := total - rows;
  if max_top < 0 then
    max_top := 0;
  if first > max_top then
    first := max_top;
  index := first + row;
  if index < 0 or index >= total then
    return -1;
  return index;
end;

// The `top` that keeps row `selected` inside the `rows` visible rows: scrolls
// up to the selection when it is above, down so it becomes the last row when
// below, otherwise leaves `top` unchanged. `rows <= 0` or an empty selection
// returns a non-negative `top`; the list draw clamps it against `count`.
function ui_list_ensure_visible(selected: int; top: int; rows: int): int;
begin
  if top < 0 then
    top := 0;
  if rows <= 0 then
    return top;
  if selected < 0 then
    return top;
  if selected < top then
    return selected;
  if selected >= top + rows then
    return selected - rows + 1;
  return top;
end;

// The row selected by a move of `delta` rows (usually -1 or +1), clamped to
// `0..count-1` without wrapping. `count <= 0` returns 0. A `selected` outside
// the range is clamped first.
function ui_list_move(selected: int; count: int; delta: int): int;
var
  at: int;
begin
  if count <= 0 then
    return 0;
  at := selected;
  if at < 0 then
    at := 0;
  if at > count - 1 then
    at := count - 1;
  at := at + delta;
  if at < 0 then
    at := 0;
  if at > count - 1 then
    at := count - 1;
  return at;
end;

// ---- menus (spec 0003, "UI menus") -------------------------------------
//
// A menu-bar model, pull-down drawing, hit-testing and small state-transition
// helpers (M2.4.3). The application owns the event loop and every state scalar:
// the open menu (-1 closed), the highlighted item, and one `uint` mask per
// concern. The library stores nothing, takes no callback and runs no pump.
//
// The item masks are plain `uint` bit sets: bit `i` means item `i`, indices
// 0..=63. The language has no bitwise operators, so `ui_menu_bit`/`ui_menu_has`/
// `ui_menu_set` use `div`/`mod` over `uint` and powers of two. `checked` and
// `disabled` are independent masks, so the highlight never changes them.
//
// A separator is an item whose label is exactly `-`; it is never selectable and
// has no check mark, shortcut or highlight. A disabled item is drawn with gray
// text, is skipped by `ui_menu_move` and `ui_menu_first`, and is never
// activated. `ui_menu_hit` still returns a disabled row so the application can
// decide what to do; `ui_menu_selectable` tells it whether the row may act.

// Height of the menu bar drawn by `ui_menu_bar`, matching the desktop shell's
// System 7.5 menu bar.
function ui_menu_bar_height(): int;
begin
  return UiMenuBarHeight;
end;

// True when `label` is the separator marker, exactly `-`.
function ui_menu_is_separator(label: const string[0]): bool;
begin
  return label = "-";
end;

// The `uint` with only bit `index` set. `index` outside 0..=63 returns 0,
// which `ui_menu_has` and `ui_menu_set` treat as a no-op.
function ui_menu_bit(index: int): uint;
var
  bit: uint;
  i: int;
begin
  if index < 0 or index >= 64 then
    return uint(0);
  bit := uint(1);
  i := 0;
  while i < index do
  begin
    bit := bit * uint(2);
    i := i + 1;
  end;
  return bit;
end;

// True when `mask` has bit `index` set. An out-of-range index is false.
function ui_menu_has(mask: uint; index: int): bool;
var
  bit: uint;
begin
  bit := ui_menu_bit(index);
  if bit = uint(0) then
    return false;
  return (mask div bit) mod uint(2) = uint(1);
end;

// `mask` with bit `index` set to `value`, returned as a new value. Setting a
// set bit or clearing a clear bit leaves the mask unchanged; an out-of-range
// index returns `mask` untouched. No bitwise operators are involved.
function ui_menu_set(mask: uint; index: int; value: bool): uint;
var
  bit: uint;
  present: bool;
begin
  bit := ui_menu_bit(index);
  if bit = uint(0) then
    return mask;
  present := ui_menu_has(mask, index);
  if value then
  begin
    if present then
      return mask;
    return mask + bit;
  end;
  if not present then
    return mask;
  return mask - bit;
end;

// True when item `index` may be highlighted or activated: it is not a
// separator and its `disabled` bit is clear.
function ui_menu_selectable(label: const string[0]; disabled_mask: uint; index: int): bool;
begin
  if ui_menu_is_separator(label) then
    return false;
  return not ui_menu_has(disabled_mask, index);
end;

// The first selectable item in `items[0..count)`, or -1 when every item is a
// separator or disabled. `count` is clamped to `0..16`.
function ui_menu_first(count: int; items: const array[string[64], 16]; disabled_mask: uint): int;
var
  total: int;
  i: int;
begin
  total := count;
  if total < 0 then
    total := 0;
  if total > UiMenuItemCount then
    total := UiMenuItemCount;
  i := 0;
  while i < total do
  begin
    if ui_menu_selectable(items[i], disabled_mask, i) then
      return i;
    i := i + 1;
  end;
  return -1;
end;

// The open transition: returns `menu` (or -1 for a negative index) and sets
// `selected` to the menu's first selectable item, or -1 when it has none.
function ui_menu_open(menu: int; count: int; items: const array[string[64], 16]; disabled_mask: uint; selected: var int): int;
begin
  if menu < 0 then
  begin
    selected := -1;
    return -1;
  end;
  selected := ui_menu_first(count, items, disabled_mask);
  return menu;
end;

// The close transition: the closed menu index, -1.
function ui_menu_close(): int;
begin
  return -1;
end;

// The highlighted item after a `delta` move (usually -1 or +1) over
// `items[0..count)`, skipping separators and disabled items without wrapping:
// the highlight stops at the last selectable item in the direction of travel.
// A `selected` outside the menu is clamped first; an unselectable `selected`
// snaps to the nearest selectable item in the direction of travel (or the
// other way when there is none), and that snap is the whole move.
// Returns -1 when the menu has no selectable item.
function ui_menu_move(selected: int; count: int; delta: int; items: const array[string[64], 16]; disabled_mask: uint): int;
var
  total: int;
  at: int;
  probe: int;
  found: int;
  remaining: int;
  dir: int;
begin
  total := count;
  if total < 0 then
    total := 0;
  if total > UiMenuItemCount then
    total := UiMenuItemCount;
  if total <= 0 then
    return -1;
  at := selected;
  if at < 0 then
    at := 0;
  if at > total - 1 then
    at := total - 1;
  if delta < 0 then
    dir := -1
  else
    dir := 1;
  if not ui_menu_selectable(items[at], disabled_mask, at) then
  begin
    // A stale or unselectable starting item snaps to the nearest selectable
    // item in the direction of travel (then the other way); the snap is the
    // whole move, so the caller never lands on a separator or disabled row.
    found := -1;
    probe := at;
    while probe >= 0 and probe < total and found < 0 do
    begin
      if ui_menu_selectable(items[probe], disabled_mask, probe) then
        found := probe
      else
        probe := probe + dir;
    end;
    probe := at - dir;
    while probe >= 0 and probe < total and found < 0 do
    begin
      if ui_menu_selectable(items[probe], disabled_mask, probe) then
        found := probe
      else
        probe := probe - dir;
    end;
    return found;
  end;
  remaining := delta;
  if remaining < 0 then
    remaining := 0 - remaining;
  while remaining > 0 do
  begin
    found := -1;
    probe := at + dir;
    while probe >= 0 and probe < total and found < 0 do
    begin
      if ui_menu_selectable(items[probe], disabled_mask, probe) then
        found := probe
      else
        probe := probe + dir;
    end;
    if found < 0 then
      remaining := 0
    else
    begin
      at := found;
      remaining := remaining - 1;
    end;
  end;
  return at;
end;

// The keyboard action for a packed `Event.code` (spec 0010) given whether a
// menu is open, as one of:
//
//   0 none, 1 up, 2 down, 3 left, 4 right, 5 activate, 6 close.
//
// Escape (key code 0x100) is close whether a menu is open or not, so an
// application can apply it unconditionally. While a menu is open Enter (0x103)
// activates the highlight, Up/Down (0x112/0x113) move it and Left/Right
// (0x110/0x111) switch to the previous/next top-level menu (the application
// wraps with `ui_focus_next` and reopens). Every other key, and every key
// except Escape while no menu is open, is none. There is no command or callback
// model: the application applies the action to its own state.
function ui_menu_key(code: uint; open: bool): int;
var
  key: int;
begin
  key := ui_key_code(code);
  if key = 0x100 then
    return UiMenuActionClose;
  if not open then
    return UiMenuActionNone;
  if key = 0x103 then
    return UiMenuActionActivate;
  if key = 0x112 then
    return UiMenuActionUp;
  if key = 0x113 then
    return UiMenuActionDown;
  if key = 0x110 then
    return UiMenuActionLeft;
  if key = 0x111 then
    return UiMenuActionRight;
  return UiMenuActionNone;
end;

// A menu bar inside `(x, y, w, 22)`: a white bar, the bottom rule, and one
// left-to-right label per title from `x + 8`, each followed by a 16-pixel gap.
// The active menu's label cell is inverted black with white text; the others
// are black on white. A label is cut at a code-point boundary to the remaining
// bar width (at most 32 bytes). `count` is clamped to `0..8`; `active` outside
// the count draws no active cell. `w <= 0` returns `Invalid` and draws nothing;
// drawing is otherwise clipped by `syslib`, never faulted.
function ui_menu_bar(x: int; y: int; w: int; titles: const array[string[32], 8]; count: int; active: int): Status;
var
  st: Status;
  total: int;
  i: int;
  label_x: int;
  label_w: int;
  slot_x: int;
  avail: int;
  fits: int;
  at: int;
  next: int;
  codepoint: int;
  length: int;
  shown_chars: int;
  shown_bytes: int;
  staged: string[32];
  copied: int;
  text_y: int;
  ink: uint;
begin
  if w <= 0 then
    return Invalid;
  total := count;
  if total < 0 then
    total := 0;
  if total > UiMenuBarCount then
    total := UiMenuBarCount;
  st := syslib.fill_rect(x, y, w, UiMenuBarHeight, uint(UiWhite));
  if st <> Ok then
    return st;
  text_y := ui_text_center_y(y, UiMenuBarHeight);
  label_x := x + UiMenuBarInset;
  i := 0;
  while i < total do
  begin
    label_w := ui_text_width(titles[i]);
    ink := uint(UiBlack);
    if i = active then
    begin
      slot_x := label_x - UiMenuBarPad;
      if slot_x < x then
        slot_x := x;
      st := syslib.fill_rect(
        slot_x, y + 2, label_w + 2 * UiMenuBarPad, UiMenuBarHeight - 4,
        uint(UiBlack));
      if st <> Ok then
        return st;
      ink := uint(UiWhite);
    end;
    avail := x + w - UiMenuBarInset - label_x;
    if avail >= UiGlyphWidth then
    begin
      fits := avail div UiGlyphWidth;
      length := int(core.len(titles[i]));
      at := 0;
      shown_chars := 0;
      shown_bytes := 0;
      while at < length and shown_chars < fits do
      begin
        st := utf8_decode(titles[i], at, codepoint, next);
        if st <> Ok then
          at := length
        else if next > 32 then
          at := length
        else
        begin
          at := next;
          shown_chars := shown_chars + 1;
          shown_bytes := at;
        end;
      end;
      if shown_bytes > 0 then
      begin
        copied := core.substr_into(staged, titles[i], 0, shown_bytes);
        if copied <> shown_bytes then
          return Invalid;
        st := syslib.draw_text(label_x, text_y, staged, ink);
        if st <> Ok then
          return st;
      end;
    end;
    label_x := label_x + label_w + UiMenuBarGap;
    i := i + 1;
  end;
  return ui_separator(x, y + UiMenuBarHeight - 1, w);
end;

// The menu index under `(px, py)` in the bar drawn by `ui_menu_bar`, or -1
// outside the bar, in a gap or past the last title. The layout is the draw
// helper's: the first cell starts at `x + 8`, and each cell is the title's
// width plus `4` pixels of padding on both sides.
function ui_menu_bar_hit(x: int; y: int; w: int; titles: const array[string[32], 8]; count: int; px: int; py: int): int;
var
  total: int;
  i: int;
  label_x: int;
  label_w: int;
  slot_x: int;
  slot_end: int;
begin
  if w <= 0 then
    return -1;
  if px < x or px >= x + w then
    return -1;
  if py < y or py >= y + UiMenuBarHeight then
    return -1;
  total := count;
  if total < 0 then
    total := 0;
  if total > UiMenuBarCount then
    total := UiMenuBarCount;
  label_x := x + UiMenuBarInset;
  i := 0;
  while i < total do
  begin
    label_w := ui_text_width(titles[i]);
    slot_x := label_x - UiMenuBarPad;
    if slot_x < x then
      slot_x := x;
    slot_end := label_x + label_w + UiMenuBarPad;
    if slot_end > x + w then
      slot_end := x + w;
    if px >= slot_x and px < slot_end then
      return i;
    label_x := label_x + label_w + UiMenuBarGap;
    i := i + 1;
  end;
  return -1;
end;

// A pull-down for `items[0..count)` at `(x, y)`, width `w`: a white interior,
// a 1-px black border, and one 16-pixel row per item. A row is laid out as a
// check column at `x + 8` (a seven-pixel check mark when `checked_mask` has the
// item's bit), the label at `x + 22`, and the shortcut right-aligned at
// `x + w - 8`. The selected row is accent-filled with white text when `active`
// and the item is enabled; other enabled labels are black, disabled labels and
// their check marks are gray. A separator (label `-`) is a 1-px black line
// inset by 8 pixels and is never highlighted or checked.
//
// The height is `count * 16 + 2`. `count` is clamped to `0..16`; `w < 32` or an
// empty menu returns `Invalid` and draws nothing. Labels and shortcuts are cut
// at a code-point boundary to the space they can occupy (labels at most 64
// bytes, shortcuts at most 32); a shortcut that does not fit is omitted.
// Drawing is clipped by `syslib`, never faulted.
function ui_menu_dropdown(x: int; y: int; w: int; items: const array[string[64], 16]; shortcuts: const array[string[32], 16]; count: int; selected: int; checked_mask: uint; disabled_mask: uint; active: bool): Status;
var
  st: Status;
  total: int;
  height: int;
  i: int;
  row_y: int;
  text_y: int;
  enabled: bool;
  chosen: bool;
  ink: uint;
  mark_ink: uint;
  shortcut_w: int;
  shortcut_x: int;
  label_room: int;
  fits: int;
  at: int;
  next: int;
  codepoint: int;
  length: int;
  shown_chars: int;
  shown_bytes: int;
  staged: string[64];
  copied: int;
begin
  total := count;
  if total < 0 then
    total := 0;
  if total > UiMenuItemCount then
    total := UiMenuItemCount;
  if w < 32 or total <= 0 then
    return Invalid;
  height := total * UiMenuRowHeight + 2;
  st := syslib.fill_rect(x + 1, y + 1, w - 2, height - 2, uint(UiWhite));
  if st <> Ok then
    return st;
  st := ui_frame(x, y, w, height);
  if st <> Ok then
    return st;

  i := 0;
  while i < total do
  begin
    row_y := y + 1 + i * UiMenuRowHeight;
    text_y := ui_text_center_y(row_y, UiMenuRowHeight);
    if ui_menu_is_separator(items[i]) then
    begin
      if w > 2 * UiMenuEdge then
      begin
        st := syslib.fill_rect(
          x + UiMenuEdge, row_y + UiMenuRowHeight div 2, w - 2 * UiMenuEdge, 1,
          uint(UiBlack));
        if st <> Ok then
          return st;
      end;
    end
    else
    begin
      enabled := not ui_menu_has(disabled_mask, i);
      chosen := active and enabled and i = selected;
      if chosen then
      begin
        st := syslib.fill_rect(
          x + 1, row_y, w - 2, UiMenuRowHeight, uint(UiAccent));
        if st <> Ok then
          return st;
        ink := uint(UiWhite);
      end
      else if enabled then
        ink := uint(UiBlack)
      else
        ink := uint(UiGray);

      if ui_menu_has(checked_mask, i) then
      begin
        if chosen then
          mark_ink := uint(UiWhite)
        else if enabled then
          mark_ink := uint(UiBlack)
        else
          mark_ink := uint(UiGray);
        st := syslib.fill_rect(x + UiMenuCheckX + 2, row_y + 5, 1, 1, mark_ink);
        if st <> Ok then
          return st;
        st := syslib.fill_rect(x + UiMenuCheckX + 3, row_y + 6, 1, 1, mark_ink);
        if st <> Ok then
          return st;
        st := syslib.fill_rect(x + UiMenuCheckX + 4, row_y + 7, 1, 1, mark_ink);
        if st <> Ok then
          return st;
        st := syslib.fill_rect(x + UiMenuCheckX + 5, row_y + 6, 1, 1, mark_ink);
        if st <> Ok then
          return st;
        st := syslib.fill_rect(x + UiMenuCheckX + 6, row_y + 5, 1, 1, mark_ink);
        if st <> Ok then
          return st;
        st := syslib.fill_rect(x + UiMenuCheckX + 7, row_y + 4, 1, 1, mark_ink);
        if st <> Ok then
          return st;
        st := syslib.fill_rect(x + UiMenuCheckX + 8, row_y + 3, 1, 1, mark_ink);
        if st <> Ok then
          return st;
      end;

      label_room := w - UiMenuTextX - 2 * UiMenuEdge;
      shortcut_w := ui_text_width(shortcuts[i]);
      if shortcut_w > 0 and shortcut_w <= label_room - UiMenuEdge then
      begin
        label_room := label_room - shortcut_w - UiMenuEdge;
        shortcut_x := x + w - UiMenuEdge - shortcut_w;
        st := syslib.draw_text(shortcut_x, text_y, shortcuts[i], ink);
        if st <> Ok then
          return st;
      end;
      if label_room < UiGlyphWidth then
        label_room := UiGlyphWidth;
      fits := label_room div UiGlyphWidth;
      length := int(core.len(items[i]));
      at := 0;
      shown_chars := 0;
      shown_bytes := 0;
      while at < length and shown_chars < fits do
      begin
        st := utf8_decode(items[i], at, codepoint, next);
        if st <> Ok then
          at := length
        else if next > UiTitleCapacity then
          at := length
        else
        begin
          at := next;
          shown_chars := shown_chars + 1;
          shown_bytes := at;
        end;
      end;
      if shown_bytes > 0 then
      begin
        copied := core.substr_into(staged, items[i], 0, shown_bytes);
        if copied <> shown_bytes then
          return Invalid;
        st := syslib.draw_text(x + UiMenuTextX, text_y, staged, ink);
        if st <> Ok then
          return st;
      end;
    end;
    i := i + 1;
  end;
  return Ok;
end;

// The item row under `(px, py)` in a pull-down drawn by `ui_menu_dropdown`, or
// -1 outside the menu, in the border, past the last item or on a separator. A
// disabled row is returned; the caller checks `ui_menu_selectable` before it
// highlights or activates the row.
function ui_menu_hit(x: int; y: int; w: int; items: const array[string[64], 16]; count: int; px: int; py: int): int;
var
  total: int;
  row: int;
  height: int;
begin
  total := count;
  if total < 0 then
    total := 0;
  if total > UiMenuItemCount then
    total := UiMenuItemCount;
  if w < 32 or total <= 0 then
    return -1;
  height := total * UiMenuRowHeight + 2;
  if px < x or px >= x + w then
    return -1;
  if py < y + 1 or py >= y + height - 1 then
    return -1;
  row := (py - (y + 1)) div UiMenuRowHeight;
  if row < 0 or row >= total then
    return -1;
  if ui_menu_is_separator(items[row]) then
    return -1;
  return row;
end;

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
  FrameCapacity = 256;
  FrameHeader = 4;
  FramePayloadCapacity = 252;
  LineCapacity = 255;
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

unit bitio;

// Importable LSB-first bit reader (spec 0023, G1). It is ordinary `/lib`
// module source under the M5/M6 rules: no `main`, no top-level `var` state and
// no stream endpoint. Ordinary language code only: the bit shifts are the
// machine-level `core.shl`/`core.shr` builtins (there are no bitwise
// operators), and the byte copy is `core.copy_bytes`.
//
// The reader owns one 512-byte window, a cursor into it, and one 64-bit
// accumulator holding the next `nbits` bits in its LOW positions:
//
//   type Reader = record
//     buf: bytes[512];   // the current window
//     len: int;          // bytes valid in buf
//     pos: int;          // next unread byte
//     acc: uint;         // the next bits in the low positions
//     nbits: int;        // valid bits in acc (0..39; a whole byte is 8 bits)
//   end;
//
// Bit order is LSB-first: the first bit read is bit 0 of the next byte, and
// `read_bits(n)` returns those n bits with the first bit in bit 0. This is the
// DEFLATE order: a stored-block length or a header field is read directly, and
// a Huffman code is decoded from the bits in the order they arrive.
//
// The window is compacted by `fill`: unread bytes move to the front before new
// bytes are appended, so a partially consumed 512-byte window (not only an
// empty one) can accept input. That keeps a long stream flowing across window
// boundaries instead of blocking on a full window with `pos > 0`.
//
//   var r: bitio.Reader;
//   bitio.init(r);                       // zero the state
//   bitio.fill(r, chunk, offset, used);  // stage bytes; `used` were copied
//   bitio.read_bits(r, n, value);        // peek then consume
//   bitio.align(r);                      // skip to the next byte boundary
//   bitio.read_byte(r, value);           // one whole byte (stored blocks)
//
// Statuses:
//   * `WouldBlock` means the window has no more bytes; the caller refills with
//     `bitio.fill` and retries. No state changes on `WouldBlock`.
//   * `Invalid` means the request is not well formed: `ensure`/`peek`/
//     `read_bits` with `count > 32` (the accumulator is bounded) or `drop` with
//     more bits than are buffered.
//   * `Full` means `fill` had no free window space after compaction (512
//     unread bytes); `used` is 0.
// "All failures leave out-parameters untouched" holds except for `fill`'s
// `used` on `Full`, which is defined to be 0.
//
// State invariant: `pos <= len <= 512` and `0 <= nbits <= 39`. `available` is
// `nbits + 8 * (len - pos)`; the caller never needs to look inside.

uses core;

type Reader = record
  buf: bytes[512];
  len: int;
  pos: int;
  acc: uint;
  nbits: int;
end;

// Zeroes the reader so it can be filled. The window's runtime length is reset
// too, so a reused reader starts with no staged bytes.
function init(r: var Reader): Status;
begin
  r.buf := "";
  r.len := 0;
  r.pos := 0;
  r.acc := uint(0);
  r.nbits := 0;
  return Ok;
end;

// Stages bytes from `data[offset..]` into the window, up to the free space.
// Unread bytes are compacted to the front first (the copy is overlap-safe), so
// a full window with `pos > 0` still accepts input; a window whose bytes were
// all consumed is reset. `used` receives the number copied; a window that is
// still full after compaction pushes `Full` with `used = 0`. `data` is copied
// by its runtime length, so any `bytes` buffer of any capacity works and only
// the bytes that actually exist are read.
function fill(r: var Reader; data: const bytes[0]; offset: uint;
              used: var int): Status;
var
  free: uint;
  have: uint;
  count: uint;
  copied: int;
  st: Status;
begin
  if r.pos = r.len then
  begin
    r.pos := 0;
    r.len := 0;
  end
  else if r.pos > 0 then
  begin
    st := core.copy_bytes(r.buf, uint(0), r.buf, uint(r.pos),
                          uint(r.len - r.pos));
    if st <> Ok then
      return st;
    r.len := r.len - r.pos;
    r.pos := 0;
  end;
  free := uint(512 - r.len);
  if free = uint(0) then
  begin
    used := 0;
    return Full;
  end;
  have := core.len(data);
  count := uint(0);
  if offset < have then
  begin
    count := have - offset;
    if count > free then
      count := free;
  end;
  if count > uint(0) then
  begin
    copied := int(count);
    st := core.copy_bytes(r.buf, uint(r.len), data, offset, count);
    if st <> Ok then
      return st;
    r.len := r.len + copied;
  end;
  used := int(count);
  return Ok;
end;

// The number of bits the reader can produce without a refill: buffered bits
// plus eight per unread byte.
function available(r: const Reader): uint;
begin
  return uint(r.nbits) + uint(8 * (r.len - r.pos));
end;

// Makes sure at least `count` bits (at most 32) are buffered. Whole bytes are
// pulled from the window and shifted above the buffered bits, so the first bit
// of each byte stays the low one. Insufficient window bytes push `WouldBlock`
// and change nothing, which lets the caller refill and retry.
function ensure(r: var Reader; count: uint): Status;
var
  needed: uint;
  shift: uint;
begin
  if count > uint(32) then
    return Invalid;
  if uint(r.nbits) >= count then
    return Ok;
  needed := (count - uint(r.nbits) + uint(7)) div uint(8);
  if uint(r.len - r.pos) < needed then
    return WouldBlock;
  while uint(r.nbits) < count do
  begin
    shift := core.shl(uint(1), uint(r.nbits));
    r.acc := r.acc + uint(r.buf[r.pos]) * shift;
    r.nbits := r.nbits + 8;
    r.pos := r.pos + 1;
  end;
  return Ok;
end;

// Looks at the next `count` bits without consuming them. The value is the
// buffered accumulator masked to `count` bits, so the first bit is bit 0.
function peek(r: var Reader; count: uint; value: var uint): Status;
var
  st: Status;
  bits: uint;
begin
  st := ensure(r, count);
  if st <> Ok then
    return st;
  bits := r.acc mod core.shl(uint(1), count);
  value := bits;
  return Ok;
end;

// Discards the next `count` bits from the accumulator. Asking for more bits
// than are buffered is `Invalid` and consumes nothing.
function drop(r: var Reader; count: uint): Status;
begin
  if uint(r.nbits) < count then
    return Invalid;
  r.acc := core.shr(r.acc, count);
  r.nbits := r.nbits - int(count);
  return Ok;
end;

// `peek` then `drop`: the next `count` bits as a value, with the first bit in
// bit 0. A failure changes nothing and leaves `value` untouched.
function read_bits(r: var Reader; count: uint; value: var uint): Status;
var
  st: Status;
  bits: uint;
begin
  st := peek(r, count, bits);
  if st <> Ok then
    return st;
  st := drop(r, count);
  if st <> Ok then
    return st;
  value := bits;
  return Ok;
end;

// Discards the partial byte in the accumulator (`nbits mod 8` bits), which
// aligns the stream to the next byte boundary for a stored block. Whole bytes
// left in the accumulator stay there and `read_byte` hands them out.
procedure align(r: var Reader);
var
  partial: int;
begin
  partial := r.nbits mod 8;
  if partial > 0 then
  begin
    r.acc := core.shr(r.acc, uint(partial));
    r.nbits := r.nbits - partial;
  end;
end;

// Reads one whole byte after `align`. After a successful `align` the buffered
// bits are whole bytes or nothing, so a byte already in the accumulator is
// drained first (`value = acc mod 256`, `acc = shr(acc, 8)`); otherwise the next
// byte comes from the window. `WouldBlock` only when the accumulator holds no
// whole byte and the window is empty. `nbits` is never nonzero and below 8
// after `align`, but that case is `Invalid` rather than data loss. `value` is
// untouched on failure.
function read_byte(r: var Reader; value: var uint): Status;
begin
  align(r);
  if r.nbits >= 8 then
  begin
    value := r.acc mod uint(256);
    r.acc := core.shr(r.acc, uint(8));
    r.nbits := r.nbits - 8;
    return Ok;
  end;
  if r.nbits <> 0 then
    return Invalid;
  if r.pos < r.len then
  begin
    value := uint(r.buf[r.pos]);
    r.pos := r.pos + 1;
    return Ok;
  end;
  return WouldBlock;
end;

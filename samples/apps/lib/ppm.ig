unit ppm;

// Row-streaming reader for uncompressed binary PPM (P6) images (spec 0024;
// ADR 0058). Ordinary `/lib` module source under the M5/M6 rules: no `main`,
// no top-level `var` state and no stream endpoint. No new builtin or opcode.
//
// PPM P6 is the shipped corpus format: a Netpbm header followed by 8-bit RGB
// pixels in row-major order. There is no compression, no row filter and no
// palette, and the byte order already matches `syslib.blit_rgb`, so a row is a
// direct read rather than a decode. The header is ASCII tokens separated by
// whitespace, and `#` starts a comment through the next newline:
//
//   P6
//   8 8
//   255
//   <8*8*3 RGB bytes>
//
// The reader never allocates or returns a full-image buffer. `open` parses the
// header once and records the absolute offset of the first pixel; `read_row`
// seeks to `data_offset + y * stride` and copies one row of `width * 3` bytes
// in 256-byte `core.fs_read` chunks into the caller's 16,384-byte buffer. That
// also gives a thumbnail path for free: any row can be read by index, so a
// zoomed-out preview reads every Nth row instead of decoding everything.
//
//   var d: ppm.Decoder;
//   var row: bytes[16384];
//   var w, h, got_w: uint;
//   st := ppm.open(path, d);
//   while st = Ok do ...
//     st := ppm.read_row(d, y, row, got_w);
//
// Accepted input is P6 with maxval 255 only. P3 (ASCII), P5 (grey) and any
// other magic are `Invalid`, as is a maxval other than 255. A missing file is
// `NotFound`; a read that fails after a successful open is `IoError`; a header
// or raster that ends early is `Invalid`; a row wider than the caller's buffer
// is `CapacityExceeded`. Nothing faults.
//
// Failure rule: every status leaves its out-parameters untouched, except that
// `read_row` clears the caller's row before it starts and leaves it empty on a
// failed read (a failed row is never partially published); `got_w` stays
// untouched on failure.

uses core;

const
  ChunkSize = 256;   // one `core.fs_read` piece for the header and a row
  RowCap = 16384;    // the caller's RGB row (5461 pixels)

type Decoder = record
  path: string[256];
  width: uint;
  height: uint;
  stride: uint;       // width * 3
  data_offset: uint;  // absolute file offset of the first pixel
  opened: bool;
end;

// Header scanner over 256-byte refills. `base` is the absolute offset of
// `buf[0]`, `len` the valid byte count and `pos` the absolute offset of the
// next unread byte. Refills discard the previous chunk, so only the token
// being read lives in memory.
type Cursor = record
  buf: bytes[ChunkSize];
  base: uint;
  len: int;
  pos: uint;
end;

// Netpbm whitespace: space, tab, LF, VT, FF, CR. The raster separator after
// maxval is exactly one of these bytes.
function is_space(b: byte): bool;
begin
  return b = byte(32) or b = byte(9) or b = byte(10)
    or b = byte(11) or b = byte(12) or b = byte(13);
end;

procedure reset(c: var Cursor);
begin
  c.buf := "";
  c.base := uint(0);
  c.len := 0;
  c.pos := uint(0);
end;

procedure advance(c: var Cursor);
begin
  c.pos := c.pos + uint(1);
end;

// Returns the byte at the cursor and leaves the cursor in place. A chunk is
// loaded at `pos` when the current one is exhausted; `Invalid` means EOF
// before a byte (a short or truncated file), other failures are the file API's
// status. The first refill covers a plain header; a long comment crosses
// refills at the current absolute position and discards the old chunk.
function byte_at(path: const string[0]; c: var Cursor; value: var byte): Status;
var
  got: int;
  st: Status;
begin
  if c.pos < c.base or c.pos >= c.base + uint(c.len) then
  begin
    got := -1;
    st := core.fs_read(path, c.pos, c.buf, got);
    if st <> Ok then
      return st;
    if got <= 0 then
      return Invalid;
    c.base := c.pos;
    c.len := got;
  end;
  value := c.buf[c.pos - c.base];
  return Ok;
end;

// Skips whitespace and `#` comments. A comment runs through the next newline;
// EOF inside a comment is `Invalid` because the header cannot be complete.
function skip_blank(path: const string[0]; c: var Cursor): Status;
var
  b: byte;
  st: Status;
  at_end: bool;
begin
  at_end := false;
  while not at_end do
  begin
    st := byte_at(path, c, b);
    if st <> Ok then
      return st;
    if is_space(b) then
      advance(c)
    else if b = byte(35) then
    begin
      // Consume the '#' and the comment body through the newline.
      advance(c);
      st := byte_at(path, c, b);
      while st = Ok and b <> byte(10) do
      begin
        advance(c);
        st := byte_at(path, c, b);
      end;
      if st <> Ok then
        return Invalid;
      advance(c);
    end
    else
      at_end := true;
  end;
  return Ok;
end;

// Reads one header token (at most 15 bytes) into `tok`. Whitespace and
// comments separate tokens; a token may end at EOF (the next token then
// reports the truncation, and maxval still requires its separator). An empty
// token is `Invalid`.
function read_token(path: const string[0]; c: var Cursor;
                   tok: var bytes[16]): Status;
var
  b: byte;
  st: Status;
  done: bool;
begin
  tok := "";
  st := skip_blank(path, c);
  if st <> Ok then
    return st;
  done := false;
  while not done do
  begin
    st := byte_at(path, c, b);
    if st = Invalid then
      done := true
    else if st <> Ok then
      return st;
    else if is_space(b) or b = byte(35) then
      done := true
    else
    begin
      if core.len(tok) >= uint(15) then
        return Invalid;
      tok[core.len(tok)] := b;
      advance(c);
    end;
  end;
  if core.len(tok) = uint(0) then
    return Invalid;
  return Ok;
end;

// Parses a decimal token. Only digits are accepted (no sign, no `+`, no
// whitespace inside). Values above 4,294,967,295 are rejected, so dimensions
// fit a 32-bit image size and cannot overflow the 64-bit accumulator.
function parse_uint(tok: const bytes[16]; value: var uint): Status;
var
  i: int;
  v: uint;
  d: int;
begin
  if core.len(tok) = uint(0) then
    return Invalid;
  v := uint(0);
  i := 0;
  while i < int(core.len(tok)) do
  begin
    d := int(tok[uint(i)]);
    if d < 48 or d > 57 then
      return Invalid;
    if v > uint(4294967295) div uint(10) then
      return Invalid;
    v := v * uint(10);
    if v > uint(4294967295) - uint(d - 48) then
      return Invalid;
    v := v + uint(d - 48);
    i := i + 1;
  end;
  value := v;
  return Ok;
end;

// Parses the whole P6 header: magic, width, height, maxval, then exactly one
// whitespace byte and the pixel offset. Outputs are written only on `Ok`.
function parse_header(path: const string[0]; width: var uint; height: var uint;
                      data_offset: var uint; stride: var uint): Status;
var
  c: Cursor;
  tok: bytes[16];
  b: byte;
  w, h, maxval: uint;
  st: Status;
begin
  reset(c);
  st := read_token(path, c, tok);
  if st <> Ok then
    return st;
  if tok <> "P6" then
    return Invalid;
  st := read_token(path, c, tok);
  if st <> Ok then
    return st;
  st := parse_uint(tok, w);
  if st <> Ok then
    return st;
  st := read_token(path, c, tok);
  if st <> Ok then
    return st;
  st := parse_uint(tok, h);
  if st <> Ok then
    return st;
  st := read_token(path, c, tok);
  if st <> Ok then
    return st;
  st := parse_uint(tok, maxval);
  if st <> Ok then
    return st;
  if w = uint(0) or h = uint(0) then
    return Invalid;
  if maxval <> uint(255) then
    return Invalid;
  // The Netpbm separator: exactly one whitespace byte, then the raster.
  st := byte_at(path, c, b);
  if st <> Ok then
    return st;
  if not is_space(b) then
    return Invalid;
  advance(c);
  width := w;
  height := h;
  data_offset := c.pos;
  stride := w * uint(3);
  return Ok;
end;

// Parses only the header and reports the dimensions. The raster is not read.
function probe(path: const string[0]; width: var uint; height: var uint): Status;
var
  w, h, off, stride: uint;
  st: Status;
begin
  st := parse_header(path, w, h, off, stride);
  if st <> Ok then
    return st;
  width := w;
  height := h;
  return Ok;
end;

// Parses the header and fills a decoder. `path` is copied because `read_row`
// has no path argument. A path over the 256-byte capacity is
// `CapacityExceeded` and leaves the decoder untouched.
function open(path: const string[0]; d: var Decoder): Status;
var
  w, h, off, stride: uint;
  st: Status;
  copied: int;
begin
  st := parse_header(path, w, h, off, stride);
  if st <> Ok then
    return st;
  if core.len(path) > uint(256) then
    return CapacityExceeded;
  copied := core.substr_into(d.path, path, 0, int(core.len(path)));
  if copied <> int(core.len(path)) then
    return CapacityExceeded;
  d.width := w;
  d.height := h;
  d.stride := stride;
  d.data_offset := off;
  d.opened := true;
  return Ok;
end;

// Copies the dimensions of an opened decoder.
function info(d: const Decoder; width: var uint; height: var uint): Status;
begin
  if not d.opened then
    return Invalid;
  width := d.width;
  height := d.height;
  return Ok;
end;

// Reads row `y` top to bottom into the caller's row, appending
// `width * 3` bytes in 256-byte file reads. `got_w` receives the width on
// `Ok` and is untouched on failure; the row is cleared before the read and
// left empty on failure. A row over 16,384 bytes is `CapacityExceeded`; a
// short raster is `Invalid`; a failed file read is `IoError`.
function read_row(d: var Decoder; y: uint; row: var bytes[16384];
                  got_w: var uint): Status;
var
  st: Status;
  chunk: bytes[ChunkSize];
  base, done: uint;
  got, m: int;
begin
  if not d.opened then
    return Invalid;
  if y >= d.height then
    return Invalid;
  if d.width > uint(RowCap) div uint(3) then
    return CapacityExceeded;
  row := "";
  base := d.data_offset + y * d.stride;
  done := uint(0);
  while done < d.stride do
  begin
    got := -1;
    st := core.fs_read(d.path, base + done, chunk, got);
    if st <> Ok then
    begin
      row := "";
      return IoError;
    end;
    if got <= 0 then
    begin
      row := "";
      return Invalid;
    end;
    m := got;
    if uint(m) > d.stride - done then
      m := int(d.stride - done);
    st := core.copy_bytes(row, core.len(row), chunk, uint(0), uint(m));
    if st <> Ok then
    begin
      row := "";
      return st;
    end;
    done := done + uint(m);
  end;
  got_w := d.width;
  return Ok;
end;

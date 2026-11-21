unit inflate;

// Bounded zlib/DEFLATE inflater (spec 0023, G2). It is ordinary `/lib` module
// source under the M5/M6 rules: no `main`, no top-level `var` state and no
// stream endpoint. It decodes zlib (RFC 1950) and DEFLATE (RFC 1951) from a
// caller-supplied compressed prefix into bounded output chunks.
//
// WHY A BOUNDED PREFIX: the PNG layer stages up to 512 KiB of IDAT payload in
// its own state (the Adam7 pass-1 prefix; the measured corpus maximum is
// 368,317 bytes) and calls `read` repeatedly with that same buffer. The
// inflater never stages the whole file: it keeps a 32 KiB output window, the
// canonical Huffman tables and one 512-byte bit-reader window, so its state is
// 40,816 bytes (about 40 KiB) no matter how large the stream is. `read` is
// resumable at symbol and byte boundaries: `in_pos` and the bit reader persist,
// and a call that runs out of input returns `WouldBlock` (or `Ok` after
// producing output) with no lost or duplicated bytes. The zlib Adler-32 trailer
// is never read or verified: a caller that stops after a prefix (or after a
// preview) cannot see it, and structural bounds are what catch a corrupt
// stream.
//
//   var d: inflate.Inflater;
//   inflate.init(d);
//   ...
//   inflate.read(d, idat, idat_len, out, got);
//   if inflate.done(d) then ...
//
// API:
//   function init(d: var Inflater): Status;
//   function done(d: const Inflater): bool;
//   function read(d: var Inflater; input: const bytes[0]; input_len: uint;
//                 out: var bytes[4096]; got: var int): Status;
//
// Statuses:
//   * Ok: `got` bytes were appended to `out` (0..4096). A call that cannot
//     make progress reports `WouldBlock` instead, so `Ok` with `got = 0` means
//     the stream is done.
//   * WouldBlock: more compressed input is needed than `input_len` provides.
//     Nothing is lost; call again with a larger prefix.
//   * Invalid: a bad zlib header, a malformed block header or table, a bad
//     stored-block complement, an invalid symbol, length or distance, or a
//     distance past the produced output. No input can repair it.
//   * Full is internal to `bitio.fill`; `read` never returns it.
//
// `input_len` must be at most `core.len(input)`. Blocks: BTYPE 0 stored,
// BTYPE 1 fixed Huffman, BTYPE 2 dynamic Huffman; BTYPE 3 is Invalid. The
// output window is a ring: every produced byte is written there (wrapping) and
// to `out`; a match copies byte by byte from the ring, so overlapping matches
// (distance < length) work.

uses core, lib.bitio;

const
  WindowSize = 32768;
  OutSize = 4096;
  ReaderWindow = 512;   // `bitio.Reader`'s byte window
  SymbolTotal = 352;    // 288 literal/length + 32 distance + 19 code-length
  MaxLengths = 318;     // most HLIT + HDIST code lengths in one header
  TableSlots = 48;      // three canonical tables of 16 lengths
  TableSpan = 16;       // entries per canonical table
  LengthCodes = 288;    // fixed literal/length alphabet
  DistCodes = 32;       // distance alphabet
  ClBase = 320;         // code-length lengths in `lens`
  DistBase = 288;       // distance lengths in `lens`

type Inflater = record
  // Input cursor: `in_pos` counts bytes handed to the bit reader, not bytes
  // consumed, so it persists across calls and a larger prefix continues.
  reader: bitio.Reader;
  in_pos: uint;
  // Output ring and cursor. `total_out` counts every produced byte and
  // `wpos = total_out mod WindowSize`.
  window: bytes[WindowSize];
  wpos: int;
  total_out: uint;
  // Stream and block state.
  header_done: bool;
  done: bool;
  mode: int;            // 0 zlib header, 1 block header, 2 stored, 3 huffman, 4 match
  bfinal: bool;
  stored_left: int;
  match_len: int;       // a decoded length waiting for its distance (mode 4)
  // Checkpoint before a block header, so a WouldBlock in the (possibly long)
  // dynamic header restarts it without losing or duplicating output.
  save_reader: bitio.Reader;
  save_in_pos: uint;
  // Code lengths and the canonical decode tables. `sbase`/`cbase` select the
  // literal/length, distance and code-length tables.
  lens: array[int, SymbolTotal];
  minc: array[int, TableSlots];
  maxc: array[int, TableSlots];
  ptr: array[int, TableSlots];
  syms: array[int, SymbolTotal];
  sbase: array[int, 3];
  cbase: array[int, 3];
  maxbits: array[int, 3];
end;

// Zeroes the state. The same value can be reused for a new stream.
function init(d: var Inflater): Status;
var
  i: int;
begin
  bitio.init(d.reader);
  bitio.init(d.save_reader);
  d.in_pos := uint(0);
  d.save_in_pos := uint(0);
  d.window := "";
  d.wpos := 0;
  d.total_out := uint(0);
  d.header_done := false;
  d.done := false;
  d.mode := 0;
  d.bfinal := false;
  d.stored_left := 0;
  d.match_len := 0;
  i := 0;
  while i < SymbolTotal do
  begin
    d.lens[i] := 0;
    d.syms[i] := 0;
    i := i + 1;
  end;
  i := 0;
  while i < TableSlots do
  begin
    d.minc[i] := 0;
    d.maxc[i] := -1;
    d.ptr[i] := 0;
    i := i + 1;
  end;
  d.sbase[0] := 0;
  d.sbase[1] := LengthCodes;
  d.sbase[2] := ClBase;
  d.cbase[0] := 0;
  d.cbase[1] := TableSpan;
  d.cbase[2] := 2 * TableSpan;
  d.maxbits[0] := 0;
  d.maxbits[1] := 0;
  d.maxbits[2] := 0;
  return Ok;
end;

function done(d: const Inflater): bool;
begin
  return d.done;
end;

// Stages more caller bytes into the bit reader while fewer than 64 bits are
// buffered. `bitio.fill` copies by the source's runtime length, so the prefix
// bound `input_len` is applied here: at most the reader's free window space is
// copied into a small staging buffer first, and `fill` appends exactly that.
// The loop always terminates because it stages at least one byte or reports a
// full window (which already has plenty of bits).
function top_up(d: var Inflater; input: const bytes[0]; input_len: uint): Status;
var
  stage: bytes[ReaderWindow];
  free, room, count: int;
  used: int;
  st: Status;
begin
  while bitio.available(d.reader) < uint(64) and d.in_pos < input_len do
  begin
    // Mirrors `fill`'s rewind: an exhausted window is rewound before copying.
    if d.reader.pos = d.reader.len then
      free := ReaderWindow
    else
      free := ReaderWindow - d.reader.len;
    if free <= 0 then
      return Ok;
    room := int(input_len - d.in_pos);
    count := room;
    if count > free then
      count := free;
    if count <= 0 then
      return Ok;
    stage := "";
    st := core.copy_bytes(stage, uint(0), input, d.in_pos, uint(count));
    if st <> Ok then
      return st;
    used := 0;
    st := bitio.fill(d.reader, stage, uint(0), used);
    if st <> Ok then
      return st;
    if used <= 0 then
      return Ok;
    d.in_pos := d.in_pos + uint(used);
  end;
  return Ok;
end;

// Builds the canonical decode table `tbl` (0 literal/length, 1 distance,
// 2 code-length) from `n` code lengths starting at `lstart` in `lens`.
// `minc`/`maxc` hold the first and last canonical code of each length and
// `ptr` the first symbol index; `syms` holds symbols ordered by (length,
// symbol). An over-subscribed set is Invalid; an incomplete one is allowed
// (the code-length and distance alphabets may omit codes).
function build_table(d: var Inflater; tbl: int; lstart: int; n: int): Status;
var
  counts: array[int, 16];
  curs: array[int, 16];
  i, l, code, k, base, cb: int;
begin
  l := 0;
  while l < 16 do
  begin
    counts[l] := 0;
    curs[l] := 0;
    l := l + 1;
  end;
  i := 0;
  while i < n do
  begin
    l := d.lens[lstart + i];
    if l < 0 or l > 15 then
      return Invalid;
    counts[l] := counts[l] + 1;
    i := i + 1;
  end;
  cb := tbl * TableSpan;
  code := 0;
  k := 0;
  l := 1;
  while l <= 15 do
  begin
    d.minc[cb + l] := code;
    d.ptr[cb + l] := k;
    if counts[l] > 0 then
    begin
      if code + counts[l] > int(core.shl(uint(1), uint(l))) then
        return Invalid;
      d.maxc[cb + l] := code + counts[l] - 1;
    end
    else
      d.maxc[cb + l] := -1;
    code := (code + counts[l]) * 2;
    k := k + counts[l];
    l := l + 1;
  end;
  l := 15;
  while l > 0 and counts[l] = 0 do
    l := l - 1;
  d.maxbits[tbl] := l;
  l := 1;
  while l <= 15 do
  begin
    curs[l] := d.ptr[cb + l];
    l := l + 1;
  end;
  base := d.sbase[tbl];
  i := 0;
  while i < n do
  begin
    l := d.lens[lstart + i];
    if l > 0 then
    begin
      d.syms[base + curs[l]] := i;
      curs[l] := curs[l] + 1;
    end;
    i := i + 1;
  end;
  return Ok;
end;

// Decodes one Huffman symbol without consuming it, using the next
// min(max_bits, available) bits. The canonical code is accumulated MSB-first
// from the LSB-first stream, so a code may be shorter than the maximum and the
// final padding may leave fewer bits than the maximum. No code among those bits
// is Invalid when all max_bits are available and WouldBlock otherwise (more
// input could complete a longer code). `used` is the code length.
function hdecode(d: var Inflater; tbl: int; sym: var int; used: var int): Status;
var
  mb, avail, lmax, l, code, cb, bitv: int;
  shift: uint;
  v: uint;
  st: Status;
begin
  mb := d.maxbits[tbl];
  if mb <= 0 then
    return Invalid;
  avail := int(bitio.available(d.reader));
  if avail <= 0 then
    return WouldBlock;
  lmax := mb;
  if avail < lmax then
    lmax := avail;
  st := bitio.ensure(d.reader, uint(lmax));
  if st <> Ok then
    return st;
  st := bitio.peek(d.reader, uint(lmax), v);
  if st <> Ok then
    return st;
  code := 0;
  cb := tbl * TableSpan;
  l := 1;
  while l <= lmax do
  begin
    shift := uint(l - 1);
    bitv := int(core.bit_and(core.shr(v, shift), uint(1)));
    code := code * 2 + bitv;
    if d.maxc[cb + l] >= 0
      and code >= d.minc[cb + l] and code <= d.maxc[cb + l] then
    begin
      sym := d.syms[d.sbase[tbl] + d.ptr[cb + l] + (code - d.minc[cb + l])];
      used := l;
      return Ok;
    end;
    l := l + 1;
  end;
  if lmax < mb then
    return WouldBlock;
  return Invalid;
end;

// Consumes a symbol's `used` code bits and then `extra` extra bits as one
// step. The ensure happens before the drop, so a short input leaves the reader
// untouched and the caller can resume; a match symbol is never paused halfway.
function take_bits(d: var Inflater; used: int; extra: int;
                   value: var uint): Status;
var
  st: Status;
begin
  value := uint(0);
  st := bitio.ensure(d.reader, uint(used + extra));
  if st <> Ok then
    return st;
  st := bitio.drop(d.reader, uint(used));
  if st <> Ok then
    return st;
  if extra > 0 then
  begin
    st := bitio.read_bits(d.reader, uint(extra), value);
    if st <> Ok then
      return st;
  end;
  return Ok;
end;

// Installs the fixed Huffman code lengths of RFC 1951 3.2.6. Symbols 286/287
// and distance codes 30/31 decode but are rejected by `read`.
function parse_fixed(d: var Inflater): Status;
var
  i: int;
  st: Status;
begin
  i := 0;
  while i < 144 do
  begin
    d.lens[i] := 8;
    i := i + 1;
  end;
  while i < 256 do
  begin
    d.lens[i] := 9;
    i := i + 1;
  end;
  while i < 280 do
  begin
    d.lens[i] := 7;
    i := i + 1;
  end;
  while i < 288 do
  begin
    d.lens[i] := 8;
    i := i + 1;
  end;
  i := 0;
  while i < 32 do
  begin
    d.lens[DistBase + i] := 5;
    i := i + 1;
  end;
  st := build_table(d, 0, 0, LengthCodes);
  if st <> Ok then
    return st;
  return build_table(d, 1, DistBase, DistCodes);
end;

// Parses the dynamic Huffman header of RFC 1951 3.2.7: the code-length code,
// then HLIT + HDIST lengths with the repeat codes 16/17/18. Lengths are decoded
// contiguously at lens[0..hlit+hdist), then the distance half is moved above
// the literal half from the end (safe: hlit <= 286 < 288).
function parse_dynamic(d: var Inflater): Status;
var
  order: array[int, 19];
  hlit, hdist, hclen, total, i, j, sym, used, extra, rep, prev: int;
  v: uint;
  st: Status;
begin
  order[0] := 16;
  order[1] := 17;
  order[2] := 18;
  order[3] := 0;
  order[4] := 8;
  order[5] := 7;
  order[6] := 9;
  order[7] := 6;
  order[8] := 10;
  order[9] := 5;
  order[10] := 11;
  order[11] := 4;
  order[12] := 12;
  order[13] := 3;
  order[14] := 13;
  order[15] := 2;
  order[16] := 14;
  order[17] := 1;
  order[18] := 15;
  i := 0;
  while i < 19 do
  begin
    d.lens[ClBase + i] := 0;
    i := i + 1;
  end;
  st := bitio.read_bits(d.reader, uint(5), v);
  if st <> Ok then
    return st;
  hlit := int(v) + 257;
  st := bitio.read_bits(d.reader, uint(5), v);
  if st <> Ok then
    return st;
  hdist := int(v) + 1;
  st := bitio.read_bits(d.reader, uint(4), v);
  if st <> Ok then
    return st;
  hclen := int(v) + 4;
  if hlit > 286 or hlit + hdist > MaxLengths then
    return Invalid;
  i := 0;
  while i < hclen do
  begin
    st := bitio.read_bits(d.reader, uint(3), v);
    if st <> Ok then
      return st;
    d.lens[ClBase + order[i]] := int(v);
    i := i + 1;
  end;
  st := build_table(d, 2, ClBase, 19);
  if st <> Ok then
    return st;
  total := hlit + hdist;
  i := 0;
  while i < total do
  begin
    st := hdecode(d, 2, sym, used);
    if st <> Ok then
      return st;
    if sym < 16 then
    begin
      st := take_bits(d, used, 0, v);
      if st <> Ok then
        return st;
      d.lens[i] := sym;
      i := i + 1;
    end
    else if sym = 16 then
    begin
      if i = 0 then
        return Invalid;
      st := take_bits(d, used, 2, v);
      if st <> Ok then
        return st;
      rep := 3 + int(v);
      prev := d.lens[i - 1];
      if i + rep > total then
        return Invalid;
      j := 0;
      while j < rep do
      begin
        d.lens[i] := prev;
        i := i + 1;
        j := j + 1;
      end;
    end
    else if sym = 17 then
    begin
      st := take_bits(d, used, 3, v);
      if st <> Ok then
        return st;
      rep := 3 + int(v);
      if i + rep > total then
        return Invalid;
      j := 0;
      while j < rep do
      begin
        d.lens[i] := 0;
        i := i + 1;
        j := j + 1;
      end;
    end
    else if sym = 18 then
    begin
      st := take_bits(d, used, 7, v);
      if st <> Ok then
        return st;
      rep := 11 + int(v);
      if i + rep > total then
        return Invalid;
      j := 0;
      while j < rep do
      begin
        d.lens[i] := 0;
        i := i + 1;
        j := j + 1;
      end;
    end
    else
      return Invalid;
  end;
  i := hdist - 1;
  while i >= 0 do
  begin
    d.lens[DistBase + i] := d.lens[hlit + i];
    i := i - 1;
  end;
  if d.lens[256] = 0 then
    return Invalid;
  st := build_table(d, 0, 0, hlit);
  if st <> Ok then
    return st;
  return build_table(d, 1, DistBase, hdist);
end;

// The base length and extra-bit count for literal/length symbol 257..285
// (RFC 1951 3.2.5): 257..264 are 3..10 with no extra bits, 265..284 come in
// groups of four with 1..5 extra bits, and 285 is 258.
function length_base(code: int; extra: var int): int;
var
  e: int;
begin
  if code <= 264 then
  begin
    extra := 0;
    return code - 254;
  end;
  if code = 285 then
  begin
    extra := 0;
    return 258;
  end;
  e := (code - 265) div 4 + 1;
  extra := e;
  return int(core.shl(uint(1), uint(e + 2))) + 3
    + ((code - 265) mod 4) * int(core.shl(uint(1), uint(e)));
end;

// The base distance and extra-bit count for distance symbol 0..29. Codes 0..3
// are distances 1..4; 4..29 come in pairs with 1..13 extra bits.
function distance_base(code: int; extra: var int): int;
var
  e: int;
begin
  if code < 4 then
  begin
    extra := 0;
    return code + 1;
  end;
  e := (code - 2) div 2;
  extra := e;
  return int(core.shl(uint(1), uint(e + 1))) + 1
    + ((code - 2) mod 2) * int(core.shl(uint(1), uint(e)));
end;

// Appends one produced byte to the ring and to the caller's chunk. `got` is
// always below OutSize here: a literal adds one from at most 3838, and a match
// only starts with at least 258 bytes free.
procedure emit(d: var Inflater; b: byte; out: var bytes[OutSize]; got: var int);
begin
  d.window[d.wpos] := b;
  d.wpos := d.wpos + 1;
  if d.wpos >= WindowSize then
    d.wpos := 0;
  d.total_out := d.total_out + uint(1);
  out[got] := b;
  got := got + 1;
end;

// Decodes into `out` until fewer than 258 bytes remain (a DEFLATE match is at
// most 258), the stream is done, or the input prefix is exhausted. A block
// header is checkpointed so the dynamic header can restart after WouldBlock.
function read(d: var Inflater; input: const bytes[0]; input_len: uint;
              out: var bytes[OutSize]; got: var int): Status;
var
  st: Status;
  v: uint;
  cmf, flg, cm, cinfo, btype, sym, used, extra: int;
  len, nlen, length, distance, src, i: int;
begin
  got := 0;
  if input_len > core.len(input) then
    return Invalid;
  if d.done then
    return Ok;
  if not d.header_done then
  begin
    st := top_up(d, input, input_len);
    if st <> Ok then
      return st;
    if bitio.available(d.reader) < uint(16) then
      return WouldBlock;
    st := bitio.peek(d.reader, uint(16), v);
    if st <> Ok then
      return st;
    cmf := int(v mod uint(256));
    flg := int(v div uint(256));
    cm := cmf mod 16;
    cinfo := cmf div 16;
    if cm <> 8 or cinfo > 7
      or core.bit_and(uint(flg), uint(32)) <> uint(0) then
      return Invalid;
    st := bitio.drop(d.reader, uint(16));
    if st <> Ok then
      return st;
    d.header_done := true;
    d.mode := 1;
  end;
  while not d.done and got <= OutSize - 258 do
  begin
    if d.mode = 1 then
    begin
      st := top_up(d, input, input_len);
      if st <> Ok then
        return st;
      d.save_reader := d.reader;
      d.save_in_pos := d.in_pos;
      st := bitio.read_bits(d.reader, uint(3), v);
      if st <> Ok then
      begin
        d.reader := d.save_reader;
        d.in_pos := d.save_in_pos;
        if got = 0 then
          return WouldBlock;
        return Ok;
      end;
      d.bfinal := (v mod uint(2)) = uint(1);
      btype := int((v div uint(2)) mod uint(4));
      if btype = 3 then
        return Invalid;
      if btype = 0 then
      begin
        bitio.align(d.reader);
        st := bitio.read_bits(d.reader, uint(16), v);
        if st <> Ok then
        begin
          d.reader := d.save_reader;
          d.in_pos := d.save_in_pos;
          if got = 0 then
            return WouldBlock;
          return Ok;
        end;
        len := int(v);
        st := bitio.read_bits(d.reader, uint(16), v);
        if st <> Ok then
        begin
          d.reader := d.save_reader;
          d.in_pos := d.save_in_pos;
          if got = 0 then
            return WouldBlock;
          return Ok;
        end;
        nlen := int(v);
        if core.bit_xor(uint(len), uint(nlen)) <> uint(65535) then
          return Invalid;
        d.stored_left := len;
        d.mode := 2;
        if d.stored_left = 0 then
        begin
          if d.bfinal then
            d.done := true
          else
            d.mode := 1;
        end;
      end
      else if btype = 1 then
      begin
        st := parse_fixed(d);
        if st <> Ok then
          return st;
        d.mode := 3;
      end
      else
      begin
        st := parse_dynamic(d);
        if st = WouldBlock then
        begin
          d.reader := d.save_reader;
          d.in_pos := d.save_in_pos;
          if got = 0 then
            return WouldBlock;
          return Ok;
        end;
        if st <> Ok then
          return st;
        d.mode := 3;
      end;
    end
    else if d.mode = 2 then
    begin
      st := top_up(d, input, input_len);
      if st <> Ok then
        return st;
      st := bitio.read_byte(d.reader, v);
      if st = WouldBlock then
      begin
        if got = 0 then
          return WouldBlock;
        return Ok;
      end;
      if st <> Ok then
        return st;
      emit(d, byte(v), out, got);
      d.stored_left := d.stored_left - 1;
      if d.stored_left = 0 then
      begin
        if d.bfinal then
          d.done := true
        else
          d.mode := 1;
      end;
    end
    else if d.mode = 3 then
    begin
      st := top_up(d, input, input_len);
      if st <> Ok then
        return st;
      st := hdecode(d, 0, sym, used);
      if st = WouldBlock then
      begin
        if got = 0 then
          return WouldBlock;
        return Ok;
      end;
      if st <> Ok then
        return st;
      if sym < 256 then
      begin
        st := take_bits(d, used, 0, v);
        if st <> Ok then
          return st;
        emit(d, byte(sym), out, got);
      end
      else if sym = 256 then
      begin
        st := take_bits(d, used, 0, v);
        if st <> Ok then
          return st;
        if d.bfinal then
          d.done := true
        else
          d.mode := 1;
      end
      else if sym <= 285 then
      begin
        length := length_base(sym, extra);
        st := take_bits(d, used, extra, v);
        if st = WouldBlock then
        begin
          if got = 0 then
            return WouldBlock;
          return Ok;
        end;
        if st <> Ok then
          return st;
        length := length + int(v);
        if length < 3 or length > 258 then
          return Invalid;
        d.match_len := length;
        d.mode := 4;
      end
      else
        return Invalid;
    end
    else
    begin
      st := top_up(d, input, input_len);
      if st <> Ok then
        return st;
      st := hdecode(d, 1, sym, used);
      if st = WouldBlock then
      begin
        if got = 0 then
          return WouldBlock;
        return Ok;
      end;
      if st <> Ok then
        return st;
      if sym > 29 then
        return Invalid;
      distance := distance_base(sym, extra);
      st := take_bits(d, used, extra, v);
      if st = WouldBlock then
      begin
        if got = 0 then
          return WouldBlock;
        return Ok;
      end;
      if st <> Ok then
        return st;
      distance := distance + int(v);
      if distance < 1 or distance > WindowSize then
        return Invalid;
      if uint(distance) > d.total_out then
        return Invalid;
      i := 0;
      while i < d.match_len do
      begin
        src := d.wpos - distance;
        if src < 0 then
          src := src + WindowSize;
        emit(d, d.window[src], out, got);
        i := i + 1;
      end;
      d.mode := 3;
    end;
  end;
  return Ok;
end;

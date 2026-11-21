unit png;

// Row-streaming PNG decoder (spec 0023, P4-P13; ADR 0057). Ordinary `/lib`
// module source under the M5/M6 rules: no `main`, no top-level `var` state and
// no stream endpoint.
//
// The decoder never allocates or returns a full-image RGB buffer. `open`
// stages at most 512 KiB of concatenated IDAT payload and initialises a
// `lib.inflate` decoder over that prefix; `next_row` pulls one filtered source
// row (a filter byte plus `out_w * bpp` bytes), undoes filters 0-4 against the
// previous row, converts the row to opaque RGB, and returns it in the caller's
// 16,384-byte buffer.
//
//   var d: png.Decoder;
//   var row: bytes[16384];
//   var got_w, y: uint;
//   st := png.open(path, d);
//   while st = Ok do ...
//     st := png.next_row(d, row, got_w, y);
//   png.rewind(d);                       // repaint from the staged prefix
//
// Non-interlaced images yield every row. Interlaced images yield Adam7 pass 1
// only: a `ceil(width/8) x ceil(height/8)` rectangular sub-image, which is the
// fit-to-window preview. Later passes are not decoded.
//
// Accepted input is 8-bit colour type 0 (grey), 2 (RGB), 3 (palette), 4
// (grey+alpha) and 6 (RGBA). Other bit depths and colour types are `Invalid`.
// Palette entries are expanded to RGB; a palette `tRNS` alpha is supported.
// For colour types 0 and 2 a `tRNS` chunk is ignored (not composited), which
// keeps the row opaque; colour types 4 and 6 carry real alpha and are
// composited over white. Chunk CRCs and the zlib Adler-32 are not verified.
//
// `open` stages the IDAT payload prefix itself. If the inflater needs more
// input than the prefix and the prefix was filled to the 512 KiB limit, the
// row call returns `CapacityExceeded`; if the file's IDAT ended and the stream
// is truncated, it returns `Invalid`. Malformed structure is `Invalid`, a
// missing file is `NotFound`, and a file read failure is `IoError`. No corrupt
// file faults.

uses core, lib.inflate;

const
  IdatCap = 524288;    // staged compressed prefix (512 KiB)
  RowCap = 20480;      // one source row: filter byte + 5461 RGB pixels
  OutCap = 4096;       // one inflate output chunk
  PaletteCap = 768;    // 256 RGB entries
  TrnsCap = 256;       // one alpha per palette entry
  CallerRowCap = 16384; // caller's RGB row (5461 pixels)
  ChunkHeader = 8;     // length (4) + type (4)

type Decoder = record
  // Compressed side. `idat_full` records that the file had more IDAT payload
  // than the 512 KiB stage; it turns an inflater `WouldBlock` into
  // `CapacityExceeded` instead of `Invalid`.
  inf: inflate.Inflater;
  idat: bytes[IdatCap];
  idat_len: uint;
  idat_full: bool;
  // One staged inflater output chunk, shared across `next_row` calls so the
  // decoder resumes exactly where it stopped.
  out_buf: bytes[OutCap];
  out_len: int;
  out_pos: int;
  // Filter undo needs the previous unfiltered source row.
  prev: bytes[RowCap];
  cur: bytes[RowCap];
  // Palette and its optional alpha table.
  palette: bytes[PaletteCap];
  trns: bytes[TrnsCap];
  // Geometry and row cursor.
  width: uint;
  height: uint;
  out_w: uint;
  out_h: uint;
  color_type: uint;
  interlace: uint;
  bpp: uint;        // source bytes per pixel: 1, 3, 1, 2 or 4
  src_row: uint;    // 1 + out_w * bpp
  y: uint;          // next row index to produce
  have_prev: bool;
  started: bool;
end;

// Big-endian 32-bit field at `off`.
function be32(buf: const bytes[256]; off: int): uint;
begin
  return core.shl(uint(buf[off]), uint(24))
    + core.shl(uint(buf[off + 1]), uint(16))
    + core.shl(uint(buf[off + 2]), uint(8))
    + uint(buf[off + 3]);
end;

function is_ihdr(buf: const bytes[256]): bool;
begin
  return buf[4] = byte(73) and buf[5] = byte(72)
    and buf[6] = byte(68) and buf[7] = byte(82);
end;

function is_plte(buf: const bytes[256]): bool;
begin
  return buf[4] = byte(80) and buf[5] = byte(76)
    and buf[6] = byte(84) and buf[7] = byte(69);
end;

function is_trns(buf: const bytes[256]): bool;
begin
  return buf[4] = byte(116) and buf[5] = byte(82)
    and buf[6] = byte(78) and buf[7] = byte(83);
end;

function is_idat(buf: const bytes[256]): bool;
begin
  return buf[4] = byte(73) and buf[5] = byte(68)
    and buf[6] = byte(65) and buf[7] = byte(84);
end;

function is_iend(buf: const bytes[256]): bool;
begin
  return buf[4] = byte(73) and buf[5] = byte(69)
    and buf[6] = byte(78) and buf[7] = byte(68);
end;

// Reads and validates the signature and IHDR only. It rejects the unsupported
// subset here so `probe` never needs the IDAT: 8-bit depth, colour type 0/2/3/
// 4/6, compression 0, filter 0 and interlace 0/1.
function read_header(path: const string[0]; width: var uint; height: var uint;
                     depth: var uint; color: var uint;
                     interlace: var uint): Status;
var
  chunk: bytes[256];
  got: int;
  st: Status;
  length: uint;
begin
  st := core.fs_read(path, uint(0), chunk, got);
  if st <> Ok then
    return st;
  if got < 8 then
    return Invalid;
  if chunk[0] <> byte(137) or chunk[1] <> byte(80)
    or chunk[2] <> byte(78) or chunk[3] <> byte(71)
    or chunk[4] <> byte(13) or chunk[5] <> byte(10)
    or chunk[6] <> byte(26) or chunk[7] <> byte(10) then
    return Invalid;
  st := core.fs_read(path, uint(8), chunk, got);
  if st <> Ok then
    return st;
  if got < ChunkHeader then
    return Invalid;
  length := be32(chunk, 0);
  if length <> uint(13) or not is_ihdr(chunk) then
    return Invalid;
  st := core.fs_read(path, uint(16), chunk, got);
  if st <> Ok then
    return st;
  if got < 13 then
    return Invalid;
  width := be32(chunk, 0);
  height := be32(chunk, 4);
  depth := uint(chunk[8]);
  color := uint(chunk[9]);
  if uint(chunk[10]) <> uint(0) or uint(chunk[11]) <> uint(0) then
    return Invalid;
  interlace := uint(chunk[12]);
  if width = uint(0) or height = uint(0) then
    return Invalid;
  if depth <> uint(8) then
    return Invalid;
  if color <> uint(0) and color <> uint(2) and color <> uint(3)
    and color <> uint(4) and color <> uint(6) then
    return Invalid;
  if interlace > uint(1) then
    return Invalid;
  return Ok;
end;

// Own source bytes per pixel for an accepted 8-bit colour type.
function bytes_per_pixel(color: uint): uint;
begin
  if color = uint(0) then
    return uint(1);
  if color = uint(2) then
    return uint(3);
  if color = uint(3) then
    return uint(1);
  if color = uint(4) then
    return uint(2);
  return uint(4);
end;

// Computes the decoded geometry and checks both row bounds. Interlaced input
// is pass 1 only, so its output is the `ceil(w/8) x ceil(h/8)` sub-image; the
// source rows of that pass are that same width.
function geometry(width: uint; height: uint; color: uint; interlace: uint;
                  out_w: var uint; out_h: var uint; bpp: var uint): Status;
begin
  bpp := bytes_per_pixel(color);
  if interlace = uint(1) then
  begin
    out_w := (width + uint(7)) div uint(8);
    out_h := (height + uint(7)) div uint(8);
  end
  else
  begin
    out_w := width;
    out_h := height;
  end;
  if out_w = uint(0) or out_h = uint(0) then
    return Invalid;
  if out_w * uint(3) > uint(CallerRowCap) then
    return CapacityExceeded;
  if uint(1) + out_w * bpp > uint(RowCap) then
    return CapacityExceeded;
  return Ok;
end;

// Reads the signature and IHDR, validates the supported subset, and computes
// the output geometry without touching the IDAT.
function probe(path: const string[0]; width: var uint; height: var uint;
               out_w: var uint; out_h: var uint;
               color_type: var uint; interlace: var uint): Status;
var
  depth, color, il, ow, oh, bp: uint;
  st: Status;
begin
  st := read_header(path, width, height, depth, color, il);
  if st <> Ok then
    return st;
  st := geometry(width, height, color, il, ow, oh, bp);
  if st <> Ok then
    return st;
  out_w := ow;
  out_h := oh;
  color_type := color;
  interlace := il;
  return Ok;
end;

// Walks the chunks after the IHDR, collecting PLTE, tRNS and up to 512 KiB of
// concatenated IDAT payload. Chunk data is read in 256-byte pieces; only the
// IDAT prefix is retained, and only up to `idat_full`. Header/CRC bytes and
// unknown chunks are skipped.
function parse_chunks(path: const string[0]; total: uint; d: var Decoder): Status;
var
  chunk: bytes[256];
  got, m: int;
  st: Status;
  off, length, next, data_off, done, room, take: uint;
  seen_end: bool;
begin
  off := uint(8);
  seen_end := false;
  while not seen_end do
  begin
    if total < uint(ChunkHeader) or off > total - uint(ChunkHeader) then
      return Invalid;
    st := core.fs_read(path, off, chunk, got);
    if st <> Ok then
      return st;
    if got < ChunkHeader then
      return Invalid;
    length := be32(chunk, 0);
    data_off := off + uint(ChunkHeader);
    if total < data_off or length > total - data_off then
      return Invalid;
    next := data_off + length + uint(4);
    if next > total then
      return Invalid;

    if is_ihdr(chunk) then
    begin
      // Already parsed and validated by `read_header`.
    end
    else if is_plte(chunk) then
    begin
      if d.color_type = uint(3) then
      begin
        if length = uint(0) or length > uint(PaletteCap)
          or length mod uint(3) <> uint(0) then
          return Invalid;
        if core.len(d.palette) <> uint(0) then
          return Invalid;
        done := uint(0);
        while done < length do
        begin
          got := -1;
          st := core.fs_read(path, data_off + done, chunk, got);
          if st <> Ok then
            return st;
          if got <= 0 then
            return Invalid;
          take := uint(got);
          if take > length - done then
            take := length - done;
          st := core.copy_bytes(d.palette, done, chunk, uint(0), take);
          if st <> Ok then
            return st;
          done := done + take;
        end;
      end;
    end
    else if is_trns(chunk) then
    begin
      // Only a palette alpha table is used; 0/2 tRNS is ignored so the row
      // stays opaque, and 4/6 carry real alpha per pixel.
      if d.color_type = uint(3) then
      begin
        if length = uint(0) or length > uint(TrnsCap) then
          return Invalid;
        if core.len(d.trns) <> uint(0) then
          return Invalid;
        done := uint(0);
        while done < length do
        begin
          got := -1;
          st := core.fs_read(path, data_off + done, chunk, got);
          if st <> Ok then
            return st;
          if got <= 0 then
            return Invalid;
          take := uint(got);
          if take > length - done then
            take := length - done;
          st := core.copy_bytes(d.trns, done, chunk, uint(0), take);
          if st <> Ok then
            return st;
          done := done + take;
        end;
      end;
    end
    else if is_idat(chunk) then
    begin
      if not d.idat_full then
      begin
        room := uint(IdatCap) - d.idat_len;
        take := length;
        if take > room then
        begin
          take := room;
          d.idat_full := true;
        end;
        done := uint(0);
        while done < take do
        begin
          got := -1;
          st := core.fs_read(path, data_off + done, chunk, got);
          if st <> Ok then
            return st;
          if got <= 0 then
            return Invalid;
          m := got;
          if uint(m) > take - done then
            m := int(take - done);
          st := core.copy_bytes(d.idat, d.idat_len + done,
                                chunk, uint(0), uint(m));
          if st <> Ok then
            return st;
          done := done + uint(m);
        end;
        d.idat_len := d.idat_len + take;
      end;
    end
    else if is_iend(chunk) then
      seen_end := true;

    off := next;
  end;
  return Ok;
end;

// Re-reads the signature/IHDR, initialises the inflater, parses the remaining
// chunks, stages the IDAT prefix, and records the geometry.
function open(path: const string[0]; d: var Decoder): Status;
var
  width, height, depth, color, il, ow, oh, bp, total: uint;
  st: Status;
begin
  d.started := false;
  d.idat := "";
  d.idat_len := uint(0);
  d.idat_full := false;
  d.out_buf := "";
  d.out_len := 0;
  d.out_pos := 0;
  d.prev := "";
  d.cur := "";
  d.palette := "";
  d.trns := "";
  d.y := uint(0);
  d.have_prev := false;

  st := inflate.init(d.inf);
  if st <> Ok then
    return st;
  st := read_header(path, width, height, depth, color, il);
  if st <> Ok then
    return st;
  st := geometry(width, height, color, il, ow, oh, bp);
  if st <> Ok then
    return st;

  d.width := width;
  d.height := height;
  d.out_w := ow;
  d.out_h := oh;
  d.color_type := color;
  d.interlace := il;
  d.bpp := bp;
  d.src_row := uint(1) + ow * bp;

  st := core.fs_size(path, total);
  if st <> Ok then
    return st;
  st := parse_chunks(path, total, d);
  if st <> Ok then
    return st;
  if d.idat_len = uint(0) then
    return Invalid;
  if d.color_type = uint(3) then
  begin
    if core.len(d.palette) = uint(0) then
      return Invalid;
    if core.len(d.trns) > core.len(d.palette) div uint(3) then
      return Invalid;
  end;
  d.started := true;
  return Ok;
end;

function info(d: const Decoder; width: var uint; height: var uint;
              out_w: var uint; out_h: var uint;
              color_type: var uint; interlace: var uint): Status;
begin
  if not d.started then
    return Invalid;
  width := d.width;
  height := d.height;
  out_w := d.out_w;
  out_h := d.out_h;
  color_type := d.color_type;
  interlace := d.interlace;
  return Ok;
end;

// Restarts row decoding from the staged IDAT prefix without touching the
// filesystem. The compressed prefix, palette/tRNS and geometry stay put; only
// the inflater, the staged output chunk and the row cursor are reset, and the
// previous unfiltered row is cleared so the first row sees zeros. This is the
// repaint path for a window Expose: the viewer does not re-read 512 KiB.
function rewind(d: var Decoder): Status;
var
  st: Status;
begin
  if not d.started or d.idat_len = uint(0) then
    return Invalid;
  st := inflate.init(d.inf);
  if st <> Ok then
    return st;
  d.out_buf := "";
  d.out_len := 0;
  d.out_pos := 0;
  d.cur := "";
  d.prev := "";
  d.y := uint(0);
  d.have_prev := false;
  return Ok;
end;

// Paeth predictor (PNG filter 4), with left/up/upper-left bytes.
function paeth(a: int; b: int; c: int): int;
var
  p, pa, pb, pc: int;
begin
  p := a + b - c;
  pa := p - a;
  if pa < 0 then
    pa := -pa;
  pb := p - b;
  if pb < 0 then
    pb := -pb;
  pc := p - c;
  if pc < 0 then
    pc := -pc;
  if pa <= pb and pa <= pc then
    return a;
  if pb <= pc then
    return b;
  return c;
end;

// Pulls exactly one filtered source row (filter byte + out_w * bpp data bytes)
// into `d.cur`, draining any bytes left in the staged inflate chunk first.
function fill_row(d: var Decoder): Status;
var
  want, have, take: uint;
  got: int;
  st: Status;
begin
  d.cur := "";
  want := d.src_row;
  while core.len(d.cur) < want do
  begin
    if d.out_pos >= d.out_len then
    begin
      got := 0;
      st := inflate.read(d.inf, d.idat, d.idat_len, d.out_buf, got);
      if st = WouldBlock then
      begin
        if d.idat_full then
          return CapacityExceeded;
        return Invalid;
      end;
      if st <> Ok then
        return st;
      if got <= 0 then
        return Invalid;
      d.out_len := got;
      d.out_pos := 0;
    end;
    have := uint(d.out_len - d.out_pos);
    take := want - core.len(d.cur);
    if take > have then
      take := have;
    st := core.copy_bytes(d.cur, core.len(d.cur),
                          d.out_buf, uint(d.out_pos), take);
    if st <> Ok then
      return st;
    d.out_pos := d.out_pos + int(take);
  end;
  return Ok;
end;

// Undoes the row filter in place with the previous unfiltered row.
function unfilter(d: var Decoder): Status;
var
  i, bpp, filt, left, up, upper_left, pred: int;
begin
  filt := int(d.cur[0]);
  if filt < 0 or filt > 4 then
    return Invalid;
  bpp := int(d.bpp);
  i := 0;
  while i < int(d.out_w) * bpp do
  begin
    left := 0;
    if i >= bpp then
      left := int(d.cur[uint(1 + i - bpp)]);
    up := 0;
    if d.have_prev then
      up := int(d.prev[uint(1 + i)]);
    upper_left := 0;
    if i >= bpp and d.have_prev then
      upper_left := int(d.prev[uint(1 + i - bpp)]);
    if filt = 1 then
      pred := left
    else if filt = 2 then
      pred := up
    else if filt = 3 then
      pred := (left + up) div 2
    else if filt = 4 then
      pred := paeth(left, up, upper_left)
    else
      pred := 0;
    d.cur[uint(1 + i)] := byte((int(d.cur[uint(1 + i)]) + pred) mod 256);
    i := i + 1;
  end;
  return Ok;
end;

// Alpha composite over white: (c*a + 255*(255-a)) div 255.
function over_white(c: int; a: int): int;
begin
  return (c * a + 255 * (255 - a)) div 255;
end;

// Expands the unfiltered source row to opaque RGB in the caller's row.
function convert(d: const Decoder; row: var bytes[CallerRowCap]): Status;
var
  x, base: uint;
  idx, r, g, b, a, grey: int;
begin
  row := "";
  x := uint(0);
  while x < d.out_w do
  begin
    base := uint(1) + x * d.bpp;
    if d.color_type = uint(0) then
    begin
      r := int(d.cur[base]);
      g := int(d.cur[base]);
      b := int(d.cur[base]);
    end
    else if d.color_type = uint(2) then
    begin
      r := int(d.cur[base]);
      g := int(d.cur[base + uint(1)]);
      b := int(d.cur[base + uint(2)]);
    end
    else if d.color_type = uint(3) then
    begin
      idx := int(d.cur[base]);
      if idx < 0 or uint(idx) >= core.len(d.palette) div uint(3) then
        return Invalid;
      r := int(d.palette[uint(3 * idx)]);
      g := int(d.palette[uint(3 * idx + 1)]);
      b := int(d.palette[uint(3 * idx + 2)]);
      if uint(idx) < core.len(d.trns) then
      begin
        a := int(d.trns[uint(idx)]);
        r := over_white(r, a);
        g := over_white(g, a);
        b := over_white(b, a);
      end;
    end
    else if d.color_type = uint(4) then
    begin
      grey := int(d.cur[base]);
      a := int(d.cur[base + uint(1)]);
      r := over_white(grey, a);
      g := r;
      b := r;
    end
    else
    begin
      r := int(d.cur[base]);
      g := int(d.cur[base + uint(1)]);
      b := int(d.cur[base + uint(2)]);
      a := int(d.cur[base + uint(3)]);
      r := over_white(r, a);
      g := over_white(g, a);
      b := over_white(b, a);
    end;
    row[uint(3) * x] := byte(r);
    row[uint(3) * x + uint(1)] := byte(g);
    row[uint(3) * x + uint(2)] := byte(b);
    x := x + uint(1);
  end;
  return Ok;
end;

// Produces the next row top to bottom. `got_w` is the output width and `y` is
// the row index. After `out_h` rows the caller stops; a further call is
// `Invalid` rather than a fault.
function next_row(d: var Decoder; row: var bytes[CallerRowCap];
                  got_w: var uint; y: var uint): Status;
var
  st: Status;
begin
  if not d.started or d.y >= d.out_h then
    return Invalid;
  st := fill_row(d);
  if st <> Ok then
    return st;
  st := unfilter(d);
  if st <> Ok then
    return st;
  d.prev := d.cur;
  d.have_prev := true;
  st := convert(d, row);
  if st <> Ok then
    return st;
  got_w := d.out_w;
  y := d.y;
  d.y := d.y + uint(1);
  return Ok;
end;

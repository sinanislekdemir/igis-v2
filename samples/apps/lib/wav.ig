unit wav;

// Streaming reader for uncompressed PCM WAV (RIFF/WAVE) under the M21 rules.
// Ordinary `/lib` module source: no `main`, no top-level `var` state and no
// stream endpoint. No new builtin or opcode.
//
// The player never holds the file: `open` walks the chunk headers once and
// records the absolute offset and length of the `data` chunk; `read` returns
// one 256-byte `core.fs_read` piece of it. Accepted input is PCM (`fmt` tag 1),
// 16-bit, stereo, at 44.1 kHz or 48 kHz, matching the two stream formats the
// HDA driver programs (spec 0030 A8; ADR 0064). Anything else is `Invalid`;
// there is no resampling and no compressed format.
//
//   var info: wav.Info;
//   st := wav.open(path, info);
//   while ... do
//     st := wav.read(path, info, cursor, chunk, got);
//
// A missing file is `NotFound`; a malformed header, a non-PCM or unsupported
// format, or a missing `data` chunk is `Invalid`; a read failure after a
// successful open is that read's status. `open` publishes `info` only on `Ok`.

uses core;

const
  ChunkCap = 256;   // one `core.fs_read` piece covers a header or a data block
  MaxChunks = 24;

type Info = record
  data_offset: uint;  // absolute file offset of the first sample byte
  data_size: uint;    // bytes in the data chunk (clamped to the file)
  rate: uint;         // 44100 or 48000
  channels: uint;     // 2
  bits: uint;         // 16
  opened: bool;
end;

function u16le(buf: const bytes[ChunkCap]; at: uint): uint;
begin
  return uint(buf[at]) + uint(buf[at + uint(1)]) * uint(256);
end;

function u32le(buf: const bytes[ChunkCap]; at: uint): uint;
begin
  return uint(buf[at])
    + uint(buf[at + uint(1)]) * uint(256)
    + uint(buf[at + uint(2)]) * uint(65536)
    + uint(buf[at + uint(3)]) * uint(16777216);
end;

function is_riff(buf: const bytes[ChunkCap]; at: uint): bool;
begin
  return uint(buf[at]) = uint(82) and uint(buf[at + uint(1)]) = uint(73)
    and uint(buf[at + uint(2)]) = uint(70)
    and uint(buf[at + uint(3)]) = uint(70);
end;

function is_wave(buf: const bytes[ChunkCap]; at: uint): bool;
begin
  return uint(buf[at]) = uint(87) and uint(buf[at + uint(1)]) = uint(65)
    and uint(buf[at + uint(2)]) = uint(86)
    and uint(buf[at + uint(3)]) = uint(69);
end;

function is_fmt(buf: const bytes[ChunkCap]; at: uint): bool;
begin
  return uint(buf[at]) = uint(102) and uint(buf[at + uint(1)]) = uint(109)
    and uint(buf[at + uint(2)]) = uint(116)
    and uint(buf[at + uint(3)]) = uint(32);
end;

function is_data(buf: const bytes[ChunkCap]; at: uint): bool;
begin
  return uint(buf[at]) = uint(100) and uint(buf[at + uint(1)]) = uint(97)
    and uint(buf[at + uint(2)]) = uint(116)
    and uint(buf[at + uint(3)]) = uint(97);
end;

// Parses the RIFF/WAVE header and the first `fmt `/`data` pair. The whole
// record is filled locally and published only on success.
function open(path: const string[0]; info: var Info): Status;
var
  buf: bytes[ChunkCap];
  got: int;
  file_size: uint;
  at: uint;
  size: uint;
  rate: uint;
  channels: uint;
  bits: uint;
  data_offset: uint;
  data_size: uint;
  have_fmt: bool;
  have_data: bool;
  chunks: int;
  st: Status;
begin
  info.opened := false;
  st := core.fs_size(path, file_size);
  if st <> Ok then
    return st;
  if file_size < uint(44) then
    return Invalid;
  st := core.fs_read(path, uint(0), buf, got);
  if st <> Ok then
    return st;
  if got < 12 or not is_riff(buf, uint(0)) or not is_wave(buf, uint(8)) then
    return Invalid;

  at := uint(12);
  have_fmt := false;
  have_data := false;
  chunks := 0;
  while at + uint(8) <= file_size and not have_data and chunks < MaxChunks do
  begin
    st := core.fs_read(path, at, buf, got);
    if st <> Ok or got < 8 then
      return Invalid;
    size := u32le(buf, uint(4));
    if is_fmt(buf, uint(0)) then
    begin
      // `fmt ` is at least 16 bytes of PCM description; the 256-byte read
      // covers it whenever the chunk starts inside the file.
      if size < uint(16) or got < 24 then
        return Invalid;
      if u16le(buf, uint(8)) <> uint(1) then
        return Invalid;
      channels := u16le(buf, uint(10));
      rate := u32le(buf, uint(12));
      bits := u16le(buf, uint(22));
      have_fmt := true;
    end
    else if is_data(buf, uint(0)) then
    begin
      data_offset := at + uint(8);
      data_size := size;
      // A size of zero or one past the file is a streamed/incomplete header;
      // clamp to what the file actually holds.
      if data_size = uint(0) or data_size > file_size - data_offset then
        data_size := file_size - data_offset;
      have_data := true;
    end;
    at := at + uint(8) + size + (size mod uint(2));
    chunks := chunks + 1;
  end;

  if not have_fmt or not have_data then
    return Invalid;
  if channels <> uint(2) or bits <> uint(16) then
    return Invalid;
  if rate <> uint(44100) and rate <> uint(48000) then
    return Invalid;
  if data_size = uint(0) then
    return Invalid;

  info.data_offset := data_offset;
  info.data_size := data_size;
  info.rate := rate;
  info.channels := channels;
  info.bits := bits;
  info.opened := true;
  return Ok;
end;

// Reads one chunk of the data block at `cursor` (0 = first sample byte). The
// result is clamped to the data chunk, so a file with a trailing chunk never
// leaks metadata into the stream. `got` is 0 at the end of the data.
function read(path: const string[0]; info: var Info; cursor: uint;
              dst: var bytes[ChunkCap]; got: var int): Status;
var
  remaining: uint;
  st: Status;
begin
  if not info.opened then
  begin
    got := 0;
    return NotFound;
  end;
  if cursor >= info.data_size then
  begin
    got := 0;
    return Ok;
  end;
  remaining := info.data_size - cursor;
  st := core.fs_read(path, info.data_offset + cursor, dst, got);
  if st <> Ok then
    return st;
  if uint(got) > remaining then
    got := int(remaining);
  return Ok;
end;

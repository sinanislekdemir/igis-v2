unit inflate_probe;

// Test-only proof for `lib.inflate` (spec 0023, G2). It is staged only by
// `xtask image --shell` and by `langhost`'s e2e test; it opens no window.
//
//   run inflate_probe
//
// Every vector is read from `system/inflate_<name>.zlib` (at most 309 B) and
// its expected output from `system/inflate_<name>.bin` (at most 4500 B), using
// `core.fs_size` plus 256-byte `core.fs_read` chunks. The binaries live in the
// test-only `system/` directory rather than `base/` because `base/` already
// holds more than the browser's 96-entry listing cap would tolerate in the
// `--shell` image; both directories are staged by `xtask image --shell`.
// Each stream is decoded
// through the bounded-prefix API, every output chunk is compared, and the exact
// total length plus `done` are required. The `long` vector is a 64 KiB
// 12-symbol pseudorandom payload (Python `random.Random(7)`, `randrange(12)`)
// compressed to 34,373 bytes; it fills the 512-byte bit-reader window 67 times
// and proves `fill` compacts unread bytes. Its success prints nothing, so the
// committed e2e output is unchanged. `dynamic` is decoded twice: once with
// half of the compressed prefix, which must end in `WouldBlock` after some
// output, and then with the full prefix from the same state, so no byte can be
// lost or repeated at the pause. `truncated` and `corrupt` must produce a
// non-Ok status and no fault.

uses core, lib.out, lib.inflate;

const
  ChunkSize = 256;
  LongCap = 65536;      // 64 KiB: the long vector and its compressed form

var
  failures: int;

procedure fail(name: const string[0]; what: const string[0]);
begin
  out.say("inflate: ");
  out.say(name);
  out.say(": ");
  out.say(what);
  out.say("\n");
  failures := failures + 1;
end;

// Loads at most 4 KiB of a compressed vector, chunked at 256 bytes.
function load_z(name: const string[0]; dst: var bytes[4096];
                size: var int): bool;
var
  total: uint;
  offset: uint;
  got: int;
  st: Status;
  chunk: bytes[ChunkSize];
begin
  dst := "";
  size := 0;
  st := core.fs_size(name, total);
  if st <> Ok then
    return false;
  if total > uint(4096) then
    return false;
  offset := uint(0);
  while offset < total do
  begin
    got := -1;
    st := core.fs_read(name, offset, chunk, got);
    if st <> Ok then
      return false;
    if got <= 0 then
      return false;
    if uint(got) > total - offset then
      return false;
    st := core.copy_bytes(dst, offset, chunk, uint(0), uint(got));
    if st <> Ok then
      return false;
    offset := offset + uint(got);
  end;
  if core.len(dst) <> total then
    return false;
  size := int(total);
  return true;
end;

// Loads at most 8 KiB of expected output, chunked at 256 bytes.
function load_b(name: const string[0]; dst: var bytes[8192];
                size: var int): bool;
var
  total: uint;
  offset: uint;
  got: int;
  st: Status;
  chunk: bytes[ChunkSize];
begin
  dst := "";
  size := 0;
  st := core.fs_size(name, total);
  if st <> Ok then
    return false;
  if total > uint(8192) then
    return false;
  offset := uint(0);
  while offset < total do
  begin
    got := -1;
    st := core.fs_read(name, offset, chunk, got);
    if st <> Ok then
      return false;
    if got <= 0 then
      return false;
    if uint(got) > total - offset then
      return false;
    st := core.copy_bytes(dst, offset, chunk, uint(0), uint(got));
    if st <> Ok then
      return false;
    offset := offset + uint(got);
  end;
  if core.len(dst) <> total then
    return false;
  size := int(total);
  return true;
end;

// Compares one output chunk with expected[offset..] and advances `offset`.
function same_bytes(outbuf: const bytes[4096]; expected: const bytes[8192];
                    offset: int; got: int): bool;
var
  i: int;
begin
  if got < 0 or offset + got > int(core.len(expected)) then
    return false;
  i := 0;
  while i < got do
  begin
    if outbuf[i] <> expected[offset + i] then
      return false;
    i := i + 1;
  end;
  return true;
end;

// Decodes with the full prefix to `done`, comparing every chunk from
// `offset` with `expected` and requiring the exact total length.
function drain_full(name: const string[0]; d: var inflate.Inflater;
                    z: const bytes[4096]; zlen: int;
                    expected: const bytes[8192]; elen: int;
                    offset: var int): bool;
var
  outbuf: bytes[4096];
  got: int;
  st: Status;
begin
  while not inflate.done(d) do
  begin
    got := 0;
    st := inflate.read(d, z, uint(zlen), outbuf, got);
    if st = WouldBlock then
    begin
      fail(name, "unexpected WouldBlock");
      return false;
    end;
    if st <> Ok then
    begin
      fail(name, "status");
      return false;
    end;
    if not same_bytes(outbuf, expected, offset, got) then
    begin
      fail(name, "byte mismatch");
      return false;
    end;
    offset := offset + got;
    if got = 0 and not inflate.done(d) then
    begin
      fail(name, "stalled");
      return false;
    end;
  end;
  if offset <> elen then
  begin
    fail(name, "total length");
    return false;
  end;
  return true;
end;

function decode_vector(name: const string[0]): bool;
var
  zpath, bpath: string[64];
  z: bytes[4096];
  expected: bytes[8192];
  d: inflate.Inflater;
  zlen, elen, offset: int;
  st: Status;
begin
  zpath := "system/inflate_" + name + ".zlib";
  bpath := "system/inflate_" + name + ".bin";
  if not load_z(zpath, z, zlen) then
  begin
    fail(name, "zlib read");
    return false;
  end;
  if not load_b(bpath, expected, elen) then
  begin
    fail(name, "bin read");
    return false;
  end;
  st := inflate.init(d);
  if st <> Ok then
  begin
    fail(name, "init");
    return false;
  end;
  offset := 0;
  return drain_full(name, d, z, zlen, expected, elen, offset);
end;

// Loads at most 64 KiB of the long vector, chunked at 256 bytes.
function load_long(name: const string[0]; dst: var bytes[LongCap];
                   size: var int): bool;
var
  total: uint;
  offset: uint;
  got: int;
  st: Status;
  chunk: bytes[ChunkSize];
begin
  dst := "";
  size := 0;
  st := core.fs_size(name, total);
  if st <> Ok then
    return false;
  if total > uint(LongCap) then
    return false;
  offset := uint(0);
  while offset < total do
  begin
    got := -1;
    st := core.fs_read(name, offset, chunk, got);
    if st <> Ok then
      return false;
    if got <= 0 then
      return false;
    if uint(got) > total - offset then
      return false;
    st := core.copy_bytes(dst, offset, chunk, uint(0), uint(got));
    if st <> Ok then
      return false;
    offset := offset + uint(got);
  end;
  if core.len(dst) <> total then
    return false;
  size := int(total);
  return true;
end;

// A 64 KiB 12-symbol pseudorandom payload compressed to 34,373 bytes: the
// compressed stream spans 67 full 512-byte bit-reader windows, so decoding
// crosses the boundary where `fill` must compact unread bytes. Success prints
// nothing, so the committed e2e output is unchanged; a failure names `long`.
procedure decode_long();
var
  z: bytes[LongCap];
  expected: bytes[LongCap];
  outbuf: bytes[4096];
  d: inflate.Inflater;
  zlen, elen, got, offset: int;
  st: Status;
begin
  if not load_long("system/inflate_long.zlib", z, zlen) then
  begin
    fail("long", "zlib read");
    return;
  end;
  if not load_long("system/inflate_long.bin", expected, elen) then
  begin
    fail("long", "bin read");
    return;
  end;
  st := inflate.init(d);
  if st <> Ok then
  begin
    fail("long", "init");
    return;
  end;
  offset := 0;
  while not inflate.done(d) do
  begin
    got := 0;
    st := inflate.read(d, z, uint(zlen), outbuf, got);
    if st = WouldBlock then
    begin
      fail("long", "unexpected WouldBlock");
      return;
    end;
    if st <> Ok then
    begin
      fail("long", "status");
      return;
    end;
    if not same_bytes(outbuf, expected, offset, got) then
    begin
      fail("long", "byte mismatch");
      return;
    end;
    offset := offset + got;
    if got = 0 and not inflate.done(d) then
    begin
      fail("long", "stalled");
      return;
    end;
  end;
  if offset <> elen then
    fail("long", "total length");
end;

// A truncated or corrupt stream must report Invalid or WouldBlock, never run to
// completion and never fault.
function decode_bad(name: const string[0]): bool;
var
  zpath: string[64];
  z: bytes[4096];
  outbuf: bytes[4096];
  d: inflate.Inflater;
  zlen, got, calls: int;
  st: Status;
  bad, go: bool;
begin
  zpath := "system/inflate_" + name + ".zlib";
  if not load_z(zpath, z, zlen) then
  begin
    fail(name, "zlib read");
    return false;
  end;
  st := inflate.init(d);
  if st <> Ok then
  begin
    fail(name, "init");
    return false;
  end;
  bad := false;
  go := true;
  calls := 0;
  while go and calls < 64 do
  begin
    got := 0;
    st := inflate.read(d, z, uint(zlen), outbuf, got);
    if st <> Ok then
    begin
      bad := true;
      go := false;
    end
    else if inflate.done(d) then
      go := false
    else if got = 0 then
      go := false
    else
      calls := calls + 1;
  end;
  if not bad then
  begin
    fail(name, "decoded anyway");
    return false;
  end;
  return true;
end;

// The bounded-prefix contract. First, half of the `dynamic` stream must decode
// some output and stop with WouldBlock, then the full prefix must continue from
// the same state to the exact expected bytes. Then a 4-byte prefix (shorter than
// the dynamic block header) must stop with WouldBlock and no output, and the
// full prefix must still produce the whole stream from the block-header
// checkpoint.
function decode_bounded(): bool;
var
  z: bytes[4096];
  expected: bytes[8192];
  outbuf: bytes[4096];
  d: inflate.Inflater;
  zlen, elen, got, offset, half: int;
  st: Status;
  blocked, go: bool;
begin
  if not load_z("system/inflate_dynamic.zlib", z, zlen) then
  begin
    fail("bounded", "zlib read");
    return false;
  end;
  if not load_b("system/inflate_dynamic.bin", expected, elen) then
  begin
    fail("bounded", "bin read");
    return false;
  end;
  st := inflate.init(d);
  if st <> Ok then
  begin
    fail("bounded", "init");
    return false;
  end;
  half := zlen div 2;
  offset := 0;
  blocked := false;
  go := true;
  while go do
  begin
    got := 0;
    st := inflate.read(d, z, uint(half), outbuf, got);
    if st = WouldBlock then
    begin
      blocked := true;
      go := false;
    end
    else if st <> Ok then
    begin
      fail("bounded", "half status");
      return false;
    end
    else
    begin
      if not same_bytes(outbuf, expected, offset, got) then
      begin
        fail("bounded", "half mismatch");
        return false;
      end;
      offset := offset + got;
      if inflate.done(d) then
        go := false
      else if got = 0 then
      begin
        fail("bounded", "half stalled");
        return false;
      end;
    end;
  end;
  if not blocked then
  begin
    fail("bounded", "half prefix completed");
    return false;
  end;
  if offset = 0 then
  begin
    fail("bounded", "half produced nothing");
    return false;
  end;
  if not drain_full("bounded", d, z, zlen, expected, elen, offset) then
    return false;

  // A prefix that ends inside the dynamic Huffman header: the block header is
  // checkpointed, so the retry re-parses it and nothing is lost.
  st := inflate.init(d);
  if st <> Ok then
  begin
    fail("bounded", "tiny init");
    return false;
  end;
  offset := 0;
  got := 0;
  st := inflate.read(d, z, uint(4), outbuf, got);
  if st <> WouldBlock or got <> 0 then
  begin
    fail("bounded", "tiny prefix");
    return false;
  end;
  if not drain_full("bounded", d, z, zlen, expected, elen, offset) then
    return false;
  return true;
end;

procedure vector(name: const string[0]);
begin
  if decode_vector(name) then
  begin
    out.say("inflate: ");
    out.say(name);
    out.say(" ok\n");
  end;
end;

procedure main(args: const string[1024]);
begin
  failures := 0;
  vector("stored");
  vector("fixed");
  vector("dynamic");
  vector("lz");
  vector("mixed");
  decode_long();
  if decode_bad("truncated") then
    out.say("inflate: truncated ok\n");
  if decode_bad("corrupt") then
    out.say("inflate: corrupt ok\n");
  if decode_bounded() then
    out.say("inflate: bounded ok\n");
  if failures = 0 then
    out.say("inflate: ok\n");
end;

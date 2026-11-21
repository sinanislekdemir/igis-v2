unit png_probe;

// Test-only proof for `lib.png` (spec 0023, P4-P13; ADR 0057). It is staged
// only by `xtask image --shell` and by `langhost`'s e2e test; it opens no
// window.
//
//   run png_probe
//
// Every fixture is read from `system/png_<name>.png` (at most 4.5 KiB) with
// `core.fs_size` plus 256-byte `core.fs_read` chunks and `core.copy_bytes`.
// Geometry is checked through `png.probe`; decoded rows are compared pixel by
// pixel after `png.open` + `png.next_row`. The fixtures live in the test-only
// `system/` tree rather than `base/` because `base/` is already near the
// browser's 96-entry listing cap in the `--shell` image; both trees are staged
// by `xtask image --shell`.
//
// `png_truncated.png`, `png_corrupt.png` and `png_not_a_png.bin` must fail with
// `Invalid` or `CapacityExceeded` somewhere in probe/open/next_row and must
// not fault. The supplied `corrupt.png` differed from `grad_il.png` only in the
// zlib Adler-32, which spec 0023 P12 does not verify, so the committed
// `png_corrupt.png` changes the first DEFLATE block type to BTYPE 3 (invalid)
// instead: the structural failure is observable in the first row call.

uses core, lib.out, lib.png;

const
  ChunkSize = 256;
  FileCap = 8192;
  RowCap = 16384;

var
  failures: int;
  d: png.Decoder;
  pixels: bytes[RowCap];

procedure fail(name: const string[0]; what: const string[0]);
begin
  out.say("png: ");
  out.say(name);
  out.say(": ");
  out.say(what);
  out.say("\n");
  failures := failures + 1;
end;

procedure ok_line(name: const string[0]);
begin
  out.say("png: ");
  out.say(name);
  out.say(" ok\n");
end;

// Loads a fixture through the ordinary file primitives. The bytes are not used
// by the decoder (which reads the path itself), but this proves the fixture is
// present and readable in both the host and the `--shell` image.
function load(path: const string[0]; dst: var bytes[FileCap];
              size: var int): bool;
var
  total, offset: uint;
  got: int;
  st: Status;
  chunk: bytes[ChunkSize];
begin
  dst := "";
  size := 0;
  st := core.fs_size(path, total);
  if st <> Ok then
    return false;
  if total > uint(FileCap) then
    return false;
  offset := uint(0);
  while offset < total do
  begin
    got := -1;
    st := core.fs_read(path, offset, chunk, got);
    if st <> Ok then
      return false;
    if got <= 0 then
      return false;
    if uint(got) > total - offset then
      return false;
    if core.copy_bytes(dst, offset, chunk, uint(0), uint(got)) <> Ok then
      return false;
    offset := offset + uint(got);
  end;
  if core.len(dst) <> total then
    return false;
  size := int(total);
  return true;
end;

function pixel_is(x: uint; r: int; g: int; b: int): bool;
begin
  if uint(3) * x + uint(2) >= core.len(pixels) then
    return false;
  return int(pixels[uint(3) * x]) = r
    and int(pixels[uint(3) * x + uint(1)]) = g
    and int(pixels[uint(3) * x + uint(2)]) = b;
end;

function rewind_ok(name: const string[0]): bool;
begin
  if png.rewind(d) <> Ok then
  begin
    fail(name, "rewind");
    return false;
  end;
  return true;
end;

function get_row(name: const string[0]; y: uint; want: uint): bool;
var
  got_w, yy: uint;
  st: Status;
begin
  got_w := uint(0);
  yy := uint(0);
  st := png.next_row(d, pixels, got_w, yy);
  if st <> Ok then
  begin
    fail(name, "row status");
    return false;
  end;
  if yy <> y then
  begin
    fail(name, "row index");
    return false;
  end;
  if got_w <> want then
  begin
    fail(name, "row width");
    return false;
  end;
  if core.len(pixels) <> uint(3) * want then
  begin
    fail(name, "row length");
    return false;
  end;
  return true;
end;

function expect_probe(name: const string[0]; path: const string[0];
                      ew: uint; eh: uint; eow: uint; eoh: uint;
                      ect: uint; eil: uint): bool;
var
  staged: bytes[FileCap];
  size: int;
  w, h, ow, oh, ct, il: uint;
  st: Status;
begin
  if not load(path, staged, size) then
  begin
    fail(name, "read");
    return false;
  end;
  w := uint(0);
  h := uint(0);
  ow := uint(0);
  oh := uint(0);
  ct := uint(0);
  il := uint(0);
  st := png.probe(path, w, h, ow, oh, ct, il);
  if st <> Ok then
  begin
    fail(name, "probe status");
    return false;
  end;
  if w <> ew or h <> eh or ow <> eow or oh <> eoh
    or ct <> ect or il <> eil then
  begin
    fail(name, "probe geometry");
    return false;
  end;
  return true;
end;

function expect_open(name: const string[0]; path: const string[0];
                     eow: uint; eoh: uint): bool;
var
  w, h, ow, oh, ct, il: uint;
  st: Status;
begin
  st := png.open(path, d);
  if st <> Ok then
  begin
    fail(name, "open status");
    return false;
  end;
  w := uint(0);
  h := uint(0);
  ow := uint(0);
  oh := uint(0);
  ct := uint(0);
  il := uint(0);
  st := png.info(d, w, h, ow, oh, ct, il);
  if st <> Ok then
  begin
    fail(name, "info status");
    return false;
  end;
  if ow <> eow or oh <> eoh then
  begin
    fail(name, "info geometry");
    return false;
  end;
  return true;
end;

// `probe` alone must reject an unsupported depth and a preview wider than the
// caller's row.
function test_probe_status(name: const string[0]; path: const string[0];
                           want: Status): bool;
var
  staged: bytes[FileCap];
  size: int;
  w, h, ow, oh, ct, il: uint;
  st: Status;
begin
  if not load(path, staged, size) then
  begin
    fail(name, "read");
    return false;
  end;
  w := uint(0);
  h := uint(0);
  ow := uint(0);
  oh := uint(0);
  ct := uint(0);
  il := uint(0);
  st := png.probe(path, w, h, ow, oh, ct, il);
  if st <> want then
  begin
    fail(name, "probe status");
    return false;
  end;
  return true;
end;

function test_grad_il(): bool;
var
  path: string[64];
  ok: bool;
begin
  path := "system/png_grad_il.png";
  ok := expect_probe("grad_il", path, uint(16), uint(16),
                     uint(2), uint(2), uint(2), uint(1));
  if ok then
    ok := expect_open("grad_il", path, uint(2), uint(2));
  if ok then
    ok := get_row("grad_il", uint(0), uint(2));
  if ok then
    ok := pixel_is(uint(0), 0, 0, 0);
  if ok then
    ok := pixel_is(uint(1), 128, 0, 64);
  if ok then
    ok := get_row("grad_il", uint(1), uint(2));
  if ok then
    ok := pixel_is(uint(0), 0, 128, 32);
  if ok then
    ok := pixel_is(uint(1), 128, 128, 96);
  if ok then
    ok := rewind_ok("grad_il");
  if ok then
    ok := get_row("grad_il", uint(0), uint(2));
  if ok then
    ok := pixel_is(uint(0), 0, 0, 0);
  if ok then
    ok := pixel_is(uint(1), 128, 0, 64);
  if ok then
    ok := get_row("grad_il", uint(1), uint(2));
  if ok then
    ok := pixel_is(uint(0), 0, 128, 32);
  if ok then
    ok := pixel_is(uint(1), 128, 128, 96);
  if not ok then
    fail("grad_il", "pixel");
  return ok;
end;

// The RGB filter fixtures all encode pixel (x,y) as
// ((x*29) mod 256, (y*37) mod 256, (x*11 + y*7) mod 256).
function filter_pixel(x: int; y: int): bool;
begin
  return pixel_is(uint(x), (x * 29) mod 256, (y * 37) mod 256,
                  (x * 11 + y * 7) mod 256);
end;

// Decodes all eight rows of a filter fixture, checking row 0 and row 7.
function filter_pass(name: const string[0]): bool;
var
  ok: bool;
  y: uint;
begin
  ok := get_row(name, uint(0), uint(8));
  if ok then
    ok := filter_pixel(0, 0);
  if ok then
    ok := filter_pixel(7, 0);
  y := uint(1);
  while ok and y < uint(8) do
  begin
    ok := get_row(name, y, uint(8));
    y := y + uint(1);
  end;
  if ok then
    ok := filter_pixel(0, 7);
  if ok then
    ok := filter_pixel(7, 7);
  return ok;
end;

function test_filter(name: const string[0]): bool;
var
  path: string[64];
  ok: bool;
begin
  path := "system/png_" + name + ".png";
  ok := expect_probe(name, path, uint(8), uint(8), uint(8), uint(8),
                     uint(2), uint(0));
  if ok then
    ok := expect_open(name, path, uint(8), uint(8));
  if ok then
    ok := filter_pass(name);
  if ok then
    ok := rewind_ok(name);
  if ok then
    ok := filter_pass(name);
  if not ok then
    fail(name, "pixel");
  return ok;
end;

// The supplied `gray.png` scanlines alternate: row 0 is 0,255,0,255..., row 1
// is 255,0,255,0.... The ticket expectation had these three pixels inverted;
// the fixture was inspected and the values below are the corrected ones.
function test_gray(): bool;
var
  path: string[64];
  ok: bool;
begin
  path := "system/png_gray.png";
  ok := expect_probe("gray", path, uint(8), uint(8), uint(8), uint(8),
                     uint(0), uint(0));
  if ok then
    ok := expect_open("gray", path, uint(8), uint(8));
  if ok then
    ok := get_row("gray", uint(0), uint(8));
  if ok then
    ok := pixel_is(uint(0), 0, 0, 0);
  if ok then
    ok := pixel_is(uint(1), 255, 255, 255);
  if ok then
    ok := get_row("gray", uint(1), uint(8));
  if ok then
    ok := pixel_is(uint(0), 255, 255, 255);
  if not ok then
    fail("gray", "pixel");
  return ok;
end;

function test_palette(): bool;
var
  path: string[64];
  ok: bool;
begin
  path := "system/png_palette.png";
  ok := expect_probe("palette", path, uint(8), uint(8), uint(8), uint(8),
                     uint(3), uint(0));
  if ok then
    ok := expect_open("palette", path, uint(8), uint(8));
  if ok then
    ok := get_row("palette", uint(0), uint(8));
  if ok then
    ok := pixel_is(uint(0), 255, 0, 0);
  if ok then
    ok := pixel_is(uint(1), 0, 0, 255);
  if not ok then
    fail("palette", "pixel");
  return ok;
end;

// `rgba` and `graya` must composite every pixel over white.
function test_uniform(name: const string[0]; ct: uint;
                      r: int; g: int; b: int): bool;
var
  path: string[64];
  ok: bool;
  y, x: uint;
begin
  path := "system/png_" + name + ".png";
  ok := expect_probe(name, path, uint(8), uint(8), uint(8), uint(8),
                     ct, uint(0));
  if ok then
    ok := expect_open(name, path, uint(8), uint(8));
  y := uint(0);
  while ok and y < uint(8) do
  begin
    ok := get_row(name, y, uint(8));
    x := uint(0);
    while ok and x < uint(8) do
    begin
      ok := pixel_is(x, r, g, b);
      x := x + uint(1);
    end;
    y := y + uint(1);
  end;
  if not ok then
    fail(name, "pixel");
  return ok;
end;

function test_wide(): bool;
var
  path: string[64];
  ok: bool;
begin
  path := "system/png_wide.png";
  ok := expect_probe("wide", path, uint(1000), uint(1000),
                     uint(1000), uint(1000), uint(2), uint(0));
  if ok then
    ok := expect_open("wide", path, uint(1000), uint(1000));
  if ok then
    ok := get_row("wide", uint(0), uint(1000));
  if ok then
    ok := pixel_is(uint(0), 10, 20, 30);
  if ok then
    ok := pixel_is(uint(999), 10, 20, 30);
  if ok then
    ok := rewind_ok("wide");
  if ok then
    ok := get_row("wide", uint(0), uint(1000));
  if ok then
    ok := pixel_is(uint(0), 10, 20, 30);
  if ok then
    ok := pixel_is(uint(999), 10, 20, 30);
  if not ok then
    fail("wide", "pixel");
  return ok;
end;

// A bad file must report Invalid or CapacityExceeded from probe, open or a
// row read, and must not fault. A corrupt stream may decode a prefix first, so
// the row loop tolerates successful calls until the failure or the row limit.
function test_rejects(name: const string[0]; path: const string[0]): bool;
var
  w, h, ow, oh, ct, il, got_w, y: uint;
  st: Status;
  rows: uint;
  good: bool;
begin
  st := png.probe(path, w, h, ow, oh, ct, il);
  if st = Ok then
  begin
    st := png.open(path, d);
    if st = Ok then
    begin
      // A corrupt stream may decode a prefix, but it must fail before the
      // last declared row; passing the whole preview would mean the
      // corruption was never observed.
      st := png.info(d, w, h, ow, oh, ct, il);
      got_w := uint(0);
      y := uint(0);
      rows := uint(0);
      while st = Ok and rows < oh do
      begin
        st := png.next_row(d, pixels, got_w, y);
        rows := rows + uint(1);
      end;
    end;
  end;
  good := st = Invalid or st = CapacityExceeded;
  if not good then
    fail(name, "not rejected");
  return good;
end;

procedure main(args: const string[1024]);
begin
  failures := 0;
  if png.rewind(d) <> Invalid then
    fail("rewind", "before open");
  if test_grad_il() then
    ok_line("grad_il");
  if test_filter("filter0") then
    ok_line("filter0");
  if test_filter("filter1") then
    ok_line("filter1");
  if test_filter("filter2") then
    ok_line("filter2");
  if test_filter("filter3") then
    ok_line("filter3");
  if test_filter("filter4") then
    ok_line("filter4");
  if test_gray() then
    ok_line("gray");
  if test_palette() then
    ok_line("palette");
  if test_uniform("rgba", uint(6), 255, 127, 127) then
    ok_line("rgba");
  if test_uniform("graya", uint(4), 227, 227, 227) then
    ok_line("graya");
  if test_wide() then
    ok_line("wide");
  if test_probe_status("depth16", "system/png_depth16.png", Invalid)
    and test_probe_status("gray_il", "system/png_gray_il.png", Invalid)
    and test_probe_status("too_wide", "system/png_too_wide.png", CapacityExceeded)
    and test_rejects("truncated", "system/png_truncated.png")
    and test_rejects("corrupt", "system/png_corrupt.png")
    and test_rejects("not_a_png", "system/png_not_a_png.bin") then
    out.say("png: rejects ok\n");
  if failures = 0 then
    out.say("png: ok\n");
end;

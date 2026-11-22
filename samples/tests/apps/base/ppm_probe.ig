unit ppm_probe;

// Test-only proof for `lib.ppm` (spec 0024; ADR 0058). It is staged only by
// `xtask image --shell` and by `langhost`'s e2e test; it opens no window.
//
//   run ppm_probe
//
// Every fixture is read from `system/ppm_<name>.ppm` with the ordinary file
// API. The 8x8 RGB fixtures encode pixel (x,y) as
// ((x*29) mod 256, (y*37) mod 256, (x*11 + y*7) mod 256); every pixel of every
// row is checked. `ppm_comment.ppm` is the same raster with leading whitespace
// and comments between header tokens. The negative fixtures prove that a P5
// magic, a P3 magic and maxval 65535 are `Invalid`, that a missing file is
// `NotFound`, that a short raster fails the row read with `Invalid`, and that
// a 6000-pixel row is `CapacityExceeded`. None of them may fault.
//
// The fixtures live in the test-only `system/` tree because `base/` is already
// near the browser's 96-entry listing cap in the `--shell` image; both trees
// are staged by `xtask image --shell`.

uses core, lib.out, lib.ppm;

const
  RowCap = 16384;

var
  failures: int;
  d: ppm.Decoder;
  pixels: bytes[RowCap];

procedure fail(name: const string[0]; what: const string[0]);
begin
  out.say("ppm: ");
  out.say(name);
  out.say(": ");
  out.say(what);
  out.say("\n");
  failures := failures + 1;
end;

procedure ok_line(name: const string[0]);
begin
  out.say("ppm: ");
  out.say(name);
  out.say(" ok\n");
end;

function pixel_is(x: uint; r: int; g: int; b: int): bool;
begin
  if uint(3) * x + uint(2) >= core.len(pixels) then
    return false;
  return int(pixels[uint(3) * x]) = r
    and int(pixels[uint(3) * x + uint(1)]) = g
    and int(pixels[uint(3) * x + uint(2)]) = b;
end;

// One row of the 8x8 formula, including the row length.
function formula_row(name: const string[0]; y: uint): bool;
var
  got_w: uint;
  st: Status;
  x: uint;
  ok: bool;
begin
  got_w := uint(0);
  st := ppm.read_row(d, y, pixels, got_w);
  if st <> Ok then
  begin
    fail(name, "row status");
    return false;
  end;
  if got_w <> uint(8) then
  begin
    fail(name, "row width");
    return false;
  end;
  if core.len(pixels) <> uint(24) then
  begin
    fail(name, "row length");
    return false;
  end;
  ok := true;
  x := uint(0);
  while ok and x < uint(8) do
  begin
    ok := pixel_is(x, (int(x) * 29) mod 256, (int(y) * 37) mod 256,
                   (int(x) * 11 + int(y) * 7) mod 256);
    x := x + uint(1);
  end;
  return ok;
end;

// Probes, opens and reads every row of an 8x8 formula image.
function test_image(name: const string[0]; path: const string[0]): bool;
var
  w, h, got_w: uint;
  st: Status;
  y: uint;
  ok: bool;
begin
  w := uint(0);
  h := uint(0);
  st := ppm.probe(path, w, h);
  if st <> Ok or w <> uint(8) or h <> uint(8) then
  begin
    fail(name, "probe");
    return false;
  end;
  st := ppm.open(path, d);
  if st <> Ok then
  begin
    fail(name, "open status");
    return false;
  end;
  w := uint(0);
  h := uint(0);
  st := ppm.info(d, w, h);
  if st <> Ok or w <> uint(8) or h <> uint(8) then
  begin
    fail(name, "info");
    return false;
  end;
  ok := true;
  y := uint(0);
  while ok and y < uint(8) do
  begin
    ok := formula_row(name, y);
    y := y + uint(1);
  end;
  if ok then
  begin
    // A row index at the height is `Invalid`, not a fault.
    got_w := uint(0);
    st := ppm.read_row(d, uint(8), pixels, got_w);
    if st <> Invalid then
    begin
      fail(name, "past last row");
      ok := false;
    end;
  end;
  return ok;
end;

// A status-only probe check. It leaves the out-parameters untouched on
// failure; the sentinels prove it.
function probe_status(name: const string[0]; path: const string[0];
                      want: Status): bool;
var
  w, h: uint;
  st: Status;
begin
  w := uint(999);
  h := uint(999);
  st := ppm.probe(path, w, h);
  if st <> want then
  begin
    fail(name, "probe status");
    return false;
  end;
  if want <> Ok and (w <> uint(999) or h <> uint(999)) then
  begin
    fail(name, "probe out-params");
    return false;
  end;
  return true;
end;

// A rejected header must fail both probe and open. A fresh decoder must not
// become opened after a failed open.
function test_bad_header(name: const string[0]; path: const string[0]): bool;
var
  bad: ppm.Decoder;
  w, h: uint;
begin
  if not probe_status(name, path, Invalid) then
    return false;
  if ppm.open(path, bad) <> Invalid then
  begin
    fail(name, "open status");
    return false;
  end;
  w := uint(0);
  h := uint(0);
  if ppm.info(bad, w, h) <> Invalid then
  begin
    fail(name, "info after failed open");
    return false;
  end;
  return true;
end;

// The header is valid but the raster ends early: the row read reports
// `Invalid`, clears the row and leaves `got_w` alone.
function test_truncated(): bool;
var
  w, h, got_w: uint;
  st: Status;
begin
  if not probe_status("truncated", "system/ppm_truncated.ppm", Ok) then
    return false;
  st := ppm.open("system/ppm_truncated.ppm", d);
  if st <> Ok then
  begin
    fail("truncated", "open status");
    return false;
  end;
  st := ppm.info(d, w, h);
  if st <> Ok or w <> uint(8) or h <> uint(8) then
  begin
    fail("truncated", "info");
    return false;
  end;
  got_w := uint(999);
  st := ppm.read_row(d, uint(0), pixels, got_w);
  if st <> Invalid then
  begin
    fail("truncated", "row status");
    return false;
  end;
  if got_w <> uint(999) then
  begin
    fail("truncated", "got_w touched");
    return false;
  end;
  if core.len(pixels) <> uint(0) then
  begin
    fail("truncated", "partial row");
    return false;
  end;
  return true;
end;

// A 6000-pixel row is 18,000 bytes, over the caller's 16,384-byte buffer.
function test_wide(): bool;
var
  w, h, got_w: uint;
  st: Status;
begin
  if not probe_status("wide", "system/ppm_wide.ppm", Ok) then
    return false;
  st := ppm.open("system/ppm_wide.ppm", d);
  if st <> Ok then
  begin
    fail("wide", "open status");
    return false;
  end;
  st := ppm.info(d, w, h);
  if st <> Ok or w <> uint(6000) or h <> uint(1) then
  begin
    fail("wide", "info");
    return false;
  end;
  got_w := uint(999);
  st := ppm.read_row(d, uint(0), pixels, got_w);
  if st <> CapacityExceeded then
  begin
    fail("wide", "row status");
    return false;
  end;
  if got_w <> uint(999) then
  begin
    fail("wide", "got_w touched");
    return false;
  end;
  return true;
end;

// Before open, `info` and `read_row` are `Invalid`.
function test_unopened(): bool;
var
  fresh: ppm.Decoder;
  w, h, got_w: uint;
begin
  w := uint(0);
  h := uint(0);
  if ppm.info(fresh, w, h) <> Invalid then
  begin
    fail("unopened", "info");
    return false;
  end;
  got_w := uint(0);
  if ppm.read_row(fresh, uint(0), pixels, got_w) <> Invalid then
  begin
    fail("unopened", "read_row");
    return false;
  end;
  return true;
end;

function test_rejects(): bool;
var
  bad: ppm.Decoder;
  w, h: uint;
begin
  if not test_bad_header("badmagic", "system/ppm_badmagic.ppm") then
    return false;
  if not test_bad_header("p3", "system/ppm_p3.ppm") then
    return false;
  if not test_bad_header("maxval", "system/ppm_maxval.ppm") then
    return false;
  if not probe_status("missing", "system/ppm_missing.ppm", NotFound) then
    return false;
  if ppm.open("system/ppm_missing.ppm", bad) <> NotFound then
  begin
    fail("missing", "open status");
    return false;
  end;
  w := uint(0);
  h := uint(0);
  if ppm.info(bad, w, h) <> Invalid then
  begin
    fail("missing", "info after failed open");
    return false;
  end;
  if not test_truncated() then
    return false;
  if not test_wide() then
    return false;
  if not test_unopened() then
    return false;
  return true;
end;

procedure main(args: const string[1024]);
begin
  failures := 0;
  if test_image("rgb", "system/ppm_rgb.ppm") then
    ok_line("rgb");
  if test_image("comment", "system/ppm_comment.ppm") then
    ok_line("comment");
  if test_rejects() then
    out.say("ppm: rejects ok\n");
  if failures = 0 then
    out.say("ppm: ok\n");
end;

unit visual;

// M2.6.3 visual-regression specimen (spec 0009, "Visual proof"; ADR 0032).
//
//   run visual          draws the specimen, checks it and prints `visual: ok`
//   run visual shot     the same, then `visual: ready` and a bounded hold
//
// The frame must be deterministic pixel for pixel so the host harness
// (`scripts/visual_regression.py`) can compare a QMP `screendump` against a
// committed golden. Everything that could vary is excluded on purpose:
//
//   - the window is full-screen: the frame is exactly the service chrome plus
//     this application's content, so the two kernel text windows (boot log and
//     terminal) and any earlier output are covered and never sampled;
//   - no clock, no animation, no events and no `syslib.clear` (the window
//     service owns the desktop underneath);
//   - drawing order and every coordinate are fixed; the dither is the
//     deterministic `(X + Y)` rule of spec 0009 G11;
//   - the pointer stays where it boots (0, 0) because the harness injects no
//     events, so the composited cursor has one fixed position;
//   - the hold after `visual: ready` changes no pixel.
//
// The specimen exercises the parts of the drawing surface the golden is meant
// to pin: the service chrome (border, focused title bar, stripes, prop title),
// the proportional font, the 8x8 console font, filled/dithered/outlined
// rectangles, a solid palette row and a diagonal raster. If graphics are
// disabled the sample checks the disabled-screen contract and prints
// `visual: host ok`, so the `langhost` transcript stays deterministic.

uses core, syslib, lib.out;

const
  HoldNs = 5000000000;
  MinW = 640;
  MinH = 480;

var
  failed: bool;
  step: int;
  fail_step: int;
  fail_label: string[64];
  status: Status;
  screen_w: uint;
  screen_h: uint;
  id: int;
  rect: Rect;
  black: uint;
  white: uint;
  grey: uint;
  accent: uint;
  red: uint;
  green: uint;
  blue: uint;
  cyan: uint;
  yellow: uint;
  bar: uint;
  x: int;
  y: int;
  i: int;
  start: uint;

// Records the first failure only; the runner reports it once after the frame
// is stable (printing between drawing steps would not hurt here because the
// console is covered, but keeping the paint path free of output is simpler).
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed and not failed then
  begin
    failed := true;
    fail_step := step;
    fail_label := label;
  end;
end;

procedure swatch(x0: int; y0: int; color: uint);
begin
  status := syslib.fill_rect(x0, y0, 32, 32, color);
  status := syslib.draw_rect(x0, y0, 32, 32, black);
end;

// The whole fixed composition, drawn relative to the window's content
// rectangle. At 1024x768 the content is (1, 16, 1022, 751).
procedure compose();
begin
  x := rect.x;
  y := rect.y;
  status := syslib.fill_rect(x, y, rect.w, rect.h, white);

  // Header band and 8x8 title.
  status := syslib.fill_rect(x, y, rect.w, 24, accent);
  status := syslib.draw_text(x + 12, y + 8, "igis-v2 visual regression", white);

  // Proportional-font specimen. The explicit `i` and `M` below are the
  // pixel checks for the narrow and wide ends of the advance range.
  status := syslib.draw_text_prop(x + 12, y + 36, "Chicago 12 px specimen", black);
  status := syslib.draw_text_prop(x + 12, y + 56, "ii MM Wi mw 0123456789", black);
  status := syslib.draw_text_prop(x + 12, y + 76, "ABCDEFGHIJKLMNOPQRSTUVWXYZ", black);
  status := syslib.draw_text_prop(x + 12, y + 96, "The quick brown fox jumps over the lazy dog.", black);
  status := syslib.draw_text_prop(x + 420, y + 60, "i", black);
  status := syslib.draw_text_prop(x + 440, y + 60, "M", black);

  // 8x8 console-font specimen (`A` is the explicit pixel check).
  status := syslib.draw_text(x + 12, y + 124, "A", black);
  status := syslib.draw_text(x + 28, y + 124, "8x8 console font", black);
  status := syslib.draw_text(x + 12, y + 140, "0123456789 abcdefghijklmnopqrstuvwxyz", black);
  status := syslib.draw_text(x + 12, y + 156, "ABCDEFGHIJKLMNOPQRSTUVWXYZ", black);

  // Solid palette row.
  swatch(x + 12, y + 184, black);
  swatch(x + 52, y + 184, white);
  swatch(x + 92, y + 184, grey);
  swatch(x + 132, y + 184, accent);
  swatch(x + 172, y + 184, red);
  swatch(x + 212, y + 184, green);
  swatch(x + 252, y + 184, blue);
  swatch(x + 292, y + 184, cyan);
  swatch(x + 332, y + 184, yellow);

  // Deterministic dither patch: `(X + Y)` even is inked (spec 0009 G11).
  status := syslib.fill_rect(x + 380, y + 176, 130, 100, white);
  status := syslib.fill_dither(x + 380, y + 176, 130, 100, black);
  status := syslib.draw_rect(x + 380, y + 176, 130, 100, black);

  // A framed control-like box with centred 8x8 text.
  status := syslib.fill_rect(x + 12, y + 300, 120, 28, grey);
  status := syslib.draw_rect(x + 12, y + 300, 120, 28, black);
  status := syslib.draw_text(x + 12 + 52, y + 310, "OK", black);

  // A 3x3 diagonal raster (each step a 3px block, no anti-aliasing).
  i := 0;
  while i < 60 do
  begin
    status := syslib.fill_rect(x + 160 + i * 2, y + 300 + i * 2, 3, 3, black);
    i := i + 1;
  end;

  // Alternating vertical bars on the right: more fill area with two colours,
  // and a deterministic `mod 2` pattern.
  i := 0;
  while i < 12 do
  begin
    if i mod 2 = 0 then
      bar := black
    else
      bar := grey;
    status := syslib.fill_rect(x + 560 + i * 36, y + 40, 28, 240, bar);
    i := i + 1;
  end;

  // A prop-font footer under the specimen.
  status := syslib.draw_text_prop(x + 12, y + 440, "M2.6.3 golden frame", black);
end;

procedure check();
begin
  // The window is exactly the screen: content is the frame minus the 1-px
  // border and the 16-px title bar.
  verify("content x", rect.x = 1);
  verify("content y", rect.y = 16);
  verify("content width", rect.w = int(screen_w) - 2);
  verify("content height", rect.h = int(screen_h) - 17);

  // Service chrome, read from the shadow (the cursor is composited into the
  // framebuffer only, so get_pixel sees the uncomposited shadow).
  verify("left border", syslib.get_pixel(0, 100) = black);
  verify("title fill", syslib.get_pixel(1, 1) = white);
  verify("title stripe", syslib.get_pixel(1, 2) = black);
  verify("title stripe gap", syslib.get_pixel(1, 3) = white);

  // Content background, header band and text.
  verify("content white", syslib.get_pixel(x + rect.w - 4, y + rect.h - 4) = white);
  verify("header band", syslib.get_pixel(x + 2, y + 2) = accent);
  verify("8x8 A ink", syslib.get_pixel(x + 14, y + 124) = black);
  verify("8x8 A leading cell", syslib.get_pixel(x + 12, y + 124) = white);
  verify("prop i dot", syslib.get_pixel(x + 421, y + 63) = black);
  verify("prop i stem", syslib.get_pixel(x + 421, y + 69) = black);
  verify("prop i cell blank", syslib.get_pixel(x + 420, y + 69) = white);
  verify("prop M left stem", syslib.get_pixel(x + 440, y + 62) = black);
  verify("prop M right stem", syslib.get_pixel(x + 449, y + 62) = black);

  // Palette row.
  verify("swatch black", syslib.get_pixel(x + 28, y + 200) = black);
  verify("swatch white", syslib.get_pixel(x + 68, y + 200) = white);
  verify("swatch grey", syslib.get_pixel(x + 108, y + 200) = grey);
  verify("swatch accent", syslib.get_pixel(x + 148, y + 200) = accent);
  verify("swatch red", syslib.get_pixel(x + 188, y + 200) = red);
  verify("swatch green", syslib.get_pixel(x + 228, y + 200) = green);
  verify("swatch blue", syslib.get_pixel(x + 268, y + 200) = blue);

  // Dither parity: inside the patch (the outline is black) the absolute sum
  // (382 + 193) is odd, so the pixel stays white; one to the right the sum is
  // even and the pixel is inked.
  verify("dither odd white", syslib.get_pixel(x + 381, y + 177) = white);
  verify("dither even ink", syslib.get_pixel(x + 382, y + 177) = black);

  // Control box and diagonal raster.
  verify("box border", syslib.get_pixel(x + 12, y + 300) = black);
  verify("box face", syslib.get_pixel(x + 14, y + 302) = grey);
  verify("diagonal start", syslib.get_pixel(x + 160, y + 300) = black);
  verify("first bar", syslib.get_pixel(x + 560, y + 40) = black);
  verify("second bar", syslib.get_pixel(x + 596, y + 40) = grey);
end;

// The disabled-screen contract: metrics are 0 and drawing is Invalid. This is
// the `langhost` path and a kernel without a framebuffer.
procedure host_check();
begin
  failed := false;
  step := 0;
  verify("screen width is 0", syslib.screen_width() = uint(0));
  verify("fill is Invalid", syslib.fill_rect(0, 0, 8, 8, white) = Invalid);
  verify("text is Invalid", syslib.draw_text(0, 0, "A", black) = Invalid);
  verify("prop is Invalid", syslib.draw_text_prop(0, 0, "A", black) = Invalid);
  verify("get_pixel is 0", syslib.get_pixel(0, 0) = uint(0));
  if failed then
    out.say("visual: host FAIL\n")
  else
    out.say("visual: host ok\n");
end;

// Opens the full-screen window, composes, checks and presents the specimen.
// `hold` is true for `shot`: `visual: ready` follows the complete present and
// the task then holds the frame for the host screenshot harness.
procedure specimen(hold: bool);
begin
  failed := false;
  step := 0;
  fail_step := 0;

  status := syslib.window_open(0, 0, int(screen_w) - 2, int(screen_h) - 17, "Visual Regression", id);
  if status <> Ok then
  begin
    out.say("visual: no window\n");
    return;
  end;
  status := syslib.window_content_rect(id, rect);
  if status <> Ok then
  begin
    status := syslib.window_close(id);
    out.say("visual: no content rect\n");
    return;
  end;

  compose();
  check();
  status := syslib.window_present(id);

  if failed then
  begin
    out.say("visual: FAIL at step ");
    out.say_int(fail_step);
    out.say(": ");
    out.say(fail_label);
    out.say("\n");
    return;
  end;

  // Printing goes to the shell's terminal window, which this full-screen
  // window covers; the console paints only its visible parts, so these lines
  // change no pixel of the specimen.
  out.say("visual: ok\n");
  if hold then
  begin
    out.say("visual: ready\n");
    start := core.clock();
    while core.clock() < start + uint(HoldNs) do
    begin
      core.heartbeat();
    end;
  end;
  status := syslib.window_close(id);
  out.say("visual: done\n");
end;

procedure main(args: const string[1024]);
begin
  black := uint(0x000000);
  white := uint(0x00FFFFFF);
  grey := uint(0x00CCCCCC);
  accent := uint(0x000080);
  red := uint(0x00C02020);
  green := uint(0x0020A020);
  blue := uint(0x002040C0);
  cyan := uint(0x0020C0C0);
  yellow := uint(0x00E0C020);
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();

  if args = "shot" then
  begin
    if screen_w = uint(0) or screen_h = uint(0) then
      out.say("visual: no graphics\n")
    else if int(screen_w) < MinW or int(screen_h) < MinH then
      out.say("visual: screen too small\n")
    else
      specimen(true);
  end
  else if screen_w = uint(0) or screen_h = uint(0) then
    host_check()
  else if int(screen_w) < MinW or int(screen_h) < MinH then
    out.say("visual: screen too small\n")
  else
    specimen(false);
end;

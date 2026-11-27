unit image_viewer;

// PNG/PPM image viewer (spec 0023, spec 0024). A client of the window service
// over the row-streaming `lib.png` decoder and the row-seeking `lib.ppm`
// reader: the application state never holds a full-image RGB buffer, only the
// PNG decoder, one 16,384-byte source row, one 3,072-byte preview row and a
// small 2x2 check preview.
//
//   run image_viewer                 modal file dialog, then view the choice
//   run image_viewer <path>          view one PNG or PPM
//   run image_viewer <path> shot     view, print `image_viewer: ready WxH -> WxH`
//                                    and keep pumping events for the QMP capture
//   run image_viewer check           deterministic headless check; prints
//                                    `image_viewer: ok` (or `image_viewer: host ok`)
//
// The viewer draws a fit-to-window preview 1:1. A PNG uses the Adam7 pass-1
// geometry from `lib.png`; a PPM reads every Nth source row and samples every
// Nth column into `preview_row`, so the preview is `ceil(width/8) x
// ceil(height/8)` without decoding anything. A frame that cannot fit, a probe
// failure or a read/decode failure opens a small error window instead of
// faulting.
//
// The first frame is drawn and presented *before* `png.open`: opening stages
// up to 512 KiB of IDAT payload, which takes visible time on the real corpus.
// That one render owns the window canvas for the window's whole life. The
// service keeps the canvas and recomposites it, so a move, raise, restore or
// display-mode change is an origin change in the service and needs nothing
// from this client (spec 0026 C10/C14/C16): the viewer ignores `Expose`, and
// the image is drawn and decoded exactly once. There is no image buffer to
// keep. Close/Escape leaves. Each output row reaches the window through one
// `syslib.blit_rgb` call with `pw = ow, ph = 1`.
//
// `check` is independent of the test-only `png_probe`/`ppm_probe`: it probes
// and decodes `system/png_grad_il.png` itself, asserts the four Adam7 pass-1
// pixels, then probes/opens `system/ppm_rgb.ppm`, reads all eight rows and
// asserts every pixel against `(x*29%256, y*37%256, (x*11+y*7)%256)`. With
// graphics it also blits the 2x2 PNG preview into an 8x8 window and reads the
// first two pixels back with `get_pixel`. `langhost` has no screen and stops
// after the two fixture checks with `image_viewer: host ok`.

uses core, stdlib, syslib, lib.out, lib.file_dialog, lib.png, lib.ppm;

const
  ModeNormal = 0;
  ModeShot = 1;
  ModeCheck = 2;

  KindPng = 0;
  KindPpm = 1;

  PathCapacity = 256;
  TitleCapacity = 64;
  RowCapacity = 16384;
  PreviewCapacity = 3072;   // 3 * 1024: widest PPM preview
  PpmPreviewMax = 1024;

  // A window content rectangle is at least 8x8 (spec 0011, W9).
  MinContent = 8;

  // The decode progress strip directly below the image (spec 0023).
  StatusStripH = 20;

  // Error panel content, centred like the image window.
  ErrorW = 400;
  ErrorH = 120;

  EscKey = 0x100;

var
  black: uint;
  white: uint;
  gray: uint;
  status: Status;
  ev: Event;
  screen_w: uint;
  screen_h: uint;

  // Parsed command line and the window title (the path's basename).
  mode: int;
  path: string[256];
  title: string[64];
  stage: string[256];
  message: string[256];
  digits: string[16];
  got: int;

  // Probe geometry and the window.
  sw: uint;
  sh: uint;
  ow: uint;
  oh: uint;
  ct: uint;
  il: uint;
  content_w: int;
  content_h: int;
  id: int;

  // Decoder state: one PNG decoder, one PPM decoder, one source row, one
  // sampled preview row, the 2x2 PNG check preview and the detected format.
  dec: png.Decoder;
  ppm_dec: ppm.Decoder;
  kind: int;
  row: bytes[RowCapacity];
  src_row: bytes[RowCapacity];
  preview_row: bytes[PreviewCapacity];
  preview: bytes[12];
  got_w: uint;
  ry: uint;
  y: uint;

  running: bool;
  check_where: string[64];
  // One warning only when realtime admission is refused.
  boost_warned: bool;

procedure say_uint(value: uint);
begin
  got := core.format_int_into(digits, int(value), uint(10), uint(0));
  out.say(digits);
end;

// Appends `part` to `message` (a 128-byte staging buffer).
procedure append_message(part: const string[0]);
begin
  status := stdlib.str_insert_into(message, int(core.len(message)), part);
end;

// `basename` of the path, truncated at a code-point boundary to the 64-byte
// window title cap; `Image Viewer` when there is no name.
procedure make_title(source: const string[0]);
var
  length: int;
  at: int;
  start: int;
  take: int;
begin
  title := "Image Viewer";
  length := int(core.len(source));
  at := length - 1;
  while at >= 0 and source[at] <> byte(47) do
    at := at - 1;
  start := at + 1;
  take := length - start;
  if take <= 0 then
    return;
  if take > TitleCapacity then
    take := TitleCapacity;
  while take > 0 and not stdlib.str_boundary(source, start + take) do
    take := take - 1;
  if take <= 0 then
    return;
  got := core.substr_into(title, source, start, take);
  if got <= 0 then
    title := "Image Viewer";
end;

// `check` is a first token: `check` alone or `check ...`.
function has_check_token(text: const string[0]): bool;
begin
  if int(core.len(text)) < 5 then
    return false;
  if not stdlib.starts_with(text, "check") then
    return false;
  if int(core.len(text)) = 5 then
    return true;
  return text[5] = byte(32);
end;

// Trim the whole argument tail first, then classify. `check` may be the first
// token; `shot` is the last token. Everything else is one path, possibly
// empty (the normal-mode file dialog).
procedure parse_args(raw: const string[1024]);
var
  trimmed: string[256];
  rest: string[256];
  length: int;
  last: int;
  i: int;
  tail_len: int;
  copied: int;
  is_shot: bool;
begin
  mode := ModeNormal;
  path := "";
  copied := stdlib.trim_into(trimmed, raw);
  if copied <= 0 then
    return;
  if has_check_token(trimmed) then
  begin
    mode := ModeCheck;
    return;
  end;
  length := int(core.len(trimmed));
  last := -1;
  i := 0;
  while i < length do
  begin
    if trimmed[i] = byte(32) then
      last := i;
    i := i + 1;
  end;
  if last < 0 then
  begin
    if trimmed = "shot" then
      mode := ModeShot
    else
      path := trimmed;
    return;
  end;
  tail_len := length - last - 1;
  is_shot := tail_len = 4
    and trimmed[last + 1] = byte(115) and trimmed[last + 2] = byte(104)
    and trimmed[last + 3] = byte(111) and trimmed[last + 4] = byte(116);
  if is_shot then
  begin
    mode := ModeShot;
    copied := core.substr_into(rest, trimmed, 0, last);
    if copied > 0 then
      copied := stdlib.trim_into(path, rest);
    if copied <= 0 then
      path := "";
  end
  else
    path := trimmed;
end;

// True when `text` ends with `suffix` (an exact byte suffix, case-sensitive).
function ends_with(text: const string[0]; suffix: const string[0]): bool;
var
  lt: int;
  ls: int;
  i: int;
begin
  lt := int(core.len(text));
  ls := int(core.len(suffix));
  if ls > lt then
    return false;
  i := 0;
  while i < ls do
  begin
    if text[lt - ls + i] <> suffix[i] then
      return false;
    i := i + 1;
  end;
  return true;
end;

// Detects PNG or PPM and fills the source/preview geometry. A `.png` path is
// PNG only and a `.ppm` path is PPM only; a path with neither suffix is
// sniffed by trying PPM first and PNG second. PPM previews are
// ceil(width/8) x ceil(height/8); the preview row buffer caps the width at
// 1,024 pixels and the caller's screen caps the height.
function probe_image(source: const string[0]): Status;
var
  st: Status;
begin
  kind := KindPng;
  if ends_with(source, ".png") then
  begin
    st := png.probe(source, sw, sh, ow, oh, ct, il);
    if st <> Ok then
      return st;
    return Ok;
  end;
  if ends_with(source, ".ppm") then
  begin
    kind := KindPpm;
    st := ppm.probe(source, sw, sh);
    if st <> Ok then
      return st;
  end
  else
  begin
    st := ppm.probe(source, sw, sh);
    if st = Ok then
      kind := KindPpm
    else
    begin
      st := png.probe(source, sw, sh, ow, oh, ct, il);
      if st <> Ok then
        return st;
      return Ok;
    end;
  end;

  if sw = uint(0) or sh = uint(0) then
    return Invalid;
  ow := (sw + uint(7)) div uint(8);
  oh := (sh + uint(7)) div uint(8);
  if ow > uint(PpmPreviewMax) then
    return CapacityExceeded;
  if uint(int(oh) + StatusStripH + 17) > screen_h then
    return Invalid;
  ct := uint(0);
  il := uint(0);
  return Ok;
end;

// ---- drawing --------------------------------------------------------------

// Draws the 20-pixel status strip directly below the image: a separator, the
// `Decoding NN%` / `Ready` label and a 6-pixel progress bar. The strip lies
// inside the content rectangle; the image area is never touched here.
procedure draw_strip(done: uint; total: uint; ready: bool);
var
  top: int;
  bar_x: int;
  bar_w: int;
  fill_w: int;
  pct: int;
  used: int;
begin
  // Window-local (spec 0026 C8): the strip starts right below the image.
  top := int(oh);
  status := syslib.fill_rect(0, top, content_w, StatusStripH, white);
  status := syslib.fill_rect(0, top, content_w, 1, gray);
  bar_x := 4;
  bar_w := content_w - 8;
  if bar_w > 0 then
  begin
    status := syslib.fill_rect(bar_x, top + 12, bar_w, 6, gray);
    if ready then
      fill_w := bar_w
    else
    begin
      pct := 0;
      if total > uint(0) then
        pct := (int(done) * 100) div int(total);
      fill_w := (bar_w * pct) div 100;
    end;
    if fill_w > 0 then
      status := syslib.fill_rect(bar_x, top + 12, fill_w, 6, black);
  end;
  if ready then
  begin
    if content_w >= 5 * 8 then
      status := syslib.draw_text(4, top + 2, "Ready", black);
  end
  else if content_w >= 13 * 8 then
  begin
    status := syslib.draw_text(4, top + 2, "Decoding ", black);
    pct := 0;
    if total > uint(0) then
      pct := (int(done) * 100) div int(total);
    used := core.format_int_into(digits, pct, uint(10), uint(0));
    status := syslib.draw_text(4 + 9 * 8, top + 2, digits, black);
    status := syslib.draw_text(
      4 + (9 + int(core.len(digits))) * 8, top + 2, "%", black);
  end;
end;

// One complete image frame, drawn once into the retained canvas: one blitted
// row per decoded row, plus the status strip. The whole content is filled
// white and the strip at 0% is presented before the staged-IDAT work, then the
// row loop presents a progress strip every 8 rows. Returns the first failing
// status so the caller can open the error window.
//
// The frame stages the 512 KiB IDAT prefix with `png.open` under the normal
// class, which fits the normal budget. Only the row loop is promoted to the
// realtime class (heartbeat immediately and once per row), because a
// full-resolution corpus pass 1 is more work than the normal 10 s CPU budget
// allows. The promotion is one-way and the task stays realtime while it pumps
// events; admission refusal (`Full`) keeps the normal class and is reported
// once.
function render_image(source: const string[0]): Status;
var
  st: Status;
  gw: uint;
  at: uint;
  boosted: bool;
  sy: uint;
  sx: uint;
  x: uint;
begin
  st := syslib.fill_rect(0, 0, content_w, content_h, white);
  if st <> Ok then
    return st;
  draw_strip(uint(0), oh, false);
  st := syslib.window_present(id);
  if st <> Ok then
    return st;

  if kind = KindPpm then
  begin
    st := ppm.open(source, ppm_dec);
    if st <> Ok then
      return st;
    // A full-corpus row-seek preview still spends more than the normal 10 s
    // CPU budget under TCG, so the PPM row loop is promoted the same way as
    // PNG and heartbeats once per output row. Admission refusal falls back to
    // the normal class and is reported once.
    boosted := false;
    st := core.realtime_on();
    if st = Ok then
    begin
      boosted := true;
      core.heartbeat();
    end
    else if not boost_warned then
    begin
      boost_warned := true;
      out.say("image_viewer: realtime boost unavailable\n");
    end;
    y := uint(0);
    while y < oh do
    begin
      if boosted then
        core.heartbeat();
      sy := (y * sh) div oh;
      st := ppm.read_row(ppm_dec, sy, src_row, gw);
      if st <> Ok then
        return st;
      if gw <> sw then
        return Invalid;
      preview_row := "";
      x := uint(0);
      while x < ow do
      begin
        sx := (x * sw) div ow;
        st := core.copy_bytes(
          preview_row, uint(3) * x, src_row, uint(3) * sx, uint(3));
        if st <> Ok then
          return st;
        x := x + uint(1);
      end;
      st := syslib.blit_rgb(
        0, int(y), int(ow), 1, preview_row, int(ow), 1);
      if st <> Ok then
        return st;
      y := y + uint(1);
      if y mod uint(8) = uint(0) then
      begin
        draw_strip(y, oh, false);
        st := syslib.window_present(id);
        if st <> Ok then
          return st;
      end;
    end;
    draw_strip(oh, oh, true);
    return syslib.window_present(id);
  end;

  st := png.open(source, dec);
  if st <> Ok then
    return st;

  boosted := false;
  st := core.realtime_on();
  if st = Ok then
  begin
    boosted := true;
    core.heartbeat();
  end
  else if not boost_warned then
  begin
    boost_warned := true;
    out.say("image_viewer: realtime boost unavailable\n");
  end;

  y := uint(0);
  while y < oh do
  begin
    if boosted then
      core.heartbeat();
    st := png.next_row(dec, row, gw, at);
    if st <> Ok then
      return st;
    st := syslib.blit_rgb(0, int(at), int(ow), 1, row, int(ow), 1);
    if st <> Ok then
      return st;
    y := y + uint(1);
    if y mod uint(8) = uint(0) then
    begin
      draw_strip(y, oh, false);
      st := syslib.window_present(id);
      if st <> Ok then
        return st;
    end;
  end;
  draw_strip(oh, oh, true);
  return syslib.window_present(id);
end;

// ---- error window ---------------------------------------------------------

procedure build_error_message(st: Status);
begin
  message := "Cannot open image (status ";
  got := core.format_int_into(digits, int(st), uint(10), uint(0));
  append_message(digits);
  append_message(")");
end;

procedure draw_error();
begin
  status := syslib.fill_rect(0, 0, ErrorW, ErrorH, white);
  status := syslib.draw_rect(0, 0, ErrorW, ErrorH, black);
  status := stdlib.ui_message(8, 8, ErrorW - 16, ErrorH - 16, message);
  status := syslib.window_present(id);
end;

// A small white panel with the failure status; it waits for its close box or
// Escape in normal and shot mode. A screen too small for the panel prints the
// failure instead.
procedure show_error(st: Status);
var
  ex: int;
  ey: int;
  keep: bool;
begin
  build_error_message(st);
  if screen_w < uint(ErrorW + 2) or screen_h < uint(ErrorH + 17) then
  begin
    out.say("image_viewer: ");
    out.say(message);
    out.say("\n");
    return;
  end;
  ex := (int(screen_w) - (ErrorW + 2)) div 2;
  ey := (int(screen_h) - (ErrorH + 17)) div 2;
  if ex < 0 then
    ex := 0;
  if ey < 0 then
    ey := 0;
  status := syslib.window_open(ex, ey, ErrorW, ErrorH, "Image Viewer", id);
  if status <> Ok then
  begin
    id := 0;
    out.say("image_viewer: ");
    out.say(message);
    out.say("\n");
    return;
  end;
  draw_error();
  keep := true;
  while keep do
  begin
    status := syslib.next_event(ev);
    if status = Ok then
    begin
      if ev.kind = Close then
      begin
        if ev.window = id then
          keep := false;
      end
      else if ev.kind = KeyDown then
      begin
        if ev.window = id and stdlib.ui_key_code(ev.code) = EscKey then
          keep := false;
      end;
    end;
  end;
  status := syslib.window_close(id);
  id := 0;
end;

// ---- viewing --------------------------------------------------------------

// Probe, open a content-sized window, render and pump. `announce` prints the
// one `ready` line after the first complete frame and is used by `shot`.
procedure show_image(source: const string[0]; announce: bool);
var
  st: Status;
  cx: int;
  cy: int;
  keep: bool;
begin
  make_title(source);
  st := probe_image(source);
  if st <> Ok then
  begin
    show_error(st);
    return;
  end;
  content_w := int(ow);
  content_h := int(oh) + StatusStripH;
  if content_w < MinContent then
    content_w := MinContent;
  if screen_w < uint(content_w + 2) or screen_h < uint(content_h + 17) then
  begin
    show_error(Invalid);
    return;
  end;
  cx := (int(screen_w) - (content_w + 2)) div 2;
  cy := (int(screen_h) - (content_h + 17)) div 2;
  if cx < 0 then
    cx := 0;
  if cy < 0 then
    cy := 0;
  status := syslib.window_open(cx, cy, content_w, content_h, title, id);
  if status <> Ok then
  begin
    id := 0;
    show_error(status);
    return;
  end;

  st := render_image(source);
  if st <> Ok then
  begin
    status := syslib.window_close(id);
    id := 0;
    show_error(st);
    return;
  end;
  if announce then
  begin
    out.say("image_viewer: ready ");
    say_uint(sw);
    out.say("x");
    say_uint(sh);
    out.say(" -> ");
    say_uint(ow);
    out.say("x");
    say_uint(oh);
    out.say("\n");
  end;

  // The image canvas is never invalidated: the service recomposites it on
  // every move, raise, restore and mode change (spec 0026 C10/C14/C16), so an
  // `Expose` needs no redraw and no decode. Only Close and Escape end the
  // window.
  keep := true;
  while keep do
  begin
    status := syslib.next_event(ev);
    if status = Ok then
    begin
      if ev.kind = Close then
      begin
        if ev.window = id then
          keep := false;
      end
      else if ev.kind = KeyDown then
      begin
        if ev.window = id and stdlib.ui_key_code(ev.code) = EscKey then
          keep := false;
      end;
    end;
  end;
  status := syslib.window_close(id);
  id := 0;
end;

// ---- deterministic check --------------------------------------------------

function pixel_is(buf: const bytes[RowCapacity]; x: uint;
                 red: int; green: int; blue: int): bool;
begin
  return int(buf[uint(3) * x]) = red
    and int(buf[uint(3) * x + uint(1)]) = green
    and int(buf[uint(3) * x + uint(2)]) = blue;
end;

// Probe, open and decode the committed 16x16 Adam7 fixture. Pass 1 is 2x2:
// (0,0) black, (1,0) (128,0,64), (0,1) (0,128,32), (1,1) (128,128,96). The
// two rows are also staged into `preview` for the window blit.
function check_fixture(): bool;
var
  st: Status;
  gw: uint;
  row_y: uint;
begin
  check_where := "probe";
  st := png.probe("system/png_grad_il.png", sw, sh, ow, oh, ct, il);
  if st <> Ok then
    return false;
  check_where := "geometry";
  if sw <> uint(16) or sh <> uint(16) then
    return false;
  if ow <> uint(2) or oh <> uint(2) or il <> uint(1) then
    return false;
  check_where := "open";
  st := png.open("system/png_grad_il.png", dec);
  if st <> Ok then
    return false;
  check_where := "row0";
  st := png.next_row(dec, row, gw, row_y);
  if st <> Ok or gw <> uint(2) or row_y <> uint(0) then
    return false;
  if not pixel_is(row, uint(0), 0, 0, 0) then
    return false;
  if not pixel_is(row, uint(1), 128, 0, 64) then
    return false;
  check_where := "copy0";
  st := core.copy_bytes(preview, uint(0), row, uint(0), uint(6));
  if st <> Ok then
    return false;
  check_where := "row1";
  st := png.next_row(dec, row, gw, row_y);
  if st <> Ok or gw <> uint(2) or row_y <> uint(1) then
    return false;
  if not pixel_is(row, uint(0), 0, 128, 32) then
    return false;
  if not pixel_is(row, uint(1), 128, 128, 96) then
    return false;
  check_where := "copy1";
  return core.copy_bytes(preview, uint(6), row, uint(0), uint(6)) = Ok;
end;

// Probe, open and read every row of the committed 8x8 P6 fixture. Its pixel
// formula is (x*29%256, y*37%256, (x*11+y*7)%256). This is independent of the
// test-only `ppm_probe`.
function check_ppm_fixture(): bool;
var
  st: Status;
  w: uint;
  h: uint;
  gw2: uint;
  y2: uint;
  x2: uint;
  er: int;
  eg: int;
  eb: int;
begin
  check_where := "ppm probe";
  st := ppm.probe("system/ppm_rgb.ppm", w, h);
  if st <> Ok or w <> uint(8) or h <> uint(8) then
    return false;
  check_where := "ppm open";
  st := ppm.open("system/ppm_rgb.ppm", ppm_dec);
  if st <> Ok then
    return false;
  y2 := uint(0);
  while y2 < uint(8) do
  begin
    check_where := "ppm row";
    st := ppm.read_row(ppm_dec, y2, src_row, gw2);
    if st <> Ok or gw2 <> uint(8) then
      return false;
    x2 := uint(0);
    while x2 < uint(8) do
    begin
      er := int((x2 * uint(29)) mod uint(256));
      eg := int((y2 * uint(37)) mod uint(256));
      eb := int((x2 * uint(11) + y2 * uint(7)) mod uint(256));
      if not pixel_is(src_row, x2, er, eg, eb) then
        return false;
      x2 := x2 + uint(1);
    end;
    y2 := y2 + uint(1);
  end;
  return true;
end;

// Kernel-only proof of the blit path: an 8x8 window receives the 2x2 preview
// 1:1 and `get_pixel` reads the same two pixels back. The window is always
// closed, on success or failure.
function check_window(): bool;
var
  st: Status;
  p0: uint;
  p1: uint;
  ok: bool;
begin
  ok := false;
  check_where := "window open";
  st := syslib.window_open(64, 64, MinContent, MinContent, "Image Viewer Check", id);
  if st <> Ok then
  begin
    id := 0;
    return false;
  end;
  check_where := "fill";
  st := syslib.fill_rect(0, 0, MinContent, MinContent, white);
  if st = Ok then
  begin
    check_where := "blit";
    st := syslib.blit_rgb(0, 0, 2, 2, preview, 2, 2);
    if st = Ok then
    begin
      check_where := "pixels";
      p0 := syslib.get_pixel(0, 0);
      p1 := syslib.get_pixel(1, 0);
      ok := p0 = uint(0) and p1 = uint(0x800040);
    end;
  end;
  status := syslib.window_close(id);
  id := 0;
  return ok;
end;

procedure check_mode();
begin
  if not check_fixture() then
  begin
    out.say("image_viewer: check failed (");
    out.say(check_where);
    out.say(")\n");
    return;
  end;
  if not check_ppm_fixture() then
  begin
    out.say("image_viewer: check failed (");
    out.say(check_where);
    out.say(")\n");
    return;
  end;
  if syslib.screen_width() = uint(0) then
  begin
    out.say("image_viewer: host ok\n");
    return;
  end;
  if not check_window() then
  begin
    out.say("image_viewer: check failed (");
    out.say(check_where);
    out.say(")\n");
    return;
  end;
  out.say("image_viewer: ok\n");
end;

// ---- entry point ----------------------------------------------------------

procedure main(args: const string[1024]);
begin
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  gray := stdlib.ui_gray();
  id := 0;
  running := true;
  kind := KindPng;
  boost_warned := false;
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  parse_args(args);

  if mode = ModeCheck then
  begin
    check_mode();
    return;
  end;

  if screen_w = uint(0) then
  begin
    out.say("image_viewer: no display\n");
    return;
  end;

  // A shot without a path uses the small committed fixture; a normal launch
  // without a path asks the file dialog.
  if mode = ModeShot and int(core.len(path)) = 0 then
    path := "system/png_grad_il.png";

  if int(core.len(path)) = 0 then
  begin
    status := file_dialog.open_file("image", path);
    if status = Ok then
      show_image(path, false);
    return;
  end;

  show_image(path, mode = ModeShot);
end;

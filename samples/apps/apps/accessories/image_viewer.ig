unit image_viewer;

// PNG image viewer (spec 0023; ADR 0057). A client of the window service over
// the row-streaming `lib.png` decoder: the application state never holds a
// full-image RGB buffer, only the decoder (611,352 bytes), one 16,384-byte
// caller row and one 12-byte 2x2 check preview.
//
//   run image_viewer                 modal file dialog, then view the choice
//   run image_viewer <path>          view one PNG
//   run image_viewer <path> shot     view, print `image_viewer: ready WxH -> WxH`
//                                    and keep pumping events for the QMP capture
//   run image_viewer check           deterministic headless check; prints
//                                    `image_viewer: ok` (or `image_viewer: host ok`)
//
// The viewer draws the interlace pass-1 preview 1:1 into a window whose
// content is exactly the decoded geometry `out_w x out_h` (at least 8x8). It
// does not resample: the corpus is well under every runtime display mode, so a
// pass-1 pixel is a screen pixel. A frame that cannot fit, a probe failure or
// a decode failure opens a small error window instead of faulting.
//
// The first frame is drawn and presented *before* `png.open`: opening stages
// up to 512 KiB of IDAT payload, which takes visible time on the real corpus.
// An owner `Expose` re-reads the content rectangle (W12/W28, ADR 0046) and
// re-runs the whole decode; pass 1 is cheap and there is no image buffer to
// keep. Close/Escape leaves. Each decoded row reaches the window through one
// `syslib.blit_rgb` call with `pw = ow, ph = 1`.
//
// `check` is independent of the test-only `png_probe`: it probes and decodes
// `system/png_grad_il.png` itself, asserts the four Adam7 pass-1 pixels, and
// (with graphics) blits the 2x2 preview into an 8x8 window and reads the first
// two pixels back with `get_pixel`. `langhost` has no screen and stops after
// the decode assertions with `image_viewer: host ok`.

uses core, stdlib, syslib, lib.out, lib.file_dialog, lib.png;

const
  ModeNormal = 0;
  ModeShot = 1;
  ModeCheck = 2;

  PathCapacity = 256;
  TitleCapacity = 64;
  RowCapacity = 16384;

  // A window content rectangle is at least 8x8 (spec 0011, W9).
  MinContent = 8;

  // Error panel content, centred like the image window.
  ErrorW = 400;
  ErrorH = 120;

  EscKey = 0x100;

var
  black: uint;
  white: uint;
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
  rect: Rect;

  // Decoder state: one decoder, one caller row and the 2x2 check preview.
  dec: png.Decoder;
  row: bytes[RowCapacity];
  preview: bytes[12];
  got_w: uint;
  ry: uint;
  y: uint;

  running: bool;
  error_status: Status;
  check_where: string[64];
  // True once `png.open` has staged the IDAT prefix; an Expose then uses
  // `png.rewind` instead of touching the file again.
  decoded: bool;
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

// ---- drawing --------------------------------------------------------------

// One complete image frame: one blitted row per decoded row. On the first
// frame the content is also filled white and `Decoding...` is presented before
// the staged-IDAT work; an `Expose` repaint deliberately skips that blanking
// so the previous image stays visible while `png.rewind` and the row loop run
// (the service queues an initial expose for the new window). Returns the first
// failing status so the caller can open the error window.
//
// The first frame stages the 512 KiB IDAT prefix with `png.open` under the
// normal class, which fits the normal budget. Only the row loop is promoted to
// the realtime class (heartbeat immediately and once per row), because a
// full-resolution corpus pass 1 is more work than the normal 10 s CPU budget
// allows. An `Expose` repaint calls `png.rewind` over the already-staged prefix
// (no `fs_read`), then runs the same boosted row loop. The promotion is
// one-way and the task stays realtime while it pumps events; admission
// refusal (`Full`) keeps the normal class and is reported once.
function render_image(source: const string[0]): Status;
var
  st: Status;
  gw: uint;
  at: uint;
  boosted: bool;
begin
  st := syslib.window_content_rect(id, rect);
  if st <> Ok then
    return st;
  if not decoded then
  begin
    st := syslib.fill_rect(rect.x, rect.y, content_w, content_h, white);
    if st <> Ok then
      return st;
    st := syslib.draw_text(rect.x + 8, rect.y + 8, "Decoding...", black);
    if st <> Ok then
      return st;
    st := syslib.window_present(id);
    if st <> Ok then
      return st;
  end;

  if decoded then
    st := png.rewind(dec)
  else
  begin
    st := png.open(source, dec);
    if st = Ok then
      decoded := true;
  end;
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
    st := syslib.blit_rgb(rect.x, rect.y + int(at), int(ow), 1, row, int(ow), 1);
    if st <> Ok then
      return st;
    y := y + uint(1);
  end;
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
var
  r: Rect;
begin
  status := syslib.window_content_rect(id, r);
  if status <> Ok then
    return;
  status := syslib.fill_rect(r.x, r.y, ErrorW, ErrorH, white);
  status := syslib.draw_rect(r.x, r.y, ErrorW, ErrorH, black);
  status := stdlib.ui_message(r.x + 8, r.y + 8, ErrorW - 16, ErrorH - 16, message);
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
      if ev.kind = Expose then
      begin
        if ev.window = id then
          draw_error();
      end
      else if ev.kind = Close then
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
  st := png.probe(source, sw, sh, ow, oh, ct, il);
  if st <> Ok then
  begin
    show_error(st);
    return;
  end;
  content_w := int(ow);
  content_h := int(oh);
  if content_w < MinContent then
    content_w := MinContent;
  if content_h < MinContent then
    content_h := MinContent;
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

  error_status := Ok;
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

  keep := true;
  while keep do
  begin
    status := syslib.next_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose then
      begin
        if ev.window = id then
        begin
          st := render_image(source);
          if st <> Ok then
          begin
            error_status := st;
            keep := false;
          end;
        end;
      end
      else if ev.kind = Close then
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
  if error_status <> Ok then
    show_error(error_status);
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

// Kernel-only proof of the blit path: an 8x8 window receives the 2x2 preview
// 1:1 and `get_pixel` reads the same two pixels back. The window is always
// closed, on success or failure.
function check_window(): bool;
var
  st: Status;
  r: Rect;
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
  check_where := "content rect";
  st := syslib.window_content_rect(id, r);
  if st = Ok then
  begin
    check_where := "fill";
    st := syslib.fill_rect(r.x, r.y, MinContent, MinContent, white);
    if st = Ok then
    begin
      check_where := "blit";
      st := syslib.blit_rgb(r.x, r.y, 2, 2, preview, 2, 2);
      if st = Ok then
      begin
        check_where := "pixels";
        p0 := syslib.get_pixel(r.x, r.y);
        p1 := syslib.get_pixel(r.x + 1, r.y);
        ok := p0 = uint(0) and p1 = uint(0x800040);
      end;
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
  id := 0;
  error_status := Ok;
  running := true;
  decoded := false;
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

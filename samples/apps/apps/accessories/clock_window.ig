unit clock_window;

// M2.5.2 clock: a windowed analog face and digital readout, ticking from the
// monotonic `core.clock()`.
//
//   run clock_window          // live window; stays until its close box
//   run clock_window check    // deterministic check at 10:08:30, prints `clock_window: ok`
//   run clock_window shot     // live clock, holds ~5 s and prints
//                             // `clock_window: ready` for the QMP harness
//
// The live window promotes to realtime so its once-per-second redraw loop is
// exempt from the normal CPU/step budget; `core.heartbeat()` feeds the realtime
// watchdog. When the realtime slot is unavailable (a single-core boot where
// the desktop shell already holds it) it falls back to blocking on window
// events and repaints the current time whenever it is exposed or receives an
// event. Every redraw paints the whole face and presents once with
// `window_present`, so a presented frame is never half drawn. An owner
// `Expose` repaints the whole content.
//
// Geometry is integer-only: a midpoint circle for the rim, a 60-entry
// fixed-point direction table (built once by rotating 6 degrees per step) for
// the minute ticks and the hands, and Bresenham 1x1 `fill_rect` lines. There
// is no floating point, no square root and no per-pixel host call beyond the
// 1x1 rectangles.
//
// `langhost` and a kernel without a framebuffer model a disabled screen, so
// the sample checks the pure time decomposition, the formatting and the
// direction table there and prints `clock_window: host ok`.

uses core, stdlib, syslib, lib.out;

const
  WindowX = 340;
  WindowY = 130;
  ContentW = 260;
  ContentH = 280;

  // Client bezel: two one-pixel frames with a two-pixel gap.
  BezelOuter = 4;
  BezelInner = 7;

  // Analog face: centre at (rect.x + ContentW / 2, rect.y + FaceCenterDy).
  FaceCenterDy = 128;
  FaceRadius = 96;
  HourHandLen = 52;
  MinuteHandLen = 75;
  SecondHandLen = 88;

  // Digital readout: a framed 80x24 panel with `HH:MM:SS` in four cells per
  // byte (8 px each) starting at (cx - 32, rect.y + DigitalY).
  DigitalY = 240;
  DigitalW = 80;
  DigitalH = 24;
  DigitalCells = 8;

  // Caption under the readout.
  CaptionDy = 262;

  // Fixed-point direction table: `DirScale` is 1.0, and `Cos6`/`Sin6` are
  // cos(6 deg) and sin(6 deg) scaled by `DirScale` (rounded).
  DirScale = 16384;
  Cos6 = 16294;
  Sin6 = 1713;

  FixedHour = 10;
  FixedMinute = 8;
  FixedSecond = 30;

  LiveSeconds = 5;
  NanosPerSecond = 1000000000;

var
  failed: bool;
  step: int;
  step_text: string[16];
  status: Status;
  ignored: Status;
  written: int;
  screen_w: uint;
  screen_h: uint;
  id: int;
  rect: Rect;
  black: uint;
  white: uint;

  // Unit directions, one per 6 degrees clockwise from 12 o'clock.
  dir_x: array[int, 60];
  dir_y: array[int, 60];

  // Absolute screen geometry, computed from the content rectangle.
  cx: int;
  cy: int;
  dx0: int;
  dy0: int;

  hh: string[2];
  mm: string[2];
  ss: string[2];
  hour: int;
  minute: int;
  second: int;
  now_s: int;
  last_s: int;
  end_s: int;

  px: int;
  py: int;
  qx: int;
  qy: int;

  ev: Event;
  draining: bool;
  quit: bool;

// Silent check: the first failure is printed with its step number.
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed then
  begin
    failed := true;
    out.say("clock_window: failed check ");
    written := core.format_int_into(step_text, step, uint(10), uint(0));
    out.say(step_text);
    out.say(" (");
    out.say(label);
    out.say(")\n");
  end;
end;

procedure format_two(value: int; dst: var string[2]);
begin
  written := core.format_int_into(dst, value, uint(10), uint(2));
end;

// 24-hour clock decomposition of a whole number of seconds since boot.
procedure split(total: int; out_hour: var int; out_minute: var int; out_second: var int);
begin
  out_hour := (total div 3600) mod 24;
  out_minute := (total div 60) mod 60;
  out_second := total mod 60;
end;

// Rounds `value / scale` to nearest (away from zero on a tie), which keeps the
// fixed-point rotation table symmetric.
function div_round(value: int; scale: int): int;
begin
  if value >= 0 then
    return (value + scale div 2) div scale;
  return -((-value + scale div 2) div scale);
end;

// One direction per 6 degrees, clockwise from straight up, in math
// coordinates (x right, y up). Screen y is `cy - dy * len / DirScale`.
procedure build_directions();
var
  index: int;
  x: int;
  y: int;
  nx: int;
  ny: int;
begin
  x := 0;
  y := DirScale;
  index := 0;
  while index < 60 do
  begin
    dir_x[index] := x;
    dir_y[index] := y;
    nx := div_round(x * Cos6 + y * Sin6, DirScale);
    ny := div_round(y * Cos6 - x * Sin6, DirScale);
    x := nx;
    y := ny;
    index := index + 1;
  end;
end;

// One point on the rim at direction `index`, at `radius` pixels from centre.
procedure point_at(index: int; radius: int; out_x: var int; out_y: var int);
begin
  out_x := cx + (dir_x[index] * radius) div DirScale;
  out_y := cy - (dir_y[index] * radius) div DirScale;
end;

// Linear interpolation between two adjacent 6-degree directions.
procedure dir_interp(index: int; num: int; den: int; out_x: var int; out_y: var int);
var
  next: int;
  nx: int;
  ny: int;
begin
  next := (index + 1) mod 60;
  nx := dir_x[next];
  ny := dir_y[next];
  out_x := dir_x[index] + ((nx - dir_x[index]) * num) div den;
  out_y := dir_y[index] + ((ny - dir_y[index]) * num) div den;
end;

procedure second_dir(value: int; out_x: var int; out_y: var int);
begin
  dir_interp(value mod 60, 0, 1, out_x, out_y);
end;

procedure minute_dir(value_minute: int; value_second: int; out_x: var int; out_y: var int);
begin
  dir_interp(value_minute mod 60, value_second, 60, out_x, out_y);
end;

// The hour hand advances 0.5 degrees per minute, i.e. one 6-degree step every
// 12 minutes, interpolated between the table entries.
procedure hour_dir(value_hour: int; value_minute: int; value_second: int;
                   out_x: var int; out_y: var int);
var
  index: int;
  num: int;
begin
  index := ((value_hour mod 12) * 5 + value_minute div 12) mod 60;
  num := (value_minute mod 12) * 60 + value_second;
  dir_interp(index, num, 720, out_x, out_y);
end;

// One pixel through the drawing primitive. The result is intentionally
// ignored: every coordinate here is inside the window content.
procedure put(x: int; y: int; color: uint);
begin
  ignored := syslib.fill_rect(x, y, 1, 1, color);
end;

// Integer Bresenham line, one 1x1 `fill_rect` per point.
procedure line(ax: int; ay: int; bx: int; by: int; color: uint);
var
  dx: int;
  sx: int;
  dy: int;
  sy: int;
  err: int;
  e2: int;
  done: bool;
begin
  if bx >= ax then
  begin
    dx := bx - ax;
    sx := 1;
  end
  else
  begin
    dx := ax - bx;
    sx := -1;
  end;
  if by >= ay then
  begin
    dy := by - ay;
    sy := 1;
  end
  else
  begin
    dy := ay - by;
    sy := -1;
  end;
  err := dx - dy;
  done := false;
  while not done do
  begin
    put(ax, ay, color);
    if ax = bx and ay = by then
      done := true
    else
    begin
      e2 := 2 * err;
      if e2 > -dy then
      begin
        err := err - dy;
        ax := ax + sx;
      end;
      if e2 < dx then
      begin
        err := err + dx;
        ay := ay + sy;
      end;
    end;
  end;
end;

// Midpoint circle, one pixel per plotted point.
procedure draw_circle(radius: int; color: uint);
var
  x: int;
  y: int;
  err: int;
begin
  x := radius;
  y := 0;
  err := 1 - radius;
  while x >= y do
  begin
    put(cx + x, cy + y, color);
    put(cx + y, cy + x, color);
    put(cx - y, cy + x, color);
    put(cx - x, cy + y, color);
    put(cx - x, cy - y, color);
    put(cx - y, cy - x, color);
    put(cx + y, cy - x, color);
    put(cx + x, cy - y, color);
    y := y + 1;
    if err < 0 then
      err := err + 2 * y + 1
    else
    begin
      x := x - 1;
      err := err + 2 * (y - x) + 1;
    end;
  end;
end;

// 60 minute ticks; every fifth is a longer hour mark.
procedure draw_ticks();
var
  index: int;
begin
  index := 0;
  while index < 60 do
  begin
    if index mod 5 = 0 then
    begin
      point_at(index, FaceRadius - 12, px, py);
      point_at(index, FaceRadius - 2, qx, qy);
      line(px, py, qx, qy, black);
    end
    else
    begin
      point_at(index, FaceRadius - 5, px, py);
      point_at(index, FaceRadius - 1, qx, qy);
      line(px, py, qx, qy, black);
    end;
    index := index + 1;
  end;
end;

procedure draw_hands(value_hour: int; value_minute: int; value_second: int);
begin
  hour_dir(value_hour, value_minute, value_second, px, py);
  qx := cx + (px * HourHandLen) div DirScale;
  qy := cy - (py * HourHandLen) div DirScale;
  line(cx, cy, qx, qy, black);

  minute_dir(value_minute, value_second, px, py);
  qx := cx + (px * MinuteHandLen) div DirScale;
  qy := cy - (py * MinuteHandLen) div DirScale;
  line(cx, cy, qx, qy, black);

  second_dir(value_second, px, py);
  qx := cx + (px * SecondHandLen) div DirScale;
  qy := cy - (py * SecondHandLen) div DirScale;
  line(cx, cy, qx, qy, black);

  ignored := syslib.fill_rect(cx - 2, cy - 2, 5, 5, black);
end;

procedure draw_digital(value_hour: int; value_minute: int; value_second: int);
begin
  format_two(value_hour, hh);
  format_two(value_minute, mm);
  format_two(value_second, ss);
  ignored := syslib.draw_text(dx0, dy0, hh, black);
  ignored := syslib.draw_text(dx0 + 16, dy0, ":", black);
  ignored := syslib.draw_text(dx0 + 24, dy0, mm, black);
  ignored := syslib.draw_text(dx0 + 40, dy0, ":", black);
  ignored := syslib.draw_text(dx0 + 48, dy0, ss, black);
  ignored := syslib.draw_text(dx0 + 16, rect.y + CaptionDy, "IGIS", black);
end;

// One complete frame: content base, bezel, face, ticks, readout, hands. The
// caller presents once afterwards, so no partial frame is ever shown.
procedure draw_clock(value_hour: int; value_minute: int; value_second: int);
begin
  ignored := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, white);
  ignored := stdlib.ui_frame(rect.x + BezelOuter, rect.y + BezelOuter,
                             ContentW - 2 * BezelOuter, ContentH - 2 * BezelOuter);
  ignored := stdlib.ui_frame(rect.x + BezelInner, rect.y + BezelInner,
                             ContentW - 2 * BezelInner, ContentH - 2 * BezelInner);
  draw_circle(FaceRadius, black);
  draw_ticks();
  ignored := stdlib.ui_frame(dx0 - 8, dy0 - 8, DigitalW, DigitalH);
  draw_digital(value_hour, value_minute, value_second);
  draw_hands(value_hour, value_minute, value_second);
end;

procedure present_clock();
begin
  ignored := syslib.window_present(id);
end;

// Re-reads the content rectangle and recomputes the absolute face/readout
// geometry after the service moved the window (spec 0011, W12/W17/W28;
// ADR 0046).
procedure refresh_geometry();
begin
  status := syslib.window_content_rect(id, rect);
  if status = Ok then
  begin
    cx := rect.x + ContentW div 2;
    cy := rect.y + FaceCenterDy;
    dx0 := cx - DigitalCells * 4;
    dy0 := rect.y + DigitalY;
  end;
end;

procedure check_pixels();
begin
  // Client bezel: two frames with a white gap.
  verify("bezel outline", syslib.get_pixel(rect.x + BezelOuter, rect.y + BezelOuter) = black);
  verify("bezel gap", syslib.get_pixel(rect.x + BezelOuter + 1, rect.y + BezelOuter + 1) = white);
  verify("bezel inner", syslib.get_pixel(rect.x + BezelInner, rect.y + BezelInner) = black);
  verify("content base", syslib.get_pixel(rect.x + 10, rect.y + 10) = white);

  // Rim at the four cardinal points (the hour marks stop short of it).
  verify("circle 12", syslib.get_pixel(cx, cy - FaceRadius) = black);
  verify("circle 3", syslib.get_pixel(cx + FaceRadius, cy) = black);
  verify("circle 6", syslib.get_pixel(cx, cy + FaceRadius) = black);
  verify("circle 9", syslib.get_pixel(cx - FaceRadius, cy) = black);

  // Hour and minute marks.
  verify("hour mark 12", syslib.get_pixel(cx, cy - 88) = black);
  verify("hour mark 3", syslib.get_pixel(cx + 88, cy) = black);
  verify("hour mark 9", syslib.get_pixel(cx - 88, cy) = black);
  verify("minute tick", syslib.get_pixel(cx + 9, cy - 92) = black);

  // Hands at 10:08:30: hour up-left, minute up-right, second straight down.
  verify("hour hand", syslib.get_pixel(cx - 42, cy - 29) = black);
  verify("minute hand", syslib.get_pixel(cx + 58, cy - 47) = black);
  verify("second hand", syslib.get_pixel(cx, cy + 60) = black);
  verify("centre dot", syslib.get_pixel(cx, cy) = black);

  // Two documented empty face points (lower left/right, away from the hands).
  verify("face empty right", syslib.get_pixel(cx + 40, cy + 40) = white);
  verify("face empty left", syslib.get_pixel(cx - 40, cy + 40) = white);

  // Digital readout "10:08:30": one stroke, one hole and one colon dot per
  // tested glyph, using the documented 8x8 font rows.
  verify("digital 1", syslib.get_pixel(dx0, dy0 + 6) = black);
  verify("digital 0 top", syslib.get_pixel(dx0 + 10, dy0) = black);
  verify("digital 0 hole", syslib.get_pixel(dx0 + 10, dy0 + 3) = white);
  verify("digital colon", syslib.get_pixel(dx0 + 18, dy0 + 1) = black);
  verify("digital colon gap", syslib.get_pixel(dx0 + 18, dy0 + 4) = white);
  verify("digital minutes 0", syslib.get_pixel(dx0 + 26, dy0) = black);
  verify("digital 8 stroke", syslib.get_pixel(dx0 + 32, dy0 + 2) = black);
  verify("digital 8 hole", syslib.get_pixel(dx0 + 34, dy0 + 2) = white);
  verify("digital second colon", syslib.get_pixel(dx0 + 42, dy0 + 5) = black);
  verify("digital 3", syslib.get_pixel(dx0 + 50, dy0 + 6) = black);
  verify("digital last 0", syslib.get_pixel(dx0 + 57, dy0) = black);
  verify("digital gap", syslib.get_pixel(dx0 + 7, dy0) = white);
  verify("readout frame", syslib.get_pixel(dx0 - 8, dy0 - 8) = black);
  verify("readout interior", syslib.get_pixel(dx0 - 6, dy0 - 6) = white);
  verify("caption", syslib.get_pixel(dx0 + 18, dy0 + 22) = black);
end;

procedure check_mode();
begin
  draw_clock(FixedHour, FixedMinute, FixedSecond);
  present_clock();
  check_pixels();
  ignored := syslib.window_close(id);
  if failed then
    out.say("clock_window: failed\n")
  else
    out.say("clock_window: ok\n");
end;

// Drain the machine-wide queue without blocking. A whole-content `Expose` for
// this window repaints from the current clock; a close request ends the live
// loop so the app closes its own window and exits (spec 0011, W23).
procedure drain_events();
begin
  draining := true;
  while draining do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose and ev.window = id then
      begin
        refresh_geometry();
        now_s := int(core.clock() div uint(NanosPerSecond));
        split(now_s, hour, minute, second);
        draw_clock(hour, minute, second);
        present_clock();
      end
      else if ev.kind = Close and ev.window = id then
      begin
        quit := true;
        draining := false;
      end;
    end
    else
      draining := false;
  end;
end;

// Live mode: derive the time from the monotonic nanosecond clock, redraw once
// per second, and never block (the scheduler preempts the spin at the
// quantum). `shot` holds the last frame for the screenshot harness.
procedure shot_mode();
begin
  now_s := int(core.clock() div uint(NanosPerSecond));
  last_s := now_s;
  end_s := now_s + LiveSeconds;
  quit := false;
  split(now_s, hour, minute, second);
  draw_clock(hour, minute, second);
  present_clock();
  out.say("clock_window: ready\n");
  while now_s <= end_s and not quit do
  begin
    now_s := int(core.clock() div uint(NanosPerSecond));
    if now_s <> last_s then
    begin
      last_s := now_s;
      split(now_s, hour, minute, second);
      draw_clock(hour, minute, second);
      present_clock();
    end;
    drain_events();
  end;
  ignored := syslib.window_close(id);
  if quit then
    out.say("clock_window: closed\n")
  else
    out.say("clock_window: done\n");
end;

// The normal window: a live clock until the close box. The once-per-second
// redraw loop promotes to realtime so it is not charged to the normal-task
// budget; the heartbeat keeps the realtime watchdog fed. A single-core boot
// where the desktop shell already holds the realtime slot falls back to a
// blocking event loop and redraws the current time on each event.
procedure live_mode();
begin
  status := core.realtime_on();
  quit := false;
  now_s := int(core.clock() div uint(NanosPerSecond));
  last_s := now_s;
  split(now_s, hour, minute, second);
  draw_clock(hour, minute, second);
  present_clock();
  out.say("clock_window: ready\n");
  if status = Ok then
  begin
    while not quit do
    begin
      core.heartbeat();
      now_s := int(core.clock() div uint(NanosPerSecond));
      if now_s <> last_s then
      begin
        last_s := now_s;
        split(now_s, hour, minute, second);
        draw_clock(hour, minute, second);
        present_clock();
      end;
      drain_events();
    end;
  end
  else
  begin
    while not quit do
    begin
      status := syslib.next_event(ev);
      if status = Ok then
      begin
        if ev.kind = Close and ev.window = id then
          quit := true
        else
        begin
          if ev.kind = Expose and ev.window = id then
            refresh_geometry();
          now_s := int(core.clock() div uint(NanosPerSecond));
          split(now_s, hour, minute, second);
          draw_clock(hour, minute, second);
          present_clock();
        end;
      end;
    end;
  end;
  ignored := syslib.window_close(id);
  out.say("clock_window: closed\n");
end;

// The disabled-screen path: time decomposition, formatting and the direction
// table are pure and must agree on every host.
procedure host_path();
begin
  split(36510, hour, minute, second);
  verify("split hour", hour = 10);
  verify("split minute", minute = 8);
  verify("split second", second = 30);

  split(0, hour, minute, second);
  verify("midnight", hour = 0 and minute = 0 and second = 0);

  split(86399, hour, minute, second);
  verify("last second", hour = 23 and minute = 59 and second = 59);

  split(90061, hour, minute, second);
  verify("wrap hour", hour = 1 and minute = 1 and second = 1);

  format_two(0, hh);
  verify("zero pad", hh = "00");
  format_two(8, hh);
  verify("single pad", hh = "08");
  format_two(59, hh);
  verify("no pad", hh = "59");
  format_two(23, hh);
  verify("hour pad", hh = "23");

  verify("direction up", dir_x[0] = 0 and dir_y[0] = DirScale);
  verify("direction right", dir_x[15] > 16000);
  verify("direction down", dir_y[30] < -16000);
  verify("direction left", dir_x[45] < -16000);

  second_dir(0, px, py);
  verify("second up", px > -300 and px < 300 and py > 16000);
  second_dir(15, px, py);
  verify("second right", px > 16000 and py > -300 and py < 300);
  second_dir(45, px, py);
  verify("second left", px < -16000 and py > -300 and py < 300);

  hour_dir(3, 0, 0, px, py);
  verify("hour three", px > 16000 and py > -300 and py < 300);
  minute_dir(30, 0, px, py);
  verify("minute six", px > -300 and px < 300 and py < -16000);

  if failed then
    out.say("clock_window: host failed\n")
  else
    out.say("clock_window: host ok\n");
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  step := 0;
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  build_directions();

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    host_path()
  else if screen_w < uint(WindowX + ContentW + 40) or screen_h < uint(WindowY + ContentH + 60) then
    out.say("clock_window: screen too small\n")
  else
  begin
    status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Clock", id);
    if status <> Ok then
      out.say("clock_window: no window\n")
    else
    begin
      refresh_geometry();
      if status <> Ok then
        out.say("clock_window: no content rect\n")
      else
      begin
        if stdlib.starts_with(args, "shot") then
          shot_mode()
        else if stdlib.starts_with(args, "check") then
          check_mode()
        else
          live_mode();
      end;
    end;
  end;
end;

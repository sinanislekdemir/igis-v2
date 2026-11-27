unit ui_layout;

// M2.4.0 proof: the stdlib UI layout layer draws a synthetic dialog and a
// control panel inside a real window's content, using only the client-side
// helpers, presents the window and checks shadow pixels with `syslib.get_pixel`.
//
//   run ui_layout
//
// The helpers never touch the window service's chrome: the service draws the
// outer frame and title bar of the window itself, and everything inside the
// content rectangle comes from `stdlib.ui_*`.
//
// `langhost` and a kernel without a framebuffer model a disabled screen: every
// drawing primitive returns `Invalid`. The sample then checks the metrics, the
// palette and that disabled-screen contract and prints `ui_layout: host ok`,
// which keeps the host test deterministic.

uses core, stdlib, syslib, lib.out;

const
  WindowX = 120;
  WindowY = 120;
  ContentW = 360;
  ContentH = 220;
  DialogW = 320;
  DialogH = 170;

var
  failed: bool;
  step: int;
  screen_w: uint;
  screen_h: uint;
  id: int;
  status: Status;
  rect: Rect;
  dx: int;
  dy: int;
  ux: int;
  uy: int;
  bx: int;
  by: int;
  black: uint;
  white: uint;
  gray: uint;
  accent: uint;
  digits: string[16];
  used: int;

// Silent check: the console is a window over the same shadow, so a failure is
// reported as soon as it happens but success stays quiet until the end.
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed then
  begin
    failed := true;
    out.say("ui_layout: failed check ");
    used := core.format_int_into(digits, step, uint(10), uint(0));
    out.say(digits);
    out.say(" (");
    out.say(label);
    out.say(")\n");
  end;
end;

// Metrics and palette are pure and behave the same on every host.
procedure check_metrics();
begin
  verify("empty width", stdlib.ui_text_width("") = 0);
  verify("ascii width", stdlib.ui_text_width("abc") = 24);
  verify("utf8 width", stdlib.ui_text_width("aé") = 16);
  verify("text height", stdlib.ui_text_height() = 8);
  verify("line height", stdlib.ui_line_height() = 16);
  verify("content inset", stdlib.ui_content_inset() = 8);
  verify("title bar height", stdlib.ui_title_bar_height() = 16);
  verify("close box size", stdlib.ui_close_box_size() = 11);
  verify("center x", stdlib.ui_text_center_x(0, 100, "abc") = 38);
  verify("center y", stdlib.ui_text_center_y(0, 16) = 4);
  verify("narrow center x", stdlib.ui_text_center_x(7, 8, "abc") = 7);
  verify("short center y", stdlib.ui_text_center_y(9, 8) = 9);
  verify("black", stdlib.ui_black() = uint(0x000000));
  verify("white", stdlib.ui_white() = uint(0x00FFFFFF));
  verify("gray", stdlib.ui_gray() = uint(0x00CCCCCC));
  verify("focus", stdlib.ui_focus() = uint(0x00FFFFFF));
  verify("accent", stdlib.ui_accent() = uint(0x000080));
end;

// Draw the synthetic dialog inside the content rectangle.
procedure draw_dialog(origin_x: int; origin_y: int);
var
  st: Status;
begin
  dx := origin_x;
  dy := origin_y;

  // The dialog body: a one-pixel frame, then the client-drawn title bar.
  st := stdlib.ui_frame(dx, dy, DialogW, DialogH);
  verify("frame ok", st = Ok);
  st := stdlib.ui_title_bar(dx, dy, DialogW, "UI Layout", true);
  verify("title bar ok", st = Ok);

  // One text row.
  st := syslib.draw_text(dx + 12, dy + 30, "metrics 8x8", black);
  verify("text ok", st = Ok);

  // A button-shaped control: a frame with centred text.
  bx := dx + 124;
  by := dy + 56;
  st := stdlib.ui_frame(bx, by, 72, 20);
  verify("button frame ok", st = Ok);
  st := stdlib.ui_center_text(bx, by, 72, 20, "OK", black);
  verify("button text ok", st = Ok);

  // A second, unfocused title bar standing alone.
  ux := dx;
  uy := dy + 92;
  st := stdlib.ui_frame(ux, uy, DialogW, 16);
  verify("inactive frame ok", st = Ok);
  st := stdlib.ui_title_bar(ux, uy, DialogW, "Inactive", false);
  verify("inactive bar ok", st = Ok);

  // A palette swatch from `ui_accent`.
  st := syslib.fill_rect(dx + 12, dy + 140, 64, 12, accent);
  verify("accent swatch ok", st = Ok);
end;

// Read the shadow back. The window service draws its own chrome outside the
// content rectangle; every pixel below is inside the content and was painted by
// the helper under test.
procedure check_pixels();
begin
  verify("content white", syslib.get_pixel(2, 2) = white);
  verify("dialog top border", syslib.get_pixel(dx, dy) = black);
  verify("dialog bottom border", syslib.get_pixel(dx + DialogW - 1, dy + DialogH - 1) = black);
  verify("focused stripe on", syslib.get_pixel(dx + 300, dy + 2) = black);
  verify("focused stripe off", syslib.get_pixel(dx + 300, dy + 3) = white);
  verify("title separator", syslib.get_pixel(dx + 300, dy + 15) = black);
  verify("close box outline", syslib.get_pixel(dx + 4, dy + 3) = black);
  verify("close box interior", syslib.get_pixel(dx + 9, dy + 8) = white);
  verify("inactive fill", syslib.get_pixel(ux + 2, uy + 2) = gray);
  verify("inactive no stripe", syslib.get_pixel(ux + 2, uy + 3) = gray);
  verify("inactive close box", syslib.get_pixel(ux + 4, uy + 3) = black);
  verify("button border", syslib.get_pixel(bx, by) = black);
  verify("button interior", syslib.get_pixel(bx + 2, by + 2) = white);
  verify("button glyph", syslib.get_pixel(bx + 30, by + 6) = black);
  verify("accent swatch", syslib.get_pixel(dx + 12, dy + 140) = accent);
end;

// The disabled-screen path: metrics and palette are still real values, while
// every drawing helper reports `Invalid` and draws nothing.
procedure host_path();
begin
  verify("frame disabled", stdlib.ui_frame(0, 0, 32, 32) = Invalid);
  verify("title bar disabled", stdlib.ui_title_bar(0, 0, 100, "host", true) = Invalid);
  verify("close box disabled", stdlib.ui_close_box(0, 0) = Invalid);
  verify("separator disabled", stdlib.ui_separator(0, 0, 32) = Invalid);
  verify(
    "center text disabled",
    stdlib.ui_center_text(0, 0, 100, 16, "host", stdlib.ui_black()) = Invalid);
  if failed then
    out.say("ui_layout: host failed\n")
  else
    out.say("ui_layout: host ok\n");
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  step := 0;
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  gray := stdlib.ui_gray();
  accent := stdlib.ui_accent();

  check_metrics();

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    host_path()
  else if screen_w < uint(WindowX + ContentW + 40) or screen_h < uint(WindowY + ContentH + 60) then
    out.say("ui_layout: screen too small\n")
  else
  begin
    status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "UI Layout", id);
    if status <> Ok then
      out.say("ui_layout: no window\n")
    else
    begin
      status := syslib.window_content_rect(id, rect);
      if status <> Ok then
        out.say("ui_layout: no content rect\n")
      else
      begin
        // Window-local (spec 0026 C8): the dialog is placed at (20, 20).
        status := syslib.fill_rect(0, 0, rect.w, rect.h, white);
        verify("content fill ok", status = Ok);
        draw_dialog(20, 20);
        status := syslib.window_present(id);
        verify("window present", status = Ok);
        check_pixels();
        status := syslib.window_close(id);
        verify("window close", status = Ok);
        if failed then
          out.say("ui_layout: failed\n")
        else
          out.say("ui_layout: ok\n");
      end;
    end;
  end;
end;

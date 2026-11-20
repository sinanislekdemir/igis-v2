unit windows_demo;

// M2.3.2 visual proof: service-drawn chrome, the dither desktop, damage and
// repair. Everything visible here comes from the shadow screen:
//
//   run windows_demo
//
//   1. Clear to the 50% dither desktop.
//   2. Open three overlapping windows with distinct titles and content
//      colours; the service draws the border, title bar, stripes (focused) and
//      close box itself, and fills the desktop under each new frame.
//   3. Draw each content once inside its own content rectangle, bottom-to-top.
//   4. Move Beta and raise Alpha. The service restores the desktop under the
//      old frame, redraws the chrome bottom-to-top and presents the damage
//      union; the app then repaints its contents and rotates the z-order so
//      every chrome is repainted in its visible parts (the M2.3.3 `Expose`
//      equivalent, done by hand).
//   5. Check chrome and content pixels and present the whole screen. Printing
//      a line scrolls the console, which shares the shadow, so the checks are
//      silent and the final composition is painted *after* the last line:
//      `windows_demo: ready`, then clear/draw/present once more, then hold for
//      ~15 s so a screenshot can be taken. `windows_demo: ok` follows the hold.
//
// The hold promotes the task to the realtime class (with heartbeats) because a
// normal task's 10 s / 200M-step budget would kill a fifteen-second spin. If
// admission is refused (every core already has a realtime task), it falls back
// to a short normal-class hold that still leaves the screen up for a moment.

uses core, syslib, lib.out;

const
  HoldNs = 15000000000;
  FallbackHoldNs = 1500000000;

var
  ok: bool;
  failed: bool;
  fail_step: int;
  step: int;
  status: Status;
  white: uint;
  black: uint;
  grey: uint;
  alpha_fill: uint;
  beta_fill: uint;
  gamma_fill: uint;
  screen_w: uint;
  screen_h: uint;
  a: int;
  b: int;
  c: int;
  rect: Rect;
  started: uint;
  now: uint;
  digits: string[16];
  used: int;

// Silent check: the console shares the shadow, so printing between composition
// steps would scroll the windows. The first failure is reported after the
// composition.
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed then
  begin
    if not failed then
    begin
      failed := true;
      fail_step := step;
    end;
    out.say("windows_demo: failed check ");
    used := core.format_int_into(digits, step, uint(10), uint(0));
    out.say(digits);
    out.say(" (");
    out.say(label);
    out.say(")\n");
  end;
end;

// Fills one window's content rectangle and draws a label and a rule. The
// coordinates come from the service, so the drawing never enters the chrome.
procedure draw_content(id: int; fill: uint; label: const string[0]);
begin
  status := syslib.window_content_rect(id, rect);
  if status = Ok then
  begin
    status := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    status := syslib.draw_rect(rect.x + 2, rect.y + 2, rect.w - 4, rect.h - 4, black);
    status := syslib.draw_text(rect.x + 10, rect.y + 12, label, black);
    status := syslib.draw_text(rect.x + 10, rect.y + 28, "M2.3.2 composition", black);
  end;
end;

// The complete composition sequence, used once for the checks and once for the
// final screenshot after the console lines have scrolled the shadow.
procedure compose();
begin
  status := syslib.clear(white);
  status := syslib.fill_dither(0, 0, int(screen_w), int(screen_h), black);
  status := syslib.window_open(60, 70, 300, 180, "Alpha", a);
  status := syslib.window_open(240, 150, 320, 200, "Beta", b);
  status := syslib.window_open(420, 280, 280, 180, "Gamma", c);
  draw_content(a, alpha_fill, "alpha content");
  draw_content(b, beta_fill, "beta content");
  draw_content(c, gamma_fill, "gamma content");
  status := syslib.window_move(b, 520, 420);
  draw_content(a, alpha_fill, "alpha content");
  draw_content(b, beta_fill, "beta content");
  draw_content(c, gamma_fill, "gamma content");
  status := syslib.window_raise(a);
  draw_content(b, beta_fill, "beta content");
  draw_content(c, gamma_fill, "gamma content");
  draw_content(a, alpha_fill, "alpha content");
  status := syslib.window_raise(b);
  status := syslib.window_raise(c);
  status := syslib.window_raise(a);
  status := syslib.present(0, 0, int(screen_w), int(screen_h));
end;

// Repairs the live windows after the console scrolled the shadow: the desktop
// is redrawn, then contents bottom-to-top, then a raise rotation repaints every
// chrome in its visible parts, and finally the whole screen is presented.
procedure repaint();
begin
  status := syslib.clear(white);
  status := syslib.fill_dither(0, 0, int(screen_w), int(screen_h), black);
  draw_content(b, beta_fill, "beta content");
  draw_content(c, gamma_fill, "gamma content");
  draw_content(a, alpha_fill, "alpha content");
  status := syslib.window_raise(b);
  status := syslib.window_raise(c);
  status := syslib.window_raise(a);
  status := syslib.present(0, 0, int(screen_w), int(screen_h));
end;

procedure main(args: const string[1024]);
begin
  ok := true;
  failed := false;
  fail_step := 0;
  step := 0;
  white := uint(0x00FFFFFF);
  black := uint(0x00000000);
  grey := uint(0x00CCCCCC);
  alpha_fill := uint(0x0022BBDD);
  beta_fill := uint(0x00DD66AA);
  gamma_fill := uint(0x00E8D070);
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    out.say("windows_demo: no graphics\n")
  else if screen_w < uint(900) or screen_h < uint(700) then
    out.say("windows_demo: screen too small\n")
  else
  begin
    // Compose and check without printing between steps.
    compose();
    verify("open Alpha", a > 0);
    verify("focus Alpha", syslib.window_focus() = a);
    verify("focused title white", syslib.get_pixel(61, 71) = white);
    verify("focused title striped", syslib.get_pixel(61, 72) = black);
    verify("left border", syslib.get_pixel(60, 75) = black);
    verify("Alpha content", syslib.get_pixel(81, 146) = alpha_fill);
    verify("Gamma title unfocused", syslib.get_pixel(421, 281) = grey);
    verify("Gamma content", syslib.get_pixel(441, 356) = gamma_fill);
    verify("Alpha right border", syslib.get_pixel(361, 75) = black);
    verify("focus ends on Alpha", syslib.window_focus() = a);

    // The console shares the shadow: report, then repaint the whole frame.
    if failed then
      out.say("windows_demo: checks FAILED\n")
    else
      out.say("windows_demo: checks ok\n");
    out.say("windows_demo: ready\n");
    repaint();

    // Hold the frame for the screenshot. Realtime gives us the time; a normal
    // task would be killed by the total budget.
    status := core.realtime_on();
    started := core.clock();
    if status = Ok then
    begin
      while core.clock() - started < uint(HoldNs) do
      begin
        core.heartbeat();
        now := core.clock();
      end;
    end
    else
    begin
      while core.clock() - started < uint(FallbackHoldNs) do
        now := core.clock();
    end;

    if ok and not failed then
      out.say("windows_demo: ok\n");
  end;
end;

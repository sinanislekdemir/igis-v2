unit windows_events;

// M2.3.3 proof: event routing, `Expose` delivery, `window_present` and
// close-on-task-termination (spec 0011, W12/W16/W17).
//
//   run windows_events
//
// Everything is checked silently: the console shares the shadow, so printing
// between composition steps would scroll the windows. The first failure is
// reported by its step number at the end (`windows_events: FAIL at step N`).
//
//   1. Open A and B; move the pointer inside A while B is focused: a pointer
//      move is routed to the focused window.
//   2. Click inside A: the topmost window at the point is raised and focused
//      *before* the PointerDown is delivered.
//   3. A key event is routed to the focused window.
//   4. Moving B (below A) over A marks a whole-content Expose for A. The app
//      repaints A and calls `window_present`; the content and chrome pixels
//      are checked.
//   5. Opening C above A and moving it over A marks exposes for C and A. The
//      app repaints a visible part of A (never C's rectangle), presents it,
//      and verifies that C's content and border are intact.
//   6. A child task polls the queue and tries `window_present` on the parent's
//      window: it must get `WouldBlock` and `Invalid`, and the parent's pending
//      Expose must still be readable afterwards.
//   7. A child opens a window and exits: the parent verifies the window is gone,
//      the focus falls back, and the close repair marked an Expose for A.
//
// Steps 6 and 7 spawn children, whose spawn/exit log lines scroll the console;
// all pixel checks therefore happen before step 6.

uses core, syslib, lib.out, lib.outcome;

var
  failed: bool;
  fail_step: int;
  step: int;
  status: Status;
  present_status: Status;
  diag: Diagnostic;
  reason: ExitReason;
  exited_ok: bool;
  ev: Event;
  child: int;
  a: int;
  b: int;
  c: int;
  next_id: int;
  rect: Rect;
  screen_w: uint;
  screen_h: uint;
  white: uint;
  black: uint;
  cyan: uint;
  magenta: uint;
  yellow: uint;
  marker: uint;
  text: string[16];
  used: int;
  flushed: uint;
  seen: bool;
  more: bool;
  polls: int;
  baseline: int;

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
  end;
end;

procedure classify_reason();
begin
  exited_ok := outcome.is_ok(reason);
end;

// Drains pollable events, presenting every window an Expose names. Never
// blocks: stops at WouldBlock or after 16 events.
procedure pump();
var
  count: int;
  running: bool;
begin
  count := 0;
  running := true;
  while running and count < 16 do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose then
        status := syslib.window_present(ev.window);
      count := count + 1;
    end
    else
      running := false;
  end;
end;

// Fills the window's content, outlines it and writes a label, all inside the
// content rectangle the service reported.
procedure draw_content(id: int; fill: uint; label: const string[0]);
begin
  status := syslib.window_content_rect(id, rect);
  if status = Ok then
  begin
    status := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    status := syslib.draw_rect(rect.x + 2, rect.y + 2, rect.w - 4, rect.h - 4, black);
    status := syslib.draw_text(rect.x + 10, rect.y + 12, label, black);
  end;
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  fail_step := 0;
  step := 0;
  white := uint(0x00FFFFFF);
  black := uint(0x00000000);
  cyan := uint(0x0022BBDD);
  magenta := uint(0x00DD66AA);
  yellow := uint(0x00E8D070);
  marker := uint(0x00116633);
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    out.say("windows_events: no graphics\n")
  else if screen_w < uint(560) or screen_h < uint(420) then
    out.say("windows_events: screen too small\n")
  else
  begin
    // The kernel's terminal and system-log windows exist from boot (spec
    // 0012); the idle focus is the previously topmost window, not 0.
    baseline := syslib.window_focus();
    status := syslib.clear(white);
    status := syslib.fill_dither(0, 0, int(screen_w), int(screen_h), black);
    flushed := syslib.event_flush();
    status := syslib.inject_pointer(-100000, -100000, uint(0));
    flushed := syslib.event_flush();

    // 1. Two windows; B is focused.
    status := syslib.window_open(80, 80, 260, 150, "Window A", a);
    verify("open A", status = Ok and a > 0);
    draw_content(a, cyan, "alpha content");
    status := syslib.window_open(420, 300, 220, 130, "Window B", b);
    verify("open B", status = Ok and b > a);
    draw_content(b, magenta, "beta content");
    verify("B focused", syslib.window_focus() = b);
    pump();

    // Pointer move while B is focused: routed to B.
    status := syslib.inject_pointer(150, 120, uint(0));
    status := syslib.next_event(ev);
    verify(
      "pointer move routed to focus",
      status = Ok and ev.kind = PointerMove and ev.window = b
        and ev.x = 150 and ev.y = 120);

    // 2. Click inside A: A is raised and focused before delivery.
    status := syslib.inject_pointer(0, 0, uint(1));
    status := syslib.next_event(ev);
    verify(
      "click delivered to A",
      status = Ok and ev.kind = PointerDown and ev.window = a);
    verify("click focused A", syslib.window_focus() = a);
    status := syslib.inject_pointer(0, 0, uint(0));
    pump();

    // 3. Key event routed to the focused window.
    ev.kind := KeyDown;
    ev.x := 0;
    ev.y := 0;
    ev.code := uint(65);
    ev.window := 0;
    status := syslib.inject_event(ev);
    status := syslib.next_event(ev);
    verify(
      "key routed to A",
      status = Ok and ev.kind = KeyDown and ev.window = a and ev.code = uint(65));

    // 4. Move B (below A) over A: A is the topmost damaged window.
    pump();
    flushed := syslib.event_flush();
    status := syslib.window_move(b, 40, 120);
    status := syslib.next_event(ev);
    verify(
      "expose for A",
      status = Ok and ev.kind = Expose and ev.window = a
        and ev.x = 0 and ev.y = 0 and ev.code = uint(0));
    draw_content(a, marker, "alpha repaint");
    present_status := syslib.window_present(a);
    verify("present A", present_status = Ok);
    verify("A repainted", syslib.get_pixel(100, 120) = marker);
    verify("A chrome intact", syslib.get_pixel(80, 150) = black);
    pump();

    // 5. C above A, moved over A's content. Repaint only a visible part of A.
    status := syslib.window_open(250, 180, 200, 120, "Window C", c);
    verify("open C", status = Ok and c > b and syslib.window_focus() = c);
    draw_content(c, yellow, "gamma content");
    pump();
    flushed := syslib.event_flush();
    status := syslib.window_move(c, 270, 190);
    seen := false;
    polls := 0;
    more := true;
    while more and polls < 8 do
    begin
      status := syslib.poll_event(ev);
      if status = Ok then
      begin
        if ev.kind = Expose then
        begin
          if ev.window = c then
          begin
            draw_content(c, yellow, "gamma content");
            status := syslib.window_present(c);
          end
          else if ev.window = a then
          begin
            seen := true;
            verify("A expose fields", ev.x = 0 and ev.y = 0 and ev.code = uint(0));
            status := syslib.fill_rect(90, 110, 80, 40, marker);
            status := syslib.window_present(a);
          end
          else
            status := syslib.window_present(ev.window);
        end;
        polls := polls + 1;
      end
      else
        more := false;
    end;
    verify("A exposed under C", seen);
    verify("marker visible", syslib.get_pixel(100, 120) = marker);
    verify("C content intact", syslib.get_pixel(300, 220) = yellow);
    verify("C border intact", syslib.get_pixel(270, 190) = black);
    verify("A border intact", syslib.get_pixel(80, 150) = black);

    // 6. A foreign task cannot take the owner's expose and cannot present its
    // window. A fresh expose for A is pending before the child runs.
    pump();
    flushed := syslib.event_flush();
    status := syslib.window_move(b, 40, 180);
    used := core.format_int_into(text, a, uint(10), uint(0));
    status := core.spawn("windows_events_child", text, diag, child);
    verify("spawn poll child", status = Ok and child > 0);
    status := core.wait(child, reason);
    classify_reason();
    verify("poll child exited Ok", status = Ok and exited_ok);
    status := syslib.next_event(ev);
    verify(
      "foreign child did not steal the expose",
      status = Ok and ev.kind = Expose and ev.window = a);
    // B was exposed by the same move; present it before the child's report.
    status := syslib.next_event(ev);
    polls := 0;
    while status = Ok and ev.kind = Expose and polls < 4 do
    begin
      status := syslib.window_present(ev.window);
      status := syslib.next_event(ev);
      polls := polls + 1;
    end;
    verify(
      "child reported WouldBlock and present Invalid",
      status = Ok and ev.kind = KeyDown and ev.code = uint(1));

    // 7. A child window disappears with its task and the close repair exposes A.
    pump();
    flushed := syslib.event_flush();
    next_id := c + 1;
    status := core.spawn("windows_events_open", "", diag, child);
    verify("spawn opener child", status = Ok and child > 0);
    status := core.wait(child, reason);
    classify_reason();
    verify("opener exited Ok", status = Ok and exited_ok);
    status := syslib.window_frame_rect(next_id, rect);
    verify("child window gone", status = NotFound);
    verify("focus fell back to C", syslib.window_focus() = c);
    seen := false;
    polls := 0;
    more := true;
    while more and polls < 8 do
    begin
      status := syslib.poll_event(ev);
      if status = Ok then
      begin
        if ev.kind = Expose then
        begin
          if ev.window = a then
          begin
            seen := true;
            status := syslib.fill_rect(90, 110, 80, 40, marker);
            status := syslib.window_present(a);
          end
          else
            status := syslib.window_present(ev.window);
        end;
        polls := polls + 1;
      end
      else
        more := false;
    end;
    verify("close exposed A", seen);
    status := syslib.window_present(next_id);
    verify("present of a closed id is NotFound", status = NotFound);

    verify("close A", syslib.window_close(a) = Ok);
    verify("close B", syslib.window_close(b) = Ok);
    verify("close C", syslib.window_close(c) = Ok);
    verify("focus none", syslib.window_focus() = baseline);

    if failed then
    begin
      used := core.format_int_into(text, fail_step, uint(10), uint(0));
      out.say("windows_events: FAIL at step ");
      out.say(text);
      out.say("\n");
    end
    else
      out.say("windows_events: ok\n");
  end;
end;

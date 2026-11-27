unit windows_drag;

// M8.1/M8.2 proof: service-owned pointer drag and close requests
// (spec 0011, W22/W23/W24/W25).
//
//   run windows_drag check   // headless proof, prints `windows_drag: ok`
//   run windows_drag shot    // same checks, then holds the frame for a QMP
//                            // screenshot (`windows_drag: ready`)
//
// The application opens a large "Underlay" window with a distinctive colour,
// then a "Drag Target" window above it. Every pointer event is a real
// `syslib.inject_pointer` call, so the same service path a device takes runs
// synchronously in the caller:
//
//   1. A press in the title bar consumes the `PointerDown` (no client click),
//      raises/focuses the window and starts the drag without moving it.
//   2. Moves with button 1 held follow the pointer minus the grab offset; the
//      frame rectangle is asserted after every step, including both screen
//      clamps. Release ends the drag and later motion does not move it.
//   3. Draining the owner `Expose`s and repainting shows the target content at
//      the new position and the underlay colour where the target used to be.
//   4. A content-area press is still delivered normally (window = target id).
//   5. A same-task dialog receives `Close` (no client click); the app closes
//      only that window and its main window survives.
//   6. `windows_drag_victim` opens "Close Target" and blocks in `next_event`;
//      a press in its close box delivers `Close`, the victim closes its own
//      window and exits `Ok` (it is never killed).
//
// The underlay is a retained canvas below the target; the compositor paints it
// first, so a whole-content repaint of the lower window cannot overwrite the
// higher one (spec 0026 C14/C15).
//
// With graphics disabled the app prints `windows_drag: host ok`; `langhost`
// runs exactly that path.

uses core, syslib, lib.out, lib.inject, lib.outcome;

const
  WaitMs = 5000;
  ShotHoldNs = 3000000000;
  UnderX = 100;
  UnderY = 60;
  UnderW = 700;
  UnderH = 500;
  TargetX = 150;
  TargetY = 150;
  TargetW = 300;
  TargetH = 200;

var
  failed: bool;
  fail_step: int;
  fail_label: string[64];
  step: int;
  status: Status;
  diag: Diagnostic;
  reason: ExitReason;
  ev: Event;
  shot: bool;
  under: int;
  id: int;
  child: int;
  victim: int;
  dialog: int;
  rect: Rect;
  frame: Rect;
  content: Rect;
  old_content: Rect;
  screen_w: uint;
  screen_h: uint;
  black: uint;
  base: uint;
  mark: uint;
  under_color: uint;
  deadline: uint;
  started: uint;
  ok: bool;
  saw_down: bool;
  saw_close: bool;
  expect_x: int;
  expect_y: int;
  drag_x: int;
  drag_y: int;
  dx: int;
  dy: int;
  count: int;
  running: bool;
  flushed: uint;
  paint_count: int;
  marker_color: uint;
  last_marker: uint;
  marker_a: uint;
  marker_b: uint;
  copied_marker: uint;
  repaints_before: int;

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

function now_ms(): uint;
begin
  return core.clock() div uint(1000000);
end;

// The id of the first live window with this title, or 0.
function window_id_of(wanted: const string[0]): int;
var
  result: int;
  keep: bool;
  pos: uint;
  id_here: int;
  r: Rect;
  t: string[64];
  s: Status;
begin
  result := 0;
  keep := true;
  pos := uint(0);
  s := syslib.window_info(pos, id_here, r, t);
  while keep and pos < uint(32) do
  begin
    if s = Ok then
    begin
      if t = wanted then
      begin
        result := id_here;
        keep := false;
      end
      else
      begin
        pos := pos + uint(1);
        if pos < uint(32) then
          s := syslib.window_info(pos, id_here, r, t)
        else
          keep := false;
      end;
    end
    else
      keep := false;
  end;
  return result;
end;

// Fill a rectangle, tolerating a non-positive size (the bands can vanish).
procedure fill_if(x: int; y: int; w: int; h: int; color: uint);
begin
  if w > 0 and h > 0 then
    status := syslib.fill_rect(x, y, w, h, color);
end;

// Paints the "Underlay" window. Since M16 the compositor paints the retained
// canvases bottom to top, so filling the whole content is safe: the target's
// canvas is composited above it and a client never overwrites another window
// (spec 0026 C14/C15).
procedure paint_underlay();
var
  c: Rect;
  st: Status;
begin
  st := syslib.window_begin(under);
  st := syslib.window_content_rect(under, c);
  if st = Ok then
  begin
    st := syslib.fill_rect(0, 0, c.w, c.h, under_color);
    st := syslib.window_present(under);
  end;
end;

// Paints and presents the target: a solid base colour and a 16x16 mark near the
// content origin. The target is topmost while it is dragged, so a whole-content
// paint is safe here.
procedure paint_target();
var
  st: Status;
begin
  st := syslib.window_begin(id);
  st := syslib.window_content_rect(id, content);
  if st = Ok then
  begin
    // Window-local (spec 0026 C8); the rectangle is read only for its size.
    st := syslib.fill_rect(0, 0, content.w, content.h, base);
    st := syslib.fill_rect(20, 20, 16, 16, mark);
    st := syslib.draw_rect(2, 2, content.w - 4, content.h - 4, black);
    st := syslib.draw_text(20, 48, "Drag Target", black);
    // Repaint marker (W28 proof): a 4x4 patch at content+(2,2), after the
    // outline, alternating between two colours on every repaint. The service
    // copy moves the current colour to the new origin; the client's refreshed
    // repaint must replace it with the other one.
    last_marker := marker_color;
    st := syslib.fill_rect(2, 2, 4, 4, marker_color);
    if marker_color = marker_a then
      marker_color := marker_b
    else
      marker_color := marker_a;
    paint_count := paint_count + 1;
    st := syslib.window_present(id);
  end;
end;

// Services queued events without blocking: every expose for our two windows is
// repainted, and the loop is bounded so a busy queue cannot starve the proof.
procedure pump();
begin
  count := 0;
  running := true;
  while running and count < 32 do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose then
      begin
        if ev.window = id then
          paint_target()
        else if ev.window = under then
          paint_underlay();
      end;
      count := count + 1;
    end
    else
      running := false;
  end;
end;

// Drains the queue and reports whether a client PointerDown for `wanted`
// appeared. Exposes are repainted on the way.
procedure drain_for_down(wanted: int);
begin
  saw_down := false;
  count := 0;
  running := true;
  while running and count < 32 do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      if ev.kind = PointerDown and ev.window = wanted then
        saw_down := true;
      if ev.kind = Expose then
      begin
        if ev.window = id then
          paint_target()
        else if ev.window = under then
          paint_underlay();
      end;
      count := count + 1;
    end
    else
      running := false;
  end;
end;

// Drains the queue looking for the close request of `wanted` and a client
// PointerDown addressed to it. The close-box press is consumed by the service,
// so only the Close record may appear.
procedure drain_for_close(wanted: int);
begin
  saw_close := false;
  saw_down := false;
  count := 0;
  running := true;
  while running and count < 32 do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      if ev.kind = Close and ev.window = wanted then
        saw_close := true;
      if ev.kind = PointerDown and ev.window = wanted then
        saw_down := true;
      count := count + 1;
    end
    else
      running := false;
  end;
end;

// Move the pointer with button 1 held by screen-oriented deltas.
procedure drag_pointer(x: int; y: int);
begin
  dx := x - syslib.pointer_x();
  dy := y - syslib.pointer_y();
  status := syslib.inject_pointer(dx, dy, uint(1));
end;

procedure wait_victim(ms: uint);
begin
  victim := 0;
  deadline := now_ms() + ms;
  while victim = 0 and now_ms() < deadline do
  begin
    pump();
    victim := window_id_of("Close Target");
  end;
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  step := 0;
  fail_step := 0;
  black := uint(0x00000000);
  base := uint(0x00224466);
  mark := uint(0x0033CC66);
  under_color := uint(0x00CC44AA);
  marker_a := uint(0x00FF00FF);
  marker_b := uint(0x0000FFFF);
  marker_color := marker_a;
  last_marker := marker_a;
  paint_count := 0;
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();

  if screen_w = uint(0) then
    out.say("windows_drag: host ok\n")
  else if screen_w < uint(UnderX + UnderW + 40) or screen_h < uint(UnderY + UnderH + 40) then
    out.say("windows_drag: screen too small\n")
  else
  begin
    shot := args = "shot";
    under := 0;
    id := 0;
    child := 0;
    victim := 0;
    dialog := 0;

    // A known pointer state and no stale events.
    status := inject.pointer_to(0, 0);
    flushed := syslib.event_flush();

    status := syslib.window_open(UnderX, UnderY, UnderW, UnderH, "Underlay", under);
    verify("open underlay", status = Ok and under > 0);
    status := syslib.window_open(TargetX, TargetY, TargetW, TargetH, "Drag Target", id);
    verify("open target", status = Ok and id > 0);
    if id > 0 and under > 0 then
    begin
      paint_underlay();
      paint_target();
      old_content := content;
      status := syslib.window_frame_rect(id, frame);
      verify("target frame", status = Ok
        and frame.x = TargetX and frame.y = TargetY
        and frame.w = TargetW + 2 and frame.h = TargetH + 17);

      // 1. Press the title bar: the service raises/focuses, starts the drag and
      //    consumes the PointerDown, so no client click reaches us.
      drag_x := frame.x + 100;
      drag_y := frame.y + 8;
      dx := drag_x - syslib.pointer_x();
      dy := drag_y - syslib.pointer_y();
      status := syslib.inject_pointer(dx, dy, uint(0));
      flushed := syslib.event_flush();
      status := syslib.inject_pointer(0, 0, uint(1));
      drain_for_down(id);
      verify("title press consumed", not saw_down);
      verify("press focused", syslib.window_focus() = id);
      status := syslib.window_frame_rect(id, rect);
      verify("press did not move", status = Ok
        and rect.x = TargetX and rect.y = TargetY);

      // 2. Follow the pointer with button 1 held.
      drag_pointer(drag_x + 40, drag_y + 30);
      status := syslib.window_frame_rect(id, rect);
      verify("drag step 1", status = Ok
        and rect.x = TargetX + 40 and rect.y = TargetY + 30);

      drag_pointer(drag_x + 120, drag_y + 90);
      status := syslib.window_frame_rect(id, rect);
      verify("drag step 2", status = Ok
        and rect.x = TargetX + 120 and rect.y = TargetY + 90);

      // 3. Clamp at the top-left and then the bottom-right.
      drag_pointer(-50, -50);
      status := syslib.window_frame_rect(id, rect);
      verify("top-left clamp", status = Ok and rect.x = 0 and rect.y = 0);

      drag_pointer(int(screen_w) + 100, int(screen_h) + 100);
      status := syslib.window_frame_rect(id, rect);
      expect_x := int(screen_w) - (TargetW + 2);
      expect_y := int(screen_h) - (TargetH + 17);
      verify("bottom-right clamp", status = Ok
        and rect.x = expect_x and rect.y = expect_y);

      // 4. Release ends the drag; further motion does not move the window.
      status := syslib.inject_pointer(0, 0, uint(0));
      status := syslib.inject_pointer(-200, -200, uint(0));
      status := syslib.window_frame_rect(id, rect);
      verify("release parks", status = Ok
        and rect.x = expect_x and rect.y = expect_y);

      // 5. The service already copied the topmost content snapshot to the new
      //    origin during the move (W28): the distinctive mark and the current
      //    repaint-marker colour are there before any client repaint. Then the
      //    refreshed `Expose` repaint must replace the marker at the new origin
      //    and restore the old area to the underlay.
      // The composition checks read the composed shadow (spec 0026 C6).
      status := syslib.window_begin(0);
      status := syslib.window_content_rect(id, content);
      verify("service copy has the mark", status = Ok
        and syslib.get_pixel(content.x + 28, content.y + 28) = mark);
      copied_marker := syslib.get_pixel(content.x + 2, content.y + 2);
      verify("service copy carries the marker", copied_marker = last_marker);
      repaints_before := paint_count;
      pump();
      status := syslib.window_begin(0);
      status := syslib.window_content_rect(id, content);
      verify("refresh repainted the client", paint_count > repaints_before);
      verify("new content mark", status = Ok
        and syslib.get_pixel(content.x + 28, content.y + 28) = mark);
      verify("client repainted the marker", status = Ok
        and syslib.get_pixel(content.x + 2, content.y + 2) = last_marker
        and syslib.get_pixel(content.x + 2, content.y + 2) <> copied_marker);
      verify("old origin is not the new marker",
        syslib.get_pixel(old_content.x + 2, old_content.y + 2)
          <> syslib.get_pixel(content.x + 2, content.y + 2));
      verify("old area restored",
        syslib.get_pixel(TargetX + 21, TargetY + 36) = under_color);

      // 6. A content-area press is still delivered to the client, routed to
      //    this window.
      dx := content.x + 40 - syslib.pointer_x();
      dy := content.y + 40 - syslib.pointer_y();
      status := syslib.inject_pointer(dx, dy, uint(0));
      flushed := syslib.event_flush();
      status := syslib.inject_pointer(0, 0, uint(1));
      drain_for_down(id);
      verify("content press delivered", saw_down);
      status := syslib.inject_pointer(0, 0, uint(0));

      // 7. A same-task dialog: a second window owned by this task gets a
      //    close request, the app closes only that window and its main window
      //    survives. The close-box press never reaches the client as a click.
      status := syslib.window_open(820, 620, 160, 100, "Drag Dialog", dialog);
      verify("open dialog", status = Ok and dialog > 0);
      if dialog > 0 then
      begin
        status := syslib.window_frame_rect(dialog, frame);
        verify("dialog frame", status = Ok);
        dx := frame.x + 5 - syslib.pointer_x();
        dy := frame.y + 5 - syslib.pointer_y();
        status := syslib.inject_pointer(dx, dy, uint(0));
        flushed := syslib.event_flush();
        status := syslib.inject_pointer(0, 0, uint(1));
        status := syslib.inject_pointer(0, 0, uint(0));
        drain_for_close(dialog);
        verify("dialog close requested", saw_close);
        verify("dialog close is not a client click", not saw_down);
        verify("dialog stays until the owner closes", window_id_of("Drag Dialog") = dialog);
        status := syslib.window_close(dialog);
        verify(
          "dialog closed by its owner",
          status = Ok and window_id_of("Drag Dialog") = 0);
        verify("main window survives the dialog", window_id_of("Drag Target") = id);
        dialog := 0;
      end;

      // 8. The close box of another task's window asks that task to close it;
      //    the victim handles Close, closes its own window and exits Ok.
      status := core.spawn("windows_drag_victim", "", diag, child);
      verify("spawn victim", status = Ok and child > 0);
      if child > 0 then
      begin
        wait_victim(uint(WaitMs));
        verify("victim window", victim > 0);
        if victim > 0 then
        begin
          status := syslib.window_frame_rect(victim, frame);
          verify("victim frame", status = Ok);
          dx := frame.x + 5 - syslib.pointer_x();
          dy := frame.y + 5 - syslib.pointer_y();
          status := syslib.inject_pointer(dx, dy, uint(0));
          flushed := syslib.event_flush();
          status := syslib.inject_pointer(0, 0, uint(1));
          status := syslib.inject_pointer(0, 0, uint(0));
          status := core.wait(child, reason);
          verify("victim closed by request", status = Ok and outcome.is_ok(reason));
          verify("victim window gone", window_id_of("Close Target") = 0);
          status := syslib.window_frame_rect(victim, rect);
          verify("victim frame gone", status = NotFound);
        end;
      end;

      // Optional screenshot hold: leave the dragged window composed.
      if not failed and shot then
      begin
        status := syslib.window_move(under, UnderX, UnderY);
        out.say("windows_drag: ready\n");
        started := core.clock();
        while core.clock() - started < uint(ShotHoldNs) do
        begin
          status := syslib.inject_pointer(0, 0, uint(0));
          flushed := syslib.event_flush();
        end;
      end;
    end;

    if dialog > 0 then
      status := syslib.window_close(dialog);
    if id > 0 then
      status := syslib.window_close(id);
    if under > 0 then
      status := syslib.window_close(under);

    if failed then
    begin
      out.say("windows_drag: FAIL at step ");
      out.say_int(fail_step);
      out.say(" (");
      out.say(fail_label);
      out.say(")\n");
    end
    else
      out.say("windows_drag: ok\n");
  end;
end;

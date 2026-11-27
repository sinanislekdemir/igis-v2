unit local_coords;

// M17 proof: window-local client coordinates (spec 0026 C4/C8/C9).
//
//   run local_coords check   // headless proof, prints `local_coords: ok`
//   run local_coords ready   // draws one frame, prints its frame origin and
//                            // holds for `scripts/local_coords_qmp.py`
//
// The proof pins the two consequences of the local-coordinate model:
//
//   1. A window moved by the service *while the owner is mid-frame* does not
//      change anything the client computes. The client draws a corner mark at
//      local (4, 4), the service drags the frame, the client finishes the
//      frame without re-reading the origin, and the completed frame lands at
//      the new origin: the mark is present at the new screen position and the
//      old one is gone.
//   2. A window-addressed pointer event carries content-local `x`/`y`
//      (spec 0026 C9): a pointer move at `content + (30, 25)` arrives as
//      `(30, 25)` with this window as the target.
//
// The drag is the real service path: the pointer press goes to the title bar
// through `syslib.inject_pointer`, the service consumes it, moves the frame and
// releases. `langhost` has a disabled screen and prints `local_coords: host ok`.

uses core, stdlib, syslib, lib.out, lib.inject;

const
  WindowX = 200;
  WindowY = 150;
  ContentW = 240;
  ContentH = 160;
  DragDx = 60;
  DragDy = 40;
  HoldNs = 4000000000;

  MarkA = 0x00116633;
  MarkB = 0x00AA2266;
  White = 0x00FFFFFF;
  Black = 0x00000000;
  Ink = 0x00000000;

var
  failed: bool;
  step: int;
  fail_step: int;
  fail_label: string[64];
  status: Status;
  ignored: Status;
  id: int;
  content: Rect;
  frame: Rect;
  ev: Event;
  screen_w: uint;
  screen_h: uint;
  flushed: uint;
  start: uint;

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

// The complete frame in window-local coordinates: paper, outline, both corner
// marks and a caption. Never reads the content origin.
procedure draw_frame();
begin
  status := syslib.fill_rect(0, 0, ContentW, ContentH, uint(White));
  status := syslib.draw_rect(2, 2, ContentW - 4, ContentH - 4, uint(Black));
  status := syslib.fill_rect(4, 4, 8, 8, uint(MarkA));
  status := syslib.fill_rect(ContentW - 12, ContentH - 12, 8, 8, uint(MarkB));
  status := syslib.draw_text(10, 24, "local coordinates", uint(Ink));
end;

// The title bar of the current frame: one row below the top border, away from
// the close and minimize boxes.
procedure drag_title();
var
  grab_x: int;
  grab_y: int;
begin
  status := syslib.window_frame_rect(id, frame);
  if status <> Ok then
    return;
  grab_x := frame.x + 120;
  grab_y := frame.y + 8;
  status := inject.pointer_to(grab_x, grab_y);
  status := inject.pointer_down();
  // The service consumed the press and started the drag.
  status := syslib.inject_pointer(DragDx, DragDy, uint(1));
  status := syslib.inject_pointer(0, 0, uint(0));
  // Drop every queued event: the drag's moves are addressed to this window.
  flushed := syslib.event_flush();
end;

procedure check_mode();
var
  old_x: int;
  old_y: int;
  seen: bool;
begin
  failed := false;
  step := 0;
  fail_step := 0;

  status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Local", id);
  verify("window open", status = Ok and id > 0);
  if status <> Ok then
  begin
    out.say("local_coords: no window\n");
    return;
  end;
  status := syslib.window_content_rect(id, content);
  verify("content rect", status = Ok and content.w = ContentW and content.h = ContentH);

  // 1. A complete frame reads back locally and lands at the content origin.
  draw_frame();
  status := syslib.window_present(id);
  verify("present", status = Ok);
  verify("local corner A", syslib.get_pixel(5, 5) = uint(MarkA));
  verify("local corner B", syslib.get_pixel(ContentW - 11, ContentH - 11) = uint(MarkB));
  // Composition checks read the composed shadow with no scope (C6).
  status := syslib.window_begin(0);
  verify("screen corner A", syslib.get_pixel(content.x + 5, content.y + 5) = uint(MarkA));

  // 2. Half a frame, a service drag, then the rest of the same frame. The
  //    client never re-reads the origin between the two halves.
  old_x := content.x;
  old_y := content.y;
  status := syslib.window_begin(id);
  status := syslib.fill_rect(0, 0, ContentW, ContentH, uint(White));
  status := syslib.draw_rect(2, 2, ContentW - 4, ContentH - 4, uint(Black));
  status := syslib.fill_rect(4, 4, 8, 8, uint(MarkA));
  drag_title();
  status := syslib.fill_rect(ContentW - 12, ContentH - 12, 8, 8, uint(MarkB));
  status := syslib.draw_text(10, 24, "local coordinates", uint(Ink));
  status := syslib.window_present(id);
  status := syslib.window_begin(0);
  status := syslib.window_frame_rect(id, frame);
  verify("frame moved", status = Ok and frame.x = WindowX + DragDx and frame.y = WindowY + DragDy);
  status := syslib.window_content_rect(id, content);
  verify("content followed", status = Ok and content.x = old_x + DragDx and content.y = old_y + DragDy);
  verify("mark at the new origin", syslib.get_pixel(content.x + 5, content.y + 5) = uint(MarkA));
  verify("second mark at the new origin",
    syslib.get_pixel(content.x + ContentW - 11, content.y + ContentH - 11) = uint(MarkB));
  verify("old origin is not the mark", syslib.get_pixel(old_x + 5, old_y + 5) <> uint(MarkA));

  // 3. A window-addressed pointer event is content-local (C9). The move queued
  //    an owner `Expose`; drain it, then inject the pointer at the new content
  //    origin plus (30, 25).
  status := syslib.poll_event(ev);
  while status = Ok do
    status := syslib.poll_event(ev);
  status := inject.pointer_to(content.x + 30, content.y + 25);
  seen := false;
  if status = Ok then
  begin
    status := syslib.next_event(ev);
    if status = Ok and ev.kind = PointerMove and ev.window = id then
      seen := ev.x = 30 and ev.y = 25;
  end;
  verify("pointer event is content-local", seen);

  status := syslib.window_close(id);
  verify("window closed", status = Ok);
  if failed then
  begin
    out.say("local_coords: FAIL at step ");
    out.say_int(fail_step);
    out.say(" (");
    out.say(fail_label);
    out.say(")\n");
  end
  else
    out.say("local_coords: ok\n");
end;

// `ready` mode: draw one frame, report the frame origin and keep repainting on
// an owner `Expose` so the QMP harness's drag lands on a fresh frame.
procedure ready_mode();
begin
  status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Local", id);
  if status <> Ok then
  begin
    out.say("local_coords: no window\n");
    return;
  end;
  draw_frame();
  status := syslib.window_present(id);
  status := syslib.window_frame_rect(id, frame);
  out.say("local_coords: frame ");
  out.say_int(frame.x);
  out.say(" ");
  out.say_int(frame.y);
  out.say("\n");
  out.say("local_coords: ready\n");
  start := core.clock();
  while core.clock() < start + uint(HoldNs) do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose and ev.window = id then
      begin
        draw_frame();
        status := syslib.window_present(id);
        status := syslib.window_frame_rect(id, frame);
        out.say("local_coords: frame ");
        out.say_int(frame.x);
        out.say(" ");
        out.say_int(frame.y);
        out.say("\n");
      end;
    end
    else
      ignored := core.sleep(uint(5));
  end;
  status := syslib.window_close(id);
end;

procedure host_path();
begin
  // A disabled screen still answers with 0 metrics and `Invalid` drawing.
  failed := false;
  step := 0;
  verify("screen width is 0", syslib.screen_width() = uint(0));
  verify("fill is Invalid", syslib.fill_rect(0, 0, 8, 8, uint(White)) = Invalid);
  verify("get_pixel is 0", syslib.get_pixel(0, 0) = uint(0));
  if failed then
    out.say("local_coords: host FAIL\n")
  else
    out.say("local_coords: host ok\n");
end;

procedure main(args: const string[1024]);
begin
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    host_path()
  else if screen_w < uint(WindowX + ContentW + 40) or
          screen_h < uint(WindowY + ContentH + 60) then
    out.say("local_coords: screen too small\n")
  else if stdlib.starts_with(args, "ready") then
    ready_mode()
  else
    check_mode();
end;

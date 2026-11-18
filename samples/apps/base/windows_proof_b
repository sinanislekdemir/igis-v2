unit windows_proof_b;

// M2.3.5 child B: the second side-by-side application (spec 0011).
//
// It opens "Proof B" at (560, 300), draws its content and presents it. It reads
// the parent's command stage from the "Proof Control" frame origin:
//
//   stage >= 2   four owner `window_move` calls left over A: B becomes the
//                topmost overlapping window (the click that focused B raised
//                it), so its whole content draw is legal; the moves mark A's
//                whole-content Expose.
//   stage >= 3   four owner moves back to its own side: vacating the overlap
//                marks A again, and A repaints the uncovered content.
//
// B is the topmost window during both scripts, so it paints its whole content
// without a visible-part limit. It stays runnable and services its own Expose
// events until the parent sets stage 4; then it closes its window and exits.
//
// The moves are the scripted stand-in for a drag: M2 has no pointer-driven
// continuous drag loop (M2.4).

uses core, syslib, lib.out;

const
  BaseX = 560;
  BaseY = 300;
  OverX = 250;
  OverY = 430;
  StepAX = 480;
  StepAY = 330;
  StepBX = 400;
  StepBY = 360;
  StepCX = 320;
  StepCY = 400;
  ContentW = 300;
  ContentH = 240;
  BaseColor = 0x00DD66AA;
  MarkColor = 0x00552266;
  Ink = 0x00000000;

var
  id: int;
  status: Status;
  ev: Event;
  stage: int;
  done_two: bool;
  done_three: bool;
  finished: bool;
  content: Rect;

procedure paint();
begin
  status := syslib.window_content_rect(id, content);
  if status = Ok then
  begin
    status := syslib.fill_rect(
      content.x, content.y, content.w, content.h, uint(BaseColor));
    status := syslib.fill_rect(
      content.x + 20, content.y + 20, 16, 16, uint(MarkColor));
    status := syslib.draw_text(content.x + 20, content.y + 48, "B", uint(Ink));
    status := syslib.window_present(id);
  end;
end;

// Four owner moves from the side to the overlap with A.
procedure move_over();
begin
  status := syslib.window_move(id, StepAX, StepAY);
  paint();
  status := syslib.window_move(id, StepBX, StepBY);
  paint();
  status := syslib.window_move(id, StepCX, StepCY);
  paint();
  status := syslib.window_move(id, OverX, OverY);
  paint();
end;

// Four owner moves back to the side.
procedure move_back();
begin
  status := syslib.window_move(id, StepCX, StepCY);
  paint();
  status := syslib.window_move(id, StepBX, StepBY);
  paint();
  status := syslib.window_move(id, StepAX, StepAY);
  paint();
  status := syslib.window_move(id, BaseX, BaseY);
  paint();
end;

function control_stage(): int;
var
  found: bool;
  keep: bool;
  at: uint;
  cw: int;
  cr: Rect;
  ct: string[64];
  cs: Status;
  stage: int;
begin
  stage := 0;
  found := false;
  keep := true;
  at := uint(0);
  cs := syslib.window_info(at, cw, cr, ct);
  while keep and at < uint(32) do
  begin
    if cs = Ok then
    begin
      if ct = "Proof Control" then
      begin
        found := true;
        keep := false;
      end
      else
      begin
        at := at + uint(1);
        if at < uint(32) then
          cs := syslib.window_info(at, cw, cr, ct)
        else
          keep := false;
      end;
    end
    else
      keep := false;
  end;
  if found then
  begin
    if cr.y >= 680 then
      stage := 4
    else if cr.y >= 670 then
      stage := 3
    else if cr.y >= 660 then
      stage := 2
    else if cr.y >= 650 then
      stage := 1;
  end;
  return stage;
end;

procedure main(args: const string[1024]);
begin
  if syslib.screen_width() = uint(0) then
    out.say("windows_proof_b: no graphics\n")
  else
  begin
    status := syslib.window_open(BaseX, BaseY, ContentW, ContentH, "Proof B", id);
    if status = Ok then
    begin
      paint();
      done_two := false;
      done_three := false;
      finished := false;
      while not finished do
      begin
        status := syslib.poll_event(ev);
        if status = Ok and ev.kind = Expose and ev.window = id then
          paint();
        stage := control_stage();
        if stage >= 2 and not done_two then
        begin
          done_two := true;
          move_over();
        end;
        if stage >= 3 and not done_three then
        begin
          done_three := true;
          move_back();
        end;
        // Stage 4: the parent's teardown request; close and exit normally.
        if stage >= 4 then
        begin
          status := syslib.window_close(id);
          finished := true;
        end;
      end;
    end;
  end;
end;

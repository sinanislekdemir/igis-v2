unit windows_proof_a;

// M2.3.5 child A: the first of two side-by-side applications (spec 0011).
//
// It opens "Proof A" at (40, 300), draws its content and presents it. It then
// waits for the parent's command stage, read from the frame origin of the
// parent-owned "Proof Control" window (a position is a persistent command
// channel that does not use the shared event queue):
//
//   stage >= 1   a scripted drag: four owner `window_move` calls down the left
//                side, redrawing and presenting the content after each.
//
// It stays runnable and services its own `Expose` events until the parent
// sets stage 4; then it closes its window and exits. The parent's `core.wait`
// blocks until that exit, so teardown is an acknowledged exit, not a kill of a
// possibly-running task.
//
// The content is drawn only in the visible part of the content rectangle. A
// window above A with no per-task clip (M2) would be overwritten by a whole
// repaint, so A limits its fill to the part left of a higher overlapping
// window, exactly the rule `desktop_shell` uses. `window_present` then presents
// only the content's visible parts.
//
// M2 has no pointer-driven continuous drag: the four moves are the scripted
// stand-in; a real drag loop is M2.4.

uses core, syslib, lib.out;

const
  BaseX = 40;
  BaseY = 300;
  DragStep = 30;
  ContentW = 300;
  ContentH = 240;
  BaseColor = 0x0022BBDD;
  MarkColor = 0x00116633;
  Ink = 0x00000000;

var
  id: int;
  status: Status;
  ev: Event;
  stage: int;
  dragged: bool;
  finished: bool;
  content: Rect;
  probe: Rect;
  title: string[64];
  wid: int;
  marked: int;
  a_pos: int;
  b_pos: int;
  b_frame: Rect;
  saw_b: bool;
  scanning: bool;
  pos: uint;
  index: int;
  limit: int;
  width: int;

// Paints the part of the content A may own on the shadow and presents it.
// The scan finds A's z-order position and B's frame; when B is above A and
// overlaps, A stops at B's left edge so it cannot overwrite the higher window.
procedure paint();
begin
  status := syslib.window_content_rect(id, content);
  if status = Ok then
  begin
    limit := content.x + content.w;
    a_pos := -1;
    b_pos := -1;
    saw_b := false;
    scanning := true;
    pos := uint(0);
    index := 0;
    status := syslib.window_info(pos, wid, probe, title);
    while scanning and pos < uint(32) do
    begin
      if status = Ok then
      begin
        if wid = id then
          a_pos := index;
        if title = "Proof B" then
        begin
          b_pos := index;
          b_frame := probe;
          saw_b := true;
        end;
        index := index + 1;
        pos := pos + uint(1);
        if pos < uint(32) then
          status := syslib.window_info(pos, wid, probe, title)
        else
          scanning := false;
      end
      else
        scanning := false;
    end;

    if saw_b and a_pos >= 0 and b_pos >= 0 and b_pos < a_pos then
    begin
      if b_frame.x <= content.x then
        limit := content.x
      else if b_frame.x < limit then
        limit := b_frame.x;
    end;

    width := limit - content.x;
    if width > 0 then
    begin
      status := syslib.fill_rect(content.x, content.y, width, content.h, uint(BaseColor));
      if width >= 40 then
      begin
        status := syslib.fill_rect(
          content.x + 20, content.y + 20, 16, 16, uint(MarkColor));
        status := syslib.draw_text(content.x + 20, content.y + 48, "A", uint(Ink));
      end;
    end;
    status := syslib.window_present(id);
  end;
end;

// The scripted drag: four owner moves, each followed by a repaint and present.
procedure drag();
begin
  status := syslib.window_move(id, BaseX, BaseY + DragStep);
  paint();
  status := syslib.window_move(id, BaseX, BaseY + DragStep * 2);
  paint();
  status := syslib.window_move(id, BaseX, BaseY + DragStep * 3);
  paint();
  status := syslib.window_move(id, BaseX, BaseY + DragStep * 4);
  paint();
end;

// Reads the parent's command stage from the "Proof Control" frame origin.
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
    out.say("windows_proof_a: no graphics\n")
  else
  begin
    status := syslib.window_open(BaseX, BaseY, ContentW, ContentH, "Proof A", id);
    if status = Ok then
    begin
      paint();
      dragged := false;
      finished := false;
      while not finished do
      begin
        status := syslib.poll_event(ev);
        if status = Ok and ev.kind = Expose and ev.window = id then
          paint();
        stage := control_stage();
        if stage >= 1 and not dragged then
        begin
          dragged := true;
          drag();
        end;
        // Stage 4 is the parent's teardown request: close and exit normally.
        // The parent blocks in `core.wait`, so this is a deterministic exit
        // acknowledgement rather than a kill of a possibly-running task.
        if stage >= 4 then
        begin
          status := syslib.window_close(id);
          finished := true;
        end;
      end;
    end;
  end;
end;

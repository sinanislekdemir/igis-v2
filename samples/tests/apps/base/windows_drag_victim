unit windows_drag_victim;

// M8.2 close-request target (spec 0011, W23/W25): open "Close Target", paint
// and present it, then block in `next_event` until the parent's close-box
// press delivers a `Close` request. The victim closes its own window, prints
// one line and exits `Ok`; the task is never killed. The parent's `core.wait`
// observes `Ok` and the window is gone.

uses core, syslib, lib.out;

const
  WindowX = 650;
  WindowY = 80;
  ContentW = 200;
  ContentH = 120;

var
  id: int;
  status: Status;
  ev: Event;
  rect: Rect;
  closed: bool;

procedure draw();
begin
  if id > 0 then
  begin
    status := syslib.window_content_rect(id, rect);
    if status = Ok then
    begin
      status := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, uint(0x00EEDD66));
      status := syslib.draw_rect(rect.x + 2, rect.y + 2, rect.w - 4, rect.h - 4, uint(0));
      status := syslib.draw_text(rect.x + 16, rect.y + 20, "Close Target", uint(0));
      status := syslib.window_present(id);
    end;
  end;
end;

procedure main(args: const string[1024]);
begin
  id := 0;
  closed := false;
  status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Close Target", id);
  if status = Ok then
  begin
    draw();
    out.say("windows_drag_victim: ready\n");
    // Blocking reads are the normal client loop. A close-box press arrives as
    // the owner-only `Close` event; the app closes its own window and exits.
    while not closed do
    begin
      status := syslib.next_event(ev);
      if status = Ok then
      begin
        if ev.kind = Expose and ev.window = id then
          draw()
        else if ev.kind = Close and ev.window = id then
        begin
          status := syslib.window_close(id);
          id := 0;
          closed := true;
          out.say("windows_drag_victim: closed\n");
        end;
      end;
    end;
  end
  else
    out.say("windows_drag_victim: no window\n");
end;

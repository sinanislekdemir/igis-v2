unit windows_proof;

// M2.3.5 proof: two applications side by side, a focus switch and raise by a
// real injected pointer click, a scripted drag and an `Expose` redraw
// (spec 0011, W5/W6/W11/W12/W16/W17).
//
//   run windows_proof          // headless proof, prints `windows_proof: ok`
//   run windows_proof shot     // same, with a three-second composed hold
//                              // (prints `ready`) for a QMP screenshot
//
// The two apps are separate tasks (`windows_proof_a`/`windows_proof_b`), each
// owning one window. The proof cannot call another task's `window_move`, so it
// drives the scripts through a third, parent-owned window: the frame origin of
// "Proof Control" is a persistent command stage read by both children with
// `window_info`. A position is a state, not an event, so no command is lost
// and the machine-wide event queue stays free for the injected clicks.
//
// Synchronisation, and why it is deterministic on `-smp 4` and `-smp 1`:
//
//   - The parent opens the control window after spawning the children and then
//     waits for both child windows and their content pixels. A child reads the
//     persistent stage on every loop iteration, so an early command is still
//     seen; no sleep or ordering assumption is needed.
//   - Clicks use the real `syslib.inject_pointer` path. The pointer events are
//     queued machine-wide and *any* task may poll them; routing (hit-test,
//     raise, focus) happens inside the poll, so the parent asserts the
//     resulting focus and z-order, not which task received the event. A child
//     that wins the event still performs the same routing.
//   - Each scripted move is acknowledged by an observable state the parent
//     waits for: the child's final frame rectangle and its content pixels.
//     The parent sets the next stage only after those checks.
//
// The desktop is the service's white + dither pattern over the two kernel text
// windows (owner 0). A and B cover parts of those windows; when a window moves
// away, the service marks the underlying kernel window's whole-content Expose
// and `console::pump` repaints its grid. The proof checks that restoration at
// a blank terminal cell (white), so the old area really returns to the live
// window below rather than to stale application pixels.
//
// The drag is a scripted sequence of owner `window_move` calls; pointer-driven
// continuous dragging is M2.4 and is not implemented here.

uses core, syslib, lib.out;

const
  WaitMs = 5000;
  ShotHoldNs = 3000000000;
  ControlX = 900;
  ControlY = 640;
  ControlStep = 10;
  AFrameX = 40;
  AFrameY = 300;
  ADragY = 420;
  BFrameX = 560;
  BFrameY = 300;
  BOverX = 250;
  BOverY = 430;
  FrameW = 302;
  FrameH = 257;

var
  failed: bool;
  fail_step: int;
  fail_label: string[64];
  step: int;
  status: Status;
  diag: Diagnostic;
  reason: ExitReason;
  ev: Event;
  child_a: int;
  child_b: int;
  control: int;
  a: int;
  b: int;
  baseline: int;
  rect: Rect;
  screen_w: uint;
  screen_h: uint;
  white: uint;
  black: uint;
  grey: uint;
  a_base: uint;
  a_mark: uint;
  b_base: uint;
  b_mark: uint;
  control_base: uint;
  ok: bool;
  deadline: uint;
  reason_ok: bool;
  idx_a: int;
  idx_b: int;
  shot: bool;
  started: uint;
  flushed: uint;

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
  found: int;
  keep: bool;
  at: uint;
  id: int;
  r: Rect;
  t: string[64];
  s: Status;
begin
  found := 0;
  keep := true;
  at := uint(0);
  s := syslib.window_info(at, id, r, t);
  while keep and at < uint(32) do
  begin
    if s = Ok then
    begin
      if t = wanted then
      begin
        found := id;
        keep := false;
      end
      else
      begin
        at := at + uint(1);
        if at < uint(32) then
          s := syslib.window_info(at, id, r, t)
        else
          keep := false;
      end;
    end
    else
      keep := false;
  end;
  return found;
end;

// The z-order index of the first live window with this title (0 topmost), or
// -1. Used for the raise checks instead of the id, because ids are opaque.
function window_index_of(wanted: const string[0]): int;
var
  found: int;
  keep: bool;
  at: uint;
  id: int;
  r: Rect;
  t: string[64];
  s: Status;
begin
  found := -1;
  keep := true;
  at := uint(0);
  s := syslib.window_info(at, id, r, t);
  while keep and at < uint(32) do
  begin
    if s = Ok then
    begin
      if t = wanted then
      begin
        found := int(at);
        keep := false;
      end
      else
      begin
        at := at + uint(1);
        if at < uint(32) then
          s := syslib.window_info(at, id, r, t)
        else
          keep := false;
      end;
    end
    else
      keep := false;
  end;
  return found;
end;

function frame_is(id: int; x: int; y: int): bool;
var
  r: Rect;
  s: Status;
begin
  s := syslib.window_frame_rect(id, r);
  return s = Ok and r.x = x and r.y = y and r.w = FrameW and r.h = FrameH;
end;

// Repaints the parent-owned control window. It is only the command channel,
// so its pixels are not part of the proof; an expose still repaints it.
procedure draw_control();
begin
  if control > 0 then
  begin
    status := syslib.window_content_rect(control, rect);
    if status = Ok then
    begin
      status := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, control_base);
      status := syslib.draw_rect(rect.x + 2, rect.y + 2, rect.w - 4, rect.h - 4, black);
      status := syslib.window_present(control);
    end;
  end;
end;

// Drains the machine-wide queue without blocking. Pointer events are routed at
// poll time by whichever task polls them; the parent does not depend on
// receiving the click itself. A pending expose for the control is repainted.
procedure service();
begin
  status := syslib.poll_event(ev);
  while status = Ok do
  begin
    if ev.kind = Expose and control > 0 and ev.window = control then
      draw_control();
    status := syslib.poll_event(ev);
  end;
end;

procedure wait_focus(expected: int; ms: uint);
begin
  ok := false;
  deadline := now_ms() + ms;
  while not ok and now_ms() < deadline do
  begin
    if syslib.window_focus() = expected then
      ok := true
    else
      service();
  end;
end;

procedure wait_frame(id: int; x: int; y: int; ms: uint);
begin
  ok := false;
  deadline := now_ms() + ms;
  while not ok and now_ms() < deadline do
  begin
    if frame_is(id, x, y) then
      ok := true
    else
      service();
  end;
end;

procedure wait_pixel(x: int; y: int; color: uint; ms: uint);
begin
  ok := false;
  deadline := now_ms() + ms;
  while not ok and now_ms() < deadline do
  begin
    if syslib.get_pixel(x, y) = color then
      ok := true
    else
      service();
  end;
end;

// One real pointer click: clamp the pointer to (0, 0), move to the target,
// press, drain (the poll routes and raises), release.
procedure click(x: int; y: int);
begin
  status := syslib.inject_pointer(-100000, -100000, uint(0));
  status := syslib.inject_pointer(x, y, uint(0));
  status := syslib.inject_pointer(0, 0, uint(1));
  service();
  status := syslib.inject_pointer(0, 0, uint(0));
  service();
end;

// The persistent command stage: the control window's frame y.
procedure set_stage(value: int);
begin
  if control > 0 then
    status := syslib.window_move(control, ControlX, ControlY + value * ControlStep);
end;

procedure classify();
begin
  reason_ok := false;
  case reason of
    Ok: reason_ok := true
    Killed: reason_ok := false
    Faulted(code): reason_ok := false
  end;
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  step := 0;
  fail_step := 0;
  white := uint(0x00FFFFFF);
  black := uint(0x00000000);
  grey := uint(0x00CCCCCC);
  a_base := uint(0x0022BBDD);
  a_mark := uint(0x00116633);
  b_base := uint(0x00DD66AA);
  b_mark := uint(0x00552266);
  control_base := uint(0x00E8D070);
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    out.say("windows_proof: no graphics\n")
  else if screen_w < uint(960) or screen_h < uint(760) then
    out.say("windows_proof: screen too small\n")
  else
  begin
    shot := args = "shot";
    child_a := 0;
    child_b := 0;
    control := 0;

    // Baseline and a fresh input state: the pointer starts at (0, 0) and the
    // queue is empty, so the proof does not depend on an earlier app.
    baseline := syslib.window_focus();
    status := syslib.inject_pointer(-100000, -100000, uint(0));
    flushed := syslib.event_flush();

    status := core.spawn("windows_proof_a", "", diag, child_a);
    verify("spawn A", status = Ok and child_a > 0);
    status := core.spawn("windows_proof_b", "", diag, child_b);
    verify("spawn B", status = Ok and child_b > 0);

    status := syslib.window_open(ControlX, ControlY, 40, 40, "Proof Control", control);
    verify("open control", status = Ok and control > 0);
    if control > 0 then
      draw_control();

    // Wait for both applications: the windows exist and their content is
    // drawn and presented. No sleep is involved.
    a := 0;
    b := 0;
    deadline := now_ms() + uint(WaitMs);
    while (a = 0 or b = 0) and now_ms() < deadline do
    begin
      service();
      a := window_id_of("Proof A");
      b := window_id_of("Proof B");
    end;
    verify("both app windows exist", a > 0 and b > 0 and a <> b);

    if a > 0 and b > 0 then
    begin
      status := syslib.window_frame_rect(a, rect);
      verify(
        "A initial frame",
        status = Ok and rect.x = AFrameX and rect.y = AFrameY
          and rect.w = FrameW and rect.h = FrameH);
      status := syslib.window_frame_rect(b, rect);
      verify(
        "B initial frame",
        status = Ok and rect.x = BFrameX and rect.y = BFrameY
          and rect.w = FrameW and rect.h = FrameH);

      wait_pixel(61, 336, a_mark, uint(WaitMs));
      verify("A content drawn", ok);
      wait_pixel(581, 336, b_mark, uint(WaitMs));
      verify("B content drawn", ok);
      verify(
        "A and B contents differ",
        syslib.get_pixel(100, 350) = a_base and syslib.get_pixel(700, 350) = b_base);

      // The control is raised by any task: this proves any-task raise and
      // returns the focus to a known, non-app window before the first click.
      status := syslib.window_raise(control);
      verify("raise control", status = Ok and syslib.window_focus() = control);
      wait_pixel(60, 306, grey, uint(WaitMs));
      verify("A title unfocused before the click", ok);

      // 1. Click A: the unfocused window is raised and focused before the
      //    PointerDown is delivered. The focused title bar gets stripes; the
      //    other one becomes plain grey.
      click(100, 350);
      wait_focus(a, uint(WaitMs));
      verify("click focused A", ok and syslib.window_focus() = a);
      wait_pixel(60, 306, black, uint(WaitMs));
      verify("A title focused", ok);
      wait_pixel(580, 306, grey, uint(WaitMs));
      verify("B title unfocused", ok);
      idx_a := window_index_of("Proof A");
      idx_b := window_index_of("Proof B");
      verify("A raised above B", idx_a >= 0 and idx_b >= 0 and idx_a < idx_b);

      // 2. Click B: the focus switches and B is raised above A.
      click(700, 350);
      wait_focus(b, uint(WaitMs));
      verify("click focused B", ok and syslib.window_focus() = b);
      wait_pixel(580, 306, black, uint(WaitMs));
      verify("B title focused", ok);
      wait_pixel(60, 306, grey, uint(WaitMs));
      verify("A title unfocused again", ok);
      idx_a := window_index_of("Proof A");
      idx_b := window_index_of("Proof B");
      verify("B raised above A", idx_b >= 0 and idx_a >= 0 and idx_b < idx_a);

      // 3. Scripted drag: A's owner moves itself in four steps. The service
      //    restores the old frame to the terminal below (the console pump
      //    repaints it), and A paints its content at the new origin.
      set_stage(1);
      wait_frame(a, AFrameX, ADragY, uint(WaitMs));
      verify("A dragged to its new origin", ok);
      wait_pixel(61, 456, a_mark, uint(WaitMs));
      verify("A content repainted after the drag", ok);
      verify("A new frame border", syslib.get_pixel(AFrameX, ADragY + 30) = black);
      wait_pixel(100, 350, white, uint(WaitMs));
      verify("A old area restored to the terminal", ok);

      // 4. B moves over A: B is topmost, so its whole content is legal. The
      //    service marks A's whole-content Expose. The overlap shows B's
      //    content, A's visible strip stays A's, and A's chrome stays.
      set_stage(2);
      wait_frame(b, BOverX, BOverY, uint(WaitMs));
      verify("B moved over A", ok);
      wait_pixel(300, 500, b_base, uint(WaitMs));
      verify("A covered part shows B", ok);
      wait_pixel(100, 500, a_base, uint(WaitMs));
      verify("A visible strip still A", ok);
      verify("B frame border", syslib.get_pixel(BOverX, BOverY) = black);
      verify("A visible border", syslib.get_pixel(AFrameX, 470) = black);

      // 5. B moves away: the overlap it vacated is A's again only if A takes
      //    its Expose and repaints. A stale client would leave B's pixels.
      set_stage(3);
      wait_frame(b, BFrameX, BFrameY, uint(WaitMs));
      verify("B moved back to its side", ok);
      wait_pixel(300, 500, a_base, uint(WaitMs));
      verify("A repainted the uncovered content", ok);
      wait_pixel(700, 350, b_base, uint(WaitMs));
      verify("B content repainted", ok);
      // M2.5.5: the boot Terminal is now the shell's attached window at
      // (24, 64), so the restoration probe stays inside its content; the old
      // (400, 500) point is desktop dither after the kernel Terminal closed.
      wait_pixel(400, 435, white, uint(WaitMs));
      verify("B old area restored to the terminal", ok);
      verify("focus stayed on B", syslib.window_focus() = b);

      // Optional screenshot hold: the composed frame is two side-by-side
      // windows, B focused and raised above A. Only `shot` mode waits; the
      // normal proof proceeds straight to teardown.
      if shot then
      begin
        out.say("windows_proof: ready\n");
        status := core.realtime_on();
        started := core.clock();
        if status = Ok then
        begin
          while core.clock() - started < uint(ShotHoldNs) do
          begin
            core.heartbeat();
            service();
          end;
        end
        else
        begin
          while core.clock() - started < uint(ShotHoldNs) do
            service();
        end;
      end;
    end;

    // Teardown: the command stage asks both apps to close their own windows
    // and exit. `core.wait` then blocks until each really terminates, so the
    // proof also covers close-on-exit. A graceful exit is used instead of
    // `kill`: the children are runnable busy-pollers, so a stage command is a
    // deterministic request, while `kill` of a task that is being claimed on
    // another core has no synchronous acknowledgement in M2.
    set_stage(4);
    if child_a > 0 then
    begin
      status := core.wait(child_a, reason);
      classify();
      verify("A exited Ok", status = Ok and reason_ok);
    end;
    if child_b > 0 then
    begin
      status := core.wait(child_b, reason);
      classify();
      verify("B exited Ok", status = Ok and reason_ok);
    end;

    status := syslib.window_frame_rect(a, rect);
    verify("A window closed with its task", status = NotFound);
    status := syslib.window_frame_rect(b, rect);
    verify("B window closed with its task", status = NotFound);

    if control > 0 then
    begin
      status := syslib.window_close(control);
      verify("close control", status = Ok);
    end;
    verify("focus fell back", syslib.window_focus() = baseline);

    if failed then
    begin
      out.say("windows_proof: FAIL at step ");
      out.say_int(fail_step);
      out.say(": ");
      out.say(fail_label);
      out.say("\n");
    end
    else
      out.say("windows_proof: ok\n");
  end;
end;

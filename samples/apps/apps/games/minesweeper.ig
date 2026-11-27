unit minesweeper;

// M5.2: a windowed minesweeper over the existing UI toolkit.
//
//   run minesweeper          // interactive: click or type, blocks in `next_event`
//   run minesweeper check    // deterministic checks, prints `minesweeper: ok`
//   run minesweeper shot     // composes a mid-game board, prints
//                            // `minesweeper: ready` and holds ~5 s for the QMP
//                            // screenshot harness
//
// One window (content 300x400, title `Minesweeper`): a white client panel with a
// one-pixel bezel, a 30-pixel header (a three-digit mine counter, a 30x30
// face/restart button and an MM:SS timer), a 9x9 field of 30-pixel cells at
// (8, 52) with ten mines, and a 120x28 `Flag` mode button with a status line
// below the field. Everything is client content drawn through `syslib` and
// `stdlib`: no `syslib.clear`, one complete frame before one `window_present`,
// and an owner `Expose` repaints the whole content.
//
// Mines are placed by an app-local LCG (`x = (x * 1103515245 + 12345) mod 2^31`)
// from a seed: `core.clock()` in play mode and a fixed seed in `check`/`shot`.
// The first reveal is never a mine: a mine under the first clicked cell is
// moved to the first free cell in scan order before the neighbours are
// counted. Left-clicking a closed cell reveals it; a zero-neighbour cell
// floods its region through an iterative stack (`array[int, 81]`). The right
// button or the `Flag` mode button toggles a flag (ten at most); a flagged or
// already-open cell is a no-op. Revealing a mine loses and shows every mine;
// revealing all 71 safe cells wins. The face (or `R`) restarts, `F` toggles
// flag mode and Ctrl+Q closes the window and exits, so the task never outlives
// the shell.
//
// `langhost` and a kernel without a framebuffer model a disabled screen, so the
// sample runs the same placement, counting, first-click safety, flood and win
// logic there and prints `minesweeper: host ok`.

uses core, stdlib, syslib, lib.out, lib.inject;

const
  WindowX = 150;
  WindowY = 80;
  ContentW = 300;
  ContentH = 400;

  // Header: counter at the left, a 30x30 face in the middle, the timer at the
  // right (five 8-pixel glyphs plus the 8-pixel content margin).
  CounterX = 8;
  CounterY = 15;
  FaceX = 135;
  FaceY = 8;
  FaceSize = 30;
  TimerX = 252;
  TimerY = 15;

  // Field: 9 columns of 30-pixel cells, 270x270 at (8, 52).
  FieldX = 8;
  FieldY = 52;
  CellSize = 30;
  GridN = 9;
  CellCount = 81;
  MineCount = 10;
  SafeCount = 71;

  // Below the field: the flag-mode button and the status line.
  FlagX = 8;
  FlagY = 336;
  FlagW = 120;
  FlagH = 28;
  StatusX = 136;
  StatusY = 346;

  // Deterministic board for `check`/`shot`; the LCG keeps its state in an int.
  FixedSeed = 12345;
  LcgModulus = 2147483648;
  LcgMultiplier = 1103515245;
  LcgIncrement = 12345;

  // Shot mode hold.
  HoldSeconds = 5;
  NanosPerSecond = 1000000000;

  KeyQ = 0x71;
  KeyR = 82;
  KeyF = 70;

var
  // Board. `counts` is zero on a mine and holds the neighbour count elsewhere.
  mine: array[bool, CellCount];
  opened: array[bool, CellCount];
  flagged: array[bool, CellCount];
  counts: array[int, CellCount];
  stack: array[int, CellCount];

  // Game state.
  seed: int;
  fixed_seed: bool;
  mines_placed: bool;
  started: bool;
  lost: bool;
  won: bool;
  flags: int;
  safe_open: int;
  start_ns: uint;
  stop_ns: uint;
  flag_mode: bool;
  running: bool;
  focus: int;

  // Text staged for drawing.
  counter_text: string[8];
  timer_text: string[8];
  status_text: string[32];
  face_text: string[4];
  flag_text: string[8];
  digits: string[8];

  // Window and absolute geometry.
  id: int;
  // The content rectangle: drawing is window-local (spec 0026 C8), but the
  // debug pointer hook takes screen coordinates (spec 0010 I35).
  origin: Rect;

  // Palette and event pump.
  black: uint;
  white: uint;
  gray: uint;
  status: Status;
  ignored: Status;
  dirty: bool;
  ev: Event;

  // Check reporting and injection scratch.
  failed: bool;
  step: int;
  written: int;
  known_total: int;
  test_mine: int;
  test_zero: int;
  test_moved: int;
  test_cell: int;
  test_number: int;
  test_ink: int;
  t0: int;
  t1: int;
  started_ns: uint;
  at: int;
  screen_w: uint;
  screen_h: uint;

// Silent check: every failure prints its step number and label.
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed then
  begin
    failed := true;
    out.say("minesweeper: failed check ");
    out.say_int(step);
    out.say(" (");
    out.say(label);
    out.say(")\n");
  end;
end;

// ---- board ---------------------------------------------------------------

procedure clear_cells();
var
  i: int;
begin
  i := 0;
  while i < CellCount do
  begin
    mine[i] := false;
    opened[i] := false;
    flagged[i] := false;
    counts[i] := 0;
    i := i + 1;
  end;
end;

// Fills `mine` from the app-local LCG. The state stays below 2^31 so the
// product fits an `int` (the language has no bitwise operators and `mod` is the
// only reduction needed).
procedure place_mines(value: int);
var
  x: int;
  idx: int;
  placed: int;
begin
  x := value mod LcgModulus;
  if x < 0 then
    x := x + LcgModulus;
  placed := 0;
  while placed < MineCount do
  begin
    x := (x * LcgMultiplier + LcgIncrement) mod LcgModulus;
    idx := x mod CellCount;
    if not mine[idx] then
    begin
      mine[idx] := true;
      placed := placed + 1;
    end;
  end;
end;

function in_grid(r: int; c: int): bool;
begin
  return r >= 0 and r < GridN and c >= 0 and c < GridN;
end;

// Counts the mines around every safe cell; a mine keeps count 0.
procedure compute_counts();
var
  i: int;
  r: int;
  c: int;
  dr: int;
  dc: int;
  nr: int;
  nc: int;
  n: int;
begin
  i := 0;
  while i < CellCount do
  begin
    counts[i] := 0;
    if not mine[i] then
    begin
      r := i div GridN;
      c := i mod GridN;
      n := 0;
      dr := -1;
      while dr <= 1 do
      begin
        dc := -1;
        while dc <= 1 do
        begin
          if not (dr = 0 and dc = 0) then
          begin
            nr := r + dr;
            nc := c + dc;
            if in_grid(nr, nc) then
              if mine[nr * GridN + nc] then
                n := n + 1;
          end;
          dc := dc + 1;
        end;
        dr := dr + 1;
      end;
      counts[i] := n;
    end;
    i := i + 1;
  end;
end;

function mine_total(): int;
var
  i: int;
  n: int;
begin
  n := 0;
  i := 0;
  while i < CellCount do
  begin
    if mine[i] then
      n := n + 1;
    i := i + 1;
  end;
  return n;
end;

function count_flags(): int;
var
  i: int;
  n: int;
begin
  n := 0;
  i := 0;
  while i < CellCount do
  begin
    if flagged[i] then
      n := n + 1;
    i := i + 1;
  end;
  return n;
end;

// The first reveal is never a mine: move a mine under `first` to the first
// free cell in scan order. Called before `compute_counts`, so every number
// reflects the final board.
procedure make_first_safe(first: int);
var
  i: int;
  placed: bool;
begin
  if not mine[first] then
    return;
  mine[first] := false;
  placed := false;
  i := 0;
  while not placed and i < CellCount do
  begin
    if not mine[i] then
    begin
      mine[i] := true;
      placed := true;
    end;
    i := i + 1;
  end;
end;

// ---- state ---------------------------------------------------------------

procedure update_face();
begin
  if lost then
    face_text := ":("
  else if won then
    face_text := "B)"
  else
    face_text := ":)";
end;

procedure update_status();
begin
  if lost then
    status_text := "Boom"
  else if won then
    status_text := "You win"
  else if not mines_placed then
    status_text := "Click to start"
  else
  begin
    written := core.format_int_into(digits, flags, uint(10), uint(0));
    status_text := "Flags: " + digits;
  end;
end;

// A three-digit counter; a negative value (unreachable with the ten-flag cap)
// is clamped to zero so the field is always three glyphs.
procedure format_counter(dst: var string[8]; value: int);
var
  v: int;
begin
  v := value;
  if v < 0 then
    v := 0;
  if v > MineCount then
    v := MineCount;
  written := core.format_int_into(dst, v, uint(10), uint(3));
end;

// `MM:SS`, clamped at 99:59 so the five-glyph band never overflows.
procedure format_time(dst: var string[8]; total: int);
var
  mm: string[4];
  ss: string[4];
  mid: string[5];
  t: int;
begin
  t := total;
  if t < 0 then
    t := 0;
  if t > 5999 then
    t := 5999;
  written := core.format_int_into(mm, t div 60, uint(10), uint(2));
  written := core.format_int_into(ss, t mod 60, uint(10), uint(2));
  mid := mm + ":";
  dst := mid + ss;
end;

function timer_seconds(): int;
var
  now: uint;
  span: uint;
begin
  if not started then
    return 0;
  if lost or won then
    now := stop_ns
  else
    now := core.clock();
  if now < start_ns then
    return 0;
  span := (now - start_ns) div uint(NanosPerSecond);
  if span > uint(5999) then
    span := uint(5999);
  return int(span);
end;

// A fresh board. `check`/`shot` keep the fixed seed; play mode reseeds from the
// clock so a restart is a different game.
procedure restart();
begin
  clear_cells();
  if fixed_seed then
    seed := FixedSeed
  else
    seed := int(core.clock() mod uint(LcgModulus));
  mines_placed := false;
  started := false;
  lost := false;
  won := false;
  flags := 0;
  safe_open := 0;
  start_ns := uint(0);
  stop_ns := uint(0);
  flag_mode := false;
  focus := 0;
  status_text := "Click to start";
  face_text := ":)";
end;

// The first reveal: place the board, guarantee a safe first cell and count.
procedure begin_play(first: int);
begin
  place_mines(seed);
  make_first_safe(first);
  compute_counts();
  mines_placed := true;
  started := true;
  start_ns := core.clock();
  stop_ns := uint(0);
end;

procedure toggle_flag(index: int);
begin
  if lost or won then
    return;
  if opened[index] then
    return;
  if flagged[index] then
  begin
    flagged[index] := false;
    flags := flags - 1;
  end
  else if flags < MineCount then
  begin
    flagged[index] := true;
    flags := flags + 1;
  end;
  update_status();
end;

procedure lose();
var
  i: int;
begin
  lost := true;
  stop_ns := core.clock();
  i := 0;
  while i < CellCount do
  begin
    if mine[i] then
    begin
      opened[i] := true;
      flagged[i] := false;
    end;
    i := i + 1;
  end;
  flags := count_flags();
  update_face();
  update_status();
end;

procedure check_win();
begin
  if not lost and not won and safe_open >= SafeCount then
  begin
    won := true;
    stop_ns := core.clock();
    update_face();
    update_status();
  end;
end;

// Reveals one cell. A zero-neighbour cell floods its region with an iterative
// stack: a cell is marked open when it is pushed, so each of the 81 cells is
// pushed at most once and the stack cannot overflow.
procedure reveal(index: int);
var
  top: int;
  i: int;
  j: int;
  r: int;
  c: int;
  dr: int;
  dc: int;
  nr: int;
  nc: int;
begin
  if lost or won then
    return;
  if opened[index] or flagged[index] then
    return;
  opened[index] := true;
  safe_open := safe_open + 1;
  if mine[index] then
  begin
    lose();
    return;
  end;
  top := 0;
  if counts[index] = 0 then
  begin
    stack[0] := index;
    top := 1;
  end;
  while top > 0 do
  begin
    top := top - 1;
    i := stack[top];
    r := i div GridN;
    c := i mod GridN;
    dr := -1;
    while dr <= 1 do
    begin
      dc := -1;
      while dc <= 1 do
      begin
        if not (dr = 0 and dc = 0) then
        begin
          nr := r + dr;
          nc := c + dc;
          if in_grid(nr, nc) then
          begin
            j := nr * GridN + nc;
            if not opened[j] and not flagged[j] then
            begin
              opened[j] := true;
              safe_open := safe_open + 1;
              if mine[j] then
              begin
                lose();
                return;
              end;
              if counts[j] = 0 then
              begin
                stack[top] := j;
                top := top + 1;
              end;
            end;
          end;
        end;
        dc := dc + 1;
      end;
      dr := dr + 1;
    end;
  end;
  check_win();
end;

procedure left_click(index: int);
begin
  if lost or won then
    return;
  if not mines_placed then
    begin_play(index);
  reveal(index);
  update_status();
end;

// ---- drawing -------------------------------------------------------------

procedure cell_rect(index: int; x: var int; y: var int);
begin
  // Window-local (spec 0026 C8): cells and header derive from the constants.
  x := FieldX + (index mod GridN) * CellSize;
  y := FieldY + (index div GridN) * CellSize;
end;

// Closed cell: gray face, white top/left bevel, black outline.
procedure draw_closed(x: int; y: int);
begin
  ignored := syslib.fill_rect(x + 1, y + 1, CellSize - 2, CellSize - 2, gray);
  ignored := syslib.fill_rect(x + 1, y + 1, CellSize - 2, 2, white);
  ignored := syslib.fill_rect(x + 1, y + 1, 2, CellSize - 2, white);
  ignored := syslib.draw_rect(x, y, CellSize, CellSize, black);
end;

// A small black flag: pole, three-row pennant and a base.
procedure draw_flag(x: int; y: int);
begin
  ignored := syslib.fill_rect(x + 14, y + 7, 2, 15, black);
  ignored := syslib.fill_rect(x + 16, y + 7, 8, 2, black);
  ignored := syslib.fill_rect(x + 16, y + 9, 6, 2, black);
  ignored := syslib.fill_rect(x + 16, y + 11, 4, 2, black);
  ignored := syslib.fill_rect(x + 11, y + 21, 10, 2, black);
end;

// A black mine: an octagonal body with four spikes. The cell centre is ink.
procedure draw_mine(x: int; y: int);
begin
  ignored := syslib.fill_rect(x + 14, y + 5, 2, 20, black);
  ignored := syslib.fill_rect(x + 5, y + 14, 20, 2, black);
  ignored := syslib.fill_rect(x + 11, y + 9, 8, 2, black);
  ignored := syslib.fill_rect(x + 9, y + 11, 12, 8, black);
  ignored := syslib.fill_rect(x + 11, y + 19, 8, 2, black);
end;

procedure draw_number(x: int; y: int; n: int);
var
  t: string[4];
begin
  written := core.format_int_into(t, n, uint(10), uint(0));
  ignored := syslib.draw_text(x + 11, y + 11, t, black);
end;

// Open cell: white face with a black outline, then the mine or the number.
procedure draw_open(index: int; x: int; y: int);
begin
  ignored := syslib.fill_rect(x + 1, y + 1, CellSize - 2, CellSize - 2, white);
  ignored := syslib.draw_rect(x, y, CellSize, CellSize, black);
  if mine[index] then
    draw_mine(x, y)
  else if counts[index] > 0 then
    draw_number(x, y, counts[index]);
end;

procedure draw_cell(index: int);
var
  x: int;
  y: int;
begin
  cell_rect(index, x, y);
  if opened[index] then
    draw_open(index, x, y)
  else
  begin
    draw_closed(x, y);
    if flagged[index] then
      draw_flag(x, y);
  end;
end;

// One complete frame; the caller presents once afterwards.
procedure draw_frame();
var
  i: int;
begin
  ignored := syslib.fill_rect(0, 0, ContentW, ContentH, white);
  ignored := stdlib.ui_frame(0, 0, ContentW, ContentH);

  update_status();
  update_face();
  format_counter(counter_text, MineCount - flags);
  ignored := syslib.draw_text(
    CounterX, CounterY, counter_text, black);
  ignored := stdlib.ui_button(
    FaceX, FaceY, FaceSize, FaceSize, face_text, false, focus = 0);
  format_time(timer_text, timer_seconds());
  ignored := syslib.draw_text(
    TimerX, TimerY, timer_text, black);

  i := 0;
  while i < CellCount do
  begin
    draw_cell(i);
    i := i + 1;
  end;

  if flag_mode then
    flag_text := "Flag [F]"
  else
    flag_text := "Flag";
  ignored := stdlib.ui_button(
    FlagX, FlagY, FlagW, FlagH, flag_text, flag_mode, focus = 1);

  ignored := syslib.draw_text(
    StatusX, StatusY, status_text, black);
end;

procedure present_frame();
begin
  ignored := syslib.window_present(id);
end;

// ---- input ---------------------------------------------------------------

// The button mask: `pointer_buttons()` is the live state, and `Event.code`
// carries the same pressed bits for a pointer event (spec 0010), so a release
// that was already queued cannot turn a right-click into a left-click.
function pressed_mask(): int;
var
  mask: int;
begin
  mask := int(syslib.pointer_buttons() mod uint(8));
  if mask = 0 then
    mask := int(ev.code mod uint(8));
  return mask;
end;

procedure press_at(px: int; py: int; right: bool);
var
  r: int;
  c: int;
  index: int;
begin
  if stdlib.ui_button_hit(
    FaceX, FaceY, FaceSize, FaceSize, px, py) then
  begin
    focus := 0;
    restart();
  end
  else if stdlib.ui_button_hit(
    FlagX, FlagY, FlagW, FlagH, px, py) then
  begin
    focus := 1;
    flag_mode := not flag_mode;
    update_status();
  end
  else if stdlib.ui_rect_hit(
    FieldX, FieldY, GridN * CellSize, GridN * CellSize, px, py) then
  begin
    r := (py - FieldY) div CellSize;
    c := (px - FieldX) div CellSize;
    index := r * GridN + c;
    if right or flag_mode then
      toggle_flag(index)
    else
      left_click(index);
  end;
end;

procedure handle_pointer();
begin
  press_at(ev.x, ev.y, (pressed_mask() div 2) mod 2 = 1);
end;

procedure handle_key();
var
  kc: int;
  mods: int;
  b: byte;
begin
  kc := stdlib.ui_key_code(ev.code);
  mods := stdlib.ui_key_modifiers(ev.code);
  b := stdlib.ui_key_text(ev.code);
  // A real Ctrl+Q carries key code 'Q' (81) and the Ctrl text byte 0x11; the
  // synthetic check event uses the lower-case code. Accept both shapes.
  if (mods div 2) mod 2 = 1 then
    if kc = KeyQ or kc = 81 or int(b) = 17 then
    begin
      running := false;
      return;
    end;
  if kc = KeyR or int(b) = 114 or int(b) = 82 then
    restart()
  else if kc = KeyF or int(b) = 102 or int(b) = 70 then
  begin
    flag_mode := not flag_mode;
    update_status();
  end;
end;

// Poll and handle every queued event, then repaint once if anything changed.
procedure drain_events();
var
  more: bool;
  count: int;
begin
  more := true;
  count := 0;
  while more and count < 64 do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      count := count + 1;
      if ev.kind = Expose then
      begin
        if ev.window = id then
        begin
          dirty := true;
        end;
      end
      else if ev.kind = Close then
      begin
        // A close request closes only this window (spec 0011, W23); the
        // interactive loop exits.
        if ev.window = id then
          more := false;
      end
      else if ev.kind = KeyDown then
      begin
        handle_key();
        dirty := true;
      end
      else if ev.kind = PointerDown then
      begin
        handle_pointer();
        dirty := true;
      end
      else if ev.kind = PointerUp or ev.kind = PointerMove then
        dirty := true;
    end
    else
      more := false;
  end;
  if dirty then
  begin
    draw_frame();
    present_frame();
    dirty := false;
  end;
end;

// Interactive mode: draw once, then block for one event at a time.
procedure interactive();
begin
  draw_frame();
  present_frame();
  while running do
  begin
    status := syslib.next_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose then
      begin
        if ev.window = id then
        begin
          draw_frame();
          present_frame();
        end;
      end
      else if ev.kind = Close then
      begin
        if ev.window = id then
          running := false;
      end
      else if ev.window = id then
      begin
        if ev.kind = KeyDown then
          handle_key()
        else if ev.kind = PointerDown then
          handle_pointer();
        draw_frame();
        present_frame();
      end;
    end;
  end;
  status := syslib.window_close(id);
end;

// ---- check mode ----------------------------------------------------------

procedure cell_center(index: int; cx: var int; cy: var int);
var
  x: int;
  y: int;
begin
  cell_rect(index, x, y);
  cx := x + CellSize div 2;
  cy := y + CellSize div 2;
end;

procedure click_at(px: int; py: int);
begin
  // `px`/`py` are window-local; the pointer state is screen-coordinate
  // (spec 0010 I35), so the content origin is added here.
  status := inject.pointer_to(origin.x + px, origin.y + py);
  drain_events();
  status := inject.pointer_down();
  drain_events();
  status := inject.pointer_up();
  drain_events();
end;

procedure click_cell(index: int);
var
  x: int;
  y: int;
begin
  cell_center(index, x, y);
  click_at(x, y);
end;

// A right-button click: place the pointer, inject the secondary mask (bit 1),
// drain, then release. The app reads `pointer_buttons()` on the down event.
procedure right_click_cell(index: int);
var
  x: int;
  y: int;
begin
  cell_center(index, x, y);
  status := inject.pointer_to(origin.x + x, origin.y + y);
  drain_events();
  status := syslib.inject_pointer(0, 0, uint(2));
  drain_events();
  status := syslib.inject_pointer(0, 0, uint(0));
  drain_events();
end;

procedure key(key_code: int; text_byte: int);
begin
  status := inject.inject_key(key_code, text_byte);
  drain_events();
end;

// Places the fixed board once to learn its mine/zero/number cells, then clears
// it. Placement is a pure function of the seed, so the real first reveal
// rebuilds exactly the same board.
procedure prepare_known();
var
  i: int;
begin
  restart();
  place_mines(seed);
  compute_counts();
  known_total := mine_total();
  test_mine := -1;
  test_zero := -1;
  test_moved := -1;
  i := 0;
  while i < CellCount do
  begin
    if mine[i] and test_mine < 0 then
      test_mine := i;
    if not mine[i] and counts[i] = 0 and test_zero < 0 then
      test_zero := i;
    if not mine[i] and test_moved < 0 then
      test_moved := i;
    i := i + 1;
  end;
  clear_cells();
  mines_placed := false;
  started := false;
  lost := false;
  won := false;
  flags := 0;
  safe_open := 0;
end;

procedure first_open_cell_with_number();
var
  i: int;
begin
  test_number := -1;
  i := 0;
  while i < CellCount do
  begin
    if opened[i] and counts[i] > 0 and test_number < 0 then
      test_number := i;
    i := i + 1;
  end;
end;

procedure first_closed_cell();
var
  i: int;
begin
  test_cell := -1;
  i := 0;
  while i < CellCount do
  begin
    if not opened[i] and not flagged[i] and test_cell < 0 then
      test_cell := i;
    i := i + 1;
  end;
end;

// The 8x8 digit rows start with ink at a known column for every 1..8 glyph;
// this is the offset used for the "number ink" pixel check.
function digit_ink_offset(n: int): int;
begin
  if n = 1 then
    return 2;
  if n = 2 or n = 3 or n = 8 then
    return 1;
  if n = 4 then
    return 3;
  if n = 6 then
    return 2;
  return 0;
end;

// The kernel proof: real pointer/key events drive the same handlers the
// interactive loop uses, and every result is asserted as game state plus a
// representative pixel in the shadow.
procedure check_mode();
var
  x: int;
  y: int;
  i: int;
  n: int;
  ok: bool;
begin
  draw_frame();
  present_frame();

  // Baseline: empty board, header, face and status.
  verify("content white", syslib.get_pixel(2, 2) = white);
  verify("bezel black", syslib.get_pixel(0, 20) = black);
  cell_rect(0, x, y);
  verify("closed outline", syslib.get_pixel(x, y) = black);
  verify("closed face", syslib.get_pixel(x + 5, y + 5) = gray);
  verify("closed bevel", syslib.get_pixel(x + 2, y + 2) = white);
  verify("counter text", counter_text = "010");
  verify("counter ink", syslib.get_pixel(CounterX + 1, CounterY) = black);
  verify("timer text", timer_text = "00:00");
  verify("timer ink", syslib.get_pixel(TimerX + 1, TimerY) = black);
  verify("face playing", face_text = ":)");
  verify("face border", syslib.get_pixel(FaceX, FaceY) = black);
  verify("face interior", syslib.get_pixel(FaceX + 2, FaceY + 2) = white);
  verify("face ink", syslib.get_pixel(FaceX + 9, FaceY + 12) = black);
  verify("status start", status_text = "Click to start");
  verify("status ink", syslib.get_pixel(StatusX + 2, StatusY) = black);
  verify("flag button off", not flag_mode);

  // 1. The fixed board: ten mines, a zero cell and a numbered cell.
  prepare_known();
  verify("known mines", known_total = MineCount);
  verify("known mine", test_mine >= 0);
  verify("known zero", test_zero >= 0);
  verify("known moved", test_moved >= 0);

  // 2. A zero cell floods its region (the fixed seed opens six cells).
  click_cell(test_zero);
  verify("first click starts", mines_placed and started and not lost);
  verify("flood zero open", opened[test_zero]);
  verify("flood size", safe_open = 6);
  verify("flood row", opened[1] and opened[9] and opened[10]);
  verify("flood edge", opened[18] and opened[19]);
  verify("flood stops", not opened[2]);
  first_open_cell_with_number();
  verify("flood frontier number", test_number >= 0);
  cell_rect(test_zero, x, y);
  verify("open cell white", syslib.get_pixel(x + 10, y + 10) = white);
  cell_rect(test_number, x, y);
  n := counts[test_number];
  test_ink := digit_ink_offset(n);
  verify(
    "number ink",
    syslib.get_pixel(x + 11 + test_ink, y + 11) = black);
  cell_rect(2, x, y);
  verify("closed mine gray", syslib.get_pixel(x + 10, y + 10) = gray);
  verify("play status", status_text = "Flags: 0");
  verify("play status ink", syslib.get_pixel(StatusX, StatusY) = black);
  verify("timer running", timer_seconds() >= 0);

  // 3. First-click safety: clicking a known mine moves it to the first free
  // cell and never loses.
  prepare_known();
  test_cell := test_mine;
  click_cell(test_cell);
  verify("first click safe", not lost and opened[test_cell]);
  verify("first click unmined", not mine[test_cell]);
  verify("mine moved", mine[test_moved]);
  verify("mines kept", mine_total() = MineCount);
  verify("first click opened", safe_open >= 1);

  // 4. A right-button click flags and unflags a closed cell.
  prepare_known();
  click_cell(test_zero);
  first_closed_cell();
  verify("flag target closed", test_cell >= 0 and not opened[test_cell]);
  cell_center(test_cell, x, y);
  status := inject.pointer_to(origin.x + x, origin.y + y);
  drain_events();
  status := syslib.inject_pointer(0, 0, uint(2));
  verify("right mask", syslib.pointer_buttons() = uint(2));
  drain_events();
  status := syslib.inject_pointer(0, 0, uint(0));
  drain_events();
  verify("right flag set", flagged[test_cell] and flags = 1);
  verify("right flag closed", not opened[test_cell]);
  verify("flag status", status_text = "Flags: 1");
  cell_rect(test_cell, x, y);
  verify("flag ink", syslib.get_pixel(x + 16, y + 7) = black);
  right_click_cell(test_cell);
  verify("right flag cleared", not flagged[test_cell] and flags = 0);
  cell_rect(test_cell, x, y);
  verify("flag gone", syslib.get_pixel(x + 16, y + 7) = gray);

  // 5. Flag mode: the button turns it on and a left click flags.
  click_at(FlagX + FlagW div 2, FlagY + FlagH div 2);
  verify("flag mode on", flag_mode and focus = 1);
  verify("flag button pressed", syslib.get_pixel(FlagX + 2, FlagY + 2) = gray);
  first_closed_cell();
  cell_center(test_cell, x, y);
  click_at(x, y);
  verify("mode flags", flagged[test_cell] and not opened[test_cell] and flags = 1);
  click_at(x, y);
  verify("mode unflags", not flagged[test_cell] and flags = 0);
  click_at(FlagX + FlagW div 2, FlagY + FlagH div 2);
  verify("flag mode off", not flag_mode);
  verify("flag button released", syslib.get_pixel(FlagX + 2, FlagY + 2) = white);

  // 6. Flags are capped at ten.
  prepare_known();
  click_cell(test_zero);
  i := 0;
  while i < CellCount and flags < MineCount do
  begin
    if not opened[i] and not flagged[i] then
      toggle_flag(i);
    i := i + 1;
  end;
  verify("flag cap reached", flags = MineCount);
  first_closed_cell();
  verify("eleventh target", test_cell >= 0);
  toggle_flag(test_cell);
  verify("flag cap holds", flags = MineCount and not flagged[test_cell]);
  format_counter(counter_text, MineCount - flags);
  verify("counter zero", counter_text = "000");
  format_counter(counter_text, -1);
  verify("counter clamp", counter_text = "000");

  // 7. A revealed mine loses, shows every mine and stops the timer.
  prepare_known();
  click_cell(test_zero);
  test_cell := test_mine;
  click_cell(test_cell);
  verify("loss state", lost and not won);
  verify("loss face", face_text = ":(");
  verify("loss status", status_text = "Boom");
  verify("loss opens the mine", opened[test_cell]);
  ok := true;
  i := 0;
  while i < CellCount do
  begin
    if mine[i] and not opened[i] then
      ok := false;
    i := i + 1;
  end;
  verify("loss shows every mine", ok);
  cell_rect(test_cell, x, y);
  verify("mine ink", syslib.get_pixel(x + 15, y + 15) = black);
  verify("boom ink", syslib.get_pixel(StatusX, StatusY) = black);
  t0 := timer_seconds();
  at := 0;
  while at < 200000 do
    at := at + 1;
  t1 := timer_seconds();
  verify("loss freezes the timer", t1 = t0);

  // 8. Revealing every safe cell wins.
  prepare_known();
  click_cell(test_zero);
  i := 0;
  while i < CellCount do
  begin
    if not mine[i] then
      reveal(i);
    i := i + 1;
  end;
  verify("win state", won and not lost);
  verify("win opens all safe cells", safe_open = SafeCount);
  ok := true;
  i := 0;
  while i < CellCount do
  begin
    if not mine[i] and not opened[i] then
      ok := false;
    i := i + 1;
  end;
  verify("win leaves no safe cell closed", ok);
  verify("win face", face_text = "B)");
  verify("win status", status_text = "You win");
  draw_frame();
  present_frame();
  verify("win ink", syslib.get_pixel(StatusX, StatusY) = black);
  t0 := timer_seconds();
  at := 0;
  while at < 200000 do
    at := at + 1;
  t1 := timer_seconds();
  verify("win freezes the timer", t1 = t0);

  // 9. The face restarts: counters and timer reset.
  click_at(FaceX + FaceSize div 2, FaceY + FaceSize div 2);
  verify("restart state", not mines_placed and not started and not lost and not won);
  verify("restart flags", flags = 0 and safe_open = 0);
  verify("restart timer", timer_seconds() = 0 and timer_text = "00:00");
  verify("restart face", face_text = ":)");
  verify("restart status", status_text = "Click to start");
  cell_rect(0, x, y);
  verify("restart closed", not opened[0] and syslib.get_pixel(x + 5, y + 5) = gray);

  // 10. Keys: F toggles flag mode, R restarts and Ctrl+Q ends the loop.
  key(KeyF, 102);
  verify("key f on", flag_mode);
  key(KeyF, 102);
  verify("key f off", not flag_mode);
  click_cell(test_zero);
  verify("key r start", mines_placed);
  key(KeyR, 114);
  verify("key r restart", not mines_placed and flags = 0 and not started);
  running := true;
  status := inject.inject_key_mods(KeyQ, 0, 2);
  drain_events();
  verify("ctrl q quits", not running);
  running := true;
  // The real keyboard shape: key code 'Q' (81) with the Ctrl text byte 0x11.
  status := inject.inject_key_mods(81, 17, 2);
  drain_events();
  verify("ctrl q real shape", not running);
  running := true;

  // 11. Pure timer formatting and a non-decreasing running clock.
  format_time(timer_text, 0);
  verify("time zero", timer_text = "00:00");
  format_time(timer_text, 5);
  verify("time five", timer_text = "00:05");
  format_time(timer_text, 65);
  verify("time minute", timer_text = "01:05");
  format_time(timer_text, 599);
  verify("time nine", timer_text = "09:59");
  format_time(timer_text, 3599);
  verify("time fiftynine", timer_text = "59:59");
  format_time(timer_text, 99999);
  verify("time clamp", timer_text = "99:59");
  prepare_known();
  click_cell(test_zero);
  t0 := timer_seconds();
  at := 0;
  while at < 200000 do
    at := at + 1;
  t1 := timer_seconds();
  verify("timer non-decreasing", t1 >= t0);

  status := syslib.window_close(id);
  verify("window closed", status = Ok);

  if failed then
    out.say("minesweeper: failed\n")
  else
    out.say("minesweeper: ok\n");
end;

// ---- shot mode -----------------------------------------------------------

// Composes a real mid-game board: the fixed seed's zero cell floods, two closed
// cells are flagged and flag mode is on. One frame, then hold without drawing.
procedure shot_mode();
begin
  left_click(0);
  toggle_flag(77);
  toggle_flag(80);
  flag_mode := true;
  focus := 1;
  update_status();
  draw_frame();
  present_frame();
  out.say("minesweeper: ready\n");
  started_ns := core.clock();
  while core.clock() - started_ns < uint(HoldSeconds * NanosPerSecond) do
    at := 0;
  status := syslib.window_close(id);
  out.say("minesweeper: done\n");
end;

// ---- host path -----------------------------------------------------------

// The disabled-screen path: placement, counts, first-click safety, flood,
// flags, win/loss and the timer text are pure and must agree on every host.
procedure host_path();
var
  i: int;
  ok: bool;
begin
  verify("initial board empty", not mines_placed and not started and flags = 0);
  verify("initial face", face_text = ":)");

  // Placement is deterministic and counts every neighbour.
  restart();
  place_mines(seed);
  verify("host mines", mine_total() = MineCount);
  compute_counts();
  verify("host zero count", counts[0] = 0);
  verify("host neighbour count", counts[1] = 1);
  verify("host mine count zero", counts[2] = 0);
  verify("host count bounded", counts[24] >= 0 and counts[24] <= 8);

  // First-click safety moves a mine instead of losing.
  prepare_known();
  left_click(test_mine);
  verify("host first safe", not lost and opened[test_mine] and not mine[test_mine]);
  verify("host mine moved", mine[test_moved]);
  verify("host mine kept", mine_total() = MineCount);

  // A zero cell floods its fixed region.
  restart();
  left_click(0);
  verify("host flood", safe_open = 6 and opened[0] and opened[19] and not opened[2]);
  verify("host play status", status_text = "Flags: 0");

  // A flag toggles on a closed cell, never on an open one, and caps at ten.
  toggle_flag(2);
  verify("host flag", flagged[2] and flags = 1);
  toggle_flag(2);
  verify("host unflag", not flagged[2] and flags = 0);
  toggle_flag(0);
  verify("host open no flag", not flagged[0] and flags = 0);
  i := 0;
  while i < CellCount and flags < MineCount do
  begin
    if not opened[i] and not flagged[i] then
      toggle_flag(i);
    i := i + 1;
  end;
  verify("host flag cap", flags = MineCount);
  toggle_flag(80);
  verify("host flag cap holds", flags = MineCount and not flagged[80]);

  // Revealing every safe cell wins.
  restart();
  left_click(0);
  i := 0;
  while i < CellCount do
  begin
    if not mine[i] then
      reveal(i);
    i := i + 1;
  end;
  verify("host win", won and safe_open = SafeCount);
  verify("host win face", face_text = "B)");
  verify("host win status", status_text = "You win");

  // Revealing a mine after a restart loses.
  restart();
  left_click(0);
  i := 0;
  ok := true;
  while i < CellCount and ok do
  begin
    if mine[i] then
    begin
      reveal(i);
      ok := false;
    end;
    i := i + 1;
  end;
  verify("host loss", lost and not won);
  verify("host loss face", face_text = ":(");
  verify("host loss status", status_text = "Boom");

  // The timer text is pure.
  format_time(timer_text, 65);
  verify("host time", timer_text = "01:05");

  if failed then
    out.say("minesweeper: host failed\n")
  else
    out.say("minesweeper: host ok\n");
end;

// ---- entry point ---------------------------------------------------------

procedure main(args: const string[1024]);
begin
  fixed_seed := false;
  failed := false;
  step := 0;
  dirty := false;
  focus := 0;
  running := true;
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  gray := stdlib.ui_gray();
  restart();

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
  begin
    fixed_seed := true;
    restart();
    host_path();
  end
  else if screen_w < uint(WindowX + ContentW + 20)
      or screen_h < uint(WindowY + ContentH + 20) then
    out.say("minesweeper: screen too small\n")
  else
  begin
    status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Minesweeper", id);
    if status <> Ok then
      out.say("minesweeper: no window\n")
    else
    begin
      status := syslib.window_content_rect(id, origin);
      if args = "check" then
      begin
        fixed_seed := true;
        restart();
        check_mode();
      end
      else if args = "shot" then
      begin
        fixed_seed := true;
        restart();
        shot_mode();
      end
      else
        interactive();
    end;
  end;
end;

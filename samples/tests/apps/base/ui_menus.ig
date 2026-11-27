unit ui_menus;

// M2.4.3 proof: the stdlib menus layer in a real window.
//
//   run ui_menus
//
// The app draws a three-menu bar (File/Edit/View) and one pull-down at a time,
// then drives the model with injected pointer and key events:
//
//   1. A pointer click on the View bar label opens View; its first item is
//      highlighted and its check mark is white on the accent.
//   2. Down moves the highlight; item 0's check mark is black on white, and a
//      second Down skips the separator and the disabled item and stays put.
//   3. Enter activates "Show Ruler" and toggles its check mark.
//   4. File opens by a bar click; Down skips the separator and clamps on the
//      disabled "Quit"; Enter activates "Close".
//   5. A click on the disabled "Zoom" row neither selects nor activates it.
//   6. Escape closes the menu; Left/Right switch the open top-level menu.
//   7. Pointer press+release activates an item through the same helpers.
//
// Checkmarks and disabled items use independent `uint` masks, and the app owns
// every state scalar. The first failing check is printed with its step number.
// `langhost` and a kernel without a framebuffer run the pure helpers and the
// disabled-screen `Invalid` contract and print `ui_menus: host ok`.

uses core, stdlib, syslib, lib.out, lib.inject;

const
  MenuBarCount = 3;
  MenuW = 220;
  MenuRowH = 16;

var
  failed: bool;
  fail_step: int;
  fail_label: string[64];
  step: int;
  status: Status;
  screen_w: uint;
  screen_h: uint;
  id: int;
  rect: Rect;
  ev: Event;
  flushed: uint;

  mx: int;
  my: int;
  bar_w: int;
  dx: int;
  dy: int;

  titles: array[string[32], 8];
  file_items: array[string[64], 16];
  file_keys: array[string[32], 16];
  file_count: int;
  edit_items: array[string[64], 16];
  edit_keys: array[string[32], 16];
  edit_count: int;
  view_items: array[string[64], 16];
  view_keys: array[string[32], 16];
  view_count: int;

  file_disabled: uint;
  edit_disabled: uint;
  view_disabled: uint;
  checked_mask: uint;

  open_menu: int;
  selected: int;
  hit_row: int;
  bar_hit: int;
  action: int;
  next_menu: int;
  menu_x_at: int;
  file_actions: int;
  edit_actions: int;
  view_actions: int;

  black: uint;
  white: uint;
  accent: uint;

  dirty: bool;
  shot: bool;
  at: int;
  digits: string[16];

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

// ---- pure helpers and the disabled-screen contract ---------------------

procedure pure_checks();
var
  active: int;
  mask: uint;
  all_disabled: uint;
begin
  // Separators and masks.
  verify("separator marker", stdlib.ui_menu_is_separator("-"));
  verify("not a separator", not stdlib.ui_menu_is_separator("Open..."));
  verify("bit 0", stdlib.ui_menu_bit(0) = uint(1));
  verify("bit 3", stdlib.ui_menu_bit(3) = uint(8));
  verify("bit 15", stdlib.ui_menu_bit(15) = uint(32768));
  verify("bit negative", stdlib.ui_menu_bit(-1) = uint(0));
  verify("bit past 63", stdlib.ui_menu_bit(64) = uint(0));
  verify("has set", stdlib.ui_menu_has(uint(8), 3));
  verify("has clear", not stdlib.ui_menu_has(uint(8), 2));
  verify("has out of range", not stdlib.ui_menu_has(uint(8), 64));
  verify("set adds", stdlib.ui_menu_set(uint(0), 3, true) = uint(8));
  verify("set keeps", stdlib.ui_menu_set(uint(8), 3, true) = uint(8));
  verify("set clears", stdlib.ui_menu_set(uint(8), 3, false) = uint(0));
  verify("set stays clear", stdlib.ui_menu_set(uint(8), 2, false) = uint(8));
  verify("set bad index", stdlib.ui_menu_set(uint(8), 64, true) = uint(8));

  // Selectability, first and move over the View menu.
  verify("selectable enabled", stdlib.ui_menu_selectable(view_items[0], view_disabled, 0));
  verify("separator not selectable", not stdlib.ui_menu_selectable(view_items[2], view_disabled, 2));
  verify("disabled not selectable", not stdlib.ui_menu_selectable(view_items[3], view_disabled, 3));
  verify("first enabled", stdlib.ui_menu_first(view_count, view_items, view_disabled) = 0);
  mask := stdlib.ui_menu_set(uint(0), 0, true);
  verify("first skips disabled", stdlib.ui_menu_first(2, view_items, mask) = 1);
  all_disabled := stdlib.ui_menu_set(uint(0), 0, true);
  all_disabled := stdlib.ui_menu_set(all_disabled, 1, true);
  all_disabled := stdlib.ui_menu_set(all_disabled, 2, true);
  all_disabled := stdlib.ui_menu_set(all_disabled, 3, true);
  verify("first none", stdlib.ui_menu_first(4, view_items, all_disabled) = -1);
  verify("move down", stdlib.ui_menu_move(0, view_count, 1, view_items, view_disabled) = 1);
  verify("move skips separator and disabled", stdlib.ui_menu_move(1, view_count, 1, view_items, view_disabled) = 1);
  verify("move clamps at top", stdlib.ui_menu_move(0, view_count, -1, view_items, view_disabled) = 0);
  verify("move back from disabled", stdlib.ui_menu_move(3, view_count, -1, view_items, view_disabled) = 1);
  verify("move from separator", stdlib.ui_menu_move(2, view_count, 1, view_items, view_disabled) = 1);
  verify("move empty", stdlib.ui_menu_move(0, 0, 1, view_items, view_disabled) = -1);

  // Open/close transitions.
  selected := -1;
  active := stdlib.ui_menu_open(2, view_count, view_items, view_disabled, selected);
  verify("open sets menu", active = 2);
  verify("open selects first", selected = 0);
  verify("close", stdlib.ui_menu_close() = -1);

  // Pull-down hit test: row 0/1, separator and disabled rows, past the end.
  verify("hit row 0", stdlib.ui_menu_hit(0, 0, 200, view_items, view_count, 10, 5) = 0);
  verify("hit row 1", stdlib.ui_menu_hit(0, 0, 200, view_items, view_count, 10, 22) = 1);
  verify("hit separator", stdlib.ui_menu_hit(0, 0, 200, view_items, view_count, 10, 38) = -1);
  verify("hit disabled row", stdlib.ui_menu_hit(0, 0, 200, view_items, view_count, 10, 54) = 3);
  verify("hit past menu", stdlib.ui_menu_hit(0, 0, 200, view_items, view_count, 10, 70) = -1);
  verify("hit border", stdlib.ui_menu_hit(0, 0, 200, view_items, view_count, 10, 0) = -1);

  // Menu bar layout: File, Edit and View cells, a gap and the bar's rule.
  verify("bar hit file", stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuBarCount, 10, 10) = 0);
  verify("bar hit edit", stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuBarCount, 60, 10) = 1);
  verify("bar hit view", stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuBarCount, 110, 10) = 2);
  verify("bar hit gap", stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuBarCount, 48, 10) = -1);
  verify("bar hit below", stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuBarCount, 10, 30) = -1);
  verify("bar height", stdlib.ui_menu_bar_height() = 22);

  // Keyboard mapping: Escape always closes; the rest needs an open menu.
  verify("key esc", stdlib.ui_menu_key(uint(0x100 * 65536), false) = 6);
  verify("key esc open", stdlib.ui_menu_key(uint(0x100 * 65536), true) = 6);
  verify("key enter open", stdlib.ui_menu_key(uint(0x103 * 65536), true) = 5);
  verify("key enter closed", stdlib.ui_menu_key(uint(0x103 * 65536), false) = 0);
  verify("key up", stdlib.ui_menu_key(uint(0x112 * 65536), true) = 1);
  verify("key down", stdlib.ui_menu_key(uint(0x113 * 65536), true) = 2);
  verify("key left", stdlib.ui_menu_key(uint(0x110 * 65536), true) = 3);
  verify("key right", stdlib.ui_menu_key(uint(0x111 * 65536), true) = 4);
  verify("key other", stdlib.ui_menu_key(uint(72 * 65536 + 104), true) = 0);
end;

procedure host_checks();
begin
  verify("menu bar disabled", stdlib.ui_menu_bar(0, 0, 200, titles, MenuBarCount, 0) = Invalid);
  verify(
    "menu dropdown disabled",
    stdlib.ui_menu_dropdown(
      0, 0, 200, view_items, view_keys, view_count, 0, checked_mask, view_disabled, true) = Invalid);
  verify(
    "menu dropdown too small",
    stdlib.ui_menu_dropdown(
      0, 0, 8, view_items, view_keys, view_count, 0, checked_mask, view_disabled, true) = Invalid);
  verify("menu bar no width", stdlib.ui_menu_bar(0, 0, 0, titles, MenuBarCount, 0) = Invalid);
end;

// ---- model -------------------------------------------------------------

// The pull-down's left edge for a menu, aligning it with the bar cell: the bar
// label starts at `x + 8`, the cell at `x + 4`, and every earlier cell adds the
// title's width plus the 16-pixel gap.
function menu_x(index: int): int;
var
  i: int;
  offset: int;
begin
  offset := 4;
  i := 0;
  while i < index do
  begin
    offset := offset + stdlib.ui_text_width(titles[i]) + 16;
    i := i + 1;
  end;
  return mx + offset;
end;

procedure open_menu_at(menu: int);
begin
  if menu = 0 then
    open_menu := stdlib.ui_menu_open(0, file_count, file_items, file_disabled, selected)
  else if menu = 1 then
    open_menu := stdlib.ui_menu_open(1, edit_count, edit_items, edit_disabled, selected)
  else
    open_menu := stdlib.ui_menu_open(2, view_count, view_items, view_disabled, selected);
end;

procedure close_menu();
begin
  open_menu := stdlib.ui_menu_close();
  selected := -1;
end;

procedure menu_move(delta: int);
begin
  if open_menu = 0 then
    selected := stdlib.ui_menu_move(selected, file_count, delta, file_items, file_disabled)
  else if open_menu = 1 then
    selected := stdlib.ui_menu_move(selected, edit_count, delta, edit_items, edit_disabled)
  else if open_menu = 2 then
    selected := stdlib.ui_menu_move(selected, view_count, delta, view_items, view_disabled);
end;

procedure menu_activate();
begin
  if open_menu = 0 then
  begin
    file_actions := file_actions + 1;
    close_menu();
  end
  else if open_menu = 1 then
  begin
    edit_actions := edit_actions + 1;
    close_menu();
  end
  else if open_menu = 2 then
  begin
    if selected = 0 then
      checked_mask := stdlib.ui_menu_set(
        checked_mask, 0, not stdlib.ui_menu_has(checked_mask, 0))
    else if selected = 1 then
      checked_mask := stdlib.ui_menu_set(
        checked_mask, 1, not stdlib.ui_menu_has(checked_mask, 1));
    view_actions := view_actions + 1;
    close_menu();
  end;
end;

procedure menu_select(menu: int; px: int; py: int);
begin
  if menu = 0 then
    hit_row := stdlib.ui_menu_hit(mx, my + 22, MenuW, file_items, file_count, px, py)
  else if menu = 1 then
    hit_row := stdlib.ui_menu_hit(menu_x(1), my + 22, MenuW, edit_items, edit_count, px, py)
  else
    hit_row := stdlib.ui_menu_hit(menu_x(2), my + 22, MenuW, view_items, view_count, px, py);
end;

function menu_selectable(menu: int; index: int): bool;
begin
  if menu = 0 then
    return stdlib.ui_menu_selectable(file_items[index], file_disabled, index);
  if menu = 1 then
    return stdlib.ui_menu_selectable(edit_items[index], edit_disabled, index);
  return stdlib.ui_menu_selectable(view_items[index], view_disabled, index);
end;

procedure switch_menu(delta: int);
begin
  if open_menu >= 0 then
  begin
    next_menu := stdlib.ui_focus_next(open_menu, MenuBarCount, delta);
    open_menu_at(next_menu);
  end;
end;

// ---- drawing -----------------------------------------------------------

procedure draw_menu();
var
  st: Status;
begin
  // Window-local (spec 0026 C8).
  st := syslib.fill_rect(0, 0, rect.w, rect.h, white);
  st := stdlib.ui_center_text(
    0, 60, rect.w, 16, "M2.4.3 menus", black);
  st := stdlib.ui_menu_bar(mx, my, bar_w, titles, MenuBarCount, open_menu);
  if open_menu >= 0 then
  begin
    dx := menu_x(open_menu);
    dy := my + 22;
    if open_menu = 0 then
      st := stdlib.ui_menu_dropdown(
        dx, dy, MenuW, file_items, file_keys, file_count, selected,
        uint(0), file_disabled, true)
    else if open_menu = 1 then
      st := stdlib.ui_menu_dropdown(
        dx, dy, MenuW, edit_items, edit_keys, edit_count, selected,
        uint(0), edit_disabled, true)
    else
      st := stdlib.ui_menu_dropdown(
        dx, dy, MenuW, view_items, view_keys, view_count, selected,
        checked_mask, view_disabled, true);
  end;
end;

procedure present_menu();
begin
  draw_menu();
  status := syslib.window_present(id);
end;

// ---- event handling ----------------------------------------------------

procedure handle_key();
begin
  action := stdlib.ui_menu_key(ev.code, open_menu >= 0);
  if action = 6 then
    close_menu()
  else if action = 1 then
    menu_move(-1)
  else if action = 2 then
    menu_move(1)
  else if action = 3 then
    switch_menu(-1)
  else if action = 4 then
    switch_menu(1)
  else if action = 5 then
  begin
    if open_menu >= 0 and selected >= 0 then
      menu_activate();
  end;
end;

procedure handle_pointer();
begin
  if ev.kind = PointerDown then
  begin
    bar_hit := stdlib.ui_menu_bar_hit(
      mx, my, bar_w, titles, MenuBarCount, ev.x, ev.y);
    if bar_hit >= 0 then
      open_menu_at(bar_hit)
    else if open_menu >= 0 then
    begin
      menu_select(open_menu, ev.x, ev.y);
      if hit_row >= 0 then
      begin
        if menu_selectable(open_menu, hit_row) then
          selected := hit_row;
      end
      else
        close_menu();
    end;
  end
  else if ev.kind = PointerUp then
  begin
    if open_menu >= 0 then
    begin
      menu_select(open_menu, ev.x, ev.y);
      if hit_row >= 0 and hit_row = selected then
      begin
        if menu_selectable(open_menu, hit_row) then
          menu_activate();
      end;
    end;
  end;
end;

procedure drain_events();
var
  running: bool;
  count: int;
begin
  running := true;
  count := 0;
  while running and count < 64 do
  begin
    status := syslib.poll_event(ev);
    if status = Ok then
    begin
      count := count + 1;
      if ev.kind = Expose then
        dirty := true
      else if ev.kind = KeyDown then
      begin
        handle_key();
        dirty := true;
      end
      else if ev.kind = PointerDown or ev.kind = PointerUp then
      begin
        handle_pointer();
        dirty := true;
      end;
    end
    else
      running := false;
  end;
  if dirty then
  begin
    present_menu();
    dirty := false;
  end;
end;

// ---- injection ---------------------------------------------------------

// `x`/`y` are window-local; the debug pointer hook takes screen coordinates
// (spec 0010 I35), so the content origin is added here.
procedure pointer_to(x: int; y: int);
begin
  status := inject.pointer_to(rect.x + x, rect.y + y);
  drain_events();
end;

procedure pointer_down();
begin
  status := inject.pointer_down();
  drain_events();
end;

procedure pointer_up();
begin
  status := inject.pointer_up();
  drain_events();
end;

procedure inject_key(key_code: int; text_byte: int);
begin
  status := inject.inject_key(key_code, text_byte);
  drain_events();
end;

// ---- kernel proof ------------------------------------------------------

procedure kernel_proof();
begin
  // Baseline: the bar is white with a black bottom rule.
  present_menu();
  verify("bar white", syslib.get_pixel(mx + 200, my + 5) = white);
  verify("bar rule", syslib.get_pixel(mx + 200, my + 21) = black);

  // 1. Open View by clicking its bar label.
  pointer_to(mx + 110, my + 10);
  pointer_down();
  pointer_up();
  verify("view opened", open_menu = 2 and selected = 0);
  verify("view tab inverted", syslib.get_pixel(mx + 106, my + 3) = black);
  verify("file tab plain", syslib.get_pixel(mx + 10, my + 3) = white);
  verify("dropdown border", syslib.get_pixel(dx, dy) = black);
  verify("selected row accent", syslib.get_pixel(dx + 2, dy + 2) = accent);
  verify("checked mark white on accent", syslib.get_pixel(dx + 10, dy + 6) = white);
  verify("separator line", syslib.get_pixel(dx + 10, dy + 41) = black);
  verify("disabled row plain", syslib.get_pixel(dx + 2, dy + 54) = white);

  // 2. Down moves to "Show Ruler"; item 0's mark is now black on white.
  inject_key(0x113, 0);
  verify("view moved", selected = 1);
  verify("unselected mark black", syslib.get_pixel(dx + 10, dy + 6) = black);
  verify("row 1 accent", syslib.get_pixel(dx + 2, dy + 22) = accent);
  verify("row 0 plain", syslib.get_pixel(dx + 2, dy + 5) = white);

  // A second Down skips the separator and the disabled Zoom and stays put.
  inject_key(0x113, 0);
  verify("skips separator and disabled", selected = 1);
  verify("still row 1 accent", syslib.get_pixel(dx + 2, dy + 22) = accent);

  // 3. Enter toggles "Show Ruler" and closes the menu.
  inject_key(0x103, 0);
  verify("view action", view_actions = 1);
  verify("ruler checked", stdlib.ui_menu_has(checked_mask, 1));
  verify("menu closed", open_menu = -1);
  verify("view tab plain again", syslib.get_pixel(mx + 106, my + 3) = white);

  // 4. File: open by a bar click, skip the separator, clamp on disabled Quit.
  pointer_to(mx + 10, my + 10);
  pointer_down();
  pointer_up();
  verify("file opened", open_menu = 0 and selected = 0);
  inject_key(0x113, 0);
  verify("file moved to Open", selected = 1);
  inject_key(0x113, 0);
  verify("file skips separator", selected = 3);
  inject_key(0x113, 0);
  verify("file clamps on disabled", selected = 3);
  inject_key(0x103, 0);
  verify("file action", file_actions = 1 and open_menu = -1);

  // 5. Disabled item: reopen View, aim at Zoom (row 3), press and release.
  pointer_to(mx + 110, my + 10);
  pointer_down();
  pointer_up();
  inject_key(0x113, 0);
  verify("view selection for disabled test", selected = 1);
  pointer_to(dx + 10, dy + 54);
  pointer_down();
  verify("disabled did not select", selected = 1 and open_menu = 2);
  pointer_up();
  verify("disabled did not activate", selected = 1 and open_menu = 2 and view_actions = 1);

  // Item 1 is checked and selected: its mark is white on the accent.
  verify("toggled mark white", syslib.get_pixel(dx + 10, dy + 22) = white);

  // 6. Escape closes.
  inject_key(0x100, 0);
  verify("escape closed", open_menu = -1);
  verify("dropdown erased", syslib.get_pixel(dx + 2, dy + 2) = white);

  // 7. Left/Right switch the open top-level menu with wrap.
  pointer_to(mx + 110, my + 10);
  pointer_down();
  pointer_up();
  verify("view reopened", open_menu = 2);
  inject_key(0x111, 0);
  verify("right switches to File", open_menu = 0 and selected = 0);
  inject_key(0x110, 0);
  verify("left wraps to View", open_menu = 2 and selected = 0);
  inject_key(0x100, 0);

  // 8. The masks stay independent of the highlight.
  verify("toolbar still checked", stdlib.ui_menu_has(checked_mask, 0));
  verify("ruler still checked", stdlib.ui_menu_has(checked_mask, 1));
  verify("zoom check clear", not stdlib.ui_menu_has(checked_mask, 2));
  verify("zoom disabled", stdlib.ui_menu_has(view_disabled, 3));
  verify("quit disabled", stdlib.ui_menu_has(file_disabled, 4));

  // 9. Pointer activation: click Show Toolbar and clear its check mark.
  pointer_to(mx + 110, my + 10);
  pointer_down();
  pointer_up();
  pointer_to(dx + 10, dy + 5);
  pointer_down();
  pointer_up();
  verify("pointer activated", view_actions = 2 and open_menu = -1);
  verify("toolbar unchecked", not stdlib.ui_menu_has(checked_mask, 0));

  status := syslib.window_close(id);
  verify("window closed", status = Ok);
end;

procedure report_and_exit();
begin
  if failed then
  begin
    out.say("ui_menus: failed check ");
    at := core.format_int_into(digits, fail_step, uint(10), uint(0));
    out.say(digits);
    out.say(" (");
    out.say(fail_label);
    out.say(")\n");
  end
  else
    out.say("ui_menus: ok\n");
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  fail_step := 0;
  step := 0;
  shot := args = "shot";
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  accent := stdlib.ui_accent();

  titles[0] := "File";
  titles[1] := "Edit";
  titles[2] := "View";

  file_items[0] := "New";
  file_items[1] := "Open...";
  file_items[2] := "-";
  file_items[3] := "Close";
  file_items[4] := "Quit";
  file_keys[0] := "Ctrl+N";
  file_keys[1] := "Ctrl+O";
  file_keys[2] := "";
  file_keys[3] := "Ctrl+W";
  file_keys[4] := "Ctrl+Q";
  file_count := 5;

  edit_items[0] := "Cut";
  edit_items[1] := "Copy";
  edit_items[2] := "Paste";
  edit_items[3] := "-";
  edit_items[4] := "Select All";
  edit_keys[0] := "Ctrl+X";
  edit_keys[1] := "Ctrl+C";
  edit_keys[2] := "Ctrl+V";
  edit_keys[3] := "";
  edit_keys[4] := "Ctrl+A";
  edit_count := 5;

  view_items[0] := "Show Toolbar";
  view_items[1] := "Show Ruler";
  view_items[2] := "-";
  view_items[3] := "Zoom";
  view_keys[0] := "";
  view_keys[1] := "";
  view_keys[2] := "";
  view_keys[3] := "";
  view_count := 4;

  file_disabled := stdlib.ui_menu_set(uint(0), 4, true);
  edit_disabled := uint(0);
  view_disabled := stdlib.ui_menu_set(uint(0), 3, true);
  checked_mask := stdlib.ui_menu_set(uint(0), 0, true);
  open_menu := -1;
  selected := -1;
  file_actions := 0;
  edit_actions := 0;
  view_actions := 0;

  pure_checks();

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
  begin
    host_checks();
    if failed then
      report_and_exit()
    else
      out.say("ui_menus: host ok\n");
  end
  else if screen_w < uint(100 + 640 + 20) or screen_h < uint(70 + 420 + 20) then
    out.say("ui_menus: screen too small\n")
  else
  begin
    status := syslib.window_open(100, 70, 640, 420, "Menus", id);
    if status <> Ok then
      out.say("ui_menus: no window\n")
    else
    begin
      status := syslib.window_content_rect(id, rect);
      if status <> Ok then
        out.say("ui_menus: no content rect\n")
      else
      begin
        mx := 8;
        my := 8;
        bar_w := rect.w - 16;
        dirty := false;
        flushed := syslib.event_flush();
        if shot then
        begin
          // The screenshot state: View open with two check marks, the first
          // item highlighted, the disabled Zoom item and the separator.
          checked_mask := stdlib.ui_menu_set(checked_mask, 1, true);
          open_menu_at(2);
          draw_menu();
          present_menu();
          out.say("ui_menus: ready\n");
          flushed := core.clock();
          while core.clock() - flushed < uint(3000000000) do
            at := 0;
          status := syslib.window_close(id);
          out.say("ui_menus: done\n");
        end
        else
          kernel_proof();
        report_and_exit();
      end;
    end;
  end;
end;

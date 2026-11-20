unit widgets;

// M2.4.6 proof: one composed "widgets" window that exercises the whole UI
// toolkit -- client chrome, a menu bar with pull-downs, a button, a checkbox, a
// text field, a scrollbar, a list, an alert and a text-input dialog, and a
// compact icon strip.
//
//   run widgets        // deterministic self-check, prints `widgets: ok`
//   run widgets check  // the same check
//   run widgets shot   // composed frame for the screenshot harness
//
// The app owns every state scalar and the one event loop; the library only
// draws and computes. The modal dialogs are the same state machine: while
// `dialog` is non-zero every key and pointer event goes to it, and only its
// default/cancel actions leave that mode.
//
// Check mode drives injected pointer and key events and checks representative
// pixels with `syslib.get_pixel`; the first failing check is printed with its
// step number. `langhost` and a kernel without a framebuffer run the pure
// helpers and the disabled-screen `Invalid` contract and print
// `widgets: host ok`.

uses core, stdlib, syslib, lib.out, lib.inject;

const
  ControlCount = 5;
  MenuCount = 3;
  MenuW = 220;

  ButtonW = 112;
  ButtonH = 24;
  FieldW = 180;
  FieldH = 22;
  ScrollW = 16;
  ScrollH = 150;
  ListW = 340;
  ListH = 98;
  DocW = 320;
  DocH = 150;
  DocTotal = 16;
  DocVisible = 8;

  IconCount = 2;
  IconCellW = 72;
  IconCellH = 56;
  IconSize = 32;
  TrashSize = 48;

  AlertH = 160;
  InputH = 200;

var
  failed: bool;
  fail_step: int;
  fail_label: string[64];
  step: int;
  status: Status;
  present_status: Status;
  screen_w: uint;
  screen_h: uint;
  id: int;
  rect: Rect;
  ev: Event;
  flushed: uint;
  auto_present: bool;
  dirty: bool;
  shot: bool;
  closed: bool;

  // Menu bar and pull-downs.
  menu_x: int;
  menu_y: int;
  menu_w: int;
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
  view_checked: uint;
  open_menu: int;
  selected: int;
  hit_row: int;
  bar_hit: int;
  action: int;
  next_menu: int;
  dx: int;
  dy: int;
  file_actions: int;
  edit_actions: int;
  view_actions: int;

  // Controls.
  bx: int;
  by: int;
  cx: int;
  cy: int;
  tx: int;
  ty: int;
  lx: int;
  ly: int;
  sbx: int;
  sby: int;
  docx: int;
  docy: int;
  icx: int;
  icy: int;
  ttx: int;
  tty: int;

  focus_index: int;
  focus_text: string[32];
  button_clicks: int;
  button_down: bool;
  checkbox_down: bool;
  checked: bool;

  field_text: string[256];
  field_cursor: int;
  field_scroll: int;
  sel_start: int;
  sel_stop: int;

  scroll_position: int;
  scroll_visible: int;
  scroll_total: int;
  doc_lines: array[string[64], 16];

  list_items: array[string[64], 8];
  list_count: int;
  list_selected: int;
  list_top: int;

  // Icon strip (the assets and the live state live in the app).
  folder_rows: array[uint, 32];
  doc_rows: array[uint, 32];
  icon_labels: array[string[32], 2];
  icon_selected: int;
  last_click_ns: uint;
  last_click_icon: int;
  opens: int;

  // Modal dialog.
  dialog: int; // 0 none, 1 alert, 2 input
  dialog_focus: int;
  dialog_text: string[256];
  dialog_cursor: int;
  dialog_scroll: int;
  dialog_sel_start: int;
  dialog_sel_stop: int;
  alert_results: int;
  alert_cancels: int;
  input_commits: int;
  input_cancels: int;

  // Status line.
  status_text: string[64];

  // Colours.
  black: uint;
  white: uint;
  gray: uint;
  accent: uint;

  // Scratch.
  one: bytes[1];
  ch: string[1];
  digits: string[16];
  at: int;
  copied: int;
  now: uint;
  clk_h: string[2];
  clk_m: string[2];
  used: int;
  cell: int;
  hit_icon: int;
  probe_x: int;
  probe_y: int;

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
  text: string[256];
  cursor: int;
  start: int;
  stop: int;
begin
  verify("focus next", stdlib.ui_focus_next(0, ControlCount, 1) = 1);
  verify("focus wrap", stdlib.ui_focus_next(4, ControlCount, 1) = 0);
  verify("focus back", stdlib.ui_focus_next(0, ControlCount, -1) = 4);
  verify("focus empty", stdlib.ui_focus_next(3, 0, 1) = 0);

  verify("rect hit", stdlib.ui_rect_hit(10, 10, 10, 10, 10, 10));
  verify("rect miss", not stdlib.ui_rect_hit(10, 10, 10, 10, 20, 10));
  verify("rect empty", not stdlib.ui_rect_hit(0, 0, 0, 10, 0, 0));
  verify("button hit", stdlib.ui_button_hit(0, 0, 20, 20, 19, 19));
  verify("button press", stdlib.ui_button_pressed(true, true));
  verify("button release outside", not stdlib.ui_button_pressed(true, false));
  verify("checkbox hit", stdlib.ui_checkbox_hit(0, 0, 5, 5));
  verify("checkbox toggle", stdlib.ui_checkbox_toggle(false));

  verify("list rows", stdlib.ui_list_rows(ListH) = 6);
  verify("list move", stdlib.ui_list_move(0, 8, 1) = 1);
  verify("list ensure below", stdlib.ui_list_ensure_visible(6, 0, 6) = 1);
  verify("list ensure above", stdlib.ui_list_ensure_visible(1, 3, 6) = 1);
  verify("list hit", stdlib.ui_list_hit(0, 0, ListW, ListH, 10, 21, 8, 1) = 2);

  verify("scroll clamp", stdlib.ui_scrollbar_clamp(99, 8, 16) = 8);
  verify(
    "scroll hit page down",
    stdlib.ui_scrollbar_hit(0, 0, ScrollW, ScrollH, 8, 140, 0, 8, 16) = 2);
  verify(
    "scroll hit thumb",
    stdlib.ui_scrollbar_hit(0, 0, ScrollW, ScrollH, 8, 5, 0, 8, 16) = 3);

  verify("key text", stdlib.ui_key_text(uint(72 * 65536 + 104)) = byte(104));
  verify("key modifiers", stdlib.ui_key_modifiers(uint(65 + 1 * 256)) = 1);
  verify("key code", stdlib.ui_key_code(uint(72 * 65536 + 104)) = 72);
  verify("key tab", stdlib.ui_key_is_tab(uint(0x102 * 65536)));
  verify("key enter", stdlib.ui_key_is_enter(uint(0x103 * 65536)));
  verify("key left", stdlib.ui_key_is_left(uint(0x110 * 65536)));
  verify("key right", stdlib.ui_key_is_right(uint(0x111 * 65536)));
  verify("key up", stdlib.ui_key_is_up(uint(0x112 * 65536)));
  verify("key down", stdlib.ui_key_is_down(uint(0x113 * 65536)));
  verify("key home", stdlib.ui_key_is_home(uint(0x114 * 65536)));
  verify("key end", stdlib.ui_key_is_end(uint(0x115 * 65536)));
  verify("key delete", stdlib.ui_key_is_delete(uint(0x119 * 65536)));

  text := "abcdef";
  verify("reveal keeps", stdlib.ui_text_field_reveal(text, 2, 0, 24) = 0);
  verify("reveal right", stdlib.ui_text_field_reveal(text, 6, 0, 24) = 3);
  verify("reveal left", stdlib.ui_text_field_reveal(text, 1, 4, 24) = 1);

  verify("separator", stdlib.ui_menu_is_separator("-"));
  verify("not separator", not stdlib.ui_menu_is_separator("New"));
  verify("menu bit", stdlib.ui_menu_bit(3) = uint(8));
  verify("menu has", stdlib.ui_menu_has(uint(8), 3));
  verify("menu has clear", not stdlib.ui_menu_has(uint(8), 2));
  verify("menu set", stdlib.ui_menu_set(uint(0), 3, true) = uint(8));
  verify("menu clear", stdlib.ui_menu_set(uint(8), 3, false) = uint(0));
  verify("menu selectable", stdlib.ui_menu_selectable("About...", uint(0), 2));
  verify("menu separator not selectable", not stdlib.ui_menu_selectable("-", uint(0), 3));
  verify("menu disabled not selectable", not stdlib.ui_menu_selectable("Quit", uint(16), 4));
  verify("menu first", stdlib.ui_menu_first(file_count, file_items, file_disabled) = 0);
  mask := stdlib.ui_menu_set(uint(0), 4, true);
  verify("menu move skips disabled", stdlib.ui_menu_move(2, 5, 1, file_items, mask) = 2);
  verify("menu key escape", stdlib.ui_menu_key(uint(0x100 * 65536), true) = 6);
  verify("menu key enter", stdlib.ui_menu_key(uint(0x103 * 65536), true) = 5);
  verify("menu key right", stdlib.ui_menu_key(uint(0x111 * 65536), true) = 4);
  verify("menu bar hit", stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuCount, 10, 10) = 0);
  verify("menu hit separator", stdlib.ui_menu_hit(0, 0, MenuW, file_items, file_count, 10, 55) = -1);
  verify("menu hit disabled", stdlib.ui_menu_hit(0, 0, MenuW, file_items, file_count, 10, 69) = 4);

  verify("dialog enter", stdlib.ui_dialog_key(uint(0x103 * 65536), true) = 1);
  verify("dialog escape", stdlib.ui_dialog_key(uint(0x100 * 65536), true) = 2);
  verify("dialog tab", stdlib.ui_dialog_key(uint(0x102 * 65536), true) = 3);
  verify("dialog shift tab", stdlib.ui_dialog_key(uint(0x102 * 65536 + 256), true) = 4);
  verify("dialog button width", stdlib.ui_dialog_button_width("OK") = 72);
  verify("dialog button top", stdlib.ui_dialog_button_top(0, AlertH) = 128);
  verify(
    "alert hit default",
    stdlib.ui_alert_hit(0, 0, 360, AlertH, "OK", "Cancel", 210, 140) = 0);
  verify(
    "input hit field",
    stdlib.ui_input_dialog_hit(0, 0, 360, InputH, "OK", "Cancel", 20, 50) = 0);

  verify("icon size", stdlib.ui_icon_size() = 32);
  verify("icon row bit", stdlib.ui_icon_row_bit(uint(5), 2));
  verify("icon cell x", stdlib.ui_icon_cell_x(1, 10, 72) = 82);
  verify("icon label y", stdlib.ui_icon_label_y(20) = 54);
  verify("icon grid hit", stdlib.ui_icon_grid_hit(0, 0, 2, 2, 72, 56, 80, 10) = 1);
  verify("icon grid outside", stdlib.ui_icon_grid_hit(0, 0, 2, 2, 72, 56, 0, 0) = 0);
  verify("double click", stdlib.ui_double_click(uint(0), uint(400000000), true));
  verify("double click late", not stdlib.ui_double_click(uint(0), uint(400000001), true));
  verify("selection normalize", stdlib.ui_selection_normalize(10, 10, 4, 6, at, used, probe_x, probe_y) = Ok);
  verify("trash hit", stdlib.ui_trash_hit(0, 0, 48, 48, 10, 10));

  verify("str boundary", stdlib.str_boundary(text, 0));
  cursor := 0;
  cursor := stdlib.str_cursor_end(text, cursor);
  verify("str end", cursor = 6);
  start := 0;
  stop := 0;
  used := stdlib.str_selection_all(text, start, stop);
  verify("str select all", used = 6 and start = 0 and stop = 6);
end;

procedure host_checks();
begin
  verify(
    "menu bar disabled",
    stdlib.ui_menu_bar(0, 0, 200, titles, MenuCount, 0) = Invalid);
  verify(
    "menu dropdown disabled",
    stdlib.ui_menu_dropdown(
      0, 0, MenuW, file_items, file_keys, file_count, 0, uint(0), file_disabled, true) = Invalid);
  verify("button disabled", stdlib.ui_button(0, 0, ButtonW, ButtonH, "Press", false, true) = Invalid);
  verify("checkbox disabled", stdlib.ui_checkbox(0, 0, "Enabled", true, true) = Invalid);
  verify(
    "field disabled",
    stdlib.ui_text_field(0, 0, FieldW, FieldH, "x", 1, 0, 0, 0, true) = Invalid);
  verify(
    "scrollbar disabled",
    stdlib.ui_scrollbar(0, 0, ScrollW, ScrollH, 0, 8, 16, true) = Invalid);
  verify(
    "list disabled",
    stdlib.ui_list(0, 0, ListW, ListH, list_items, 8, 0, 0, true) = Invalid);
  verify("frame disabled", stdlib.ui_frame(0, 0, 100, 100) = Invalid);
  verify("title bar disabled", stdlib.ui_title_bar(0, 0, 100, "t", true) = Invalid);
  verify("separator disabled", stdlib.ui_separator(0, 0, 100) = Invalid);
  verify("message disabled", stdlib.ui_message(0, 0, 100, 16, "m") = Invalid);
  verify(
    "alert disabled",
    stdlib.ui_alert(0, 0, 360, AlertH, "t", "m", "OK", "Cancel", 0) = Invalid);
  verify(
    "input dialog disabled",
    stdlib.ui_input_dialog(
      0, 0, 360, InputH, "t", "p", "OK", "Cancel", "", 0, 0, 0, 0, 0) = Invalid);
  verify("icon disabled", stdlib.ui_icon(0, 0, folder_rows, black) = Invalid);
  verify("icon label disabled", stdlib.ui_icon_label(0, 0, "Folder") = Invalid);
  verify("selection rect disabled", stdlib.ui_selection_rect(0, 0, 28, 28) = Invalid);
end;

// ---- data ---------------------------------------------------------------

procedure load_data();
begin
  titles[0] := "File";
  titles[1] := "Edit";
  titles[2] := "View";

  file_items[0] := "New";
  file_items[1] := "Rename...";
  file_items[2] := "About...";
  file_items[3] := "-";
  file_items[4] := "Quit";
  file_keys[0] := "Ctrl+N";
  file_keys[1] := "Ctrl+R";
  file_keys[2] := "";
  file_keys[3] := "";
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

  view_items[0] := "Show Grid";
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
  view_checked := stdlib.ui_menu_set(uint(0), 0, true);

  list_items[0] := "Alpha";
  list_items[1] := "Bravo";
  list_items[2] := "Charlie";
  list_items[3] := "Delta";
  list_items[4] := "Echo";
  list_items[5] := "Foxtrot";
  list_items[6] := "Golf";
  list_items[7] := "Hotel";
  list_count := 8;

  doc_lines[0] := "Document line 01";
  doc_lines[1] := "Document line 02";
  doc_lines[2] := "Document line 03";
  doc_lines[3] := "Document line 04";
  doc_lines[4] := "Document line 05";
  doc_lines[5] := "Document line 06";
  doc_lines[6] := "Document line 07";
  doc_lines[7] := "Document line 08";
  doc_lines[8] := "Document line 09";
  doc_lines[9] := "Document line 10";
  doc_lines[10] := "Document line 11";
  doc_lines[11] := "Document line 12";
  doc_lines[12] := "Document line 13";
  doc_lines[13] := "Document line 14";
  doc_lines[14] := "Document line 15";
  doc_lines[15] := "Document line 16";

  icon_labels[0] := "Folder";
  icon_labels[1] := "Document";

  // Folder and document, 32 row masks each (bit 0 = the left pixel).
  folder_rows[0] := uint(0x00000000);
  folder_rows[1] := uint(0x00000000);
  folder_rows[2] := uint(0x00000000);
  folder_rows[3] := uint(0x00000000);
  folder_rows[4] := uint(0x00000000);
  folder_rows[5] := uint(0x00000000);
  folder_rows[6] := uint(0x00000000);
  folder_rows[7] := uint(0x0000FFF0);
  folder_rows[8] := uint(0x0000FFF0);
  folder_rows[9] := uint(0x0000FFF0);
  folder_rows[10] := uint(0x3FFFFFF0);
  folder_rows[11] := uint(0x3FFFFFF0);
  folder_rows[12] := uint(0x3FFFFFF0);
  folder_rows[13] := uint(0x3FFFFFF0);
  folder_rows[14] := uint(0x30000030);
  folder_rows[15] := uint(0x30000030);
  folder_rows[16] := uint(0x30000030);
  folder_rows[17] := uint(0x30000030);
  folder_rows[18] := uint(0x30000030);
  folder_rows[19] := uint(0x30000030);
  folder_rows[20] := uint(0x30000030);
  folder_rows[21] := uint(0x30000030);
  folder_rows[22] := uint(0x30000030);
  folder_rows[23] := uint(0x30000030);
  folder_rows[24] := uint(0x30000030);
  folder_rows[25] := uint(0x30000030);
  folder_rows[26] := uint(0x3FFFFFF0);
  folder_rows[27] := uint(0x3FFFFFF0);
  folder_rows[28] := uint(0x00000000);
  folder_rows[29] := uint(0x00000000);
  folder_rows[30] := uint(0x00000000);
  folder_rows[31] := uint(0x00000000);

  doc_rows[0] := uint(0x00000000);
  doc_rows[1] := uint(0x00000000);
  doc_rows[2] := uint(0x00000000);
  doc_rows[3] := uint(0x00000000);
  doc_rows[4] := uint(0x0000FF00);
  doc_rows[5] := uint(0x0001FF00);
  doc_rows[6] := uint(0x00030300);
  doc_rows[7] := uint(0x00060300);
  doc_rows[8] := uint(0x000C0300);
  doc_rows[9] := uint(0x00180300);
  doc_rows[10] := uint(0x00C00300);
  doc_rows[11] := uint(0x00DFFB00);
  doc_rows[12] := uint(0x00DFFB00);
  doc_rows[13] := uint(0x00C00300);
  doc_rows[14] := uint(0x00C00300);
  doc_rows[15] := uint(0x00DFFB00);
  doc_rows[16] := uint(0x00C00300);
  doc_rows[17] := uint(0x00C00300);
  doc_rows[18] := uint(0x00DFFB00);
  doc_rows[19] := uint(0x00C00300);
  doc_rows[20] := uint(0x00C00300);
  doc_rows[21] := uint(0x00DFFB00);
  doc_rows[22] := uint(0x00C00300);
  doc_rows[23] := uint(0x00C00300);
  doc_rows[24] := uint(0x00DFFB00);
  doc_rows[25] := uint(0x00C00300);
  doc_rows[26] := uint(0x00C00300);
  doc_rows[27] := uint(0x00FFFF00);
  doc_rows[28] := uint(0x00FFFF00);
  doc_rows[29] := uint(0x00000000);
  doc_rows[30] := uint(0x00000000);
  doc_rows[31] := uint(0x00000000);
end;

// ---- menu model ---------------------------------------------------------

function menu_x_at(index: int): int;
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
  return menu_x + offset;
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

function menu_selectable(menu: int; index: int): bool;
begin
  if menu = 0 then
    return stdlib.ui_menu_selectable(file_items[index], file_disabled, index);
  if menu = 1 then
    return stdlib.ui_menu_selectable(edit_items[index], edit_disabled, index);
  return stdlib.ui_menu_selectable(view_items[index], view_disabled, index);
end;

procedure menu_select(menu: int; px: int; py: int);
begin
  if menu = 0 then
    hit_row := stdlib.ui_menu_hit(menu_x_at(0), menu_y + 22, MenuW, file_items, file_count, px, py)
  else if menu = 1 then
    hit_row := stdlib.ui_menu_hit(menu_x_at(1), menu_y + 22, MenuW, edit_items, edit_count, px, py)
  else
    hit_row := stdlib.ui_menu_hit(menu_x_at(2), menu_y + 22, MenuW, view_items, view_count, px, py);
end;

procedure switch_menu(delta: int);
begin
  if open_menu >= 0 then
  begin
    next_menu := stdlib.ui_focus_next(open_menu, MenuCount, delta);
    open_menu_at(next_menu);
  end;
end;

// ---- field editing (M2.4.1 helpers; the app owns the buffer) ------------

procedure collapse_field();
begin
  sel_start := field_cursor;
  sel_stop := field_cursor;
end;

procedure reveal_field();
begin
  field_scroll := stdlib.ui_text_field_reveal(
    field_text, field_cursor, field_scroll, FieldW - 6);
end;

// Publishes one whole replacement through the selection helper, so a failure
// leaves the text untouched (the M2.4.1 atomicity rule).
procedure insert_field(value: const string[0]);
begin
  status := stdlib.str_selection_replace(field_text, sel_start, sel_stop, value);
  if status = Ok then
  begin
    field_cursor := sel_start;
    sel_stop := sel_start;
    reveal_field();
  end;
end;

procedure select_all_field();
begin
  used := stdlib.str_selection_all(field_text, sel_start, sel_stop);
  field_cursor := sel_stop;
  reveal_field();
end;

procedure type_field_key();
var
  text_byte: byte;
begin
  if stdlib.ui_key_is_backspace(ev.code) then
  begin
    if sel_start <> sel_stop then
    begin
      status := stdlib.str_delete_into(field_text, sel_start, sel_stop - sel_start);
      field_cursor := sel_start;
      sel_stop := sel_start;
    end
    else
    begin
      status := stdlib.str_backspace(field_text, field_cursor);
      collapse_field();
    end;
  end
  else if stdlib.ui_key_is_delete(ev.code) then
  begin
    if sel_start <> sel_stop then
    begin
      status := stdlib.str_delete_into(field_text, sel_start, sel_stop - sel_start);
      field_cursor := sel_start;
      sel_stop := sel_start;
    end
    else if field_cursor < int(core.len(field_text)) then
    begin
      at := field_cursor;
      at := stdlib.str_cursor_right(field_text, at);
      if at > field_cursor then
        status := stdlib.str_delete_into(field_text, field_cursor, at - field_cursor);
    end;
  end
  else if stdlib.ui_key_is_left(ev.code) then
  begin
    field_cursor := stdlib.str_cursor_left(field_text, field_cursor);
    collapse_field();
  end
  else if stdlib.ui_key_is_right(ev.code) then
  begin
    field_cursor := stdlib.str_cursor_right(field_text, field_cursor);
    collapse_field();
  end
  else if stdlib.ui_key_is_home(ev.code) then
  begin
    field_cursor := stdlib.str_cursor_home(field_cursor);
    collapse_field();
  end
  else if stdlib.ui_key_is_end(ev.code) then
  begin
    field_cursor := stdlib.str_cursor_end(field_text, field_cursor);
    collapse_field();
  end
  else
  begin
    text_byte := stdlib.ui_key_text(ev.code);
    if text_byte >= byte(32) and text_byte < byte(127) then
    begin
      one[0] := text_byte;
      copied := core.substr_into(ch, one, 0, 1);
      if copied = 1 then
        insert_field(ch);
    end;
  end;
  reveal_field();
end;

// ---- dialogs (the app owns the modal state machine) --------------------

function dialog_slots(): int;
begin
  if dialog = 1 then
    return 2;
  return 3;
end;

procedure open_alert();
begin
  dialog := 1;
  dialog_focus := 0;
  status_text := "About Widgets";
end;

procedure open_input();
begin
  dialog := 2;
  dialog_focus := 0;
  dialog_text := field_text;
  dialog_cursor := stdlib.str_cursor_end(dialog_text, dialog_cursor);
  dialog_scroll := 0;
  dialog_sel_start := dialog_cursor;
  dialog_sel_stop := dialog_cursor;
  status_text := "Rename text";
end;

procedure close_dialog();
begin
  dialog := 0;
  dialog_focus := 0;
end;

// Only OK (result 1) of the input dialog publishes the buffer into the field;
// cancel leaves the field untouched (the dialog helper never mutates it).
procedure apply_dialog_result(value: int);
begin
  if dialog = 1 then
  begin
    alert_results := alert_results + 1;
    if value = 2 then
      alert_cancels := alert_cancels + 1;
    if value = 1 then
      status_text := "Alert accepted"
    else
      status_text := "Alert cancelled";
  end
  else if dialog = 2 then
  begin
    if value = 1 then
    begin
      input_commits := input_commits + 1;
      field_text := dialog_text;
      field_cursor := dialog_cursor;
      sel_start := field_cursor;
      sel_stop := field_cursor;
      reveal_field();
      status_text := "Name committed";
    end
    else if value = 2 then
    begin
      input_cancels := input_cancels + 1;
      status_text := "Rename cancelled";
    end;
  end;
  close_dialog();
end;

procedure collapse_dialog();
begin
  dialog_sel_start := dialog_cursor;
  dialog_sel_stop := dialog_cursor;
end;

procedure insert_dialog(value: const string[0]);
begin
  status := stdlib.str_selection_replace(
    dialog_text, dialog_sel_start, dialog_sel_stop, value);
  if status = Ok then
  begin
    dialog_cursor := dialog_sel_start;
    dialog_sel_stop := dialog_sel_start;
    dialog_scroll := stdlib.ui_text_field_reveal(
      dialog_text, dialog_cursor, dialog_scroll, (360 - 16) - 6);
  end;
end;

procedure type_dialog_key();
var
  text_byte: byte;
begin
  if stdlib.ui_key_is_backspace(ev.code) then
  begin
    if dialog_sel_start <> dialog_sel_stop then
    begin
      status := stdlib.str_delete_into(
        dialog_text, dialog_sel_start, dialog_sel_stop - dialog_sel_start);
      dialog_cursor := dialog_sel_start;
      dialog_sel_stop := dialog_sel_start;
    end
    else
    begin
      status := stdlib.str_backspace(dialog_text, dialog_cursor);
      collapse_dialog();
    end;
  end
  else if stdlib.ui_key_is_delete(ev.code) then
  begin
    if dialog_sel_start <> dialog_sel_stop then
    begin
      status := stdlib.str_delete_into(
        dialog_text, dialog_sel_start, dialog_sel_stop - dialog_sel_start);
      dialog_cursor := dialog_sel_start;
      dialog_sel_stop := dialog_sel_start;
    end
    else if dialog_cursor < int(core.len(dialog_text)) then
    begin
      at := dialog_cursor;
      at := stdlib.str_cursor_right(dialog_text, at);
      if at > dialog_cursor then
        status := stdlib.str_delete_into(dialog_text, dialog_cursor, at - dialog_cursor);
    end;
  end
  else if stdlib.ui_key_is_left(ev.code) then
  begin
    dialog_cursor := stdlib.str_cursor_left(dialog_text, dialog_cursor);
    collapse_dialog();
  end
  else if stdlib.ui_key_is_right(ev.code) then
  begin
    dialog_cursor := stdlib.str_cursor_right(dialog_text, dialog_cursor);
    collapse_dialog();
  end
  else if stdlib.ui_key_is_home(ev.code) then
  begin
    dialog_cursor := stdlib.str_cursor_home(dialog_cursor);
    collapse_dialog();
  end
  else if stdlib.ui_key_is_end(ev.code) then
  begin
    dialog_cursor := stdlib.str_cursor_end(dialog_text, dialog_cursor);
    collapse_dialog();
  end
  else
  begin
    text_byte := stdlib.ui_key_text(ev.code);
    if text_byte >= byte(32) and text_byte < byte(127) then
    begin
      one[0] := text_byte;
      copied := core.substr_into(ch, one, 0, 1);
      if copied = 1 then
        insert_dialog(ch);
    end;
  end;
  dialog_scroll := stdlib.ui_text_field_reveal(
    dialog_text, dialog_cursor, dialog_scroll, (360 - 16) - 6);
end;

procedure dialog_key();
begin
  action := stdlib.ui_dialog_key(ev.code, true);
  if action = 1 then
    apply_dialog_result(1)
  else if action = 2 then
    apply_dialog_result(2)
  else if action = 3 then
    dialog_focus := stdlib.ui_focus_next(dialog_focus, dialog_slots(), 1)
  else if action = 4 then
    dialog_focus := stdlib.ui_focus_next(dialog_focus, dialog_slots(), -1)
  else if dialog = 2 and dialog_focus = 0 then
    type_dialog_key();
end;

procedure dialog_pointer();
begin
  if dialog = 1 then
  begin
    hit_row := stdlib.ui_alert_hit(dx, dy, 360, AlertH, "OK", "Cancel", ev.x, ev.y);
    if ev.kind = PointerDown then
    begin
      if hit_row >= 0 then
        dialog_focus := hit_row;
    end
    else if ev.kind = PointerUp then
    begin
      if hit_row >= 0 and hit_row = dialog_focus then
        apply_dialog_result(hit_row + 1);
    end;
  end
  else if dialog = 2 then
  begin
    hit_row := stdlib.ui_input_dialog_hit(
      dx, dy, 360, InputH, "OK", "Cancel", ev.x, ev.y);
    if ev.kind = PointerDown then
    begin
      if hit_row >= 0 then
      begin
        dialog_focus := hit_row;
        if hit_row = 0 then
        begin
          dialog_cursor := stdlib.str_cursor_end(dialog_text, dialog_cursor);
          collapse_dialog();
        end;
      end;
    end
    else if ev.kind = PointerUp then
    begin
      if hit_row >= 1 and hit_row = dialog_focus then
        apply_dialog_result(hit_row);
    end;
  end;
end;

// ---- menu activation ----------------------------------------------------

procedure new_text();
begin
  field_text := "";
  field_cursor := 0;
  field_scroll := 0;
  sel_start := 0;
  sel_stop := 0;
  status_text := "New text";
end;

procedure menu_activate(menu: int; index: int);
begin
  if menu = 0 then
  begin
    file_actions := file_actions + 1;
    if index = 0 then
      new_text()
    else if index = 1 then
      open_input()
    else if index = 2 then
      open_alert();
    close_menu();
  end
  else if menu = 1 then
  begin
    edit_actions := edit_actions + 1;
    if index = 0 then
      status_text := "Cut"
    else if index = 1 then
      status_text := "Copy"
    else if index = 2 then
      status_text := "Paste"
    else if index = 4 then
    begin
      if focus_index = 2 then
      begin
        select_all_field();
        status_text := "Selected all";
      end
      else
        status_text := "Focus the text field first";
    end;
    close_menu();
  end
  else if menu = 2 then
  begin
    view_actions := view_actions + 1;
    if index = 0 then
    begin
      view_checked := stdlib.ui_menu_set(
        view_checked, 0, not stdlib.ui_menu_has(view_checked, 0));
      status_text := "Grid toggled";
    end
    else if index = 1 then
    begin
      view_checked := stdlib.ui_menu_set(
        view_checked, 1, not stdlib.ui_menu_has(view_checked, 1));
      status_text := "Ruler toggled";
    end;
    close_menu();
  end;
end;

// ---- key and pointer dispatch -----------------------------------------

procedure handle_key();
begin
  if dialog <> 0 then
    dialog_key()
  else if open_menu >= 0 then
  begin
    action := stdlib.ui_menu_key(ev.code, true);
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
      if selected >= 0 and menu_selectable(open_menu, selected) then
        menu_activate(open_menu, selected);
    end;
  end
  else if stdlib.ui_key_is_tab(ev.code) then
  begin
    if stdlib.ui_key_modifiers(ev.code) mod 2 = 1 then
      focus_index := stdlib.ui_focus_next(focus_index, ControlCount, -1)
    else
      focus_index := stdlib.ui_focus_next(focus_index, ControlCount, 1);
  end
  else if focus_index = 0 then
  begin
    if stdlib.ui_key_is_enter(ev.code) or stdlib.ui_key_code(ev.code) = 32 then
    begin
      button_clicks := button_clicks + 1;
      status_text := "Button pressed";
    end;
  end
  else if focus_index = 1 then
  begin
    if stdlib.ui_key_is_enter(ev.code) or stdlib.ui_key_code(ev.code) = 32 then
    begin
      checked := stdlib.ui_checkbox_toggle(checked);
      status_text := "Checkbox toggled";
    end;
  end
  else if focus_index = 2 then
    type_field_key()
  else if focus_index = 3 then
  begin
    if stdlib.ui_key_is_up(ev.code) then
    begin
      scroll_position := stdlib.ui_scrollbar_clamp(
        scroll_position - scroll_visible, scroll_visible, scroll_total);
      status_text := "Page up";
    end
    else if stdlib.ui_key_is_down(ev.code) then
    begin
      scroll_position := stdlib.ui_scrollbar_clamp(
        scroll_position + scroll_visible, scroll_visible, scroll_total);
      status_text := "Page down";
    end;
  end
  else if focus_index = 4 then
  begin
    if stdlib.ui_key_is_up(ev.code) then
    begin
      list_selected := stdlib.ui_list_move(list_selected, list_count, -1);
      list_top := stdlib.ui_list_ensure_visible(
        list_selected, list_top, stdlib.ui_list_rows(ListH));
      status_text := list_items[list_selected];
    end
    else if stdlib.ui_key_is_down(ev.code) then
    begin
      list_selected := stdlib.ui_list_move(list_selected, list_count, 1);
      list_top := stdlib.ui_list_ensure_visible(
        list_selected, list_top, stdlib.ui_list_rows(ListH));
      status_text := list_items[list_selected];
    end;
  end;
end;

procedure icon_down();
begin
  now := core.clock();
  cell := stdlib.ui_icon_grid_hit(
    icx, icy, 2, IconCount, IconCellW, IconCellH, ev.x, ev.y);
  if cell >= 0 then
  begin
    icon_selected := cell;
    if stdlib.ui_double_click(last_click_ns, now, last_click_icon = cell) then
    begin
      opens := opens + 1;
      if cell = 0 then
        status_text := "Opened Folder"
      else
        status_text := "Opened Document";
      last_click_ns := uint(0);
      last_click_icon := -1;
    end
    else
    begin
      last_click_ns := now;
      last_click_icon := cell;
      status_text := "Icon selected";
    end;
  end;
end;

procedure handle_pointer();
begin
  if ev.kind = PointerDown then
  begin
    bar_hit := stdlib.ui_menu_bar_hit(
      menu_x, menu_y, menu_w, titles, MenuCount, ev.x, ev.y);
    if bar_hit >= 0 then
    begin
      open_menu_at(bar_hit);
      return;
    end;
    if open_menu >= 0 then
    begin
      menu_select(open_menu, ev.x, ev.y);
      if hit_row >= 0 then
      begin
        if menu_selectable(open_menu, hit_row) then
          selected := hit_row;
      end
      else
        close_menu();
      return;
    end;
    if stdlib.ui_button_hit(bx, by, ButtonW, ButtonH, ev.x, ev.y) then
    begin
      focus_index := 0;
      button_down := true;
    end
    else if stdlib.ui_checkbox_hit(cx, cy, ev.x, ev.y) then
    begin
      focus_index := 1;
      checkbox_down := true;
    end
    else if stdlib.ui_rect_hit(tx, ty, FieldW, FieldH, ev.x, ev.y) then
    begin
      focus_index := 2;
      field_cursor := stdlib.str_cursor_end(field_text, field_cursor);
      collapse_field();
      reveal_field();
    end
    else if stdlib.ui_rect_hit(sbx, sby, ScrollW, ScrollH, ev.x, ev.y) then
    begin
      focus_index := 3;
      action := stdlib.ui_scrollbar_hit(
        sbx, sby, ScrollW, ScrollH, ev.x, ev.y,
        scroll_position, scroll_visible, scroll_total);
      if action = 1 then
      begin
        scroll_position := stdlib.ui_scrollbar_clamp(
          scroll_position - scroll_visible, scroll_visible, scroll_total);
        status_text := "Page up";
      end
      else if action = 2 then
      begin
        scroll_position := stdlib.ui_scrollbar_clamp(
          scroll_position + scroll_visible, scroll_visible, scroll_total);
        status_text := "Page down";
      end;
    end
    else
    begin
      hit_row := stdlib.ui_list_hit(
        lx, ly, ListW, ListH, ev.x, ev.y, list_count, list_top);
      if hit_row >= 0 then
      begin
        focus_index := 4;
        list_selected := hit_row;
        list_top := stdlib.ui_list_ensure_visible(
          list_selected, list_top, stdlib.ui_list_rows(ListH));
        status_text := list_items[list_selected];
      end
      else if stdlib.ui_trash_hit(ttx, tty, TrashSize, TrashSize, ev.x, ev.y) then
        status_text := "Trash target"
      else
        icon_down();
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
          menu_activate(open_menu, hit_row);
      end;
      return;
    end;
    if stdlib.ui_button_pressed(
      button_down, stdlib.ui_button_hit(bx, by, ButtonW, ButtonH, ev.x, ev.y)) then
    begin
      button_clicks := button_clicks + 1;
      status_text := "Button pressed";
    end;
    button_down := false;
    if checkbox_down and stdlib.ui_checkbox_hit(cx, cy, ev.x, ev.y) then
    begin
      checked := stdlib.ui_checkbox_toggle(checked);
      status_text := "Checkbox toggled";
    end;
    checkbox_down := false;
  end;
end;

// ---- layout ------------------------------------------------------------

// Re-reads the content rectangle after the service moved the window and
// recomputes every cached absolute control origin (spec 0011, W12/W17/W28;
// ADR 0046). The event handlers and drawing use these screen coordinates.
procedure refresh_layout();
begin
  status := syslib.window_content_rect(id, rect);
  if status = Ok then
  begin
    menu_x := rect.x + 8;
    menu_y := rect.y + 8;
    menu_w := rect.w - 16;
    bx := rect.x + 16;
    by := rect.y + 60;
    cx := rect.x + 16;
    cy := rect.y + 96;
    tx := rect.x + 16;
    ty := rect.y + 128;
    lx := rect.x + 16;
    ly := rect.y + 170;
    docx := rect.x + 400;
    docy := rect.y + 60;
    sbx := rect.x + 728;
    sby := rect.y + 60;
    icx := rect.x + 400;
    icy := rect.y + 232;
    ttx := rect.x + 660;
    tty := rect.y + 232;
    dx := rect.x + 200;
    dy := rect.y + 170;
  end;
end;

// Poll and handle every queued event. A repaint happens after the batch when
// `auto_present` is set; check mode paints explicitly before its pixel probes.
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
      begin
        refresh_layout();
        dirty := true;
      end
      else if ev.kind = Close then
      begin
        // A close-box press asks the owner to close this window (spec 0011,
        // W23). This finite check closes it at the end of the run and exits.
        if ev.window = id then
          closed := true;
      end
      else if ev.kind = KeyDown then
      begin
        handle_key();
        dirty := true;
      end
      else if ev.kind = PointerDown or ev.kind = PointerUp then
      begin
        if dialog <> 0 then
          dialog_pointer()
        else
          handle_pointer();
        dirty := true;
      end;
    end
    else
      running := false;
  end;
  if dirty then
  begin
    if auto_present then
      present_all();
    dirty := false;
  end;
end;

// ---- injection ---------------------------------------------------------

// Clamp the pointer to the origin, discard the queued move and place it on an
// absolute screen point; the window service sees one move from a known state.
procedure pointer_to(x: int; y: int);
begin
  status := inject.pointer_to(x, y);
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

procedure click_at(x: int; y: int);
begin
  pointer_to(x, y);
  pointer_down();
  pointer_up();
end;

procedure inject_key(key_code: int; text_byte: int);
begin
  status := inject.inject_key(key_code, text_byte);
  drain_events();
end;

// ---- drawing -----------------------------------------------------------

procedure sync_focus_text();
begin
  if dialog <> 0 then
    focus_text := "Dialog"
  else if focus_index = 0 then
    focus_text := "Button"
  else if focus_index = 1 then
    focus_text := "Checkbox"
  else if focus_index = 2 then
    focus_text := "Text Field"
  else if focus_index = 3 then
    focus_text := "Scrollbar"
  else
    focus_text := "List";
end;

// HH:MM from the monotonic clock, right-aligned in the menu bar.
procedure draw_clock(right: int);
begin
  now := core.clock();
  used := int((now div uint(1000000000)) div uint(60));
  clk_h := "00";
  clk_m := "00";
  used := core.format_int_into(clk_h, (used div 60) mod 24, uint(10), uint(2));
  used := core.format_int_into(clk_m, used mod 60, uint(10), uint(2));
  status := syslib.draw_text(right - 40, menu_y + 7, clk_h, black);
  status := syslib.draw_text(right - 24, menu_y + 7, ":", black);
  status := syslib.draw_text(right - 16, menu_y + 7, clk_m, black);
end;

procedure draw_menu();
begin
  status := stdlib.ui_menu_bar(menu_x, menu_y, menu_w, titles, MenuCount, open_menu);
  draw_clock(menu_x + menu_w - 8);
end;

procedure draw_dropdown();
begin
  if open_menu = 0 then
  begin
    dx := menu_x_at(0);
    dy := menu_y + 22;
    status := stdlib.ui_menu_dropdown(
      dx, dy, MenuW, file_items, file_keys, file_count, selected,
      uint(0), file_disabled, true);
  end
  else if open_menu = 1 then
  begin
    dx := menu_x_at(1);
    dy := menu_y + 22;
    status := stdlib.ui_menu_dropdown(
      dx, dy, MenuW, edit_items, edit_keys, edit_count, selected,
      uint(0), edit_disabled, true);
  end
  else if open_menu = 2 then
  begin
    dx := menu_x_at(2);
    dy := menu_y + 22;
    status := stdlib.ui_menu_dropdown(
      dx, dy, MenuW, view_items, view_keys, view_count, selected,
      view_checked, view_disabled, true);
  end;
end;

procedure draw_controls();
begin
  status := stdlib.ui_button(
    bx, by, ButtonW, ButtonH, "Press", button_down, focus_index = 0);
  status := stdlib.ui_checkbox(cx, cy, "Enabled", checked, focus_index = 1);
  status := stdlib.ui_text_field(
    tx, ty, FieldW, FieldH, field_text, field_cursor, field_scroll,
    sel_start, sel_stop, focus_index = 2);
  status := stdlib.ui_list(
    lx, ly, ListW, ListH, list_items, list_count, list_selected, list_top,
    focus_index = 4);
end;

procedure draw_document();
var
  row: int;
begin
  status := stdlib.ui_frame(docx, docy, DocW, DocH);
  row := 0;
  while row < DocVisible and scroll_position + row < DocTotal do
  begin
    status := syslib.draw_text(
      docx + 8, docy + 8 + row * 16, doc_lines[scroll_position + row], black);
    row := row + 1;
  end;
  status := stdlib.ui_scrollbar(
    sbx, sby, ScrollW, ScrollH, scroll_position, scroll_visible, scroll_total,
    focus_index = 3);
end;

function icon_x_at(index: int): int;
begin
  return stdlib.ui_icon_cell_x(index, icx, IconCellW) + (IconCellW - IconSize) div 2;
end;

function icon_y_at(index: int): int;
begin
  return stdlib.ui_icon_cell_y(0, icy, IconCellH) + 4;
end;

procedure draw_icon(index: int);
var
  ix: int;
  iy: int;
  label_y: int;
  text_w: int;
  label_x: int;
begin
  ix := icon_x_at(index);
  iy := icon_y_at(index);
  status := syslib.fill_rect(ix, iy, IconSize, IconSize, white);
  if index = 0 then
    status := stdlib.ui_icon(ix, iy, folder_rows, black)
  else
    status := stdlib.ui_icon(ix, iy, doc_rows, black);
  label_y := stdlib.ui_icon_label_y(iy);
  text_w := stdlib.ui_text_width(icon_labels[index]);
  label_x := ix + (IconSize - text_w) div 2;
  if icon_selected = index then
  begin
    status := syslib.fill_rect(label_x - 2, label_y - 2, text_w + 4, 12, accent);
    status := syslib.draw_text(label_x, label_y, icon_labels[index], white);
    status := stdlib.ui_selection_rect(ix + 2, iy + 2, IconSize - 4, IconSize - 4);
  end
  else
    status := stdlib.ui_icon_label(label_x, label_y, icon_labels[index]);
end;

procedure draw_icons();
begin
  status := stdlib.ui_focus_ring(icx, icy, IconCellW * IconCount, IconCellH);
  draw_icon(0);
  draw_icon(1);
  status := syslib.fill_rect(ttx, tty, TrashSize, TrashSize, white);
  status := syslib.draw_rect(ttx, tty, TrashSize, TrashSize, black);
  status := stdlib.ui_center_text(ttx, tty, TrashSize, TrashSize, "Trash", black);
end;

procedure draw_status();
begin
  status := syslib.draw_text(rect.x + 16, rect.y + 512, status_text, black);
  status := syslib.draw_text(rect.x + 16, rect.y + 530, "Focus: ", black);
  status := syslib.draw_text(rect.x + 72, rect.y + 530, focus_text, black);
end;

procedure draw_dialog();
begin
  if dialog = 1 then
    status := stdlib.ui_alert(
      dx, dy, 360, AlertH, "About Widgets",
      "The widgets demo exercises every control.\nAll checks ran deterministically.",
      "OK", "Cancel", dialog_focus)
  else if dialog = 2 then
    status := stdlib.ui_input_dialog(
      dx, dy, 360, InputH, "Rename", "Enter a new name:", "OK", "Cancel",
      dialog_text, dialog_cursor, dialog_scroll,
      dialog_sel_start, dialog_sel_stop, dialog_focus);
end;

procedure draw_all();
begin
  sync_focus_text();
  status := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, white);
  status := stdlib.ui_separator(rect.x + 8, rect.y + 52, rect.w - 16);
  status := syslib.draw_text(rect.x + 16, rect.y + 36, "M2.4.6 widgets", black);
  draw_controls();
  draw_document();
  draw_icons();
  draw_menu();
  draw_dropdown();
  draw_status();
  draw_dialog();
end;

procedure present_all();
begin
  draw_all();
  present_status := syslib.window_present(id);
end;

// ---- kernel proof ------------------------------------------------------

procedure kernel_proof();
begin
  auto_present := false;
  present_all();

  // Baseline: the client panel, the menu bar, every control and the icons.
  verify("content filled", syslib.get_pixel(rect.x + 2, rect.y + 2) = white);
  verify("menu bar white", syslib.get_pixel(menu_x + 300, menu_y + 5) = white);
  verify("menu bar rule", syslib.get_pixel(menu_x + 300, menu_y + 21) = black);
  verify("button face", syslib.get_pixel(bx + 2, by + 2) = white);
  verify("button border", syslib.get_pixel(bx, by) = black);
  verify("checkbox border", syslib.get_pixel(cx, cy) = black);
  verify("checkbox clear", syslib.get_pixel(cx + 2, cy + 5) = white);
  verify("field border", syslib.get_pixel(tx, ty) = black);
  verify("document frame", syslib.get_pixel(docx, docy) = black);
  verify("scrollbar thumb", syslib.get_pixel(sbx + 3, sby + 3) = gray);
  verify("list frame", syslib.get_pixel(lx, ly) = black);
  verify("list selection", syslib.get_pixel(lx + 2, ly + 4) = accent);
  verify("icon plate", syslib.get_pixel(icon_x_at(0) + 1, icon_y_at(0) + 1) = white);
  verify("folder icon", syslib.get_pixel(icon_x_at(0) + 9, icon_y_at(0) + 8) = black);

  // 1. Button: the press paints the gray face, a release inside activates it.
  pointer_to(bx + ButtonW div 2, by + ButtonH div 2);
  pointer_down();
  verify("button held", button_down and focus_index = 0);
  present_all();
  verify("pressed face", syslib.get_pixel(bx + 2, by + 2) = gray);
  pointer_up();
  verify("button click counted", button_clicks = 1);
  verify("button status", status_text = "Button pressed");
  present_all();
  verify("released face", syslib.get_pixel(bx + 2, by + 2) = white);
  verify("button focus ring", syslib.get_pixel(bx + 3, by + 3) = black);

  // 2. Checkbox: a click toggles it; Space and Enter toggle the focused box.
  pointer_to(cx + 5, cy + 5);
  pointer_down();
  verify("checkbox focus", focus_index = 1);
  pointer_up();
  verify("checkbox toggled", checked);
  present_all();
  verify("check mark", syslib.get_pixel(cx + 2, cy + 5) = black);
  verify("checkbox focus ring", syslib.get_pixel(cx + 14, cy) = black);
  inject_key(32, 32);
  verify("space toggles off", not checked);
  present_all();
  verify("check mark cleared", syslib.get_pixel(cx + 2, cy + 5) = white);
  inject_key(0x103, 0);
  verify("enter toggles on", checked);

  // 3. Text field: click focuses it; typing, selection, scrolling and a
  // multi-byte code point all go through the M2.4.1 helpers.
  click_at(tx + 20, ty + 10);
  verify("field focused", focus_index = 2);
  inject_key(72, 104);
  inject_key(73, 105);
  verify("typed hi", field_text = "hi" and field_cursor = 2);
  present_all();
  verify("typed glyph", syslib.get_pixel(tx + 3, ty + 7) = black);
  verify("caret at end", syslib.get_pixel(tx + 19, ty + 7) = black);
  select_all_field();
  present_all();
  verify("selection spans", sel_start = 0 and sel_stop = 2);
  verify("selection accent", syslib.get_pixel(tx + 7, ty + 7) = accent);
  verify("selection caret", syslib.get_pixel(tx + 19, ty + 7) = black);
  insert_field("The quick brown fox jumps");
  verify("long text", int(core.len(field_text)) = 25 and field_cursor = 25);
  verify("field scrolled", field_scroll > 0);
  present_all();
  verify("scrolled caret", syslib.get_pixel(tx + 171, ty + 7) = black);
  select_all_field();
  field_scroll := 0;
  insert_field("é");
  verify("multibyte length", int(core.len(field_text)) = 2 and field_cursor = 2);
  present_all();
  verify("multibyte caret", syslib.get_pixel(tx + 11, ty + 7) = black);
  inject_key(0x101, 0);
  verify("multibyte backspace", int(core.len(field_text)) = 0 and field_cursor = 0);
  present_all();
  verify("caret home", syslib.get_pixel(tx + 3, ty + 7) = black);
  inject_key(87, 87);
  inject_key(73, 105);
  inject_key(68, 100);
  inject_key(71, 103);
  inject_key(69, 101);
  inject_key(84, 116);
  inject_key(83, 115);
  verify("typed widgets", field_text = "Widgets" and field_cursor = 7);

  // 4. Scrollbar: a click below the thumb pages down; Up and Down page too.
  click_at(sbx + 8, sby + 140);
  verify("scrollbar focus", focus_index = 3);
  verify("page down", scroll_position = 8);
  present_all();
  verify("thumb moved", syslib.get_pixel(sbx + 3, sby + 3) = white);
  verify("thumb at page", syslib.get_pixel(sbx + 3, sby + 78) = gray);
  inject_key(0x112, 0);
  verify("key page up", scroll_position = 0);
  inject_key(0x113, 0);
  verify("key page down", scroll_position = 8);

  // 5. List: Down moves the selection and scrolls it into view; a row click
  // selects through the shared hit test.
  inject_key(0x102, 0);
  verify("tab to list", focus_index = 4);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  verify("list moved", list_selected = 6 and list_top = 1);
  present_all();
  verify("list selected row", syslib.get_pixel(lx + 2, ly + 85) = accent);
  click_at(lx + 10, ly + 21);
  verify("list click selects", focus_index = 4 and list_selected = 2);

  // 6. Menus: File opens, the highlight skips a separator and the disabled
  // Quit, and Enter opens the alert.
  click_at(menu_x + 12, menu_y + 10);
  verify("file opened", open_menu = 0 and selected = 0);
  present_all();
  verify("file cell inverted", syslib.get_pixel(menu_x + 6, menu_y + 3) = black);
  verify("edit cell plain", syslib.get_pixel(menu_x + 54, menu_y + 3) = white);
  verify("dropdown border", syslib.get_pixel(menu_x + 4, menu_y + 22) = black);
  verify("file row accent", syslib.get_pixel(menu_x + 6, menu_y + 24) = accent);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  verify("file skips to About", selected = 2);
  inject_key(0x103, 0);
  verify("alert opened", dialog = 1 and file_actions = 1);
  present_all();
  verify("alert frame", syslib.get_pixel(dx, dy) = black);
  verify("alert title stripe", syslib.get_pixel(dx + 2, dy + 2) = black);
  verify("alert message", syslib.get_pixel(dx + 8, dy + 24) = black);
  verify("default double border", syslib.get_pixel(dx + 198, dy + 130) = black);
  verify("default focus ring", syslib.get_pixel(dx + 199, dy + 131) = black);
  inject_key(0x102, 0);
  verify("tab to cancel", dialog_focus = 1);
  present_all();
  verify("cancel focus ring", syslib.get_pixel(dx + 283, dy + 131) = black);
  verify("default focus cleared", syslib.get_pixel(dx + 199, dy + 131) = white);
  inject_key(0x103, 0);
  verify("enter takes default", dialog = 0 and alert_results = 1 and alert_cancels = 0);
  present_all();
  verify("alert erased", syslib.get_pixel(dx, dy) = white);

  // Escape cancels the alert.
  click_at(menu_x + 12, menu_y + 10);
  inject_key(0x113, 0);
  inject_key(0x113, 0);
  inject_key(0x103, 0);
  verify("alert reopened", dialog = 1 and file_actions = 2);
  inject_key(0x100, 0);
  verify("escape cancels alert", dialog = 0 and alert_cancels = 1 and alert_results = 2);

  // 7. Input dialog: File > Rename opens it, typing edits its own buffer, and
  // Escape cancels without touching the field.
  click_at(menu_x + 12, menu_y + 10);
  inject_key(0x113, 0);
  inject_key(0x103, 0);
  verify("input opened", dialog = 2 and file_actions = 3);
  present_all();
  verify("input prompt", syslib.get_pixel(dx + 8, dy + 24) = black);
  verify("input field border", syslib.get_pixel(dx + 8, dy + 44) = black);
  verify("input caret end", syslib.get_pixel(dx + 67, dy + 51) = black);
  inject_key(82, 114);
  verify("dialog typed", dialog_text = "Widgetsr" and dialog_cursor = 8);
  inject_key(0x102, 0);
  inject_key(0x102, 0);
  inject_key(0x102, 0);
  verify("dialog focus wraps", dialog_focus = 0);
  inject_key(0x100, 0);
  verify("input cancelled", dialog = 0 and input_cancels = 1);
  verify("field untouched", field_text = "Widgets" and field_cursor = 7);

  // Reopen, append, and commit with a pointer click on OK (slot 1).
  click_at(menu_x + 12, menu_y + 10);
  inject_key(0x113, 0);
  inject_key(0x103, 0);
  verify("input reopened", dialog = 2 and file_actions = 4);
  inject_key(82, 82);
  inject_key(69, 101);
  inject_key(80, 112);
  inject_key(79, 111);
  inject_key(82, 114);
  inject_key(84, 116);
  verify("dialog appended", dialog_text = "WidgetsReport" and dialog_cursor = 13);
  click_at(dx + 220, dy + 180);
  verify("ok click focuses", dialog = 0);
  verify("input committed", input_commits = 1 and field_text = "WidgetsReport");
  verify("field cursor committed", field_cursor = 13);
  present_all();
  verify("committed glyph", syslib.get_pixel(tx + 3, ty + 7) = black);

  // 8. View: toggle the Ruler check mark, then refuse the disabled Zoom row.
  click_at(menu_x + 110, menu_y + 10);
  verify("view opened", open_menu = 2 and selected = 0);
  present_all();
  verify("ruler mark absent", syslib.get_pixel(menu_x + 111, menu_y + 45) = white);
  inject_key(0x113, 0);
  verify("view moved", selected = 1);
  inject_key(0x103, 0);
  verify("ruler toggled", stdlib.ui_menu_has(view_checked, 1) and view_actions = 1);
  verify("menu closed after action", open_menu = -1);
  click_at(menu_x + 110, menu_y + 10);
  present_all();
  verify("ruler mark black", syslib.get_pixel(menu_x + 111, menu_y + 45) = black);
  inject_key(0x113, 0);
  verify("view re-moved", selected = 1);
  present_all();
  verify("grid mark black", syslib.get_pixel(menu_x + 111, menu_y + 29) = black);
  verify("ruler row accent", syslib.get_pixel(menu_x + 102, menu_y + 41) = accent);
  pointer_to(menu_x + 110, menu_y + 75);
  pointer_down();
  verify("disabled not selected", selected = 1 and open_menu = 2);
  pointer_up();
  verify("disabled not activated", selected = 1 and view_actions = 1);
  inject_key(0x111, 0);
  verify("right switches to File", open_menu = 0 and selected = 0);
  inject_key(0x100, 0);
  verify("escape closed menu", open_menu = -1);
  present_all();
  verify("dropdown erased", syslib.get_pixel(menu_x + 102, menu_y + 24) = white);

  // 9. Icon strip: a click selects (outline and accent label), a same-target
  // second click opens through the 400 ms double-click rule.
  click_at(icon_x_at(0) + 16, icon_y_at(0) + 16);
  verify("icon selected", icon_selected = 0);
  present_all();
  verify("icon outline", syslib.get_pixel(icon_x_at(0) + 2, icon_y_at(0) + 2) = black);
  click_at(icon_x_at(1) + 16, icon_y_at(1) + 16);
  verify("second icon selected", icon_selected = 1);
  present_all();
  verify("first outline cleared", syslib.get_pixel(icon_x_at(0) + 2, icon_y_at(0) + 2) = white);
  verify("second outline", syslib.get_pixel(icon_x_at(1) + 2, icon_y_at(1) + 2) = black);
  pointer_to(icon_x_at(1) + 16, icon_y_at(1) + 16);
  pointer_down();
  pointer_up();
  verify("double click opens", opens = 1);
  verify("open status", status_text = "Opened Document");

  // 10. Tab/Shift+Tab wrap around the five controls; Enter activates the
  // focused button.
  inject_key(0x102, 0);
  verify("tab wraps to button", focus_index = 0);
  inject_key(0x102, 256);
  verify("shift tab wraps back to list", focus_index = 4);
  inject_key(0x102, 0);
  verify("tab wraps again", focus_index = 0);
  present_all();
  verify("button ring after wrap", syslib.get_pixel(bx + 3, by + 3) = black);
  inject_key(0x103, 0);
  verify("enter activates button", button_clicks = 2);

  status := syslib.window_close(id);
  verify("window closed", status = Ok);
end;

// ---- shot state ---------------------------------------------------------

procedure draw_shot();
begin
  checked := true;
  field_text := "Hello, world";
  field_cursor := 5;
  field_scroll := 0;
  sel_start := 0;
  sel_stop := 5;
  scroll_position := 8;
  list_selected := 2;
  list_top := 0;
  view_checked := stdlib.ui_menu_set(view_checked, 0, true);
  view_checked := stdlib.ui_menu_set(view_checked, 1, true);
  icon_selected := 1;
  focus_index := 2;
  open_menu_at(2);
  selected := 0;
  dialog := 2;
  dialog_focus := 0;
  dialog_text := "Hello, world";
  dialog_cursor := 5;
  dialog_scroll := 0;
  dialog_sel_start := 0;
  dialog_sel_stop := 5;
  status_text := "Composed widgets demo";
  present_all();
  out.say("widgets: ready\n");
  flushed := core.clock();
  while core.clock() - flushed < uint(3000000000) do
    at := 0;
  status := syslib.window_close(id);
  out.say("widgets: done\n");
end;

procedure report_and_exit();
begin
  if failed then
  begin
    out.say("widgets: failed check ");
    at := core.format_int_into(digits, fail_step, uint(10), uint(0));
    out.say(digits);
    out.say(" (");
    out.say(fail_label);
    out.say(")\n");
  end
  else
    out.say("widgets: ok\n");
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  fail_step := 0;
  step := 0;
  shot := args = "shot";
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  gray := stdlib.ui_gray();
  accent := stdlib.ui_accent();
  focus_index := 0;
  button_clicks := 0;
  button_down := false;
  checkbox_down := false;
  checked := false;
  field_text := "";
  field_cursor := 0;
  field_scroll := 0;
  sel_start := 0;
  sel_stop := 0;
  scroll_position := 0;
  scroll_visible := DocVisible;
  scroll_total := DocTotal;
  list_selected := 0;
  list_top := 0;
  icon_selected := -1;
  last_click_ns := uint(0);
  last_click_icon := -1;
  opens := 0;
  dialog := 0;
  dialog_focus := 0;
  dialog_text := "";
  dialog_cursor := 0;
  dialog_scroll := 0;
  dialog_sel_start := 0;
  dialog_sel_stop := 0;
  alert_results := 0;
  alert_cancels := 0;
  input_commits := 0;
  input_cancels := 0;
  status_text := "Ready";
  open_menu := -1;
  selected := -1;
  file_actions := 0;
  edit_actions := 0;
  view_actions := 0;
  auto_present := true;
  dirty := false;
  closed := false;

  load_data();
  pure_checks();

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
  begin
    host_checks();
    if failed then
      report_and_exit()
    else
      out.say("widgets: host ok\n");
  end
  else if screen_w < uint(64 + 760 + 20) or screen_h < uint(64 + 560 + 20) then
    out.say("widgets: screen too small\n")
  else
  begin
    status := syslib.window_open(64, 64, 760, 560, "Widgets", id);
    if status <> Ok then
      out.say("widgets: no window\n")
    else
    begin
      refresh_layout();
      if status <> Ok then
        out.say("widgets: no content rect\n")
      else
      begin
        // Flush anything the boot windows queued so the injected stream starts
        // from a known state; our own panel is repainted immediately.
        flushed := syslib.event_flush();
        if shot then
          draw_shot()
        else
          kernel_proof();
        if closed then
          status := syslib.window_close(id);
        report_and_exit();
      end;
    end;
  end;
end;

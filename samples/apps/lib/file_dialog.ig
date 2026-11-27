unit file_dialog;

// Importable file-open/save dialogs (M9.3, spec 0018 M5/M6, spec 0013 "The text
// editor"). It is ordinary `/lib` module source: no `main`, no top-level `var`
// state, no stream endpoint and no kernel service. The calling application
// supplies the path it is currently showing and receives a chosen path, or a
// null-like cancel.
//
//   value := file_dialog.open_file(path, chosen);
//   if value = Ok then ...open(chosen)... else ...cancelled...
//
//   value := file_dialog.save_file(suggested, chosen);
//   if value = Ok then ...write(chosen)... else ...cancelled...
//
// `save_file` shares the listing and navigation with `open_file` and adds a
// name field and a `Save` button. It refuses an existing target rather than
// overwriting it; the editor writes through its temp+rename path after the
// dialog returns.
//
// `open_file` opens a normal task-owned window (title `Open File`; the window
// service owns the frame, border, title bar and close box) and runs the modal
// loop itself. The start directory is the directory part of `initial` when
// that directory is listable (`base/child` starts in `base` with `child`
// selected); otherwise the root. Entries come from `core.fs_list` in image
// order, up to 96: kind 1 (directory) and kind 0 (regular file) are selectable,
// kind 2 is shown gray and refused. The caller's window stays behind; the
// dialog reads only events addressed to its own window, so the caller is
// blocked for the dialog's lifetime.
//
// Keys: Up/Down, PageUp/PageDown, Home/End, Enter (descend into a directory or
// return a file), Backspace/Ctrl+Up (parent, a no-op at the root) and Escape
// (cancel). Mouse: a row click selects, a double-click opens, the `Up` button
// goes to the parent, `Open` and `Cancel` act, the scrollbar track pages and
// the thumb drags. A thumb drag is deferred (M16.8): the held thumb moves alone
// in the scrollbar strip, and the listing jumps to the dropped position when
// the button is released, so no drag step redraws the rows. A close-box press
// is an owner `EventKind.Close` and cancels. Only a visible change repaints
// (the M9.2 lesson); an owner `Expose` re-reads the content rectangle, so a
// dragged dialog redraws at its new origin.
//
// The result is staged and published atomically: `chosen` is written only on
// `Ok`, with the whole path in one `core.substr_into`. A cancel returns
// `Invalid` and leaves `chosen` untouched; a path that does not fit the
// caller's 256-byte destination returns `CapacityExceeded`, also untouched. A
// disabled screen (the `langhost` path) returns `Invalid` without opening a
// window.
//
// No new opcode, builtin, `Host` method, file API or `stdlib` helper: drawing
// uses `syslib` and the `stdlib.ui_*` layer, and listing/reading/checking uses
// the existing `core.fs_*` surface. `langhost` never calls `open_file`.

uses core, stdlib, syslib;

const
  // Content size; the frame adds the service chrome (1-px border, 16-px title
  // bar).
  ContentW = 520;
  ContentH = 380;
  // Save mode adds one name field below the listing and a deeper status/button
  // band. The open dialog keeps its original 380-pixel content.
  SaveContentH = 400;
  SaveStatusTop = 336;
  SaveButtonTop = 360;
  NameTop = 308;
  NameLeft = 56;
  NameW = ContentW - NameLeft - 8;
  NameH = 20;
  ModeOpen = 0;
  ModeSave = 1;

  PathCapacity = 256;
  EntryCapacity = 96;
  NameCapacity = 64;

  LineH = 16;
  PathH = 24;
  ListLeft = 8;
  ListTop = 30;
  ListW = 472;
  ListRows = 17;
  BodyH = ListRows * LineH + 2;
  ScrollLeft = 488;
  ScrollW = 16;
  StatusTop = 312;
  ButtonTop = 340;
  ButtonW = 72;
  ButtonH = 22;
  UpW = 44;
  UpH = 18;

  MaxNameBytes = 30;
  MaxPathDirect = 50;
  MaxPathTail = 38;
  MaxStatusBytes = 60;

  // Palette (the M2.4 `stdlib` constants are functions, so the module keeps
  // its three colours as constants and converts at the call site).
  ColorBlack = 0x000000;
  ColorWhite = 0xFFFFFF;
  ColorGray = 0xCCCCCC;
  ColorAccent = 0x000080;

// The dialog's whole state. A module may not own top-level state, so one local
// record is declared in `open_file` and passed by reference to the helpers.
// The name/kinds arrays are assigned by `load_dir` before any read.
type Dialog = record
  id: int;
  rect: Rect;
  dir: string[PathCapacity];
  names: array[string[NameCapacity], EntryCapacity];
  kinds: array[uint, EntryCapacity];
  count: int;
  selected: int;
  top: int;
  truncated: bool;
  status: string[64];
  scroll_dragging: bool;
  scroll_grab_dy: int;
  // M16.8: the thumb position while held; `top` changes only on release.
  scroll_preview: int;
  last_click_ns: uint;
  last_click_item: int;
  done: bool;
  result: Status;
  picked: string[PathCapacity];
  // Save mode (M19.x): the same listing plus a file-name field. Open mode
  // leaves every one of these at its zero value and keeps the 380-pixel layout.
  mode: int;
  content_h: int;
  status_top: int;
  button_top: int;
  name: string[PathCapacity];
  name_cursor: int;
  name_scroll: int;
  name_sel_start: int;
  name_sel_stop: int;
  field_focus: bool;
end;

// `base/name`, or just `name` when `base` is empty, into a 256-byte image path.
// A result that does not fit is `CapacityExceeded` and writes nothing.
function join_path(base: const string[0]; entry_name: const string[0];
                   out: var string[PathCapacity]): Status;
var
  base_len: int;
  name_len: int;
  total: int;
  i: int;
  split: int;
  got: int;
  stage: bytes[PathCapacity];
begin
  base_len := int(core.len(base));
  name_len := int(core.len(entry_name));
  if base_len = 0 then
  begin
    total := name_len;
    split := 0;
  end
  else
  begin
    total := base_len + 1 + name_len;
    split := base_len + 1;
  end;
  if total > PathCapacity then
    return CapacityExceeded;

  i := 0;
  while i < base_len do
  begin
    stage[i] := base[i];
    i := i + 1;
  end;
  if base_len > 0 then
    stage[base_len] := byte(47);   // '/'
  i := 0;
  while i < name_len do
  begin
    stage[split + i] := entry_name[i];
    i := i + 1;
  end;

  got := core.substr_into(out, stage, 0, total);
  if got = total then
    return Ok;
  if got = -6 then
    return CapacityExceeded;
  return Invalid;
end;

// True for `home` and `home/...`, the only writable tree (spec 0019, A2).
function is_home_path(value: const string[0]): bool;
begin
  return value = "home" or stdlib.starts_with(value, "home/");
end;

// The directory before the last `/` of `base`; `false` at the root. The result
// is `""` when `base` is a single component.
function parent_of(base: const string[0]; out: var string[PathCapacity]): bool;
var
  length: int;
  at: int;
  got: int;
begin
  length := int(core.len(base));
  got := core.substr_into(out, base, 0, 0);
  if length = 0 then
    return false;
  at := length - 1;
  while at > 0 and base[at] <> byte(47) do
    at := at - 1;
  if base[at] = byte(47) then
    got := core.substr_into(out, base, 0, at);
  return true;
end;

// Splits `initial` at its last `/`: `base/child` -> (`base`, `child`), `lib` ->
// (`""`, `lib`), `""` -> (`""`, `""`). The name is empty when it cannot fit the
// 64-byte component limit; only an exact name match is used for the start
// selection, so a too-long name simply starts at the first row.
procedure split_initial(initial: const string[0];
                        out_dir: var string[PathCapacity];
                        out_name: var string[NameCapacity]);
var
  length: int;
  at: int;
  start: int;
  name_len: int;
  got: int;
begin
  got := core.substr_into(out_dir, initial, 0, 0);
  got := core.substr_into(out_name, initial, 0, 0);
  length := int(core.len(initial));
  at := length - 1;
  while at >= 0 and initial[at] <> byte(47) do
    at := at - 1;
  if at < 0 then
  begin
    at := -1;
    start := 0;
  end
  else
    start := at + 1;
  if at > 0 then
    got := core.substr_into(out_dir, initial, 0, at);
  name_len := length - start;
  if name_len > 0 and name_len <= NameCapacity then
    got := core.substr_into(out_name, initial, start, name_len);
end;

// ---- listing --------------------------------------------------------------

function is_selectable(d: const Dialog; at: int): bool;
var
  kind: uint;
begin
  if at < 0 or at >= d.count then
    return false;
  kind := d.kinds[at];
  return kind = uint(0) or kind = uint(1);
end;

procedure ensure_visible(d: var Dialog);
begin
  if d.selected >= 0 then
    d.top := stdlib.ui_list_ensure_visible(d.selected, d.top, ListRows)
  else if d.top < 0 then
    d.top := 0;
  d.top := stdlib.ui_scrollbar_clamp(d.top, ListRows, d.count);
end;

procedure select_first(d: var Dialog);
var
  i: int;
begin
  d.selected := -1;
  i := 0;
  while i < d.count and d.selected < 0 do
  begin
    if is_selectable(d, i) then
      d.selected := i;
    i := i + 1;
  end;
  ensure_visible(d);
end;

procedure select_last(d: var Dialog);
var
  i: int;
begin
  d.selected := -1;
  i := d.count - 1;
  while i >= 0 and d.selected < 0 do
  begin
    if is_selectable(d, i) then
      d.selected := i;
    i := i - 1;
  end;
  ensure_visible(d);
end;

// Reads the current directory in image order until `NotFound`. 96 entries is
// the listing capacity; a longer directory reports the truncation and keeps
// the first 96. The selection starts at the first selectable row.
procedure load_dir(d: var Dialog);
var
  index: uint;
  name: string[NameCapacity];
  kind: uint;
  size: uint;
  st: Status;
begin
  d.count := 0;
  d.truncated := false;
  d.scroll_dragging := false;
  index := uint(0);
  st := core.fs_list(d.dir, index, name, kind, size);
  while st = Ok and d.count < EntryCapacity do
  begin
    d.names[d.count] := name;
    d.kinds[d.count] := kind;
    d.count := d.count + 1;
    index := index + uint(1);
    st := core.fs_list(d.dir, index, name, kind, size);
  end;
  if d.count >= EntryCapacity and st = Ok then
  begin
    d.truncated := true;
    d.status := "listing truncated (96)";
  end
  else if st <> NotFound then
    d.status := "cannot list this folder"
  else
    d.status := "";
  d.top := 0;
  d.selected := -1;
  select_first(d);
  d.last_click_ns := uint(0);
  d.last_click_item := -1;
end;

// Moves the selection by `delta` rows, skipping kind-2 entries. Without a
// selectable neighbour the selection stays put; the scrollbar clamp keeps the
// row visible.
procedure move_selection(d: var Dialog; delta: int);
var
  at: int;
  found: bool;
begin
  if d.count <= 0 then
  begin
    d.selected := -1;
    return;
  end;
  found := false;
  if delta < 0 then
  begin
    if d.selected < 0 then
      at := d.count
    else
      at := d.selected - 1;
    while at >= 0 and not found do
    begin
      if is_selectable(d, at) then
      begin
        d.selected := at;
        found := true;
      end
      else
        at := at - 1;
    end;
  end
  else if delta > 0 then
  begin
    if d.selected < 0 then
      at := -1
    else
      at := d.selected + 1;
    while at < d.count and not found do
    begin
      if is_selectable(d, at) then
      begin
        d.selected := at;
        found := true;
      end
      else
        at := at + 1;
    end;
  end;
  ensure_visible(d);
end;

// One page is `ListRows` single steps, so the same skip rule applies.
procedure page_selection(d: var Dialog; direction: int);
var
  step: int;
begin
  step := 0;
  while step < ListRows do
  begin
    move_selection(d, direction);
    step := step + 1;
  end;
end;

// After the scrollbar moves the view, keep a selectable row inside it.
procedure clamp_selection_to_view(d: var Dialog);
var
  at: int;
  stop: int;
  found: bool;
begin
  if d.selected < 0 then
    return;
  if d.selected >= d.top and d.selected < d.top + ListRows then
    return;
  at := d.top;
  if at < 0 then
    at := 0;
  stop := d.top + ListRows;
  if stop > d.count then
    stop := d.count;
  found := false;
  while at < stop and not found do
  begin
    if is_selectable(d, at) then
    begin
      d.selected := at;
      found := true;
    end
    else
      at := at + 1;
  end;
end;

// ---- navigation -----------------------------------------------------------

procedure go_up(d: var Dialog);
var
  parent: string[PathCapacity];
begin
  if parent_of(d.dir, parent) then
  begin
    d.dir := parent;
    load_dir(d);
  end;
end;

procedure cancel(d: var Dialog);
begin
  d.result := Invalid;
  d.done := true;
end;

// Enter/double-click/Open: a directory becomes the current listing, a regular
// file is staged into `d.picked` and closes the dialog, and every other kind
// is refused with a status line.
procedure activate(d: var Dialog);
var
  candidate: string[PathCapacity];
  probe_name: string[NameCapacity];
  probe_kind: uint;
  probe_size: uint;
  st: Status;
begin
  if d.selected < 0 or d.selected >= d.count then
  begin
    d.status := "nothing selected";
    return;
  end;
  if d.kinds[d.selected] = uint(1) then
  begin
    st := join_path(d.dir, d.names[d.selected], candidate);
    if st <> Ok then
    begin
      d.status := "path too long";
      return;
    end;
    st := core.fs_list(candidate, uint(0), probe_name, probe_kind, probe_size);
    if st <> Ok and st <> NotFound then
    begin
      d.status := "cannot open this folder";
      return;
    end;
    d.dir := candidate;
    load_dir(d);
  end
  else if d.kinds[d.selected] = uint(0) then
  begin
    st := join_path(d.dir, d.names[d.selected], d.picked);
    if st <> Ok then
    begin
      d.status := "path too long";
      return;
    end;
    d.result := Ok;
    d.done := true;
  end
  else
    d.status := "cannot open this entry";
end;

// Save-mode activation: a directory descends; a regular file fills the name
// field with that entry and selects it so typing replaces the name. Nothing is
// written from the listing itself.
procedure save_activate(d: var Dialog);
var
  candidate: string[PathCapacity];
  probe_name: string[NameCapacity];
  probe_kind: uint;
  probe_size: uint;
  length: int;
  copied: int;
  st: Status;
begin
  if d.selected < 0 or d.selected >= d.count then
  begin
    d.status := "nothing selected";
    return;
  end;
  if d.kinds[d.selected] = uint(1) then
  begin
    st := join_path(d.dir, d.names[d.selected], candidate);
    if st <> Ok then
    begin
      d.status := "path too long";
      return;
    end;
    st := core.fs_list(candidate, uint(0), probe_name, probe_kind, probe_size);
    if st <> Ok and st <> NotFound then
    begin
      d.status := "cannot open this folder";
      return;
    end;
    d.dir := candidate;
    load_dir(d);
  end
  else if d.kinds[d.selected] = uint(0) then
  begin
    length := int(core.len(d.names[d.selected]));
    copied := core.substr_into(d.name, d.names[d.selected], 0, length);
    if copied <> length then
    begin
      d.status := "name too long";
      return;
    end;
    d.name_cursor := stdlib.str_cursor_end(d.name, d.name_cursor);
    d.name_sel_start := 0;
    d.name_sel_stop := d.name_cursor;
    d.name_scroll := 0;
    d.field_focus := true;
    d.status := "name set";
  end
  else
    d.status := "cannot save this entry";
end;

// Save: join the current directory and name, insist on `home/`, and refuse an
// existing regular file or a directory. The editor writes the document through
// its temp+rename path after this returns.
function commit_save(d: var Dialog): bool;
var
  candidate: string[PathCapacity];
  size: uint;
  length: int;
  i: int;
  copied: int;
  st: Status;
begin
  length := int(core.len(d.name));
  if length = 0 then
  begin
    d.status := "name required";
    return true;
  end;
  if length > NameCapacity then
  begin
    d.status := "name too long";
    return true;
  end;
  i := 0;
  while i < length do
  begin
    if d.name[i] = byte(47) then
    begin
      d.status := "name has /";
      return true;
    end;
    i := i + 1;
  end;
  st := join_path(d.dir, d.name, candidate);
  if st <> Ok then
  begin
    d.status := "path too long";
    return true;
  end;
  if not is_home_path(candidate) then
  begin
    d.status := "choose home/";
    return true;
  end;
  st := core.fs_size(candidate, size);
  if st = Ok then
  begin
    d.status := "file exists";
    return true;
  end;
  if st = Invalid then
  begin
    d.status := "not a regular file";
    return true;
  end;
  if st <> NotFound then
  begin
    d.status := "cannot save";
    return true;
  end;
  copied := core.substr_into(
    d.picked, candidate, 0, int(core.len(candidate)));
  if copied <> int(core.len(candidate)) then
  begin
    d.status := "path too long";
    return true;
  end;
  d.result := Ok;
  d.done := true;
  return true;
end;

// One key while the save dialog's name field has focus: text editing, Tab to
// the listing, Enter to save, Escape to cancel. ASCII input is enough for a
// file name; the shared `stdlib` string helpers keep the buffer valid.
function handle_name_key(d: var Dialog; ev: const Event): bool;
var
  key: int;
  at: int;
  copied: int;
  text_byte: byte;
  one: bytes[1];
  ch: string[2];
  st: Status;
begin
  key := stdlib.ui_key_code(ev.code);
  if key = 0x100 then
  begin
    cancel(d);
    return true;
  end;
  if stdlib.ui_key_is_enter(ev.code) then
    return commit_save(d);
  if stdlib.ui_key_is_tab(ev.code) then
  begin
    d.field_focus := false;
    return true;
  end;
  if stdlib.ui_key_is_backspace(ev.code) then
  begin
    if d.name_sel_start <> d.name_sel_stop then
    begin
      st := stdlib.str_delete_into(
        d.name, d.name_sel_start, d.name_sel_stop - d.name_sel_start);
      d.name_cursor := d.name_sel_start;
      d.name_sel_stop := d.name_sel_start;
    end
    else
    begin
      st := stdlib.str_backspace(d.name, d.name_cursor);
      d.name_sel_start := d.name_cursor;
      d.name_sel_stop := d.name_cursor;
    end;
    d.status := "";
  end
  else if stdlib.ui_key_is_delete(ev.code) then
  begin
    if d.name_sel_start <> d.name_sel_stop then
    begin
      st := stdlib.str_delete_into(
        d.name, d.name_sel_start, d.name_sel_stop - d.name_sel_start);
      d.name_cursor := d.name_sel_start;
      d.name_sel_stop := d.name_sel_start;
    end
    else if d.name_cursor < int(core.len(d.name)) then
    begin
      at := stdlib.str_cursor_right(d.name, d.name_cursor);
      if at > d.name_cursor then
        st := stdlib.str_delete_into(
          d.name, d.name_cursor, at - d.name_cursor);
      d.name_sel_start := d.name_cursor;
      d.name_sel_stop := d.name_cursor;
    end;
    d.status := "";
  end
  else if stdlib.ui_key_is_left(ev.code) then
  begin
    d.name_cursor := stdlib.str_cursor_left(d.name, d.name_cursor);
    d.name_sel_start := d.name_cursor;
    d.name_sel_stop := d.name_cursor;
  end
  else if stdlib.ui_key_is_right(ev.code) then
  begin
    d.name_cursor := stdlib.str_cursor_right(d.name, d.name_cursor);
    d.name_sel_start := d.name_cursor;
    d.name_sel_stop := d.name_cursor;
  end
  else if stdlib.ui_key_is_home(ev.code) then
  begin
    d.name_cursor := stdlib.str_cursor_home(d.name_cursor);
    d.name_sel_start := d.name_cursor;
    d.name_sel_stop := d.name_cursor;
  end
  else if stdlib.ui_key_is_end(ev.code) then
  begin
    d.name_cursor := stdlib.str_cursor_end(d.name, d.name_cursor);
    d.name_sel_start := d.name_cursor;
    d.name_sel_stop := d.name_cursor;
  end
  else
  begin
    text_byte := stdlib.ui_key_text(ev.code);
    if text_byte >= byte(32) and text_byte < byte(127) then
    begin
      one[0] := text_byte;
      copied := core.substr_into(ch, one, 0, 1);
      if copied = 1 then
      begin
        st := stdlib.str_selection_replace(
          d.name, d.name_sel_start, d.name_sel_stop, ch);
        if st = Ok then
        begin
          d.name_cursor := d.name_sel_start;
          d.name_sel_stop := d.name_sel_start;
          d.status := "";
        end
        else
          d.status := "name too long";
      end;
    end;
  end;
  d.name_scroll := stdlib.ui_text_field_reveal(
    d.name, d.name_cursor, d.name_scroll, NameW - 6);
  return true;
end;

// ---- drawing --------------------------------------------------------------

procedure refresh_rect(d: var Dialog);
var
  r: Rect;
  st: Status;
begin
  st := syslib.window_content_rect(d.id, r);
  if st = Ok then
    d.rect := r;
end;

// Drawing and hit-testing are window-local (spec 0026 C8); `d.rect` is kept
// only for `syslib.present`'s screen rectangle (C7).
function list_x(d: const Dialog): int;
begin
  return ListLeft;
end;

function list_y(d: const Dialog): int;
begin
  return ListTop;
end;

// `igis:/` plus the current directory; a path too wide for the line keeps the
// prefix and its deepest components, cut at a code-point boundary.
procedure build_display_path(d: const Dialog; out: var string[64]);
var
  length: int;
  start: int;
  total: int;
  i: int;
  got: int;
  stage: bytes[64];
begin
  length := int(core.len(d.dir));
  stage[0] := byte(105);  // i
  stage[1] := byte(103);  // g
  stage[2] := byte(105);  // i
  stage[3] := byte(115);  // s
  stage[4] := byte(58);   // :
  stage[5] := byte(47);   // /
  if length <= MaxPathDirect then
  begin
    i := 0;
    while i < length do
    begin
      stage[6 + i] := d.dir[i];
      i := i + 1;
    end;
    total := 6 + length;
    start := 0;
  end
  else
  begin
    stage[6] := byte(46);   // .
    stage[7] := byte(46);   // .
    stage[8] := byte(46);   // .
    start := length - MaxPathTail;
    while start < length and not stdlib.str_boundary(d.dir, start) do
      start := start + 1;
    i := start;
    while i < length do
    begin
      stage[9 + (i - start)] := d.dir[i];
      i := i + 1;
    end;
    total := 9 + (length - start);
  end;
  got := core.substr_into(out, stage, 0, total);
  if got <> total then
    out := "";
end;

procedure draw_marker(x: int; y: int; entry_kind: uint; color: uint);
var
  st: Status;
begin
  if entry_kind = uint(1) then
  begin
    st := syslib.fill_rect(x, y + 1, 9, 6, color);
    st := syslib.fill_rect(x, y, 6, 2, color);
  end
  else
  begin
    st := syslib.draw_rect(x + 1, y, 7, 8, color);
    if entry_kind = uint(0) then
    begin
      st := syslib.fill_rect(x + 3, y + 2, 3, 1, color);
      st := syslib.fill_rect(x + 3, y + 4, 3, 1, color);
    end;
  end;
end;

procedure draw_path(d: const Dialog);
var
  display: string[64];
  up_x: int;
  up_y: int;
  st: Status;
begin
  build_display_path(d, display);
  st := syslib.draw_text(8, 4, display, uint(ColorBlack));
  up_x := ContentW - 8 - UpW;
  up_y := 3;
  st := stdlib.ui_button(up_x, up_y, UpW, UpH, "Up", false, false);
  st := stdlib.ui_separator(0, PathH - 1, ContentW);
end;

procedure draw_list(d: const Dialog);
var
  row: int;
  at: int;
  y: int;
  to: int;
  copied: int;
  color: uint;
  label: string[8];
  label_w: int;
  name_buf: string[NameCapacity];
  st: Status;
begin
  st := syslib.fill_rect(
    list_x(d), list_y(d), ListW, BodyH, uint(ColorWhite));
  st := syslib.draw_rect(list_x(d), list_y(d), ListW, BodyH, uint(ColorBlack));

  row := 0;
  while row < ListRows and d.top + row < d.count do
  begin
    at := d.top + row;
    y := list_y(d) + 1 + row * LineH;
    if at = d.selected then
    begin
      st := syslib.fill_rect(
        list_x(d) + 1, y, ListW - 2, LineH, uint(ColorAccent));
      color := uint(ColorWhite);
    end
    else if is_selectable(d, at) then
      color := uint(ColorBlack)
    else
      color := uint(ColorGray);

    draw_marker(list_x(d) + 8, y + 4, d.kinds[at], color);

    to := int(core.len(d.names[at]));
    if to > MaxNameBytes then
      to := MaxNameBytes;
    while to > 0 and not stdlib.str_boundary(d.names[at], to) do
      to := to - 1;
    copied := core.substr_into(name_buf, d.names[at], 0, to);
    if copied > 0 then
      st := syslib.draw_text(list_x(d) + 24, y + 4, name_buf, color);

    label_w := 0;
    if d.kinds[at] = uint(1) then
    begin
      label := "dir";
      label_w := 3 * 8;
    end
    else if d.kinds[at] = uint(2) then
    begin
      label := "other";
      label_w := 5 * 8;
    end
    else
    begin
      label := "file";
      label_w := 4 * 8;
    end;
    st := syslib.draw_text(
      list_x(d) + ListW - 8 - label_w, y + 4, label, color);
    row := row + 1;
  end;
end;

procedure draw_status(d: const Dialog);
var
  to: int;
  copied: int;
  width: int;
  text: string[64];
  hint: string[32];
  st: Status;
begin
  st := stdlib.ui_separator(0, d.status_top - 6, ContentW);
  to := int(core.len(d.status));
  if to > MaxStatusBytes then
    to := MaxStatusBytes;
  while to > 0 and not stdlib.str_boundary(d.status, to) do
    to := to - 1;
  copied := core.substr_into(text, d.status, 0, to);
  if copied > 0 then
    st := syslib.draw_text(
      8, d.status_top, text, uint(ColorBlack));
  if d.mode = ModeSave then
    hint := "Enter save  Tab list  Esc cancel"
  else
    hint := "Enter open  Bksp up  Esc cancel";
  width := stdlib.ui_text_width(hint);
  st := syslib.draw_text(
    ContentW - 8 - width, d.status_top,
    hint, uint(ColorGray));
end;

// The save dialog's file-name field: a label and the checked text field. The
// listing stops at 304, so the field and the deeper status band fit without
// changing the open dialog's layout.
procedure draw_name(d: const Dialog);
var
  st: Status;
begin
  st := syslib.draw_text(8, NameTop + 6, "Name:", uint(ColorBlack));
  st := stdlib.ui_text_field(
    NameLeft, NameTop, NameW, NameH,
    d.name, d.name_cursor, d.name_scroll,
    d.name_sel_start, d.name_sel_stop, d.field_focus);
end;

procedure draw_buttons(d: const Dialog);
var
  cancel_x: int;
  open_x: int;
  y: int;
  st: Status;
begin
  y := d.button_top;
  cancel_x := ContentW - 8 - ButtonW;
  open_x := cancel_x - 12 - ButtonW;
  if d.mode = ModeSave then
    st := stdlib.ui_dialog_button(
      open_x, y, ButtonW, ButtonH, "Save", false, true)
  else
    st := stdlib.ui_dialog_button(
      open_x, y, ButtonW, ButtonH, "Open", false, true);
  st := stdlib.ui_dialog_button(
    cancel_x, y, ButtonW, ButtonH, "Cancel", false, false);
end;

// The scrollbar strip at `position` over the current listing.
procedure draw_scrollbar_at(d: const Dialog; position: int);
var
  st: Status;
begin
  st := stdlib.ui_scrollbar(
    ScrollLeft, ListTop, ScrollW, BodyH,
    position, ListRows, d.count, false);
end;

// The scrollbar strip alone after a drag preview moved the thumb (M16.8): the
// canvas keeps the list rows, and `syslib.present` recomposites just this
// rectangle from the surface stack.
procedure preview_scrollbar(d: const Dialog);
var
  st: Status;
begin
  st := syslib.window_begin(d.id);
  draw_scrollbar_at(d, d.scroll_preview);
  // `present` takes a screen rectangle (spec 0026 C7): the one call that still
  // adds the content origin.
  st := syslib.present(
    d.rect.x + ScrollLeft, d.rect.y + ListTop, ScrollW, BodyH);
end;

// One complete client frame; the caller presents once. While a thumb is held
// the strip shows the preview position and the listing stays at `d.top`
// (M16.8).
procedure draw_dialog(d: const Dialog);
var
  st: Status;
begin
  st := syslib.fill_rect(0, 0, ContentW, d.content_h, uint(ColorWhite));
  draw_path(d);
  draw_list(d);
  if d.scroll_dragging then
    draw_scrollbar_at(d, d.scroll_preview)
  else
    draw_scrollbar_at(d, d.top);
  if d.mode = ModeSave then
    draw_name(d);
  draw_status(d);
  draw_buttons(d);
end;

// ---- pointer geometry -----------------------------------------------------

// The list row under a content point, or -1 outside the interior.
function index_at(d: const Dialog; px: int; py: int): int;
var
  row: int;
  at: int;
  top_y: int;
begin
  if not stdlib.ui_rect_hit(list_x(d), list_y(d), ListW, BodyH, px, py) then
    return -1;
  top_y := list_y(d) + 1;
  if py < top_y or py >= top_y + ListRows * LineH then
    return -1;
  row := (py - top_y) div LineH;
  at := d.top + row;
  if at < 0 or at >= d.count then
    return -1;
  return at;
end;

// Thumb height and travel for the dialog's listing, matching
// `stdlib.ui_scrollbar`: track `BodyH - 4`, at least 8 px tall, travelling
// over `track - thumb`.
procedure thumb_metrics(d: const Dialog; thumb_h: var int; travel: var int);
var
  track_h: int;
begin
  track_h := BodyH - 4;
  if d.count <= 0 then
    thumb_h := track_h
  else
  begin
    if ListRows <= 0 then
      thumb_h := 8
    else
      thumb_h := (track_h * ListRows) div d.count;
    if thumb_h < 8 then
      thumb_h := 8;
    if thumb_h > track_h then
      thumb_h := track_h;
  end;
  travel := track_h - thumb_h;
end;

// The current thumb geometry at `d.top`; the press needs its y for the grab
// offset. The geometry matches `stdlib.ui_scrollbar`.
procedure thumb_geometry(d: const Dialog;
                         thumb_y: var int; thumb_h: var int);
var
  max: int;
  pos: int;
  travel: int;
begin
  thumb_metrics(d, thumb_h, travel);
  max := d.count - ListRows;
  if max < 0 then
    max := 0;
  pos := stdlib.ui_scrollbar_clamp(d.top, ListRows, d.count);
  thumb_y := ListTop + 2;
  if max > 0 and travel > 0 then
    thumb_y := ListTop + 2 + (travel * pos) div max;
end;

// Ends a thumb drag and applies the preview to the view (M16.8): the listing
// jumps to the dropped position once, on release, and the selection is kept
// inside the new window.
function end_scroll_drag(d: var Dialog): bool;
begin
  d.scroll_dragging := false;
  if d.scroll_preview = d.top then
    return false;
  d.top := d.scroll_preview;
  clamp_selection_to_view(d);
  return true;
end;

// A scrollbar track action: 1 pages up, 2 pages down. Returns whether the
// view or the selection changed. (The thumb press is handled by the caller,
// which has the pointer's y for the grab offset.)
function scroll_action(d: var Dialog; action: int): bool;
var
  was_top: int;
  was_selected: int;
begin
  was_top := d.top;
  was_selected := d.selected;
  if action = 1 then
    d.top := stdlib.ui_scrollbar_clamp(d.top - ListRows, ListRows, d.count)
  else if action = 2 then
    d.top := stdlib.ui_scrollbar_clamp(d.top + ListRows, ListRows, d.count);
  clamp_selection_to_view(d);
  return d.top <> was_top or d.selected <> was_selected;
end;

// ---- input ----------------------------------------------------------------

// One pointer event addressed to this window. Returns whether a whole frame is
// needed. Plain moves are free; a thumb drag redraws only the scrollbar strip
// and applies the dropped position on release (M16.8); the press/release that
// start and end a drag are free themselves.
function handle_pointer(d: var Dialog; ev: const Event): bool;
var
  hit: int;
  action: int;
  now: uint;
  was_top: int;
  was_selected: int;
  thumb_y: int;
  thumb_h: int;
  travel: int;
  max_top: int;
  desired: int;
  preview: int;
  top_y: int;
  changed: bool;
begin
  if d.scroll_dragging then
  begin
    if ev.kind = PointerUp then
      return end_scroll_drag(d);
    if ev.kind = PointerMove then
    begin
      if ev.code mod uint(2) <> uint(1) then
        return end_scroll_drag(d);
      thumb_metrics(d, thumb_h, travel);
      max_top := d.count - ListRows;
      if max_top < 0 then
        max_top := 0;
      if travel > 0 and max_top > 0 then
      begin
        desired := ev.y - d.scroll_grab_dy;
        top_y := ListTop + 2;
        if desired < top_y then
          desired := top_y;
        if desired > top_y + travel then
          desired := top_y + travel;
        preview := ((desired - top_y) * max_top) div travel;
        preview := stdlib.ui_scrollbar_clamp(preview, ListRows, d.count);
        if preview <> d.scroll_preview then
        begin
          d.scroll_preview := preview;
          preview_scrollbar(d);
        end;
      end;
      return false;
    end;
    return false;
  end;

  if ev.kind <> PointerDown then
    return false;

  if stdlib.ui_button_hit(
       ContentW - 8 - UpW, 3, UpW, UpH, ev.x, ev.y) then
  begin
    go_up(d);
    return true;
  end;

  if stdlib.ui_button_hit(
       ContentW - 8 - ButtonW, d.button_top,
       ButtonW, ButtonH, ev.x, ev.y) then
  begin
    cancel(d);
    return true;
  end;

  if stdlib.ui_button_hit(
       ContentW - 8 - ButtonW - 12 - ButtonW, d.button_top,
       ButtonW, ButtonH, ev.x, ev.y) then
  begin
    if d.mode = ModeSave then
      commit_save(d)
    else
      activate(d);
    return true;
  end;

  if d.mode = ModeSave and stdlib.ui_rect_hit(
       NameLeft, NameTop, NameW, NameH, ev.x, ev.y) then
  begin
    d.field_focus := true;
    d.name_cursor := stdlib.str_cursor_end(d.name, d.name_cursor);
    d.name_sel_start := d.name_cursor;
    d.name_sel_stop := d.name_cursor;
    d.name_scroll := stdlib.ui_text_field_reveal(
      d.name, d.name_cursor, d.name_scroll, NameW - 6);
    return true;
  end;

  if stdlib.ui_rect_hit(
       ScrollLeft, ListTop, ScrollW, BodyH, ev.x, ev.y) then
  begin
    action := stdlib.ui_scrollbar_hit(
      ScrollLeft, ListTop, ScrollW, BodyH,
      ev.x, ev.y, d.top, ListRows, d.count);
    if action = 3 then
    begin
      thumb_geometry(d, thumb_y, thumb_h);
      d.scroll_dragging := true;
      d.scroll_preview := d.top;
      d.scroll_grab_dy := ev.y - thumb_y;
      return false;
    end;
    return scroll_action(d, action);
  end;

  hit := index_at(d, ev.x, ev.y);
  if hit < 0 then
    return false;
  d.field_focus := false;
  if not is_selectable(d, hit) then
  begin
    changed := d.status <> "cannot open this entry";
    d.status := "cannot open this entry";
    return changed;
  end;

  was_top := d.top;
  was_selected := d.selected;
  d.selected := hit;
  ensure_visible(d);
  now := core.clock();
  if stdlib.ui_double_click(d.last_click_ns, now, d.last_click_item = hit) then
  begin
    d.last_click_ns := uint(0);
    d.last_click_item := -1;
    if d.mode = ModeSave then
      save_activate(d)
    else
      activate(d);
    return true;
  end;
  d.last_click_ns := now;
  d.last_click_item := hit;
  return d.selected <> was_selected or d.top <> was_top;
end;

// One key event addressed to this window. Escape cancels; Enter activates;
// Backspace and Ctrl+Up go up; the arrows move and PageUp/PageDown page.
function handle_key(d: var Dialog; ev: const Event): bool;
var
  key: int;
  mods: int;
  ctrl: bool;
  was_selected: int;
  was_top: int;
begin
  key := stdlib.ui_key_code(ev.code);
  mods := stdlib.ui_key_modifiers(ev.code);
  ctrl := (mods div 2) mod 2 = 1;

  if d.mode = ModeSave then
  begin
    if d.field_focus then
      return handle_name_key(d, ev);
    if stdlib.ui_key_is_tab(ev.code) then
    begin
      d.field_focus := true;
      return true;
    end;
  end;

  if ctrl and stdlib.ui_key_is_up(ev.code) then
  begin
    if int(core.len(d.dir)) = 0 then
      return false;
    go_up(d);
    return true;
  end;
  was_selected := d.selected;
  was_top := d.top;
  if stdlib.ui_key_is_up(ev.code) then
  begin
    move_selection(d, -1);
    return d.selected <> was_selected or d.top <> was_top;
  end;
  if stdlib.ui_key_is_down(ev.code) then
  begin
    move_selection(d, 1);
    return d.selected <> was_selected or d.top <> was_top;
  end;
  if stdlib.ui_key_is_home(ev.code) then
  begin
    select_first(d);
    return d.selected <> was_selected or d.top <> was_top;
  end;
  if stdlib.ui_key_is_end(ev.code) then
  begin
    select_last(d);
    return d.selected <> was_selected or d.top <> was_top;
  end;
  if key = 0x116 then
  begin
    page_selection(d, -1);
    return d.selected <> was_selected or d.top <> was_top;
  end;
  if key = 0x117 then
  begin
    page_selection(d, 1);
    return d.selected <> was_selected or d.top <> was_top;
  end;
  if stdlib.ui_key_is_backspace(ev.code) then
  begin
    if int(core.len(d.dir)) = 0 then
      return false;
    go_up(d);
    return true;
  end;
  if stdlib.ui_key_is_enter(ev.code) then
  begin
    if d.mode = ModeSave then
      save_activate(d)
    else
      activate(d);
    return true;
  end;
  if key = 0x100 then
  begin
    cancel(d);
    return true;
  end;
  return false;
end;

// ---- public entry ---------------------------------------------------------

// Opens the modal file dialog. `chosen` is written only on `Ok`; a cancel is
// `Invalid` with `chosen` untouched. `initial` is the caller's current path:
// its directory starts the listing and its file name starts selected when it
// exists there. A disabled screen returns `Invalid` without opening a window,
// which is the `langhost` path.
function open_file(initial: const string[0]; chosen: var string[256]): Status;
var
  d: Dialog;
  screen_w: uint;
  screen_h: uint;
  x: int;
  y: int;
  st: Status;
  start_name: string[NameCapacity];
  probe_name: string[NameCapacity];
  probe_kind: uint;
  probe_size: uint;
  i: int;
  length: int;
  got: int;
  r: Rect;
  ev: Event;
begin
  if syslib.screen_width() = uint(0) then
    return Invalid;

  d.id := 0;
  d.dir := "";
  d.count := 0;
  d.selected := -1;
  d.top := 0;
  d.truncated := false;
  d.status := "";
  d.scroll_dragging := false;
  d.scroll_grab_dy := 0;
  d.scroll_preview := 0;
  d.last_click_ns := uint(0);
  d.last_click_item := -1;
  d.done := false;
  d.result := Invalid;
  d.picked := "";
  d.mode := ModeOpen;
  d.content_h := ContentH;
  d.status_top := StatusTop;
  d.button_top := ButtonTop;
  d.name := "";
  d.name_cursor := 0;
  d.name_scroll := 0;
  d.name_sel_start := 0;
  d.name_sel_stop := 0;
  d.field_focus := false;

  split_initial(initial, d.dir, start_name);
  if int(core.len(d.dir)) > 0 then
  begin
    st := core.fs_list(d.dir, uint(0), probe_name, probe_kind, probe_size);
    if st <> Ok and st <> NotFound then
      d.dir := "";
  end;
  load_dir(d);
  if int(core.len(start_name)) > 0 then
  begin
    i := 0;
    while i < d.count do
    begin
      if d.names[i] = start_name and is_selectable(d, i) then
      begin
        d.selected := i;
        i := d.count;
      end
      else
        i := i + 1;
    end;
    ensure_visible(d);
  end;

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  x := (int(screen_w) - (ContentW + 2)) div 2;
  y := (int(screen_h) - (ContentH + 17)) div 2;
  if x < 0 then
    x := 0;
  if y < 0 then
    y := 0;

  st := syslib.window_open(x, y, ContentW, ContentH, "Open File", d.id);
  if st <> Ok then
    return st;
  st := syslib.window_content_rect(d.id, r);
  if st <> Ok then
  begin
    st := syslib.window_close(d.id);
    return Invalid;
  end;
  d.rect := r;

  draw_dialog(d);
  st := syslib.window_present(d.id);
  while not d.done do
  begin
    st := syslib.next_event(ev);
    if st <> Ok then
    begin
      d.result := Invalid;
      d.done := true;
    end
    else if ev.kind = Expose then
    begin
      if ev.window = d.id then
      begin
        refresh_rect(d);
        draw_dialog(d);
        st := syslib.window_present(d.id);
      end;
    end
    else if ev.kind = Close then
    begin
      if ev.window = d.id then
        cancel(d);
    end
    else if ev.kind = KeyDown then
    begin
      if ev.window = d.id then
      begin
        if handle_key(d, ev) and not d.done then
        begin
          draw_dialog(d);
          st := syslib.window_present(d.id);
        end;
      end;
    end
    else if ev.kind = PointerDown or ev.kind = PointerUp or ev.kind = PointerMove then
    begin
      if ev.window = d.id then
      begin
        if handle_pointer(d, ev) and not d.done then
        begin
          draw_dialog(d);
          st := syslib.window_present(d.id);
        end;
      end;
    end;
  end;
  st := syslib.window_close(d.id);

  if d.result = Ok then
  begin
    length := int(core.len(d.picked));
    got := core.substr_into(chosen, d.picked, 0, length);
    if got <> length then
      return CapacityExceeded;
    if length = 0 then
      return NotFound;
    return Ok;
  end;
  return d.result;
end;

// Opens the modal **Save File** dialog. `initial` is the suggested target:
// its directory starts the listing (or `home` when it is not under the
// writable tree) and its name pre-fills the field with the whole name selected,
// so typing replaces it. `chosen` is written only on `Ok`; cancel is `Invalid`.
//
// The listing is the same one as `open_file`; single-click selects, double-click
// on a directory descends and on a file fills the name, `Up` walks to the
// parent. The name field edits with the usual keys, Tab moves between the field
// and the listing, Enter saves, Escape cancels. Save refuses a name that is
// empty or contains `/`, a target outside `home/`, an existing regular file and
// a directory; it never overwrites. A disabled screen returns `Invalid`.
function save_file(initial: const string[0]; chosen: var string[256]): Status;
var
  d: Dialog;
  screen_w: uint;
  screen_h: uint;
  x: int;
  y: int;
  st: Status;
  start_name: string[NameCapacity];
  probe_name: string[NameCapacity];
  probe_kind: uint;
  probe_size: uint;
  length: int;
  got: int;
  copied: int;
  r: Rect;
  ev: Event;
begin
  if syslib.screen_width() = uint(0) then
    return Invalid;

  d.id := 0;
  d.dir := "";
  d.count := 0;
  d.selected := -1;
  d.top := 0;
  d.truncated := false;
  d.status := "";
  d.scroll_dragging := false;
  d.scroll_grab_dy := 0;
  d.scroll_preview := 0;
  d.last_click_ns := uint(0);
  d.last_click_item := -1;
  d.done := false;
  d.result := Invalid;
  d.picked := "";
  d.mode := ModeSave;
  d.content_h := SaveContentH;
  d.status_top := SaveStatusTop;
  d.button_top := SaveButtonTop;
  d.name := "";
  d.name_cursor := 0;
  d.name_scroll := 0;
  d.name_sel_start := 0;
  d.name_sel_stop := 0;
  d.field_focus := true;

  split_initial(initial, d.dir, start_name);
  if not is_home_path(d.dir) then
    d.dir := "home";
  // A stale or read-only start directory falls back to `home`; an empty home
  // is still listable as `NotFound` and is a valid save target.
  st := core.fs_list(d.dir, uint(0), probe_name, probe_kind, probe_size);
  if st <> Ok and st <> NotFound then
  begin
    d.dir := "home";
    core.fs_list(d.dir, uint(0), probe_name, probe_kind, probe_size);
  end;
  load_dir(d);
  if int(core.len(start_name)) = 0 then
  begin
    copied := core.substr_into(d.name, "untitled.txt", 0, 12);
  end
  else
  begin
    length := int(core.len(start_name));
    copied := core.substr_into(d.name, start_name, 0, length);
  end;
  if copied < 0 then
    copied := core.substr_into(d.name, "untitled.txt", 0, 12);
  d.name_cursor := 0;
  d.name_cursor := stdlib.str_cursor_end(d.name, d.name_cursor);
  d.name_scroll := 0;
  d.name_sel_start := 0;
  d.name_sel_stop := d.name_cursor;

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  x := (int(screen_w) - (ContentW + 2)) div 2;
  y := (int(screen_h) - (SaveContentH + 17)) div 2;
  if x < 0 then
    x := 0;
  if y < 0 then
    y := 0;

  st := syslib.window_open(x, y, ContentW, SaveContentH, "Save File", d.id);
  if st <> Ok then
    return st;
  st := syslib.window_content_rect(d.id, r);
  if st <> Ok then
  begin
    st := syslib.window_close(d.id);
    return Invalid;
  end;
  d.rect := r;

  draw_dialog(d);
  st := syslib.window_present(d.id);
  while not d.done do
  begin
    st := syslib.next_event(ev);
    if st <> Ok then
    begin
      d.result := Invalid;
      d.done := true;
    end
    else if ev.kind = Expose then
    begin
      if ev.window = d.id then
      begin
        refresh_rect(d);
        draw_dialog(d);
        st := syslib.window_present(d.id);
      end;
    end
    else if ev.kind = Close then
    begin
      if ev.window = d.id then
        cancel(d);
    end
    else if ev.kind = KeyDown then
    begin
      if ev.window = d.id then
      begin
        if handle_key(d, ev) and not d.done then
        begin
          draw_dialog(d);
          st := syslib.window_present(d.id);
        end;
      end;
    end
    else if ev.kind = PointerDown or ev.kind = PointerUp or ev.kind = PointerMove then
    begin
      if ev.window = d.id then
      begin
        if handle_pointer(d, ev) and not d.done then
        begin
          draw_dialog(d);
          st := syslib.window_present(d.id);
        end;
      end;
    end;
  end;
  st := syslib.window_close(d.id);

  if d.result = Ok then
  begin
    length := int(core.len(d.picked));
    got := core.substr_into(chosen, d.picked, 0, length);
    if got <> length then
      return CapacityExceeded;
    if length = 0 then
      return NotFound;
    return Ok;
  end;
  return d.result;
end;

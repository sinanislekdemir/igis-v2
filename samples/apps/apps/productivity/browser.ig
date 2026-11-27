unit browser;

// M2.5.4, navigation M3.4, icon grid M18: a windowed file browser over the
// read-only application file API.
//
//   run browser         // interactive: navigate, open and launch
//   run browser check   // deterministic checks, prints `browser: ok`
//   run browser shot    // shows the icon grid, prints `browser: ready` and
//                       // holds ~5 s for the QMP screenshot harness
//
// The browser shows one directory at a time (spec 0013, "The file browser";
// spec 0028, "Browser integration"). `dir` is the current image path and `""`
// is the root; the path line shows `igis:/` or `igis:/<dir>`. The listing is
// `core.fs_list(dir, ...)` read from index 0 until `NotFound` into a 96-entry
// `lib.iconview` view plus a parallel size array. `.` and `..` never appear,
// the order is the image's directory order and no sorting is assumed. A
// directory with more than 96 entries reports a truncated listing rather than
// dropping silently.
//
// The body is a full-width 32x32 icon grid in the style of the old Windows
// Explorer: 100x68 cells (six columns and six visible rows at 640x420), a
// two-line 12-pixel label under each icon, the unchanged path line above and
// the status line below. The 16-pixel scrollbar sits at the right. There is no
// text preview pane: the browser never reads file contents for display, it
// only lists names, kinds and sizes. The four icons (`folder`, `igfile`,
// `file`, `image`) are loaded once at startup from the read-only `icons/`
// image directory, after the window is open, and reused for every frame and
// every directory. A failed load is non-fatal: `iconview.draw`/`icons.draw`
// paint the monochrome fallback and navigation keeps working. Icon ids are
// numeric module constants in `lib.icons` that are not exported across a unit,
// so the browser repeats the four fixed ids. The mapping is: kind 1 -> folder,
// kind 2 -> file; kind 0 ending in `.ig` (ASCII case-insensitive) or an
// extensionless file under `base`/`system`/`apps/<category>` (the stager drops
// the suffix there) -> source; kind 0 ending in
// `.png`/`.ppm`/`.jpg`/`.jpeg` -> image; anything else -> file.
//
// Enter and a double-click follow the default-open rules of spec 0029: a
// kind-1 entry is descended into (`dir := dir + "/" + name`, or the name at
// the root); a `.ppm`/`.png` file (ASCII case-insensitive) is opened in
// `apps/accessories/image_viewer`; a `.txt` file is opened in
// `apps/productivity/editor`; a source application (`.ig`, or an extensionless
// file in `base`/`system`/`apps`/`apps/<category>`) keeps the M18 blocking
// `core.spawn` + `core.wait` launch. The viewer, the editor and the terminal
// are spawned WITHOUT waiting, so the browser stays usable while they run. Any
// other regular file opens the context menu at the selected cell and spawns no
// child. A kind-2 entry (device, socket, symlink) is refused with a status line
// and is never entered, read or launched. Ctrl+L keeps the blocking launch
// meaning. Launching a `/lib` module is allowed and surfaces the compiler's
// `missing main` diagnostic, which is the compiler's classification, not a
// browser rule. Backspace and Ctrl+Up go up by truncating the last component
// of `dir`; at the root that is a no-op. The path line also carries a visible
// 44x18 `Up` button, so the parent directory is reachable with the pointer
// alone (M8.8). There is no `..` path and no history stack; path construction
// is bounded by PATH_MAX (256 bytes) and a combination that would not fit sets
// the status line and is not opened.
//
// A right-click (pointer button bit 1) on a regular file selects it and opens
// the context menu at the pointer; a right-click on a directory or a kind-2
// entry does not. The menu has exactly six 16-pixel rows in this order:
// `Open with editor`, `Run in terminal`, `Cut`, `Copy`, `Paste`, `Delete`. It
// is 220 pixels wide, framed, clamped inside the 680x480 content with a 4-pixel
// margin and drawn last by `stdlib.ui_menu_dropdown`. `Open with editor` spawns
// the editor with the full path and `Run in terminal` spawns
// `apps/utilities/terminal` with the single argument string `run <full path>`;
// neither waits. Up/Down move the highlight, Enter activates, Escape closes,
// a click inside activates or dismisses, and a click outside dismisses the menu
// and is consumed. A directory change closes it.
//
// The clipboard is browser-local and holds one entry (full path, kind and a
// copy/cut mode). It is not a system clipboard and lasts until it is replaced,
// until a successful cut-paste or until the browser exits. Paste targets the
// current directory under the source basename and only writes under `home/`:
// it never overwrites (an existing target is `already exists`), a copy
// streams 256-byte `core.fs_read` chunks into a new destination (removed on a
// failure), and a cut moves the source with `core.fs_rename`. `Delete` keeps
// the home-only remove rule (spec 0013).
//
// Keys: Left/Right move one entry, Up/Down one grid row (six entries),
// PageUp/PageDown one page (`columns x visible rows`), Home/End the first or
// last entry, Enter opens, Backspace/Ctrl+Up goes up, Ctrl+L launches, `N`
// creates `home/newdir`, Delete/`D` removes the selection under `home` and
// Ctrl+Q quits. A track click pages one screen at once. While the scrollbar
// thumb is held the view is deferred (M16.8): only a preview thumb
// (`scroll_preview`, a grid row) moves and the scrollbar strip alone is
// redrawn and recomposited, so `view.top` and the selection stay put; the
// release applies the dropped row once with `iconview.set_top`, clamps the
// selection into the new visible rows and repaints one frame.
//
// The status line shows a non-empty `status_line` (a failure or a completed
// home/launch operation) at the left. When it is empty it shows the selected
// entry's full name and its size or kind at the left; the short key hint stays
// at the right. Long names are never truncated in this status text beyond the
// 64-byte name capacity; the two-line grid label is the only wrapped form.
//
// Drawing is client content only: no `syslib.clear` and no assumption of a
// blank screen. Every frame is complete before one `window_present`, and an
// owner `Expose` repaints the whole content. `langhost` and a kernel without a
// framebuffer model a disabled screen, so the sample runs the same listing,
// navigation, geometry and icon-mapping logic over the same tree, loads no
// icons, draws nothing and never launches a child; it prints
// `browser: host ok`.

uses core, stdlib, syslib, lib.out, lib.inject, lib.icons, lib.iconview;

const
  WindowX = 160;
  WindowY = 90;
  ContentW = 680;
  ContentH = 480;

  // Listing capacity: 96 entries in any directory. There is no allocation, so
  // a larger directory reports the truncated listing.
  EntryCapacity = 96;

  // Path capacity: `abi::fs::PATH_MAX` (spec 0013 F2 / spec 0018 L6).
  PathCapacity = 256;

  // Path line: 34 monospace columns. A deeper directory shows `igis:/...`
  // followed by its deepest components, cut at a code-point boundary.
  MaxPathDirect = 28;
  MaxPathTail = 25;

  PathH = 22;

  // The icon grid: 640 usable pixels over 100-pixel cells is six columns; 420
  // over 68-pixel cells is six rows. The 16-pixel scrollbar sits at 656 with
  // an 8-pixel gap after the grid's right edge at 648.
  GridLeft = 8;
  GridTop = 30;
  GridW = 640;
  GridH = 420;
  CellW = 100;
  CellH = 68;
  CellMargin = 4;        // `lib.iconview`'s Margin (not exported)
  IconSide = 32;
  LabelGap = 4;
  LabelLineH = 12;
  LabelLines = 2;

  ScrollLeft = 656;
  ScrollW = 16;
  StatusTop = 460;

  // Context menu (spec 0029, M2/M3): six 16-pixel rows, 220 pixels wide, a
  // 1-pixel frame (98 pixels tall), clamped with a 4-pixel content margin.
  MenuCount = 6;
  MenuW = 220;
  MenuRowH = 16;
  MenuH = 98;
  MenuMargin = 4;
  MenuEditorRow = 0;
  MenuTerminalRow = 1;
  MenuCutRow = 2;
  MenuCopyRow = 3;
  MenuPasteRow = 4;
  MenuDeleteRow = 5;

  // Default-open actions (spec 0029, O1-O5).
  ActionEnter = 0;
  ActionRun = 1;
  ActionEditor = 2;
  ActionImage = 3;
  ActionMenu = 4;

  // The browser-local clipboard modes (spec 0029, C1).
  ClipNone = 0;
  ClipCopy = 1;
  ClipCut = 2;

  // `run <full path>` in one argument string: 4 + up to 255 bytes.
  TermCapacity = 264;

  // The `lib.icons` ids (module constants are not exported across a unit).
  FolderIcon = 0;
  SourceIcon = 1;
  FileIcon = 2;
  ImageIcon = 3;

  HoldSeconds = 5;
  NanosPerSecond = 1000000000;

var
  // Current directory ("" is the image root) and path construction.
  dir: string[PathCapacity];
  path_buf: string[PathCapacity];
  display_path: string[64];

  // The listing: `lib.iconview` owns names, kinds and icon ids; `sizes` is the
  // parallel inode size used by the status line and the proofs.
  view: iconview.View;
  sizes: array[uint, EntryCapacity];
  entry_name: string[64];
  entry_kind: uint;
  entry_size: uint;
  truncated: bool;

  // Scrollbar thumb drag (M8.9): grabbed at a pointer offset within the thumb.
  // M16.8 defers the view: the held thumb moves alone in `scroll_preview` and
  // `view.top` changes only when the button is released.
  scroll_dragging: bool;
  scroll_grab_dy: int;
  scroll_preview: int;

  status_line: string[64];
  status_text: string[128];

  // Window and absolute layout.
  id: int;
  rect: Rect;
  // Visible parent-directory button in the path line (M8.8).
  up_x: int;
  up_y: int;
  up_w: int;
  up_h: int;

  // Palette.
  black: uint;
  white: uint;
  gray: uint;
  accent: uint;

  // Icons, loaded once on the kernel path.
  icon_set: icons.Set;

  // Input and launch.
  ev: Event;
  last_click_ns: uint;
  last_click_item: int;
  opens: int;
  last_task: int;
  diag: Diagnostic;

  // Context menu (spec 0029, M1-M5). The labels and shortcuts are filled once
  // by `init_menu`; the browser owns the open state, the anchor and the
  // highlight.
  menu_items: array[string[64], 16];
  menu_shortcuts: array[string[32], 16];
  menu_open: bool;
  menu_x: int;
  menu_y: int;
  menu_selected: int;
  menu_target: int;

  // One browser-local clipboard entry (spec 0029, C1).
  clip_mode: int;
  clip_full: string[PathCapacity];
  clip_name: string[64];
  clip_kind: uint;

  // The `Run in terminal` argument buffer: `run <full path>` in one string.
  terminal_args: string[TermCapacity];
  terminal_stage: bytes[TermCapacity];

  // Mode and scratch.
  mode: int;
  running: bool;
  auto_draw: bool;
  dirty: bool;
  failed: bool;
  no_home: bool;
  last_op: Status;
  step: int;
  step_text: string[16];
  st: Status;
  used: int;
  value: Status;
  digits: string[24];
  size_buf: string[32];

// Silent check: the first failure is printed with its step number and label.
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed then
  begin
    failed := true;
    out.say("browser: failed check ");
    used := core.format_int_into(step_text, step, uint(10), uint(0));
    out.say(step_text);
    out.say(" (");
    out.say(label);
    out.say(")\n");
  end;
end;

// ---- paths ----------------------------------------------------------------

// Builds `base/name` (or just `name` when `base` is empty) into `out`.
// PATH_MAX is 256 bytes; anything longer is `CapacityExceeded` and writes
// nothing (spec 0013 F2).
function join_path(base: const string[0]; name: const string[0];
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
  name_len := int(core.len(name));
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
    stage[split + i] := name[i];
    i := i + 1;
  end;

  got := core.substr_into(out, stage, 0, total);
  if got = total then
    return Ok;
  if got = -6 then
    return CapacityExceeded;
  return Invalid;
end;

// The parent of the current directory: everything before the last `/`, or
// `""` at the root. `false` means the current directory is already the root
// (the caller treats it as a no-op).
function parent_of_current(out: var string[PathCapacity]): bool;
var
  length: int;
  at: int;
  got: int;
begin
  length := int(core.len(dir));
  if length = 0 then
    return false;
  at := length - 1;
  while at > 0 and dir[at] <> byte(47) do
    at := at - 1;
  if dir[at] = byte(47) then
    got := core.substr_into(out, dir, 0, at)
  else
    got := core.substr_into(out, dir, length, 0);
  return got >= 0;
end;

// ---- icon mapping (spec 0028 B4) ------------------------------------------

// ASCII lower-case of one byte; every other byte is unchanged.
function lower_byte(b: byte): byte;
begin
  if b >= byte(65) and b <= byte(90) then
    return byte(int(b) + 32);
  return b;
end;

// Whether `name` ends with `suffix`, ASCII case-insensitively.
function has_suffix(name: const string[0]; suffix: const string[0]): bool;
var
  name_len: int;
  suffix_len: int;
  i: int;
  a: byte;
  b: byte;
begin
  name_len := int(core.len(name));
  suffix_len := int(core.len(suffix));
  if suffix_len = 0 or name_len < suffix_len then
    return false;
  i := 0;
  while i < suffix_len do
  begin
    a := lower_byte(name[name_len - suffix_len + i]);
    b := lower_byte(suffix[i]);
    if a <> b then
      return false;
    i := i + 1;
  end;
  return true;
end;

// Whether the name has any dot, i.e. an extension separator.
function has_dot(name: const string[0]): bool;
var
  i: int;
begin
  i := 0;
  while i < int(core.len(name)) do
  begin
    if name[i] = byte(46) then
      return true;
    i := i + 1;
  end;
  return false;
end;

// The directories whose extensionless regular files are staged `.ig` source
// applications (spec 0018 L3): `base`, `system` and `apps/<category>`.
function is_source_dir(where: const string[0]): bool;
begin
  if where = "base" or where = "system" or where = "apps" then
    return true;
  return stdlib.starts_with(where, "apps/");
end;

// The icon id for one listing entry. A directory is a folder; any kind-2 entry
// is a plain file. A regular file is a source when it ends in `.ig`, or when it
// has no extension and lives in a source-application directory (the stager
// drops `.ig` there); an image by its suffix; otherwise a plain file.
function icon_for(name: const string[0]; kind: uint; where: const string[0]): int;
begin
  if kind = uint(1) then
    return FolderIcon;
  if kind <> uint(0) then
    return FileIcon;
  if has_suffix(name, ".ig") then
    return SourceIcon;
  if not has_dot(name) and is_source_dir(where) then
    return SourceIcon;
  if has_suffix(name, ".png") or has_suffix(name, ".ppm")
     or has_suffix(name, ".jpg") or has_suffix(name, ".jpeg") then
    return ImageIcon;
  return FileIcon;
end;

// The default-open action (spec 0029, O1-O5). A directory is entered; a
// regular file is handed to the one application that knows it, or to the
// blocking source launcher; everything else opens the context menu. A kind-2
// entry is never dispatched here (`open_entry` refuses it first).
function default_action(name: const string[0]; kind: uint;
                        where: const string[0]): int;
begin
  if kind = uint(1) then
    return ActionEnter;
  if kind <> uint(0) then
    return ActionMenu;
  if has_suffix(name, ".ppm") or has_suffix(name, ".png") then
    return ActionImage;
  if has_suffix(name, ".txt") then
    return ActionEditor;
  if has_suffix(name, ".ig") then
    return ActionRun;
  if not has_dot(name) and is_source_dir(where) then
    return ActionRun;
  return ActionMenu;
end;

// ---- listing --------------------------------------------------------------

// Reads the current directory in directory order until `NotFound`. A new
// directory starts with the selection, view and status reset. The icons are
// not touched: the loaded set is reused for every directory (spec 0028 B1).
procedure load_dir();
var
  i: int;
  at: int;
  cursor: uint;
begin
  iconview.clear(view);
  truncated := false;
  scroll_dragging := false;
  scroll_preview := 0;
  // A directory change closes the context menu (spec 0029, M3).
  menu_open := false;
  i := 0;
  while i < EntryCapacity do
  begin
    sizes[i] := uint(0);
    i := i + 1;
  end;
  cursor := uint(0);
  st := core.fs_list(dir, cursor, entry_name, entry_kind, entry_size);
  while st = Ok and view.count < EntryCapacity do
  begin
    at := iconview.add(view, entry_name, entry_kind, icon_for(entry_name, entry_kind, dir));
    if at >= 0 then
      sizes[at] := entry_size;
    cursor := cursor + uint(1);
    st := core.fs_list(dir, cursor, entry_name, entry_kind, entry_size);
  end;
  if view.count >= EntryCapacity and st = Ok then
  begin
    truncated := true;
    status_line := "listing truncated (96)"
  end
  else if st <> NotFound then
    status_line := "listing failed"
  else
    status_line := "";
end;

// Descends into `name`: the new path is committed only when it fits and is
// listable, so a failure leaves the current view untouched. An empty
// directory lists zero entries (`NotFound` at index 0 is a valid listing).
function enter_dir(name: const string[0]): Status;
var
  candidate: string[PathCapacity];
  probe_name: string[64];
  probe_kind: uint;
  probe_size: uint;
  err: Status;
begin
  err := join_path(dir, name, candidate);
  if err <> Ok then
  begin
    status_line := "path too long";
    return err;
  end;
  st := core.fs_list(candidate, uint(0), probe_name, probe_kind, probe_size);
  if st <> Ok and st <> NotFound then
  begin
    if st = Invalid then
      status_line := "unsupported path"
    else
      status_line := "listing failed";
    return st;
  end;
  dir := candidate;
  load_dir();
  return Ok;
end;

// Goes up one component. At the root the call changes nothing.
procedure go_up();
var
  parent: string[PathCapacity];
begin
  if parent_of_current(parent) then
  begin
    dir := parent;
    load_dir();
  end;
end;

function find_name(target: const string[0]): int;
var
  i: int;
begin
  i := 0;
  while i < view.count do
  begin
    if view.names[i] = target then
      return i;
    i := i + 1;
  end;
  return -1;
end;

// The first non-file row in the view, or -1 when every row is a regular file.
function find_first_dir(): int;
var
  i: int;
begin
  i := 0;
  while i < view.count do
  begin
    if view.kinds[i] <> uint(0) then
      return i;
    i := i + 1;
  end;
  return -1;
end;

// ---- opening and launch ---------------------------------------------------

// Blocking launcher for a full image path: spawn the source app, then wait for
// it to exit. `Ok` means `ExitReason.Ok`; a killed or faulted child is
// `IoError`. The check mode uses this for the editor, whose real path is
// `apps/productivity/editor` rather than a name under the current directory.
function launch_path(path: const string[0]; args: const string[0]): Status;
var
  err: Status;
  child: int;
  why: ExitReason;
begin
  child := 0;
  err := core.spawn(path, args, diag, child);
  if err <> Ok then
  begin
    status_line := "launch failed";
    return err;
  end;
  last_task := child;
  err := core.wait(child, why);
  if err <> Ok then
  begin
    status_line := "launch wait failed";
    return err;
  end;

  err := Ok;
  case why of
    Ok: status_line := "launched: ok"
    Killed:
    begin
      status_line := "launch killed";
      err := IoError;
    end;
    Faulted(code):
    begin
      status_line := "launch faulted";
      err := IoError;
    end;
  end;
  return err;
end;

// Blocking launcher for a name in the current directory: `dir/name`, or just
// `name` at the root. On a directory Ctrl+L descends instead; a kind-2 entry is
// refused before the spawn.
function launch_name(name: const string[0]; args: const string[0];
                     kind: uint): Status;
var
  err: Status;
begin
  if kind = uint(1) then
    return enter_dir(name);
  if kind <> uint(0) then
  begin
    status_line := "cannot launch this entry";
    return Invalid;
  end;

  err := join_path(dir, name, path_buf);
  if err <> Ok then
  begin
    status_line := "path too long";
    return err;
  end;
  return launch_path(path_buf, args);
end;

// Non-blocking launch (spec 0029, O2/O3/M4): spawn and return without a
// `core.wait`, so the browser stays usable while the viewer, the editor or the
// terminal runs. The blocking `launch_path` remains for source applications.
function spawn_detached(path: const string[0]; args: const string[0]): Status;
var
  err: Status;
  child: int;
begin
  child := 0;
  err := core.spawn(path, args, diag, child);
  if err <> Ok then
  begin
    status_line := "launch failed";
    return err;
  end;
  last_task := child;
  status_line := "launched: ok";
  return Ok;
end;

// Builds the selected entry's full image path into `path_buf`. A bad selection
// or an over-long path reports the existing status and launches nothing (O7).
function selected_path(): Status;
var
  err: Status;
begin
  if view.selected < 0 or view.selected >= view.count then
  begin
    status_line := "nothing selected";
    return Invalid;
  end;
  err := join_path(dir, view.names[view.selected], path_buf);
  if err <> Ok then
    status_line := "path too long";
  return err;
end;

// `Open with editor` (M4): the editor receives the full image path, without a
// wait.
procedure open_editor_selected();
begin
  if selected_path() <> Ok then
    return;
  value := spawn_detached("apps/productivity/editor", path_buf);
end;

// `Run in terminal` (M4, T2): the terminal receives one argument string,
// `run <full path>`, and forwards it to its shell; the browser does not wait.
procedure run_in_terminal_selected();
var
  total: int;
  length: int;
  i: int;
  got: int;
begin
  if selected_path() <> Ok then
    return;
  length := int(core.len(path_buf));
  total := 4 + length;
  if total > TermCapacity then
  begin
    status_line := "path too long";
    return;
  end;
  terminal_stage[0] := byte(114);  // r
  terminal_stage[1] := byte(117);  // u
  terminal_stage[2] := byte(110);  // n
  terminal_stage[3] := byte(32);   // space
  i := 0;
  while i < length do
  begin
    terminal_stage[4 + i] := path_buf[i];
    i := i + 1;
  end;
  got := core.substr_into(terminal_args, terminal_stage, 0, total);
  if got <> total then
  begin
    status_line := "path too long";
    return;
  end;
  value := spawn_detached("apps/utilities/terminal", terminal_args);
end;

// ---- context menu (spec 0029, M1-M3) --------------------------------------

// The disabled mask for the menu: `Paste` is gray and inert while the
// browser-local clipboard is empty.
function menu_disabled_mask(): uint;
begin
  if clip_mode = ClipNone then
    return stdlib.ui_menu_bit(MenuPasteRow);
  return uint(0);
end;

// Opens the menu at a window-local point, clamped into the 680x480 content
// with a 4-pixel margin (M2). The top-left anchor is clamped, so the frame can
// never leave the canvas.
procedure open_menu(at: int; px: int; py: int);
var
  hit: int;
begin
  menu_target := at;
  menu_x := px;
  menu_y := py;
  if menu_x < MenuMargin then
    menu_x := MenuMargin;
  if menu_x > ContentW - MenuMargin - MenuW then
    menu_x := ContentW - MenuMargin - MenuW;
  if menu_y < MenuMargin then
    menu_y := MenuMargin;
  if menu_y > ContentH - MenuMargin - MenuH then
    menu_y := ContentH - MenuMargin - MenuH;
  menu_open := true;
  hit := stdlib.ui_menu_hit(
    menu_x, menu_y, MenuW, menu_items, MenuCount, px, py);
  if hit < 0 or not stdlib.ui_menu_selectable(
       menu_items[hit], menu_disabled_mask(), hit) then
    hit := stdlib.ui_menu_first(MenuCount, menu_items, menu_disabled_mask());
  menu_selected := hit;
end;

// The context menu anchored to the selected cell (O5): the whole cell.
procedure open_selected_menu();
var
  row: int;
  col: int;
  at: int;
begin
  at := view.selected;
  if at < 0 or at >= view.count then
    return;
  if view.kinds[at] <> uint(0) then
    return;
  row := (at div view.cols) - view.top;
  col := at mod view.cols;
  open_menu(
    at,
    GridLeft + CellMargin + col * CellW + 8,
    GridTop + CellMargin + row * CellH + 8);
end;

// Dispatches one listing entry by the default-open rule: a directory is
// entered, a known regular file is spawned without waiting, a source
// application is launched with the blocking `core.wait` and every other
// regular file opens the context menu at its cell. There is no content read
// and no preview. A kind-2 entry is refused before any call.
function open_entry(name: const string[0]; kind: uint): Status;
var
  action: int;
  err: Status;
begin
  if kind = uint(1) then
    return enter_dir(name);
  if kind <> uint(0) then
  begin
    status_line := "not a regular file";
    return Invalid;
  end;

  action := default_action(name, kind, dir);
  if action = ActionMenu then
  begin
    open_selected_menu();
    return Ok;
  end;

  err := join_path(dir, name, path_buf);
  if err <> Ok then
  begin
    status_line := "path too long";
    return err;
  end;
  if action = ActionImage then
    return spawn_detached("apps/accessories/image_viewer", path_buf);
  if action = ActionEditor then
    return spawn_detached("apps/productivity/editor", path_buf);
  return launch_path(path_buf, "");
end;

procedure launch_selected();
begin
  if view.selected >= 0 and view.selected < view.count then
    value := launch_name(
      view.names[view.selected], "", view.kinds[view.selected]);
end;

procedure activate_selected();
begin
  if view.selected >= 0 and view.selected < view.count then
  begin
    value := open_entry(view.names[view.selected], view.kinds[view.selected]);
    // A menu is an offer, not an open: only a real launch increments `opens`.
    if value = Ok and not menu_open then
      opens := opens + 1;
  end;
end;

// ---- home operations (spec 0019, E2) --------------------------------------

function under_home(): bool;
begin
  return dir = "home" or stdlib.starts_with(dir, "home/");
end;

// `N`: create `newdir` in the current home directory, or `home/newdir` from
// anywhere else. A duplicate name and a missing home mount both report a
// status; the check path probes writability separately so the messages differ.
procedure create_directory();
var
  target: string[PathCapacity];
  err: Status;
begin
  last_op := Ok;
  if under_home() then
    err := join_path(dir, "newdir", target)
  else
    err := join_path("", "home/newdir", target);
  if err <> Ok then
  begin
    last_op := err;
    status_line := "path too long";
    return;
  end;
  err := core.fs_mkdir(target);
  last_op := err;
  if err = Ok then
  begin
    load_dir();
    status_line := "created newdir";
  end
  else if err = Invalid then
  begin
    if no_home then
      status_line := "no writable home"
    else
      status_line := "already exists or read-only";
  end
  else if err = NotFound then
    status_line := "no such directory"
  else if err = CapacityExceeded then
    status_line := "no space left"
  else
    status_line := "create failed";
end;

// `Delete` or `D`: remove the selected regular file or empty directory, only
// when it lies under `home`. The listing refreshes and keeps the row position.
procedure remove_selected();
var
  full: string[PathCapacity];
  err: Status;
  keep: int;
begin
  last_op := Ok;
  if view.selected < 0 or view.selected >= view.count then
  begin
    status_line := "nothing selected";
    return;
  end;
  err := join_path(dir, view.names[view.selected], full);
  if err <> Ok then
  begin
    last_op := err;
    status_line := "path too long";
    return;
  end;
  if not stdlib.starts_with(full, "home/") then
  begin
    last_op := Invalid;
    status_line := "read-only: outside home";
    return;
  end;
  keep := view.selected;
  err := core.fs_remove(full);
  last_op := err;
  if err = Ok then
  begin
    load_dir();
    if keep >= view.count then
      keep := view.count - 1;
    if keep < 0 then
      keep := 0;
    iconview.move_to(view, keep);
    status_line := "removed";
  end
  else if err = Invalid then
    status_line := "not removable"
  else if err = NotFound then
    status_line := "no such file"
  else if err = CapacityExceeded then
    status_line := "no space left"
  else
    status_line := "remove failed";
end;

// ---- browser-local clipboard (spec 0029, C1-C5) ---------------------------

// `Copy` / `Cut` (C2): store the selected regular file and set the status.
// Nothing is written to the filesystem and no child is spawned.
procedure store_clipboard(want: int);
var
  err: Status;
  length: int;
  copied: int;
begin
  if view.selected < 0 or view.selected >= view.count then
  begin
    status_line := "nothing selected";
    return;
  end;
  err := join_path(dir, view.names[view.selected], clip_full);
  if err <> Ok then
  begin
    status_line := "path too long";
    return;
  end;
  length := int(core.len(view.names[view.selected]));
  copied := core.substr_into(clip_name, view.names[view.selected], 0, length);
  if copied <> length then
  begin
    clip_mode := ClipNone;
    status_line := "path too long";
    return;
  end;
  clip_kind := view.kinds[view.selected];
  clip_mode := want;
  if want = ClipCut then
    status_line := "cut"
  else
    status_line := "copied";
end;

// Streams `src[0..total)` into the new `dst` in 256-byte chunks (C4). The
// destination is created first (a one-byte write, then truncate to zero, so a
// zero-byte source still produces a file) and a partial destination is removed
// on any failure. The source may be read-only; only `dst` must be under home.
function copy_file(src: const string[0]; dst: const string[0];
                   total: uint): Status;
var
  chunk: bytes[256];
  one: bytes[1];
  offset: uint;
  got: int;
  written: int;
  err: Status;
begin
  one[0] := byte(0);
  written := -1;
  err := core.fs_write(dst, uint(0), one, written);
  if err <> Ok or written <> 1 then
  begin
    err := core.fs_remove(dst);
    return IoError;
  end;
  err := core.fs_truncate(dst, uint(0));
  if err <> Ok then
  begin
    err := core.fs_remove(dst);
    return IoError;
  end;
  offset := uint(0);
  while offset < total do
  begin
    got := -1;
    err := core.fs_read(src, offset, chunk, got);
    if err <> Ok or got <= 0 then
    begin
      err := core.fs_remove(dst);
      return IoError;
    end;
    written := -1;
    err := core.fs_write(dst, offset, chunk, written);
    if err <> Ok or written <> got then
    begin
      err := core.fs_remove(dst);
      return IoError;
    end;
    offset := offset + uint(got);
  end;
  return Ok;
end;

// `Paste` (C3-C5): the target is the current directory plus the source
// basename. It refuses an empty clipboard, the source itself, an existing
// target (never overwrite) and a destination outside `home/`. A cut moves with
// `core.fs_rename`; a copy streams. Success reloads the directory, selects the
// pasted name and reports `pasted`; a cut clears the clipboard, a copy keeps it.
procedure paste_clipboard();
var
  target: string[PathCapacity];
  total: uint;
  at: int;
  err: Status;
  was_cut: bool;
begin
  last_op := Ok;
  if clip_mode = ClipNone then
  begin
    status_line := "clipboard empty";
    return;
  end;
  err := join_path(dir, clip_name, target);
  if err <> Ok then
  begin
    last_op := err;
    status_line := "path too long";
    return;
  end;
  if target = clip_full then
  begin
    last_op := Invalid;
    status_line := "same directory";
    return;
  end;
  total := uint(0);
  err := core.fs_size(target, total);
  // A regular file is `Ok`; a directory or another non-regular entry is
  // `Invalid`. Both are an existing target and must never be removed or
  // overwritten, so only `NotFound` proceeds.
  if err = Ok or err = Invalid then
  begin
    last_op := Invalid;
    status_line := "already exists";
    return;
  end;
  if err <> NotFound then
  begin
    last_op := err;
    status_line := "paste failed";
    return;
  end;
  if not under_home() then
  begin
    last_op := Invalid;
    status_line := "read-only: outside home";
    return;
  end;

  was_cut := clip_mode = ClipCut;
  if was_cut then
  begin
    err := core.fs_rename(clip_full, target);
    last_op := err;
    if err <> Ok then
    begin
      status_line := "paste failed";
      return;
    end;
  end
  else
  begin
    err := core.fs_size(clip_full, total);
    if err <> Ok then
    begin
      last_op := err;
      status_line := "copy failed";
      return;
    end;
    err := copy_file(clip_full, target, total);
    last_op := err;
    if err <> Ok then
    begin
      status_line := "copy failed";
      return;
    end;
  end;

  load_dir();
  at := find_name(clip_name);
  if at >= 0 then
    iconview.move_to(view, at);
  status_line := "pasted";
  if was_cut then
    clip_mode := ClipNone;
end;

// The six menu labels (M2), filled once. The shortcuts array belongs to the
// library's headless menu model and stays empty: the browser binds no menu
// accelerators beyond the documented keys.
procedure init_menu();
var
  i: int;
begin
  menu_items[0] := "Open with editor";
  menu_items[1] := "Run in terminal";
  menu_items[2] := "Cut";
  menu_items[3] := "Copy";
  menu_items[4] := "Paste";
  menu_items[5] := "Delete";
  i := 0;
  while i < 16 do
  begin
    menu_shortcuts[i] := "";
    i := i + 1;
  end;
  menu_open := false;
  menu_x := 0;
  menu_y := 0;
  menu_selected := 0;
  menu_target := -1;
end;

// Activates one menu row (M3/M4). A disabled row is a no-op and the menu stays
// open; every other activation closes it first, so an action that reloads the
// directory (paste, delete) cannot leave stale menu pixels behind.
procedure activate_menu_row(row: int);
begin
  if row < 0 or row >= MenuCount then
    return;
  if not stdlib.ui_menu_selectable(
       menu_items[row], menu_disabled_mask(), row) then
    return;
  menu_open := false;
  menu_target := view.selected;
  if row = MenuEditorRow then
    open_editor_selected()
  else if row = MenuTerminalRow then
    run_in_terminal_selected()
  else if row = MenuCutRow then
    store_clipboard(ClipCut)
  else if row = MenuCopyRow then
    store_clipboard(ClipCopy)
  else if row = MenuPasteRow then
    paste_clipboard()
  else if row = MenuDeleteRow then
    remove_selected();
end;

// ---- scrollbar (over grid rows) -------------------------------------------

// The thumb height and travel for `total` rows against the visible rows,
// matching `stdlib.ui_scrollbar` (M8.9): the track is `GridH - 4` tall, the
// thumb the visible fraction with an 8-pixel floor, travelling over
// `track - thumb`.
procedure thumb_metrics(total: int; thumb_h: var int; travel: var int);
var
  track_h: int;
begin
  track_h := GridH - 4;
  if total <= 0 then
    thumb_h := track_h
  else
  begin
    if view.rows <= 0 then
      thumb_h := 8
    else
      thumb_h := (track_h * view.rows) div total;
    if thumb_h < 8 then
      thumb_h := 8;
    if thumb_h > track_h then
      thumb_h := track_h;
  end;
  travel := track_h - thumb_h;
end;

// The current scrollbar thumb geometry for `total` rows. The press, the drag
// preview and the check-mode proof use the same arithmetic.
procedure scrollbar_thumb(total: int; thumb_y: var int; thumb_h: var int);
var
  max: int;
  pos: int;
  travel: int;
begin
  thumb_metrics(total, thumb_h, travel);
  max := total - view.rows;
  if max < 0 then
    max := 0;
  pos := stdlib.ui_scrollbar_clamp(view.top, view.rows, total);
  thumb_y := GridTop + 2;
  if max > 0 and travel > 0 then
    thumb_y := GridTop + 2 + (travel * pos) div max;
end;

// Ends a thumb drag and applies the preview to the view (M16.8): the grid
// jumps to the dropped row once, on release, and the selection is kept inside
// the new visible row window. Returns whether anything visible changed.
function end_scroll_drag(): bool;
var
  first_at: int;
  last_at: int;
begin
  scroll_dragging := false;
  if scroll_preview = view.top then
    return false;
  iconview.set_top(view, scroll_preview);
  first_at := view.top * view.cols;
  last_at := (view.top + view.rows) * view.cols - 1;
  if last_at >= view.count then
    last_at := view.count - 1;
  if view.selected < first_at then
    view.selected := first_at;
  if view.selected > last_at then
    view.selected := last_at;
  return true;
end;

// ---- key and pointer handling ---------------------------------------------

procedure handle_key();
var
  key: int;
  mods: int;
  ctrl: bool;
  action: int;
begin
  key := stdlib.ui_key_code(ev.code);
  mods := stdlib.ui_key_modifiers(ev.code);
  ctrl := (mods div 2) mod 2 = 1;

  // The context menu owns the documented menu keys (M3): Up/Down move the
  // highlight, Enter activates, Escape closes. Other keys are ignored while it
  // is open so they cannot move the grid underneath.
  if menu_open then
  begin
    action := stdlib.ui_menu_key(ev.code, true);
    if action = 6 then
      menu_open := false
    else if action = 1 then
      menu_selected := stdlib.ui_menu_move(
        menu_selected, MenuCount, -1, menu_items, menu_disabled_mask())
    else if action = 2 then
      menu_selected := stdlib.ui_menu_move(
        menu_selected, MenuCount, 1, menu_items, menu_disabled_mask())
    else if action = 5 then
      activate_menu_row(menu_selected);
    return;
  end;

  if ctrl then
  begin
    if key = 0x6C or key = 0x4C then
    begin
      launch_selected();
      return;
    end;
    if key = 0x71 or key = 0x51 then
    begin
      running := false;
      return;
    end;
    if stdlib.ui_key_is_up(ev.code) then
    begin
      go_up();
      return;
    end;
  end;

  if stdlib.ui_key_is_left(ev.code) then
    iconview.move(view, -1, 0)
  else if stdlib.ui_key_is_right(ev.code) then
    iconview.move(view, 1, 0)
  else if stdlib.ui_key_is_up(ev.code) then
    iconview.move(view, 0, -1)
  else if stdlib.ui_key_is_down(ev.code) then
    iconview.move(view, 0, 1)
  else if stdlib.ui_key_is_delete(ev.code) then
    remove_selected()
  else if key = 0x4E or key = 0x6E then
    create_directory()
  else if key = 0x44 or key = 0x64 then
    remove_selected()
  else if stdlib.ui_key_is_backspace(ev.code) then
    go_up()
  else if key = 0x116 then
    iconview.page(view, -1)
  else if key = 0x117 then
    iconview.page(view, 1)
  else if stdlib.ui_key_is_home(ev.code) then
    iconview.first(view)
  else if stdlib.ui_key_is_end(ev.code) then
    iconview.last(view)
  else if stdlib.ui_key_is_enter(ev.code) then
    activate_selected();
end;

// One pointer event addressed to the browser. Returns whether a whole frame is
// needed (M16.8): a held thumb drag redraws only the scrollbar strip, and the
// grid view changes once, on release.
function handle_pointer(): bool;
var
  hit: int;
  action: int;
  now: uint;
  thumb_y: int;
  thumb_h: int;
  travel: int;
  desired: int;
  total: int;
  max_top: int;
  preview: int;
  was_top: int;
  was_selected: int;
  right: bool;
begin
  total := iconview.total_rows(view);

  // A grabbed scrollbar thumb follows the pointer while the primary button is
  // held (M8.9); the move maps the thumb position onto the row travel. M16.8
  // defers the view: release (or a move without the button) applies the
  // dropped row.
  if scroll_dragging then
  begin
    if ev.kind = PointerUp then
      return end_scroll_drag();
    if ev.kind = PointerMove then
    begin
      if ev.code mod uint(2) <> uint(1) then
        return end_scroll_drag();
      thumb_metrics(total, thumb_h, travel);
      max_top := total - view.rows;
      if max_top < 0 then
        max_top := 0;
      if travel > 0 and max_top > 0 then
      begin
        desired := ev.y - scroll_grab_dy;
        if desired < GridTop + 2 then
          desired := GridTop + 2;
        if desired > GridTop + 2 + travel then
          desired := GridTop + 2 + travel;
        preview := ((desired - (GridTop + 2)) * max_top) div travel;
        if preview < 0 then
          preview := 0;
        if preview > max_top then
          preview := max_top;
        if preview <> scroll_preview then
        begin
          scroll_preview := preview;
          preview_scrollbar();
        end;
      end;
      return false;
    end;
    return false;
  end;

  // The context menu consumes the pointer while it is open (M3). A move
  // follows the pointer, a press inside activates or dismisses it, and a press
  // outside dismisses it and is consumed by the browser.
  if menu_open then
  begin
    if ev.kind = PointerMove then
    begin
      hit := stdlib.ui_menu_hit(
        menu_x, menu_y, MenuW, menu_items, MenuCount, ev.x, ev.y);
      if hit >= 0 and stdlib.ui_menu_selectable(
           menu_items[hit], menu_disabled_mask(), hit) then
      begin
        if hit <> menu_selected then
        begin
          menu_selected := hit;
          return true;
        end;
      end;
      return false;
    end;
    if ev.kind <> PointerDown then
      return false;
    if stdlib.ui_rect_hit(menu_x, menu_y, MenuW, MenuH, ev.x, ev.y) then
    begin
      hit := stdlib.ui_menu_hit(
        menu_x, menu_y, MenuW, menu_items, MenuCount, ev.x, ev.y);
      if hit >= 0 then
        activate_menu_row(hit)
      else
        menu_open := false;
      return true;
    end;
    menu_open := false;
    return true;
  end;

  if ev.kind <> PointerDown then
    return false;

  // A right-click (button bit 1) on a regular file selects it and opens the
  // menu at the pointer (M1). A directory or a kind-2 entry only selects.
  right := (ev.code div uint(2)) mod uint(2) = uint(1);
  if right then
  begin
    hit := iconview.index_at(view, ev.x, ev.y);
    if hit < 0 then
      return false;
    iconview.move_to(view, hit);
    last_click_ns := uint(0);
    last_click_item := -1;
    if view.kinds[hit] = uint(0) then
      open_menu(hit, ev.x, ev.y);
    return true;
  end;

  if stdlib.ui_button_hit(up_x, up_y, up_w, up_h, ev.x, ev.y) then
  begin
    go_up();
    return true;
  end;

  if stdlib.ui_rect_hit(ScrollLeft, GridTop, ScrollW, GridH, ev.x, ev.y) then
  begin
    action := stdlib.ui_scrollbar_hit(
      ScrollLeft, GridTop, ScrollW, GridH, ev.x, ev.y,
      view.top, view.rows, total);
    if action = 3 then
    begin
      scrollbar_thumb(total, thumb_y, thumb_h);
      scroll_dragging := true;
      scroll_preview := view.top;
      scroll_grab_dy := ev.y - thumb_y;
      return false;
    end;
    was_top := view.top;
    was_selected := view.selected;
    if action = 1 then
      iconview.page(view, -1)
    else if action = 2 then
      iconview.page(view, 1);
    return view.top <> was_top or view.selected <> was_selected;
  end;

  hit := iconview.index_at(view, ev.x, ev.y);
  if hit < 0 then
    return false;
  iconview.move_to(view, hit);
  now := core.clock();
  if stdlib.ui_double_click(last_click_ns, now, last_click_item = hit)
     and view.kinds[hit] <> uint(2) then
  begin
    last_click_ns := uint(0);
    last_click_item := -1;
    value := open_entry(view.names[hit], view.kinds[hit]);
    if value = Ok and not menu_open then
      opens := opens + 1;
  end
  else
  begin
    last_click_ns := now;
    last_click_item := hit;
    // A single click on a kind-2 entry still explains why it cannot open.
    if view.kinds[hit] = uint(2) then
      value := open_entry(view.names[hit], view.kinds[hit]);
  end;
  return true;
end;

// ---- drawing --------------------------------------------------------------

procedure compute_layout();
begin
  // A small `Up` button at the right of the path line, left of the writability
  // label. It is the visible parent-directory affordance (M8.8); the Backspace
  // and Ctrl+Up keys keep working.
  up_w := 44;
  up_h := 18;
  up_x := ContentW - 8 - stdlib.ui_text_width("home writable")
    - 8 - up_w;
  up_y := 2;
end;

// The view rectangle is fixed in window-local coordinates (M17); only the
// `Up` button position depends on the palette text width.
procedure setup_view();
begin
  compute_layout();
  iconview.init(view, GridLeft, GridTop, GridW, GridH, CellW, CellH);
end;

// Builds the path line: `igis:/` plus the current directory. A directory too
// wide for the line shows `igis:/...` and its deepest components, cut at a
// code-point boundary.
procedure build_display_path();
var
  length: int;
  start: int;
  total: int;
  i: int;
  got: int;
  stage: bytes[64];
begin
  length := int(core.len(dir));
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
      stage[6 + i] := dir[i];
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
    while start < length and not stdlib.str_boundary(dir, start) do
      start := start + 1;
    i := start;
    while i < length do
    begin
      stage[9 + (i - start)] := dir[i];
      i := i + 1;
    end;
    total := 9 + (length - start);
  end;
  got := core.substr_into(display_path, stage, 0, total);
  if got <> total then
    display_path := "";
end;

procedure draw_path();
var
  width: int;
  path_width: int;
  count_x: int;
begin
  build_display_path();
  st := syslib.draw_text(8, 4, display_path, black);
  path_width := stdlib.ui_text_width(display_path);
  count_x := 8 + path_width + 16;
  if count_x < 96 then
    count_x := 96;
  used := core.format_int_into(digits, view.count, uint(10), uint(0));
  st := syslib.draw_text(count_x, 4, digits, black);
  st := syslib.draw_text(count_x + used * 8, 4, " entries", gray);
  width := stdlib.ui_text_width("home writable");
  st := syslib.draw_text(
    ContentW - 8 - width, 4, "home writable", gray);
  st := stdlib.ui_button(up_x, up_y, up_w, up_h, "Up", false, false);
  st := stdlib.ui_separator(0, PathH - 1, ContentW);
end;

// The status line. A non-empty `status_line` wins; otherwise the selected
// entry's full name and its size or kind are shown at the left (spec 0028 B6).
// The short key hint is always drawn at the right.
procedure draw_status();
var
  width: int;
  left_width: int;
  at: int;
  hint: string[64];
begin
  hint := "Enter open  Ctrl+L run  Bksp up  N new  Del del";
  st := stdlib.ui_separator(0, StatusTop - 6, ContentW);
  left_width := 0;

  if int(core.len(status_line)) > 0 then
  begin
    st := syslib.draw_text(8, StatusTop, status_line, black);
    left_width := stdlib.ui_text_width(status_line);
  end
  else if view.selected >= 0 and view.selected < view.count then
  begin
    at := view.selected;
    if view.kinds[at] = uint(1) then
      status_text := view.names[at] + "  dir"
    else if view.kinds[at] = uint(2) then
      status_text := view.names[at] + "  other"
    else
    begin
      used := core.format_int_into(
        size_buf, int(sizes[at]), uint(10), uint(0));
      status_text := view.names[at] + "  " + size_buf + " B";
    end;
    st := syslib.draw_text(8, StatusTop, status_text, black);
    left_width := stdlib.ui_text_width(status_text);
  end;

  // The hint keeps its right-aligned position but is omitted when the left
  // text would overlap it; a long full name is never truncated (spec 0028 B6).
  width := stdlib.ui_text_width(hint);
  if 8 + left_width + 16 <= ContentW - 8 - width then
    st := syslib.draw_text(ContentW - 8 - width, StatusTop, hint, gray);
end;

// The scrollbar strip at `position` (a grid row) over the visible rows.
procedure draw_scrollbar_at(position: int);
begin
  st := stdlib.ui_scrollbar(
    ScrollLeft, GridTop, ScrollW, GridH,
    position, view.rows, iconview.total_rows(view), false);
end;

// The scrollbar strip alone after a drag preview moved the thumb (M16.8): the
// canvas keeps the grid rows, and `syslib.present` recomposites just this
// rectangle from the surface stack.
procedure preview_scrollbar();
begin
  st := syslib.window_begin(id);
  draw_scrollbar_at(scroll_preview);
  // `present` is a screen rectangle (spec 0026 C7), so it is the one call that
  // still adds the content origin.
  st := syslib.present(rect.x + ScrollLeft, rect.y + GridTop, ScrollW, GridH);
end;

// One complete frame; the caller presents once afterwards. The icon view draws
// the white grid, border, icons and two-line labels inside the open window
// scope; while a thumb is held the strip shows the preview and the grid stays
// at `view.top` (M16.8).
procedure draw_all();
begin
  st := syslib.fill_rect(0, 0, ContentW, ContentH, white);
  draw_path();
  st := iconview.draw(view, icon_set);
  if scroll_dragging then
    draw_scrollbar_at(scroll_preview)
  else
    draw_scrollbar_at(view.top);
  draw_status();
  // The context menu is drawn last, over the grid and status line but clipped
  // to the content canvas (M2).
  if menu_open then
    st := stdlib.ui_menu_dropdown(
      menu_x, menu_y, MenuW, menu_items, menu_shortcuts, MenuCount,
      menu_selected, uint(0), menu_disabled_mask(), true);
end;

procedure present_frame();
begin
  st := syslib.window_present(id);
end;

// Re-reads the content rectangle. Drawing and hit-testing are window-local
// (spec 0026 C8), so only `syslib.present`'s screen rectangle needs the origin
// (C7); the layout itself never changes with a move.
procedure refresh_rect();
begin
  st := syslib.window_content_rect(id, rect);
  if st = Ok then
    compute_layout();
end;

// ---- event pump -----------------------------------------------------------

// Drains every queued event; drawing is deferred to one frame at the end. In
// check mode `auto_draw` is off during injected sequences so the double-click
// deadline never includes a full repaint.
procedure drain_events();
var
  active: bool;
  count: int;
begin
  active := true;
  count := 0;
  while active and count < 64 do
  begin
    st := syslib.poll_event(ev);
    if st = Ok then
    begin
      count := count + 1;
      if ev.kind = Expose then
      begin
        refresh_rect();
        dirty := true;
      end
      else if ev.window = id then
      begin
        if ev.kind = KeyDown then
        begin
          handle_key();
          dirty := true;
        end
        else if ev.kind = PointerDown or ev.kind = PointerUp
                or ev.kind = PointerMove then
        begin
          if handle_pointer() then
            dirty := true;
        end;
      end;
    end
    else
      active := false;
  end;
  if dirty and auto_draw then
  begin
    draw_all();
    present_frame();
  end;
  dirty := false;
end;

// ---- injection (the deterministic test hook) ------------------------------

procedure pointer_to(px: int; py: int);
begin
  // `px`/`py` are window-local; the pointer state is screen-coordinate
  // (spec 0010 I35), so the content origin is added here.
  st := inject.pointer_to(rect.x + px, rect.y + py);
  drain_events();
end;

procedure pointer_down();
begin
  st := inject.pointer_down();
  drain_events();
end;

procedure pointer_up();
begin
  st := inject.pointer_up();
  drain_events();
end;

// A right-click at the current pointer: button bit 1 (`uint(2)`), then the
// release, each drained. The pointer must already be placed with `pointer_to`.
procedure pointer_right();
begin
  st := syslib.inject_pointer(0, 0, uint(2));
  drain_events();
  st := syslib.inject_pointer(0, 0, uint(0));
  drain_events();
end;

// Moves with the primary button held, for the scrollbar-thumb drag proof
// (M8.9). `inject.pointer_to` would release the button, so the delta is sent
// directly through the debug pointer hook.
procedure pointer_drag_to(px: int; py: int);
begin
  // `px`/`py` are window-local; the pointer state is screen-coordinate, so the
  // content origin is added before the delta is computed (spec 0010 I35).
  st := syslib.inject_pointer(
    rect.x + px - syslib.pointer_x(), rect.y + py - syslib.pointer_y(), uint(1));
  drain_events();
end;

procedure inject_key_mods(key_code: int; text_byte: int; mods: int);
begin
  st := inject.inject_key_mods(key_code, text_byte, mods);
  drain_events();
end;

procedure inject_key(key_code: int; text_byte: int);
begin
  st := inject.inject_key(key_code, text_byte);
  drain_events();
end;

// ---- pixel probes ---------------------------------------------------------

// Whether a `w x h` window-local rectangle contains `color`.
function region_has(x: int; y: int; w: int; h: int; color: uint): bool;
var
  dx: int;
  dy: int;
begin
  dy := 0;
  while dy < h do
  begin
    dx := 0;
    while dx < w do
    begin
      if syslib.get_pixel(x + dx, y + dy) = color then
        return true;
      dx := dx + 1;
    end;
    dy := dy + 1;
  end;
  return false;
end;

// Whether a `w x h` window-local rectangle contains a pixel other than
// `color`. Used for icon ink over the white grid.
function region_has_other(x: int; y: int; w: int; h: int; color: uint): bool;
var
  dx: int;
  dy: int;
begin
  dy := 0;
  while dy < h do
  begin
    dx := 0;
    while dx < w do
    begin
      if syslib.get_pixel(x + dx, y + dy) <> color then
        return true;
      dx := dx + 1;
    end;
    dy := dy + 1;
  end;
  return false;
end;

// ---- shared checks --------------------------------------------------------

function has_kind(target: const string[0]; want: uint): bool;
var
  at: int;
begin
  at := find_name(target);
  if at < 0 then
    return false;
  return view.kinds[at] = want;
end;

// The pure icon mapping, checked with synthetic names (spec 0028 B4).
procedure mapping_checks();
begin
  verify("map folder", icon_for("base", uint(1), "") = FolderIcon);
  verify("map source", icon_for("child.ig", uint(0), "base") = SourceIcon);
  verify("map source upper", icon_for("CHILD.IG", uint(0), "lib") = SourceIcon);
  verify("map app source", icon_for("shell", uint(0), "base") = SourceIcon);
  verify("map category source",
    icon_for("editor", uint(0), "apps/productivity") = SourceIcon);
  verify("map system source", icon_for("entrypoint", uint(0), "system") = SourceIcon);
  verify("map home plain", icon_for("readme", uint(0), "home") = FileIcon);
  verify("map png", icon_for("photo.png", uint(0), "image") = ImageIcon);
  verify("map ppm upper", icon_for("PHOTO.PPM", uint(0), "image") = ImageIcon);
  verify("map jpg", icon_for("photo.jpg", uint(0), "image") = ImageIcon);
  verify("map jpeg", icon_for("photo.jpeg", uint(0), "image") = ImageIcon);
  verify("map plain", icon_for("readme.txt", uint(0), "home") = FileIcon);
  verify("map other", icon_for("device", uint(2), "home") = FileIcon);
end;

// The pure default-open mapping (spec 0029, O1-O7 / V1): directory, source,
// image, text and the context-menu fallback, including uppercase extensions.
procedure action_checks();
begin
  verify("action dir", default_action("base", uint(1), "") = ActionEnter);
  verify("action source app", default_action("child", uint(0), "base") = ActionRun);
  verify("action ig", default_action("math.ig", uint(0), "lib") = ActionRun);
  verify("action ig upper", default_action("MATH.IG", uint(0), "lib") = ActionRun);
  verify("action category source",
    default_action("editor", uint(0), "apps/productivity") = ActionRun);
  verify("action system source",
    default_action("entrypoint", uint(0), "system") = ActionRun);
  verify("action ppm", default_action("alps.ppm", uint(0), "image") = ActionImage);
  verify("action png", default_action("photo.png", uint(0), "image") = ActionImage);
  verify("action ppm upper", default_action("ALPS.PPM", uint(0), "image") = ActionImage);
  verify("action png upper", default_action("PHOTO.PNG", uint(0), "image") = ActionImage);
  verify("action txt", default_action("readme.txt", uint(0), "home") = ActionEditor);
  verify("action txt upper", default_action("README.TXT", uint(0), "home") = ActionEditor);
  verify("action other", default_action("notes.md", uint(0), "home") = ActionMenu);
  verify("action jpg is other", default_action("photo.jpg", uint(0), "image") = ActionMenu);
  verify("action kind two", default_action("device", uint(2), "home") = ActionMenu);
end;

// The menu model and its pure geometry/hit test (spec 0029, M2 / V1). No window
// is opened and nothing is drawn.
procedure menu_model_checks();
begin
  verify("menu rows", MenuCount = 6);
  verify("menu width", MenuW = 220);
  verify("menu row height", MenuRowH = 16);
  verify("menu height", MenuH = 98);
  verify("menu item editor", menu_items[MenuEditorRow] = "Open with editor");
  verify("menu item terminal", menu_items[MenuTerminalRow] = "Run in terminal");
  verify("menu item cut", menu_items[MenuCutRow] = "Cut");
  verify("menu item copy", menu_items[MenuCopyRow] = "Copy");
  verify("menu item paste", menu_items[MenuPasteRow] = "Paste");
  verify("menu item delete", menu_items[MenuDeleteRow] = "Delete");
  verify("menu hit first",
    stdlib.ui_menu_hit(0, 0, MenuW, menu_items, MenuCount, 10, 9) = 0);
  verify("menu hit last",
    stdlib.ui_menu_hit(
      0, 0, MenuW, menu_items, MenuCount, 10,
      1 + MenuDeleteRow * MenuRowH + 8) = MenuDeleteRow);
  verify("menu hit border",
    stdlib.ui_menu_hit(0, 0, MenuW, menu_items, MenuCount, 10, 0) = -1);
  verify("menu hit outside",
    stdlib.ui_menu_hit(0, 0, MenuW, menu_items, MenuCount, MenuW + 1, 9) = -1);
end;

// The grid geometry the browser initialized (spec 0028 B3).
procedure grid_checks();
begin
  verify("grid columns", iconview.columns(view) = 6);
  verify("visible rows", iconview.visible_rows(view) = 6);
  verify("page size", iconview.page_size(view) = 36);
  verify("grid origin", view.x = GridLeft and view.y = GridTop);
  verify("grid size", view.w = GridW and view.h = GridH);
  verify("cell size", view.cell_w = CellW and view.cell_h = CellH);
end;

// The listing and the documented error classes. Used by both the host path and
// the kernel proof. Leaves the view in `base`.
procedure data_checks(expect_lost_found: bool);
var
  child_at: int;
  text_at: int;
  at: int;
  err: Status;
begin
  dir := "";
  load_dir();
  verify("root listed", view.count >= 6);
  verify("root complete", not truncated);
  verify("root base", has_kind("base", uint(1)));
  verify("root apps", has_kind("apps", uint(1)));
  verify("root home", has_kind("home", uint(1)));
  verify("root lib", has_kind("lib", uint(1)));
  verify("root system", has_kind("system", uint(1)));
  verify("root image", has_kind("image", uint(1)));
  verify("base icon", view.icon_ids[find_name("base")] = FolderIcon);
  if expect_lost_found then
  begin
    verify("root icons", has_kind("icons", uint(1)));
    verify("root lost+found", has_kind("lost+found", uint(1)));
    verify("root entry count", view.count = 8);
  end
  else
    verify("root entry count", view.count = 6);

  err := enter_dir("base");
  verify("enter base", err = Ok and dir = "base");
  verify("listing non-empty", view.count > 0);
  verify("listing complete", not truncated);
  verify("listing capacity", view.count <= EntryCapacity);
  verify("listing size", view.count >= 50);
  verify("shell listed", has_kind("shell", uint(0)));
  verify("child listed", has_kind("child", uint(0)));
  verify("text listed", has_kind("text", uint(0)));
  verify("base is flat", find_first_dir() < 0);

  child_at := find_name("child");
  verify("child found", child_at >= 0);
  if child_at >= 0 then
  begin
    // The size comes from the listing; no content is read for display.
    verify("child size", sizes[child_at] > uint(0));
    verify("child icon", view.icon_ids[child_at] = SourceIcon);
    iconview.move_to(view, child_at);
    verify("child selected", view.selected = child_at);
  end;
  text_at := find_name("text");
  verify("text found", text_at >= 0);
  if text_at >= 0 then
    verify("text icon", view.icon_ids[text_at] = SourceIcon);

  // A kind-2 entry is shown with the generic file icon but not openable.
  value := open_entry("child", uint(2));
  verify("kind two refused",
    value = Invalid and status_line = "not a regular file");
  verify("kind two keeps view", find_name("child") >= 0);

  // A malformed name is refused by the file API before anything is listed or
  // read; the current view is untouched.
  value := enter_dir(".");
  verify("malformed path",
    value = Invalid and status_line = "unsupported path");
  verify("malformed keeps dir", dir = "base");

  // The old text preview refused a file over 2048 bytes; the icon grid lists a
  // large source with its size and never reads it (spec 0028 B2).
  go_up();
  err := enter_dir("apps");
  verify("enter apps", err = Ok and dir = "apps");
  at := find_name("accessories");
  verify("accessories listed", at >= 0 and view.kinds[at] = uint(1));
  err := enter_dir("accessories");
  verify("enter accessories", err = Ok and dir = "apps/accessories");
  at := find_name("widgets");
  verify("widgets listed", at >= 0 and view.kinds[at] = uint(0));
  verify("no preview cap",
    at >= 0 and sizes[at] > uint(2048) and status_line = "");
  go_up();
  go_up();
  verify("back at root", int(core.len(dir)) = 0);
  err := enter_dir("base");
  verify("back in base", err = Ok and dir = "base");
end;

// Navigation (M3.4): the root, descend, back up with the parent rule, and the
// sibling directories. Used by both paths; the host path sets `report` so its
// deterministic transcript names the stages it proved.
procedure navigation_checks(expect_lost_found: bool; report: bool);
var
  err: Status;
  at: int;
begin
  // `data_checks` leaves the view in `base`.
  if report then
    out.say("browser: nav dir=base\n");
  verify("base current", dir = "base" and find_name("child") >= 0);

  go_up();
  verify("back up to root", int(core.len(dir)) = 0);
  verify("up lists root", has_kind("base", uint(1)) and has_kind("lib", uint(1)));
  if expect_lost_found then
  begin
    verify("up lists icons", has_kind("icons", uint(1)));
    verify("up lists lost+found", has_kind("lost+found", uint(1)));
  end;
  if report then
    out.say("browser: nav dir=/\n");
  go_up();
  verify("root up no-op", int(core.len(dir)) = 0 and has_kind("base", uint(1)));

  err := enter_dir("system");
  verify("enter system", err = Ok and dir = "system");
  verify("system_probe listed", has_kind("system_probe", uint(0)));
  go_up();
  verify("back from system", int(core.len(dir)) = 0);

  err := enter_dir("home");
  verify("enter home", err = Ok and dir = "home");
  verify("readme listed", has_kind("readme.txt", uint(0)));
  go_up();
  verify("back from home", int(core.len(dir)) = 0);

  err := enter_dir("lib");
  verify("enter lib", err = Ok and dir = "lib");
  at := find_name("math.ig");
  verify("math.ig listed", at >= 0 and view.kinds[at] = uint(0));
  if at >= 0 then
    verify("math icon", view.icon_ids[at] = SourceIcon);
  if report then
    out.say("browser: nav lib=math.ig\n");
  go_up();
  verify("back from lib", int(core.len(dir)) = 0);

  err := enter_dir("base");
  verify("back into base", err = Ok and dir = "base"
    and has_kind("child", uint(0)));
end;

// Path construction is bounded by PATH_MAX: a 255-byte directory plus a name
// cannot be joined, so nothing is opened and the status line says why.
procedure long_path_check();
var
  saved: string[PathCapacity];
  stage: bytes[PathCapacity];
  i: int;
  got: int;
begin
  saved := dir;
  i := 0;
  while i < PathCapacity - 1 do
  begin
    stage[i] := byte(97);   // 'a'
    i := i + 1;
  end;
  got := core.substr_into(dir, stage, 0, PathCapacity - 1);
  verify("long path built", got = PathCapacity - 1);
  value := enter_dir("x");
  verify("long path refused",
    value = CapacityExceeded and status_line = "path too long");
  verify("long path keeps dir", int(core.len(dir)) = PathCapacity - 1);
  dir := saved;
  load_dir();
  verify("long path restored", dir = "base" and has_kind("child", uint(0)));
end;

// Movement, clamping, paging and the row scrollbar share the iconview model.
procedure selection_checks();
var
  keep_count: int;
begin
  keep_count := view.count;
  iconview.first(view);
  verify("first entry", view.selected = 0 and view.top = 0);
  iconview.move(view, 1, 0);
  verify("right one", view.selected = 1 and view.top = 0);
  iconview.move(view, -1, 0);
  verify("left one", view.selected = 0 and view.top = 0);
  iconview.move(view, 1, 0);
  verify("right again", view.selected = 1);

  iconview.first(view);
  iconview.page(view, -1);
  verify("page up clamps", view.selected = 0 and view.top = 0);
  iconview.page(view, 1);
  verify("page down", view.selected = 36 and view.top = 1);
  iconview.page(view, -1);
  verify("page up", view.selected = 0 and view.top = 0);

  iconview.move(view, 0, 1);
  verify("down row", view.selected = 6 and view.top = 0);
  iconview.move(view, 0, -1);
  verify("up row", view.selected = 0 and view.top = 0);

  iconview.last(view);
  verify("end reaches last", view.selected = keep_count - 1);
  verify("end scrolls last row",
    view.top = iconview.total_rows(view) - view.rows);
  iconview.first(view);
  verify("home back to first", view.selected = 0 and view.top = 0);
  verify("move clamps",
    not iconview.move(view, 0, 100) or view.selected = keep_count - 1);
end;

// Home operations (spec 0019, E2): create `home/newdir` from the root, list it,
// then remove it from inside `home`; a document outside `home` is refused. The
// probe remove is non-destructive: `NotFound` means a writable mount exists and
// `Invalid` means there is none, without creating anything.
procedure writable_checks(via_keys: bool);
var
  err: Status;
  at: int;
begin
  last_op := core.fs_remove("home/igis_probe_none");
  no_home := last_op = Invalid;
  // Clean up a leftover from an interrupted run before creating.
  err := core.fs_remove("home/newdir");
  dir := "";
  load_dir();
  if via_keys then
  begin
    auto_draw := false;
    inject_key(0x4E, 0);
    auto_draw := true;
  end
  else
    create_directory();

  if no_home then
  begin
    verify("create no home", last_op = Invalid and status_line = "no writable home");
    verify("no home keeps root", int(core.len(dir)) = 0);
  end
  else
  begin
    verify("create ok", last_op = Ok and status_line = "created newdir");
    err := enter_dir("home");
    verify("enter home after create", err = Ok and dir = "home");
    at := find_name("newdir");
    verify("created listed", at >= 0 and view.kinds[at] = uint(1));
    if at >= 0 then
    begin
      iconview.move_to(view, at);
      if via_keys then
      begin
        auto_draw := false;
        inject_key(0x44, 0);
        auto_draw := true;
      end
      else
        remove_selected();
      verify("remove ok", last_op = Ok and status_line = "removed");
      verify("removed gone", find_name("newdir") < 0);
    end;
    // A path outside home is refused before the filesystem is touched.
    value := core.fs_remove("base/child");
    verify("outside home refused", value = Invalid);
    go_up();
    verify("writable back at root", int(core.len(dir)) = 0);
  end;

  // Leave the view where the drawing and launch checks expect it.
  err := enter_dir("base");
  verify("writable back in base", err = Ok and dir = "base");
end;

// The right-click menu and the browser-local clipboard (spec 0029, M1-M5,
// C1-C5 / V1). The clipboard source is `base/child`; the paste target is
// `home/child`. The persistent home disk is cleaned before the proof so an
// interrupted earlier run cannot make this one fail.
procedure clipboard_checks();
var
  child_at: int;
  readme_at: int;
  at: int;
  row: int;
  col: int;
  cell_x: int;
  cell_y: int;
  menu_row_y: int;
  source_size: uint;
  check_size: uint;
  err: Status;
  first_a: bytes[256];
  first_b: bytes[256];
  got_a: int;
  got_b: int;
  same: bool;
  i: int;
begin
  clip_mode := ClipNone;
  verify("paste disabled empty",
    stdlib.ui_menu_has(menu_disabled_mask(), MenuPasteRow));

  // In `base`: right-click `child` and take Copy from the menu. This half only
  // needs the window and the clipboard, so it runs even without a home disk.
  // `writable_checks` has already left the view in `base`; entering `base`
  // again would build `base/base`.
  if dir <> "base" then
    err := enter_dir("base")
  else
    err := Ok;
  verify("clip in base", err = Ok and dir = "base");
  if not no_home then
  begin
    // Idempotent cleanup after an interrupted run.
    err := core.fs_remove("home/child");
    verify("clip cleanup", err = Ok or err = NotFound);
  end;

  child_at := find_name("child");
  verify("clip source found", child_at >= 0 and view.kinds[child_at] = uint(0));
  if child_at < 0 then
    return;
  source_size := sizes[child_at];
  iconview.move_to(view, child_at);
  row := (child_at div view.cols) - view.top;
  col := child_at mod view.cols;
  cell_x := GridLeft + CellMargin + col * CellW;
  cell_y := GridTop + CellMargin + row * CellH;

  auto_draw := false;
  pointer_to(cell_x + 12, cell_y + 12);
  pointer_right();
  auto_draw := true;
  verify("right-click opens menu", menu_open and menu_target = child_at);
  draw_all();
  present_frame();
  verify("menu frame pixel", syslib.get_pixel(menu_x, menu_y) = black);
  verify("menu text ink", region_has(
    menu_x + 22, menu_y + 1 + MenuRowH, 180, 5 * MenuRowH, black));

  menu_row_y := menu_y + 1 + MenuCopyRow * MenuRowH + MenuRowH div 2;
  auto_draw := false;
  pointer_to(menu_x + 40, menu_row_y);
  pointer_down();
  pointer_up();
  auto_draw := true;
  verify("copy from menu",
    not menu_open and clip_mode = ClipCopy and status_line = "copied");
  verify("copy enables paste",
    not stdlib.ui_menu_has(menu_disabled_mask(), MenuPasteRow));

  if no_home then
  begin
    // `writable_checks` already reported the missing home; the menu and the
    // browser-local clipboard still ran above, the list/paste half cannot.
    out.say("browser: clipboard paste skipped (no home)\n");
    return;
  end;

  // Go up to the root and enter `home` (a directory change closes any menu).
  go_up();
  err := enter_dir("home");
  verify("clip in home", err = Ok and dir = "home");
  err := core.fs_remove("home/child");
  verify("clip target clean", err = Ok or err = NotFound);

  readme_at := find_name("readme.txt");
  verify("clip target row", readme_at >= 0 and view.kinds[readme_at] = uint(0));
  if readme_at < 0 then
    return;
  iconview.move_to(view, readme_at);
  row := (readme_at div view.cols) - view.top;
  col := readme_at mod view.cols;
  cell_x := GridLeft + CellMargin + col * CellW;
  cell_y := GridTop + CellMargin + row * CellH;
  auto_draw := false;
  pointer_to(cell_x + 12, cell_y + 12);
  pointer_right();
  auto_draw := true;
  verify("paste menu opens", menu_open and menu_target = readme_at);

  menu_row_y := menu_y + 1 + MenuPasteRow * MenuRowH + MenuRowH div 2;
  auto_draw := false;
  pointer_to(menu_x + 40, menu_row_y);
  pointer_down();
  pointer_up();
  auto_draw := true;
  verify("paste status", not menu_open and status_line = "pasted");
  at := find_name("child");
  verify("pasted selected", at >= 0 and view.selected = at);
  verify("pasted size", at >= 0 and sizes[at] = source_size);

  got_a := -1;
  got_b := -1;
  err := core.fs_read("base/child", uint(0), first_a, got_a);
  err := core.fs_read("home/child", uint(0), first_b, got_b);
  same := got_a > 0 and got_a = got_b;
  i := 0;
  while same and i < got_a do
  begin
    if first_a[i] <> first_b[i] then
      same := false;
    i := i + 1;
  end;
  verify("pasted bytes", same);

  // Select the copy, right-click it and delete it from the menu.
  at := find_name("child");
  verify("copy row found", at >= 0);
  if at >= 0 then
  begin
    iconview.move_to(view, at);
    row := (at div view.cols) - view.top;
    col := at mod view.cols;
    cell_x := GridLeft + CellMargin + col * CellW;
    cell_y := GridTop + CellMargin + row * CellH;
    auto_draw := false;
    pointer_to(cell_x + 12, cell_y + 12);
    pointer_right();
    auto_draw := true;
    verify("delete menu opens", menu_open and menu_target = at);
    menu_row_y := menu_y + 1 + MenuDeleteRow * MenuRowH + MenuRowH div 2;
    auto_draw := false;
    pointer_to(menu_x + 40, menu_row_y);
    pointer_down();
    pointer_up();
    auto_draw := true;
    verify("delete from menu",
      status_line = "removed" and find_name("child") < 0);
    check_size := uint(0);
    verify("copy gone",
      core.fs_size("home/child", check_size) = NotFound);
  end;

  go_up();
  err := enter_dir("base");
  verify("clip back in base", err = Ok and dir = "base");
end;

// ---- modes ---------------------------------------------------------------

procedure host_path();
begin
  failed := false;
  step := 0;
  setup_view();
  mapping_checks();
  action_checks();
  menu_model_checks();
  grid_checks();
  data_checks(false);
  navigation_checks(false, true);
  long_path_check();
  selection_checks();
  writable_checks(false);
  verify("host no window", syslib.screen_width() = uint(0));
  if failed then
    out.say("browser: host failed\n")
  else
    out.say("browser: host ok\n");
end;

// The kernel proof. Listing, navigation and selection checks first, then one
// composed frame with pixel probes, then real navigation and terminating
// launches.
procedure check_mode();
var
  child_at: int;
  text_at: int;
  before_opens: int;
  row: int;
  col: int;
  cell_x: int;
  cell_y: int;
  icon_x: int;
  icon_y: int;
  band_y: int;
  hint_x: int;
  total: int;
  thumb_h: int;
  thumb_y: int;
  max_top: int;
  home_at: int;
  err: Status;
begin
  verify("icon folder", icons.ready(icon_set, FolderIcon));
  verify("icon source", icons.ready(icon_set, SourceIcon));
  verify("icon file", icons.ready(icon_set, FileIcon));
  verify("icon image", icons.ready(icon_set, ImageIcon));

  mapping_checks();
  action_checks();
  menu_model_checks();
  data_checks(true);
  grid_checks();
  navigation_checks(true, false);
  long_path_check();
  selection_checks();
  writable_checks(true);
  clipboard_checks();

  // Double-click: two real clicks on `child`'s visible cell launch it (the
  // blocking launcher returns after the child exits).
  child_at := find_name("child");
  text_at := find_name("text");
  before_opens := opens;
  iconview.move_to(view, child_at);
  auto_draw := false;
  last_click_ns := uint(0);
  last_click_item := -1;
  row := (child_at div view.cols) - view.top;
  col := child_at mod view.cols;
  cell_x := GridLeft + CellMargin + col * CellW;
  cell_y := GridTop + CellMargin + row * CellH;
  pointer_to(cell_x + 12, cell_y + 12);
  pointer_down();
  pointer_up();
  pointer_down();
  pointer_up();
  auto_draw := true;
  verify("double click launches",
    opens = before_opens + 1 and status_line = "launched: ok");

  // Enter on the (already selected) `text` row launches it by full path.
  iconview.move_to(view, text_at);
  auto_draw := false;
  inject_key(0x103, 0);
  auto_draw := true;
  verify("enter launches", opens = before_opens + 2
    and status_line = "launched: ok");

  // Build a frame whose selected cell is `text`; clear the operation status so
  // the status bar shows the full-name fallback.
  iconview.move_to(view, text_at);
  status_line := "";
  draw_all();
  present_frame();

  row := (text_at div view.cols) - view.top;
  col := text_at mod view.cols;
  cell_x := GridLeft + CellMargin + col * CellW;
  cell_y := GridTop + CellMargin + row * CellH;
  icon_x := cell_x + (CellW - IconSide) div 2;
  icon_y := cell_y + CellMargin;
  band_y := icon_y + IconSide + LabelGap;
  hint_x := ContentW - 8 - stdlib.ui_text_width(
    "Enter open  Ctrl+L run  Bksp up  N new  Del del");
  verify("path rule", syslib.get_pixel(300, PathH - 1) = black);
  verify("grid border", syslib.get_pixel(GridLeft, GridTop) = black);
  verify("grid background", syslib.get_pixel(GridLeft + 2, GridTop + 2) = white);
  verify("selection band", syslib.get_pixel(cell_x + 1, band_y + 1) = accent);
  verify("label ink (white)", region_has(
    cell_x + 1, band_y, CellW - 2, LabelLines * LabelLineH, white));
  verify("icon ink", region_has_other(
    icon_x, icon_y, IconSide, IconSide, white));
  verify("status rule", syslib.get_pixel(300, StatusTop - 6) = black);
  verify("status name ink", region_has(8, StatusTop, 240, 8, black));
  verify("hint ink", syslib.get_pixel(hint_x + 1, StatusTop + 1) = gray);

  // The scrollbar is over grid rows. The thumb sits at `view.top` after the
  // frame above, so probe the computed thumb.
  total := iconview.total_rows(view);
  scrollbar_thumb(total, thumb_y, thumb_h);
  verify("scrollbar border", syslib.get_pixel(ScrollLeft, GridTop) = black);
  verify("scrollbar thumb", syslib.get_pixel(ScrollLeft + 4, thumb_y + 2) = gray);
  verify("thumb metrics", thumb_h >= 8 and thumb_h <= GridH - 4);

  // A track click pages one screen through the real pointer path.
  iconview.first(view);
  draw_all();
  pointer_to(ScrollLeft + 8, GridTop + GridH - 6);
  pointer_down();
  pointer_up();
  verify("track click pages", view.selected = 36 and view.top = 1);
  iconview.first(view);
  draw_all();

  // Grab the thumb and drag it to the bottom (M8.9). M16.8 defers the view:
  // the held drag moves the thumb strip only, and release applies the dropped
  // row and clamps the selection into the visible window.
  scrollbar_thumb(total, thumb_y, thumb_h);
  pointer_to(ScrollLeft + 8, thumb_y + thumb_h div 2);
  pointer_down();
  pointer_drag_to(ScrollLeft + 8, GridTop + GridH - 4);
  max_top := total - view.rows;
  verify("scrollbar drag preview", view.top = 0 and scroll_preview = max_top);
  verify("preview thumb leaves the top",
    syslib.get_pixel(ScrollLeft + 4, GridTop + 4) = white);
  verify("preview thumb at the bottom",
    syslib.get_pixel(ScrollLeft + 4, GridTop + GridH - 6) = gray);
  pointer_up();
  verify("scrollbar drag applies", not scroll_dragging and view.top = max_top);
  verify("drag clamps selection", view.selected = view.top * view.cols);
  iconview.first(view);

  // The visible `Up` button goes to the parent through the real pointer path
  // (M8.8); the Backspace and Ctrl+Up keys are checked next.
  draw_all();
  verify("up button drawn", syslib.get_pixel(up_x, up_y) = black);
  pointer_to(up_x + up_w div 2, up_y + up_h div 2);
  pointer_down();
  pointer_up();
  verify("up button returns to root",
    int(core.len(dir)) = 0 and has_kind("base", uint(1)));
  err := enter_dir("base");
  verify("back in base for keys", err = Ok and dir = "base");

  // Backspace goes up from `base` to the root through the real key path.
  auto_draw := false;
  inject_key(0x101, 0);
  auto_draw := true;
  verify("backspace up", int(core.len(dir)) = 0
    and has_kind("base", uint(1)));

  // A directory selected with Ctrl+L descends instead of launching. The key is
  // injected with the Ctrl modifier and takes the normal event path.
  home_at := find_name("home");
  verify("home row found", home_at >= 0);
  iconview.move_to(view, home_at);
  auto_draw := false;
  inject_key_mods(0x6C, 0, 2);
  auto_draw := true;
  verify("ctrl-l descends", dir = "home" and has_kind("readme.txt", uint(0)));

  // Ctrl+Up is the second way up; it returns to the root.
  auto_draw := false;
  inject_key_mods(0x112, 0, 2);
  auto_draw := true;
  verify("ctrl-up up", int(core.len(dir)) = 0);

  // A kind-2 entry cannot open or launch.
  value := open_entry("child", uint(2));
  verify("open kind two refused",
    value = Invalid and status_line = "not a regular file");
  value := launch_name("child", "", uint(2));
  verify("launch kind two refused",
    value = Invalid and status_line = "cannot launch this entry");

  // Back in `base`, `editor check` is a real terminating source app, so the
  // blocking launcher leaves no child behind. The full path is
  // `apps/productivity/editor`.
  err := enter_dir("base");
  verify("launch in base", err = Ok and dir = "base");
  value := launch_path("apps/productivity/editor", "check");
  verify("launch completed",
    value = Ok and last_task > 0 and int(core.len(diag.message)) = 0);
  verify("launch status", status_line = "launched: ok");

  // Launching a /lib module by its full path is allowed and ends as a compile
  // fault (a module declares no `main`); the kernel reports the compiler's
  // diagnostic, and the browser sees `Faulted(IoError)`.
  go_up();
  err := enter_dir("lib");
  verify("launch lib", err = Ok and has_kind("math.ig", uint(0)));
  value := launch_name("math.ig", "", uint(0));
  verify("module launch faults",
    value = IoError and status_line = "launch faulted");
  go_up();
  err := enter_dir("base");
  verify("final view", err = Ok and dir = "base");

  st := syslib.window_close(id);
  if failed then
    out.say("browser: failed\n")
  else
    out.say("browser: ok\n");
end;

procedure interactive_mode();
begin
  dir := "";
  load_dir();
  draw_all();
  present_frame();
  running := true;
  auto_draw := true;
  while running do
  begin
    st := syslib.next_event(ev);
    if st <> Ok then
      running := false
    else if ev.kind = Expose then
    begin
      if ev.window = id then
      begin
        refresh_rect();
        draw_all();
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
      dirty := false;
      if ev.kind = KeyDown then
      begin
        handle_key();
        dirty := true;
      end
      else if ev.kind = PointerDown or ev.kind = PointerUp
              or ev.kind = PointerMove then
      begin
        if handle_pointer() then
          dirty := true;
      end;
      if dirty and running then
      begin
        draw_all();
        present_frame();
      end;
    end;
  end;
  st := syslib.window_close(id);
end;

procedure shot_mode();
var
  deadline: uint;
  child_at: int;
begin
  dir := "";
  load_dir();
  st := enter_dir("base");
  child_at := find_name("child");
  if child_at >= 0 then
    iconview.move_to(view, child_at);
  draw_all();
  present_frame();
  out.say("browser: ready\n");
  deadline := core.clock() + uint(HoldSeconds) * uint(NanosPerSecond);
  auto_draw := true;
  // Block in `core.sleep` between event drains so the hold does not spend the
  // CPU watchdog budget (ADR 0061), like the editor and iconview shot modes.
  while core.clock() < deadline and running do
  begin
    drain_events();
    st := core.sleep(uint(50));
  end;
  st := syslib.window_close(id);
  out.say("browser: done\n");
end;

procedure main(args: const string[1024]);
var
  screen_w: uint;
  screen_h: uint;
begin
  failed := false;
  step := 0;
  running := true;
  auto_draw := true;
  dirty := false;
  opens := 0;
  last_click_ns := uint(0);
  last_click_item := -1;
  last_task := 0;
  status_line := "";
  dir := "";
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  gray := stdlib.ui_gray();
  accent := stdlib.ui_accent();
  init_menu();
  clip_mode := ClipNone;
  clip_full := "";
  clip_name := "";
  clip_kind := uint(0);

  // A non-destructive writability probe: a missing name on a writable mount is
  // `NotFound`, while an absent disk refuses the remove with `Invalid`. This
  // lets `N` distinguish "already exists" from "no writable home".
  last_op := core.fs_remove("home/igis_probe_none");
  no_home := last_op = Invalid;

  mode := 0;
  if stdlib.starts_with(args, "check") then
    mode := 1
  else if stdlib.starts_with(args, "shot") then
    mode := 2;

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    host_path()
  else if screen_w < uint(WindowX + ContentW + 40) or
          screen_h < uint(WindowY + ContentH + 60) then
    out.say("browser: screen too small\n")
  else
  begin
    st := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Browser", id);
    if st <> Ok then
      out.say("browser: no window\n")
    else
    begin
      st := syslib.window_content_rect(id, rect);
      if st <> Ok then
        out.say("browser: no content rect\n")
      else
      begin
        setup_view();
        // Load the four icons once, after the window is open and before any
        // frame. A failed load is non-fatal: the fallback outline is drawn and
        // navigation keeps working (spec 0028 B1).
        value := icons.load(icon_set, "icons");
        if mode = 1 then
          check_mode()
        else if mode = 2 then
          shot_mode()
        else
          interactive_mode();
      end;
    end;
  end;
end;

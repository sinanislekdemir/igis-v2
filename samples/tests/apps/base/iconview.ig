unit iconview_proof;

// Test-only proof for `lib.iconview` and `lib.icons` (spec 0028, P1/P2). The
// unit is `iconview_proof` because the imported module already owns the unit
// name `iconview`; the file is staged as `iconview` by `xtask image --shell`
// and by `langhost`'s e2e test.
//
//   run iconview                 deterministic self-check
//   run iconview check           the same self-check
//   run iconview shot            680x480 framed icon grid for the screenshot
//                                harness; prints `iconview: ready` and holds
//
// On the host (disabled screen) only the pure helpers run: the computed grid
// geometry (6 columns and 6 rows for a 648x420 grid with 100x68 cells), the
// hit test inside/outside the margin and past `count`, every navigation clamp,
// the row-scroll rule, all label-wrapping cases (space break, long unbroken
// name, UTF-8 code-point boundary, empty second line) and the 96/97 entry
// capacity. Nothing is drawn and the icon assets are never read, so the host
// prints exactly `iconview: host ok`.
//
// In the kernel the proof opens a 680x480 window at (160,90), loads the four
// icons from the read-only `icons/` image directory, builds a deterministic
// eight-entry view at (8,30) sized 648x420, draws it inside the window scope
// and reads representative pixels back: the white background, the black grid
// border, the accent selection band, the white label ink inside that band and
// at least one non-white pixel inside the first icon's 32x32 cell. A missing
// icon does not fault: `icons.draw` paints its fallback outline, and the
// `icons.ready` checks then report the failure. The window is always closed.

uses core, stdlib, syslib, lib.out, lib.icons, lib.iconview;

const
  // The `lib.icons` ids (module constants are not exported across a unit).
  FolderId = 0;
  SourceId = 1;
  FileId = 2;
  ImageId = 3;

  EntryCapacity = 96;
  NameCapacity = 64;

  WindowX = 160;
  WindowY = 90;
  ContentW = 680;
  ContentH = 480;

  GridX = 8;
  GridY = 30;
  GridW = 648;
  GridH = 420;
  CellW = 100;
  CellH = 68;

  IconSide = 32;
  Margin = 4;
  LabelGap = 4;
  LabelLineH = 12;

  HoldNs = 5000000000;

var
  failed: bool;
  step: int;
  fail_step: int;
  fail_label: string[64];
  digits: string[16];

  status: Status;
  screen_w: uint;
  screen_h: uint;
  id: int;
  rect: Rect;
  shot_mode: bool;

  white: uint;
  black: uint;
  accent: uint;

  set: icons.Set;
  v: iconview.View;
  wide: iconview.View;
  cap: iconview.View;
  added: int;

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

procedure report_failure();
begin
  out.say("iconview: failed check ");
  added := core.format_int_into(digits, fail_step, uint(10), uint(0));
  out.say(digits);
  out.say(" (");
  out.say(fail_label);
  out.say(")\n");
end;

// Whether any pixel inside a 32x32 cell at `(x, y)` differs from white. The
// first icon's raster, or the fallback outline when it is not ready, must
// leave ink there.
function cell_has_ink(x: int; y: int): bool;
var
  dx: int;
  dy: int;
  ink: int;
begin
  ink := 0;
  dy := 0;
  while dy < IconSide do
  begin
    dx := 0;
    while dx < IconSide do
    begin
      if syslib.get_pixel(x + dx, y + dy) <> white then
        ink := ink + 1;
      dx := dx + 1;
    end;
    dy := dy + 1;
  end;
  return ink > 0;
end;

// ---- host checks -----------------------------------------------------------

procedure host_checks();
var
  label: string[NameCapacity];
  i: int;
  at: int;
  page_entries: int;
  long65: string[128];
  name64: string[128];
begin
  // 1. Geometry: 640 usable pixels over 100-pixel cells is six columns; 420
  //    over 68-pixel cells is six rows.
  iconview.init(v, GridX, GridY, GridW, GridH, CellW, CellH);
  verify("columns", iconview.columns(v) = 6);
  verify("visible rows", iconview.visible_rows(v) = 6);
  verify("page size", iconview.page_size(v) = 36);
  verify("empty total rows", iconview.total_rows(v) = 0);
  verify("empty selected", v.selected = -1);
  verify("empty top", v.top = 0);
  verify("empty hit", iconview.index_at(v, GridX + Margin, GridY + Margin) = -1);
  verify("init stores rect", v.x = GridX and v.y = GridY and v.w = GridW and v.h = GridH);
  verify("init stores cells", v.cell_w = CellW and v.cell_h = CellH);

  // 2. Add: the first entry is selected; thirteen entries make three rows.
  verify("add folder", iconview.add(v, "Folder", uint(1), FolderId) = 0);
  verify("first selected", v.selected = 0);
  verify("add space name", iconview.add(v, "alpha beta gamma", uint(0), ImageId) = 1);
  verify("add long name", iconview.add(v, "abcdefghijklmnop", uint(0), FileId) = 2);
  verify("add utf8 name", iconview.add(v, "éééééééééééé", uint(0), FileId) = 3);
  verify("add short name", iconview.add(v, "short", uint(0), SourceId) = 4);
  verify("add empty name", iconview.add(v, "", uint(0), FileId) = 5);
  i := 6;
  while i < 13 do
  begin
    added := iconview.add(v, "entry", uint(i mod 3), i mod 4);
    verify("add filler", added = i);
    i := i + 1;
  end;
  verify("count thirteen", v.count = 13);
  verify("total three rows", iconview.total_rows(v) = 3);

  // 3. Hit test: the cell geometry, the margin and `count`.
  verify("hit cell 0", iconview.index_at(v, 12, 34) = 0);
  verify("hit cell 0 far corner", iconview.index_at(v, 111, 101) = 0);
  verify("hit cell 1", iconview.index_at(v, 112, 34) = 1);
  verify("hit row 1", iconview.index_at(v, 12, 102) = 6);
  verify("hit x margin", iconview.index_at(v, 8, 40) = -1);
  verify("hit y margin", iconview.index_at(v, 12, 32) = -1);
  verify("hit outside left", iconview.index_at(v, 7, 40) = -1);
  verify("hit right edge", iconview.index_at(v, GridX + GridW, 40) = -1);
  verify("hit bottom edge", iconview.index_at(v, 12, GridY + GridH) = -1);
  verify("hit past count", iconview.index_at(v, 112, 170) = -1);

  // 4. Navigation on the thirteen-entry view: every clamp, no wrap.
  verify("move right", iconview.move(v, 1, 0) and v.selected = 1);
  verify("move down row", iconview.move(v, 0, 1) and v.selected = 7);
  verify("move left", iconview.move(v, -1, 0) and v.selected = 6);
  verify("move clamp low", iconview.move(v, -100, 0) and v.selected = 0);
  verify("move up at start", not iconview.move(v, 0, -1) and v.selected = 0);
  verify("move clamp high", iconview.move(v, 100, 0) and v.selected = 12);
  verify("move past end", not iconview.move(v, 1, 0) and v.selected = 12);
  verify("move_to absolute", iconview.move_to(v, 5) and v.selected = 5);
  verify("move_to same", not iconview.move_to(v, 5) and v.selected = 5);
  verify("move_to clamp low", iconview.move_to(v, -4) and v.selected = 0);
  verify("move_to clamp high", iconview.move_to(v, 999) and v.selected = 12);
  iconview.first(v);
  verify("first", v.selected = 0 and v.top = 0);
  iconview.last(v);
  verify("last", v.selected = 12);
  iconview.first(v);
  page_entries := iconview.page_size(v);
  verify("page down", iconview.move(v, page_entries, 0) and v.selected = 12);
  verify("page down clamp", not iconview.move(v, page_entries, 0));
  verify("page up", iconview.move(v, -page_entries, 0) and v.selected = 0);

  // 5. Row scrolling on a sixty-entry view (ten rows, six visible, top 0..4).
  iconview.init(wide, GridX, GridY, GridW, GridH, CellW, CellH);
  i := 0;
  while i < 60 do
  begin
    iconview.add(wide, "item", uint(1), FolderId);
    i := i + 1;
  end;
  verify("wide count", wide.count = 60);
  verify("wide total rows", iconview.total_rows(wide) = 10);
  iconview.set_top(wide, 2);
  verify("top set", wide.top = 2);
  verify("hit with top", iconview.index_at(wide, 12, 34) = 12);
  verify("hit with top column", iconview.index_at(wide, 112, 34) = 13);
  iconview.set_top(wide, -3);
  verify("top clamp low", wide.top = 0);
  iconview.set_top(wide, 100);
  verify("top clamp high", wide.top = 4);

  verify("move_to last row", iconview.move_to(wide, 59) and wide.selected = 59);
  verify("last row top", wide.top = 4);
  verify("move_to same row", not iconview.move_to(wide, 59));
  verify("move_to middle row", iconview.move_to(wide, 24) and wide.top = 4);
  verify("move_to upper row", iconview.move_to(wide, 6) and wide.top = 1);
  verify("move row up", iconview.move(wide, 0, -1) and wide.selected = 0);
  verify("move row up top", wide.top = 0);
  verify("move clamp start", not iconview.move(wide, -1, 0));
  verify("move clamp end", iconview.move(wide, 0, 100) and wide.selected = 59);
  verify("move clamp end top", wide.top = 4);
  iconview.first(wide);
  verify("wide first", wide.selected = 0 and wide.top = 0);
  iconview.last(wide);
  verify("wide last", wide.selected = 59 and wide.top = 4);

  // 6. Pages move by `rows * page_size` entries and scroll the target into view.
  iconview.first(wide);
  iconview.page(wide, 1);
  verify("page down selection", wide.selected = 36);
  verify("page down top", wide.top = 1);
  iconview.page(wide, -1);
  verify("page up selection", wide.selected = 0 and wide.top = 0);
  iconview.page(wide, -1);
  verify("page up clamp", wide.selected = 0 and wide.top = 0);
  iconview.page(wide, 1);
  iconview.page(wide, 1);
  verify("page end selection", wide.selected = 59);
  verify("page end top", wide.top = 4);

  // 7. ensure_visible moves `top` by grid rows only.
  iconview.set_top(wide, 3);
  iconview.move_to(wide, 12);
  verify("ensure scroll up", wide.top = 2);
  iconview.move_to(wide, 30);
  verify("ensure inside stays", wide.top = 2);
  iconview.move_to(wide, 42);
  verify("ensure last visible stays", wide.top = 2);
  iconview.move_to(wide, 48);
  verify("ensure scroll down", wide.top = 3);
  iconview.set_top(v, 2);
  verify("small view top clamps to zero", v.top = 0);

  // 8. Labels: 11 glyphs for a 100-pixel cell, space break, hard break, UTF-8
  //    boundary and an untouched destination on an empty line.
  label := "keep";
  verify("space line 0", iconview.label_line(v, 1, 0, label));
  verify("space line 0 text", label = "alpha beta");
  verify("space line 1", iconview.label_line(v, 1, 1, label) and label = "gamma");
  label := "keep";
  verify("space no line 2", not iconview.label_line(v, 1, 2, label));
  verify("space line 2 untouched", label = "keep");
  verify("hard line 0", iconview.label_line(v, 2, 0, label) and label = "abcdefghijk");
  verify("hard line 1", iconview.label_line(v, 2, 1, label) and label = "lmnop");
  verify("utf8 line 0", iconview.label_line(v, 3, 0, label));
  verify("utf8 line 0 bytes", int(core.len(label)) = 22);
  verify("utf8 line 1", iconview.label_line(v, 3, 1, label) and label = "é");
  verify("short line 0", iconview.label_line(v, 4, 0, label) and label = "short");
  label := "keep";
  verify("short line 1 empty", not iconview.label_line(v, 4, 1, label));
  verify("short line 1 untouched", label = "keep");
  label := "keep";
  verify("empty name line 0", not iconview.label_line(v, 5, 0, label));
  verify("empty name untouched", label = "keep");
  verify("label out of range", not iconview.label_line(v, 13, 0, label));

  // 9. Name capacity and the 96/97 entry capacity.
  iconview.init(cap, GridX, GridY, GridW, GridH, CellW, CellH);
  long65 := "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijklmno";
  name64 := "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijklmn";
  verify("name too long", iconview.add(cap, long65, uint(0), FileId) = -1);
  verify("name too long adds nothing", cap.count = 0);
  verify("name at capacity", iconview.add(cap, name64, uint(0), FileId) = 0);
  verify("name at capacity label 0",
    iconview.label_line(cap, 0, 0, label) and int(core.len(label)) = 11);
  verify("name at capacity label 1",
    iconview.label_line(cap, 0, 1, label) and int(core.len(label)) = 11);
  iconview.clear(cap);
  verify("clear count", cap.count = 0 and cap.selected = -1 and cap.top = 0);
  i := 0;
  at := -1;
  while i < EntryCapacity do
  begin
    at := iconview.add(cap, "n", uint(0), FileId);
    i := i + 1;
  end;
  verify("capacity accepts 96", at = 95 and cap.count = 96);
  verify("capacity first selected", cap.selected = 0);
  verify("capacity total rows", iconview.total_rows(cap) = 16);
  verify("capacity refuses 97",
    iconview.add(cap, "n", uint(0), FileId) = -1 and cap.count = 96);
end;

// ---- kernel proof ----------------------------------------------------------

// The deterministic eight-entry grid used by `check` and `shot`. Entry 0 is
// selected and its label is `Folder`; entry 3 wraps to two lines.
procedure build_view();
begin
  iconview.init(v, GridX, GridY, GridW, GridH, CellW, CellH);
  added := iconview.add(v, "Folder", uint(1), FolderId);
  added := iconview.add(v, "image.png", uint(0), ImageId);
  added := iconview.add(v, "child", uint(1), FolderId);
  added := iconview.add(v, "a_long_file_name.png", uint(0), ImageId);
  added := iconview.add(v, "igfile", uint(0), SourceId);
  added := iconview.add(v, "notes.txt", uint(0), FileId);
  added := iconview.add(v, "samples", uint(1), FolderId);
  added := iconview.add(v, "photo.jpg", uint(0), ImageId);
end;

procedure load_icons();
begin
  status := icons.load(set, "icons");
end;

function open_grid(title: const string[0]): bool;
begin
  status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, title, id);
  if status <> Ok then
  begin
    id := 0;
    return false;
  end;
  status := syslib.window_begin(id);
  if status <> Ok then
    return false;
  status := syslib.window_content_rect(id, rect);
  if status <> Ok then
    return false;
  return rect.w = ContentW and rect.h = ContentH;
end;

procedure kernel_proof();
var
  label: string[NameCapacity];
  icon_x: int;
  icon_y: int;
  text_x: int;
  band_y: int;
begin
  if screen_w < uint(WindowX + ContentW + 2) or screen_h < uint(WindowY + ContentH + 17) then
  begin
    out.say("iconview: screen too small\n");
    return;
  end;
  verify("window open", open_grid("Icon View"));
  if id = 0 then
  begin
    report_failure();
    return;
  end;

  load_icons();
  verify("icon folder", icons.ready(set, FolderId));
  verify("icon source", icons.ready(set, SourceId));
  verify("icon file", icons.ready(set, FileId));
  verify("icon image", icons.ready(set, ImageId));

  build_view();
  verify("view columns", iconview.columns(v) = 6);
  verify("view rows", iconview.visible_rows(v) = 6);
  verify("view count", v.count = 8);
  verify("view selected", v.selected = 0);

  status := iconview.draw(v, set);
  verify("draw", status = Ok);
  status := syslib.window_present(id);
  verify("present", status = Ok);

  // Window-local read-back inside the open scope (spec 0026 C6/C8).
  verify("white background", syslib.get_pixel(GridX + 2, GridY + 2) = white);
  verify("black border", syslib.get_pixel(GridX, GridY) = black);
  icon_x := GridX + Margin + (CellW - IconSide) div 2;
  icon_y := GridY + 2 * Margin;
  band_y := icon_y + IconSide + LabelGap;
  verify("accent band", syslib.get_pixel(GridX + Margin + 1, band_y + 1) = accent);
  text_x := GridX + Margin + (CellW - stdlib.ui_text_width("Folder")) div 2;
  verify("white label ink", syslib.get_pixel(text_x, band_y + 2) = white);
  verify("icon ink", cell_has_ink(icon_x, icon_y));

  status := syslib.window_close(id);
  verify("window closed", status = Ok);
  id := 0;
  if failed then
    report_failure()
  else
    out.say("iconview: ok\n");
end;

// ---- shot mode -------------------------------------------------------------

procedure run_shot();
var
  start: uint;
begin
  if screen_w < uint(WindowX + ContentW + 2) or screen_h < uint(WindowY + ContentH + 17) then
  begin
    out.say("iconview: screen too small\n");
    return;
  end;
  if not open_grid("Icon View") then
  begin
    out.say("iconview: no window\n");
    return;
  end;
  load_icons();
  build_view();
  status := iconview.draw(v, set);
  if status = Ok then
    status := syslib.window_present(id);
  if status <> Ok then
  begin
    status := syslib.window_close(id);
    id := 0;
    out.say("iconview: failed\n");
    return;
  end;
  out.say("iconview: ready\n");
  // Block in `core.sleep` so the hold does not spend the CPU watchdog budget
  // (ADR 0061), like the editor's shot mode.
  start := core.clock();
  while core.clock() - start < uint(HoldNs) do
    status := core.sleep(uint(50));
  status := syslib.window_close(id);
  id := 0;
end;

// ---- entry point -----------------------------------------------------------

procedure main(args: const string[1024]);
var
  trimmed: string[256];
  used: int;
begin
  failed := false;
  step := 0;
  fail_step := 0;
  added := 0;
  id := 0;
  shot_mode := false;
  white := stdlib.ui_white();
  black := stdlib.ui_black();
  accent := stdlib.ui_accent();
  used := stdlib.trim_into(trimmed, args);
  if used > 0 and trimmed = "shot" then
    shot_mode := true;

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
  begin
    // No screen: prove the pure helpers, never draw.
    host_checks();
    if failed then
      report_failure()
    else
      out.say("iconview: host ok\n");
    return;
  end;
  if shot_mode then
  begin
    run_shot();
    return;
  end;
  kernel_proof();
end;

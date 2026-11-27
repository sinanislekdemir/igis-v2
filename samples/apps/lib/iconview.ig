unit iconview;

// The icon grid widget (spec 0028; ADR 0062). Ordinary `/lib` module source
// under the M5/M6 rules: no `main`, no top-level `var` state and no stream
// endpoint. The application owns the `View`; the module owns no state and no
// window. Every geometry, navigation and label helper is pure and runs on
// `langhost`; only `draw` (and `lib.icons`' `draw`) needs a screen.
//
//   var v: iconview.View;
//   var set: icons.Set;
//   iconview.init(v, 8, 30, 648, 420, 100, 68);
//   at := iconview.add(v, "child", uint(1), 0);   // 0 is the folder id
//   ...
//   st := iconview.draw(v, set);   // inside a `window_begin` scope
//
// Geometry (V3/V4): a cell's top-left is
// `(x + Margin + col*cell_w, y + Margin + row*cell_h)` and its icon is centred
// horizontally starting at `cell_y + Margin`. The label band below the icon is
// two 12-pixel lines. A point maps to `(top + row) * cols + col`; a point
// outside the grid rectangle, in the margin or past `count` is `-1`. The hit
// test uses the same arithmetic as the drawing, never the glyphs.
//
// Labels (V6): each line holds at most `(cell_w - 8) div 8` glyphs. Line 0
// breaks after the last space inside the limit when there is one, otherwise on
// a code-point boundary; line 1 takes the remainder up to the same limit and
// text beyond two lines is cut on a boundary. An empty line leaves `out`
// untouched.
//
// Drawing uses only `syslib.fill_rect`, `syslib.draw_rect`, `syslib.draw_text`
// and `icons.draw`; the caller opens the window scope. The white grid, the
// one-pixel black border, the accent label band and the two-pixel accent icon
// border of the selected entry are all ordinary calls. A failure returns the
// first non-`Ok` status; an empty view draws only the rectangle and border.
//
// No new opcode, builtin or `Host` method.

uses core, stdlib, syslib, lib.icons;

const
  EntryCapacity = 96;
  NameCapacity = 64;
  IconSide = 32;
  Margin = 4;
  LabelGap = 4;
  LabelLineH = 12;
  LabelLines = 2;

  // The 8x8 font cell, used for the label glyph limit (`ui_text_width` counts
  // the same cells for the ASCII the font covers).
  GlyphWidth = 8;

type View = record
  x, y, w, h: int;
  cell_w, cell_h: int;
  cols, rows: int;       // computed by init
  names: array[string[NameCapacity], EntryCapacity];
  kinds: array[uint, EntryCapacity];
  icon_ids: array[int, EntryCapacity];
  count: int;
  selected: int;
  top: int;              // first visible grid row
end;

// ---- geometry and navigation ----------------------------------------------

procedure init(v: var View; x: int; y: int; w: int; h: int;
                cell_w: int; cell_h: int);
begin
  v.x := x;
  v.y := y;
  v.w := w;
  v.h := h;
  v.cell_w := cell_w;
  v.cell_h := cell_h;
  v.cols := 1;
  v.rows := 1;
  if cell_w > 0 then
    v.cols := (w - 2 * Margin) div cell_w;
  if cell_h > 0 then
    v.rows := h div cell_h;
  if v.cols < 1 then
    v.cols := 1;
  if v.rows < 1 then
    v.rows := 1;
  v.count := 0;
  v.selected := -1;
  v.top := 0;
end;

procedure clear(v: var View);
begin
  v.count := 0;
  v.selected := -1;
  v.top := 0;
end;

function add(v: var View; name: const string[0]; kind: uint;
              icon_id: int): int;
var
  length: int;
  copied: int;
begin
  if v.count >= EntryCapacity then
    return -1;
  length := int(core.len(name));
  if length > NameCapacity then
    return -1;
  copied := core.substr_into(v.names[v.count], name, 0, length);
  if copied <> length then
    return -1;
  v.kinds[v.count] := kind;
  v.icon_ids[v.count] := icon_id;
  if v.count = 0 then
    v.selected := 0;
  v.count := v.count + 1;
  return v.count - 1;
end;

function columns(v: const View): int;
begin
  return v.cols;
end;

function visible_rows(v: const View): int;
begin
  return v.rows;
end;

function total_rows(v: const View): int;
begin
  if v.count <= 0 then
    return 0;
  return (v.count + v.cols - 1) div v.cols;
end;

function page_size(v: const View): int;
begin
  return v.cols * v.rows;
end;

// The entry under a window-local point, or -1. The rectangle test excludes the
// outer edge and the margin; the cell arithmetic is the one `draw` uses.
function index_at(v: const View; px: int; py: int): int;
var
  rel_x: int;
  rel_y: int;
  col: int;
  row: int;
  at: int;
begin
  if v.count <= 0 then
    return -1;
  if px < v.x or px >= v.x + v.w or py < v.y or py >= v.y + v.h then
    return -1;
  rel_x := px - v.x - Margin;
  rel_y := py - v.y - Margin;
  if rel_x < 0 or rel_y < 0 then
    return -1;
  if v.cell_w <= 0 or v.cell_h <= 0 then
    return -1;
  col := rel_x div v.cell_w;
  row := rel_y div v.cell_h;
  if col >= v.cols or row >= v.rows then
    return -1;
  at := (v.top + row) * v.cols + col;
  if at < 0 or at >= v.count then
    return -1;
  return at;
end;

// Clamps `top` into `[0, total_rows - visible_rows]`, never negative.
procedure clamp_top(v: var View);
var
  max: int;
begin
  max := total_rows(v) - v.rows;
  if max < 0 then
    max := 0;
  if v.top < 0 then
    v.top := 0;
  if v.top > max then
    v.top := max;
end;

procedure set_top(v: var View; top: int);
begin
  v.top := top;
  clamp_top(v);
end;

procedure ensure_visible(v: var View);
var
  row: int;
begin
  if v.count <= 0 or v.selected < 0 then
    return;
  row := v.selected div v.cols;
  if row < v.top then
    v.top := row
  else if row >= v.top + v.rows then
    v.top := row - v.rows + 1;
  clamp_top(v);
end;

function move(v: var View; dx: int; dy: int): bool;
var
  current: int;
  target: int;
  changed: bool;
begin
  if v.count <= 0 then
    return false;
  current := v.selected;
  if current < 0 then
    current := 0;
  target := current + dx + dy * v.cols;
  if target < 0 then
    target := 0;
  if target >= v.count then
    target := v.count - 1;
  changed := target <> v.selected;
  if changed then
    v.selected := target;
  ensure_visible(v);
  return changed;
end;

function move_to(v: var View; at: int): bool;
var
  target: int;
  changed: bool;
begin
  if v.count <= 0 then
    return false;
  target := at;
  if target < 0 then
    target := 0;
  if target >= v.count then
    target := v.count - 1;
  changed := target <> v.selected;
  if changed then
    v.selected := target;
  ensure_visible(v);
  return changed;
end;

// One page (or `rows` pages, backwards when negative) by grid rows.
procedure page(v: var View; rows: int);
var
  changed: bool;
begin
  if v.count <= 0 then
    return;
  changed := move(v, rows * page_size(v), 0);
end;

procedure first(v: var View);
begin
  if v.count <= 0 then
  begin
    v.selected := -1;
    return;
  end;
  v.selected := 0;
  ensure_visible(v);
end;

procedure last(v: var View);
begin
  if v.count <= 0 then
  begin
    v.selected := -1;
    return;
  end;
  v.selected := v.count - 1;
  ensure_visible(v);
end;

// ---- label wrapping --------------------------------------------------------

// The byte offset after the code point that starts at `from` (the length when
// `from` is at or past the end).
function next_glyph(text: const string[0]; from: int): int;
var
  at: int;
  length: int;
begin
  length := int(core.len(text));
  at := from;
  if at >= length then
    return length;
  at := at + 1;
  while at < length and text[at] >= byte(0x80) and text[at] <= byte(0xBF) do
    at := at + 1;
  return at;
end;

// The byte offset after at most `limit` code points starting at `start`.
function limited_end(text: const string[0]; start: int; limit: int): int;
var
  at: int;
  used: int;
  length: int;
begin
  length := int(core.len(text));
  at := start;
  used := 0;
  while used < limit and at < length do
  begin
    at := next_glyph(text, at);
    used := used + 1;
  end;
  return at;
end;

// Publishes line 0 or line 1 of the two-line label of entry `at` and returns
// whether the line is non-empty. `out` is untouched for an empty line.
function label_line(v: const View; at: int; line: int;
                     out: var string[NameCapacity]): bool;
var
  name: string[NameCapacity];
  limit: int;
  length: int;
  end0: int;
  space: int;
  start1: int;
  end1: int;
  take: int;
  i: int;
  copied: int;
begin
  if at < 0 or at >= v.count or line < 0 or line >= LabelLines then
    return false;
  limit := (v.cell_w - 8) div GlyphWidth;
  if limit < 1 then
    return false;
  name := v.names[at];
  length := int(core.len(name));
  if length = 0 then
    return false;

  // The first line's hard limit and the last space inside it.
  end0 := limited_end(name, 0, limit);
  space := -1;
  i := 0;
  while i < end0 do
  begin
    if name[i] = byte(32) then
      space := i;
    i := i + 1;
  end;

  if end0 >= length then
  begin
    // The whole name fits line 0; line 1 is empty.
    if line <> 0 then
      return false;
    copied := core.substr_into(out, name, 0, length);
    return copied = length;
  end;

  if line = 0 then
  begin
    take := end0;
    if space >= 0 then
      take := space;
    if take <= 0 then
      return false;
    copied := core.substr_into(out, name, 0, take);
    return copied = take;
  end;

  if space >= 0 then
    start1 := space + 1
  else
    start1 := end0;
  if start1 >= length then
    return false;
  end1 := limited_end(name, start1, limit);
  if end1 <= start1 then
    return false;
  copied := core.substr_into(out, name, start1, end1 - start1);
  return copied = end1 - start1;
end;

// ---- drawing ---------------------------------------------------------------

function draw(v: const View; s: const icons.Set): Status;
var
  st: Status;
  first: Status;
  white: uint;
  black: uint;
  accent: uint;
  row: int;
  col: int;
  at: int;
  cell_x: int;
  cell_y: int;
  icon_x: int;
  icon_y: int;
  band_y: int;
  line: int;
  text_w: int;
  text_x: int;
  label: string[NameCapacity];
begin
  white := stdlib.ui_white();
  black := stdlib.ui_black();
  accent := stdlib.ui_accent();
  first := Ok;

  st := syslib.fill_rect(v.x, v.y, v.w, v.h, white);
  if st <> Ok then
    first := st;
  st := syslib.draw_rect(v.x, v.y, v.w, v.h, black);
  if st <> Ok and first = Ok then
    first := st;
  if v.count <= 0 then
    return first;

  row := 0;
  while row < v.rows do
  begin
    col := 0;
    while col < v.cols do
    begin
      at := (v.top + row) * v.cols + col;
      if at >= v.count then
        col := v.cols
      else
      begin
        cell_x := v.x + Margin + col * v.cell_w;
        cell_y := v.y + Margin + row * v.cell_h;
        icon_x := cell_x + (v.cell_w - IconSide) div 2;
        icon_y := cell_y + Margin;
        band_y := icon_y + IconSide + LabelGap;

        if at = v.selected then
        begin
          // The two-line label band and the two-pixel icon border.
          st := syslib.fill_rect(
            cell_x, band_y, v.cell_w, LabelLines * LabelLineH, accent);
          if st <> Ok and first = Ok then
            first := st;
          st := syslib.draw_rect(
            icon_x - 2, icon_y - 2, IconSide + 4, IconSide + 4, accent);
          if st <> Ok and first = Ok then
            first := st;
          st := syslib.draw_rect(
            icon_x - 1, icon_y - 1, IconSide + 2, IconSide + 2, accent);
          if st <> Ok and first = Ok then
            first := st;
        end;

        st := icons.draw(s, v.icon_ids[at], icon_x, icon_y, black);
        if st <> Ok and first = Ok then
          first := st;

        line := 0;
        while line < LabelLines do
        begin
          if label_line(v, at, line, label) then
          begin
            text_w := stdlib.ui_text_width(label);
            text_x := cell_x + (v.cell_w - text_w) div 2;
            if at = v.selected then
              st := syslib.draw_text(
                text_x, band_y + line * LabelLineH + 2, label, white)
            else
              st := syslib.draw_text(
                text_x, band_y + line * LabelLineH + 2, label, black);
            if st <> Ok and first = Ok then
              first := st;
          end;
          line := line + 1;
        end;
        col := col + 1;
      end;
    end;
    row := row + 1;
  end;
  return first;
end;

unit calendar;

// A read-only month calendar (spec 0027). Today comes from the CMOS RTC; the
// view walks months and years with the arrow keys or the `<`/`>` buttons, and
// a click selects a day. There is no file store: day notes are out of scope.
//
//   run apps/productivity/calendar          // live window until its close box
//   run apps/productivity/calendar check    // deterministic checks, prints `calendar: ok`
//   run apps/productivity/calendar shot     // draws the host month, holds ~5 s
//                                          // and prints `calendar: ready YYYY-MM-DD`
//
// The grid is Monday-first and always six rows (42 cells); a cell outside the
// month is blank and is not a hit target. `today` is inverted only while the
// shown month is the host month. A selected day gets a one-pixel accent
// rectangle. All date math is integer-only and pure, so `langhost` runs the
// same leap/month/weekday/navigation logic with a disabled screen and prints
// `calendar: host ok`.
//
// Geometry and drawing follow `clock_window`: client content only, one complete
// frame before one `window_present`, an owner `Expose` repaints the whole
// content after re-reading `window_content_rect`, and the live loop blocks in
// `syslib.next_event` so blocked time is not charged to the task budget.

uses core, stdlib, syslib, lib.out;

const
  WindowX = 300;
  WindowY = 110;
  ContentW = 252;
  ContentH = 240;

  // Client frame and header.
  Inset = 4;
  HeaderY = 8;
  NavW = 20;
  NavH = 18;
  NavLeftX = 8;
  NavRightX = ContentW - 8 - NavW;

  // Weekday row and the six-row grid.
  WeekY = 34;
  GridX = 14;
  GridY = 48;
  CellW = 32;
  CellH = 24;
  GridRows = 6;
  GridCols = 7;
  GridW = GridCols * CellW;
  GridH = GridRows * CellH;

  // Footer lines.
  FooterY = 200;
  HintY = 216;

  // Navigation button identities.
  NavNone = 0;
  NavPrev = 1;
  NavNext = 2;

  MonthCount = 12;
  MinYear = 1970;
  MaxYear = 9999;

  // Deterministic check date (a Thursday).
  FixedYear = 2026;
  FixedMonth = 11;
  FixedDay = 26;

  // Shot mode hold.
  HoldMs = 5000;

  // CMOS RTC registers (QEMU and a PC RTC).
  RtcIndex = 0x70;
  RtcData = 0x71;
  RtcStatusA = 0x0A;
  RtcStatusB = 0x0B;
  RtcDay = 0x07;
  RtcMonth = 0x08;
  RtcYear = 0x09;
  RtcCentury = 0x32;

var
  failed: bool;
  step: int;
  step_text: string[16];
  status: Status;
  ignored: Status;
  written: int;

  screen_w: uint;
  screen_h: uint;
  id: int;
  rect: Rect;

  black: uint;
  white: uint;
  accent: uint;

  month_names: array[string[10], MonthCount];
  week_names: array[string[2], GridCols];

  // View state and the host's today.
  view_year: int;
  view_month: int;
  selected_day: int;
  today_known: bool;
  today_year: int;
  today_month: int;
  today_day: int;

  // Scratch text.
  day_text: string[4];
  year_text: string[8];

  ev: Event;
  running: bool;
  pressed_nav: int;

// Silent check: the first failure is printed with its step number.
procedure verify(label: const string[0]; passed: bool);
begin
  step := step + 1;
  if not passed then
  begin
    failed := true;
    out.say("calendar: failed check ");
    written := core.format_int_into(step_text, step, uint(10), uint(0));
    out.say(step_text);
    out.say(" (");
    out.say(label);
    out.say(")\n");
  end;
end;

procedure init_names();
begin
  month_names[0] := "January";
  month_names[1] := "February";
  month_names[2] := "March";
  month_names[3] := "April";
  month_names[4] := "May";
  month_names[5] := "June";
  month_names[6] := "July";
  month_names[7] := "August";
  month_names[8] := "September";
  month_names[9] := "October";
  month_names[10] := "November";
  month_names[11] := "December";

  week_names[0] := "Mo";
  week_names[1] := "Tu";
  week_names[2] := "We";
  week_names[3] := "Th";
  week_names[4] := "Fr";
  week_names[5] := "Sa";
  week_names[6] := "Su";
end;

// ---- date math ----------------------------------------------------------

function is_leap(year: int): bool;
begin
  if year mod 400 = 0 then
    return true;
  if year mod 100 = 0 then
    return false;
  return year mod 4 = 0;
end;

function month_days(year: int; month: int): int;
begin
  if month = 2 then
  begin
    if is_leap(year) then
      return 29;
    return 28;
  end;
  if month = 4 or month = 6 or month = 9 or month = 11 then
    return 30;
  return 31;
end;

// Days since 1970-01-01 (Howard Hinnant's civil-from-days inverse). The view
// never leaves 1970..9999, so every intermediate value is positive.
function days_from_civil(year: int; month: int; day: int): int;
var
  y: int;
  era: int;
  yoe: int;
  mp: int;
  doy: int;
  doe: int;
begin
  y := year;
  if month <= 2 then
    y := y - 1;
  era := y div 400;
  if y < 0 and y mod 400 <> 0 then
    era := era - 1;
  yoe := y - era * 400;
  if month > 2 then
    mp := month - 3
  else
    mp := month + 9;
  doy := (153 * mp + 2) div 5 + day - 1;
  doe := yoe * 365 + yoe div 4 - yoe div 100 + doy;
  return era * 146097 + doe - 719468;
end;

// 0 = Sunday .. 6 = Saturday. 1970-01-01 was a Thursday, so +4.
function weekday_sun(year: int; month: int; day: int): int;
begin
  return (days_from_civil(year, month, day) + 4) mod 7;
end;

// Monday-first column of the first day of the view month.
function lead_days(): int;
begin
  return (weekday_sun(view_year, view_month, 1) + 6) mod 7;
end;

function days_in_view(): int;
begin
  return month_days(view_year, view_month);
end;

function day_for_cell(row: int; col: int): int;
var
  slot: int;
begin
  slot := row * GridCols + col - lead_days();
  if slot < 0 then
    return 0;
  if slot >= days_in_view() then
    return 0;
  return slot + 1;
end;

function cell_row(day: int): int;
begin
  return (lead_days() + day - 1) div GridCols;
end;

function cell_col(day: int): int;
begin
  return (lead_days() + day - 1) mod GridCols;
end;

function valid_date(year: int; month: int; day: int): bool;
begin
  if year < MinYear or year > MaxYear then
    return false;
  if month < 1 or month > MonthCount then
    return false;
  if day < 1 or day > month_days(year, month) then
    return false;
  return true;
end;

// ---- navigation ---------------------------------------------------------

procedure select_none();
begin
  selected_day := 0;
end;

procedure prev_month();
begin
  if view_month = 1 then
  begin
    if view_year > MinYear then
    begin
      view_year := view_year - 1;
      view_month := MonthCount;
    end;
  end
  else
    view_month := view_month - 1;
  select_none();
end;

procedure next_month();
begin
  if view_month = MonthCount then
  begin
    if view_year < MaxYear then
    begin
      view_year := view_year + 1;
      view_month := 1;
    end;
  end
  else
    view_month := view_month + 1;
  select_none();
end;

procedure prev_year();
begin
  if view_year > MinYear then
    view_year := view_year - 1;
  select_none();
end;

procedure next_year();
begin
  if view_year < MaxYear then
    view_year := view_year + 1;
  select_none();
end;

procedure go_today();
begin
  if today_known then
  begin
    view_year := today_year;
    view_month := today_month;
    selected_day := today_day;
  end;
end;

// ---- CMOS RTC -----------------------------------------------------------

procedure rtc_select(register: uint);
begin
  syslib.port_out8(uint(RtcIndex), register);
end;

function rtc_read(register: uint): uint;
begin
  rtc_select(register);
  return syslib.port_in8(uint(RtcData));
end;

// BCD to binary: (v div 16) * 10 + (v mod 16).
function rtc_bcd(value: uint): int;
begin
  return int((value div uint(16)) * uint(10) + (value mod uint(16)));
end;

// Reads today's date once. Honours the status-B binary flag, waits out the
// update-in-progress window and accepts only two equal consecutive reads. A
// missing century register falls back to 2000 + yy. On any failure the old
// `today` state is cleared and today is not highlighted.
procedure sync_today();
var
  status_b: uint;
  binary: bool;
  day1: uint;
  month1: uint;
  year1: uint;
  day2: uint;
  month2: uint;
  year2: uint;
  century: uint;
  century_value: int;
  yy: int;
  guard: uint;
  tries: int;
  year: int;
  month: int;
  day: int;
begin
  today_known := false;
  status_b := rtc_read(uint(RtcStatusB));
  binary := (status_b div uint(4)) mod uint(2) = uint(1);
  tries := 0;
  while tries < 16 do
  begin
    guard := uint(0);
    while rtc_read(uint(RtcStatusA)) >= uint(0x80) and guard < uint(1000000) do
      guard := guard + uint(1);
    day1 := rtc_read(uint(RtcDay));
    month1 := rtc_read(uint(RtcMonth));
    year1 := rtc_read(uint(RtcYear));
    day2 := rtc_read(uint(RtcDay));
    month2 := rtc_read(uint(RtcMonth));
    year2 := rtc_read(uint(RtcYear));
    if day1 = day2 and month1 = month2 and year1 = year2 then
    begin
      if binary then
      begin
        day := int(day1);
        month := int(month1);
        yy := int(year1);
      end
      else
      begin
        day := rtc_bcd(day1);
        month := rtc_bcd(month1);
        yy := rtc_bcd(year1);
      end;
      century := rtc_read(uint(RtcCentury));
      if binary then
        century_value := int(century)
      else
        century_value := rtc_bcd(century);
      if century_value >= 19 and century_value <= 21 then
        year := century_value * 100 + yy
      else
        year := 2000 + yy;
      if valid_date(year, month, day) then
      begin
        today_year := year;
        today_month := month;
        today_day := day;
        today_known := true;
        view_year := year;
        view_month := month;
        selected_day := day;
      end;
      return;
    end;
    tries := tries + 1;
  end;
end;

// ---- layout and drawing -------------------------------------------------

function put_text(x: int; y: int; text: const string[0]; color: uint): int;
begin
  ignored := syslib.draw_text(x, y, text, color);
  return x + stdlib.ui_text_width(text);
end;

procedure draw_title();
var
  month_width: int;
  year_width: int;
  total: int;
  area_x: int;
  area_w: int;
  x: int;
begin
  month_width := stdlib.ui_text_width(month_names[view_month - 1]);
  written := core.format_int_into(year_text, view_year, uint(10), uint(0));
  year_width := stdlib.ui_text_width(year_text);
  total := month_width + 8 + year_width;
  area_x := NavLeftX + NavW + 4;
  area_w := NavRightX - area_x;
  x := rect.x + area_x;
  if total < area_w then
    x := x + (area_w - total) div 2;
  x := put_text(x, rect.y + HeaderY + 5, month_names[view_month - 1], black);
  x := put_text(x, rect.y + HeaderY + 5, " ", black);
  x := put_text(x, rect.y + HeaderY + 5, year_text, black);
end;

procedure draw_week_header();
var
  col: int;
begin
  col := 0;
  while col < GridCols do
  begin
    ignored := syslib.draw_text(
      rect.x + GridX + col * CellW + (CellW - 16) div 2,
      rect.y + WeekY, week_names[col], black);
    col := col + 1;
  end;
end;

// One day cell: today is inverted, a selected day gets an accent outline, and
// every other day is plain black ink on the white panel.
procedure draw_day(row: int; col: int; day: int);
var
  x: int;
  y: int;
  width: int;
  color: uint;
begin
  x := rect.x + GridX + col * CellW;
  y := rect.y + GridY + row * CellH;
  written := core.format_int_into(day_text, day, uint(10), uint(0));
  width := written * 8;
  color := black;
  if today_known and view_year = today_year and view_month = today_month
     and day = today_day then
  begin
    ignored := syslib.fill_rect(x + 2, y + 2, CellW - 4, CellH - 4, black);
    color := white;
  end
  else if day = selected_day then
    ignored := syslib.draw_rect(x + 2, y + 2, CellW - 4, CellH - 4, accent);
  ignored := syslib.draw_text(x + (CellW - width) div 2, y + (CellH - 8) div 2,
                              day_text, color);
end;

procedure draw_grid();
var
  row: int;
  col: int;
  day: int;
begin
  row := 0;
  while row < GridRows do
  begin
    col := 0;
    while col < GridCols do
    begin
      day := day_for_cell(row, col);
      if day > 0 then
        draw_day(row, col, day);
      col := col + 1;
    end;
    row := row + 1;
  end;
end;

procedure draw_footer();
var
  x: int;
  total: int;
  day_width: int;
begin
  if today_known then
  begin
    written := core.format_int_into(day_text, today_day, uint(10), uint(0));
    day_width := written * 8;
    written := core.format_int_into(year_text, today_year, uint(10), uint(0));
    total := 56 + day_width + 8
             + stdlib.ui_text_width(month_names[today_month - 1])
             + 8 + stdlib.ui_text_width(year_text);
    x := rect.x;
    if total < ContentW then
      x := x + (ContentW - total) div 2;
    x := put_text(x, rect.y + FooterY, "Today: ", black);
    x := put_text(x, rect.y + FooterY, day_text, black);
    x := put_text(x, rect.y + FooterY, " ", black);
    x := put_text(x, rect.y + FooterY, month_names[today_month - 1], black);
    x := put_text(x, rect.y + FooterY, " ", black);
    x := put_text(x, rect.y + FooterY, year_text, black);
  end
  else
  begin
    total := 144;
    x := rect.x + (ContentW - total) div 2;
    ignored := syslib.draw_text(x, rect.y + FooterY, "Today: unavailable", black);
  end;
  ignored := syslib.draw_text(
    rect.x + (ContentW - 224) div 2, rect.y + HintY,
    "Arrows: month/year  H: today", black);
end;

procedure draw_calendar();
begin
  ignored := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, white);
  ignored := stdlib.ui_frame(rect.x + Inset, rect.y + Inset,
                             ContentW - 2 * Inset, ContentH - 2 * Inset);
  ignored := stdlib.ui_button(rect.x + NavLeftX, rect.y + HeaderY, NavW, NavH,
                              "<", pressed_nav = NavPrev, false);
  ignored := stdlib.ui_button(rect.x + NavRightX, rect.y + HeaderY, NavW, NavH,
                              ">", pressed_nav = NavNext, false);
  draw_title();
  draw_week_header();
  draw_grid();
  draw_footer();
  ignored := syslib.window_present(id);
end;

procedure refresh_geometry();
begin
  status := syslib.window_content_rect(id, rect);
end;

// ---- input helpers ------------------------------------------------------

// 1 for `<`, 2 for `>`, 0 for the rest of the header.
function nav_hit(px: int; py: int): int;
begin
  if stdlib.ui_button_hit(rect.x + NavLeftX, rect.y + HeaderY, NavW, NavH, px, py) then
    return NavPrev;
  if stdlib.ui_button_hit(rect.x + NavRightX, rect.y + HeaderY, NavW, NavH, px, py) then
    return NavNext;
  return NavNone;
end;

// The day under a screen point, or 0 when the point is outside the grid or the
// cell belongs to a neighbouring month.
function day_at_point(px: int; py: int): int;
var
  rel_x: int;
  rel_y: int;
begin
  rel_x := px - (rect.x + GridX);
  rel_y := py - (rect.y + GridY);
  if rel_x < 0 or rel_x >= GridW or rel_y < 0 or rel_y >= GridH then
    return 0;
  return day_for_cell(rel_y div CellH, rel_x div CellW);
end;

procedure activate_nav(key: int);
begin
  if key = NavPrev then
    prev_month()
  else if key = NavNext then
    next_month();
end;

procedure handle_pointer();
var
  hit: int;
  day: int;
begin
  if ev.kind = PointerDown then
  begin
    hit := nav_hit(ev.x, ev.y);
    if hit <> NavNone then
      pressed_nav := hit
    else
    begin
      day := day_at_point(ev.x, ev.y);
      if day > 0 then
        selected_day := day;
    end;
  end
  else if ev.kind = PointerUp then
  begin
    if pressed_nav <> NavNone then
    begin
      if nav_hit(ev.x, ev.y) = pressed_nav then
        activate_nav(pressed_nav);
      pressed_nav := NavNone;
    end;
  end;
end;

procedure handle_key();
var
  key: int;
  text: byte;
begin
  key := stdlib.ui_key_code(ev.code);
  text := stdlib.ui_key_text(ev.code);
  if stdlib.ui_key_is_left(ev.code) then
    prev_month()
  else if stdlib.ui_key_is_right(ev.code) then
    next_month()
  else if stdlib.ui_key_is_up(ev.code) then
    prev_year()
  else if stdlib.ui_key_is_down(ev.code) then
    next_year()
  else if stdlib.ui_key_is_home(ev.code) then
    go_today()
  else if text = byte(72) or text = byte(104) then
    go_today();
end;

// ---- modes --------------------------------------------------------------

procedure check_logic();
begin
  verify("bcd 26", rtc_bcd(uint(0x26)) = 26);
  verify("bcd 59", rtc_bcd(uint(0x59)) = 59);
  verify("bcd 20", rtc_bcd(uint(0x20)) = 20);
  verify("valid fixed", valid_date(2026, 11, 26));
  verify("invalid feb day", not valid_date(2026, 2, 30));
  verify("invalid month", not valid_date(2026, 13, 1));

  verify("leap 2024", is_leap(2024));
  verify("leap 2000", is_leap(2000));
  verify("no leap 1900", not is_leap(1900));
  verify("no leap 2025", not is_leap(2025));
  verify("feb 2024", month_days(2024, 2) = 29);
  verify("feb 2025", month_days(2025, 2) = 28);
  verify("april 30", month_days(2026, 4) = 30);
  verify("january 31", month_days(2026, 1) = 31);

  verify("1970 thursday", weekday_sun(1970, 1, 1) = 4);
  verify("2000 saturday", weekday_sun(2000, 1, 1) = 6);
  verify("2026-11-26 thursday", weekday_sun(2026, 11, 26) = 4);

  view_year := 2026;
  view_month := 11;
  verify("november lead", lead_days() = 6);
  verify("november first cell", day_for_cell(0, 6) = 1);
  verify("november 26 cell", day_for_cell(4, 3) = 26);
  verify("26 row", cell_row(26) = 4);
  verify("26 column", cell_col(26) = 3);
  verify("blank cell", day_for_cell(0, 0) = 0);
  verify("past month", day_for_cell(5, 6) = 0);

  view_year := 2026;
  view_month := 12;
  selected_day := 15;
  next_month();
  verify("december wraps", view_year = 2027 and view_month = 1 and selected_day = 0);
  prev_month();
  verify("january back", view_year = 2026 and view_month = 12);
  prev_month();
  verify("november back", view_month = 11);
  next_year();
  verify("next year", view_year = 2027);
  prev_year();
  verify("prev year", view_year = 2026);

  view_year := MinYear;
  view_month := 1;
  prev_month();
  verify("min clamp", view_year = MinYear and view_month = 1);
  view_year := MaxYear;
  view_month := 12;
  next_month();
  verify("max clamp", view_year = MaxYear and view_month = 12);
  prev_year();
  verify("prev max year", view_year = MaxYear - 1);
  next_year();
  verify("next max year", view_year = MaxYear);
end;

procedure check_mode();
var
  px: int;
  py: int;
  row: int;
  col: int;
  saved_month: int;
begin
  check_logic();

  today_known := true;
  today_year := FixedYear;
  today_month := FixedMonth;
  today_day := FixedDay;
  view_year := FixedYear;
  view_month := FixedMonth;
  selected_day := 0;
  pressed_nav := NavNone;
  draw_calendar();

  verify("bezel outline", syslib.get_pixel(rect.x + Inset, rect.y + Inset) = black);
  verify("content base", syslib.get_pixel(rect.x + 1, rect.y + 1) = white);
  verify("prev frame", syslib.get_pixel(rect.x + NavLeftX, rect.y + HeaderY) = black);
  verify("next frame",
         syslib.get_pixel(rect.x + NavRightX + NavW - 1, rect.y + HeaderY) = black);

  row := cell_row(FixedDay);
  col := cell_col(FixedDay);
  verify("today fill",
         syslib.get_pixel(rect.x + GridX + col * CellW + 3,
                          rect.y + GridY + row * CellH + 3) = black);
  verify("blank day base",
         syslib.get_pixel(rect.x + GridX + 3, rect.y + GridY + 3) = white);

  verify("nav prev hit",
         nav_hit(NavLeftX + 5 + rect.x, HeaderY + 5 + rect.y) = NavPrev);
  verify("nav next hit",
         nav_hit(NavRightX + 5 + rect.x, HeaderY + 5 + rect.y) = NavNext);
  verify("nav miss", nav_hit(rect.x + 100, rect.y + 100) = NavNone);
  px := rect.x + GridX + col * CellW + 16;
  py := rect.y + GridY + row * CellH + 12;
  verify("day hit", day_at_point(px, py) = FixedDay);
  verify("grid miss", day_at_point(rect.x + 2, rect.y + 2) = 0);

  saved_month := view_month;
  ev.kind := KeyDown;
  ev.code := uint(0x110 * 65536);
  handle_key();
  verify("left key", view_month = saved_month - 1 and selected_day = 0);
  view_month := saved_month;
  selected_day := 0;

  ev.kind := PointerDown;
  ev.x := rect.x + NavRightX + 5;
  ev.y := rect.y + HeaderY + 5;
  handle_pointer();
  verify("next pressed", pressed_nav = NavNext);
  ev.kind := PointerUp;
  handle_pointer();
  verify("next released", view_month = saved_month + 1 and pressed_nav = NavNone);
  view_month := saved_month;
  selected_day := 0;

  ignored := syslib.window_close(id);
  if failed then
    out.say("calendar: failed\n")
  else
    out.say("calendar: ok\n");
end;

procedure say_ready();
begin
  out.say("calendar: ready ");
  if today_known then
  begin
    written := core.format_int_into(year_text, today_year, uint(10), uint(0));
    out.say(year_text);
    out.say("-");
    written := core.format_int_into(day_text, today_month, uint(10), uint(2));
    out.say(day_text);
    out.say("-");
    written := core.format_int_into(day_text, today_day, uint(10), uint(2));
    out.say(day_text);
  end
  else
    out.say("(no rtc)");
  out.say("\n");
end;

procedure shot_mode();
begin
  if today_known then
    go_today()
  else
  begin
    view_year := FixedYear;
    view_month := FixedMonth;
    selected_day := FixedDay;
  end;
  pressed_nav := NavNone;
  draw_calendar();
  say_ready();
  status := core.sleep(uint(HoldMs));
  ignored := syslib.window_close(id);
  out.say("calendar: done\n");
end;

// Interactive: draw once, then block for one event at a time. Only this
// window's own events are handled; a close request ends the loop so the task
// closes its window and exits (spec 0011, W23).
procedure live_mode();
begin
  running := true;
  pressed_nav := NavNone;
  draw_calendar();
  say_ready();
  while running do
  begin
    status := syslib.next_event(ev);
    if status = Ok then
    begin
      if ev.kind = Expose then
      begin
        if ev.window = id then
        begin
          refresh_geometry();
          draw_calendar();
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
        else if ev.kind = PointerDown or ev.kind = PointerUp then
          handle_pointer();
        draw_calendar();
      end;
    end;
  end;
  ignored := syslib.window_close(id);
  out.say("calendar: closed\n");
end;

// The disabled-screen path: the date math, navigation and key dispatch are
// pure and must agree on every host.
procedure host_path();
begin
  check_logic();
  if failed then
    out.say("calendar: host failed\n")
  else
    out.say("calendar: host ok\n");
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  step := 0;
  init_names();
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  accent := stdlib.ui_accent();
  today_known := false;
  view_year := MinYear;
  view_month := 1;
  selected_day := 0;

  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();
  if screen_w = uint(0) then
    host_path()
  else if screen_w < uint(WindowX + ContentW + 40)
          or screen_h < uint(WindowY + ContentH + 60) then
    out.say("calendar: screen too small\n")
  else
  begin
    sync_today();
    status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Calendar", id);
    if status <> Ok then
      out.say("calendar: no window\n")
    else
    begin
      refresh_geometry();
      if status <> Ok then
        out.say("calendar: no content rect\n")
      else if stdlib.starts_with(args, "check") then
        check_mode()
      else if stdlib.starts_with(args, "shot") then
        shot_mode()
      else
        live_mode();
    end;
  end;
end;

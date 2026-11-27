unit desktop_shell;

// M2.3.4 desktop shell (spec 0012): a System 7.5-style menu bar, one desktop
// window, application launching and a read-only window list.
//
//   run desktop_shell          // normal: draw, print `ready`, pump events
//   run desktop_shell check    // deterministic self-check, print `ok`
//
// M2.5.6 makes this the boot entry: the image stages this source under the
// file name `entrypoint` (the unit stays `desktop_shell` for diagnostics), so
// booting the ISO lands on this menu bar. With no graphics the app spawns the
// text `shell` instead, keeping a serial REPL on a machine that cannot draw.
//
// The menu bar has direct items File/Windows/Terminal and two pull-downs,
// `Apps` (M8.3) and `System` (M2.6.4, spec 0017):
//
//   File      enumerate the live windows into the terminal window
//   Apps      pull-down: Calculator, Clock, Widgets, Image Viewer,
//             Minesweeper, Text Editor, File Browser, Calendar, Terminal,
//             Settings; each is a non-blocking core.spawn
//   Windows   raise the terminal window (the existing "bring forward" item)
//   Terminal  launch `shell` non-blocking; a later click raises the existing
//             Terminal window instead of spawning a second shell
//   Apple     raise the shell's own window, reopening it if it was closed
//   System    pull-down: About igis-v2 (Ctrl+A), Restart (Ctrl+R), Halt (Ctrl+H)
//
// Both pull-downs are drawn with the M2.4.3 menu model
// (`stdlib.ui_menu_dropdown`/`ui_menu_hit`/`ui_menu_move`/`ui_menu_key`) and
// the M2.4.4 dialog helpers (`ui_message`/`ui_dialog_button`). The menu bar and
// every open pull-down are **planes** (spec 0026, C11/C12): screen rectangles
// above all windows with retained canvases. Opening a menu creates its plane
// and takes the machine-wide modal popup input grab (spec 0011, W26; ADR
// 0044), so every queued pointer and key event goes to this task; destroying
// the plane on close recomposites what it covered. The About panel is an
// ordinary desktop-owned window.
//
// The shell draws its own window content and presents it with
// `syslib.window_present`; an `Expose` for its id repaints that content. Window
// lifetime, z-order and chrome belong to the kernel service. The persistent
// bar plane is above every window, and `work_area_reserve` keeps window frames
// below it (spec 0026, C13).
//
// The window list uses `syslib.window_info` (builtin 40), which returns the
// topmost window first. `check` mode injects one pointer-down per item and
// pumps the event, so the whole surface is deterministic without a pointer
// device. `check` assumes the caller already holds the one terminal (it runs
// from a shell), so its Terminal item raises that window rather than spawning.
//
// The desktop shell does not touch the kernel's terminal or system-log pixels:
// no `syslib.clear`, and every text line goes to the terminal window through
// `core.write`. It never attaches the terminal itself: the launched `shell`
// does (spec 0014, the one-terminal rule).

uses core, stdlib, syslib, lib.out, lib.inject;

const
  MenuHeight = 22;
  SysMenuWidth = 224;
  SysMenuCount = 4;
  AppMenuWidth = 220;
  AppMenuCount = 14;
  WindowMenuWidth = 220;
  WindowListCapacity = 16;
  AboutContentW = 420;
  AboutContentH = 200;
  SecondsPerDay = 86400;

  // CMOS RTC ports and registers for the menu-bar clock.
  RtcIndex = 0x70;
  RtcData = 0x71;
  RtcStatusA = 0x0A;
  RtcStatusB = 0x0B;
  RtcSeconds = 0x00;
  RtcMinutes = 0x02;
  RtcHours = 0x04;

var
  white: uint;
  black: uint;
  screen_w: int;
  screen_h: int;
  status: Status;
  ev: Event;
  diag: Diagnostic;

  // The shell's own window.
  own: int;
  rect: Rect;

  // The last enumeration (`list_windows`) and the raise action.
  win_id: int;
  win_rect: Rect;
  win_title: string[64];
  count: int;
  saw_terminal: bool;
  saw_log: bool;
  saw_desktop: bool;
  raised_id: int;

  // Launch bookkeeping.
  launch_status: Status;
  last_task: int;
  launches: int;

  // The desktop-launched shell (M2.5.6). `shell_launched` is set on the first
  // successful spawn; later Terminal clicks raise the live window instead.
  shell_launched: bool;
  shell_task: int;
  // Check mode must not leave a real emulator running; the direct Terminal
  // item still goes through the same handler but skips the spawn.
  probe_spawn: bool;
  // Check mode opens a plain window titled `Terminal` so the Windows raise
  // path is testable even when the check runs from a serial shell.
  probe_term: int;
  running: bool;

  // Menu bookkeeping.
  handled: bool;
  file_runs: int;
  app_runs: int;
  window_runs: int;
  terminal_runs: int;
  // The persistent menu-bar plane (spec 0026, C11): created after startup and
  // recreated at the new geometry after a mode change. `bar` is 0 when
  // creation failed. `menu_plane` is the one open pull-down plane.
  bar_status: Status;
  area_status: Status;
  bar: int;
  menu_plane: int;
  halt_plane: int;
  // Proportional menu-bar layout, derived from the measured label widths
  // (spec 0015) so the hit ranges and the drawn labels cannot drift.
  menu_file_x: int;
  menu_apps_x: int;
  menu_windows_x: int;
  menu_system_x: int;
  menu_system_end: int;

  // The Apps pull-down (M8.3, spec 0012 D20). Items launch an application with a
  // non-blocking `core.spawn`; the app owns the open state and the highlight.
  app_items: array[string[64], 16];
  app_keys: array[string[32], 16];
  app_open: bool;
  app_selected: int;
  apps_opened: int;
  apps_closed: int;
  grab_status: Status;

  // The Windows pull-down (M11): one row per live window in topmost-first
  // order; a minimized window is marked with a leading `*`. Selecting a row
  // raises/restores it.
  window_items: array[string[64], 16];
  window_keys: array[string[32], 16];
  window_menu_ids: array[int, 16];
  window_count: int;
  windows_open: bool;
  windows_selected: int;
  windows_runs: int;
  menu_label: string[64];

  // The System pull-down (M2.6.4, spec 0017). The app owns the open state, the
  // highlight and the item tables; the helpers draw and hit-test only.
  sys_items: array[string[64], 16];
  sys_keys: array[string[32], 16];
  sys_disabled: uint;
  sys_checked: uint;
  system_open: bool;
  system_selected: int;
  system_runs: int;

  // The `File` window list (M11): an ordinary desktop-owned read-only window.
  winlist_open: bool;
  winlist_id: int;
  winlist_rect: Rect;
  win_lines: array[string[256], WindowListCapacity];
  win_ids: array[int, WindowListCapacity];
  win_line_count: int;
  win_line: string[256];

  // The About window (M2.6.4): an ordinary desktop-owned window whose content
  // is the alert panel. `about_runs` counts activations for the check.
  about_open: bool;
  about_id: int;
  about_rect: Rect;
  about_runs: int;

  // The Restart/Halt actions (check mode never runs them).
  restart_runs: int;
  halt_runs: int;
  action: int;
  hit_row: int;

  // Check-mode bookkeeping.
  failed: bool;
  step: int;
  fail_step: int;
  fail_label: string[64];
  reason: ExitReason;
  wait_status: Status;

  // Scratch.
  digits: string[16];
  split: int;
  clk_h: string[2];
  clk_m: string[2];
  used: int;
  flushed: uint;
  saw_click: bool;
  more_events: bool;
  count_events: int;
  now: uint;
  minutes: uint;
  hours: uint;
  rtc_base: int;
  mono_base: int;
  synced: bool;
  focus: int;
  pos: int;
  xx: int;
  yy: int;
  ink_found: bool;

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

// The measured width of one label in the proportional font.
function tw(text: const string[0]): int;
begin
  return int(syslib.text_width_prop(text));
end;

// The menu bar is a persistent plane (spec 0026, C11): retained pixels above
// every window, created after startup and recreated at the new geometry after
// a mode change. The work area keeps window frames below it (C13), so a title
// bar can never be dragged under the bar. A failed creation is reported and
// the shell keeps running; `draw_menu_bar` then has nothing to present.
procedure create_bar_plane();
begin
  bar := 0;
  bar_status := syslib.plane_create(0, 0, screen_w, MenuHeight, bar);
  if bar_status <> Ok then
    out.say("desktop_shell: menu bar plane failed\n");
end;

procedure setup_work_area();
begin
  area_status := syslib.work_area_reserve(MenuHeight);
  create_bar_plane();
end;

// HH:MM from the monotonic clock, right-aligned in the menu bar. The width is
// the proportional measurement (spec 0015), so the digits and the colon sit
// tight to the right edge instead of assuming 8-pixel cells.
procedure rtc_select(register: uint);
begin
  syslib.port_out8(uint(RtcIndex), register);
end;

function rtc_read(register: uint): uint;
begin
  rtc_select(register);
  return syslib.port_in8(uint(RtcData));
end;

function rtc_bcd(value: uint): int;
begin
  return int((value div uint(16)) * uint(10) + (value mod uint(16)));
end;

// Reads the CMOS RTC as seconds since midnight (status-B binary/24-hour
// aware), waiting out the update window and proving the fields are stable.
function rtc_read_time(total: var int): bool;
var
  status_b: uint;
  binary: bool;
  hour24: bool;
  pm: bool;
  rtc_sec: uint;
  rtc_min: uint;
  rtc_hour: uint;
  rtc_again: uint;
  rtc_guard: uint;
  rtc_tries: int;
  hour: int;
  minute: int;
  second: int;
begin
  total := 0;
  status_b := rtc_read(uint(RtcStatusB));
  binary := (status_b div uint(4)) mod uint(2) = uint(1);
  hour24 := (status_b div uint(2)) mod uint(2) = uint(1);
  rtc_tries := 0;
  while rtc_tries < 16 do
  begin
    rtc_guard := uint(0);
    while rtc_read(uint(RtcStatusA)) >= uint(0x80) and rtc_guard < uint(1000000) do
      rtc_guard := rtc_guard + uint(1);
    rtc_sec := rtc_read(uint(RtcSeconds));
    rtc_min := rtc_read(uint(RtcMinutes));
    rtc_hour := rtc_read(uint(RtcHours));
    rtc_again := rtc_read(uint(RtcSeconds));
    if rtc_sec = rtc_again then
    begin
      pm := false;
      if not hour24 then
      begin
        pm := (rtc_hour div uint(0x80)) mod uint(2) = uint(1);
        rtc_hour := rtc_hour mod uint(0x80);
      end;
      if binary then
      begin
        hour := int(rtc_hour);
        minute := int(rtc_min);
        second := int(rtc_sec);
      end
      else
      begin
        hour := rtc_bcd(rtc_hour);
        minute := rtc_bcd(rtc_min);
        second := rtc_bcd(rtc_sec);
      end;
      if not hour24 then
      begin
        if pm and hour < 12 then
          hour := hour + 12;
        if not pm and hour = 12 then
          hour := 0;
      end;
      if hour >= 0 and hour < 24 and minute >= 0 and minute < 60
         and second >= 0 and second < 60 then
      begin
        total := hour * 3600 + minute * 60 + second;
        return true;
      end;
      return false;
    end;
    rtc_tries := rtc_tries + 1;
  end;
  return false;
end;

procedure sync_clock();
var
  total: int;
begin
  synced := false;
  if rtc_read_time(total) then
  begin
    rtc_base := total;
    mono_base := int(core.clock() div uint(1000000000));
    synced := true;
  end;
end;

function clock_seconds(): int;
var
  seconds: int;
begin
  seconds := int(core.clock() div uint(1000000000));
  if synced then
    return (rtc_base + (seconds - mono_base)) mod SecondsPerDay;
  return seconds mod SecondsPerDay;
end;

procedure draw_clock(right: int);
var
  clock_x: int;
  clock_w: int;
begin
  now := uint(clock_seconds());
  minutes := (now div uint(60)) mod uint(60);
  hours := (now div uint(3600)) mod uint(24);
  used := core.format_int_into(clk_h, int(hours), uint(10), uint(2));
  used := core.format_int_into(clk_m, int(minutes), uint(10), uint(2));
  clock_w := tw(clk_h) + tw(":") + tw(clk_m);
  clock_x := right - clock_w;
  if clock_x > 200 then
  begin
    status := syslib.draw_text_prop(clock_x, 7, clk_h, black);
    clock_x := clock_x + tw(clk_h);
    status := syslib.draw_text_prop(clock_x, 7, ":", black);
    clock_x := clock_x + tw(":");
    status := syslib.draw_text_prop(clock_x, 7, clk_m, black);
  end;
end;

// Left edges of the direct menu items: the Apple square, then File/Apps/
// Windows/Terminal with a 14-pixel gap. Rebuilt on every bar draw so a font
// change moves the labels and the hit ranges together.
procedure layout_menu();
begin
  menu_file_x := 24;
  menu_apps_x := menu_file_x + tw("File") + 14;
  menu_windows_x := menu_apps_x + tw("Apps") + 14;
  menu_system_x := menu_windows_x + tw("Windows") + 14;
  menu_system_end := menu_system_x + tw("System");
end;

// The menu bar is a plane, not a window. Labels and the clock use the
// proportional font (spec 0015): positions come from the measured widths, not
// from 8-pixel cells. A runtime mode change (spec 0022) reflows the windows in
// the service and marks them for an `Expose`; the shell also has to rebuild
// the bar plane at the new screen width and reapply the work area (spec 0026,
// C17). An open pull-down is discarded with its plane: its saved rectangle is
// stale.
procedure refresh_screen();
var
  width: int;
  height: int;
begin
  width := int(syslib.screen_width());
  height := int(syslib.screen_height());
  if width <> screen_w or height <> screen_h then
  begin
    screen_w := width;
    screen_h := height;
    if screen_w > 0 and screen_h > 0 then
    begin
      if menu_plane > 0 then
      begin
        status := syslib.plane_destroy(menu_plane);
        menu_plane := 0;
        app_open := false;
        system_open := false;
        windows_open := false;
        status := syslib.popup_release();
      end;
      if bar > 0 then
        status := syslib.plane_destroy(bar);
      setup_work_area();
    end;
  end;
end;

procedure draw_menu_bar();
begin
  refresh_screen();
  if bar <= 0 then
    return;
  status := syslib.plane_begin(bar);
  status := syslib.fill_rect(0, 0, screen_w, MenuHeight, white);
  // The Apple square is aligned with the proportional cap rows (line top 7 +
  // the glyph box's two-row top bearing), not with the old 8×8 cell.
  status := syslib.fill_rect(8, 9, 8, 8, black);
  layout_menu();
  status := syslib.draw_text_prop(menu_file_x, 7, "File", black);
  if app_open then
  begin
    status := syslib.fill_rect(menu_apps_x - 4, 2, tw("Apps") + 8, MenuHeight - 4, black);
    status := syslib.draw_text_prop(menu_apps_x, 7, "Apps", white);
  end
  else
    status := syslib.draw_text_prop(menu_apps_x, 7, "Apps", black);
  if windows_open then
  begin
    status := syslib.fill_rect(menu_windows_x - 4, 2, tw("Windows") + 8, MenuHeight - 4, black);
    status := syslib.draw_text_prop(menu_windows_x, 7, "Windows", white);
  end
  else
    status := syslib.draw_text_prop(menu_windows_x, 7, "Windows", black);
  if system_open then
  begin
    status := syslib.fill_rect(menu_system_x - 4, 2, tw("System") + 8, MenuHeight - 4, black);
    status := syslib.draw_text_prop(menu_system_x, 7, "System", white);
  end
  else
    status := syslib.draw_text_prop(menu_system_x, 7, "System", black);
  status := syslib.fill_rect(0, MenuHeight - 1, screen_w, 1, black);
  draw_clock(screen_w - 8);
  status := syslib.plane_present(bar);
end;

// Draws the shell's own window content and presents it. The service clips the
// presentation to the window's visible parts, so the shell repaints even when
// another window covers part of it; a move that uncovered it then shows fresh
// content instead of stale pixels (spec 0011, W17/W31).
procedure draw_shell_window();
begin
  if own > 0 and not syslib.window_minimized(own) then
  begin
    status := syslib.window_begin(own);
    status := syslib.window_content_rect(own, rect);
    if status = Ok then
    begin
      // Window-local content (spec 0026 C8): a move changes nothing here; the
      // rectangle is read only for its size.
      status := syslib.fill_rect(0, 0, rect.w, rect.h, white);
      status := syslib.draw_text(12, 12, "igis-v2", black);
      status := syslib.draw_text(12, 28, "M2.3.4 desktop shell", black);
      status := syslib.draw_text(12, 52, "File: list windows", black);
      status := syslib.draw_text(12, 68, "Apps: application menu", black);
      status := syslib.draw_text(12, 84, "Windows: raise terminal", black);
      status := syslib.draw_text(12, 100, "Terminal: launch shell", black);
      status := syslib.draw_text(12, 116, "focus:", black);
      focus := syslib.window_focus();
      used := core.format_int_into(digits, focus, uint(10), uint(0));
      status := syslib.draw_text(68, 116, digits, black);
      // A small black mark in the content, used by the check-mode pixel test
      // and visible as the shell's own content marker.
      status := syslib.fill_rect(rect.w - 8, rect.h - 8, 8, 8, black);
    end;
    status := syslib.window_present(own);
  end;
end;

// Opens (or raises) the shell's own `Desktop` window. The window is the
// shell's help/status panel, not the shell itself: closing it with its close
// box only closes the window, the desktop keeps running, and the Apple square
// reopens it (M8.7; spec 0012 D12/D19). The shell is the boot desktop, so
// exiting here would leave no UI and halt the idle system.
procedure open_desktop_window();
begin
  if own > 0 then
  begin
    status := syslib.window_raise(own);
    draw_shell_window();
    return;
  end;
  status := syslib.window_open(600, 72, 360, 164, "Desktop", own);
  if status = Ok then
  begin
    out.say("desktop_shell: desktop window open\n");
    draw_shell_window();
  end
  else
    out.say("desktop_shell: desktop window open failed\n");
end;

// A close-box request for the shell's own window closes only that window; the
// menu bar, the Apps/System menus and every launched application stay live.
procedure close_desktop_window();
begin
  if own > 0 then
  begin
    status := syslib.window_close(own);
    own := 0;
    out.say("desktop_shell: desktop window closed\n");
  end;
end;

// Enumerates the live windows, terminal wins first, and writes the list into
// the terminal window. Records whether the kernel windows and this shell's
// window were found.
procedure list_windows();
begin
  saw_terminal := false;
  saw_log := false;
  saw_desktop := false;
  count := 0;
  pos := 0;
  win_line_count := 0;
  status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  while status = Ok and pos < 32 do
  begin
    win_line := "";
    split := core.format_int_into(digits, win_id, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), " \"");
    if syslib.window_minimized(win_id) then
      status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "*");
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), win_title);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "\" ");
    split := core.format_int_into(digits, win_rect.x, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), ",");
    split := core.format_int_into(digits, win_rect.y, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), " ");
    split := core.format_int_into(digits, win_rect.w, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "x");
    split := core.format_int_into(digits, win_rect.h, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    if win_line_count < WindowListCapacity then
    begin
      win_lines[win_line_count] := win_line;
      win_ids[win_line_count] := win_id;
      win_line_count := win_line_count + 1;
    end;
    if win_title = "Terminal" then
      saw_terminal := true
    else if win_title = "System Log" then
      saw_log := true
    else if win_title = "Desktop" then
      saw_desktop := true;
    count := count + 1;
    pos := pos + 1;
    status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  end;
end;

// Paints the desktop-owned window list in window-local coordinates (spec 0026
// C8); the content rectangle is read only for its size.
procedure draw_window_list();
begin
  if winlist_id <= 0 or syslib.window_minimized(winlist_id) then
    return;
  status := syslib.window_begin(winlist_id);
  status := syslib.window_content_rect(winlist_id, winlist_rect);
  if status <> Ok then
    return;
  status := syslib.fill_rect(0, 0, winlist_rect.w, winlist_rect.h, white);
  pos := 0;
  while pos < win_line_count do
  begin
    status := syslib.draw_text(
      8, 8 + pos * 16, win_lines[pos], black);
    pos := pos + 1;
  end;
  status := syslib.window_present(winlist_id);
end;

// The `File` action: enumerate into the list buffer and open (or raise) the
// desktop-owned "Windows" window. It no longer writes through a kernel
// terminal (spec 0021, M4).
procedure show_window_list();
begin
  list_windows();
  if winlist_id > 0 then
  begin
    status := syslib.window_raise(winlist_id);
    draw_window_list();
    return;
  end;
  status := syslib.window_open(320, 140, 460, 300, "Windows", winlist_id);
  if status = Ok then
  begin
    winlist_open := true;
    out.say("desktop_shell: window list open\n");
    draw_window_list();
  end
  else
  begin
    winlist_id := 0;
    out.say("desktop_shell: window list failed\n");
  end;
end;

procedure close_window_list();
begin
  if not winlist_open then
    return;
  if winlist_id > 0 then
    status := syslib.window_close(winlist_id);
  winlist_id := 0;
  winlist_open := false;
  out.say("desktop_shell: window list closed\n");
  draw_shell_window();
end;

// A click on a Window List row raises that window; a minimized window is
// restored by the same `window_raise` (M11 minimize).
procedure raise_window_at(px: int; py: int);
var
  row: int;
begin
  if winlist_id <= 0 then
    return;
  status := syslib.window_content_rect(winlist_id, winlist_rect);
  if status <> Ok then
    return;
  // A window-addressed pointer event is content-local (spec 0026 C9).
  if px < 0 or px >= winlist_rect.w then
    return;
  row := (py - 8) div 16;
  if row < 0 or row >= win_line_count then
    return;
  status := syslib.window_raise(win_ids[row]);
  if status = Ok then
  begin
    out.say("desktop_shell: raised window ");
    out.say_int(win_ids[row]);
    out.say("\n");
  end;
end;

// Raises the terminal window: the "Windows" direct item's action. Enumeration
// order is topmost first, so find it by title rather than by index.
procedure raise_terminal();
begin
  raised_id := 0;
  pos := 0;
  status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  while status = Ok and pos < 32 and raised_id = 0 do
  begin
    if win_title = "Terminal" then
    begin
      status := syslib.window_raise(win_id);
      if status = Ok then
        raised_id := win_id;
    end;
    pos := pos + 1;
    status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  end;
end;

// ---- the System menu and About panel (M2.6.4, spec 0017) -----------------
//
// Item order (0-based, the order drawn and the order Up/Down traverses):
//
//   0  About igis-v2   Ctrl+A
//   1  -               (separator, never selectable)
//   2  Restart         Ctrl+R
//   3  Halt            Ctrl+H
//
// The labels and shortcuts live in the app's state; the library owns the
// rendering and the state transitions. The pull-down's frame is
// `(menu_system_x - 4, MenuHeight, SysMenuWidth, 4 * 16 + 2)`.

procedure init_system_menu();
begin
  sys_items[0] := "About igis-v2";
  sys_keys[0] := "Ctrl+A";
  sys_items[1] := "-";
  sys_keys[1] := "";
  sys_items[2] := "Restart";
  sys_keys[2] := "Ctrl+R";
  sys_items[3] := "Halt";
  sys_keys[3] := "Ctrl+H";
  sys_disabled := uint(0);
  sys_checked := uint(0);
  system_open := false;
  system_selected := -1;
  system_runs := 0;
end;

procedure draw_system_menu();
begin
  if menu_plane <= 0 then
    return;
  status := syslib.plane_begin(menu_plane);
  status := stdlib.ui_menu_dropdown(
    menu_system_x - 4, MenuHeight, SysMenuWidth,
    sys_items, sys_keys, SysMenuCount, system_selected, sys_checked, sys_disabled, true);
  status := syslib.plane_present(menu_plane);
end;

procedure open_system_menu();
begin
  if app_open then
    close_apps_menu();
  if windows_open then
    close_windows_menu();
  menu_plane := 0;
  status := syslib.plane_create(
    menu_system_x - 4, MenuHeight, SysMenuWidth, SysMenuCount * 16 + 2, menu_plane);
  if status <> Ok then
  begin
    out.say("desktop_shell: system menu plane failed\n");
    return;
  end;
  system_open := true;
  system_selected := stdlib.ui_menu_first(SysMenuCount, sys_items, sys_disabled);
  system_runs := system_runs + 1;
  grab_status := syslib.popup_grab(
    menu_system_x - 4, MenuHeight, SysMenuWidth, SysMenuCount * 16 + 2);
  draw_menu_bar();
  draw_system_menu();
end;

// ---- the Windows pull-down (M11) ----------------------------------------
//
// One row per live window in `window_info` order (topmost first). A minimized
// window is listed at the end and prefixed with `*`; activating any row raises
// it, which restores a minimized window (spec 0011, W30).

procedure build_windows_menu();
begin
  window_count := 0;
  pos := 0;
  status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  while status = Ok and pos < 32 and window_count < 16 do
  begin
    window_menu_ids[window_count] := win_id;
    window_items[window_count] := win_title;
    if syslib.window_minimized(win_id) then
      window_keys[window_count] := "min"
    else
      window_keys[window_count] := "";
    window_count := window_count + 1;
    pos := pos + 1;
    status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  end;
end;

procedure draw_windows_menu();
begin
  if menu_plane <= 0 or window_count <= 0 then
    return;
  status := syslib.plane_begin(menu_plane);
  status := stdlib.ui_menu_dropdown(
    menu_windows_x - 4, MenuHeight, WindowMenuWidth,
    window_items, window_keys, window_count, windows_selected, uint(0), uint(0), true);
  status := syslib.plane_present(menu_plane);
end;

procedure open_windows_menu();
begin
  if app_open then
    close_apps_menu();
  if system_open then
    close_system_menu();
  build_windows_menu();
  if window_count <= 0 then
    return;
  menu_plane := 0;
  status := syslib.plane_create(
    menu_windows_x - 4, MenuHeight, WindowMenuWidth, window_count * 16 + 2, menu_plane);
  if status <> Ok then
  begin
    out.say("desktop_shell: windows menu plane failed\n");
    return;
  end;
  windows_open := true;
  windows_selected := stdlib.ui_menu_first(window_count, window_items, uint(0));
  windows_runs := windows_runs + 1;
  out.say("desktop_shell: windows menu open\n");
  grab_status := syslib.popup_grab(
    menu_windows_x - 4, MenuHeight, WindowMenuWidth, window_count * 16 + 2);
  draw_menu_bar();
  draw_windows_menu();
end;

procedure close_windows_menu();
begin
  if not windows_open then
    return;
  windows_open := false;
  windows_selected := -1;
  out.say("desktop_shell: windows menu closed\n");
  status := syslib.popup_release();
  if menu_plane > 0 then
  begin
    status := syslib.plane_destroy(menu_plane);
    menu_plane := 0;
  end;
  draw_menu_bar();
  draw_shell_window();
end;

procedure update_windows_highlight();
begin
  hit_row := stdlib.ui_menu_hit(
    menu_windows_x - 4, MenuHeight, WindowMenuWidth,
    window_items, window_count, ev.x, ev.y);
  if hit_row >= 0 and hit_row <> windows_selected then
  begin
    windows_selected := hit_row;
    draw_windows_menu();
  end;
end;

procedure activate_window_item();
var
  target: int;
begin
  if windows_selected < 0 or windows_selected >= window_count then
    return;
  target := window_menu_ids[windows_selected];
  close_windows_menu();
  status := syslib.window_raise(target);
  if status = Ok then
  begin
    raised_id := target;
    out.say("desktop_shell: raised window ");
    out.say_int(target);
    out.say("\n");
  end;
end;

procedure handle_windows_key(code: uint);
var
  key: int;
begin
  key := stdlib.ui_key_code(code);
  if key = 0x128 then
  begin
    close_windows_menu();
    open_apps_menu();
    return;
  end;
  if key = 0x129 then
  begin
    close_windows_menu();
    open_system_menu();
    return;
  end;
  action := stdlib.ui_menu_key(code, windows_open);
  if action = 6 then
    close_windows_menu()
  else if action = 1 then
  begin
    windows_selected := stdlib.ui_menu_move(
      windows_selected, window_count, -1, window_items, uint(0));
    draw_windows_menu();
  end
  else if action = 2 then
  begin
    windows_selected := stdlib.ui_menu_move(
      windows_selected, window_count, 1, window_items, uint(0));
    draw_windows_menu();
  end
  else if action = 5 then
    activate_window_item()
  else if action = 3 then
  begin
    close_windows_menu();
    open_apps_menu();
  end
  else if action = 4 then
  begin
    close_windows_menu();
    open_system_menu();
  end;
end;

procedure handle_windows_menu_click();
begin
  if ev.y < MenuHeight then
    handle_menu(ev.x, ev.y)
  else
  begin
    hit_row := stdlib.ui_menu_hit(
      menu_windows_x - 4, MenuHeight, WindowMenuWidth,
      window_items, window_count, ev.x, ev.y);
    if hit_row >= 0 then
    begin
      windows_selected := hit_row;
      activate_window_item();
    end
    else
      close_windows_menu();
  end;
end;

// ---- the Apps pull-down (M8.3/M11, spec 0012 D20) -----------------------
//
// Item order (0-based; `#` rows are section headers and never selectable):
//
//   0  #Accessories
//   1  Calculator       apps/accessories/calculator
//   2  Clock            apps/accessories/clock_window
//   3  Widgets          apps/accessories/widgets
//   4  Image Viewer     apps/accessories/image_viewer
//   5  #Games
//   6  Minesweeper      apps/games/minesweeper
//   7  #Productivity
//   8  Text Editor      apps/productivity/editor
//   9  File Browser     apps/productivity/browser
//  10  Calendar         apps/productivity/calendar
//  11  #Utilities
//  12  Terminal         apps/utilities/terminal (always a new instance)
//  13  Settings         apps/utilities/settings
//
// Every item is a non-blocking `core.spawn`; the shell never waits. The menu
// frame is `(menu_apps_x - 4, MenuHeight, AppMenuWidth, 14 * 16 + 2)`.

procedure init_apps_menu();
begin
  app_items[0] := "#Accessories";
  app_items[1] := "Calculator";
  app_items[2] := "Clock";
  app_items[3] := "Widgets";
  app_items[4] := "Image Viewer";
  app_items[5] := "#Games";
  app_items[6] := "Minesweeper";
  app_items[7] := "#Productivity";
  app_items[8] := "Text Editor";
  app_items[9] := "File Browser";
  app_items[10] := "Calendar";
  app_items[11] := "#Utilities";
  app_items[12] := "Terminal";
  app_items[13] := "Settings";
  pos := 0;
  while pos < AppMenuCount do
  begin
    app_keys[pos] := "";
    pos := pos + 1;
  end;
  app_open := false;
  app_selected := -1;
  app_runs := 0;
  apps_opened := 0;
  apps_closed := 0;
end;

procedure draw_apps_menu();
begin
  if menu_plane <= 0 then
    return;
  status := syslib.plane_begin(menu_plane);
  status := stdlib.ui_menu_dropdown(
    menu_apps_x - 4, MenuHeight, AppMenuWidth,
    app_items, app_keys, AppMenuCount, app_selected, uint(0), uint(0), true);
  status := syslib.plane_present(menu_plane);
end;

procedure open_apps_menu();
begin
  if system_open then
    close_system_menu();
  if windows_open then
    close_windows_menu();
  menu_plane := 0;
  status := syslib.plane_create(
    menu_apps_x - 4, MenuHeight, AppMenuWidth, AppMenuCount * 16 + 2, menu_plane);
  if status <> Ok then
  begin
    out.say("desktop_shell: apps menu plane failed\n");
    return;
  end;
  app_open := true;
  app_selected := stdlib.ui_menu_first(AppMenuCount, app_items, uint(0));
  apps_opened := apps_opened + 1;
  // The modal grab makes this task the only recipient of queued input while
  // the menu plane is open (spec 0011, W26); the plane also owns a press inside
  // its rectangle (spec 0026, C12).
  grab_status := syslib.popup_grab(
    menu_apps_x - 4, MenuHeight, AppMenuWidth, AppMenuCount * 16 + 2);
  out.say("desktop_shell: apps menu open\n");
  draw_menu_bar();
  draw_apps_menu();
end;

procedure close_apps_menu();
begin
  if not app_open then
    return;
  app_open := false;
  app_selected := -1;
  status := syslib.popup_release();
  // Destroying the plane recomposites everything it covered (spec 0026, C11):
  // no repair_menu_area and no client round trip.
  if menu_plane > 0 then
  begin
    status := syslib.plane_destroy(menu_plane);
    menu_plane := 0;
  end;
  draw_menu_bar();
  draw_shell_window();
  apps_closed := apps_closed + 1;
  out.say("desktop_shell: apps menu closed\n");
end;

procedure close_system_menu();
begin
  if not system_open then
    return;
  system_open := false;
  system_selected := -1;
  status := syslib.popup_release();
  if menu_plane > 0 then
  begin
    status := syslib.plane_destroy(menu_plane);
    menu_plane := 0;
  end;
  draw_menu_bar();
  draw_shell_window();
end;

procedure update_menu_highlight();
begin
  hit_row := stdlib.ui_menu_hit(
    menu_system_x - 4, MenuHeight, SysMenuWidth,
    sys_items, SysMenuCount, ev.x, ev.y);
  if hit_row >= 0
     and stdlib.ui_menu_selectable(sys_items[hit_row], sys_disabled, hit_row)
     and hit_row <> system_selected then
  begin
    system_selected := hit_row;
    draw_system_menu();
  end;
end;

procedure update_apps_highlight();
begin
  hit_row := stdlib.ui_menu_hit(
    menu_apps_x - 4, MenuHeight, AppMenuWidth,
    app_items, AppMenuCount, ev.x, ev.y);
  if hit_row >= 0 and hit_row <> app_selected then
  begin
    app_selected := hit_row;
    draw_apps_menu();
  end;
end;

// Non-blocking launch of one application-choice item. The label is the menu's
// own text; the path is the image name resolved under `base/`.
procedure launch_app(name: const string[0]; label: const string[0]);
begin
  // Check mode launches the deterministic `check` path so the child exits and
  // `core.wait` can reap it; a normal desktop launch has no arguments and the
  // application stays live (Clock, for example, is a live window now).
  if probe_spawn then
    launch_status := core.spawn(name, "check", diag, last_task)
  else
    launch_status := core.spawn(name, "", diag, last_task);
  launches := launches + 1;
  if launch_status = Ok then
  begin
    win_line := "";
    status := stdlib.str_insert_into(win_line, 0, "desktop_shell: launched ");
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), label);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), " task ");
    split := core.format_int_into(digits, last_task, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "\n");
    out.say(win_line);
  end
  else
  begin
    win_line := "";
    status := stdlib.str_insert_into(win_line, 0, "desktop_shell: launch failed: ");
    status := stdlib.str_insert_into(
      win_line, int(core.len(win_line)), diag.message);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "\n");
    out.say(win_line);
  end;
end;

procedure activate_app_item();
begin
  action := app_selected;
  if action < 0 or action >= AppMenuCount then
    return;
  if not stdlib.ui_menu_selectable(app_items[action], uint(0), action) then
    return;
  app_runs := app_runs + 1;
  close_apps_menu();
  if action = 1 then
    launch_app("apps/accessories/calculator", "Calculator")
  else if action = 2 then
    launch_app("apps/accessories/clock_window", "Clock")
  else if action = 3 then
    launch_app("apps/accessories/widgets", "Widgets")
  else if action = 4 then
    launch_app("apps/accessories/image_viewer", "Image Viewer")
  else if action = 6 then
    launch_app("apps/games/minesweeper", "Minesweeper")
  else if action = 8 then
    launch_app("apps/productivity/editor", "Text Editor")
  else if action = 9 then
    launch_app("apps/productivity/browser", "File Browser")
  else if action = 10 then
    launch_app("apps/productivity/calendar", "Calendar")
  else if action = 12 then
  begin
    terminal_runs := terminal_runs + 1;
    launch_shell();
  end
  else if action = 13 then
    launch_app("apps/utilities/settings", "Settings");
end;

procedure handle_apps_key(code: uint);
var
  key: int;
begin
  key := stdlib.ui_key_code(code);
  // F10 switches to System; F9 closes. Left/Right also switch, because the two
  // pull-downs are the only two menus.
  if key = 0x129 then
  begin
    close_apps_menu();
    open_system_menu();
    return;
  end;
  if key = 0x128 then
  begin
    close_apps_menu();
    return;
  end;
  action := stdlib.ui_menu_key(code, app_open);
  if action = 6 then
    close_apps_menu()
  else if action = 1 then
  begin
    app_selected := stdlib.ui_menu_move(
      app_selected, AppMenuCount, -1, app_items, uint(0));
    draw_apps_menu();
  end
  else if action = 2 then
  begin
    app_selected := stdlib.ui_menu_move(
      app_selected, AppMenuCount, 1, app_items, uint(0));
    draw_apps_menu();
  end
  else if action = 5 then
    activate_app_item()
  else if action = 3 or action = 4 then
  begin
    close_apps_menu();
    open_system_menu();
  end;
end;

procedure request_restart();
begin
  out.say("desktop_shell: restart\n");
  status := core.reboot();
  out.say("desktop_shell: reboot returned\n");
end;

// The final screen for a desktop-requested halt (System > Halt / Ctrl+H). It is
// a full-screen plane above every window and the bar (spec 0026, C11): a plane
// canvas is retained, so no later recomposition can paint over it before
// `core.halt` stops the machine.
procedure draw_halt_screen();
var
  message: string[64];
  width: int;
  x: int;
  y: int;
begin
  message := "You may now shut down your computer.";
  halt_plane := 0;
  status := syslib.plane_create(0, 0, screen_w, screen_h, halt_plane);
  if halt_plane <= 0 then
  begin
    out.say("desktop_shell: halt screen plane failed\n");
    return;
  end;
  status := syslib.plane_begin(halt_plane);
  status := syslib.fill_rect(0, 0, screen_w, screen_h, black);
  width := tw(message);
  x := (screen_w - width) div 2;
  y := screen_h div 2;
  status := syslib.draw_text_prop(x, y, message, white);
  status := syslib.plane_present(halt_plane);
end;

procedure request_halt();
begin
  out.say("desktop_shell: halt\n");
  draw_halt_screen();
  status := core.halt();
  out.say("desktop_shell: halt returned\n");
end;

procedure activate_system_item();
begin
  if system_selected = 0 then
  begin
    close_system_menu();
    open_about();
  end
  else if system_selected = 2 then
  begin
    restart_runs := restart_runs + 1;
    close_system_menu();
    request_restart();
  end
  else if system_selected = 3 then
  begin
    halt_runs := halt_runs + 1;
    close_system_menu();
    request_halt();
  end;
end;

procedure handle_system_key(code: uint);
var
  key: int;
begin
  key := stdlib.ui_key_code(code);
  // F9 switches to Apps; F10 closes. Left/Right also switch, because the two
  // pull-downs are the only two menus.
  if key = 0x128 then
  begin
    close_system_menu();
    open_apps_menu();
    return;
  end;
  if key = 0x129 then
  begin
    close_system_menu();
    return;
  end;
  action := stdlib.ui_menu_key(code, system_open);
  if action = 6 then
    close_system_menu()
  else if action = 1 then
  begin
    system_selected := stdlib.ui_menu_move(
      system_selected, SysMenuCount, -1, sys_items, sys_disabled);
    draw_system_menu();
  end
  else if action = 2 then
  begin
    system_selected := stdlib.ui_menu_move(
      system_selected, SysMenuCount, 1, sys_items, sys_disabled);
    draw_system_menu();
  end
  else if action = 5 then
    activate_system_item()
  else if action = 3 or action = 4 then
  begin
    close_system_menu();
    open_apps_menu();
  end;
end;

// Global shortcuts, only while the desktop's own window has focus and no menu
// or About panel is open: F9 opens/toggles Apps, F10 opens/toggles System,
// Ctrl+A the About panel, Ctrl+R Restart and Ctrl+H Halt. Keeping them
// focus-scoped means a key typed into another window (the editor's Ctrl+A, for
// example) never triggers a system action by accident.
procedure handle_shortcut(code: uint);
var
  key: int;
  mods: int;
begin
  key := stdlib.ui_key_code(code);
  mods := stdlib.ui_key_modifiers(code);
  if key = 0x128 then
  begin
    if app_open then close_apps_menu() else open_apps_menu();
  end
  else if key = 0x129 then
  begin
    if system_open then close_system_menu() else open_system_menu();
  end
  else if (mods div 2) mod 2 = 1 then
  begin
    if key = 65 then
      open_about()
    else if key = 82 then
    begin
      restart_runs := restart_runs + 1;
      request_restart();
    end
    else if key = 72 then
    begin
      halt_runs := halt_runs + 1;
      request_halt();
    end;
  end;
end;

// The About panel is a desktop-owned window: the service's own chrome is the
// panel frame, and the content is the alert body. Using a real window means the
// service restores whatever the panel covered through the ordinary damage and
// `Expose` protocol, with no app-managed screen save.
procedure open_about();
var
  aw: int;
  ah: int;
begin
  if about_open then
    return;
  aw := AboutContentW;
  ah := AboutContentH;
  if aw > screen_w - 16 then aw := screen_w - 16;
  if ah > screen_h - MenuHeight - 16 then ah := screen_h - MenuHeight - 16;
  status := syslib.window_open(
    (screen_w - aw) div 2,
    MenuHeight + ((screen_h - MenuHeight - ah) div 2),
    aw, ah, "About igis-v2", about_id);
  if status = Ok then
  begin
    about_runs := about_runs + 1;
    about_open := true;
    out.say("desktop_shell: about open\n");
    draw_about();
  end
  else
  begin
    about_id := 0;
    out.say("desktop_shell: about failed\n");
  end;
end;

procedure draw_about();
var
  row_top: int;
begin
  if about_id = 0 or syslib.window_minimized(about_id) then
    return;
  status := syslib.window_begin(about_id);
  status := syslib.window_content_rect(about_id, about_rect);
  if status = Ok then
  begin
    status := syslib.fill_rect(0, 0, about_rect.w, about_rect.h, white);
    status := syslib.draw_text_prop(14, 10, "igis-v2", black);
    status := stdlib.ui_message(
      14, 32, about_rect.w - 28, about_rect.h - 32 - 44,
      "Interpreted desktop at ring 0.\nWindows, terminal, text and files.\nFonts, menus and a golden frame.\nM2, 2026.\n\nSinan Islekdemir - sinan@islekdemir.com");
    row_top := about_rect.h - 34;
    status := stdlib.ui_dialog_button(
      (about_rect.w - 80) div 2, row_top, 80, 24,
      "OK", true, true);
    status := syslib.window_present(about_id);
  end;
end;

procedure close_about();
begin
  if not about_open then
    return;
  if about_id > 0 then
    status := syslib.window_close(about_id);
  about_id := 0;
  about_open := false;
  out.say("desktop_shell: about closed\n");
  draw_menu_bar();
  draw_shell_window();
end;

procedure handle_about_key(code: uint);
begin
  action := stdlib.ui_dialog_key(code, true);
  if action = 1 or action = 2 then
    close_about();
end;

// A modal event while the About panel is open: an owner expose repaints it, a
// key applies the dialog mapping (Enter is the default, Escape cancels; both
// close), a close request for the panel closes only the panel, and a
// pointer-down routed to it closes it (the OK button, the title bar or any
// click inside).
procedure handle_about_event();
begin
  if ev.kind = Expose then
  begin
    if ev.window = about_id then
      draw_about();
  end
  else if ev.kind = Close then
  begin
    // The close box of the About window asks the owner (this task) to close
    // it. A dialog closes itself; the desktop keeps running (spec 0011, W23).
    if ev.window = about_id then
      close_about();
  end
  else if ev.kind = KeyDown then
    handle_about_key(ev.code)
  else if ev.kind = PointerDown then
  begin
    if ev.window = about_id then
      close_about();
  end;
end;

// A pointer-down while a pull-down is open. With the modal grab the shell is
// the only recipient, so the event is still `window = 0` and the menu's own
// rectangle is handled here by coordinate: a row activates, anything else
// dismisses the menu. A click on the menu bar goes back through `handle_menu`,
// which can switch or close the other menu.
procedure handle_system_menu_click();
begin
  if ev.y < MenuHeight then
    handle_menu(ev.x, ev.y)
  else
  begin
    hit_row := stdlib.ui_menu_hit(
      menu_system_x - 4, MenuHeight, SysMenuWidth,
      sys_items, SysMenuCount, ev.x, ev.y);
    if hit_row >= 0 then
    begin
      if stdlib.ui_menu_selectable(sys_items[hit_row], sys_disabled, hit_row) then
      begin
        system_selected := hit_row;
        activate_system_item();
      end;
    end
    else
      close_system_menu();
  end;
end;

procedure handle_apps_menu_click();
begin
  if ev.y < MenuHeight then
    handle_menu(ev.x, ev.y)
  else
  begin
    hit_row := stdlib.ui_menu_hit(
      menu_apps_x - 4, MenuHeight, AppMenuWidth,
      app_items, AppMenuCount, ev.x, ev.y);
    if hit_row >= 0 then
    begin
      app_selected := hit_row;
      activate_app_item();
    end
    else
      close_apps_menu();
  end;
end;

// Non-blocking terminal launch (M11). Every activation starts a fresh
// emulator; the emulator owns its own session and spawns its shell. There is
// no raise-the-existing-terminal path (spec 0021, T5).
procedure launch_shell();
begin
  if probe_spawn then
  begin
    launch_status := Ok;
    shell_launched := true;
    out.say("desktop_shell: terminal spawn skipped in check\n");
    return;
  end;
  launch_status := core.spawn("terminal", "", diag, shell_task);
  if launch_status = Ok then
  begin
    shell_launched := true;
    win_line := "";
    status := stdlib.str_insert_into(
      win_line, 0, "desktop_shell: launched terminal task ");
    split := core.format_int_into(digits, shell_task, uint(10), uint(0));
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), digits);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "\n");
    out.say(win_line);
  end
  else
  begin
    win_line := "";
    status := stdlib.str_insert_into(
      win_line, 0, "desktop_shell: terminal launch failed: ");
    status := stdlib.str_insert_into(
      win_line, int(core.len(win_line)), diag.message);
    status := stdlib.str_insert_into(win_line, int(core.len(win_line)), "\n");
    out.say(win_line);
  end;
end;

// With no graphics the desktop cannot be the interactive surface. Spawn the
// text shell (the pre-M2.5.6 boot entry) so the machine keeps a serial REPL;
// the desktop then exits and the shell becomes the only task.
procedure serial_shell();
begin
  launch_status := core.spawn("shell", "", diag, shell_task);
  if launch_status = Ok then
  begin
    out.say("desktop_shell: serial shell task ");
    out.say_int(shell_task);
    out.say("\n");
  end
  else
  begin
    out.say("desktop_shell: shell launch failed: ");
    out.say(diag.message);
    out.say("\n");
  end;
end;

// A direct menu item or a pull-down label: the row is the menu bar, the x
// ranges are the item pads. A click on another item dismisses an open menu
// first; the two pull-downs toggle and exclude each other.
procedure handle_menu(x: int; y: int);
begin
  layout_menu();
  handled := false;
  if y >= 0 and y < MenuHeight then
  begin
    if x >= 2 and x < 20 then
    begin
      if system_open then close_system_menu();
      if app_open then close_apps_menu();
      open_desktop_window();
      handled := true;
    end
    else if x >= 20 and x < menu_apps_x - 4 then
    begin
      if system_open then close_system_menu();
      if app_open then close_apps_menu();
      show_window_list();
      file_runs := file_runs + 1;
      handled := true;
    end
    else if x >= menu_apps_x - 4 and x < menu_windows_x - 4 then
    begin
      if app_open then
        close_apps_menu()
      else
        open_apps_menu();
      handled := true;
    end
    else if x >= menu_windows_x - 4 and x < menu_system_x - 4 then
    begin
      if windows_open then
        close_windows_menu()
      else
        open_windows_menu();
      window_runs := window_runs + 1;
      handled := true;
    end
    else if x >= menu_system_x - 4 and x < menu_system_end + 4 then
    begin
      if system_open then
        close_system_menu()
      else
        open_system_menu();
      handled := true;
    end;
  end;
end;

// Check mode calls the menu handlers directly. From M2.5.6 the boot desktop is
// itself a consumer of the one machine-wide event queue, so an injected click
// in a second instance could be taken by the boot desktop instead; the direct
// call keeps the check deterministic wherever it runs. The real pointer path
// (hit-test -> raise/focus -> menu) is proved by the boot harness instead.

procedure check_mode();
begin
  failed := false;
  step := 0;
  fail_step := 0;
  file_runs := 0;
  app_runs := 0;
  window_runs := 0;
  terminal_runs := 0;
  launches := 0;
  raised_id := 0;
  // A real emulator would run forever; the handler is still exercised, but
  // the spawn is skipped in check mode.
  probe_spawn := true;
  shell_launched := true;
  // The bar plane and the work area are machine-wide surfaces created after
  // startup; a second desktop instance may create its own plane (up to 8).
  verify(
    "menu bar plane",
    area_status = Ok and bar_status = Ok and bar > 0);

  draw_menu_bar();
  status := syslib.window_open(600, 72, 360, 164, "Desktop", own);
  verify("open desktop window", status = Ok and own > 0);
  if status = Ok then
  begin
    draw_shell_window();
    // The window open requested a full-stack repaint (W31); the service owns
    // the desktop base and the bar has no window, so the check redraws its own
    // app-drawn bar before the pixel checks, exactly as the normal loop does
    // after every event.
    draw_menu_bar();

    // The menu bar and the service-drawn chrome/content are really in the
    // shadow. The shell window sits below the bar; no kernel window starts
    // above y=26.
    verify("menu bar white", syslib.get_pixel(700, 2) = white);
    verify("menu baseline", syslib.get_pixel(700, 21) = black);
    verify("apple square", syslib.get_pixel(10, 10) = black);

    // Chrome and the composed shadow are read with no draw scope (spec 0026
    // C6): the bar plane's scope would answer from the bar's canvas instead.
    status := syslib.window_begin(0);
    status := syslib.window_frame_rect(own, rect);
    verify(
      "frame geometry",
      status = Ok and rect.x = 600 and rect.y = 72 and rect.w = 362 and rect.h = 181);
    verify("frame border", syslib.get_pixel(rect.x, rect.y) = black);
    verify(
      "content marker",
      syslib.get_pixel(rect.x + rect.w - 4, rect.y + rect.h - 4) = black);

    // A plain Terminal-titled window makes the raise path deterministic in a
    // serial boot too; it is closed before the final screen.
    probe_term := 0;
    status := syslib.window_open(700, 500, 200, 100, "Terminal", probe_term);
    verify("probe terminal window", status = Ok and probe_term > 0);

    // File: enumerate. The kernel terminal and system log must be present,
    // together with this shell's window.
    handle_menu(menu_file_x + 4, 10);
    verify("file item handled", handled and file_runs = 1);
    verify("terminal listed", saw_terminal);
    verify("system log listed", saw_log);
    verify("desktop listed", saw_desktop);
    verify("at least three windows", count >= 3);
    verify(
      "window list open",
      winlist_open and winlist_id > 0 and win_line_count >= 3);

    // ---- Apps pull-down (M8.3/M11, spec 0012 D20) ---------------------
    // Section headers are non-selectable; the selectable rows are the app
    // contract and launch explicit /apps paths.
    verify(
      "apps section headers",
      stdlib.ui_menu_is_header(app_items[0])
        and stdlib.ui_menu_is_header(app_items[5])
        and stdlib.ui_menu_is_header(app_items[7])
        and stdlib.ui_menu_is_header(app_items[11]));
    verify(
      "apps item labels",
      app_items[1] = "Calculator" and app_items[2] = "Clock"
        and app_items[3] = "Widgets" and app_items[4] = "Image Viewer"
        and app_items[6] = "Minesweeper"
        and app_items[8] = "Text Editor" and app_items[9] = "File Browser"
        and app_items[10] = "Calendar"
        and app_items[12] = "Terminal" and app_items[13] = "Settings");
    verify(
      "apps image viewer item",
      app_items[4] = "Image Viewer"
        and stdlib.ui_menu_selectable(app_items[4], uint(0), 4));
    verify(
      "apps headers not selectable",
      not stdlib.ui_menu_selectable(app_items[0], uint(0), 0)
        and not stdlib.ui_menu_selectable(app_items[5], uint(0), 5)
        and not stdlib.ui_menu_selectable(app_items[11], uint(0), 11));
    verify(
      "apps items are selectable",
      stdlib.ui_menu_selectable(app_items[1], uint(0), 1)
        and stdlib.ui_menu_selectable(app_items[4], uint(0), 4)
        and stdlib.ui_menu_selectable(app_items[10], uint(0), 10)
        and stdlib.ui_menu_selectable(app_items[12], uint(0), 12)
        and stdlib.ui_menu_selectable(app_items[13], uint(0), 13));
    // Opening the menu takes the modal grab and launches nothing.
    handle_menu(menu_apps_x + 4, 10);
    verify("apps menu open", handled and app_open and not system_open);
    verify("apps menu first item", app_selected = 1);
    verify("apps menu grab", grab_status = Ok and apps_opened = 1);
    verify("apps menu opens no app", launches = 0);
    draw_apps_menu();
    verify(
      "apps accent row",
      syslib.get_pixel(menu_apps_x + 4, MenuHeight + 1 + 16 + 13)
        = uint(0x000080));
    verify("apps frame", syslib.get_pixel(menu_apps_x - 4, MenuHeight) = black);
    // Row 2 (Clock) is not selected: its label is black ink on white.
    ink_found := false;
    yy := MenuHeight + 1 + 32;
    while yy < MenuHeight + 1 + 48 and not ink_found do
    begin
      xx := menu_apps_x + 16;
      while xx < menu_apps_x + AppMenuWidth - 8 and not ink_found do
      begin
        if syslib.get_pixel(xx, yy) = black then ink_found := true;
        xx := xx + 1;
      end;
      yy := yy + 1;
    end;
    verify("apps item ink", ink_found);
    // Down/Up move the highlight across selectable rows without launching.
    handle_apps_key(uint(0x1130000));
    verify("apps down", app_selected = 2 and launches = 0);
    handle_apps_key(uint(0x1120000));
    verify("apps up", app_selected = 1 and launches = 0);
    // Enter on Clock: one non-blocking spawn, the menu closes and the grab is
    // released. `clock_window` runs its deterministic check and exits, so the
    // shell reaps it with `core.wait` and no child is left behind.
    handle_apps_key(uint(0x1130000));
    verify("apps clock selected", app_selected = 2);
    handle_apps_key(uint(0x1030000));
    verify(
      "apps launch",
      launch_status = Ok and last_task > 0 and launches = 1 and app_runs = 1);
    verify("apps menu closed by launch", not app_open and not system_open);
    verify("apps grab released", syslib.popup_release() = NotFound);
    wait_status := core.wait(last_task, reason);
    verify("apps child exited", wait_status = Ok);
    // An outside click dismisses the menu. The grab is modal, so the click is
    // consumed by the owner and never reaches the window it lies over.
    handle_menu(menu_apps_x + 4, 10);
    verify("apps menu reopened", app_open and apps_opened = 2);
    ev.kind := PointerDown;
    ev.window := 0;
    ev.x := 900;
    ev.y := 400;
    handle_apps_menu_click();
    verify("outside click closes apps", not app_open);
    verify("outside click releases the grab", syslib.popup_release() = NotFound);
    // The Terminal row goes through launch_shell; check mode skips the real
    // emulator so no long-lived child is left behind.
    handle_menu(menu_apps_x + 4, 10);
    pos := 0;
    while app_selected <> 12 and pos < AppMenuCount do
    begin
      handle_apps_key(uint(0x1130000));
      pos := pos + 1;
    end;
    verify("apps terminal selected", app_selected = 12);
    handle_apps_key(uint(0x1030000));
    verify(
      "apps terminal launches",
      launch_status = Ok and terminal_runs = 1 and probe_spawn);
    verify("apps terminal menu closed", not app_open);
    // F9/F10 toggle their own menu and switch between them; Left/Right switch
    // too. Every close path releases the grab.
    handle_shortcut(uint(0x1280000));
    verify("F9 opens apps", app_open and not system_open and grab_status = Ok);
    handle_shortcut(uint(0x1290000));
    verify("F10 switches to system", system_open and not app_open and grab_status = Ok);
    handle_shortcut(uint(0x1290000));
    verify("F10 closes system", not system_open);
    handle_shortcut(uint(0x1280000));
    verify("F9 reopens apps", app_open);
    handle_apps_key(uint(0x1110000));
    verify("right switches to system", system_open and not app_open);
    handle_system_key(uint(0x1100000));
    verify("left switches to apps", app_open and not system_open);
    handle_apps_key(uint(0x1000000));
    verify("escape closes apps", not app_open);
    verify("apps grab released after switching", syslib.popup_release() = NotFound);

    // Windows: the pull-down lists every live window; activating the Terminal
    // row raises it (and would restore it if it had been minimized).
    handle_menu(menu_windows_x + 4, 10);
    verify(
      "windows menu open",
      handled and windows_open and window_runs = 1 and windows_runs = 1);
    verify("windows menu lists windows", window_count >= 3);
    pos := 0;
    raised_id := 0;
    while pos < window_count and raised_id = 0 do
    begin
      if window_items[pos] = "Terminal" or window_items[pos] = "*Terminal" then
      begin
        windows_selected := pos;
        raised_id := window_menu_ids[pos];
      end;
      pos := pos + 1;
    end;
    verify("terminal row found", raised_id > 0);
    if raised_id > 0 then
    begin
      activate_window_item();
      verify(
        "terminal raised from windows menu",
        not windows_open and syslib.window_focus() = raised_id);
      status := syslib.window_info(uint(0), win_id, win_rect, win_title);
      verify(
        "raised window is the terminal",
        status = Ok and win_id = raised_id and win_title = "Terminal");
    end;

    // Enumeration boundary and the sanctioned presentation path.
    status := syslib.window_info(uint(999), win_id, win_rect, win_title);
    verify("index past end is NotFound", status = NotFound);
    status := syslib.window_present(own);
    verify("window_present", status = Ok);

    // ---- System pull-down and About panel (M2.6.4, spec 0017) ----------
    // Item order, labels and shortcut labels are the app's contract.
    verify(
      "system about item",
      sys_items[0] = "About igis-v2" and sys_keys[0] = "Ctrl+A");
    verify("system separator", stdlib.ui_menu_is_separator(sys_items[1]));
    verify(
      "system restart item",
      sys_items[2] = "Restart" and sys_keys[2] = "Ctrl+R");
    verify(
      "system halt item",
      sys_items[3] = "Halt" and sys_keys[3] = "Ctrl+H");
    verify(
      "system selectability",
      stdlib.ui_menu_selectable(sys_items[0], sys_disabled, 0)
        and not stdlib.ui_menu_selectable(sys_items[1], sys_disabled, 1)
        and stdlib.ui_menu_selectable(sys_items[2], sys_disabled, 2)
        and stdlib.ui_menu_selectable(sys_items[3], sys_disabled, 3));

    // Open through the bar handler: the same path a real click takes.
    handle_menu(menu_system_x + 4, 10);
    verify("system opened", handled and system_open and system_selected = 0);
    verify(
      "system accent row",
      syslib.get_pixel(menu_system_x + 56, MenuHeight + 13) = uint(0x000080));
    verify(
      "system separator rule",
      syslib.get_pixel(menu_system_x + 20, MenuHeight + 25) = black);
    // Down skips the separator; Up walks back to the first item.
    handle_system_key(uint(0x1130000));
    verify("system down skips separator", system_selected = 2);
    handle_system_key(uint(0x1130000));
    verify("system down reaches halt", system_selected = 3);
    handle_system_key(uint(0x1120000));
    verify("system up reaches restart", system_selected = 2);
    handle_system_key(uint(0x1120000));
    verify("system up reaches about", system_selected = 0);
    // Escape closes with no action; F10 opens it again.
    handle_system_key(uint(0x1000000));
    verify(
      "system escape closes",
      not system_open and about_runs = 0
        and syslib.popup_release() = NotFound);
    handle_shortcut(uint(0x1290000));
    verify("F10 opens system", system_open and grab_status = Ok);
    // Enter activates the highlighted About item.
    handle_system_key(uint(0x1030000));
    verify("about activated", about_runs = 1 and about_open and about_id > 0);
    verify("system closed by action", not system_open);
    status := syslib.window_info(uint(0), win_id, win_rect, win_title);
    verify(
      "about is topmost",
      status = Ok and win_id = about_id and win_title = "About igis-v2");
    draw_about();
    status := syslib.window_info(uint(0), win_id, win_rect, win_title);
    verify("about still topmost", status = Ok and win_id = about_id);
    status := syslib.window_content_rect(about_id, about_rect);
    
    verify(
      "about content white",
      status = Ok
        and syslib.get_pixel(2, about_rect.h - 2) = white);
    verify(
      "about button border",
      syslib.get_pixel((about_rect.w - 80) div 2, about_rect.h - 34) = black);
    // The prop face's exact ink pixels are not a contract; any ink in the
    // title band proves the panel text was drawn.
    ink_found := false;
    yy := 8;
    while yy < 24 and not ink_found do
    begin
      xx := 12;
      while xx < 140 and not ink_found do
      begin
        if syslib.get_pixel(xx, yy) = black then ink_found := true;
        xx := xx + 1;
      end;
      yy := yy + 1;
    end;
    verify("about title ink", ink_found);
    // Escape closes the modal panel.
    handle_about_key(uint(0x1000000));
    verify("about escape closes", not about_open and about_id = 0);
    status := syslib.window_info(uint(0), win_id, win_rect, win_title);
    verify("about gone from enumeration", status = Ok and win_title <> "About igis-v2");
    // A pointer-down routed to the panel closes it as well.
    open_about();
    verify("about reopened", about_open and about_runs = 2);
    ev.kind := PointerDown;
    ev.window := about_id;
    handle_about_event();
    verify("about click closes", not about_open and about_id = 0);
    // Ctrl+A opens the panel through the shortcut path.
    handle_shortcut(uint(0x410200));
    verify("ctrl+a opens about", about_open and about_runs = 3);
    handle_about_key(uint(0x1000000));
    verify("shortcut about closed", not about_open);
    // The close box is an owner close request, never a kill (M8.2, spec 0011
    // W23/W25). A real pointer press in the About close box must deliver
    // exactly one Close event for the panel (no client PointerDown), and the
    // dialog handler closes only the panel while the desktop stays alive.
    open_about();
    verify("about reopened for close box", about_open and about_runs = 4);
    status := syslib.window_frame_rect(about_id, about_rect);
    verify("about frame for close box", status = Ok);
    // Close box: frame + (4..15, 3..14); its centre is frame + (9, 8).
    status := inject.pointer_to(about_rect.x + 9, about_rect.y + 8);
    flushed := syslib.event_flush();
    status := inject.pointer_down();
    status := inject.pointer_up();
    ev.kind := Expose;
    ev.window := 0;
    status := syslib.poll_event(ev);
    verify(
      "close box requests close",
      status = Ok and ev.kind = Close and ev.window = about_id);
    verify("close box is not a click", ev.kind <> PointerDown);
    handle_about_event();
    verify(
      "about close box closes only the panel",
      not about_open and about_id = 0);
    // Drain what the press left behind: the consumed PointerDown must not have
    // been queued, so no client click for the About window may appear.
    saw_click := false;
    more_events := true;
    count_events := 0;
    while more_events and count_events < 16 do
    begin
      status := syslib.poll_event(ev);
      if status = Ok then
      begin
        if ev.kind = PointerDown and ev.window = about_id then
          saw_click := true;
        count_events := count_events + 1;
      end
      else
        more_events := false;
    end;
    verify("close box press never reaches the client", not saw_click);
    // The desktop is still alive: it can open About again and close it.
    open_about();
    verify("desktop survives the close box", about_open and about_runs = 5);
    handle_about_key(uint(0x1000000));
    verify("about closes after survival", not about_open);
    // The checks never fired a system action.
    verify("no system action fired", restart_runs = 0 and halt_runs = 0);
    // The desktop window still redraws after the panels.
    draw_shell_window();
    status := syslib.window_content_rect(own, about_rect);
    verify(
      "desktop marker restored",
      status = Ok
        and syslib.get_pixel(about_rect.w - 4, about_rect.h - 4) = black);

    close_window_list();
    if probe_term > 0 then
      status := syslib.window_close(probe_term);
    probe_term := 0;
    if own > 0 then
      status := syslib.window_raise(own);

    // Leave the bar and the window composed for the final screen.
    draw_menu_bar();
    draw_shell_window();
  end;

  if failed then
  begin
    out.say("desktop_shell: FAIL at step ");
    out.say_int(fail_step);
    out.say(": ");
    out.say(fail_label);
    out.say("\n");
  end
  else
    out.say("desktop_shell: ok\n");
end;

procedure normal_mode();
begin
  draw_menu_bar();
  open_desktop_window();

  // A normal task would be killed by the 10-second budget. The shell is a
  // controller: promote to realtime and heartbeat around each blocked wait.
  // If every core already holds a realtime task, the short normal-budget life
  // is still enough to show the frame.
  status := core.realtime_on();
  out.say("desktop_shell: ready\n");
  running := true;
  while running do
  begin
    core.heartbeat();
    status := syslib.next_event(ev);
    if status = Ok then
    begin
      core.heartbeat();
      // A close request is owner-only and always addressed: the About panel
      // closes itself, and the shell's own `Desktop` window closes only the
      // help panel; the desktop keeps running (the close box never kills the
      // task; spec 0011, W23; spec 0012 D12/D19; M8.7).
      if ev.kind = Close then
      begin
        if about_open and ev.window = about_id then
          close_about()
        else if ev.window = winlist_id then
          close_window_list()
        else if ev.window = own then
          close_desktop_window();
      end
      // The About panel is modal: every other event goes to it until it
      // closes.
      else if about_open then
        handle_about_event()
      else if ev.kind = Expose then
      begin
        if ev.window = own then
          draw_shell_window()
        else if ev.window = winlist_id then
          draw_window_list();
      end
      else if ev.kind = KeyDown then
      begin
        if system_open then
          handle_system_key(ev.code)
        else if app_open then
          handle_apps_key(ev.code)
        else if windows_open then
          handle_windows_key(ev.code)
        else if ev.window = own or ev.window = 0 then
          handle_shortcut(ev.code);
      end
      else if ev.kind = PointerDown then
      begin
        if system_open then
          handle_system_menu_click()
        else if app_open then
          handle_apps_menu_click()
        else if windows_open then
          handle_windows_menu_click()
        else if ev.window = 0 then
          handle_menu(ev.x, ev.y)
        else if ev.window = winlist_id then
          raise_window_at(ev.x, ev.y)
        else if ev.window = own then
          draw_shell_window();
      end
      else if ev.kind = PointerMove then
      begin
        if system_open then
          update_menu_highlight()
        else if app_open then
          update_apps_highlight()
        else if windows_open then
          update_windows_highlight();
      end;
      // Keep the bar visible after every event; no window starts above it.
      draw_menu_bar();
    end;
  end;
  // The loop normally never ends: a Desktop-window close only closes the help
  // panel (`close_desktop_window`) and the shell keeps pumping; Restart/Halt
  // and task termination are the only exits. If a future quit path leaves the
  // loop, close the shell's window before returning.
  if own > 0 then
    status := syslib.window_close(own);
end;

procedure main(args: const string[1024]);
begin
  white := uint(0xFFFFFF);
  black := uint(0x000000);
  screen_w := int(syslib.screen_width());
  screen_h := int(syslib.screen_height());
  sync_clock();
  shell_launched := false;
  probe_spawn := false;
  terminal_runs := 0;
  init_apps_menu();
  init_system_menu();
  winlist_open := false;
  winlist_id := 0;
  win_line_count := 0;
  about_open := false;
  about_id := 0;
  about_runs := 0;
  restart_runs := 0;
  halt_runs := 0;
  bar := 0;
  menu_plane := 0;
  halt_plane := 0;
  if args = "check" then
  begin
    if screen_w = 0 or screen_h = 0 then
      out.say("desktop_shell: no graphics\n")
    else if screen_w < 400 or screen_h < 300 then
      out.say("desktop_shell: screen too small\n")
    else
    begin
      setup_work_area();
      check_mode();
    end;
  end
  else if screen_w = 0 or screen_h = 0 then
  begin
    out.say("desktop_shell: no graphics\n");
    serial_shell();
  end
  else if screen_w < 400 or screen_h < 300 then
  begin
    out.say("desktop_shell: screen too small\n");
    serial_shell();
  end
  else
  begin
    setup_work_area();
    normal_mode();
    // A terminated task releases its planes through `Task::reclaim` (spec
    // 0026, C1); there is no explicit release step.
  end;
end;

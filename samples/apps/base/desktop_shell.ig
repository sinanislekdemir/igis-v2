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
//   Apps      pull-down: Calculator, Minesweeper, Text Editor, File Browser,
//             Clock (clock_window), Widgets; each is a non-blocking core.spawn
//   Windows   raise the terminal window (the existing "bring forward" item)
//   Terminal  launch `shell` non-blocking; a later click raises the existing
//             Terminal window instead of spawning a second shell
//   Apple     raise the shell's own window, reopening it if it was closed
//   System    pull-down: About igis-v2 (Ctrl+A), Restart (Ctrl+R), Halt (Ctrl+H)
//
// Both pull-downs are drawn with the M2.4.3 menu model
// (`stdlib.ui_menu_dropdown`/`ui_menu_hit`/`ui_menu_move`/`ui_menu_key`) and
// the M2.4.4 dialog helpers (`ui_message`/`ui_dialog_button`). They are
// app-drawn and have no window, so opening one takes the machine-wide modal
// popup input grab (spec 0011, W26; ADR 0044): every queued pointer and key
// event goes to this task while the menu is open, which is what makes a row
// that lies over another window reachable. The About panel is an ordinary
// desktop-owned window, so the window service restores what it covered through
// the usual Expose protocol.
//
// The shell draws its own window content and presents it with
// `syslib.window_present`; an `Expose` for its id repaints that content. Window
// lifetime, z-order and chrome belong to the kernel service, so the shell never
// draws outside its content rectangle except for the menu bar and its pull-down,
// which occupy the top rows where no window starts by convention.
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
  AppMenuWidth = 176;
  AppMenuCount = 6;
  AboutContentW = 420;
  AboutContentH = 200;

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
  running: bool;

  // Menu bookkeeping.
  handled: bool;
  file_runs: int;
  app_runs: int;
  window_runs: int;
  terminal_runs: int;
  // The persistent menu-bar pointer-region claim (M8.4, spec 0011 W27). The
  // modal grab only starts once a menu opens; the bar click that opens it must
  // already belong to this task.
  bar_status: Status;
  // Proportional menu-bar layout, derived from the measured label widths
  // (spec 0015) so the hit ranges and the drawn labels cannot drift.
  menu_file_x: int;
  menu_apps_x: int;
  menu_windows_x: int;
  menu_terminal_x: int;
  menu_terminal_end: int;
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

  // The System pull-down (M2.6.4, spec 0017). The app owns the open state, the
  // highlight and the item tables; the helpers draw and hit-test only.
  sys_items: array[string[64], 16];
  sys_keys: array[string[32], 16];
  sys_disabled: uint;
  sys_checked: uint;
  system_open: bool;
  system_selected: int;
  system_runs: int;

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
  under_id: int;
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
  top_id: int;
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

// The menu bar is app-drawn and has no window either, but unlike a pull-down it
// is permanent: the press that opens a menu arrives before any modal grab
// exists, and another client blocked in `next_event` can otherwise take it
// from the one machine-wide queue. Claim the bar strip for as long as the
// shell runs (spec 0011, W27; ADR 0045): a queued pointer-down inside it is
// routed here even while a launched app is waiting for input. The modal grab
// still covers an open pull-down and takes precedence. A failed claim (another
// owner, or graphics disabled) is reported and the shell keeps running.
procedure claim_menu_bar();
begin
  bar_status := syslib.region_claim(0, 0, screen_w, MenuHeight);
  if bar_status <> Ok then
    out.say("desktop_shell: menu bar claim failed\n");
end;

// HH:MM from the monotonic clock, right-aligned in the menu bar. The width is
// the proportional measurement (spec 0015), so the digits and the colon sit
// tight to the right edge instead of assuming 8-pixel cells.
procedure draw_clock(right: int);
var
  clock_x: int;
  clock_w: int;
begin
  now := core.clock();
  minutes := (now div uint(1000000000)) div uint(60);
  hours := (minutes div uint(60)) mod uint(24);
  minutes := minutes mod uint(60);
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
  menu_terminal_x := menu_windows_x + tw("Windows") + 14;
  menu_terminal_end := menu_terminal_x + tw("Terminal");
  menu_system_x := menu_terminal_end + 14;
  menu_system_end := menu_system_x + tw("System");
end;

// The menu bar is application-drawn (no global work area in M2.3.4). It stays
// in the top 22 rows; app windows are expected to open below it. The labels and
// the clock use the proportional font (spec 0015): positions come from the
// measured widths, not from 8-pixel cells.
procedure draw_menu_bar();
begin
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
  status := syslib.draw_text_prop(menu_windows_x, 7, "Windows", black);
  status := syslib.draw_text_prop(menu_terminal_x, 7, "Terminal", black);
  if system_open then
  begin
    status := syslib.fill_rect(menu_system_x - 4, 2, tw("System") + 8, MenuHeight - 4, black);
    status := syslib.draw_text_prop(menu_system_x, 7, "System", white);
  end
  else
    status := syslib.draw_text_prop(menu_system_x, 7, "System", black);
  status := syslib.fill_rect(0, MenuHeight - 1, screen_w, 1, black);
  draw_clock(screen_w - 8);
  // The bar is application-drawn into the shadow; present its rows so the
  // framebuffer shows it (a window repair may have presented only its own
  // damage union).
  status := syslib.present(0, 0, screen_w, MenuHeight);
end;

// Draws the shell's own window content. It only repaints when the shell is the
// topmost window: a covered client has no per-task clip in M2, so painting the
// whole content would overwrite the window above it. When it is not topmost it
// only presents, which leaves the visible part as the service last left it
// (stale/dither), exactly the accepted M2 liveness rule (spec 0011, W18).
procedure draw_shell_window();
begin
  if own > 0 then
  begin
    status := syslib.window_info(uint(0), top_id, win_rect, win_title);
    if status = Ok and top_id = own then
    begin
      status := syslib.window_content_rect(own, rect);
      if status = Ok then
      begin
        status := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, white);
        status := syslib.draw_text(rect.x + 12, rect.y + 12, "igis-v2", black);
        status := syslib.draw_text(rect.x + 12, rect.y + 28, "M2.3.4 desktop shell", black);
        status := syslib.draw_text(rect.x + 12, rect.y + 52, "File: list windows", black);
        status := syslib.draw_text(rect.x + 12, rect.y + 68, "Apps: application menu", black);
        status := syslib.draw_text(rect.x + 12, rect.y + 84, "Windows: raise terminal", black);
        status := syslib.draw_text(rect.x + 12, rect.y + 100, "Terminal: launch shell", black);
        status := syslib.draw_text(rect.x + 12, rect.y + 116, "focus:", black);
        focus := syslib.window_focus();
        used := core.format_int_into(digits, focus, uint(10), uint(0));
        status := syslib.draw_text(rect.x + 68, rect.y + 116, digits, black);
        // A small black mark in the content, used by the check-mode pixel test
        // and visible as the shell's own content marker.
        status := syslib.fill_rect(
          rect.x + rect.w - 8, rect.y + rect.h - 8, 8, 8, black);
      end;
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
  status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  while status = Ok and pos < 32 do
  begin
    out.say("window ");
    out.say_int(win_id);
    out.say(" \"");
    out.say(win_title);
    out.say("\" ");
    out.say_int(win_rect.x);
    out.say(",");
    out.say_int(win_rect.y);
    out.say(" ");
    out.say_int(win_rect.w);
    out.say("x");
    out.say_int(win_rect.h);
    out.say("\n");
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
  status := stdlib.ui_menu_dropdown(
    menu_system_x - 4, MenuHeight, SysMenuWidth,
    sys_items, sys_keys, SysMenuCount, system_selected, sys_checked, sys_disabled, true);
  status := syslib.present(
    menu_system_x - 4, MenuHeight, SysMenuWidth, SysMenuCount * 16 + 2);
end;

procedure open_system_menu();
begin
  if app_open then
    close_apps_menu();
  system_open := true;
  system_selected := stdlib.ui_menu_first(SysMenuCount, sys_items, sys_disabled);
  system_runs := system_runs + 1;
  grab_status := syslib.popup_grab(
    menu_system_x - 4, MenuHeight, SysMenuWidth, SysMenuCount * 16 + 2);
  draw_menu_bar();
  draw_system_menu();
end;

// ---- the Apps pull-down (M8.3, spec 0012 D20) ----------------------------
//
// Item order (0-based, the order drawn and the order Up/Down traverses):
//
//   0  Calculator       (calculator)
//   1  Minesweeper      (minesweeper)
//   2  Text Editor      (editor)
//   3  File Browser     (browser)
//   4  Clock            (clock_window)
//   5  Widgets          (widgets)
//
// Every item is a non-blocking `core.spawn`; the shell never waits. The menu
// frame is `(menu_apps_x - 4, MenuHeight, AppMenuWidth, 6 * 16 + 2)`.

procedure init_apps_menu();
begin
  app_items[0] := "Calculator";
  app_items[1] := "Minesweeper";
  app_items[2] := "Text Editor";
  app_items[3] := "File Browser";
  app_items[4] := "Clock";
  app_items[5] := "Widgets";
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
  status := stdlib.ui_menu_dropdown(
    menu_apps_x - 4, MenuHeight, AppMenuWidth,
    app_items, app_keys, AppMenuCount, app_selected, uint(0), uint(0), true);
  status := syslib.present(
    menu_apps_x - 4, MenuHeight, AppMenuWidth, AppMenuCount * 16 + 2);
end;

procedure open_apps_menu();
begin
  if system_open then
    close_system_menu();
  app_open := true;
  app_selected := stdlib.ui_menu_first(AppMenuCount, app_items, uint(0));
  apps_opened := apps_opened + 1;
  // The menu is app-drawn and has no window: the grab makes this task the only
  // recipient of queued input while it is open (spec 0011, W26). The rectangle
  // is advisory; the click handler below decides inside/outside.
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
  repair_menu_area(
    menu_apps_x - 4, MenuHeight, AppMenuWidth, AppMenuCount * 16 + 2);
  draw_menu_bar();
  draw_shell_window();
  apps_closed := apps_closed + 1;
  out.say("desktop_shell: apps menu closed\n");
end;

// Erases the pull-down pixels. The shared shadow has no window for the menu, so
// the topmost window whose frame intersects the menu area is raised: the
// service repaints its chrome and marks its `Expose`, and the owner (the
// console pump for the terminal, the app otherwise) restores its content. The
// desktop window is raised again afterwards so focus returns to the shell.
procedure repair_menu_area(mx: int; my: int; mw: int; mh: int);
begin
  under_id := 0;
  pos := 0;
  status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  while status = Ok and pos < 32 and under_id = 0 do
  begin
    if not (win_rect.x + win_rect.w <= mx) then
      if not (win_rect.x >= mx + mw) then
        if not (win_rect.y + win_rect.h <= my) then
          if not (win_rect.y >= my + mh) then
            under_id := win_id;
    pos := pos + 1;
    status := syslib.window_info(uint(pos), win_id, win_rect, win_title);
  end;
  if under_id > 0 and under_id <> own then
    status := syslib.window_raise(under_id);
  if own > 0 then
    status := syslib.window_raise(own);
end;

procedure close_system_menu();
begin
  if not system_open then
    return;
  system_open := false;
  system_selected := -1;
  status := syslib.popup_release();
  repair_menu_area(
    menu_system_x - 4, MenuHeight, SysMenuWidth, SysMenuCount * 16 + 2);
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
  launch_status := core.spawn(name, "", diag, last_task);
  launches := launches + 1;
  if launch_status = Ok then
  begin
    out.say("desktop_shell: launched ");
    out.say(label);
    out.say(" task ");
    out.say_int(last_task);
    out.say("\n");
  end
  else
  begin
    out.say("desktop_shell: launch failed: ");
    out.say(diag.message);
    out.say("\n");
  end;
end;

procedure activate_app_item();
begin
  action := app_selected;
  if action < 0 or action >= AppMenuCount then
    return;
  app_runs := app_runs + 1;
  close_apps_menu();
  if action = 0 then
    launch_app("calculator", "Calculator")
  else if action = 1 then
    launch_app("minesweeper", "Minesweeper")
  else if action = 2 then
    launch_app("editor", "Text Editor")
  else if action = 3 then
    launch_app("browser", "File Browser")
  else if action = 4 then
    launch_app("clock_window", "Clock")
  else if action = 5 then
    launch_app("widgets", "Widgets");
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

procedure request_halt();
begin
  out.say("desktop_shell: halt\n");
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
  if about_id = 0 then
    return;
  status := syslib.window_content_rect(about_id, about_rect);
  if status = Ok then
  begin
    status := syslib.fill_rect(
      about_rect.x, about_rect.y, about_rect.w, about_rect.h, white);
    status := syslib.draw_text_prop(
      about_rect.x + 14, about_rect.y + 10, "igis-v2", black);
    status := stdlib.ui_message(
      about_rect.x + 14, about_rect.y + 32, about_rect.w - 28,
      about_rect.h - 32 - 44,
      "Interpreted desktop at ring 0.\nWindows, terminal, text and files.\nFonts, menus and a golden frame.\nM2, 2026.");
    row_top := about_rect.y + about_rect.h - 34;
    status := stdlib.ui_dialog_button(
      about_rect.x + (about_rect.w - 80) div 2, row_top, 80, 24,
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

// Non-blocking shell launch (M2.5.6). The desktop is the boot entry, so the
// first Terminal click spawns `shell` with `core.spawn` and keeps pumping; the
// shell opens its own terminal window and attaches it (spec 0014). A later
// click raises the existing Terminal window instead of starting a second shell
// that could not hold the one terminal. If the window is gone (the shell
// exited), the click spawns a fresh one.
procedure launch_shell();
begin
  raised_id := 0;
  if shell_launched then
    raise_terminal();
  if raised_id > 0 then
  begin
    out.say("desktop_shell: terminal raised\n");
  end
  else
  begin
    launch_status := core.spawn("shell", "", diag, shell_task);
    if launch_status = Ok then
    begin
      shell_launched := true;
      out.say("desktop_shell: launched shell task ");
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
      list_windows();
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
    else if x >= menu_windows_x - 4 and x < menu_terminal_x - 4 then
    begin
      if system_open then close_system_menu();
      if app_open then close_apps_menu();
      raise_terminal();
      window_runs := window_runs + 1;
      handled := true;
    end
    else if x >= menu_terminal_x - 4 and x < menu_terminal_end + 4 then
    begin
      if system_open then close_system_menu();
      if app_open then close_apps_menu();
      launch_shell();
      terminal_runs := terminal_runs + 1;
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
  // `check` is run from a shell that already holds the one terminal, so the
  // Terminal item must raise that window, not spawn a second shell.
  shell_launched := true;
  // The claim is `Ok` on an image without the boot desktop and `Full` when the
  // running boot desktop already holds the bar, which is the expected result
  // when `run desktop_shell check` is issued from the desktop's terminal.
  verify("menu bar claim", bar_status = Ok or bar_status = Full);

  draw_menu_bar();
  status := syslib.window_open(600, 72, 360, 164, "Desktop", own);
  verify("open desktop window", status = Ok and own > 0);
  if status = Ok then
  begin
    draw_shell_window();

    // The menu bar and the service-drawn chrome/content are really in the
    // shadow. The shell window sits below the bar; no kernel window starts
    // above y=26.
    verify("menu bar white", syslib.get_pixel(700, 2) = white);
    verify("menu baseline", syslib.get_pixel(700, 21) = black);
    verify("apple square", syslib.get_pixel(10, 10) = black);

    status := syslib.window_frame_rect(own, rect);
    verify(
      "frame geometry",
      status = Ok and rect.x = 600 and rect.y = 72 and rect.w = 362 and rect.h = 181);
    verify("frame border", syslib.get_pixel(rect.x, rect.y) = black);
    verify(
      "content marker",
      syslib.get_pixel(rect.x + rect.w - 4, rect.y + rect.h - 4) = black);

    // File: enumerate. The kernel terminal and system log must be present,
    // together with this shell's window.
    handle_menu(menu_file_x + 4, 10);
    verify("file item handled", handled and file_runs = 1);
    verify("terminal listed", saw_terminal);
    verify("system log listed", saw_log);
    verify("desktop listed", saw_desktop);
    verify("at least three windows", count >= 3);

    // ---- Apps pull-down (M8.3, spec 0012 D20) -------------------------
    // The labels and order are the app's contract; every item launches a
    // different source application with `core.spawn`.
    verify(
      "apps item labels",
      app_items[0] = "Calculator" and app_items[1] = "Minesweeper"
        and app_items[2] = "Text Editor" and app_items[3] = "File Browser"
        and app_items[4] = "Clock" and app_items[5] = "Widgets");
    verify(
      "apps menu has no separators",
      not stdlib.ui_menu_is_separator(app_items[0])
        and not stdlib.ui_menu_is_separator(app_items[5]));
    verify(
      "apps items are selectable",
      stdlib.ui_menu_selectable(app_items[0], uint(0), 0)
        and stdlib.ui_menu_selectable(app_items[5], uint(0), 5));
    // Opening the menu takes the modal grab and launches nothing.
    handle_menu(menu_apps_x + 4, 10);
    verify("apps menu open", handled and app_open and not system_open);
    verify("apps menu first item", app_selected = 0);
    verify("apps menu grab", grab_status = Ok and apps_opened = 1);
    verify("apps menu opens no app", launches = 0);
    verify(
      "apps accent row",
      syslib.get_pixel(menu_apps_x + 56, MenuHeight + 13) = uint(0x000080));
    verify("apps frame", syslib.get_pixel(menu_apps_x - 4, MenuHeight) = black);
    // Row 1 (Minesweeper) is not selected: its label is black ink on white.
    ink_found := false;
    yy := MenuHeight + 1 + 16;
    while yy < MenuHeight + 1 + 32 and not ink_found do
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
    // Down/Up move the highlight without wrapping or launching.
    handle_apps_key(uint(0x1130000));
    verify("apps down", app_selected = 1 and launches = 0);
    handle_apps_key(uint(0x1120000));
    verify("apps up", app_selected = 0 and launches = 0);
    // Enter on Clock: one non-blocking spawn, the menu closes and the grab is
    // released. `clock_window` runs its deterministic check and exits, so the
    // shell reaps it with `core.wait` and no child is left behind (a kill right
    // after `spawn` can race the compile window, spec 0007).
    handle_apps_key(uint(0x1130000));
    handle_apps_key(uint(0x1130000));
    handle_apps_key(uint(0x1130000));
    handle_apps_key(uint(0x1130000));
    verify("apps clock selected", app_selected = 4);
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

    // Windows: raise the terminal; focus and the top of the z-order follow.
    handle_menu(menu_windows_x + 4, 10);
    verify("windows item handled", handled and window_runs = 1);
    verify(
      "terminal raised",
      raised_id > 0 and raised_id <> own and syslib.window_focus() = raised_id);
    status := syslib.window_info(uint(0), win_id, win_rect, win_title);
    verify(
      "raised window is the terminal",
      status = Ok and win_id = raised_id and win_title = "Terminal");

    // Terminal: the live boot shell already holds the terminal, so this item
    // reuses and raises it rather than spawning a second shell (M2.5.6).
    handle_menu(menu_terminal_x + 4, 10);
    verify("terminal item handled", handled and terminal_runs = 1);
    verify(
      "terminal item reuses the terminal",
      raised_id > 0 and raised_id <> own and syslib.window_focus() = raised_id);
    status := syslib.window_info(uint(0), win_id, win_rect, win_title);
    verify(
      "terminal item raised the terminal",
      status = Ok and win_id = raised_id and win_title = "Terminal");

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
    status := syslib.window_content_rect(about_id, about_rect);
    verify(
      "about content white",
      status = Ok
        and syslib.get_pixel(about_rect.x + 2, about_rect.y + about_rect.h - 2)
          = white);
    verify(
      "about button border",
      syslib.get_pixel(
        about_rect.x + (about_rect.w - 80) div 2,
        about_rect.y + about_rect.h - 34) = black);
    // The prop face's exact ink pixels are not a contract; any ink in the
    // title band proves the panel text was drawn.
    ink_found := false;
    yy := about_rect.y + 8;
    while yy < about_rect.y + 24 and not ink_found do
    begin
      xx := about_rect.x + 12;
      while xx < about_rect.x + 140 and not ink_found do
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
        and syslib.get_pixel(about_rect.x + about_rect.w - 4, about_rect.y + about_rect.h - 4)
          = black);

    // Leave the bar and the window composed for the final screen.
    draw_menu_bar();
    draw_shell_window();
  end;

  // Release the bar claim before the verdict line: a caller can then claim it
  // (or a second `check` can) without inheriting this instance's rectangle.
  if bar_status = Ok then
    verify("menu bar claim released", syslib.region_release() = Ok)
  else
    verify("menu bar claim owned elsewhere", bar_status = Full);

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
        else if ev.window = own then
          close_desktop_window();
      end
      // The About panel is modal: every other event goes to it until it
      // closes.
      else if about_open then
        handle_about_event()
      else if ev.kind = Expose then
      begin
        if ev.window = own then draw_shell_window();
      end
      else if ev.kind = KeyDown then
      begin
        if system_open then
          handle_system_key(ev.code)
        else if app_open then
          handle_apps_key(ev.code)
        else if ev.window = own or ev.window = 0 then
          handle_shortcut(ev.code);
      end
      else if ev.kind = PointerDown then
      begin
        if system_open then
          handle_system_menu_click()
        else if app_open then
          handle_apps_menu_click()
        else if ev.window = 0 then
          handle_menu(ev.x, ev.y)
        else if ev.window = own then
          draw_shell_window();
      end
      else if ev.kind = PointerMove then
      begin
        if system_open then
          update_menu_highlight()
        else if app_open then
          update_apps_highlight();
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
  shell_launched := false;
  terminal_runs := 0;
  init_apps_menu();
  init_system_menu();
  about_open := false;
  about_id := 0;
  about_runs := 0;
  restart_runs := 0;
  halt_runs := 0;
  if args = "check" then
  begin
    if screen_w = 0 or screen_h = 0 then
      out.say("desktop_shell: no graphics\n")
    else if screen_w < 400 or screen_h < 300 then
      out.say("desktop_shell: screen too small\n")
    else
    begin
      claim_menu_bar();
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
    claim_menu_bar();
    normal_mode();
    // The Desktop window was closed and the shell is leaving; a terminated
    // task releases the claim through `Task::reclaim` anyway (spec 0011, W27).
    if bar_status = Ok then
      status := syslib.region_release();
  end;
end;

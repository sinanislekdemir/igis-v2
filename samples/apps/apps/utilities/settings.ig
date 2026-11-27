unit settings;

// Settings (spec 0022): choose a runtime display mode.
//
//   run settings            // interactive: pick a resolution, press Apply
//   run settings check      // list modes, set 800x600, restore 1024x768,
//                           // print `settings: ok`
//   run settings shot       // set 800x600, draw the window, print
//                           // `settings: ready` and hold for the screenshot
//
// The app is a client of the window service: it draws its content in
// window-local coordinates and presents once per frame. `syslib.set_resolution`
// changes the real device mode; the service scales and clamps the window and
// marks it for an `Expose`, which just repaints the fixed-size content.

uses core, stdlib, syslib, lib.out;

const
  WindowX = 260;
  WindowY = 100;
  ContentW = 360;
  ContentH = 300;

  ListX = 12;
  ListY = 44;
  ListW = 336;
  ListH = 128;

  ApplyX = 12;
  ApplyY = 184;
  ApplyW = 100;
  ApplyH = 28;

  CurrentX = 124;
  CurrentY = 192;
  MessageY = 224;
  HintY = 250;

  MaxModes = 8;

var
  id: int;
  status: Status;
  ev: Event;
  running: bool;

  black: uint;
  white: uint;

  items: array[string[64], MaxModes];
  count: int;
  selected: int;
  top: int;
  list_focus: bool;
  apply_pressed: bool;

  screen_w: uint;
  screen_h: uint;
  current_text: string[256];
  message: string[256];
  label_stage: string[256];
  item_stage: string[64];
  number_stage: string[16];
  written: int;

// ---- text helpers -------------------------------------------------------

procedure say(text: const string[0]);
begin
  out.say(text);
end;

procedure say_uint(value: uint);
begin
  written := core.format_int_into(number_stage, int(value), uint(10), uint(0));
  out.say(number_stage);
end;

// `WxH` into `label_stage`.
procedure mode_label(index: int);
begin
  written := core.format_int_into(
    label_stage, int(syslib.display_mode_width(uint(index))), uint(10), uint(0));
  status := stdlib.str_insert_into(label_stage, int(core.len(label_stage)), "x");
  written := core.format_int_into(
    number_stage, int(syslib.display_mode_height(uint(index))), uint(10), uint(0));
  status := stdlib.str_insert_into(label_stage, int(core.len(label_stage)), number_stage);
end;

// `Current: WxH` into `current_text`.
procedure update_current();
begin
  current_text := "";
  status := stdlib.str_insert_into(current_text, 0, "Current: ");
  written := core.format_int_into(number_stage, int(screen_w), uint(10), uint(0));
  status := stdlib.str_insert_into(current_text, int(core.len(current_text)), number_stage);
  status := stdlib.str_insert_into(current_text, int(core.len(current_text)), "x");
  written := core.format_int_into(number_stage, int(screen_h), uint(10), uint(0));
  status := stdlib.str_insert_into(current_text, int(core.len(current_text)), number_stage);
end;

// Builds the selectable list and selects the mode the screen is already in.
procedure update_modes();
var
  index: int;
  total: int;
begin
  total := int(syslib.display_mode_count());
  count := total;
  if count > MaxModes then
    count := MaxModes;
  index := 0;
  selected := 0;
  while index < count do
  begin
    mode_label(index);
    written := core.substr_into(item_stage, label_stage, 0, int(core.len(label_stage)));
    items[index] := item_stage;
    if syslib.display_mode_width(uint(index)) = screen_w
       and syslib.display_mode_height(uint(index)) = screen_h then
      selected := index;
    index := index + 1;
  end;
  top := 0;
  if count = 0 then
    message := "No runtime display modes."
  else
    message := "";
end;

// ---- drawing ------------------------------------------------------------

procedure draw_frame();
begin
  // Window-local coordinates (spec 0026 C8): the frame never moves in the
  // client's space, so no origin is cached.
  status := syslib.fill_rect(0, 0, ContentW, ContentH, white);
  status := syslib.draw_rect(0, 0, ContentW, ContentH, black);
  status := syslib.draw_text(12, 10, "Display", black);
  status := syslib.draw_text(12, 28, "Resolution", black);
  status := stdlib.ui_list(
    ListX, ListY, ListW, ListH,
    items, count, selected, top, list_focus);
  status := stdlib.ui_button(
    ApplyX, ApplyY, ApplyW, ApplyH,
    "Apply", apply_pressed, not list_focus);
  status := syslib.draw_text(CurrentX, CurrentY, current_text, black);
  if core.len(message) > uint(0) then
    status := syslib.draw_text(12, MessageY, message, black);
  status := syslib.draw_text(
    12, HintY, "Changes the real display mode.", black);
end;

procedure present_frame();
begin
  status := syslib.window_present(id);
end;

// ---- actions ------------------------------------------------------------

// Applies the selected row; on success the window service has already resized
// the screen and marked this window for an Expose.
procedure apply_selected();
var
  width: uint;
  height: uint;
begin
  if selected < 0 or selected >= count then
  begin
    message := "No mode selected.";
    return;
  end;
  width := syslib.display_mode_width(uint(selected));
  height := syslib.display_mode_height(uint(selected));
  status := syslib.set_resolution(width, height);
  if status = Ok then
  begin
    screen_w := width;
    screen_h := height;
    message := "Applied.";
    update_current();
  end
  else if status = NotFound then
    message := "Mode not available."
  else if status = Full then
    message := "Not enough memory."
  else if status = IoError then
    message := "Display rejected the mode."
  else
    message := "Invalid mode.";
end;

// ---- events -------------------------------------------------------------

procedure handle_pointer();
var
  hit: int;
  inside: bool;
begin
  hit := stdlib.ui_list_hit(
    ListX, ListY, ListW, ListH,
    ev.x, ev.y, count, top);
  if ev.kind = PointerDown then
  begin
    apply_pressed := stdlib.ui_button_hit(
      ApplyX, ApplyY, ApplyW, ApplyH, ev.x, ev.y);
    if hit >= 0 then
    begin
      selected := hit;
      top := stdlib.ui_list_ensure_visible(selected, top, stdlib.ui_list_rows(ListH));
      list_focus := true;
    end
    else if apply_pressed then
      list_focus := false;
  end
  else if ev.kind = PointerUp then
  begin
    inside := stdlib.ui_button_hit(
      ApplyX, ApplyY, ApplyW, ApplyH, ev.x, ev.y);
    if stdlib.ui_button_pressed(apply_pressed, inside) then
      apply_selected();
    apply_pressed := false;
  end;
end;

procedure handle_key();
begin
  if stdlib.ui_key_is_up(ev.code) then
    selected := stdlib.ui_list_move(selected, count, -1)
  else if stdlib.ui_key_is_down(ev.code) then
    selected := stdlib.ui_list_move(selected, count, 1)
  else if stdlib.ui_key_is_tab(ev.code) then
    list_focus := not list_focus
  else if stdlib.ui_key_is_enter(ev.code) then
    apply_selected();
  if selected >= 0 and selected < count then
    top := stdlib.ui_list_ensure_visible(selected, top, stdlib.ui_list_rows(ListH));
end;

procedure interactive();
begin
  running := true;
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
        else if ev.kind = PointerDown or ev.kind = PointerUp then
          handle_pointer();
        draw_frame();
        present_frame();
      end;
    end;
  end;
  status := syslib.window_close(id);
end;

// ---- fixed modes --------------------------------------------------------

// Headless proof. The list, the size check and the restore exercise the same
// kernel path the Apply button uses.
procedure check_mode();
var
  index: int;
  ok: bool;
begin
  ok := true;
  say("settings: ");
  say_uint(syslib.display_mode_count());
  say(" modes\n");
  index := 0;
  while index < int(syslib.display_mode_count()) do
  begin
    mode_label(index);
    say("settings:   ");
    say(label_stage);
    say("\n");
    index := index + 1;
  end;
  if syslib.display_mode_count() = uint(0) then
  begin
    say("settings: no runtime vbe\n");
    return;
  end;
  status := syslib.set_resolution(uint(800), uint(600));
  if status <> Ok then
    ok := false
  else if syslib.screen_width() <> uint(800) or syslib.screen_height() <> uint(600) then
    ok := false;
  status := syslib.set_resolution(uint(1024), uint(768));
  if status <> Ok then
    ok := false
  else if syslib.screen_width() <> uint(1024) or syslib.screen_height() <> uint(768) then
    ok := false;
  if ok then
    say("settings: ok\n")
  else
    say("settings: failed\n");
end;

procedure shot_mode();
begin
  draw_frame();
  present_frame();
  say("settings: ready\n");
  status := core.sleep(uint(5000));
  status := syslib.window_close(id);
  say("settings: done\n");
end;

// ---- entry point --------------------------------------------------------

procedure main(args: const string[1024]);
begin
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  list_focus := true;
  apply_pressed := false;
  count := 0;
  selected := 0;
  top := 0;
  message := "";
  screen_w := syslib.screen_width();
  screen_h := syslib.screen_height();

  if args = "check" then
  begin
    check_mode();
    return;
  end;

  if screen_w = uint(0) then
  begin
    out.say("settings: no display\n");
    return;
  end;

  if args = "shot" then
  begin
    status := syslib.set_resolution(uint(800), uint(600));
    if status = Ok then
    begin
      screen_w := uint(800);
      screen_h := uint(600);
    end;
  end;

  if screen_w < uint(WindowX + ContentW + 20)
     or screen_h < uint(WindowY + ContentH + 20) then
  begin
    out.say("settings: screen too small\n");
    return;
  end;

  status := syslib.window_open(WindowX, WindowY, ContentW, ContentH, "Settings", id);
  if status <> Ok then
  begin
    out.say("settings: no window\n");
    return;
  end;
  update_modes();
  update_current();
  if args = "shot" then
    shot_mode()
  else
    interactive();
end;

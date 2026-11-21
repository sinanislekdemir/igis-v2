unit display_probe;

// Test-only proof for the runtime display mode path (spec 0022).
//
//   run display_probe
//
// Lists the selectable modes, changes to 800x600, verifies the shadow size,
// returns to 1024x768, verifies again, and prints `display_probe: ok`. The
// kernel transcript is the proof; there is no window and no host fallback.

uses core, syslib, lib.out;

var
  count: uint;
  index: uint;
  width: uint;
  height: uint;
  status: Status;
  text: string[32];
  written: int;
  ok: bool;

// Decimal integer without a sign or padding.
procedure say_int(value: uint);
begin
  written := core.format_int_into(text, int(value), uint(10), uint(0));
  out.say(text);
end;

procedure main(args: const string[1024]);
begin
  count := syslib.display_mode_count();
  out.say("display_probe: ");
  say_int(count);
  out.say(" modes\n");
  if count = uint(0) then
  begin
    out.say("display_probe: no runtime vbe\n");
    return;
  end;

  index := uint(0);
  while index < count do
  begin
    width := syslib.display_mode_width(index);
    height := syslib.display_mode_height(index);
    out.say("display_probe:   ");
    say_int(width);
    out.say("x");
    say_int(height);
    out.say("\n");
    index := index + uint(1);
  end;

  ok := true;
  status := syslib.set_resolution(uint(800), uint(600));
  if status <> Ok then
  begin
    ok := false;
    out.say("display_probe: set 800x600 failed\n");
  end
  else if syslib.screen_width() <> uint(800) or syslib.screen_height() <> uint(600) then
  begin
    ok := false;
    out.say("display_probe: 800x600 size mismatch\n");
  end;

  status := syslib.set_resolution(uint(1024), uint(768));
  if status <> Ok then
  begin
    ok := false;
    out.say("display_probe: restore failed\n");
  end
  else if syslib.screen_width() <> uint(1024) or syslib.screen_height() <> uint(768) then
  begin
    ok := false;
    out.say("display_probe: 1024x768 size mismatch\n");
  end;

  if ok then
    out.say("display_probe: ok\n")
  else
    out.say("display_probe: failed\n");
end;

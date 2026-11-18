unit text_edit;

// M2.4.1 proof: the stdlib string editing helpers on ASCII and multi-byte
// UTF-8. Every mutator is checked for its status, its resulting text (byte
// equality) and, where useful, its resulting length; cursor and selection
// checks cover the clamping rule and code-point boundaries.
//
//   run text_edit
//
// The sample is deterministic and uses no graphics, so the same source runs
// on `langhost` (the e2e test asserts this transcript byte for byte).

uses core, stdlib, lib.out;

var
  text: string[256];
  code: string[256];
  full: string[256];
  big: string[256];
  cursor: int;
  at: int;
  start: int;
  stop: int;
  length: int;
  count: int;
  st: Status;
  ok: bool;
  failed: bool;

procedure report(name: const string[0]; passed: bool);
begin
  if passed then
  begin
    out.say(name);
    out.say(": ok\n");
  end
  else
  begin
    failed := true;
    out.say("text_edit: FAILED ");
    out.say(name);
    out.say("\n");
  end;
end;

// 256 ASCII 'a's: the capacity boundary for insert and replace.
procedure fill_full();
var
  i: int;
  raw: bytes[256];
  copied: int;
begin
  i := 0;
  while i < 256 do
  begin
    raw[i] := byte(97);
    i := i + 1;
  end;
  copied := core.substr_into(full, raw, 0, 256);
  report("fill full", copied = 256 and int(core.len(full)) = 256);
end;

// 256 ASCII 'b's, used as an oversized replacement.
procedure fill_big();
var
  i: int;
  raw: bytes[256];
  copied: int;
begin
  i := 0;
  while i < 256 do
  begin
    raw[i] := byte(98);
    i := i + 1;
  end;
  copied := core.substr_into(big, raw, 0, 256);
  report("fill big", copied = 256 and int(core.len(big)) = 256);
end;

// ---- cursors -----------------------------------------------------------

procedure check_cursors();
begin
  code := "aé€😀"; // bytes: a 0, é 1..2, € 3..5, 😀 6..9

  cursor := 10;
  cursor := stdlib.str_cursor_left(code, cursor);
  ok := cursor = 6;
  cursor := stdlib.str_cursor_left(code, cursor);
  ok := ok and cursor = 3;
  cursor := stdlib.str_cursor_left(code, cursor);
  ok := ok and cursor = 1;
  cursor := stdlib.str_cursor_left(code, cursor);
  ok := ok and cursor = 0;
  report("cursor left widths", ok);

  cursor := stdlib.str_cursor_left(code, cursor);
  report("cursor left at zero", cursor = 0);

  cursor := 0;
  cursor := stdlib.str_cursor_right(code, cursor);
  ok := cursor = 1;
  cursor := stdlib.str_cursor_right(code, cursor);
  ok := ok and cursor = 3;
  cursor := stdlib.str_cursor_right(code, cursor);
  ok := ok and cursor = 6;
  cursor := stdlib.str_cursor_right(code, cursor);
  ok := ok and cursor = 10;
  report("cursor right widths", ok);

  cursor := stdlib.str_cursor_right(code, cursor);
  report("cursor right at end", cursor = 10);

  report("clamp inside 3-byte", stdlib.str_cursor_clamp(code, 5) = 3);
  report("clamp inside 2-byte", stdlib.str_cursor_clamp(code, 2) = 1);
  report("clamp inside 4-byte", stdlib.str_cursor_clamp(code, 9) = 6);
  report("clamp negative", stdlib.str_cursor_clamp(code, -4) = 0);
  report("clamp past end", stdlib.str_cursor_clamp(code, 99) = 10);

  cursor := 7; // inside the 4-byte code point
  cursor := stdlib.str_cursor_left(code, cursor);
  report("left from inside", cursor = 3);

  cursor := 7;
  cursor := stdlib.str_cursor_right(code, cursor);
  report("right from inside", cursor = 10);

  cursor := 6;
  cursor := stdlib.str_cursor_home(cursor);
  report("cursor home", cursor = 0);

  cursor := stdlib.str_cursor_end(code, cursor);
  report("cursor end", cursor = 10 and int(core.len(code)) = 10);
end;

// ---- insertion ---------------------------------------------------------

procedure check_insert();
begin
  text := "abc";
  st := stdlib.str_insert_into(text, 0, "X");
  report("insert at start", st = Ok and text = "Xabc");

  length := int(core.len(text));
  st := stdlib.str_insert_into(text, length, "Y");
  report("insert at end", st = Ok and text = "XabcY");

  st := stdlib.str_insert_into(text, 1, "é");
  report("insert multibyte", st = Ok and text = "XéabcY" and stdlib.utf8_len(text) = 6);

  text := "aé";
  st := stdlib.str_insert_into(text, 1, "Z");
  report("insert before multibyte", st = Ok and text = "aZé");

  text := "aé€";
  st := stdlib.str_insert_into(text, 2, "x");
  report("insert non-boundary", st = Invalid and text = "aé€");

  st := stdlib.str_insert_into(text, -1, "x");
  report("insert negative", st = Invalid and text = "aé€");

  st := stdlib.str_insert_into(text, 7, "x");
  report("insert past end", st = Invalid and text = "aé€");

  st := stdlib.str_insert_into(text, 6, "");
  report("insert empty", st = Ok and text = "aé€");

  fill_full();
  st := stdlib.str_insert_into(full, 0, "z");
  report(
    "insert capacity",
    st = CapacityExceeded
      and int(core.len(full)) = 256
      and full[0] = byte(97)
      and full[255] = byte(97));

  st := stdlib.str_insert_into(full, 256, "z");
  report("insert capacity at end", st = CapacityExceeded and int(core.len(full)) = 256);
end;

// ---- deletion ----------------------------------------------------------

procedure check_delete();
begin
  text := "aé€😀b";
  st := stdlib.str_delete_into(text, 1, 2);
  report("delete 2-byte", st = Ok and text = "a€😀b");

  st := stdlib.str_delete_into(text, 1, 3);
  report("delete 3-byte", st = Ok and text = "a😀b");

  st := stdlib.str_delete_into(text, 1, 4);
  report("delete 4-byte", st = Ok and text = "ab");

  st := stdlib.str_delete_into(text, 0, 1);
  report("delete at start", st = Ok and text = "b");

  st := stdlib.str_delete_into(text, 1, 0);
  report("delete zero", st = Ok and text = "b");

  text := "aé";
  st := stdlib.str_delete_into(text, 2, 1);
  report("delete non-boundary start", st = Invalid and text = "aé");

  st := stdlib.str_delete_into(text, 1, 1);
  report("delete non-boundary end", st = Invalid and text = "aé");

  st := stdlib.str_delete_into(text, 0, -1);
  report("delete negative", st = Invalid and text = "aé");

  st := stdlib.str_delete_into(text, 0, 4);
  report("delete past end", st = Invalid and text = "aé");

  st := stdlib.str_delete_into(text, 4, 0);
  report("delete past end position", st = Invalid and text = "aé");

  text := "éa";
  st := stdlib.str_delete_into(text, 0, 2);
  report("delete whole first point", st = Ok and text = "a");

  text := "aé€😀";
  st := stdlib.str_delete_into(text, 0, int(core.len(text)));
  report("delete all", st = Ok and int(core.len(text)) = 0 and text = "");
end;

// ---- backspace ---------------------------------------------------------

procedure check_backspace();
begin
  text := "aéb";
  cursor := 3;
  st := stdlib.str_backspace(text, cursor);
  report("backspace 2-byte", st = Ok and text = "ab" and cursor = 1);

  st := stdlib.str_backspace(text, cursor);
  report("backspace to start", st = Ok and text = "b" and cursor = 0);

  st := stdlib.str_backspace(text, cursor);
  report("backspace at zero", st = Ok and text = "b" and cursor = 0);

  text := "x😀";
  cursor := 5;
  st := stdlib.str_backspace(text, cursor);
  report("backspace 4-byte", st = Ok and text = "x" and cursor = 1);

  text := "éa";
  cursor := 1; // inside the 2-byte code point; the clamp lands on 0
  st := stdlib.str_backspace(text, cursor);
  report("backspace clamped no-op", st = Ok and text = "éa" and cursor = 0);

  text := "abc";
  cursor := 99;
  st := stdlib.str_backspace(text, cursor);
  report("backspace past end", st = Ok and text = "ab" and cursor = 2);
end;

// ---- selection ---------------------------------------------------------

procedure check_selection();
begin
  code := "aé€😀";

  start := 5; // inside €
  stop := 2; // inside é
  count := stdlib.str_selection_normalize(code, start, stop);
  report(
    "selection normalize",
    count = 2 and start = 1 and stop = 3);

  count := stdlib.str_selection_all(code, start, stop);
  report("selection all", count = 10 and start = 0 and stop = 10);

  start := 3;
  stop := 6;
  cursor := stdlib.str_selection_collapse(code, start, stop, false);
  report("collapse to start", cursor = 3 and start = 3 and stop = 3);

  start := 3;
  stop := 6;
  cursor := stdlib.str_selection_collapse(code, start, stop, true);
  report("collapse to stop", cursor = 6 and start = 6 and stop = 6);

  text := "aé€😀";
  start := 1;
  stop := 3;
  st := stdlib.str_selection_replace(text, start, stop, "X");
  report("replace 2-byte", st = Ok and text = "aX€😀" and start = 2 and stop = 2);

  st := stdlib.str_selection_replace(text, start, stop, "é");
  report("replace collapsed", st = Ok and text = "aXé€😀" and start = 4 and stop = 4);

  start := 6; // inside €, normalizes to 4
  stop := 2; // start of é, stays 2
  st := stdlib.str_selection_replace(text, start, stop, "e");
  report("replace normalized pair", st = Ok and text = "aXe€😀" and start = 3 and stop = 3);

  start := 1;
  stop := 2;
  st := stdlib.str_selection_replace(text, start, stop, "");
  report("replace with empty", st = Ok and text = "ae€😀" and start = 1 and stop = 1);

  text := "aé";
  start := 0;
  stop := 1;
  st := stdlib.str_selection_replace(text, start, stop, "€");
  report("replace multibyte", st = Ok and text = "€é" and start = 3 and stop = 3);

  fill_full();
  fill_big();
  start := 0;
  stop := 1;
  st := stdlib.str_selection_replace(full, start, stop, big);
  report(
    "replace capacity",
    st = CapacityExceeded
      and int(core.len(full)) = 256
      and full[0] = byte(97)
      and start = 0
      and stop = 1);

  start := 0;
  stop := 2;
  text := full;
  st := stdlib.str_selection_replace(text, start, stop, "");
  report("replace shrinks", st = Ok and int(core.len(text)) = 254 and start = 0 and stop = 0);
end;

procedure main(args: const string[1024]);
begin
  failed := false;
  check_cursors();
  check_insert();
  check_delete();
  check_backspace();
  check_selection();
  if failed then
    out.say("text_edit: failed\n")
  else
    out.say("text_edit: ok\n");
end;

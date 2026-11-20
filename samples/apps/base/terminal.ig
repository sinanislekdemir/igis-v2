unit terminal;

// M11 terminal emulator (spec 0021). This is an ordinary GUI application, not
// a kernel console: it owns the visible grid, opens a console session, spawns
// `base/shell` into it, paints the bytes the shell writes, and sends keystrokes
// back. Each instance has its own session, shell, cwd and screen.
//
//   run terminal
//
// There is no scrollback: a full row wraps, a newline at the bottom scrolls the
// visible grid one line and drops the oldest line. A close box kills the shell,
// closes the session and closes this window; a shell `exit` closes the window
// after the last bytes are painted.

uses core, stdlib, syslib, lib.out;

const
  TermW = 640;
  TermH = 400;
  CellW = 8;
  CellH = 8;
  MaxCols = 80;
  MaxRows = 48;
  MaxLine = 256;
  ChunkBytes = 256;

var
  term: int;
  rect: Rect;
  st: Status;
  ev: Event;
  diag: Diagnostic;
  reason: ExitReason;
  session: int;
  shell_task: int;
  running: bool;
  session_closed: bool;
  cols: int;
  rows: int;
  row: int;
  col: int;
  i: int;
  n: int;
  count: int;
  id: int;
  r: Rect;
  title: string[64];
  lines: array[string[MaxLine], MaxRows];
  chunk: bytes[ChunkBytes];
  read: int;
  sources: array[uint, 8];
  ready: int;
  sent: int;
  split: int;
  b: byte;
  mods: int;
  key: int;
  raw: bytes[1];
  ch: string[2];
  more: bool;
  screen_w: int;
  screen_h: int;
  x: int;
  y: int;
  white: uint;
  black: uint;

// Erases the whole content and paints every visible row, then the cursor
// underline. The content rectangle is re-read on every call because a drag or
// a move changes the origin (spec 0011, W28).
procedure draw_screen();
begin
  if term <= 0 then
    return;
  st := syslib.window_content_rect(term, rect);
  if st <> Ok then
    return;
  st := syslib.fill_rect(rect.x, rect.y, rect.w, rect.h, white);
  i := 0;
  while i < rows do
  begin
    st := syslib.draw_text(rect.x, rect.y + i * CellH, lines[i], black);
    i := i + 1;
  end;
  if row >= 0 and row < rows and col >= 0 and col < cols then
  begin
    st := syslib.fill_rect(
      rect.x + col * CellW, rect.y + row * CellH + CellH - 2, CellW - 2, 2, black);
  end;
  st := syslib.window_present(term);
end;

procedure scroll_one();
begin
  i := 0;
  while i + 1 < rows do
  begin
    lines[i] := lines[i + 1];
    i := i + 1;
  end;
  if rows > 0 then
    lines[rows - 1] := "";
  row := rows - 1;
  col := 0;
end;

procedure new_line();
begin
  col := 0;
  if row + 1 < rows then
    row := row + 1
  else
    scroll_one();
end;

procedure put_char(value: byte);
begin
  if cols <= 0 or rows <= 0 then
    return;
  if col >= cols then
    new_line();
  if row < 0 or row >= rows then
    return;
  raw[0] := value;
  split := core.substr_into(ch, raw, 0, 1);
  if split = 1 then
  begin
    st := stdlib.str_insert_into(lines[row], col, ch);
    if st = Ok then
      col := col + 1;
  end;
end;

procedure backspace();
begin
  if col > 0 then
    st := stdlib.str_backspace(lines[row], col);
end;

procedure tab();
begin
  while col mod 8 <> 0 do
    put_char(byte(32));
end;

procedure feed_byte(value: byte);
begin
  if value = byte(13) then
    col := 0
  else if value = byte(10) then
    new_line()
  else if value = byte(8) then
    backspace()
  else if value = byte(9) then
    tab()
  else if value >= byte(32) and value <= byte(126) then
    put_char(value);
end;

procedure feed_chunk();
begin
  i := 0;
  while i < read do
  begin
    feed_byte(chunk[i]);
    i := i + 1;
  end;
end;

procedure drain_session();
begin
  more := true;
  while more and running do
  begin
    st := core.session_recv(session, chunk, read);
    if st = Ok then
    begin
      if read > 0 then
        feed_chunk()
      else
        more := false;
    end
    else
    begin
      if st = Closed then
      begin
        session_closed := true;
        running := false;
      end;
      more := false;
    end;
  end;
  draw_screen();
end;

procedure send_bytes(text: const string[0]);
begin
  st := core.session_send(session, text, sent);
end;

procedure send_byte(value: byte);
begin
  raw[0] := value;
  split := core.substr_into(ch, raw, 0, 1);
  if split = 1 then
    send_bytes(ch);
end;

procedure handle_key(event: const Event);
begin
  if event.kind <> KeyDown then
    return;
  if stdlib.ui_key_is_enter(event.code) then
    send_bytes("\r")
  else if stdlib.ui_key_is_backspace(event.code) then
    send_bytes("\x7f")
  else if stdlib.ui_key_is_tab(event.code) then
    send_bytes("\t")
  else
  begin
    mods := stdlib.ui_key_modifiers(event.code);
    key := stdlib.ui_key_code(event.code);
    if (mods div 2) mod 2 = 1 and key = 67 then
      send_bytes("\x03")
    else
    begin
      b := stdlib.ui_key_text(event.code);
      if b >= byte(32) and b <= byte(126) then
        send_byte(b);
    end;
  end;
end;

procedure handle_event();
begin
  if ev.kind = Expose then
  begin
    if ev.window = term then
      draw_screen();
  end
  else if ev.kind = Close then
  begin
    if ev.window = term then
      running := false;
  end
  else if ev.kind = KeyDown then
    handle_key(ev);
end;

function count_terminals(): int;
begin
  count := 0;
  i := 0;
  st := syslib.window_info(uint(i), id, r, title);
  while st = Ok and i < 32 do
  begin
    if title = "Terminal" then
      count := count + 1;
    i := i + 1;
    st := syslib.window_info(uint(i), id, r, title);
  end;
  return count;
end;

procedure main(args: const string[1024]);
begin
  white := uint(0xFFFFFF);
  black := uint(0x000000);
  screen_w := int(syslib.screen_width());
  screen_h := int(syslib.screen_height());
  term := 0;
  session := 0;
  shell_task := 0;
  running := false;
  session_closed := false;

  n := count_terminals();
  x := 24 + n * 28;
  y := 64 + n * 28;
  if x + TermW + 8 > screen_w then
    x := 24;
  if y + TermH + 8 > screen_h then
    y := 64;
  st := syslib.window_open(x, y, TermW, TermH, "Terminal", term);
  if st <> Ok then
  begin
    out.say("terminal: window open failed\n");
    return;
  end;

  st := core.session_open(session);
  if st <> Ok then
  begin
    out.say("terminal: session open failed\n");
    st := syslib.window_close(term);
    return;
  end;

  st := core.spawn("shell", "", diag, shell_task);
  if st <> Ok then
  begin
    out.say("terminal: shell spawn failed\n");
    st := core.session_close(session);
    st := syslib.window_close(term);
    return;
  end;

  st := syslib.window_content_rect(term, rect);
  cols := rect.w div CellW;
  rows := rect.h div CellH;
  if cols > MaxCols then
    cols := MaxCols;
  if rows > MaxRows then
    rows := MaxRows;
  if cols < 1 or rows < 1 then
  begin
    st := core.kill(shell_task);
    st := core.session_close(session);
    st := syslib.window_close(term);
    out.say("terminal: window too small\n");
    return;
  end;
  i := 0;
  while i < MaxRows do
  begin
    lines[i] := "";
    i := i + 1;
  end;
  row := 0;
  col := 0;
  draw_screen();

  sources[0] := core.session_source(session);
  sources[1] := uint(1);
  running := true;
  while running do
  begin
    ready := -1;
    st := core.next_any(sources, 2, ready);
    if st = Ok then
    begin
      if ready = 0 then
        drain_session()
      else
      begin
        st := syslib.next_event(ev);
        if st = Ok then
          handle_event();
      end;
    end;
  end;

  if not session_closed then
    st := core.kill(shell_task);
  st := core.session_close(session);
  if term > 0 then
    st := syslib.window_close(term);
end;

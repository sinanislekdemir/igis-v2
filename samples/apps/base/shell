unit shell;

uses core, stdlib, syslib, lib.out, lib.path, lib.cli;

const
  ExternalArgsCapacity = 512;

var
  banner: string[32];
  line: string[256];
  cmd: string[64];
  rest: string[256];
  path: string[256];
  argv: string[256];
  // The shell's current directory in display form (`/`, `/base`, `/home/sub`).
  // `target` is the API path a `cd` resolves to; `app_path` and `external_args`
  // stage a bare command's launch. The file commands receive
  // `"<cwd display> <rest>"` (lib/path.ig `split_cwd`); other apps get `rest`.
  cwd: string[256];
  cd_arg: string[256];
  target: string[256];
  app_path: string[256];
  external_args: string[512];
  listed_name: string[64];
  listed_kind: uint;
  listed_size: uint;
  // True when the staged external argument string does not fit.
  too_long: bool;
  // Set by whichever builtin handled this line. The dispatch is then a
  // straight-line chain instead of a deep `else if` nest, which keeps the
  // compiler's recursive codegen within the host tests' stack.
  handled: bool;
  st: Status;
  id: int;
  space: int;
  split: int;
  done: bool;
  diag: Diagnostic;
  reason: ExitReason;
  term: int;
  term_status: Status;
  term_rect: Rect;
  found_white: bool;
  found_ink: bool;
  px: int;
  py: int;
  // Console input is a raw byte stream (spec 0005, ADR 0037). The shell keeps
  // the line buffer, the cursor and the echo itself; `one` is the one-byte
  // read destination so a burst of typed bytes stays in the kernel ring until
  // each byte is consumed.
  one: bytes[1];
  ch: string[2];
  cursor: int;
  ready: int;
  sources: array[uint, 8];
  got: int;
  submitted: bool;
  cancelled: bool;
  ev: Event;
  edit_st: Status;

// Inserts one printable byte at the cursor through the M2.4.1 helpers, so the
// line stays valid UTF-8 and is published atomically. The echo is one cell,
// and the cursor only ever sits at the end (see `handle_byte`).
procedure insert_byte(b: byte);
begin
  one[0] := b;
  split := core.substr_into(ch, one, 0, 1);
  if split = 1 then
  begin
    edit_st := stdlib.str_insert_into(line, cursor, ch);
    if edit_st = Ok then
    begin
      cursor := cursor + 1;
      out.say(ch);
    end;
  end;
end;

// Applies one input byte exactly once, whether it came from the serial byte
// stream or from a key event's text byte. Supported: printable ASCII inserts,
// Backspace/0x7F deletes the last code point, CR/LF submits, Ctrl+C cancels,
// and Tab inserts one space (the shell does not expand tab stops). Every other
// byte, and every non-text key except Enter/Backspace, is ignored.
procedure handle_byte(b: byte);
var
  before: int;
begin
  if b = byte(13) or b = byte(10) then
  begin
    out.say("\n");
    submitted := true;
  end
  else if b = byte(127) or b = byte(8) then
  begin
    before := int(core.len(line));
    edit_st := stdlib.str_backspace(line, cursor);
    if int(core.len(line)) < before then
      out.say("\x08 \x08");
  end
  else if b = byte(3) then
  begin
    out.say("^C\n");
    line := "";
    cursor := 0;
    cancelled := true;
  end
  else if b = byte(9) then
    insert_byte(byte(32))
  else if b >= byte(32) and b <= byte(126) then
    insert_byte(b);
end;

// One key event: special keys first (their text byte is 0), then the text byte
// through the same editor. A KeyUp is ignored.
procedure handle_key(key: const Event);
begin
  if key.kind = KeyDown then
  begin
    if stdlib.ui_key_is_enter(key.code) then
      handle_byte(byte(13))
    else if stdlib.ui_key_is_backspace(key.code) then
      handle_byte(byte(127))
    else
      handle_byte(stdlib.ui_key_text(key.code));
  end;
end;

// Reports the child's `ExitReason` sum value (spec 0003, "Sum types"). `case`
// is exhaustive over the three variants and binds `Faulted`'s payload to a
// `FaultCode` local; `FaultCode`'s variant names are qualified because some of
// them collide with `Status` (spec 0003, "Predeclared types").
procedure report(why: const ExitReason);
begin
  case why of
    Ok: out.say("child: ok\n")
    Killed: out.say("child: killed\n")
    Faulted(code):
    begin
      out.say("child: faulted ");
      if code = FaultCode.DivideByZero then
        out.say("DivideByZero")
      else if code = FaultCode.Overflow then
        out.say("Overflow")
      else if code = FaultCode.CapacityExceeded then
        out.say("CapacityExceeded")
      else if code = FaultCode.StackOverflow then
        out.say("StackOverflow")
      else if code = FaultCode.WatchdogTimeout then
        out.say("WatchdogTimeout")
      else if code = FaultCode.IoError then
        out.say("IoError")
      else
        out.say("other");
      out.say("\n");
    end;
  end;
end;

// Reports a bare command's non-Ok termination. `run` keeps `report`; a bare
// command prints only its own output when it exits `Ok` and names itself
// otherwise (`<cmd>: killed`, `<cmd>: faulted <code>`).
procedure report_app(name: const string[0]; why: const ExitReason);
begin
  case why of
    Ok: begin end
    Killed:
    begin
      out.say(name);
      out.say(": killed\n");
    end;
    Faulted(code):
    begin
      out.say(name);
      out.say(": faulted ");
      if code = FaultCode.DivideByZero then
        out.say("DivideByZero")
      else if code = FaultCode.Overflow then
        out.say("Overflow")
      else if code = FaultCode.CapacityExceeded then
        out.say("CapacityExceeded")
      else if code = FaultCode.StackOverflow then
        out.say("StackOverflow")
      else if code = FaultCode.WatchdogTimeout then
        out.say("WatchdogTimeout")
      else if code = FaultCode.IoError then
        out.say("IoError")
      else
        out.say("other");
      out.say("\n");
    end;
  end;
end;

// Adopts the resolved API path in `target` as the current directory: the prompt
// shows its display form. `display` needs one byte more than the API path, so a
// 256-byte target is refused explicitly instead of leaving `cwd` stale.
procedure enter();
var
  shown: Status;
begin
  shown := path.display(target, cwd);
  if shown <> Ok then
  begin
    out.say("cd: ");
    cli.status_name(shown);
    out.say("\n");
  end;
end;

// `cd [dir]`: an empty argument means `~` (home); otherwise `lib.path.join`
// resolves `/`, `~`, `.`, `..` and relative names against `cwd`. Only a
// directory is accepted. There is no stat primitive, so a directory is proved
// two ways: `core.fs_list` succeeding at index 0 is a non-empty directory, and
// `fs_list` failing with `fs_size` returning `Invalid` is an empty one. A
// regular file is `Invalid` from `fs_list` and `Ok` from `fs_size`; a missing
// name is `NotFound` from both (spec 0013, F7/F13; spec 0019, A4).
// The file commands need the current directory in their first argument; every
// other application receives the raw tail exactly as typed. This keeps `nc`,
// `purl`, `text`, `child` and the rest compatible with their original `run`
// interfaces while the file commands can resolve relative paths.
function is_file_command(name: const string[0]): bool;
begin
  return name = "ls" or name = "dir" or name = "pwd" or name = "cat"
    or name = "head" or name = "tail" or name = "mkdir"
    or name = "rmdir" or name = "rm";
end;

// Stages the argument string for a bare app. File commands get
// `"<cwd> <rest>"`; every other app gets the raw tail. The procedure keeps this
// branch out of `main`, whose body is already large enough to stress the host
// compiler's test stack.
procedure prepare_args(name: const string[0]; tail: const string[0]);
var
  copied: int;
begin
  if is_file_command(name) then
  begin
    if int(core.len(cwd)) + 1 + int(core.len(tail)) > ExternalArgsCapacity then
      too_long := true
    else
    begin
      external_args := cwd + " " + tail;
      too_long := false;
    end;
  end
  else
  begin
    if int(core.len(tail)) > ExternalArgsCapacity then
      too_long := true
    else
    begin
      copied := core.substr_into(external_args, tail, 0, int(core.len(tail)));
      too_long := copied < 0;
    end;
  end;
end;

procedure change_dir(arg: const string[0]);
var
  join_st: Status;
  list_st: Status;
  size_st: Status;
begin
  join_st := path.join(cwd, arg, target);
  if join_st <> Ok then
  begin
    out.say("cd: ");
    cli.status_name(join_st);
    out.say("\n");
    return;
  end;

  list_st := core.fs_list(target, uint(0), listed_name, listed_kind, listed_size);
  if list_st = Ok then
  begin
    enter();
    return;
  end;

  size_st := core.fs_size(target, listed_size);
  if size_st = Ok then
  begin
    out.say("cd: not a directory\n");
    return;
  end;
  if size_st = Invalid then
  begin
    enter();
    return;
  end;
  if size_st = NotFound then
  begin
    out.say("cd: no such file or directory\n");
    return;
  end;
  out.say("cd: ");
  cli.status_name(size_st);
  out.say("\n");
end;

procedure main(args: const string[1024]);
begin
  // The REPL's terminal is an ordinary task-owned window (spec 0014). Open it
  // and attach the console's terminal grid before the first banner byte; when
  // graphics are disabled or the attach is refused the shell keeps the serial
  // path and prints one line explaining it.
  term := 0;
  term_status := Status.Invalid;
  st := syslib.window_open(24, 64, 560, 360, "Terminal", term);
  if st = Ok then
    term_status := core.terminal_attach(term);
  if term_status <> Ok then
  begin
    if term <> 0 then
      st := syslib.window_close(term);
    out.say("shell: no terminal window\n");
  end;

  banner := "igis-v2 shell\n";
  out.say(banner);
  out.say("shell: spawning child\n");

  // `core.spawn` returns `Status` and fills the caller's `Diagnostic` record on
  // a synchronous failure (spec 0003 E1, spec 0005). `core.wait` blocks until
  // the child exits and writes its `ExitReason`.
  st := core.spawn("child", "hello", diag, id);
  if st = Ok then
  begin
    out.say("shell: child task id assigned\n");
    st := core.wait(id, reason);
    if st = Ok then
      report(reason)
    else
      out.say("child: wait failed\n");
  end
  else
  begin
    out.say("spawn failed: ");
    out.say(diag.message);
    out.say("\n");
  end;

  // Command loop (spec 0005, ADR 0037). The console is a raw byte stream and
  // the kernel neither echoes nor assembles lines, so this loop does the line
  // editing: it waits on console bytes (source 0) and input events (source 1)
  // at once, applies one byte through `handle_byte`, and dispatches on Enter.
  // `core.find` locates the first space and `core.substr_into` splits the line
  // into the command and its tail.
  //
  // The builtins are `help`, `echo`, `cd`, `run`, `exit` and `terminal`; any
  // other word is an application under `base/` (then `system/`). The file
  // commands receive `"<cwd> <rest>"`; every other app receives the raw tail
  // exactly as typed, preserving its original `run` interface. The prompt shows
  // the current directory, which starts at the image root.
  //
  // There is no `return` yet, so `main` ends by leaving the loop with `done`.
  sources[0] := uint(0);
  sources[1] := uint(1);
  // The shell starts at the image root; the prompt shows it in display form
  // (`igis:/> `).
  cwd := "/";
  done := false;
  while not done do
  begin
    line := "";
    cursor := 0;
    submitted := false;
    cancelled := false;
    out.say("igis:");
    out.say(cwd);
    out.say("> ");
    while not submitted and not cancelled do
    begin
      ready := -1;
      st := core.next_any(sources, 2, ready);
      if st = Ok then
      begin
        if ready = 0 then
        begin
          got := core.read_input(one);
          if got = 1 then
            handle_byte(one[0]);
        end
        else
        begin
          st := syslib.next_event(ev);
          if st = Ok then
          begin
            if ev.kind = Close and ev.window = term then
            begin
              // A close-box press is an owner request, not a kill (spec 0011,
              // W23). Close our own terminal window and leave both loops.
              done := true;
              cancelled := true;
            end
            else
              handle_key(ev);
          end;
        end;
      end;
    end;
    if submitted then
    begin
    space := core.find(line, byte(32));
    if space < 0 then
    begin
      split := core.substr_into(cmd, line, 0, int(core.len(line)));
      split := core.substr_into(rest, line, int(core.len(line)), 0);
    end
    else
    begin
      split := core.substr_into(cmd, line, 0, space);
      split := core.substr_into(rest, line, space + 1, int(core.len(line)) - space - 1);
    end;

    handled := false;
    if cmd = "help" then
    begin
      out.say("shell: help, echo <text>, cd [dir], run <app> [args], exit\n");
      out.say("shell: base apps: ls dir pwd cat head tail mkdir rmdir rm\n");
      handled := true;
    end;
    if cmd = "echo" then
    begin
      out.say(rest);
      out.say("\n");
      handled := true;
    end;
    if cmd = "cd" then
    begin
      // `cd` with no argument (or only spaces) means `~`, the home directory.
      split := stdlib.trim_into(cd_arg, rest);
      if int(core.len(cd_arg)) = 0 then
        cd_arg := "~";
      change_dir(cd_arg);
      handled := true;
    end;
    if cmd = "run" then
    begin
      // `rest` is `<app> [args]`; split its tail at the first space so the
      // kernel receives an unparsed application path and argument string.
      split := core.find(rest, byte(32));
      if split < 0 then
      begin
        split := core.substr_into(path, rest, 0, int(core.len(rest)));
        split := core.substr_into(argv, rest, int(core.len(rest)), 0);
      end
      else
      begin
        split := core.substr_into(path, rest, 0, split);
        split := core.substr_into(argv, rest, split + 1, int(core.len(rest)) - split - 1);
      end;

      if core.len(path) = uint(0) then
        out.say("usage: run <app> [args]\n")
      else
      begin
        // `core.run` is spawn + wait in one primitive. It blocks the shell
        // until the child terminates and writes the child's `ExitReason`.
        st := core.run(path, argv, diag, reason);
        if st = Ok then
        begin
          // On a compile error the service discovers after `spawn` returned,
          // `run` also fills `diag` from the exit record (spec 0005).
          if core.len(diag.message) > uint(0) then
          begin
            out.say("child: ");
            out.say(diag.message);
            out.say("\n");
          end;
          report(reason)
        end
        else
        begin
          out.say("run failed: ");
          out.say(diag.message);
          out.say("\n");
        end;
      end;
      handled := true;
    end;
    if cmd = "terminal" then
    begin
      // Terminal-window proof (spec 0014): report the attached id, check a
      // refused unknown id, verify a foreground and a background pixel in the
      // window's content rectangle, then spawn a probe that must be refused
      // with Full while this shell holds the one terminal.
      if term_status = Ok then
      begin
        out.say("terminal: window ");
        split := core.format_int_into(rest, term, uint(10), uint(0));
        out.say(rest);
        out.say("\n");

        st := core.terminal_attach(999999);
        if st = NotFound then
          out.say("terminal: unknown refused\n")
        else
          out.say("terminal: unknown BAD\n");

        st := syslib.window_content_rect(term, term_rect);
        if st = Ok then
        begin
          found_white := false;
          found_ink := false;
          py := 0;
          while py < 16 do
          begin
            px := 0;
            while px < 128 do
            begin
              if syslib.get_pixel(term_rect.x + px, term_rect.y + py) = uint(0) then
                found_ink := true
              else
                found_white := true;
              px := px + 1;
            end;
            py := py + 1;
          end;
          if found_ink and found_white then
            out.say("terminal: pixels ok\n")
          else
            out.say("terminal: pixels BAD\n");
        end
        else
          out.say("terminal: no rect\n");

        st := core.run("system/terminal_probe", "", diag, reason);
        if st = Ok then
          report(reason)
        else
          out.say("terminal: probe failed\n");
        out.say("terminal: ok\n");
      end
      else
        out.say("terminal: not attached\n");
      handled := true;
    end;
    if cmd = "exit" then
    begin
      done := true;
      handled := true;
    end;
    if not handled then
    begin
      // A bare word that is not a builtin is an application under `base/`,
      // falling back to `system/`. The base commands expect
      // `args = "<cwd display> <rest>"` (lib/path.ig `split_cwd`), so `pwd`
      // prints the directory the prompt shows and `ls`/`cat` resolve their
      // target against it. Only a regular file is runnable; `fs_size` says
      // `Ok` for one and fails for a directory or a missing name.
      app_path := "base/" + cmd;
      st := core.fs_size(app_path, listed_size);
      if st <> Ok then
      begin
        app_path := "system/" + cmd;
        st := core.fs_size(app_path, listed_size);
      end;

      if st <> Ok then
      begin
        out.say("unknown command: ");
        out.say(cmd);
        out.say("\n");
      end
      else
      begin
        prepare_args(cmd, rest);
        if too_long then
        begin
          out.say(cmd);
          out.say(": arguments too long\n");
        end
        else
        begin
          st := core.run(app_path, external_args, diag, reason);
          if st <> Ok then
          begin
            out.say(cmd);
            out.say(": ");
            out.say(diag.message);
            out.say("\n");
          end
          else if core.len(diag.message) > uint(0) then
          begin
            // A compile error the service finds after `spawn` returned reaches
            // `run`'s diagnostic (spec 0005).
            out.say(cmd);
            out.say(": ");
            out.say(diag.message);
            out.say("\n");
          end
          else
            report_app(cmd, reason);
        end;
      end;
    end;
    end;
  end;

  out.say("bye\n");
  if term_status = Ok then
    st := syslib.window_close(term);
end;

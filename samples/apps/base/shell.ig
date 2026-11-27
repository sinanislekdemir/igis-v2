unit shell;

uses core, stdlib, lib.out, lib.path, lib.cli;

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
  // stage a bare launch. A `base/`/`system/` command receives
  // `"<cwd display>\n<rest>"` (lib/path.ig `split_cwd`); an `apps/` GUI app and
  // every `run` keep the raw tail.
  cwd: string[256];
  cd_arg: string[256];
  target: string[256];
  app_path: string[256];
  external_args: string[512];
  app_dirs: array[string[32], 6];
  app_dir: int;
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
// Stages the argument string for a bare launch. `base/` and `system/` entries
// are commands and receive the L11 convention: `"<cwd display>\n<tail>"`. The
// shared parser in `lib/path.split_cwd` splits the context line; commands that
// ignore arguments simply ignore the whole string. A GUI application under
// `apps/<category>/` is not a command and keeps its own raw argument string,
// exactly as with `run`. The decision is the resolved directory, never a list
// of command names. `run <app> [args]` is the explicit raw launcher and
// bypasses this helper, so the image root is its cwd.
procedure prepare_args(tail: const string[0]; command: bool);
var
  copied: int;
begin
  if command then
  begin
    if int(core.len(cwd)) + 1 + int(core.len(tail)) > ExternalArgsCapacity then
      too_long := true
    else
    begin
      external_args := cwd + "\n" + tail;
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
  // M11: the shell is a session shell. The terminal emulator owns the window
  // and the session; this task reads source 0 (its session input, or the
  // global serial ring when unbound) and writes with `core.write` (spec 0021).
  banner := "igis-v2 shell\n";
  out.say(banner);

  // Command loop (spec 0005, ADR 0037). The console is a raw byte stream and
  // the kernel neither echoes nor assembles lines, so this loop does the line
  // editing: it waits on source 0, applies one byte through `handle_byte`, and
  // dispatches on Enter. `core.find` locates the first space and
  // `core.substr_into` splits the line into the command and its tail.
  //
  // The builtins are `help`, `echo`, `cd`, `run`, `exit` and `terminal`; any
  // other word is a command under `base/` (then `system/`). Every bare command
  // receives `"<cwd>\n<rest>"`; `run` is the explicit raw launcher. The prompt
  // shows the current directory, which starts at the image root.
  //
  // There is no `return` yet, so `main` ends by leaving the loop with `done`.
  sources[0] := uint(0);
  app_dirs[0] := "base/";
  app_dirs[1] := "apps/accessories/";
  app_dirs[2] := "apps/games/";
  app_dirs[3] := "apps/productivity/";
  app_dirs[4] := "apps/utilities/";
  app_dirs[5] := "system/";
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
      st := core.next_any(sources, 1, ready);
      if st = Ok then
      begin
        got := core.read_input(one);
        if got = 1 then
          handle_byte(one[0]);
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
      out.say("shell: base apps: ls dir pwd cat head tail mkdir rmdir rm touch ifconfig nslookup\n");
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
      // Console-session proof (spec 0021). The shell is a slave; it checks the
      // stale-id rule, then runs the probe in this same session so the probe's
      // own session must succeed without disturbing the shell.
      st := core.session_close(999999);
      if st = NotFound then
        out.say("terminal: stale refused\n")
      else
        out.say("terminal: stale BAD\n");
      st := core.run("system/terminal_probe", "", diag, reason);
      if st = Ok then
        report(reason)
      else
        out.say("terminal: probe failed\n");
      out.say("terminal: ok\n");
      handled := true;
    end;
    if cmd = "exit" then
    begin
      done := true;
      handled := true;
    end;
    if not handled then
    begin
      // A bare word that is not a builtin is a `base/` command (then an
      // `apps/<category>/` GUI application, then `system/`). Commands receive
      // `args = "<cwd display>\n<rest>"` (lib/path.ig `split_cwd`), so `pwd`
      // prints the directory the prompt shows and `ls`/`cat`/`touch` resolve
      // their target against it. GUI apps keep the raw tail. Only a regular
      // file is runnable; `fs_size` says `Ok` for one and fails for a
      // directory or a missing name.
      // Search the app directories in the spec 0018 A1 order. The first
      // regular file wins; `system/` is last.
      app_dir := 0;
      st := NotFound;
      while app_dir < 6 and st <> Ok do
      begin
        app_path := app_dirs[app_dir] + cmd;
        st := core.fs_size(app_path, listed_size);
        app_dir := app_dir + 1;
      end;

      if st <> Ok then
      begin
        out.say("unknown command: ");
        out.say(cmd);
        out.say("\n");
      end
      else
      begin
        prepare_args(rest, stdlib.starts_with(app_path, "base/")
          or stdlib.starts_with(app_path, "system/"));
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
end;

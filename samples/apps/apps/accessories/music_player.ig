unit music_player;

// PCM WAV player (M21). The companion to spec 0030's `syslib.sound` surface:
// `lib.wav` parses the RIFF header, this application streams the data chunk
// through `syslib.sound_write` and draws a small transport window.
//
//   run music_player                     play the shipped sample
//   run music_player <path>              play one WAV from the image
//   run music_player check               headless: parse the sample, play one
//                                        second, print `music_player: ok`
//
// Accepted files are PCM, 16-bit, stereo, 44.1 kHz or 48 kHz (spec 0030 A8;
// ADR 0064); the kernel does no resampling, so anything else is refused by
// `lib.wav` before a window opens.
//
// The audio path is deliberately chunked: 256-byte `core.fs_read` pieces are
// appended into a 16,384-byte staging buffer, and the staging buffer is handed
// to the blocking `syslib.sound_write` once it is full. The write blocks while
// the 32 KiB kernel ring is full, which paces the loop and leaves the CPU to
// other tasks; between writes the application drains window events and updates
// the progress bar. Pausing closes the stream (the ring is zeroed), keeps the
// unplayed staging bytes and reopens on resume.
//
// Keys: Space pauses/resumes, +/- change the volume, M mutes, Escape or the
// close box ends the window. A finished file stops the stream and keeps the
// window open until it is closed.
//
// `check` is headless: it parses `audio/chopin-nocturne-21.wav`, asserts the
// format, plays one second and prints `music_player: ok`. On `langhost` the
// corpus path is absent (the host tree is `samples/apps`), so it prints
// `music_player: host ok`; a kernel with no HDA device prints
// `music_player: no audio`.

uses core, stdlib, syslib, lib.out, lib.wav;

const
  ModePlay = 0;
  ModeCheck = 1;

  StageCap = 16384;    // one `syslib.sound_write` block (85 ms at 44.1 kHz)
  ChunkCap = 256;      // one `core.fs_read` piece

  WindowX = 300;
  WindowY = 200;
  WindowW = 420;
  WindowH = 120;
  TitleCapacity = 48;

  VolStep = 5;
  DefaultVolume = 80;
  EscKey = 0x100;

  // The headless check plays at most one second of 44.1 kHz stereo s16.
  CheckBytes = 176400;

var
  black: uint;
  white: uint;
  gray: uint;
  accent: uint;

  status: Status;
  ev: Event;
  id: int;

  mode: int;
  path: string[256];
  title: string[48];
  info: wav.Info;

  cursor: uint;
  stage: bytes[StageCap];
  chunk: bytes[ChunkCap];
  got: int;
  digits: string[16];
  error_text: string[64];

  volume: int;
  muted: bool;
  playing: bool;
  paused: bool;
  quit: bool;

// ---- argument and title ---------------------------------------------------

procedure make_title(source: const string[0]);
var
  length: int;
  at: int;
  start: int;
  take: int;
begin
  title := "Music Player";
  length := int(core.len(source));
  at := length - 1;
  while at >= 0 and source[at] <> byte(47) do
    at := at - 1;
  start := at + 1;
  take := length - start;
  if take <= 0 then
    return;
  if take > TitleCapacity then
    take := TitleCapacity;
  got := core.substr_into(title, source, start, take);
  if got <= 0 then
    title := "Music Player";
end;

procedure parse_args(raw: const string[1024]);
var
  trimmed: string[256];
  length: int;
begin
  mode := ModePlay;
  path := "audio/chopin-nocturne-21.wav";
  length := stdlib.trim_into(trimmed, raw);
  if length <= 0 then
    return;
  if length = 5 and stdlib.starts_with(trimmed, "check") then
  begin
    mode := ModeCheck;
    return;
  end;
  got := core.substr_into(path, trimmed, 0, length);
  if got <= 0 then
    path := "audio/chopin-nocturne-21.wav";
end;

// ---- drawing --------------------------------------------------------------

procedure draw();
var
  fill_w: int;
  percent: int;
  width: uint;
begin
  status := syslib.window_begin(id);
  if status <> Ok then
    return;
  status := syslib.fill_rect(0, 0, WindowW, WindowH, black);
  status := syslib.draw_text_prop(12, 8, "Music Player", white);
  status := syslib.draw_text_prop(12, 28, title, gray);

  status := syslib.fill_rect(12, 56, WindowW - 24, 10, gray);
  fill_w := 0;
  percent := 0;
  if info.data_size > uint(0) then
  begin
    fill_w := int((cursor * uint(WindowW - 24)) div info.data_size);
    percent := int((cursor * uint(100)) div info.data_size);
  end;
  if fill_w > 0 then
    status := syslib.fill_rect(12, 56, fill_w, 10, accent);

  got := core.format_int_into(digits, percent, uint(10), uint(0));
  status := syslib.draw_text_prop(12, 72, digits, white);
  width := syslib.text_width_prop(digits);
  status := syslib.draw_text_prop(12 + int(width) + 2, 72, "%", gray);

  if core.len(error_text) > uint(0) then
    status := syslib.draw_text_prop(12, 94, error_text, accent)
  else if not playing then
    status := syslib.draw_text_prop(12, 94, "Finished", white)
  else if paused then
    status := syslib.draw_text_prop(12, 94, "Paused", white)
  else
    status := syslib.draw_text_prop(12, 94, "Playing", white);

  status := syslib.draw_text_prop(230, 72, "Vol", gray);
  got := core.format_int_into(digits, volume, uint(10), uint(0));
  status := syslib.draw_text_prop(256, 72, digits, white);
  if muted then
    status := syslib.draw_text_prop(280, 72, "MUTED", accent);
  status := syslib.window_present(id);
end;

// ---- transport ------------------------------------------------------------

procedure set_volume(value: int);
var
  clamped: int;
begin
  clamped := value;
  if clamped < 0 then
    clamped := 0;
  if clamped > 100 then
    clamped := 100;
  volume := clamped;
  if not muted then
    status := syslib.sound_volume(volume);
  draw();
end;

procedure toggle_mute();
var
  on: int;
begin
  muted := not muted;
  if muted then
    on := 1
  else
    on := 0;
  status := syslib.sound_mute(on);
  draw();
end;

procedure toggle_pause();
begin
  if not playing then
    return;
  if paused then
  begin
    status := syslib.sound_open(int(info.rate), 2, 16);
    if status = Ok then
    begin
      if core.len(stage) > uint(0) then
      begin
        status := syslib.sound_write(stage);
        stage := "";
      end;
      paused := false;
    end;
  end
  else
  begin
    status := syslib.sound_close();
    paused := true;
  end;
  draw();
end;

// Drains the machine-wide queue without blocking. Only events addressed to this
// window are consumed; a close request or Escape ends the player.
procedure pump();
var
  more: bool;
  code: int;
  text: uint;
begin
  more := true;
  while more do
  begin
    status := syslib.poll_event(ev);
    if status <> Ok then
      more := false
    else if ev.window = id then
    begin
      if ev.kind = Close then
      begin
        quit := true;
        more := false;
      end
      else if ev.kind = KeyDown then
      begin
        code := stdlib.ui_key_code(ev.code);
        if code = EscKey then
        begin
          quit := true;
          more := false;
        end
        else
        begin
          text := ev.code mod uint(256);
          if text = uint(32) then
            toggle_pause()
          else if text = uint(43) or text = uint(61) then
            set_volume(volume + VolStep)
          else if text = uint(45) then
            set_volume(volume - VolStep)
          else if text = uint(109) or text = uint(77) then
            toggle_mute();
        end;
      end;
    end;
  end;
end;


// ---- modes ----------------------------------------------------------------

// Waits for the kernel ring to hand its last periods to the device before a
// close, so the tail of the file is not cut (up to 32 KiB, ~0.18 s).
procedure drain();
var
  guard: int;
begin
  guard := 0;
  while syslib.sound_queued() > uint(0) and guard < 20 do
  begin
    core.sleep(uint(20));
    guard := guard + 1;
  end;
end;

procedure check_mode();
var
  played: uint;
  remaining: uint;
  st: Status;
begin
  st := wav.open(path, info);
  if st = NotFound then
  begin
    out.say("music_player: host ok\n");
    return;
  end;
  if st <> Ok then
  begin
    out.say("music_player: check failed (open)\n");
    return;
  end;
  if info.rate <> uint(44100) or info.channels <> uint(2)
     or info.bits <> uint(16) or info.data_size < uint(100000) then
  begin
    out.say("music_player: check failed (format)\n");
    return;
  end;
  // Prove the data path before touching the device; this also runs on
  // `langhost` when the corpus is reachable.
  st := wav.read(path, info, uint(0), chunk, got);
  if st <> Ok or got <> ChunkCap then
  begin
    out.say("music_player: check failed (first ");
    out.say_int(int(st));
    out.say(")\n");
    return;
  end;
  st := syslib.sound_open(int(info.rate), 2, 16);
  if st = Invalid then
  begin
    out.say("music_player: host ok\n");
    return;
  end;
  if st <> Ok then
  begin
    out.say("music_player: no audio\n");
    return;
  end;

  played := uint(0);
  while played < uint(CheckBytes) and played < info.data_size do
  begin
    remaining := info.data_size - played;
    st := core.fs_read(path, info.data_offset + played, chunk, got);
    if st <> Ok then
    begin
      out.say("music_player: check failed (read ");
      out.say_int(int(st));
      out.say(")\n");
      return;
    end;
    if got <= 0 then
    begin
      out.say("music_player: check failed (eof)\n");
      return;
    end;
    if uint(got) > remaining then
      got := int(remaining);
    st := syslib.sound_write(chunk);
    if st <> Ok then
    begin
      out.say("music_player: check failed (write ");
      out.say_int(int(st));
      out.say(")\n");
      return;
    end;
    played := played + uint(got);
  end;
  if played = uint(0) then
  begin
    out.say("music_player: check failed (silent)\n");
    return;
  end;
  drain();
  st := syslib.sound_close();
  out.say("music_player: ok\n");
end;

procedure wait_close();
begin
  draw();
  while not quit do
  begin
    core.sleep(uint(50));
    pump();
  end;
end;

procedure play_file();
begin
  stage := "";
  cursor := uint(0);
  error_text := "";
  status := wav.open(path, info);
  if status <> Ok then
  begin
    out.say("music_player: cannot open\n");
    error_text := "Cannot open file";
    playing := false;
    wait_close();
    return;
  end;
  status := syslib.sound_open(int(info.rate), 2, 16);
  if status <> Ok then
  begin
    out.say("music_player: no audio\n");
    if status = Full then
      error_text := "Audio busy"
    else if status = IoError then
      error_text := "No audio device"
    else
      error_text := "Cannot open audio";
    playing := false;
    wait_close();
    return;
  end;
  status := syslib.sound_volume(volume);
  if muted then
    status := syslib.sound_mute(1);

  playing := true;
  paused := false;
  quit := false;
  draw();

  while not quit and playing do
  begin
    if paused then
    begin
      core.sleep(uint(50));
      pump();
    end
    else
    begin
      status := wav.read(path, info, cursor, chunk, got);
      if status <> Ok or got <= 0 then
      begin
        if status <> Ok then
          error_text := "Read error";
        if core.len(stage) > uint(0) then
        begin
          status := syslib.sound_write(stage);
          stage := "";
        end;
        status := syslib.sound_close();
        playing := false;
      end
      else
      begin
        status := core.copy_bytes(stage, core.len(stage), chunk, uint(0), uint(got));
        cursor := cursor + uint(got);
        // One 1 ms block per 4 KiB of input. The 32 KiB ring absorbs ~128
        // pieces before the first write blocks, which is a long continuous run
        // for a normal task; the short sleep resets the per-run budget (spec
        // 0061) without costing audible time.
        if cursor mod uint(4096) < uint(ChunkCap) then
          core.sleep(uint(1));
        if int(core.len(stage)) + ChunkCap > StageCap or cursor >= info.data_size
        then
        begin
          // One 16 KiB block per flush (~85 ms at 44.1 kHz). Drawing per
          // 256-byte read would paint the window hundreds of times a second
          // and run past the normal-task budget before the write blocks.
          status := syslib.sound_write(stage);
          stage := "";
          pump();
          draw();
        end;
      end;
    end;
  end;

  if playing then
  begin
    drain();
    status := syslib.sound_close();
    playing := false;
  end;
  wait_close();
end;

procedure main(args: const string[1024]);
begin
  black := stdlib.ui_black();
  white := stdlib.ui_white();
  gray := stdlib.ui_gray();
  accent := stdlib.ui_accent();
  volume := DefaultVolume;
  muted := false;
  playing := false;
  paused := false;
  quit := false;
  info.data_size := uint(0);
  stage := "";
  error_text := "";

  parse_args(args);
  if mode = ModeCheck then
  begin
    check_mode();
    return;
  end;

  if syslib.screen_width() = uint(0) then
  begin
    out.say("music_player: no display\n");
    return;
  end;

  status := syslib.window_open(WindowX, WindowY, WindowW, WindowH, "Music Player", id);
  if status <> Ok then
  begin
    out.say("music_player: no window\n");
    return;
  end;
  make_title(path);
  play_file();
  status := syslib.window_close(id);
  id := 0;
end;

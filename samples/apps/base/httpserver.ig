unit httpserver;

// A small read-only HTTP/1.0 file server over the ordinary TCP listener:
//
//   httpserver [port]              // serve the shell's current directory
//   run httpserver [port]          // serve the image root
//
// The default port is 8080. A request target is decoded, resolved under the
// chosen directory, and served from the read-only application volume (or the
// writable `/home` mount) with `core.fs_size` plus 256-byte `core.fs_read`
// chunks, so a file is streamed in many small pieces and is never buffered
// whole. A directory request serves its `index.html` when one exists.
//
// Traversal is rejected, not clamped: any decoded component equal to `..`
// answers 403, and a component that is too long answers 400. Only `GET` and
// `HEAD` are accepted; every response closes the connection. There are no
// ranges, keep-alive, CGI or writes.
//
// This is an ordinary `/lib`-based application: no kernel, opcode, builtin or
// ABI change. `lib.socket` owns the handles; the fixed buffers here are the
// application's own capacity.

uses core, stdlib, lib.socket, lib.path;

const
  DefaultPort = 8080;

  // The whole request line must fit here; anything larger is a 400.
  RequestCapacity = 1024;
  // One `core.fs_read` piece and one `socket.send` buffer.
  ChunkCapacity = 256;
  // A normalized target path and its buffers.
  PathCapacity = 256;
  // Response header assembly.
  HeaderCapacity = 512;
  // One error body.
  ErrorCapacity = 64;

  // A component is at most 64 bytes (spec 0018, L6).
  NameCapacity = 64;
  // `lib.path.join` supports eight output components.
  MaxComponents = 8;

  // Per-request read window and per-send retry window in nanoseconds.
  RequestDeadlineNs = 10000000000;
  SendDeadlineNs = 30000000000;
  RequestAttempts = 200000;
  SendAttempts = 100000;

  // QEMU user networking defaults, applied only when no address is configured
  // (the same rule as `nc` and `purl`).
  LocalIp = 0x0A00020F;
  NetMask = 0xFFFFFF00;
  GatewayIp = 0x0A000202;
  DnsIp = 0x0A000203;

  // `prepare_path` results.
  PathOk = 0;
  PathForbidden = 1;
  PathBad = 2;

  // HTTP status codes used by this server.
  CodeOk = 200;
  CodeBadRequest = 400;
  CodeForbidden = 403;
  CodeNotFound = 404;
  CodeMethod = 405;
  CodeServer = 500;

// Writes one decimal integer.
procedure write_int(value: int);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

procedure usage();
begin
  core.write("httpserver: usage: httpserver [port]\n");
end;

procedure fail(text: const string[0]);
begin
  core.write("httpserver: ");
  core.write(text);
  core.write("\n");
end;

procedure fail_status(prefix: const string[0]; st: Status);
begin
  core.write("httpserver: ");
  core.write(prefix);
  core.write(" ");
  write_int(int(st));
  core.write("\n");
end;

// Returns the value of one hexadecimal digit, or -1.
function hex_digit(value: byte): int;
begin
  if value >= byte(48) and value <= byte(57) then
    return int(value) - 48;
  if value >= byte(65) and value <= byte(70) then
    return int(value) - 55;
  if value >= byte(97) and value <= byte(102) then
    return int(value) - 87;
  return -1;
end;

// Decodes the request target `raw[start..stop)` into `rel`, dropping empty and
// `.` components and joining the rest with `/`. `..` is a hard error: this
// server refuses traversal instead of letting `lib.path.join` clamp it.
//
// Percent escapes are decoded first, so `%2e%2e` is seen as `..`; a NUL or
// another control byte after decoding is a bad request. The decoded path is
// published as a `string` only through `core.substr_into`, so invalid UTF-8 is
// a bad request and the caller's buffer is never partially written.
function prepare_path(raw: const bytes[0]; start: int; stop: int;
                      rel: var string[PathCapacity]): int;
var
  decoded: bytes[PathCapacity];
  clean: bytes[PathCapacity];
  i: int;
  j: int;
  dlen: int;
  used: int;
  count: int;
  comp_start: int;
  comp_len: int;
  high: int;
  low: int;
  code: int;
  is_dot: bool;
  is_dotdot: bool;
  copied: int;
begin
  dlen := 0;
  i := start;
  while i < stop do
  begin
    code := -1;
    if raw[i] = byte(37) then
    begin
      if i + 2 >= stop then
        return PathBad;
      high := hex_digit(raw[i + 1]);
      low := hex_digit(raw[i + 2]);
      if high < 0 or low < 0 then
        return PathBad;
      code := high * 16 + low;
      i := i + 3;
    end
    else
    begin
      code := int(raw[i]);
      i := i + 1;
    end;
    // NUL and control bytes never name a file in this system.
    if code = 0 or code < 32 or code = 127 then
      return PathBad;
    if dlen >= PathCapacity then
      return PathBad;
    decoded[dlen] := byte(code);
    dlen := dlen + 1;
  end;

  used := 0;
  count := 0;
  i := 0;
  while i <= dlen do
  begin
    comp_start := i;
    while i < dlen and decoded[i] <> byte(47) do
      i := i + 1;
    comp_len := i - comp_start;
    if comp_len > 0 then
    begin
      is_dot := comp_len = 1 and decoded[comp_start] = byte(46);
      is_dotdot := comp_len = 2 and decoded[comp_start] = byte(46)
        and decoded[comp_start + 1] = byte(46);
      if is_dotdot then
        return PathForbidden;
      // `lib.path.join` maps a leading `~` component to `home`, which would
      // escape the served directory; the request form is refused here.
      if count = 0 and comp_len = 1 and decoded[comp_start] = byte(126) then
        return PathForbidden;
      if not is_dot then
      begin
        if comp_len > NameCapacity then
          return PathBad;
        if count >= MaxComponents then
          return PathBad;
        if used > 0 then
        begin
          if used + 1 + comp_len > PathCapacity then
            return PathBad;
          clean[used] := byte(47);
          used := used + 1;
        end
        else if comp_len > PathCapacity then
          return PathBad;
        j := 0;
        while j < comp_len do
        begin
          clean[used] := decoded[comp_start + j];
          used := used + 1;
          j := j + 1;
        end;
        count := count + 1;
      end;
    end;
    i := i + 1;
  end;

  copied := core.substr_into(rel, clean, 0, used);
  if copied = used then
    return PathOk;
  return PathBad;
end;

// One `socket.send_text` with the stack's non-blocking retry loop. The text is
// at most one TCP segment (the header is bounded well below it), so a short
// copy is a capacity error rather than a partial header.
function send_text_all(peer: int; text: const string[0]): Status;
var
  st: Status;
  sent: int;
  deadline: uint;
  attempts: int;
  done: bool;
begin
  deadline := core.clock() + uint(SendDeadlineNs);
  attempts := 0;
  done := false;
  while not done and attempts < SendAttempts do
  begin
    st := socket.send_text(peer, text, sent);
    if st = Ok then
    begin
      if sent <> int(core.len(text)) then
        return CapacityExceeded;
      done := true;
    end
    else
    begin
      if st <> WouldBlock then
        return st;
      if core.clock() >= deadline then
        return WouldBlock;
      attempts := attempts + 1;
    end;
  end;
  if done then
    return Ok;
  return WouldBlock;
end;

// One `socket.send` for a 256-byte file chunk. A chunk below the MSS either
// goes whole or not at all, so a short copy is an error.
function send_chunk(peer: int; chunk: const bytes[0]; length: int): Status;
var
  st: Status;
  sent: int;
  deadline: uint;
  attempts: int;
  done: bool;
begin
  deadline := core.clock() + uint(SendDeadlineNs);
  attempts := 0;
  done := false;
  while not done and attempts < SendAttempts do
  begin
    st := socket.send(peer, chunk, sent);
    if st = Ok then
    begin
      if sent <> length then
        return CapacityExceeded;
      done := true;
    end
    else
    begin
      if st <> WouldBlock then
        return st;
      if core.clock() >= deadline then
        return WouldBlock;
      attempts := attempts + 1;
    end;
  end;
  if done then
    return Ok;
  return WouldBlock;
end;

// Sends the status line and the two fixed headers, then the blank line.
function send_header(peer: int; code: int; content_type: const string[0];
                     length: uint): Status;
var
  line: string[48];
  digits: bytes[32];
  number: string[16];
  header: string[HeaderCapacity];
  used: int;
  total: int;
  copied: int;
  st: Status;
begin
  if code = CodeOk then
    line := "HTTP/1.0 200 OK\r\n"
  else if code = CodeBadRequest then
    line := "HTTP/1.0 400 Bad Request\r\n"
  else if code = CodeForbidden then
    line := "HTTP/1.0 403 Forbidden\r\n"
  else if code = CodeNotFound then
    line := "HTTP/1.0 404 Not Found\r\n"
  else if code = CodeMethod then
    line := "HTTP/1.0 405 Method Not Allowed\r\n"
  else
    line := "HTTP/1.0 500 Internal Server Error\r\n";

  used := core.format_int_into(digits, int(length), uint(10), uint(0));
  if used <= 0 or used > 16 then
    return CapacityExceeded;
  copied := core.substr_into(number, digits, 0, used);
  if copied <> used then
    return Invalid;

  // "Content-Type: " (14), "Content-Length: " (16) and
  // "Connection: close\r\n\r\n" (21), plus four CRLF pairs.
  total := int(core.len(line)) + 14 + int(core.len(content_type)) + 2 + 16
    + used + 2 + 21;
  if total > HeaderCapacity then
    return CapacityExceeded;
  header := line + "Content-Type: " + content_type + "\r\n"
    + "Content-Length: " + number + "\r\n"
    + "Connection: close\r\n\r\n";
  if int(core.len(header)) <> total then
    return Invalid;

  st := send_text_all(peer, header);
  return st;
end;

// A short plain-text error, with a matching Content-Length. A `HEAD` request
// gets the header only, as if the body had been sent.
function send_error(peer: int; code: int; head_only: bool): Status;
var
  text: string[ErrorCapacity];
  st: Status;
begin
  if code = CodeBadRequest then
    text := "400 Bad Request\n"
  else if code = CodeForbidden then
    text := "403 Forbidden\n"
  else if code = CodeNotFound then
    text := "404 Not Found\n"
  else if code = CodeMethod then
    text := "405 Method Not Allowed\n"
  else
    text := "500 Internal Server Error\n";

  st := send_header(peer, code, "text/plain", core.len(text));
  if st <> Ok then
    return st;
  if head_only then
    return Ok;
  return send_text_all(peer, text);
end;

// Chooses a content type from the target's extension.
function mime_type(target: const string[0];
                   out: var string[32]): Status;
var
  length: int;
  at: int;
  stop: int;
  copied: int;
  ext: string[16];
begin
  length := int(core.len(target));
  stop := length;
  at := length - 1;
  while at >= 0 and target[at] <> byte(46) do
    at := at - 1;
  if at < 0 or stop - at - 1 > 15 then
  begin
    copied := core.substr_into(out, "application/octet-stream", 0, 24);
    return Ok;
  end;
  copied := core.substr_into(ext, target, at + 1, stop - at - 1);
  if copied < 0 then
    return Invalid;

  if ext = "html" or ext = "htm" then
    copied := core.substr_into(out, "text/html", 0, 9)
  else if ext = "txt" or ext = "ig" or ext = "md" then
    copied := core.substr_into(out, "text/plain", 0, 10)
  else if ext = "css" then
    copied := core.substr_into(out, "text/css", 0, 8)
  else if ext = "js" then
    copied := core.substr_into(out, "text/javascript", 0, 15)
  else if ext = "json" then
    copied := core.substr_into(out, "application/json", 0, 16)
  else if ext = "png" then
    copied := core.substr_into(out, "image/png", 0, 9)
  else if ext = "ppm" then
    copied := core.substr_into(out, "image/x-portable-pixmap", 0, 23)
  else if ext = "jpg" or ext = "jpeg" then
    copied := core.substr_into(out, "image/jpeg", 0, 10)
  else if ext = "gif" then
    copied := core.substr_into(out, "image/gif", 0, 9)
  else if ext = "svg" then
    copied := core.substr_into(out, "image/svg+xml", 0, 13)
  else if ext = "pdf" then
    copied := core.substr_into(out, "application/pdf", 0, 15)
  else
    copied := core.substr_into(out, "application/octet-stream", 0, 24);
  if copied < 0 then
    return Invalid;
  return Ok;
end;

// Reads one request line into `raw`, ending with the `\n` at `raw[length-1]`.
// The connection is closed after one request, so the rest of the headers are
// not read. `Closed` is a peer that left before sending a line.
function read_request_line(peer: int; raw: var bytes[RequestCapacity];
                           length: var int): Status;
var
  chunk: bytes[RequestCapacity];
  got: int;
  j: int;
  st: Status;
  found: bool;
  deadline: uint;
  attempts: int;
begin
  length := 0;
  found := false;
  attempts := 0;
  deadline := core.clock() + uint(RequestDeadlineNs);
  while not found do
  begin
    st := socket.recv(peer, chunk, got);
    if st = Ok then
    begin
      if got = 0 then
        return Closed;
      j := 0;
      while j < got and not found do
      begin
        if length >= RequestCapacity then
          return CapacityExceeded;
        raw[length] := chunk[j];
        if chunk[j] = byte(10) then
          found := true;
        length := length + 1;
        j := j + 1;
      end;
    end
    else if st = Closed then
    begin
      return Closed;
    end
    else if st = WouldBlock then
    begin
      if core.clock() >= deadline then
        return WouldBlock;
      attempts := attempts + 1;
      if attempts >= RequestAttempts then
        return WouldBlock;
    end
    else
      return st;
  end;
  return Ok;
end;

// Streams a whole regular file in 256-byte `core.fs_read` chunks. The headers
// already carry the length, so a read failure ends the connection with a short
// body rather than a second response.
function stream_file(peer: int; target: const string[0];
                     total: uint): Status;
var
  chunk: bytes[ChunkCapacity];
  offset: uint;
  got: int;
  st: Status;
begin
  offset := uint(0);
  while offset < total do
  begin
    st := core.fs_read(target, offset, chunk, got);
    if st <> Ok then
      return st;
    if got <= 0 then
      return Invalid;
    if uint(got) > total - offset then
      return Invalid;
    st := send_chunk(peer, chunk, got);
    if st <> Ok then
      return st;
    offset := offset + uint(got);
  end;
  return Ok;
end;

// Handles exactly one request on `peer`. The caller owns the handle and closes
// it after this returns.
function handle_client(peer: int; cwd: const string[0]): Status;
var
  raw: bytes[RequestCapacity];
  total: int;
  line_end: int;
  at: int;
  method_end: int;
  target_end: int;
  target_stop: int;
  rel: string[PathCapacity];
  target: string[PathCapacity];
  index: string[PathCapacity];
  mime: string[32];
  copied: int;
  size: uint;
  path_result: int;
  code: int;
  head_only: bool;
  st: Status;
begin
  st := read_request_line(peer, raw, total);
  if st = Closed then
    return Ok;
  if st <> Ok then
  begin
    send_error(peer, CodeBadRequest, false);
    return Ok;
  end;

  // Drop the line terminator, including an HTTP CR.
  line_end := total - 1;
  if line_end > 0 and raw[line_end - 1] = byte(13) then
    line_end := line_end - 1;

  method_end := 0;
  while method_end < line_end and raw[method_end] <> byte(32) do
    method_end := method_end + 1;
  if method_end >= line_end then
  begin
    send_error(peer, CodeBadRequest, false);
    return Ok;
  end;

  head_only := false;
  if method_end = 3 and raw[0] = byte(71) and raw[1] = byte(69)
      and raw[2] = byte(84) then
    head_only := false
  else if method_end = 4 and raw[0] = byte(72) and raw[1] = byte(69)
      and raw[2] = byte(65) and raw[3] = byte(68) then
    head_only := true
  else
  begin
    send_error(peer, CodeMethod, false);
    return Ok;
  end;

  at := method_end + 1;
  target_end := at;
  while target_end < line_end and raw[target_end] <> byte(32) do
    target_end := target_end + 1;
  if target_end = at or target_end + 5 >= line_end then
  begin
    send_error(peer, CodeBadRequest, head_only);
    return Ok;
  end;
  // The only version prefix accepted is `HTTP/`; the minor number is not used.
  if raw[target_end + 1] <> byte(72) or raw[target_end + 2] <> byte(84)
      or raw[target_end + 3] <> byte(84) or raw[target_end + 4] <> byte(80)
      or raw[target_end + 5] <> byte(47) then
  begin
    send_error(peer, CodeBadRequest, head_only);
    return Ok;
  end;

  // A query string is not part of the file name; the fragment never reaches
  // the server.
  target_stop := at;
  while target_stop < target_end and raw[target_stop] <> byte(63) do
    target_stop := target_stop + 1;

  path_result := prepare_path(raw, at, target_stop, rel);
  if path_result = PathForbidden then
  begin
    core.write("httpserver: refused traversal\n");
    send_error(peer, CodeForbidden, head_only);
    return Ok;
  end;
  if path_result <> PathOk then
  begin
    send_error(peer, CodeBadRequest, head_only);
    return Ok;
  end;

  st := path.join(cwd, rel, target);
  if st <> Ok then
  begin
    send_error(peer, CodeBadRequest, head_only);
    return Ok;
  end;

  // `fs_size` is `Ok` only for a regular file, `Invalid` for a directory and
  // `NotFound` for a missing name, which separates the three cases.
  code := CodeOk;
  size := uint(0);
  st := core.fs_size(target, size);
  if st = Invalid then
  begin
    st := path.join(target, "index.html", index);
    if st = Ok then
    begin
      st := core.fs_size(index, size);
      if st = Ok then
      begin
        copied := core.substr_into(target, index, 0, int(core.len(index)));
        if copied <> int(core.len(index)) then
          code := CodeServer;
      end
      else if st = NotFound or st = Invalid then
        code := CodeNotFound
      else
        code := CodeServer;
    end
    else
      code := CodeBadRequest;
  end
  else if st = NotFound then
    code := CodeNotFound
  else if st <> Ok then
    code := CodeServer;

  if code <> CodeOk then
  begin
    core.write("httpserver: ");
    if head_only then
      core.write("HEAD ")
    else
      core.write("GET ");
    core.write("/");
    core.write(rel);
    core.write(" ");
    write_int(code);
    core.write("\n");
    send_error(peer, code, head_only);
    return Ok;
  end;

  st := mime_type(target, mime);
  if st <> Ok then
  begin
    copied := core.substr_into(mime, "application/octet-stream", 0, 24);
  end;

  core.write("httpserver: ");
  if head_only then
    core.write("HEAD ")
  else
    core.write("GET ");
  core.write("/");
  core.write(rel);
  core.write(" ");
  write_int(CodeOk);
  core.write(" ");
  write_int(int(size));
  core.write("\n");

  st := send_header(peer, CodeOk, mime, size);
  if st <> Ok then
    return st;
  if head_only then
    return Ok;
  return stream_file(peer, target, size);
end;

// Serves one accepted connection, always closing it.
procedure serve_client(peer: int; cwd: const string[0]);
var
  st: Status;
begin
  st := socket.set_timeout(peer, uint(30000));
  if st = Ok then
    st := handle_client(peer, cwd);
  socket.close(peer);
  if st <> Ok and st <> Closed then
    fail_status("client failed:", st);
end;

procedure main(args: const string[1024]);
var
  cwd: string[PathCapacity];
  rest: string[PathCapacity];
  console: bytes[RequestCapacity];
  sources: array[uint, 8];
  ip: uint;
  mask: uint;
  gateway: uint;
  dns_server: uint;
  port: int;
  listener: int;
  peer: int;
  ready: int;
  got: int;
  st: Status;
  done: bool;
begin
  st := path.split_cwd(args, cwd, rest);
  if st <> Ok then
  begin
    usage();
    return;
  end;

  if int(core.len(rest)) = 0 then
    port := DefaultPort
  else
  begin
    if stdlib.parse_int_into(rest, port) <> Ok then
    begin
      usage();
      return;
    end;
    if port <= 0 or port > 65535 then
    begin
      usage();
      return;
    end;
  end;

  st := core.net_info(ip, mask, gateway, dns_server);
  if st <> Ok then
  begin
    fail("no device");
    return;
  end;
  if ip = uint(0) then
  begin
    st := core.net_config(uint(LocalIp), uint(NetMask), uint(GatewayIp),
      uint(DnsIp));
    if st <> Ok then
    begin
      fail("no device");
      return;
    end;
  end;

  st := socket.listen(uint(port), listener);
  if st <> Ok then
  begin
    fail_status("listen failed:", st);
    return;
  end;
  st := socket.set_timeout(listener, uint(1000));
  if st <> Ok then
  begin
    socket.close(listener);
    fail_status("listen failed:", st);
    return;
  end;

  core.write("httpserver: serving ");
  if int(core.len(cwd)) = 0 then
    core.write("/")
  else
    core.write(cwd);
  core.write(" on port ");
  write_int(port);
  core.write("\nhttpserver: press Ctrl+C to stop\n");

  done := false;
  while not done do
  begin
    sources[0] := uint(0);
    sources[1] := socket.source(listener);
    ready := -1;
    st := core.next_any(sources, 2, ready);
    if st <> Ok then
    begin
      fail_status("select failed:", st);
      done := true;
    end
    else if ready = 0 then
    begin
      got := core.read_input(console);
      if got > 0 and (console[0] = byte(3) or console[0] = byte(4)) then
        done := true;
    end
    else if ready = 1 then
    begin
      st := socket.accept(listener, peer);
      if st = Ok then
        serve_client(peer, cwd)
      else if st <> WouldBlock then
      begin
        fail_status("accept failed:", st);
        done := true;
      end;
    end
    else
    begin
      fail("bad source");
      done := true;
    end;
  end;

  socket.close(listener);
  core.write("httpserver: stopped\n");
end;

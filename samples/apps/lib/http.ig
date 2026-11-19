unit http;

// Importable minimal HTTP/1.0 client (spec 0020, N16). Ordinary `/lib` module
// source on top of the `socket` unit: no `main`, no top-level `var` state, no
// stream endpoint and no kernel feature. It sends one `GET`, reads until the
// server closes (or the buffer is full), strips the response headers at the
// first `\r\n\r\n`, and publishes the body into the caller's string.
//
//   st := http.get(uint(0x0A000202), uint(6666), "10.0.2.2", "/", body);
//
// The socket surface is non-blocking in this slice, so the send and receive
// loops retry while the stack reports `WouldBlock`, bounded by a `core.clock()`
// deadline and by a retry cap. A timeout is `WouldBlock`; the caller reports
// it. The socket is always closed, on every path.
//
// Deliberate limits: HTTP/1.0 only (no chunked encoding), no redirects, no
// TLS, no keep-alive, and the body must be valid UTF-8 because the destination
// is a `string`. A response whose raw bytes overflow the local arena is
// `CapacityExceeded`.

uses core, stdlib, lib.socket;

const
  // The whole request must fit this staging buffer (`GET`, path, host and the
  // two fixed header lines).
  RequestCapacity = 256;
  // Raw response arena: headers plus body, before the body is published.
  RawCapacity = 4096;
  // The caller's body destination.
  BodyCapacity = 4096;
  // One `socket.recv` destination.
  ChunkCapacity = 1024;

  // The whole exchange must finish inside this window of `core.clock()`.
  DeadlineNs = 8000000000;
  // Bounded retries, so a dead peer cannot burn the task's CPU budget. These
  // are generous next to the real exchange (a few iterations on QEMU).
  SendAttempts = 100000;
  RecvAttempts = 200000;

// Fetches `path` from `host` at `ip`:`port` into `body`. `Ok` publishes the
// response body (valid UTF-8); `WouldBlock` is the deadline or the retry cap;
// `Closed` is a peer that closed without sending anything; any other failure
// is the socket status, with `body` untouched.
function get(ip: uint; port: uint; host: const string[0]; path: const string[0];
             body: var string[BodyCapacity]): Status;
var
  id: int;
  st: Status;
  request: string[RequestCapacity];
  request_len: int;
  total_request: int;
  path_len: int;
  host_len: int;
  deadline: uint;
  attempts: int;
  sending: bool;
  reading: bool;
  sent: int;
  chunk: bytes[ChunkCapacity];
  read: int;
  raw: bytes[RawCapacity];
  total: int;
  j: int;
  i: int;
  header_end: int;
  body_len: int;
  copied: int;
begin
  // Build `GET <path> HTTP/1.0`, `Host: <host>`, `Connection: close`. Compute
  // the exact length first so a long argument is a clean `CapacityExceeded`
  // instead of a buffer fault inside the concatenation.
  path_len := int(core.len(path));
  host_len := int(core.len(host));
  // "GET " (4) + path + " HTTP/1.0\r\n" (11) + "Host: " (6) + host +
  // "\r\n" (2) + "Connection: close\r\n\r\n" (21).
  total_request := 4 + path_len + 11 + 6 + host_len + 2 + 21;
  if total_request > RequestCapacity then
    return CapacityExceeded;
  request := "GET " + path + " HTTP/1.0\r\n" + "Host: " + host + "\r\n"
    + "Connection: close\r\n\r\n";
  request_len := int(core.len(request));

  st := socket.tcp(ip, port, id);
  if st <> Ok then
    return st;

  deadline := core.clock() + uint(DeadlineNs);

  // Send the whole request. `WouldBlock` means the handshake is still running
  // or the previous segment is unacknowledged; retry until the deadline.
  attempts := 0;
  sending := true;
  sent := 0;
  while sending and attempts < SendAttempts do
  begin
    st := socket.send_text(id, request, sent);
    if st = Ok then
      sending := false
    else
    begin
      if st <> WouldBlock then
      begin
        socket.close(id);
        return st;
      end;
      if core.clock() >= deadline then
      begin
        socket.close(id);
        return WouldBlock;
      end;
      attempts := attempts + 1;
    end;
  end;
  if sending then
  begin
    socket.close(id);
    return WouldBlock;
  end;
  if sent <> request_len then
  begin
    // A TCP send copies at most one MSS; the request is smaller than that, so
    // a short count means the stack could not take it whole.
    socket.close(id);
    return CapacityExceeded;
  end;

  // Read until end of file. The raw response is accumulated in `bytes` so a
  // chunk boundary can never split a UTF-8 code point; the header is stripped
  // and one checked `core.substr_into` publishes the body.
  total := 0;
  attempts := 0;
  reading := true;
  while reading and attempts < RecvAttempts do
  begin
    st := socket.recv(id, chunk, read);
    if st = Ok then
    begin
      if read = 0 then
        reading := false
      else
      begin
        j := 0;
        while j < read do
        begin
          if total >= RawCapacity then
          begin
            socket.close(id);
            return CapacityExceeded;
          end;
          raw[total] := chunk[j];
          total := total + 1;
          j := j + 1;
        end;
      end;
    end
    else
    begin
      // A peer that closes with a reset after sending a response is still a
      // usable response; keep what arrived and strip it below.
      if st = Closed and total > 0 then
        reading := false
      else if st = WouldBlock then
      begin
        if core.clock() >= deadline then
        begin
          socket.close(id);
          return WouldBlock;
        end;
        attempts := attempts + 1;
      end
      else
      begin
        socket.close(id);
        return st;
      end;
    end;
  end;
  if reading then
  begin
    socket.close(id);
    return WouldBlock;
  end;

  socket.close(id);
  if total = 0 then
    return Closed;

  // Find the blank line that ends the headers.
  header_end := -1;
  i := 0;
  while i + 3 < total do
  begin
    if raw[i] = byte(13) and raw[i + 1] = byte(10) and raw[i + 2] = byte(13)
        and raw[i + 3] = byte(10) then
    begin
      header_end := i + 4;
      i := total;
    end
    else
      i := i + 1;
  end;
  if header_end < 0 then
    return CapacityExceeded;

  body_len := total - header_end;
  if body_len > BodyCapacity then
    return CapacityExceeded;
  copied := core.substr_into(body, raw, header_end, body_len);
  if copied < 0 then
    return Invalid;
  return Ok;
end;

// The response body length, for a caller that wants it without `core.len`.
function body_length(body: const string[BodyCapacity]): int;
begin
  return int(core.len(body));
end;

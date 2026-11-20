unit net_echo;

// In-guest TCP echo server (spec 0020, N10/N20). It listens on port 7, accepts
// three sequential host connections, echoes every chunk back as it arrives and
// closes each connection on the peer's FIN:
//
//   run net_echo                     // with QEMU forwarding tcp::7007-:7
//
// The listener and the connections are the ordinary non-blocking socket
// surface: `socket.accept` reports `WouldBlock` until a handshake completes,
// and every wait is bounded by a `core.clock()` deadline plus a retry cap, so a
// silent peer prints `net_echo: timeout` instead of burning the CPU budget.
// No window and no graphics: this is a console service, so it also runs on the
// `--shell` image and under the host harness.

uses core, stdlib, lib.socket;

const
  // The echo service's well-known port; the host forwards 7007 to it.
  EchoPort = 7;
  // QEMU user networking: the same static configuration as `net_probe`.
  LocalIp = 0x0A00020F;
  NetMask = 0xFFFFFF00;
  GatewayIp = 0x0A000202;
  DnsIp = 0x0A000203;

  // Three sequential clients are the whole proof.
  Clients = 3;
  // One `socket.recv` destination; an echo is at most one TCP segment.
  ChunkCapacity = 1024;

  // Wall-clock windows in nanoseconds (`core.clock()`). The accept window is
  // deliberately generous because a host client is started by hand or by the
  // harness; a connected client must finish inside its own window.
  AcceptDeadlineNs = 30000000000;
  ClientDeadlineNs = 30000000000;

  // Bounded retries per wait, so a dead peer cannot spin forever.
  AcceptAttempts = 200000;
  RecvAttempts = 200000;
  SendAttempts = 100000;

// Writes one decimal integer followed by a newline.
procedure write_int(value: int);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

// `net_echo: failed <ordinal>` for a status the caller cannot classify.
procedure report_failure(st: Status);
begin
  core.write("net_echo: failed ");
  write_int(int(st));
  core.write("\n");
end;

// Waits for one completed connection on `listener`. `Ok` writes the connected
// handle through `peer`; `WouldBlock` is the deadline or the retry cap.
function accept_client(listener: int; peer: var int): Status;
var
  st: Status;
  deadline: uint;
  attempts: int;
  done: bool;
begin
  deadline := core.clock() + uint(AcceptDeadlineNs);
  attempts := 0;
  done := false;
  while not done and attempts < AcceptAttempts do
  begin
    st := socket.accept(listener, peer);
    if st = Ok then
      done := true
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

// Echoes one client until EOF or a reset, then returns. Every chunk is written
// to the console and sent back exactly once; a `WouldBlock` send retries the
// same buffer (a chunk below the MSS either goes whole or not at all).
function echo_client(peer: int): Status;
var
  st: Status;
  chunk: bytes[ChunkCapacity];
  read: int;
  sent: int;
  deadline: uint;
  attempts: int;
  active: bool;
  sending: bool;
begin
  deadline := core.clock() + uint(ClientDeadlineNs);
  active := true;
  while active do
  begin
    st := socket.recv(peer, chunk, read);
    if st = Ok then
    begin
      if read = 0 then
        active := false
      else
      begin
        core.write(chunk);
        sending := true;
        attempts := 0;
        while sending and attempts < SendAttempts do
        begin
          st := socket.send(peer, chunk, sent);
          if st = Ok then
          begin
            // A short copy would split the echo; the non-blocking contract
            // sends a whole segment or nothing.
            if sent <> read then
              return CapacityExceeded;
            sending := false;
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
        if sending then
          return WouldBlock;
      end;
    end
    else
    begin
      // A reset ends the client like a FIN; the caller closes the handle.
      if st = Closed then
        active := false
      else if st = WouldBlock then
      begin
        if core.clock() >= deadline then
          return WouldBlock;
        attempts := attempts + 1;
        if attempts >= RecvAttempts then
          return WouldBlock;
      end
      else
        return st;
    end;
  end;
  return Ok;
end;

procedure main(args: const string[1024]);
var
  st: Status;
  listener: int;
  peer: int;
  client: int;
begin
  st := core.net_config(uint(LocalIp), uint(NetMask), uint(GatewayIp),
    uint(DnsIp));
  if st <> Ok then
  begin
    core.write("net_echo: no device\n");
    return;
  end;

  st := socket.listen(uint(EchoPort), listener);
  if st <> Ok then
  begin
    report_failure(st);
    return;
  end;
  // `accept` now sleeps in the scheduler; the deadline keeps the service's
  // own 30 s window. A client connection wakes it at once.
  st := socket.set_timeout(listener, uint(30000));
  if st <> Ok then
  begin
    socket.close(listener);
    report_failure(st);
    return;
  end;
  core.write("net_echo: ready\n");

  client := 0;
  while client < Clients do
  begin
    st := accept_client(listener, peer);
    if st = WouldBlock then
    begin
      socket.close(listener);
      core.write("net_echo: timeout\n");
      return;
    end;
    if st <> Ok then
    begin
      socket.close(listener);
      report_failure(st);
      return;
    end;

    client := client + 1;
    core.write("net_echo: client ");
    write_int(client);
    core.write("\n");

    // A silent client is bounded by its own 30 s window; recv/send sleep
    // until data, space or the deadline.
    st := socket.set_timeout(peer, uint(30000));
    if st <> Ok then
    begin
      socket.close(peer);
      socket.close(listener);
      report_failure(st);
      return;
    end;

    st := echo_client(peer);
    socket.close(peer);
    if st = WouldBlock then
    begin
      socket.close(listener);
      core.write("net_echo: timeout\n");
      return;
    end;
    if st <> Ok then
    begin
      socket.close(listener);
      report_failure(st);
      return;
    end;
  end;

  socket.close(listener);
  core.write("net_echo: ok\n");
end;

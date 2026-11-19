unit net_select;

// Multi-source socket proof and deterministic timeout probe (spec 0020, N13;
// ADR 0036/0050). Unlike `net_probe`, it does not retry: the blocking socket
// builtins sleep in the scheduler and `core.next_any` names the socket source
// returned by `socket.source`.
//
//   run net_select            // one HTTP GET, selected with next_any
//   run net_select timeout    // a 300 ms UDP receive deadline, no data
//   run net_select selecttimeout // a 300 ms next_any socket-source deadline
//
// `select` opens a TCP connection to the host alias, sends a GET, waits in
// `core.next_any` on `[socket.source(id), console byte]`, then reads the
// response to EOF. The socket arm must fire (index 0). `timeout` opens a UDP
// socket to a dead port, sets a 300 ms deadline and proves that a single
// blocking `recv` returns `WouldBlock` after the deadline instead of sleeping.
// `selecttimeout` proves the same deadline through a blocking `next_any` over
// the socket source. No window, no graphics; a machine without a NIC prints
// `net_select: no device`.

uses core, stdlib, lib.socket;

const
  // QEMU user networking: the host alias, the probe's fixed HTTP port and the
  // static configuration from spec 0020 N18.
  HostIp = 0x0A000202;
  HostPort = 6666;
  LocalIp = 0x0A00020F;
  NetMask = 0xFFFFFF00;
  GatewayIp = 0x0A000202;
  DnsIp = 0x0A000203;

  // The exchange window for the HTTP proof.
  ExchangeMs = 8000;
  RequestCapacity = 128;
  ChunkCapacity = 1024;
  BodyCapacity = 4096;

  // The timeout proof: a port that never answers, and a 300 ms deadline.
  DeadPort = 9;
  TimeoutMs = 300;
  // The measured elapsed time must be at least the deadline (minus a tick)
  // and below this generous ceiling.
  MinElapsedNs = 200000000;
  MaxElapsedNs = 5000000000;

var
  sources: array[uint, 8];

// Writes one decimal integer followed by a newline.
procedure write_int(value: int);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

procedure report_failure(st: Status);
begin
  core.write("net_select: failed ");
  write_int(int(st));
  core.write("\n");
end;

// One HTTP/1.0 GET through a blocking TCP socket, selected with `next_any`.
procedure run_select();
var
  st: Status;
  id: int;
  sent: int;
  read: int;
  total: int;
  ready: int;
  count: int;
  reading: bool;
  chunk: bytes[ChunkCapacity];
  request: string[RequestCapacity];
begin
  st := core.net_config(uint(LocalIp), uint(NetMask), uint(GatewayIp),
    uint(DnsIp));
  if st <> Ok then
  begin
    core.write("net_select: no device\n");
    return;
  end;

  st := socket.tcp(uint(HostIp), uint(HostPort), id);
  if st <> Ok then
  begin
    report_failure(st);
    return;
  end;
  st := socket.set_timeout(id, uint(ExchangeMs));
  if st <> Ok then
  begin
    socket.close(id);
    report_failure(st);
    return;
  end;

  request := "GET / HTTP/1.0\r\nHost: 10.0.2.2\r\nConnection: close\r\n\r\n";
  // The send blocks until the handshake completes and the segment is
  // acknowledged, so a successful call copies the whole request.
  st := socket.send_text(id, request, sent);
  if st <> Ok then
  begin
    socket.close(id);
    if st = WouldBlock then
      core.write("net_select: timeout\n")
    else
      report_failure(st);
    return;
  end;
  if sent <> int(core.len(request)) then
  begin
    socket.close(id);
    core.write("net_select: short send\n");
    return;
  end;

  // The socket first, the console byte second. The exchange is a few
  // milliseconds, so the socket arm must win; the caller has typed nothing
  // since the `run` line was consumed by the shell.
  sources[0] := socket.source(id);
  sources[1] := uint(0);
  count := 2;
  ready := -1;
  st := core.next_any(sources, count, ready);
  if st <> Ok or ready <> 0 then
  begin
    socket.close(id);
    if st = WouldBlock then
      core.write("net_select: timeout\n")
    else
      core.write("net_select: source not ready\n");
    return;
  end;

  // The blocking receive loop: each call sleeps until data/EOF/close or the
  // deadline. Bounded by BodyCapacity and the socket's 8 s window.
  total := 0;
  reading := true;
  while reading do
  begin
    st := socket.recv(id, chunk, read);
    if st = Ok then
    begin
      if read = 0 then
        reading := false
      else
      begin
        if total + read > BodyCapacity then
          read := BodyCapacity - total;
        total := total + read;
        if total >= BodyCapacity then
          reading := false;
      end;
    end
    else
    begin
      // A reset after some bytes is still a usable response.
      if st = Closed and total > 0 then
        reading := false
      else if st = WouldBlock then
      begin
        socket.close(id);
        core.write("net_select: timeout\n");
        return;
      end
      else
      begin
        socket.close(id);
        report_failure(st);
        return;
      end;
    end;
  end;

  socket.close(id);
  core.write("net_select: ok ");
  write_int(total);
  core.write(" bytes\n");
end;

// One blocking UDP receive with a deadline and no data. The call must return
// `WouldBlock` after roughly the deadline, not block indefinitely.
procedure run_timeout();
var
  st: Status;
  id: int;
  read: int;
  chunk: bytes[ChunkCapacity];
  before: uint;
  elapsed: uint;
begin
  st := core.net_config(uint(LocalIp), uint(NetMask), uint(GatewayIp),
    uint(DnsIp));
  if st <> Ok then
  begin
    core.write("net_select: no device\n");
    return;
  end;

  st := socket.udp(uint(HostIp), uint(DeadPort), id);
  if st <> Ok then
  begin
    report_failure(st);
    return;
  end;
  st := socket.set_timeout(id, uint(TimeoutMs));
  if st <> Ok then
  begin
    socket.close(id);
    report_failure(st);
    return;
  end;

  before := core.clock();
  st := socket.recv(id, chunk, read);
  elapsed := core.clock() - before;
  socket.close(id);

  if st = WouldBlock and elapsed >= uint(MinElapsedNs)
      and elapsed <= uint(MaxElapsedNs) then
  begin
    core.write("net_select: timeout ok ");
    write_int(int(elapsed div uint(1000000)));
    core.write(" ms\n");
  end
  else if st = WouldBlock then
  begin
    core.write("net_select: timeout out of window ");
    write_int(int(elapsed div uint(1000000)));
    core.write(" ms\n");
  end
  else
    report_failure(st);
end;

// One blocking `next_any` over a socket source with a deadline and no data:
// the selector must wake at the deadline (readable reports the expiry), return
// the socket index, and the following recv must observe the timeout.
procedure run_select_timeout();
var
  st: Status;
  id: int;
  read: int;
  ready: int;
  count: int;
  chunk: bytes[ChunkCapacity];
  before: uint;
  elapsed: uint;
begin
  st := core.net_config(uint(LocalIp), uint(NetMask), uint(GatewayIp),
    uint(DnsIp));
  if st <> Ok then
  begin
    core.write("net_select: no device\n");
    return;
  end;

  st := socket.udp(uint(HostIp), uint(DeadPort), id);
  if st <> Ok then
  begin
    report_failure(st);
    return;
  end;
  st := socket.set_timeout(id, uint(TimeoutMs));
  if st <> Ok then
  begin
    socket.close(id);
    report_failure(st);
    return;
  end;

  sources[0] := socket.source(id);
  count := 1;
  ready := -1;
  before := core.clock();
  st := core.next_any(sources, count, ready);
  elapsed := core.clock() - before;
  if st = Ok and ready = 0 and elapsed >= uint(MinElapsedNs)
      and elapsed <= uint(MaxElapsedNs) then
  begin
    // The timeout flag makes the source ready; the receive consumes it and
    // reports `WouldBlock` without sleeping again.
    st := socket.recv(id, chunk, read);
    socket.close(id);
    if st = WouldBlock then
    begin
      core.write("net_select: select timeout ok ");
      write_int(int(elapsed div uint(1000000)));
      core.write(" ms\n");
    end
    else
      report_failure(st);
  end
  else
  begin
    socket.close(id);
    core.write("net_select: select timeout out of window ");
    write_int(int(elapsed div uint(1000000)));
    core.write(" ms\n");
  end;
end;

procedure main(args: const string[1024]);
var
  mode: string[256];
  trimmed: int;
begin
  mode := "";
  if int(core.len(args)) > 0 then
  begin
    trimmed := stdlib.trim_into(mode, args);
    if trimmed < 0 then
    begin
      core.write("net_select: unknown mode\n");
      return;
    end;
  end;

  if int(core.len(mode)) = 0 then
    run_select()
  else if mode = "select" then
    run_select()
  else if mode = "timeout" then
    run_timeout()
  else if mode = "selecttimeout" then
    run_select_timeout()
  else
    core.write("net_select: unknown mode\n");
end;

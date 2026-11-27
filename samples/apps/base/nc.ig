unit nc;

// A very small TCP-only `netcat`:
//
//   run nc 10.0.2.2 6666
//
// It opens one TCP connection, forwards console bytes to the socket and socket
// bytes to the console, and exits when the peer closes or the user types
// Ctrl+C (0x03). The console is a raw byte stream (spec 0005, ADR 0037), so the
// application echoes typed bytes locally; a remote that echoes too will show
// them twice. There is no listen/accept mode, no UDP, no TLS and no file
// redirection. The host may be a dotted IPv4 address or a DNS name.
//
// On a fresh QEMU test shell with no address, the same user-network defaults as
// `purl` are applied: 10.0.2.15/24, gateway 10.0.2.2, DNS 10.0.2.3. A machine
// that already has a configuration keeps it.

uses core, stdlib, lib.socket, lib.dns, lib.path;

const
  ArgsCapacity = 256;
  HostCapacity = 128;
  PartCapacity = 8;
  ChunkCapacity = 1024;
  LocalIp = 0x0A00020F;
  NetMask = 0xFFFFFF00;
  GatewayIp = 0x0A000202;
  DnsIp = 0x0A000203;

var
  chunk: bytes[ChunkCapacity];
  stage: bytes[ChunkCapacity];

procedure usage();
begin
  core.write("nc: usage: nc host port\n");
end;

procedure fail(text: const string[0]);
begin
  core.write("nc: ");
  core.write(text);
  core.write("\n");
end;

procedure fail_status(prefix: const string[0]; st: Status);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, int(st), uint(10), uint(0));
  core.write("nc: ");
  core.write(prefix);
  core.write(" ");
  core.write(digits);
  core.write("\n");
end;

// Parses exactly `a.b.c.d` with four decimal octets in 0..=255. A name falls
// through to DNS in `main`.
function parse_ipv4(text: const string[0]; value: var uint): bool;
var
  at: int;
  start: int;
  length: int;
  digits: int;
  count: int;
  copied: int;
  octet: int;
  part: string[PartCapacity];
  acc: uint;
begin
  at := 0;
  start := 0;
  count := 0;
  acc := uint(0);
  length := int(core.len(text));
  while at <= length do
  begin
    if at = length or text[at] = byte(46) then
    begin
      digits := at - start;
      if digits <= 0 or digits > 3 then
        return false;
      copied := core.substr_into(part, text, start, digits);
      if copied < 0 then
        return false;
      if stdlib.parse_int_into(part, octet) <> Ok then
        return false;
      if octet < 0 or octet > 255 then
        return false;
      acc := acc * uint(256) + uint(octet);
      count := count + 1;
      start := at + 1;
    end;
    at := at + 1;
  end;
  if count <> 4 then
    return false;
  value := acc;
  return true;
end;

// Splits `host port` at the first space. The port must be 1..=65535.
function split_args(text: const string[0]; host: var string[HostCapacity];
                    port: var int): bool;
var
  length: int;
  space: int;
  host_len: int;
  port_len: int;
  copied: int;
  parsed: int;
  port_text: string[PartCapacity];
begin
  length := int(core.len(text));
  space := core.find(text, byte(32));
  if space <= 0 or space >= length - 1 then
    return false;
  host_len := space;
  if host_len > HostCapacity then
    return false;
  copied := core.substr_into(host, text, 0, host_len);
  if copied < 0 then
    return false;

  port_len := length - space - 1;
  if port_len <= 0 or port_len > 5 then
    return false;
  copied := core.substr_into(port_text, text, space + 1, port_len);
  if copied < 0 then
    return false;
  if stdlib.parse_int_into(port_text, parsed) <> Ok then
    return false;
  if parsed <= 0 or parsed > 65535 then
    return false;
  port := parsed;
  return true;
end;

procedure main(args: const string[1024]);
var
  line: string[ArgsCapacity];
  host: string[HostCapacity];
  digits: bytes[32];
  used: int;
  trimmed: int;
  port: int;
  ip: uint;
  mask: uint;
  gateway: uint;
  dns_server: uint;
  target: uint;
  id: int;
  sources: array[uint, 8];
  ready: int;
  got: int;
  sent: int;
  offset: int;
  copied: int;
  quitting: bool;
  st: Status;
  cwd: string[256];
  rest: string[256];
begin
  rest := "";
  st := path.split_cwd(args, cwd, rest);
  if int(core.len(rest)) = 0 then
  begin
    usage();
    return;
  end;
  trimmed := stdlib.trim_into(line, rest);
  if trimmed < 0 then
  begin
    usage();
    return;
  end;
  if not split_args(line, host, port) then
  begin
    usage();
    return;
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
    dns_server := uint(DnsIp);
  end;

  if not parse_ipv4(host, target) then
  begin
    if dns_server = uint(0) then
    begin
      fail("no DNS server configured");
      return;
    end;
    st := dns.resolve(host, dns_server, target);
    if st <> Ok then
    begin
      fail_status("dns failed:", st);
      return;
    end;
  end;

  st := socket.tcp(target, uint(port), id);
  if st <> Ok then
  begin
    fail_status("connect failed:", st);
    return;
  end;
  // No idle timeout: an interactive session may be quiet for a long time. The
  // TCP connect itself still has its 5 s deadline.
  socket.set_timeout(id, uint(0));

  core.write("nc: connecting to ");
  core.write(host);
  core.write(":");
  used := core.format_int_into(digits, port, uint(10), uint(0));
  core.write(digits);
  core.write("\n");

  quitting := false;
  while not quitting do
  begin
    sources[0] := uint(0);
    sources[1] := socket.source(id);
    ready := -1;
    st := core.next_any(sources, 2, ready);
    if st <> Ok then
    begin
      fail_status("select failed:", st);
      quitting := true;
    end
    else if ready = 0 then
    begin
      got := core.read_input(chunk);
      if got > 0 then
      begin
        if got = 1 and chunk[0] = byte(3) then
        begin
          core.write("^C\n");
          quitting := true;
        end
        else if got = 1 and chunk[0] = byte(4) then
        begin
          quitting := true;
        end
        else
        begin
          // Raw console: echo locally so typing is visible, then forward.
          core.write(chunk);
          offset := 0;
          while offset < got and not quitting do
          begin
            copied := core.substr_into(stage, chunk, offset, got - offset);
            if copied < 0 then
            begin
              fail("send staging failed");
              quitting := true;
            end
            else
            begin
              st := socket.send(id, stage, sent);
              if st = Ok then
              begin
                if sent <= 0 then
                begin
                  fail("send made no progress");
                  quitting := true;
                end
                else
                  offset := offset + sent;
              end
              else if st = Closed then
              begin
                core.write("nc: closed\n");
                quitting := true;
              end
              else
              begin
                fail_status("send failed:", st);
                quitting := true;
              end;
            end;
          end;
        end;
      end;
    end
    else if ready = 1 then
    begin
      st := socket.recv(id, chunk, got);
      if st = Ok then
      begin
        if got = 0 then
        begin
          core.write("nc: closed\n");
          quitting := true;
        end
        else
          core.write(chunk);
      end
      else if st = Closed then
      begin
        core.write("nc: closed\n");
        quitting := true;
      end
      else if st = WouldBlock then
      begin
        // A readiness snapshot can be stale; try again.
      end
      else
      begin
        fail_status("recv failed:", st);
        quitting := true;
      end;
    end
    else
    begin
      fail("bad source");
      quitting := true;
    end;
  end;
  socket.close(id);
end;

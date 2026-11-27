unit nslookup;

// A small DNS A-record lookup (spec 0020, N16).
//
//   nslookup example.com
//   nslookup example.com 10.0.2.3
//   nslookup 10.0.2.2
//
// The server defaults to the machine-wide DNS value from `core.net_info`; an
// optional second argument overrides it. A numeric name is answered locally
// without a query. `lib.dns` sends one UDP A query with its own retry/deadline
// policy, so a dead server prints a timeout and the task stays in budget.

uses core, stdlib, lib.socket, lib.dns, lib.path;

const
  HeadCapacity = 128;
  NameCapacity = 256;
  PartCapacity = 8;

procedure usage();
begin
  core.write("usage: nslookup <name> [server]\n");
end;

procedure write_octet(value: uint);
var
  digits: bytes[8];
  used: int;
begin
  used := core.format_int_into(digits, int(value), uint(10), uint(0));
  core.write(digits);
end;

// Writes one dotted-quad address in host order (first octet in bits 24..31).
procedure write_address(value: uint);
begin
  write_octet((value div uint(16777216)) mod uint(256));
  core.write(".");
  write_octet((value div uint(65536)) mod uint(256));
  core.write(".");
  write_octet((value div uint(256)) mod uint(256));
  core.write(".");
  write_octet(value mod uint(256));
end;

procedure write_status(st: Status);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, int(st), uint(10), uint(0));
  core.write(digits);
end;

// Parses exactly `a.b.c.d` with four decimal octets in 0..=255.
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

procedure main(args: const string[1024]);
var
  text: string[256];
  head: string[HeadCapacity];
  tail: string[256];
  name: string[NameCapacity];
  server_text: string[256];
  ip: uint;
  mask: uint;
  gateway: uint;
  dns: uint;
  server: uint;
  answer: uint;
  st: Status;
  split: int;
  trimmed: int;
  cwd: string[256];
  rest: string[256];
begin
  text := "";
  rest := "";
  st := path.split_cwd(args, cwd, rest);
  trimmed := stdlib.trim_into(text, rest);
  if trimmed < 0 then
  begin
    usage();
    return;
  end;
  if int(core.len(text)) = 0 then
  begin
    usage();
    return;
  end;

  split := stdlib.split_once(head, tail, text, byte(32));
  if split < 0 then
  begin
    usage();
    return;
  end;
  if stdlib.trim_into(name, head) < 0 or int(core.len(name)) = 0 then
  begin
    usage();
    return;
  end;
  server_text := "";
  if stdlib.trim_into(server_text, tail) < 0 then
  begin
    usage();
    return;
  end;

  server := uint(0);
  if int(core.len(server_text)) > 0 then
  begin
    if not parse_ipv4(server_text, server) then
    begin
      core.write("nslookup: invalid server address\n");
      return;
    end;
  end
  else
  begin
    st := core.net_info(ip, mask, gateway, dns);
    if st <> Ok then
    begin
      core.write("nslookup: no network device\n");
      return;
    end;
    server := dns;
    if server = uint(0) then
    begin
      core.write("nslookup: no DNS server configured (run 'ifconfig dhcp')\n");
      return;
    end;
  end;

  core.write("Server:  ");
  write_address(server);
  core.write("\nName:    ");
  core.write(name);
  core.write("\n");

  // A numeric name needs no query.
  if parse_ipv4(name, answer) then
  begin
    core.write("Address: ");
    write_address(answer);
    core.write("\n");
    return;
  end;

  st := dns.resolve(name, server, answer);
  if st = Ok then
  begin
    core.write("Address: ");
    write_address(answer);
    core.write("\n");
  end
  else if st = NotFound then
  begin
    core.write("nslookup: ");
    core.write(name);
    core.write(": NXDOMAIN (no A record)\n");
  end
  else if st = WouldBlock then
  begin
    core.write("nslookup: ");
    core.write(name);
    core.write(": timed out\n");
  end
  else
  begin
    core.write("nslookup: ");
    core.write(name);
    core.write(": failed ");
    write_status(st);
    core.write("\n");
  end;
end;

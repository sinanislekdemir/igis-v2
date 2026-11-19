unit purl;

// A deliberately small `curl`-like command: one HTTP/1.0 GET, body to the
// console. It is an ordinary application, not a shell builtin:
//
//   run purl http://10.0.2.2:6666/
//   run purl http://example.com/
//
// Supported URL shape: `http://host[:port][/path]`, with a dotted IPv4 host or
// a DNS name. Defaults are port 80 and path `/`. There is no HTTPS, no
// redirect following, no request headers, no POST and no output file; the body
// is capped by `lib/http` at 4096 bytes. DNS uses the machine's configured
// server; if the machine has no address yet (the fresh QEMU test shell), the
// QEMU user-network defaults are applied: 10.0.2.15/24, gateway 10.0.2.2,
// DNS 10.0.2.3. On a configured machine the existing settings are kept.

uses core, stdlib, lib.socket, lib.http, lib.dns;

const
  UrlCapacity = 256;
  HostCapacity = 128;
  PathCapacity = 256;
  HeaderCapacity = 160;
  BodyCapacity = 4096;
  PartCapacity = 8;

  DefaultPort = 80;
  SchemeLength = 7;  // "http://"

  // QEMU user networking defaults, used only when no address is configured.
  LocalIp = 0x0A00020F;
  NetMask = 0xFFFFFF00;
  GatewayIp = 0x0A000202;
  DnsIp = 0x0A000203;

var
  body: string[BodyCapacity];

procedure usage();
begin
  core.write("purl: usage: purl http://host[:port][/path]\n");
end;

procedure fail(text: const string[0]);
begin
  core.write("purl: ");
  core.write(text);
  core.write("\n");
end;

procedure fail_status(prefix: const string[0]; st: Status);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, int(st), uint(10), uint(0));
  core.write("purl: ");
  core.write(prefix);
  core.write(" ");
  core.write(digits);
  core.write("\n");
end;

// Parses exactly `a.b.c.d` with four decimal octets in 0..=255. Returns false
// for a DNS name or a malformed address, so the caller falls back to DNS.
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

// Copies the URL without its `http://` prefix.
function strip_scheme(url: const string[0]; rest: var string[UrlCapacity]): bool;
var
  length: int;
  copied: int;
begin
  if not stdlib.starts_with(url, "http://") then
    return false;
  length := int(core.len(url));
  if length <= SchemeLength then
    return false;
  copied := core.substr_into(rest, url, SchemeLength, length - SchemeLength);
  return copied >= 0;
end;

// Parses `host[:port][/path]` into host, port and path. Returns false for an
// empty host, a bad port or an over-capacity component.
function split_authority(rest: const string[0]; host: var string[HostCapacity];
                         port: var int; path: var string[PathCapacity]): bool;
var
  length: int;
  slash: int;
  colon: int;
  at: int;
  host_len: int;
  port_len: int;
  copied: int;
  parsed: int;
  port_text: string[PartCapacity];
begin
  length := int(core.len(rest));
  slash := core.find(rest, byte(47));
  if slash < 0 then
    slash := length;

  colon := -1;
  at := 0;
  while at < slash do
  begin
    if rest[at] = byte(58) then
    begin
      colon := at;
      at := slash;
    end
    else
      at := at + 1;
  end;

  if colon < 0 then
  begin
    host_len := slash;
    port := DefaultPort;
  end
  else
  begin
    host_len := colon;
    port_len := slash - colon - 1;
    if port_len <= 0 or port_len > 5 then
      return false;
    copied := core.substr_into(port_text, rest, colon + 1, port_len);
    if copied < 0 then
      return false;
    if stdlib.parse_int_into(port_text, parsed) <> Ok then
      return false;
    if parsed <= 0 or parsed > 65535 then
      return false;
    port := parsed;
  end;

  if host_len <= 0 or host_len > HostCapacity then
    return false;
  copied := core.substr_into(host, rest, 0, host_len);
  if copied < 0 then
    return false;

  if slash = length then
  begin
    copied := core.substr_into(path, "/", 0, 1);
    return copied = 1;
  end;
  if length - slash > PathCapacity then
    return false;
  copied := core.substr_into(path, rest, slash, length - slash);
  return copied >= 0;
end;

// `lib.dns` is imported as a unit; the call site is `dns.resolve`. The local
// configuration variable is named `dns_server` so it does not shadow the unit.

procedure main(args: const string[1024]);
var
  url: string[UrlCapacity];
  rest: string[UrlCapacity];
  host: string[HostCapacity];
  path: string[PathCapacity];
  header: string[HeaderCapacity];
  port_text: string[PartCapacity];
  digits: bytes[32];
  used: int;
  copied: int;
  trimmed: int;
  port: int;
  ip: uint;
  mask: uint;
  gateway: uint;
  dns_server: uint;
  target: uint;
  length: int;
  st: Status;
begin
  if int(core.len(args)) = 0 then
  begin
    usage();
    return;
  end;
  trimmed := stdlib.trim_into(url, args);
  if trimmed < 0 then
  begin
    usage();
    return;
  end;

  if not strip_scheme(url, rest) then
  begin
    fail("only http:// URLs are supported");
    return;
  end;
  if not split_authority(rest, host, port, path) then
  begin
    usage();
    return;
  end;

  // Use the machine's configuration. A fresh QEMU test shell has no address,
  // so apply the user-network defaults; an already configured machine keeps
  // its own address and DNS server.
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
  // A dotted IPv4 never touches DNS; a name goes to the configured server.
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

  // HTTP/1.0 Host header. A non-default port is included, as curl does.
  copied := core.substr_into(header, host, 0, int(core.len(host)));
  if copied < 0 then
  begin
    fail("host too long");
    return;
  end;
  if port <> DefaultPort then
  begin
    used := core.format_int_into(digits, port, uint(10), uint(0));
    if used < 0 or used > PartCapacity then
    begin
      fail("port too long");
      return;
    end;
    copied := core.substr_into(port_text, digits, 0, used);
    if copied < 0 then
    begin
      fail("port too long");
      return;
    end;
    header := host + ":" + port_text;
  end;

  st := http.get(target, uint(port), header, path, body);
  if st <> Ok then
  begin
    if st = WouldBlock then
      fail("timeout")
    else if st = Closed then
      fail("closed")
    else
      fail_status("failed:", st);
    return;
  end;

  length := http.body_length(body);
  core.write(body);
  if length = 0 then
    core.write("\n")
  else if body[length - 1] <> byte(10) then
    core.write("\n");
end;

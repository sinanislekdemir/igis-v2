unit ifconfig;

// Network interface status and configuration (spec 0020).
//
//   ifconfig                                  show the current configuration
//   ifconfig dhcp                             obtain and apply a DHCP lease
//   ifconfig <ip> <netmask> <gateway> [dns]   apply a static configuration
//
// The address family is IPv4 only and there is one interface, so the output is
// a single `eth0:` block. `core.net_info` reads the machine-wide values and
// `core.net_mac` the device address; `core.net_stats` adds the bounded wire
// counters. Dotted quads are parsed locally into host-order `uint`s (the first
// octet in bits 24..31), the same encoding `core.net_config` takes.

uses core, stdlib, lib.socket, lib.dhcp;

const
  PartCapacity = 8;

procedure usage();
begin
  core.write("usage: ifconfig [dhcp | <ip> <netmask> <gateway> [dns]]\n");
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

procedure write_uint(value: uint);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, int(value), uint(10), uint(0));
  core.write(digits);
end;

procedure write_hex_byte(value: uint);
var
  digits: bytes[8];
  used: int;
begin
  used := core.format_int_into(digits, int(value), uint(16), uint(2));
  core.write(digits);
end;

// The six bytes of the packed MAC, most significant first.
procedure write_mac(mac: uint);
begin
  write_hex_byte((mac div uint(0x10000000000)) mod uint(256));
  core.write(":");
  write_hex_byte((mac div uint(0x100000000)) mod uint(256));
  core.write(":");
  write_hex_byte((mac div uint(0x1000000)) mod uint(256));
  core.write(":");
  write_hex_byte((mac div uint(0x10000)) mod uint(256));
  core.write(":");
  write_hex_byte((mac div uint(0x100)) mod uint(256));
  core.write(":");
  write_hex_byte(mac mod uint(256));
end;

// The number of set bits of a contiguous netmask, i.e. its prefix length.
function prefix_length(mask: uint): int;
var
  bits: int;
  value: uint;
begin
  bits := 0;
  value := mask;
  while value <> uint(0) do
  begin
    if value mod uint(2) = uint(1) then
      bits := bits + 1;
    value := value div uint(2);
  end;
  return bits;
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

procedure show_interface();
var
  ip: uint;
  mask: uint;
  gateway: uint;
  dns: uint;
  rx: uint;
  tx: uint;
  drops: uint;
  errors: uint;
  st: Status;
begin
  st := core.net_info(ip, mask, gateway, dns);
  if st <> Ok then
  begin
    core.write("eth0: no network device\n");
    return;
  end;

  core.write("eth0: flags=");
  if ip = uint(0) then
    core.write("DOWN\n")
  else
    core.write("UP\n");
  core.write("        inet ");
  write_address(ip);
  core.write("/");
  write_uint(uint(prefix_length(mask)));
  core.write(" netmask ");
  write_address(mask);
  core.write("\n        ether ");
  write_mac(core.net_mac());
  core.write("\n        gateway ");
  write_address(gateway);
  core.write(" dns ");
  write_address(dns);
  core.write("\n");

  st := core.net_stats(rx, tx, drops, errors);
  if st = Ok then
  begin
    core.write("        RX ");
    write_uint(rx);
    core.write(" frames  TX ");
    write_uint(tx);
    core.write(" frames  drops ");
    write_uint(drops);
    core.write("  errors ");
    write_uint(errors);
    core.write("\n");
  end;

  if ip = uint(0) then
    core.write("ifconfig: interface is not configured (try 'ifconfig dhcp')\n");
end;

procedure apply_dhcp();
var
  ip: uint;
  mask: uint;
  gateway: uint;
  dns: uint;
  st: Status;
begin
  st := dhcp.acquire(ip, mask, gateway, dns);
  if st = IoError then
  begin
    core.write("ifconfig: no network device\n");
    return;
  end;
  if st <> Ok then
  begin
    core.write("ifconfig: dhcp failed ");
    write_uint(uint(int(st)));
    core.write("\n");
    return;
  end;
  st := core.net_config(ip, mask, gateway, dns);
  if st <> Ok then
  begin
    core.write("ifconfig: no network device\n");
    return;
  end;
  core.write("ifconfig: dhcp ");
  write_address(ip);
  core.write("/");
  write_uint(uint(prefix_length(mask)));
  core.write(" gw ");
  write_address(gateway);
  core.write(" dns ");
  write_address(dns);
  core.write("\n");
  show_interface();
end;

procedure main(args: const string[1024]);
var
  text: string[256];
  parts: array[string[64], 8];
  count: int;
  ip: uint;
  mask: uint;
  gateway: uint;
  dns: uint;
  current_ip: uint;
  current_mask: uint;
  current_gateway: uint;
  current_dns: uint;
  st: Status;
  trimmed: int;
begin
  text := "";
  if int(core.len(args)) > 0 then
  begin
    trimmed := stdlib.trim_into(text, args);
    if trimmed < 0 then
    begin
      usage();
      return;
    end;
  end;

  if int(core.len(text)) = 0 then
  begin
    show_interface();
    return;
  end;
  if text = "dhcp" then
  begin
    apply_dhcp();
    return;
  end;

  st := stdlib.split_all(parts, count, text, byte(32));
  if st <> Ok or count < 3 or count > 4 then
  begin
    usage();
    return;
  end;
  if not parse_ipv4(parts[0], ip)
    or not parse_ipv4(parts[1], mask)
    or not parse_ipv4(parts[2], gateway) then
  begin
    usage();
    return;
  end;
  dns := uint(0);
  if count = 4 then
  begin
    if not parse_ipv4(parts[3], dns) then
    begin
      usage();
      return;
    end;
  end
  else
  begin
    // Three addresses: keep the configured DNS server if there is one.
    st := core.net_info(current_ip, current_mask, current_gateway, current_dns);
    if st = Ok then
      dns := current_dns;
  end;

  st := core.net_config(ip, mask, gateway, dns);
  if st <> Ok then
  begin
    core.write("ifconfig: no network device\n");
    return;
  end;
  show_interface();
end;

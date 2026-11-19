unit dhcp;

// Importable DHCP client (spec 0020, N8/N18; PLAN M10.8). Ordinary `/lib`
// module source on top of the `socket` unit: no `main`, no top-level `var`
// state, no stream endpoint and no kernel feature. It builds a BOOTP/DHCP
// message by hand in a stack buffer, sends it as one UDP datagram from port 68
// to the limited broadcast address, validates the server's reply and returns
// the offered address, netmask, router and DNS server. The caller applies them
// with `core.net_config`.
//
//   st := dhcp.acquire(ip, mask, gateway, dns);
//
// The socket surface is non-blocking in this slice, so every send and receive
// retries while the stack reports `WouldBlock`, bounded by a `core.clock()`
// deadline per attempt, a retry cap and one overall exchange deadline. The
// exchange is DISCOVER → OFFER → REQUEST → ACK; a server that never answers is
// `WouldBlock`, a malformed reply is `Invalid`, a NAK restarts discovery once
// and a second NAK is `NotFound`. The socket is closed on every path, and the
// four outputs are written only on `Ok`.
//
// Deliberate limits: no lease renewal (option 51 is parsed and ignored), no
// INFORM/DECLINE/RELEASE, no relay (`giaddr`) and no client-identifier option.
// The offered address is trusted because the reply must match the transaction
// id and the magic cookie; there is no server authentication beyond that.
//
// QEMU's slirp DHCP server (`10.0.2.2:67`) is the target. It replies to the
// offered address with the client MAC, broadcast when the BOOTP broadcast flag
// is set, and the socket is opened to the broadcast address so a reply from
// any source is accepted (spec 0020, N8).

uses core, stdlib, lib.socket;

const
  // One BOOTP header plus the options; every DHCP message here is padded to
  // `MessageBytes` (the BOOTP minimum is 300), which is also the buffer's
  // runtime length, so one send copies the whole datagram.
  PacketCapacity = 300;
  ReceiveCapacity = 1024;
  MessageBytes = 300;
  OptionsAt = 240;
  ServerPort = 67;
  ClientPort = 68;
  BroadcastIp = 0xFFFFFFFF;

  // The whole exchange must finish inside this window of `core.clock()`. The
  // per-attempt deadline is the smaller of two seconds and what is left, so a
  // dead server cannot burn the task's 10 s CPU budget.
  AttemptNs = 2000000000;
  OverallNs = 6000000000;
  // Bounded polling, so a clock that never advances still cannot spin. These
  // caps sit far beyond the real exchange (a few iterations on QEMU).
  SendAttempts = 400000;
  RecvAttempts = 400000;

  // Four attempts each, as the ticket asks, inside the overall deadline.
  DiscoverAttempts = 4;
  RequestAttempts = 4;
  // A NAK restarts discovery; a second NAK is final.
  MaxCycles = 2;

  TypeDiscover = 1;
  TypeOffer = 2;
  TypeRequest = 3;
  TypeAck = 5;
  TypeNak = 6;

  // Option codes used here.
  OptionNetmask = 1;
  OptionRouter = 3;
  OptionDns = 6;
  OptionRequested = 50;
  OptionLease = 51;
  OptionMessageType = 53;
  OptionServerId = 54;
  OptionParamList = 55;
  OptionPad = 0;
  OptionEnd = 255;

// Reads a big-endian 32-bit field. The caller guarantees `at + 3 < length`.
function read_u32(buf: const bytes[0]; at: int): uint;
begin
  return uint(buf[at]) * uint(16777216)
    + uint(buf[at + 1]) * uint(65536)
    + uint(buf[at + 2]) * uint(256)
    + uint(buf[at + 3]);
end;

// Writes a big-endian 32-bit field. The caller guarantees `at + 3` is inside
// the packet buffer.
procedure write_u32(packet: var bytes[PacketCapacity]; at: int; value: uint);
begin
  packet[at] := byte(value div uint(16777216));
  packet[at + 1] := byte((value div uint(65536)) mod uint(256));
  packet[at + 2] := byte((value div uint(256)) mod uint(256));
  packet[at + 3] := byte(value mod uint(256));
end;

// Zeroes the packet, writes the fixed BOOTP header and the magic cookie, and
// returns the option offset. `chaddr` is the packed `core.net_mac()` value:
// octet 0 in bits 40..47, which is also the wire order.
function write_header(packet: var bytes[PacketCapacity]; xid: uint;
                      mac: uint): int;
var
  n: int;
begin
  n := 0;
  while n < PacketCapacity do
  begin
    packet[n] := byte(0);
    n := n + 1;
  end;
  packet[0] := byte(1);  // op: BOOTREQUEST
  packet[1] := byte(1);  // htype: Ethernet
  packet[2] := byte(6);  // hlen
  write_u32(packet, 4, xid);
  // secs (8..10) stays zero; the broadcast flag (10..12) is 0x8000, so the
  // server answers before the client has an address.
  packet[10] := byte(128);
  // ciaddr, yiaddr, siaddr and giaddr stay zero.
  packet[28] := byte((mac div uint(1099511627776)) mod uint(256));
  packet[29] := byte((mac div uint(4294967296)) mod uint(256));
  packet[30] := byte((mac div uint(16777216)) mod uint(256));
  packet[31] := byte((mac div uint(65536)) mod uint(256));
  packet[32] := byte((mac div uint(256)) mod uint(256));
  packet[33] := byte(mac mod uint(256));
  // Magic cookie 99.130.83.99.
  packet[236] := byte(99);
  packet[237] := byte(130);
  packet[238] := byte(83);
  packet[239] := byte(99);
  return OptionsAt;
end;

// Appends option 55: request the netmask, the router and the DNS server.
function write_param_list(packet: var bytes[PacketCapacity]; at: int): int;
var
  pos: int;
begin
  pos := at;
  packet[pos] := byte(OptionParamList);
  pos := pos + 1;
  packet[pos] := byte(3);
  pos := pos + 1;
  packet[pos] := byte(OptionNetmask);
  pos := pos + 1;
  packet[pos] := byte(OptionRouter);
  pos := pos + 1;
  packet[pos] := byte(OptionDns);
  pos := pos + 1;
  return pos;
end;

// Builds one DHCPDISCOVER and returns the datagram length. The packet is
// zero-padded to `MessageBytes`, so the option area ends with 255 and PAD.
function build_discover(packet: var bytes[PacketCapacity]; xid: uint;
                        mac: uint): int;
var
  at: int;
begin
  at := write_header(packet, xid, mac);
  packet[at] := byte(OptionMessageType);
  packet[at + 1] := byte(1);
  packet[at + 2] := byte(TypeDiscover);
  at := at + 3;
  at := write_param_list(packet, at);
  packet[at] := byte(OptionEnd);
  return MessageBytes;
end;

// Builds one DHCPREQUEST for the offered address and returns its length.
function build_request(packet: var bytes[PacketCapacity]; xid: uint; mac: uint;
                       offered: uint; server: uint): int;
var
  at: int;
begin
  at := write_header(packet, xid, mac);
  packet[at] := byte(OptionMessageType);
  packet[at + 1] := byte(1);
  packet[at + 2] := byte(TypeRequest);
  at := at + 3;
  packet[at] := byte(OptionRequested);
  packet[at + 1] := byte(4);
  write_u32(packet, at + 2, offered);
  at := at + 6;
  packet[at] := byte(OptionServerId);
  packet[at + 1] := byte(4);
  write_u32(packet, at + 2, server);
  at := at + 6;
  at := write_param_list(packet, at);
  packet[at] := byte(OptionEnd);
  return MessageBytes;
end;

// Validates one BOOTREPLY and publishes its fields only on `Ok`. `NotFound`
// means the datagram does not belong to this transaction (a foreign id or a
// message that is not a BOOTREPLY), so the caller keeps waiting; `Invalid` is
// a datagram that claims to be ours but is malformed.
//
// `msg_type` is the option-53 value for `Ok`, including a NAK (6); the caller
// decides what that means. Option 51 (lease) is parsed and discarded: renewal
// is out of scope.
function parse_reply(buf: const bytes[0]; length: int; xid: uint;
                     ip: var uint; server: var uint; mask: var uint;
                     gateway: var uint; dns: var uint;
                     msg_type: var uint): Status;
var
  at: int;
  code: int;
  olen: int;
  found: bool;
  mtype: uint;
  addr: uint;
  server_id: uint;
  netmask: uint;
  router: uint;
  dns_server: uint;
  lease: uint;
begin
  if length < OptionsAt then
    return Invalid;
  if buf[0] <> byte(2) then
    return NotFound;
  if read_u32(buf, 4) <> xid then
    return NotFound;
  if buf[236] <> byte(99) or buf[237] <> byte(130)
      or buf[238] <> byte(83) or buf[239] <> byte(99) then
    return Invalid;

  addr := read_u32(buf, 16);
  server_id := read_u32(buf, 20);
  netmask := uint(0);
  router := uint(0);
  dns_server := uint(0);
  lease := uint(0);
  mtype := uint(0);
  found := false;
  at := OptionsAt;
  while at < length do
  begin
    code := int(buf[at]);
    if code = OptionEnd then
      at := length
    else if code = OptionPad then
      at := at + 1
    else
    begin
      if at + 1 >= length then
        return Invalid;
      olen := int(buf[at + 1]);
      if at + 2 + olen > length then
        return Invalid;
      if code = OptionMessageType and olen = 1 then
      begin
        mtype := uint(buf[at + 2]);
        found := true;
      end
      else if code = OptionNetmask and olen = 4 then
        netmask := read_u32(buf, at + 2)
      else if code = OptionRouter and olen >= 4 then
        router := read_u32(buf, at + 2)
      else if code = OptionDns and olen >= 4 then
        dns_server := read_u32(buf, at + 2)
      else if code = OptionServerId and olen = 4 then
        server_id := read_u32(buf, at + 2)
      else if code = OptionLease and olen = 4 then
        lease := read_u32(buf, at + 2);
      at := at + 2 + olen;
    end;
  end;
  if not found then
    return Invalid;

  ip := addr;
  server := server_id;
  mask := netmask;
  gateway := router;
  dns := dns_server;
  msg_type := mtype;
  return Ok;
end;

// Sends one datagram, retrying `WouldBlock` until `until` or the cap.
function transmit(handle: int; packet: const bytes[0]; length: int;
                  until: uint): Status;
var
  st: Status;
  sent: int;
  attempts: int;
begin
  attempts := 0;
  while attempts < SendAttempts do
  begin
    st := socket.send(handle, packet, sent);
    if st = Ok then
    begin
      if sent = length then
        return Ok;
      // One UDP send copies the whole datagram or reports a failure.
      return CapacityExceeded;
    end;
    if st <> WouldBlock then
      return st;
    if core.clock() >= until then
      return WouldBlock;
    attempts := attempts + 1;
  end;
  return WouldBlock;
end;

// Waits for a reply of type `want`. `Ok` publishes its fields, `NotFound` is a
// NAK, `WouldBlock` is the deadline or the retry cap and any other status is a
// socket failure. Datagrams for another transaction are ignored until the
// deadline; a malformed one ends the attempt with `Invalid`.
function await_reply(handle: int; xid: uint; want: uint; until: uint;
                     ip: var uint; server: var uint; mask: var uint;
                     gateway: var uint; dns: var uint;
                     msg_type: var uint): Status;
var
  reply: bytes[ReceiveCapacity];
  read: int;
  st: Status;
  attempts: int;
  done: bool;
  mtype: uint;
begin
  attempts := 0;
  done := false;
  while not done and attempts < RecvAttempts do
  begin
    st := socket.recv(handle, reply, read);
    if st = Ok then
    begin
      st := parse_reply(reply, read, xid, ip, server, mask, gateway, dns, mtype);
      if st = Ok then
      begin
        if mtype = want then
        begin
          msg_type := mtype;
          done := true;
        end
        else if mtype = uint(TypeNak) then
        begin
          msg_type := mtype;
          return NotFound;
        end
        else
          attempts := attempts + 1;
      end
      else if st = NotFound then
        attempts := attempts + 1
      else
        return st;
    end
    else if st = WouldBlock then
    begin
      if core.clock() >= until then
        return WouldBlock;
      attempts := attempts + 1;
    end
    else
      return st;
  end;
  if not done then
    return WouldBlock;
  return Ok;
end;

// Broadcasts DISCOVER until an OFFER arrives or the deadline passes.
function discover(handle: int; packet: var bytes[PacketCapacity]; length: int;
                  xid: uint; overall: uint; offered: var uint;
                  server: var uint; mask: var uint; gateway: var uint;
                  dns: var uint): Status;
var
  attempt: int;
  until: uint;
  st: Status;
  msg_type: uint;
  got_offer: bool;
begin
  attempt := 0;
  got_offer := false;
  while attempt < DiscoverAttempts and not got_offer do
  begin
    if core.clock() >= overall then
      attempt := DiscoverAttempts
    else
    begin
      until := core.clock() + uint(AttemptNs);
      if until > overall then
        until := overall;
      st := transmit(handle, packet, length, until);
      if st = Ok then
      begin
        st := await_reply(handle, xid, uint(TypeOffer), until, offered, server,
          mask, gateway, dns, msg_type);
        if st = Ok then
          got_offer := true
        else if st = NotFound then
          // A NAK before an offer: wait for the next offer.
          st := WouldBlock
        else if st <> WouldBlock then
          return st;
      end
      else if st <> WouldBlock then
        return st;
      if not got_offer then
        attempt := attempt + 1;
    end;
  end;
  if not got_offer then
    return WouldBlock;
  return Ok;
end;

// Sends REQUEST for the offered address and waits for the ACK. `NotFound` is a
// NAK; see `await_reply`.
function request(handle: int; packet: var bytes[PacketCapacity]; length: int;
                 xid: uint; overall: uint; ip: var uint; mask: var uint;
                 gateway: var uint; dns: var uint): Status;
var
  attempt: int;
  until: uint;
  st: Status;
  msg_type: uint;
  server: uint;
  got_ack: bool;
begin
  attempt := 0;
  got_ack := false;
  while attempt < RequestAttempts and not got_ack do
  begin
    if core.clock() >= overall then
      attempt := RequestAttempts
    else
    begin
      until := core.clock() + uint(AttemptNs);
      if until > overall then
        until := overall;
      st := transmit(handle, packet, length, until);
      if st = Ok then
      begin
        st := await_reply(handle, xid, uint(TypeAck), until, ip, server, mask,
          gateway, dns, msg_type);
        if st = Ok then
          got_ack := true
        else if st = NotFound then
          return NotFound;
        else if st <> WouldBlock then
          return st;
      end
      else if st <> WouldBlock then
        return st;
      if not got_ack then
        attempt := attempt + 1;
    end;
  end;
  if not got_ack then
    return WouldBlock;
  return Ok;
end;

// Obtains one lease from the DHCP server on the local link. `Ok` writes the
// address, netmask, router and DNS server (each in host order, the first octet
// in bits 24..31); every failure leaves all four untouched.
function acquire(ip: var uint; mask: var uint; gateway: var uint;
                 dns: var uint): Status;
var
  handle: int;
  st: Status;
  packet: bytes[PacketCapacity];
  length: int;
  mac: uint;
  xid: uint;
  overall: uint;
  cycle: int;
  offered: uint;
  server: uint;
  offer_mask: uint;
  offer_gateway: uint;
  offer_dns: uint;
  lease_ip: uint;
  lease_mask: uint;
  lease_gateway: uint;
  lease_dns: uint;
  result: Status;
begin
  mac := core.net_mac();
  st := socket.udp_from(uint(ClientPort), uint(BroadcastIp), uint(ServerPort),
    handle);
  if st <> Ok then
    return st;
  // Each attempt is bounded by two seconds; an expired deadline returns
  // `WouldBlock` and the loop's own `until` check decides whether to retry.
  st := socket.set_timeout(handle, uint(2000));
  if st <> Ok then
  begin
    socket.close(handle);
    return st;
  end;

  overall := core.clock() + uint(OverallNs);
  offered := uint(0);
  server := uint(0);
  offer_mask := uint(0);
  offer_gateway := uint(0);
  offer_dns := uint(0);
  lease_ip := uint(0);
  lease_mask := uint(0);
  lease_gateway := uint(0);
  lease_dns := uint(0);
  result := WouldBlock;

  cycle := 0;
  while cycle < MaxCycles do
  begin
    // A fresh transaction id per cycle, so a late reply from the previous one
    // is rejected as foreign.
    xid := (core.clock() + uint(cycle) + uint(1)) mod uint(4294967296);
    length := build_discover(packet, xid, mac);
    st := discover(handle, packet, length, xid, overall, offered, server,
      offer_mask, offer_gateway, offer_dns);
    if st <> Ok then
    begin
      result := st;
      cycle := MaxCycles;
    end
    else
    begin
      if offered = uint(0) then
      begin
        result := Invalid;
        cycle := MaxCycles;
      end
      else
      begin
        // A DHCP server names itself in siaddr or option 54; `parse_reply`
        // prefers the option, and a zero here is a malformed offer.
        length := build_request(packet, xid, mac, offered, server);
        st := request(handle, packet, length, xid, overall, lease_ip,
          lease_mask, lease_gateway, lease_dns);
        if st = Ok then
        begin
          result := Ok;
          cycle := MaxCycles;
        end
        else if st = NotFound then
        begin
          // A NAK: restart discovery with a new transaction id.
          result := WouldBlock;
          cycle := cycle + 1;
        end
        else
        begin
          result := st;
          cycle := MaxCycles;
        end;
      end;
    end;
  end;

  socket.close(handle);
  if result <> Ok then
    return result;
  if lease_ip = uint(0) then
    return Invalid;
  // A server may repeat only some options in the ACK; the offer fills the
  // gaps.
  if lease_mask = uint(0) then
    lease_mask := offer_mask;
  if lease_gateway = uint(0) then
    lease_gateway := offer_gateway;
  if lease_dns = uint(0) then
    lease_dns := offer_dns;

  ip := lease_ip;
  mask := lease_mask;
  gateway := lease_gateway;
  dns := lease_dns;
  return Ok;
end;

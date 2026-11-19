unit dns;

// Importable DNS A-record resolver (spec 0020, N16). Ordinary `/lib` module
// source on top of the `socket` unit: no `main`, no top-level `var` state, no
// stream endpoint and no kernel feature. The wire stack stays in the kernel;
// this unit builds one standard recursive query, sends it as one UDP
// datagram, validates the response and returns the first `A` record as a
// `uint` in host order (the first octet in bits 24..31).
//
//   st := dns.resolve("example.com", uint(0x0A000203), ip);
//
// The socket surface is non-blocking in this slice, so the send and receive
// loops retry while the stack reports `WouldBlock`, bounded by a `core.clock()`
// deadline (2 s per attempt) and by a retry cap. A timeout is `WouldBlock` and
// the caller reports it; `NXDOMAIN` is `NotFound`; a malformed or truncated
// message is `Invalid`. The socket is always closed, on every path, and the
// destination is written only on `Ok`.
//
// Deliberate limits: one question, type A only, no EDNS, no TCP fallback, no
// caching and no search list. An answer name is skipped as labels or a
// compression pointer; a CNAME chain inside the same response is followed up
// to four links, and a response without an `A` record is `NotFound`.

uses core, stdlib, lib.socket;

const
  // The query is 12 header bytes + the encoded name + 4 question bytes; a
  // 255-byte name is the protocol maximum, so 512 is generous.
  QueryCapacity = 512;
  // A DNS message without EDNS is at most 512 bytes. A larger datagram is
  // truncated by `core.socket_recv` and rejected as malformed.
  ResponseCapacity = 512;
  HeaderBytes = 12;
  DnsPort = 53;

  // The whole exchange must finish inside this window of `core.clock()`.
  DeadlineNs = 2000000000;
  // One retry: a second attempt after a timeout, two attempts in total.
  MaxAttempts = 2;
  // Bounded polling, so a dead server cannot burn the task's CPU budget. The
  // clock deadline is the primary bound (2 s per attempt); these caps sit
  // beyond it at the measured poll rate and only catch a clock that never
  // advances. Two attempts of ~2 s each stay well inside the normal task's
  // 10 s budget.
  SendAttempts = 400000;
  RecvAttempts = 400000;

  MaxNameBytes = 255;
  MaxLabelBytes = 63;
  MaxChain = 4;

  TypeA = 1;
  TypeCname = 5;
  ClassIn = 1;

// Reads a big-endian 16-bit field. The caller guarantees `at + 1 < length`.
function read_u16(buf: const bytes[0]; at: int): uint;
begin
  return uint(buf[at]) * uint(256) + uint(buf[at + 1]);
end;

// Reads a big-endian 32-bit field. The caller guarantees `at + 3 < length`.
function read_u32(buf: const bytes[0]; at: int): uint;
begin
  return uint(buf[at]) * uint(16777216)
    + uint(buf[at + 1]) * uint(65536)
    + uint(buf[at + 2]) * uint(256)
    + uint(buf[at + 3]);
end;

// Returns the index just past the (possibly compressed) name that starts at
// `at`, or -1 when the name is malformed or runs past `length`. A compression
// pointer ends the name in the wire encoding, so skipping never follows one
// and cannot loop.
function skip_name(buf: const bytes[0]; at: int; length: int): int;
var
  pos: int;
  b: byte;
  label: int;
begin
  pos := at;
  while pos < length do
  begin
    b := buf[pos];
    if b = byte(0) then
      return pos + 1;
    if b >= byte(192) then
    begin
      // A pointer is the two-byte end of the name.
      if pos + 1 >= length then
        return -1;
      return pos + 2;
    end;
    // 0x40/0x80 label types are not part of DNS.
    if b > byte(MaxLabelBytes) then
      return -1;
    label := int(b);
    pos := pos + 1 + label;
    if pos > length then
      return -1;
  end;
  return -1;
end;

// Builds the query into `query` and returns its byte length, or -1 when the
// name has an empty label or a label longer than 63 bytes.
function build_query(name: const string[0]; id: uint;
                     query: var bytes[QueryCapacity]): int;
var
  n: int;
  pos: int;
  label_start: int;
  label_len: int;
  j: int;
  name_len: int;
begin
  n := 0;
  // Header: id, flags 0x0100 (recursion desired), qdcount 1, other counts 0.
  query[n] := byte(id div uint(256)); n := n + 1;
  query[n] := byte(id mod uint(256)); n := n + 1;
  query[n] := byte(1); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(1); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;

  // QNAME: every label is length-prefixed, never compressed; the root label
  // is the zero byte written below.
  name_len := int(core.len(name));
  pos := 0;
  label_start := 0;
  while pos <= name_len do
  begin
    if pos = name_len or name[pos] = byte(46) then
    begin
      label_len := pos - label_start;
      if label_len = 0 or label_len > MaxLabelBytes then
        return -1;
      if n + 1 + label_len > QueryCapacity then
        return -1;
      query[n] := byte(label_len);
      n := n + 1;
      j := 0;
      while j < label_len do
      begin
        query[n] := name[label_start + j];
        n := n + 1;
        j := j + 1;
      end;
      label_start := pos + 1;
    end;
    pos := pos + 1;
  end;

  // Root label, QTYPE A, QCLASS IN.
  if n + 5 > QueryCapacity then
    return -1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(TypeA); n := n + 1;
  query[n] := byte(0); n := n + 1;
  query[n] := byte(ClassIn); n := n + 1;
  return n;
end;

// Validates one response and writes the first `A` record into `ip` only on
// `Ok`. `NXDOMAIN` is `NotFound`; a malformed or truncated message is
// `Invalid`; a well-formed response without an address is `NotFound`.
function parse_response(buf: const bytes[0]; length: int; id: uint;
                        ip: var uint): Status;
var
  flags: uint;
  rcode: uint;
  qdcount: uint;
  ancount: uint;
  pos: int;
  next: int;
  index: int;
  atype: uint;
  aclass: uint;
  rdlength: uint;
  rdata: int;
  links: int;
  value: uint;
begin
  if length < HeaderBytes then
    return Invalid;
  if read_u16(buf, 0) <> id then
    return Invalid;
  flags := read_u16(buf, 2);
  // QR must be set and the opcode must be a standard query.
  if flags div uint(32768) <> uint(1) then
    return Invalid;
  if (flags div uint(2048)) mod uint(16) <> uint(0) then
    return Invalid;
  // A truncated response is not usable at this size.
  if (flags div uint(512)) mod uint(2) <> uint(0) then
    return Invalid;
  rcode := flags mod uint(16);
  if rcode <> uint(0) then
  begin
    if rcode = uint(3) then
      return NotFound;
    return Invalid;
  end;

  qdcount := read_u16(buf, 4);
  ancount := read_u16(buf, 6);
  if qdcount <> uint(1) then
    return Invalid;
  if ancount = uint(0) then
    return NotFound;

  // Skip the echoed question: the name, then QTYPE and QCLASS.
  pos := skip_name(buf, HeaderBytes, length);
  if pos < 0 or pos + 4 > length then
    return Invalid;
  pos := pos + 4;

  links := 0;
  index := 0;
  while index < int(ancount) do
  begin
    next := skip_name(buf, pos, length);
    if next < 0 or next + 10 > length then
      return Invalid;
    atype := read_u16(buf, next);
    aclass := read_u16(buf, next + 2);
    rdlength := read_u16(buf, next + 8);
    rdata := next + 10;
    if rdata + int(rdlength) > length then
      return Invalid;
    if atype = uint(TypeA) and aclass = uint(ClassIn)
        and rdlength = uint(4) then
    begin
      value := read_u32(buf, rdata);
      ip := value;
      return Ok;
    end;
    if atype = uint(TypeCname) then
    begin
      links := links + 1;
      if links > MaxChain then
        return NotFound;
    end;
    pos := rdata + int(rdlength);
    index := index + 1;
  end;
  return NotFound;
end;

// One attempt: open a UDP socket to `server:53`, send the datagram, wait for
// a response with `id` and parse it. `WouldBlock` is the deadline or the retry
// cap; the socket is closed on every path.
function query_once(server: uint; query: const bytes[0]; qlen: int; id: uint;
                    ip: var uint): Status;
var
  handle: int;
  st: Status;
  deadline: uint;
  attempts: int;
  sent: int;
  read: int;
  response: bytes[ResponseCapacity];
  sending: bool;
  received: bool;
  timed_out: bool;
begin
  st := socket.udp(server, uint(DnsPort), handle);
  if st <> Ok then
    return st;
  deadline := core.clock() + uint(DeadlineNs);

  // Send the whole query; UDP `WouldBlock` means the next hop is not resolved
  // yet, so retry until the deadline.
  sending := true;
  attempts := 0;
  sent := 0;
  while sending and attempts < SendAttempts do
  begin
    st := socket.send(handle, query, sent);
    if st = Ok then
      sending := false
    else
    begin
      if st <> WouldBlock then
      begin
        socket.close(handle);
        return st;
      end;
      if core.clock() >= deadline then
      begin
        socket.close(handle);
        return WouldBlock;
      end;
      attempts := attempts + 1;
    end;
  end;
  if sending then
  begin
    socket.close(handle);
    return WouldBlock;
  end;
  if sent <> qlen then
  begin
    // One UDP send copies the whole datagram or reports a failure.
    socket.close(handle);
    return CapacityExceeded;
  end;

  // `Ok(0)` is not end of file for UDP, so only a datagram with a matching
  // transaction id counts; anything else is ignored until the deadline.
  // `socket.recv` is fixed at 1024 bytes, so this 512-byte DNS message calls
  // `core.socket_recv` directly (the module documents that path).
  received := false;
  timed_out := false;
  attempts := 0;
  while not received and not timed_out and attempts < RecvAttempts do
  begin
    st := core.socket_recv(handle, response, read);
    if st = Ok then
    begin
      if read >= 2 and read_u16(response, 0) = id then
        received := true
      else
        attempts := attempts + 1;
    end
    else if st = WouldBlock then
    begin
      if core.clock() >= deadline then
        timed_out := true
      else
        attempts := attempts + 1;
    end
    else
    begin
      socket.close(handle);
      return st;
    end;
  end;
  if not received then
  begin
    socket.close(handle);
    return WouldBlock;
  end;

  st := parse_response(response, read, id, ip);
  socket.close(handle);
  return st;
end;

// Resolves `name` to an IPv4 address through `server`. `Ok` writes `ip`; a
// `NotFound` (NXDOMAIN or no A record), `Invalid` (malformed name or response)
// or timeout (`WouldBlock`) leaves `ip` untouched.
function resolve(name: const string[0]; server: uint; ip: var uint): Status;
var
  query: bytes[QueryCapacity];
  qlen: int;
  id: uint;
  attempt: int;
  st: Status;
begin
  if int(core.len(name)) > MaxNameBytes then
    return CapacityExceeded;
  if int(core.len(name)) = 0 then
    return Invalid;
  // A random-ish transaction id from the low clock bits. One id serves both
  // attempts, so a late response is still accepted for this name.
  id := uint(core.clock() mod uint(65536));
  qlen := build_query(name, id, query);
  if qlen <= 0 then
    return Invalid;

  attempt := 0;
  while attempt < MaxAttempts do
  begin
    st := query_once(server, query, qlen, id, ip);
    if st = Ok then
      return Ok;
    if st <> WouldBlock then
      return st;
    attempt := attempt + 1;
  end;
  return WouldBlock;
end;

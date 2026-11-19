unit dns_check;

// Deterministic, socket-free checks for lib/dns.ig (M10.5). Host-only: this
// unit is not staged into the application image. Run with
//   cargo run -q -p langhost -- --args "" samples/lang/dns_check.ig
// and the e2e test `dns_canned_packets_parse_on_the_host`.
//
// Every helper in a unit is public, so the query builder and the response
// parser are exercised directly with canned packets. No socket is opened: the
// one `dns.resolve` call at the end stops at the host's disabled-network
// `core.socket_open`, which returns `Invalid` before anything reaches a device.
//
// The checks cover the exact query bytes, a valid A response, NXDOMAIN, a bad
// transaction id, a short header, TC set, a foreign opcode, a bad QDCOUNT, an
// answer whose RDATA overruns the datagram, a CNAME -> A chain, the CNAME
// chain bound, a CNAME-only response and an empty answer section. The
// destination is checked after every failure: a non-`Ok` parse leaves `ip`
// untouched.

uses core, stdlib, lib.dns;

var
  q: bytes[512];
  fails: int;

procedure expect(label: const string[0]; got: int; want: int);
var
  d: bytes[32];
begin
  if got <> want then
  begin
    fails := fails + 1;
    core.write("FAIL ");
    core.write(label);
    core.write(" got ");
    core.format_int_into(d, got, uint(10), uint(0));
    core.write(d);
    core.write(" want ");
    core.format_int_into(d, want, uint(10), uint(0));
    core.write(d);
    core.write("\n");
  end;
end;

procedure check(label: const string[0]; cond: bool);
begin
  if not cond then
  begin
    fails := fails + 1;
    core.write("FAIL ");
    core.write(label);
    core.write("\n");
  end;
end;

procedure main(args: const string[1024]);
var
  qlen: int;
  id: uint;
  at: int;
  i: int;
  st: Status;
  ip: uint;
  sentinel: uint;
  long_label: string[64];
begin
  id := uint(4660); // 0x1234

  // Builder: header, QNAME labels, root, QTYPE A, QCLASS IN.
  qlen := dns.build_query("example.com", id, q);
  expect("qlen", qlen, 29);
  expect("id_hi", int(q[0]), 0x12);
  expect("id_lo", int(q[1]), 0x34);
  expect("flags_hi", int(q[2]), 0x01);
  expect("flags_lo", int(q[3]), 0x00);
  expect("qd_hi", int(q[4]), 0);
  expect("qd_lo", int(q[5]), 1);
  expect("an_hi", int(q[6]), 0);
  expect("an_lo", int(q[7]), 0);
  expect("ns_hi", int(q[8]), 0);
  expect("ns_lo", int(q[9]), 0);
  expect("ar_hi", int(q[10]), 0);
  expect("ar_lo", int(q[11]), 0);
  expect("label1_len", int(q[12]), 7);
  expect("label1_e", int(q[13]), 101);
  expect("label2_len", int(q[20]), 3);
  expect("label2_c", int(q[21]), 99);
  expect("root", int(q[24]), 0);
  expect("qtype_hi", int(q[25]), 0);
  expect("qtype_lo", int(q[26]), 1);
  expect("qclass_hi", int(q[27]), 0);
  expect("qclass_lo", int(q[28]), 1);

  // Builder rejects an empty label, a trailing dot and a 64-byte label.
  expect("empty_name", dns.build_query("", id, q), -1);
  expect("double_dot", dns.build_query("a..b", id, q), -1);
  expect("trailing_dot", dns.build_query("example.com.", id, q), -1);
  long_label := "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
  expect("long_label", dns.build_query(long_label, id, q), -1);
  expect("one_label", dns.build_query("localhost", id, q), 12 + 1 + 9 + 1 + 4);

  // A one-answer A response built from that same question.
  qlen := dns.build_query("example.com", id, q);
  q[2] := byte(0x81);
  q[3] := byte(0x80);
  q[6] := byte(0);
  q[7] := byte(1);
  at := qlen;
  q[at] := byte(192); q[at + 1] := byte(12);
  q[at + 2] := byte(0); q[at + 3] := byte(1);
  q[at + 4] := byte(0); q[at + 5] := byte(1);
  q[at + 6] := byte(0); q[at + 7] := byte(0);
  q[at + 8] := byte(0); q[at + 9] := byte(60);
  q[at + 10] := byte(0); q[at + 11] := byte(4);
  q[at + 12] := byte(93); q[at + 13] := byte(184);
  q[at + 14] := byte(216); q[at + 15] := byte(34);
  ip := uint(0);
  st := dns.parse_response(q, qlen + 16, id, ip);
  check("a_ok", st = Ok);
  check("a_ip", ip = uint(0x5DB8D822));
  check("a_exact", ip = uint(93) * uint(16777216) + uint(184) * uint(65536)
    + uint(216) * uint(256) + uint(34));

  // The destination is untouched on every failure path.
  sentinel := uint(0xDEADBEEF);
  ip := sentinel;
  q[3] := byte(0x83); // NXDOMAIN (rcode 3)
  q[7] := byte(0);
  st := dns.parse_response(q, qlen, id, ip);
  check("nxdomain", st = NotFound);
  check("nxdomain_ip", ip = sentinel);

  q[3] := byte(0x80);
  q[7] := byte(1);
  st := dns.parse_response(q, qlen + 16, uint(4661), ip);
  check("bad_id", st = Invalid);
  check("bad_id_ip", ip = sentinel);

  st := dns.parse_response(q, 11, id, ip);
  check("truncated", st = Invalid);

  q[2] := byte(0x83); // TC set: 0x8180 | 0x0200
  st := dns.parse_response(q, qlen + 16, id, ip);
  check("tc", st = Invalid);
  q[2] := byte(0x81);

  q[2] := byte(0x89); // opcode 1: 0x8180 | 0x0800
  st := dns.parse_response(q, qlen + 16, id, ip);
  check("opcode", st = Invalid);
  q[2] := byte(0x81);

  q[5] := byte(2); // qdcount 2
  st := dns.parse_response(q, qlen + 16, id, ip);
  check("qdcount", st = Invalid);
  q[5] := byte(1);

  q[at + 11] := byte(5); // rdlength runs past the datagram
  st := dns.parse_response(q, qlen + 16, id, ip);
  check("rdlength_overrun", st = Invalid);
  q[at + 11] := byte(4);

  check("ok_ip_intact", ip = sentinel);

  // CNAME then A in one response: the chain is followed.
  qlen := dns.build_query("example.com", id, q);
  q[2] := byte(0x81);
  q[3] := byte(0x80);
  q[6] := byte(0);
  q[7] := byte(2);
  at := qlen;
  q[at] := byte(192); q[at + 1] := byte(12);
  q[at + 2] := byte(0); q[at + 3] := byte(5);
  q[at + 4] := byte(0); q[at + 5] := byte(1);
  q[at + 6] := byte(0); q[at + 7] := byte(0);
  q[at + 8] := byte(0); q[at + 9] := byte(60);
  q[at + 10] := byte(0); q[at + 11] := byte(2);
  q[at + 12] := byte(192); q[at + 13] := byte(16);
  at := at + 14;
  q[at] := byte(192); q[at + 1] := byte(12);
  q[at + 2] := byte(0); q[at + 3] := byte(1);
  q[at + 4] := byte(0); q[at + 5] := byte(1);
  q[at + 6] := byte(0); q[at + 7] := byte(0);
  q[at + 8] := byte(0); q[at + 9] := byte(60);
  q[at + 10] := byte(0); q[at + 11] := byte(4);
  q[at + 12] := byte(1); q[at + 13] := byte(2);
  q[at + 14] := byte(3); q[at + 15] := byte(4);
  ip := uint(0);
  st := dns.parse_response(q, qlen + 30, id, ip);
  check("cname_ok", st = Ok);
  check("cname_ip", ip = uint(0x01020304));

  // Five CNAMEs exceed the four-link bound: NotFound, no answer scanned.
  qlen := dns.build_query("example.com", id, q);
  q[2] := byte(0x81);
  q[3] := byte(0x80);
  q[6] := byte(0);
  q[7] := byte(5);
  at := qlen;
  i := 0;
  while i < 5 do
  begin
    q[at] := byte(192); q[at + 1] := byte(12);
    q[at + 2] := byte(0); q[at + 3] := byte(5);
    q[at + 4] := byte(0); q[at + 5] := byte(1);
    q[at + 6] := byte(0); q[at + 7] := byte(0);
    q[at + 8] := byte(0); q[at + 9] := byte(60);
    q[at + 10] := byte(0); q[at + 11] := byte(2);
    q[at + 12] := byte(192); q[at + 13] := byte(16);
    at := at + 14;
    i := i + 1;
  end;
  ip := sentinel;
  st := dns.parse_response(q, qlen + 70, id, ip);
  check("chain_bound", st = NotFound);
  check("chain_ip", ip = sentinel);

  // Only A records count; a response of CNAMEs alone is NotFound.
  q[7] := byte(1);
  st := dns.parse_response(q, qlen + 14, id, ip);
  check("cname_only", st = NotFound);

  // No answer records at all is NotFound, not Invalid.
  q[7] := byte(0);
  st := dns.parse_response(q, qlen, id, ip);
  check("no_answers", st = NotFound);

  // The disabled host refuses the socket open, so `resolve` is Invalid and
  // leaves the destination untouched. No socket is created.
  ip := sentinel;
  st := dns.resolve("example.com", uint(0x0A000203), ip);
  check("resolve_no_net", st = Invalid);
  check("resolve_ip", ip = sentinel);

  if fails = 0 then
    core.write("dns_check: ok\n")
  else
  begin
    core.write("dns_check: failures\n");
  end;
end;

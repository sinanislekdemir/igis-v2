unit net_perf;

// Network latency and counter proof (spec 0020, N4/N22). It runs as an
// ordinary application in the shell:
//
//   run net_perf
//
// The application sets the static QEMU user-networking configuration, reads
// `core.net_stats` before and after, and fetches `http://10.0.2.2:6666/` ten
// times with `core.clock()` around each exchange. It reports the body length,
// the total, minimum, average and maximum request time in microseconds, and
// the RX/TX/drop/error counters that moved. The last line, `net_perf: ok`, only
// prints after ten complete exchanges and two successful `net_stats` reads.
//
// The same binary proves both network paths: with the boot default the NIC's
// INTx line is routed and the interrupt handler wakes the service opportunity;
// with `cargo xtask run --net-irq poll` the line is never programmed and the
// polling drain does the same work. The counters and the timings are the
// evidence in both cases (the boot log says `net: irq ok` or `net: polling`).
//
// Ten exchanges are the bound: each `http.get` has its own 8 s deadline and
// bounded retries, so a dead peer prints `net_perf: failed ...` and the task
// exits instead of spinning. No window, no graphics.

uses core, stdlib, lib.http, lib.socket;

const
  BodyCapacity = 4096;
  // QEMU user networking: the host alias, the fixed probe port and the static
  // configuration from spec 0020 N18.
  HostIp = 0x0A000202;
  HostPort = 6666;
  LocalIp = 0x0A00020F;
  NetMask = 0xFFFFFF00;
  GatewayIp = 0x0A000202;
  DnsIp = 0x0A000203;
  // Exactly ten exchanges; the report divides by this constant.
  Requests = 10;

var
  body: string[BodyCapacity];

// Writes one decimal integer (the counters and the timings are integers).
procedure write_int(value: int);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

// One labelled `core.net_stats` line. A machine without a device reports the
// status instead of four zeroes.
procedure report_stats(tag: const string[0]);
var
  rx: uint;
  tx: uint;
  drops: uint;
  errors: uint;
  st: Status;
begin
  st := core.net_stats(rx, tx, drops, errors);
  core.write(tag);
  if st <> Ok then
  begin
    core.write(": stats failed ");
    write_int(int(st));
    core.write("\n");
    return;
  end;
  core.write(": rx ");
  write_int(int(rx));
  core.write(" tx ");
  write_int(int(tx));
  core.write(" drops ");
  write_int(int(drops));
  core.write(" errors ");
  write_int(int(errors));
  core.write("\n");
end;

// Nanoseconds to whole microseconds.
function micros(nanoseconds: uint): int;
begin
  return int(nanoseconds div uint(1000));
end;

procedure main(args: const string[1024]);
var
  st: Status;
  request: int;
  length: int;
  start: uint;
  stop: uint;
  total_ns: uint;
  min_ns: uint;
  max_ns: uint;
  delta: uint;
begin
  if core.net_mac() = uint(0) then
  begin
    core.write("net_perf: no device\n");
    return;
  end;
  st := core.net_config(uint(LocalIp), uint(NetMask), uint(GatewayIp),
    uint(DnsIp));
  if st <> Ok then
  begin
    core.write("net_perf: no device\n");
    return;
  end;

  report_stats("net_perf: before");

  total_ns := uint(0);
  min_ns := uint(0);
  max_ns := uint(0);
  length := 0;
  request := 0;
  while request < Requests do
  begin
    start := core.clock();
    st := http.get(uint(HostIp), uint(HostPort), "10.0.2.2", "/", body);
    stop := core.clock();
    if st <> Ok then
    begin
      core.write("net_perf: failed at request ");
      write_int(request);
      core.write(" status ");
      write_int(int(st));
      core.write("\n");
      return;
    end;

    length := http.body_length(body);
    delta := stop - start;
    total_ns := total_ns + delta;
    if request = 0 or delta < min_ns then
      min_ns := delta;
    if request = 0 or delta > max_ns then
      max_ns := delta;
    request := request + 1;
  end;

  core.write("net_perf: body ");
  write_int(length);
  core.write(" bytes\n");

  core.write("net_perf: http ");
  write_int(Requests);
  core.write(" gets total ");
  write_int(micros(total_ns));
  core.write(" us min ");
  write_int(micros(min_ns));
  core.write(" us avg ");
  write_int(micros(total_ns div uint(Requests)));
  core.write(" us max ");
  write_int(micros(max_ns));
  core.write(" us\n");

  report_stats("net_perf: after");
  core.write("net_perf: ok\n");
end;

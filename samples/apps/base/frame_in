unit frame_in;

// Receiver half of the framing sample (spec 0006, "Interfaces"). It decodes the
// typed views `frame_out` sends: four little-endian `u64` values and one
// length-prefixed text frame, then the two error paths — a length header that
// cannot be satisfied and the close that follows the producer's exit.
//
// The two `recv_frame_into` calls after the good frame show the promise that
// matters: a rejected frame leaves the destination's length exactly as it was,
// so the caller never observes a partial value. The last receive blocks until
// the producer terminates, which closes the stream.

uses core, stdlib, lib.out;

var
  frames: in_stream[64];
  frames_ack: out_stream[16];
  frame: bytes[256];
  digits: string[32];
  value: int;
  n: int;
  st: Status;

procedure show(number: int);
begin
  n := core.format_int_into(digits, number, uint(10), uint(0));
  core.write(digits);
end;

procedure report_value(name: const string[0]; status: Status; number: int);
begin
  out.say(name);
  if status = Ok then
  begin
    show(number);
    out.say("\n");
  end
  else
    out.say("failed\n");
end;

procedure main(args: const string[1024]);
begin
  st := stdlib.recv_u64(frames, value);
  report_value("frame_in: u64 ", st, value);
  st := stdlib.recv_u64(frames, value);
  report_value("frame_in: u64 ", st, value);
  st := stdlib.recv_u64(frames, value);
  report_value("frame_in: u64 ", st, value);
  st := stdlib.recv_u64(frames, value);
  report_value("frame_in: u64 ", st, value);

  st := stdlib.recv_frame_into(frames, frame);
  if st = Ok then
  begin
    out.say("frame_in: frame=[");
    core.write(frame);
    out.say("] len=");
    show(int(core.len(frame)));
    out.say("\n");
  end
  else
    out.say("frame_in: frame failed\n");

  st := stdlib.recv_frame_into(frames, frame);
  if st = CapacityExceeded then
    out.say("frame_in: 300-byte length rejected (CapacityExceeded)\n")
  else
    out.say("frame_in: unexpected status for a 300-byte length\n");
  out.say("frame_in: frame length still ");
  show(int(core.len(frame)));
  out.say("\n");

  // Only now may the producer exit: everything it sent has been consumed.
  st := core.send(frames_ack, "ack");
  out.say("frame_in: acked, waiting for close\n");

  st := stdlib.recv_exact(frames, frame, 1);
  if st = Closed then
    out.say("frame_in: closed\n")
  else
    out.say("frame_in: expected Closed\n");
  out.say("frame_in: frame length still ");
  show(int(core.len(frame)));
  out.say("\n");
end;

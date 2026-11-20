unit frame_out;

// Binary half of the framing sample (spec 0006, "Interfaces"). `frames` carries
// typed messages to `frame_in`; `frames_ack` carries the receiver's one-byte
// acknowledgement back. The acknowledgement is what makes the shutdown
// deterministic without a delay: this task exits only after the receiver has
// consumed every message, so termination cannot discard queued bytes (S7).
//
// The messages are four `stdlib.send_u64` values (fixed-width, little-endian)
// and one length-prefixed text frame. A payload that cannot fit a frame is
// refused locally, and a hand-built header that claims more bytes than a frame
// may hold proves the receiver's length check instead of a partial read.

uses core, stdlib;

var
  frames: out_stream[64];
  frames_ack: in_stream[16];
  chunk64: string[64];
  long: string[320];
  header: bytes[4];
  ack: bytes[8];
  st: Status;
  refused: Status;
  got: int;

procedure main(args: const string[1024]);
begin
  chunk64 := "0123456789012345678901234567890123456789012345678901234567890123";
  long := chunk64 + chunk64 + chunk64 + chunk64 + chunk64;

  st := stdlib.send_u64(frames, 0);
  st := stdlib.send_u64(frames, 42);
  st := stdlib.send_u64(frames, -1);
  st := stdlib.send_u64(frames, -9223372036854775807 - 1);

  st := stdlib.send_frame(frames, "framed payload");

  // 320 payload bytes fit neither the 256-byte frame buffer nor the 64-byte
  // stream: `send_frame` returns CapacityExceeded and sends nothing at all.
  refused := stdlib.send_frame(frames, long);

  // A header that claims 300 payload bytes which never arrive. `frame_in`'s
  // `recv_frame_into` must reject the length rather than read a partial
  // payload; the stream is desynchronised afterwards, so this goes last.
  header[0] := byte(44);
  header[1] := byte(1);
  header[2] := byte(0);
  header[3] := byte(0);
  st := stdlib.send_bytes(frames, header);

  // Blocks until the receiver has consumed everything above.
  got := core.recv(frames_ack, ack);

  core.write("frame_out: sent 4 u64 values and 1 frame\n");
  if refused = CapacityExceeded then
    core.write("frame_out: 320-byte frame refused (CapacityExceeded)\n")
  else
    core.write("frame_out: 320-byte frame was NOT refused\n");
  core.write("frame_out: sent a 300-byte length header\n");
end;

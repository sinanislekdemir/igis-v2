unit buffer_ops;

// Machine-level primitive proof (spec 0003): the five `uint` bit operations and
// the two all-or-nothing bulk buffer primitives. Test-only (`--shell`); it has
// no window and prints one line on success.
//
//   run buffer_ops
//
// The buffer cases pin the length rule: every successful write publishes
// `max(old_len, off + len)` and zero-fills the gap, exactly like an indexed
// byte write. The failure cases prove the destination is untouched.

uses core;

const
  N = 8;

var
  failures: int;

procedure check(condition: bool; label: const string[0]);
begin
  if not condition then
  begin
    core.write("buffer_ops: failed ");
    core.write(label);
    core.write("\n");
    failures := failures + 1;
  end;
end;

procedure main(args: const string[1024]);
var
  st: Status;
  buf: bytes[N];
  other: bytes[N];
  value: uint;
  i: int;
begin
  failures := 0;

  // Bit operations are `uint`-only named functions.
  check(core.bit_and(uint(0xF0F0), uint(0x0FF0)) = uint(0x00F0), "bit_and");
  check(core.bit_or(uint(0xF0F0), uint(0x0FF0)) = uint(0xFFF0), "bit_or");
  check(core.bit_xor(uint(0xF0F0), uint(0x0FF0)) = uint(0xFF00), "bit_xor");
  value := core.shl(uint(1), uint(63));
  check(core.shr(value, uint(63)) = uint(1), "shl_shr");
  check(core.shr(core.shl(uint(3), uint(62)), uint(1))
    = uint(0x6000000000000000), "shr_logical");

  // Two buffers of zeroes; fill sets the bytes and the length.
  st := core.fill_bytes(buf, uint(0), byte(0), uint(N));
  check(st = Ok and core.len(buf) = uint(N), "fill_init");
  st := core.fill_bytes(buf, uint(0), byte(1), uint(4));
  check(st = Ok and core.len(buf) = uint(N) and buf[4] = byte(0), "fill_prefix");

  // Distinct bytes 1..8, so an overlap bug is observable.
  i := 0;
  while i < N do
  begin
    st := core.fill_bytes(buf, uint(i), byte(i + 1), uint(1));
    i := i + 1;
  end;
  check(st = Ok and core.len(buf) = uint(N), "fill_each");

  // Forward overlap: copy [0..4) onto [1..5). A byte-at-a-time forward copy
  // would repeat the first byte; `memmove` gives 1,1,2,3,4,...
  st := core.copy_bytes(buf, uint(1), buf, uint(0), uint(4));
  check(st = Ok and buf[1] = byte(1) and buf[2] = byte(2) and buf[4] = byte(4),
    "copy_forward");

  // Backward overlap: copy [1..5) onto [0..4) of the 1..8 image. A naive
  // backward loop would smear the last byte; `memmove` gives 2,3,4,5,5,...
  i := 0;
  while i < N do
  begin
    st := core.fill_bytes(buf, uint(i), byte(i + 1), uint(1));
    i := i + 1;
  end;
  st := core.copy_bytes(buf, uint(0), buf, uint(1), uint(4));
  check(st = Ok and buf[0] = byte(2) and buf[3] = byte(5) and buf[4] = byte(5),
    "copy_backward");

  // Cross-buffer copy publishes the destination length.
  st := core.copy_bytes(other, uint(0), buf, uint(0), uint(4));
  check(st = Ok and core.len(other) = uint(4) and other[3] = byte(5), "copy_cross");

  // Zero length: inside the old length nothing changes; past it the gap is
  // zero-filled and the length becomes the offset.
  st := core.copy_bytes(other, uint(2), buf, uint(0), uint(0));
  check(st = Ok and core.len(other) = uint(4), "copy_zero_inside");
  st := core.copy_bytes(other, uint(6), buf, uint(0), uint(0));
  check(st = Ok and core.len(other) = uint(6) and other[4] = byte(0), "copy_zero_gap");

  // Failures are all-or-nothing.
  st := core.copy_bytes(other, uint(7), buf, uint(0), uint(2));
  check(st = CapacityExceeded and core.len(other) = uint(6), "copy_capacity");
  st := core.copy_bytes(other, uint(0), buf, uint(7), uint(2));
  check(st = Invalid and core.len(other) = uint(6), "copy_source");

  // Fill at the exact capacity boundary succeeds; one past it does not.
  st := core.fill_bytes(other, uint(4), byte(9), uint(4));
  check(st = Ok and core.len(other) = uint(8) and other[7] = byte(9), "fill_boundary");
  st := core.fill_bytes(other, uint(7), byte(9), uint(2));
  check(st = CapacityExceeded and core.len(other) = uint(8), "fill_capacity");

  if failures = 0 then
    core.write("buffer_ops: ok\n");
end;

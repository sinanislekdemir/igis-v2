unit bitio_probe;

// Test-only proof for `lib.bitio` (spec 0023, G1). It is staged only by
// `xtask image --shell` and by `langhost`'s e2e test; it opens no window and
// prints one line when every case passes.
//
//   run bitio_probe
//
// The cases pin the contract: LSB-first order across a byte boundary
// (`0xB5, 0x03` read as 3/5/4 bits gives 5, 22, 3), `ensure`/`peek`/`drop`
// without consumption, `read_bits`, `align` to the byte boundary, `read_byte`
// for stored blocks, `fill` with a nonzero source offset, `fill` at capacity
// (`Full`), and `WouldBlock` when the window is empty.

uses core, lib.out, lib.bitio;

const
  Capacity = 512;

var
  failures: int;

procedure check(condition: bool; label: const string[0]);
begin
  if not condition then
  begin
    out.say("bitio: failed ");
    out.say(label);
    out.say("\n");
    failures := failures + 1;
  end;
end;

procedure main(args: const string[1024]);
var
  r: bitio.Reader;
  q: bitio.Reader;
  p: bitio.Reader;
  two: bytes[8];
  chunk: bytes[8];
  big: bytes[Capacity];
  value: uint;
  used: int;
  i: int;
  st: Status;
begin
  failures := 0;

  // ---- init ---------------------------------------------------------------
  st := bitio.init(r);
  check(st = Ok and bitio.available(r) = uint(0), "init");

  // ---- LSB-first order across a byte boundary -----------------------------
  // 0xB5 is 1011_0101 MSB-first, so the read order is 1,0,1,0,1,1,0,1 and the
  // first 3 bits are 0b101 = 5, the next 5 are 0b10110 = 22. 0x03 then gives
  // 1,1,0,0,... so the next 4 bits are 0b0011 = 3.
  two[0] := byte(0xB5);
  two[1] := byte(0x03);
  st := bitio.fill(r, two, uint(0), used);
  check(st = Ok and used = 2 and bitio.available(r) = uint(16), "fill_two");
  st := bitio.peek(r, uint(3), value);
  check(st = Ok and value = uint(5), "peek3");
  // A second peek sees the same bits; nothing was consumed.
  st := bitio.peek(r, uint(3), value);
  check(st = Ok and value = uint(5) and bitio.available(r) = uint(16), "peek_again");
  st := bitio.drop(r, uint(3));
  check(st = Ok and bitio.available(r) = uint(13), "drop3");
  st := bitio.read_bits(r, uint(5), value);
  check(st = Ok and value = uint(22), "read5");
  check(bitio.available(r) = uint(8), "available8");
  st := bitio.read_bits(r, uint(4), value);
  check(st = Ok and value = uint(3), "read4");
  check(bitio.available(r) = uint(4), "available4");
  st := bitio.drop(r, uint(4));
  check(st = Ok and bitio.available(r) = uint(0), "drop_rest");

  // ---- empty window: WouldBlock changes nothing ---------------------------
  st := bitio.init(r);
  check(st = Ok, "reinit");
  st := bitio.ensure(r, uint(8));
  check(st = WouldBlock and bitio.available(r) = uint(0), "ensure_empty");
  st := bitio.peek(r, uint(1), value);
  check(st = WouldBlock, "peek_empty");
  st := bitio.read_bits(r, uint(1), value);
  check(st = WouldBlock and bitio.available(r) = uint(0), "read_empty");

  // ---- align then read_byte (stored blocks) -------------------------------
  st := bitio.fill(r, two, uint(0), used);
  check(st = Ok and used = 2, "fill_two_again");
  st := bitio.read_bits(r, uint(3), value);
  check(st = Ok and value = uint(5), "align_read3");
  bitio.align(r);
  // The partial 5 bits are gone; the next byte is untouched.
  check(bitio.available(r) = uint(8), "align_available");
  value := uint(99);
  st := bitio.read_byte(r, value);
  check(st = Ok and value = uint(3), "read_byte_after_align");
  value := uint(99);
  st := bitio.read_byte(r, value);
  check(st = WouldBlock and value = uint(99), "read_byte_empty");

  // Already aligned: align is a no-op.
  st := bitio.init(r);
  st := bitio.fill(r, two, uint(0), used);
  bitio.align(r);
  check(bitio.available(r) = uint(16), "align_noop");
  // Bits to the byte boundary, then a whole byte from the window.
  st := bitio.read_bits(r, uint(8), value);
  check(st = Ok and value = uint(0xB5), "read_byte_low");
  st := bitio.read_byte(r, value);
  check(st = Ok and value = uint(3), "read_byte_next");

  // ---- a whole byte already in acc is drained by read_byte ----------------
  // `ensure(8)` loads 0xB5 into the accumulator without consuming it; align
  // leaves it there, so read_byte must return it and leave 0x03 readable.
  st := bitio.init(r);
  st := bitio.fill(r, two, uint(0), used);
  st := bitio.ensure(r, uint(8));
  check(st = Ok and bitio.available(r) = uint(16), "ensure8");
  bitio.align(r);
  value := uint(99);
  st := bitio.read_byte(r, value);
  check(st = Ok and value = uint(0xB5), "read_byte_from_acc");
  st := bitio.read_byte(r, value);
  check(st = Ok and value = uint(3), "read_byte_after_acc");
  value := uint(99);
  st := bitio.read_byte(r, value);
  check(st = WouldBlock and value = uint(99), "read_byte_acc_empty");

  // Two whole bytes in the accumulator drain low byte first.
  st := bitio.init(r);
  st := bitio.fill(r, two, uint(0), used);
  st := bitio.ensure(r, uint(16));
  check(st = Ok and bitio.available(r) = uint(16), "ensure16");
  st := bitio.read_byte(r, value);
  check(st = Ok and value = uint(0xB5), "acc_first_byte");
  st := bitio.read_byte(r, value);
  check(st = Ok and value = uint(3) and bitio.available(r) = uint(0),
    "acc_second_byte");

  // ---- bounds: drop, ensure and peek reject oversized requests ------------
  st := bitio.init(r);
  st := bitio.fill(r, two, uint(0), used);
  st := bitio.ensure(r, uint(4));
  check(st = Ok, "ensure4");
  st := bitio.drop(r, uint(9));
  check(st = Invalid and bitio.available(r) = uint(16), "drop_too_many");
  st := bitio.ensure(r, uint(33));
  check(st = Invalid and bitio.available(r) = uint(16), "ensure_too_many");
  value := uint(99);
  st := bitio.peek(r, uint(33), value);
  check(st = Invalid and value = uint(99), "peek_too_many");

  // ---- fill with a nonzero source offset ---------------------------------
  i := 0;
  while i < 8 do
  begin
    chunk[i] := byte(0x11 + i);
    i := i + 1;
  end;
  st := bitio.init(q);
  st := bitio.fill(q, chunk, uint(3), used);
  check(st = Ok and used = 5 and bitio.available(q) = uint(40), "fill_offset");
  st := bitio.read_byte(q, value);
  check(st = Ok and value = uint(0x14), "fill_offset_first");
  i := 0;
  while i < 4 do
  begin
    st := bitio.read_byte(q, value);
    i := i + 1;
  end;
  check(st = Ok and value = uint(0x18) and bitio.available(q) = uint(0),
    "fill_offset_last");
  st := bitio.read_byte(q, value);
  check(st = WouldBlock, "fill_offset_empty");

  // An offset at or past the runtime length copies nothing.
  st := bitio.init(q);
  st := bitio.fill(q, chunk, uint(8), used);
  check(st = Ok and used = 0 and bitio.available(q) = uint(0), "fill_past_end");

  // ---- fill at capacity is Full, then rewinds after the bytes are read ---
  st := core.fill_bytes(big, uint(0), byte(0x5A), uint(Capacity));
  check(st = Ok and core.len(big) = uint(Capacity), "big_fill");
  st := bitio.init(p);
  st := bitio.fill(p, big, uint(0), used);
  check(st = Ok and used = Capacity and bitio.available(p) = uint(4096),
    "fill_full_window");
  used := 7;
  st := bitio.fill(p, big, uint(0), used);
  check(st = Full and used = 0 and bitio.available(p) = uint(4096), "fill_full");
  st := bitio.read_byte(p, value);
  check(st = Ok and value = uint(0x5A), "full_first_byte");
  i := 0;
  while i < Capacity - 1 do
  begin
    st := bitio.read_byte(p, value);
    i := i + 1;
  end;
  check(st = Ok and value = uint(0x5A) and bitio.available(p) = uint(0),
    "full_last_byte");
  // The window is exhausted, so a fill rewinds before it copies.
  st := bitio.fill(p, chunk, uint(0), used);
  check(st = Ok and used = 8 and bitio.available(p) = uint(64), "fill_rewind");

  if failures = 0 then
    out.say("bitio: ok\n");
end;

//! ABI v1 opcode table.
//!
//! An instruction is one or more 4-byte words. The first word is
//! `op: u8, a: u8, b: u8, c: u8`; immediate words follow it, least significant
//! byte first. Unless noted, `a`, `b` and `c` are reserved and must be zero.
//!
//! Jump immediates are signed 32-bit counts of **instruction words** (4-byte
//! units) relative to the instruction that follows the jump. The interpreter
//! checks the target again at run time; the verifier rejects targets outside
//! the function or in the middle of an instruction.
//!
//! `c` bit 0 on [`JMP`], [`JZ`], [`JNZ`] and [`CALL`] is the explicit poll
//! hint required by spec 0008 (B3). The first-milestone interpreter checks the
//! step counter at *every* instruction boundary, which already subsumes it, so
//! the bit is accepted but has no additional effect.
//!
//! Operand stack convention: every value is one 8-byte slot. `bool` and `byte`
//! are zero-extended; `f64` is stored as its IEEE-754 bit pattern.

/// Pop one 8-byte operand.
pub const DROP: u8 = 0x00;
/// Push a 64-bit immediate. Immediate words: `lo: u32`, `hi: u32`.
pub const CONST: u8 = 0x01;
/// Unconditional relative jump. Immediate: `rel: i32` in instruction words.
pub const JMP: u8 = 0x02;
/// Pop; relative jump when the value is zero.
pub const JZ: u8 = 0x03;
/// Pop; relative jump when the value is non-zero.
pub const JNZ: u8 = 0x04;
/// Call a function by index. Immediate: `function: u32`.
pub const CALL: u8 = 0x05;
/// Return from the current function. Returning from `main` ends the task.
pub const RET: u8 = 0x06;

/// Push the scalar 8 bytes at `locals[off]`. Immediate: `off: u32`.
pub const LOAD_LOCAL: u8 = 0x10;
/// Pop a scalar into `locals[off]`. Immediate: `off: u32`.
pub const STORE_LOCAL: u8 = 0x11;
/// Push a reference to `locals[off]` (the first byte of the value). Immediate: `off: u32`.
pub const REF_LOCAL: u8 = 0x12;
/// Push the scalar 8 bytes at `state[off]`. Immediate: `off: u32`.
pub const LOAD_STATE: u8 = 0x13;
/// Pop a scalar into `state[off]`. Immediate: `off: u32`.
pub const STORE_STATE: u8 = 0x14;
/// Push a reference to `state[off]` (the first byte of the value). Immediate: `off: u32`.
pub const REF_STATE: u8 = 0x15;
/// Pop a reference, push the scalar 8 bytes it names.
pub const LOAD_REF: u8 = 0x16;
/// Pop a value, pop a reference, store the value through the reference.
pub const STORE_REF: u8 = 0x17;
/// Push a read-only reference into the constant blob. Immediate: `off: u32`.
pub const REF_CONST: u8 = 0x18;
/// Pop an index, pop a buffer reference, push one raw `byte` from the buffer.
///
/// The index is a byte offset into the buffer's data. A read is checked
/// against the buffer's runtime length, so an index at or past the length
/// faults `OutOfBounds`. The result is the raw byte at that offset, zero
/// extended to 64 bits; for `string` it is the byte, not a code point
/// (spec 0003, T4).
pub const LOAD_U8: u8 = 0x19;
/// Pop a value, pop an index, pop a buffer reference, store one raw `byte`.
///
/// Immediate: `capacity: u32`, the destination's declared capacity. An index
/// at or past the capacity faults `CapacityExceeded`. An index at or past the
/// runtime length is allowed: the gap between the old length and the index is
/// filled with zero bytes and the length becomes `index + 1` (write extends).
pub const STORE_U8: u8 = 0x1A;
/// Pop a reference, add an immediate byte offset, bounds-check and push the
/// resulting reference. Immediate: `offset: u32`, a multiple of 8.
///
/// This is how a record field reference is formed: one step per field, each
/// checked against the owning region. A reference into a buffer field points at
/// its data, so the step for that field carries the extra 8 bytes of the length
/// word.
pub const REF_OFFSET: u8 = 0x1B;
/// Pop a byte count, a source reference and a destination reference; copy
/// `count` bytes. Every reference is bounds-checked against its region, the
/// destination must be writable (`state` or `stack`), and an overlap inside one
/// region is handled by a move.
///
/// Used for a whole-record assignment and for a `value` record parameter, where
/// the compiler knows the record size and emits it as a constant.
pub const COPY_REFS: u8 = 0x1C;
/// Pop an index, pop a base reference, check `index < count` and push
/// `base + index * stride`. Immediates: `stride: u32`, `count: u32`.
///
/// This is how an `array[T, N]` element reference is formed (spec 0003): the
/// base reference names the array's first byte, `stride` is the element size
/// rounded to 8, and `count` is `N`. An index at or past `count` faults
/// `OutOfBounds`; a negative `int` index is a huge `uint` and fails the same
/// check. Elements are then read and written with `load_ref`/`store_ref`
/// (scalars and enums) or `copy_refs` (records, sums and arrays), exactly like
/// record fields.
pub const REF_ELEMENT: u8 = 0x1D;

/// Checked signed integer addition.
pub const ADD: u8 = 0x20;
/// Checked signed integer subtraction.
pub const SUB: u8 = 0x21;
/// Checked signed integer multiplication.
pub const MUL: u8 = 0x22;
/// Checked signed integer division.
pub const DIV: u8 = 0x23;
/// Checked signed integer remainder.
pub const MOD: u8 = 0x24;
/// Checked signed integer negation.
pub const NEG: u8 = 0x25;
/// Checked unsigned integer addition.
pub const ADD_U: u8 = 0x26;
/// Checked unsigned integer subtraction.
pub const SUB_U: u8 = 0x27;
/// Checked unsigned integer multiplication.
pub const MUL_U: u8 = 0x28;
/// Checked unsigned integer division.
pub const DIV_U: u8 = 0x29;
/// Checked unsigned integer remainder.
pub const MOD_U: u8 = 0x2A;

/// Signed equality; pushes `bool` (0 or 1).
pub const EQ: u8 = 0x2B;
/// Signed inequality.
pub const NE: u8 = 0x2C;
/// Signed less-than.
pub const LT: u8 = 0x2D;
/// Signed less-or-equal.
pub const LE: u8 = 0x2E;
/// Signed greater-than.
pub const GT: u8 = 0x2F;
/// Signed greater-or-equal.
pub const GE: u8 = 0x30;
/// Unsigned less-than.
pub const LT_U: u8 = 0x31;
/// Unsigned less-or-equal.
pub const LE_U: u8 = 0x32;
/// Unsigned greater-than.
pub const GT_U: u8 = 0x33;
/// Unsigned greater-or-equal.
pub const GE_U: u8 = 0x34;
/// Boolean negation; pushes `bool`.
pub const NOT: u8 = 0x35;
/// Pop two buffer references, push `bool`: true when their runtime lengths and
/// their bytes are equal. Works across capacities and regions; `string` and
/// `bytes` are distinct types, so the compiler only pairs like with like.
pub const CMP_BUFFERS: u8 = 0x36;

/// Pop a destination reference and set its buffer length to zero.
pub const CLEAR: u8 = 0x40;
/// Pop a source reference, pop a destination reference, append the source
/// bytes to the destination. Immediate: `capacity: u32`, the destination's
/// declared capacity. Overflow faults `CapacityExceeded`.
pub const APPEND: u8 = 0x41;
/// Pop a `bytes` buffer reference and copy the console input bytes that are
/// available (`core.read_input`). Immediate: `capacity: u32`, the
/// destination's declared capacity. Pushes the copied length (`0..=capacity`)
/// and always publishes that length in the destination's header. This is a
/// raw, non-blocking read: no bytes available returns `0`, and the caller
/// blocks with `core.next_any` on source `0` instead (spec 0005).
pub const READ_INPUT: u8 = 0x42;
/// Pop a value, pop a buffer reference, push the first index of that `byte`
/// (`core.find`). Scans `0..length` only; pushes `-1` when the byte is absent.
/// Works for both `string` and `bytes`; a `string` is searched byte-wise, not by
/// code point.
pub const FIND: u8 = 0x43;

/// Pop a length, pop a start, pop a source reference, pop a destination
/// reference, copy the slice and push the copied length (`core.substr_into`).
///
/// `a` is the **destination** buffer kind ([`buffer_kind`]) and `b` the
/// **source** kind; all four combinations are legal. The destination kind
/// decides the check:
///
/// - `STRING` destination from a `STRING` source: both slice boundaries must
///   fall on UTF-8 code-point boundaries in the source.
/// - `STRING` destination from a `BYTES` source: the copied range must be
///   strict UTF-8 (no overlong encodings, no surrogates, nothing above
///   `U+10FFFF`, no truncated sequence, no stray continuation byte). This is
///   the only primitive that turns raw bytes into a `string` (T3).
/// - `BYTES` destination: a raw copy from either source kind, no check (T4).
///
/// Immediate: `capacity: u32`, the destination's declared capacity. Negative
/// `start` or `len` pushes `-4` (`Status::Invalid`) and writes nothing; a
/// `start` past the source length copies nothing and clears the destination.
/// The copied length is `min(len, src_len - start, capacity)`.
pub const SUBSTR: u8 = 0x44;
/// Pop a width, a base, a value and a destination reference, format the signed
/// integer and push the byte count (`core.format_int_into`).
///
/// `a` is the destination's buffer kind ([`buffer_kind`]); both kinds are
/// accepted because the output is ASCII. Immediate: `capacity: u32`, the
/// destination's declared capacity. `base` is `u64` and must be 10 (decimal) or
/// 16 (lowercase hexadecimal); any other base pushes `-4` (`Status::Invalid`)
/// and writes nothing. A negative value gets a leading `-`, then the magnitude;
/// a non-zero `width` zero-pads the digits to at least that many characters
/// (the sign is additional). A result that does not fit the capacity pushes
/// `-6` (`Status::CapacityExceeded`) and leaves the destination length alone.
pub const FORMAT_INT: u8 = 0x45;
/// Checked cast from an `int` ordinal to an enum value (`EnumName(int)`).
/// Immediate: `count: u32`, the number of variants of the destination enum.
///
/// The operand is the ordinal; the result is the same 8-byte value, now typed
/// as the enum. An ordinal outside `0..count` faults `Overflow`, so an invalid
/// enum value cannot be constructed. `int(enum_value)` emits nothing: the
/// ordinal already is the representation.
pub const ENUM_CAST: u8 = 0x46;

// ---- maps ---------------------------------------------------------------
//
// A `map[K, V, N]` value is `count: u64` at offset 0, then `capacity` slots of
// `slot_stride = 16 + key_size + value_size` bytes at offset 8. A slot is
// `state: u64` (0 empty, 1 occupied), `hash: u64`, the key image and the value
// image. These five opcodes carry the layout in their immediates, because
// hashing, probing and backward-shift deletion are one algorithm that the
// compiler must not inline at every call site (spec 0003, "Maps").
//
// Operand stack order, references as usual:
//   map_put:    [map][key][value]      -> Status
//   map_get:    [map][key][destination]-> Status
//   map_has:    [map][key]             -> bool
//   map_remove: [map][key]             -> Status
//   map_count:  [map]                  -> int
//
// `a` is the key kind ([`map_key_kind`]) and `b` is the value kind
// ([`map_value_kind`]). Immediates: `capacity: u32`, `count: u32` (the entry
// limit N), `key_size: u32`, `value_size: u32`; `slot_stride` is
// `16 + key_size + value_size`. A buffer key arrives as a reference to its data
// with the runtime length at `ref - 8`; a buffer value arrives the same way. A
// scalar key and a scalar value are the 8-byte value itself; a fixed aggregate
// value (record, sum, array, ring, nested map) is a reference to its first byte.

/// Insert or replace: pop value, key, map and push `Status` (`Ok`, or `Full`
/// when the map already holds `N` distinct keys and the key is new).
///
/// Immediates: `capacity`, `count`, `key_size`, `value_size`. `a` is the key
/// kind, `b` the value kind.
pub const MAP_PUT: u8 = 0x47;
/// Look up a key and copy its value through the destination reference: pop
/// destination, key, map and push `Status` (`Ok`, or `NotFound` with the
/// destination untouched).
///
/// Immediates: `capacity`, `count`, `key_size`, `value_size`.
pub const MAP_GET: u8 = 0x48;
/// Pop key and map, push `bool`: true when the key is present.
///
/// Immediates: `capacity`, `count`, `key_size`, `value_size`.
pub const MAP_HAS: u8 = 0x49;
/// Remove a key with backward-shift deletion: pop key and map and push
/// `Status` (`Ok`, or `NotFound`).
///
/// Immediates: `capacity`, `count`, `key_size`, `value_size`.
pub const MAP_REMOVE: u8 = 0x4A;
/// Pop a map reference, push the live entry count as an `int`. The map's
/// `count` word is the first 8 bytes of the value, so there are no immediates.
pub const MAP_COUNT: u8 = 0x4B;

// ---- streams ------------------------------------------------------------
//
// A stream endpoint is an 8-byte opaque handle in application `state`
// ([`crate::Endpoint`], spec 0006). Data is copied through the kernel; no
// application-visible pointer into the queue exists. The handle carries a
// generation and a table index, so a stale handle cannot reach a recycled
// stream (spec 0006, "Dynamic loading").
//
// Operand stack order:
//   stream_send / stream_try_send: [handle][data reference] -> Status
//   stream_recv / stream_try_recv: [handle][destination reference] -> int
//
// A blocking send/receive that cannot proceed leaves the instruction
// unexecuted and blocks the task (`WaitKind::Stream`); the try variant pushes
// `Status::WouldBlock` (send) or `-1` (receive). A closed stream pushes
// `Status::Closed` (send) or `-2` (receive).

/// `core.send(ep, data) -> Status`: copy the whole buffer atomically into the
/// stream, blocking while the queue has less free space than the buffer needs.
/// A buffer larger than the stream capacity is `Status::CapacityExceeded`
/// immediately and never blocks (spec 0006, S4/S5).
pub const STREAM_SEND: u8 = 0x4C;
/// `core.try_send(ep, data) -> Status`: as [`STREAM_SEND`], but a full queue is
/// `Status::WouldBlock` instead of a block.
pub const STREAM_TRY_SEND: u8 = 0x4D;
/// `core.recv(ep, var dst) -> int`: copy up to `dst`'s capacity bytes from the
/// queue, blocking while it is empty. Pushes the copied count (`>= 1`) or a
/// negative status: `-2` (`Status::Closed`) when the peer has closed and the
/// queue is empty. Immediate: `capacity: u32`, the destination's declared
/// `bytes[N]` capacity.
pub const STREAM_RECV: u8 = 0x4E;
/// `core.try_recv(ep, var dst) -> int`: as [`STREAM_RECV`], but an empty queue
/// pushes `-1` (`Status::WouldBlock`) instead of blocking.
pub const STREAM_TRY_RECV: u8 = 0x4F;

/// Invoke a host builtin. Immediate: `builtin: u32` (see [`builtin`]).
pub const BUILTIN: u8 = 0x50;

/// Pop a fraction width, an `f64` value and a destination reference, format the
/// value as fixed-point decimal text and push the byte count
/// (`core.format_f64_into`).
///
/// `a` is the destination's buffer kind ([`buffer_kind`]); both kinds are
/// accepted because the output is ASCII. Immediate: `capacity: u32`, the
/// destination's declared capacity; it must be non-zero. The `f64` operand is
/// its 64-bit IEEE pattern and `frac_digits` is a `u64`; at most 9 fraction
/// digits are supported, and a larger width pushes `-4` (`Status::Invalid`)
/// and writes nothing. `NaN` is written as `nan`, `+Inf`/`-Inf` as `inf`/
/// `-inf`, literally and ignoring the width. A magnitude at or above `2^63`
/// also pushes `-4`: its integer part does not fit the fixed-point form.
/// Otherwise the output is an optional `-`, the integer part without leading
/// zeros, and (when the width is non-zero) a `.` and exactly that many
/// fraction digits, rounded half away from zero with the carry applied to the
/// integer part. A result that does not fit `capacity` pushes `-6`
/// (`Status::CapacityExceeded`) and leaves the destination length alone.
pub const FORMAT_F64: u8 = 0x51;

/// Pop an `f64` value and a destination reference, format the value as the
/// shortest decimal text that parses back to the same bits, and push the byte
/// count (`core.format_f64_short_into`, spec 0003, Q41).
///
/// `a` is the destination's buffer kind ([`buffer_kind`]); both kinds are
/// accepted because the output is ASCII. Immediate: `capacity: u32`, the
/// destination's declared capacity; it must be non-zero. `b` and `c` are
/// reserved and must be zero. There is no width argument and no magnitude
/// bound: the formatter picks the shortest significant-digit string that
/// round-trips, in positional form while `-6 < e10 + 1 <= 21` and in
/// scientific form otherwise. `NaN` is written as `nan`, `+Inf`/`-Inf` as
/// `inf`/`-inf`, and a zero as `0`/`-0`. A result that does not fit `capacity`
/// pushes `-6` (`Status::CapacityExceeded`) and leaves the destination length
/// alone.
pub const FORMAT_F64_SHORT: u8 = 0x52;

/// IEEE-754 double addition.
pub const FADD: u8 = 0x60;
/// IEEE-754 double subtraction.
pub const FSUB: u8 = 0x61;
/// IEEE-754 double multiplication.
pub const FMUL: u8 = 0x62;
/// IEEE-754 double division (division by zero is `Inf`/`NaN`, not a fault).
pub const FDIV: u8 = 0x63;
/// IEEE-754 double negation.
pub const FNEG: u8 = 0x64;
/// Floating equality (`NaN` is unequal to everything, including itself).
pub const FEQ: u8 = 0x65;
/// Floating inequality (`NaN <> NaN` is true).
pub const FNE: u8 = 0x66;
/// Floating less-than (`false` when either operand is `NaN`).
pub const FLT: u8 = 0x67;
/// Floating less-or-equal.
pub const FLE: u8 = 0x68;
/// Floating greater-than.
pub const FGT: u8 = 0x69;
/// Floating greater-or-equal.
pub const FGE: u8 = 0x6A;

/// Scalar conversion. `a` is the source kind and `b` the destination kind,
/// both from [`value_kind`]. Integer narrowing and float-to-integer
/// conversions are range checked and fault `Overflow`.
pub const CONVERT: u8 = 0x70;

/// Scalar kinds used by [`CONVERT`] and by the compiler's type checker.
pub mod value_kind {
    /// Signed 64-bit integer.
    pub const INT: u8 = 0;
    /// Unsigned 64-bit integer.
    pub const UINT: u8 = 1;
    /// Unsigned 8-bit integer, held zero-extended in an 8-byte slot.
    pub const BYTE: u8 = 2;
    /// IEEE-754 binary64.
    pub const F64: u8 = 3;
}

/// Buffer kinds used by [`SUBSTR`].
pub mod buffer_kind {
    /// Valid UTF-8 text; slice boundaries are code-point checked.
    pub const STRING: u8 = 0;
    /// Raw bytes; no UTF-8 check.
    pub const BYTES: u8 = 1;
}

/// Map key kinds used by the `MAP_*` opcodes (spec 0003, "Maps").
///
/// A buffer key is hashed over `[0..len)` and compared by length then bytes;
/// `string` and `bytes` are distinct key kinds, like they are distinct types.
pub mod map_key_kind {
    /// A signed 64-bit integer, hashed over its eight little-endian bytes.
    pub const INT: u8 = 0;
    /// A `bytes[M]` key, hashed over `[0..len)`.
    pub const BYTES: u8 = 1;
    /// A `string[M]` key, hashed over `[0..len)` (byte-wise).
    pub const STRING: u8 = 2;
}

/// Map value kinds used by the `MAP_*` opcodes (spec 0003, "Maps").
pub mod map_value_kind {
    /// A scalar or enum: one 8-byte value on the operand stack.
    pub const SCALAR: u8 = 0;
    /// A record, sum, array, ring or map: a reference to its first byte, copied
    /// whole (`value_size` bytes).
    pub const FIXED: u8 = 1;
    /// A `string`/`bytes` value: a reference to its data (length at `ref - 8`),
    /// copied as a `[len][data]` image into `value_size` bytes.
    pub const BUFFER: u8 = 2;
}

/// Host builtin identifiers used by [`BUILTIN`].
pub mod builtin {
    /// `core.write(bytes)`: pop a buffer reference and write its bytes to the
    /// host console.
    pub const WRITE: u32 = 1;
    /// `core.clock()`: push the host's monotonic nanosecond counter.
    pub const CLOCK: u32 = 2;
    /// `core.spawn(path, args, var diag, var id)`: pop an id reference, a
    /// `Diagnostic` record reference and two buffer references; on success
    /// write the reserved task id through the id reference (and the cleared
    /// diagnostic through its reference) and push `Ok` (0). On failure the id
    /// reference is untouched, the host fills the diagnostic and the error
    /// `Status` ordinal is pushed (spec 0005).
    pub const SPAWN: u32 = 3;
    /// `core.wait(id, var reason)`: pop an `ExitReason` sum reference and a
    /// task id; when the child has terminated write the tag (`0` Ok, `1`
    /// Killed, `2` Faulted) and the payload (`Faulted`'s `FaultCode` ordinal,
    /// zero otherwise) through the reference and push `Ok`; when the wait
    /// itself fails push that `Status` ordinal. May block; the instruction is
    /// retried on wake without consuming operands.
    pub const WAIT: u32 = 4;
    /// `core.kill(id)`: pop a task id, push the `Status` ordinal (`Ok` on
    /// success, `NotFound` for an unknown or finished id).
    pub const KILL: u32 = 5;
    /// `core.run(path, args, var diag, var reason)`: pop an `ExitReason` sum
    /// reference, a `Diagnostic` record reference and the two buffer
    /// references (the reason reference is on top). Spawns once, then waits for
    /// the child. On a
    /// synchronous spawn failure the host fills the diagnostic and the error
    /// `Status` is pushed with the reason untouched. On completion the reason
    /// is written and `Ok` is pushed. May block while the child is live; the
    /// pending id lives in the machine, so the retry never spawns twice.
    pub const RUN: u32 = 9;
    /// `core.len(buffer)`: pop a `string` or `bytes` reference, push its
    /// runtime length as a `uint`. One builtin serves both kinds; the length is
    /// read from the buffer header, so nothing is allocated or copied.
    pub const LEN: u32 = 6;
    /// `syslib.port_in8(port)`: pop a staged `uint` port number, push the byte
    /// read from the x86 port space as a `uint`. The low 16 bits of the port
    /// number are used; `Port` is staged as a scalar (Q45).
    pub const PORT_IN8: u32 = 7;
    /// `syslib.port_out8(port, value)`: pop a value and a port number, write
    /// one byte and push `0`, or push `-4` (`Status::Invalid`) without writing
    /// when `value > 255`. The low 16 bits of the port number are used.
    pub const PORT_OUT8: u32 = 8;
    /// `syslib.cpu_id()`: push the APIC id of the core executing the call as a
    /// `uint`. This is the identity exposed to software (`0002` "SMP"), not a
    /// task affinity: a task may be claimed by a different core next quantum.
    pub const CPU_ID: u32 = 10;
    /// `core.realtime_on()`: promote the calling task to the realtime class
    /// (spec 0007, ADR 0016). Pushes `Status`: `Ok` when the task is promoted,
    /// or when it is already realtime (no second admission slot), and `Full`
    /// when every online core already holds one realtime task. Promotion is
    /// one-way; there is no `realtime_off`.
    pub const REALTIME_ON: u32 = 11;
    /// `core.heartbeat()`: reset the calling task's one-second heartbeat
    /// deadline measured in non-blocked time (spec 0007, ADR 0016). No
    /// operands and no result. It is a no-op for a normal (non-realtime) task,
    /// which keeps the normal total budget.
    pub const HEARTBEAT: u32 = 12;
    /// `syslib.screen_width()`: push the shadow screen's width in pixels as a
    /// `uint`, or `0` when graphics are disabled (spec 0009, G15).
    pub const SCREEN_WIDTH: u32 = 13;
    /// `syslib.screen_height()`: push the shadow screen's height in pixels as a
    /// `uint`, or `0` when graphics are disabled.
    pub const SCREEN_HEIGHT: u32 = 14;
    /// `syslib.fill_rect(x, y, w, h, color) -> Status`: fill an extent of the
    /// shadow. The rectangle is clipped to the clip region and the screen;
    /// `w <= 0`, `h <= 0` or a colour above `0x00FF_FFFF` pushes
    /// `Status::Invalid` (`4`) and draws nothing (spec 0009, G8/G9).
    pub const FILL_RECT: u32 = 15;
    /// `syslib.fill_dither(x, y, w, h, color) -> Status`: write `color` to every
    /// pixel `(X, Y)` of the extent with `(X + Y)` even, leaving the others
    /// untouched. Same clipping and error rules as `FILL_RECT` (G11).
    pub const FILL_DITHER: u32 = 16;
    /// `syslib.draw_rect(x, y, w, h, color) -> Status`: a one-pixel outline
    /// inside the extent, interior untouched. Same error rules (G12).
    pub const DRAW_RECT: u32 = 17;
    /// `syslib.draw_text(x, y, text, color) -> Status`: pop a colour, a
    /// `string` reference, `y` and `x`; draw 8×8 console-font glyphs with a
    /// transparent background and no newline handling. Bytes outside
    /// `0x00..=0x7F` are blank cells. Clipped (G13).
    pub const DRAW_TEXT: u32 = 18;
    /// `syslib.get_pixel(x, y) -> uint`: push the shadow pixel as
    /// `0x00RRGGBB`, or `0` outside the screen. No error path (G9).
    pub const GET_PIXEL: u32 = 19;
    /// `syslib.present(x, y, w, h) -> Status`: copy the rectangle from the
    /// shadow to the framebuffer row by row, using the pitch, clipped.
    /// `w <= 0` or `h <= 0` pushes `Status::Invalid` (G5).
    pub const PRESENT: u32 = 20;
    /// `syslib.clear(color) -> Status`: fill the whole screen in the shadow and
    /// present it. An over-wide colour pushes `Status::Invalid` (G6).
    pub const CLEAR: u32 = 21;
    /// `syslib.pointer_x() -> int`: the pointer's screen `x` in pixels
    /// (spec 0010, I10). `0` when there is no pointer device.
    pub const POINTER_X: u32 = 22;
    /// `syslib.pointer_y() -> int`: the pointer's screen `y` in pixels. `0`
    /// when there is no pointer device.
    pub const POINTER_Y: u32 = 23;
    /// `syslib.pointer_buttons() -> uint`: the button mask, bit 0 left, bit 1
    /// right, bit 2 middle. `0` when there is no pointer device.
    pub const POINTER_BUTTONS: u32 = 24;
    /// `syslib.inject_pointer(dx, dy, buttons) -> Status`: **debug-only** test
    /// hook (spec 0010, I10/I12). Moves the pointer by `dx`/`dy` screen pixels
    /// with the same clamping as a real report, sets the button mask and
    /// repaints the cursor. `buttons > 0b111` pushes `-4` (`Status::Invalid`)
    /// and changes nothing. M2.2.4 adds `inject_event` for keyboard and queue
    /// tests.
    pub const INJECT_POINTER: u32 = 25;

    /// `syslib.pointer_source() -> uint`: which device currently drives the
    /// pointer (spec 0010, I27): `0` none, `1` PS/2 auxiliary device, `2`
    /// absolute USB tablet. The value is machine-wide and does not change with
    /// graphics enablement.
    pub const POINTER_SOURCE: u32 = 26;

    /// `syslib.next_event(var ev) -> Status`: pop an `Event` record reference
    /// and fill it from the front of the machine-wide event queue (spec 0010,
    /// I19). When the queue is empty the machine blocks on input and retries
    /// the same instruction on wake. On success the record is written and `Ok`
    /// pushed; an error `Status` is pushed with the record untouched.
    pub const NEXT_EVENT: u32 = 27;

    /// `syslib.poll_event(var ev) -> Status`: as `NEXT_EVENT` but never blocks;
    /// an empty queue pushes `WouldBlock` and leaves the record untouched.
    pub const POLL_EVENT: u32 = 28;

    /// `syslib.inject_event(ev: Event) -> Status`: **debug-only** test hook
    /// (spec 0010, I19). Pop an `Event` record reference and append its image
    /// to the event queue. An unknown `kind` ordinal pushes `Invalid` and
    /// queues nothing. It does not move the pointer; use `INJECT_POINTER` for
    /// the pointer state.
    pub const INJECT_EVENT: u32 = 29;

    /// `syslib.event_drops() -> uint`: events dropped because the queue was
    /// full since boot (spec 0010, I19). Monotonic.
    pub const EVENT_DROPS: u32 = 30;

    /// `syslib.event_flush() -> uint`: discard every queued event and push how
    /// many were discarded. The drop counter is not reset.
    pub const EVENT_FLUSH: u32 = 31;

    // ---- window service (spec 0011) -------------------------------------
    //
    // The window table and its policy live in the kernel (`kernel/src/window.rs`).
    // These builtins are the whole application surface: an id is an `int`,
    // `0` is never valid, and every result is a `Status` except the focus read
    // and the two rectangle reads, which write the predeclared `Rect` image
    // (`abi::layout::rect`) only on success. The host defaults describe a
    // disabled service, so a host mock and `langhost` need no screen.

    /// `syslib.window_open(x, y, w, h, title, var id) -> Status`: open a window
    /// with frame origin `(x, y)` and content size `(w, h)`. Pushes `Ok` and
    /// writes the new id on success; `Invalid` when graphics are disabled, the
    /// content is below the minimum or the frame does not fit the screen;
    /// `Full` when the table is full. `a`, `b` and `c` are reserved. Operands:
    /// `[x][y][w][h][title reference][id reference]`.
    pub const WINDOW_OPEN: u32 = 32;

    /// `syslib.window_close(id) -> Status`: close a window owned by the calling
    /// task. `Ok`; `NotFound` for an unknown id; `Invalid` for a live window
    /// owned by another task.
    pub const WINDOW_CLOSE: u32 = 33;

    /// `syslib.window_raise(id) -> Status`: move a window to the top of the
    /// z-order and focus it. Any task may raise any window. `Ok` or `NotFound`.
    pub const WINDOW_RAISE: u32 = 34;

    /// `syslib.window_move(id, x, y) -> Status`: set the frame origin of a
    /// window owned by the calling task, clamped so the frame stays on screen.
    /// `Ok`; `NotFound` for an unknown id; `Invalid` for another task's window.
    /// Operands: `[id][x][y]`.
    pub const WINDOW_MOVE: u32 = 35;

    /// `syslib.window_frame_rect(id, var rect) -> Status`: write the frame
    /// rectangle through the `Rect` reference. `Ok`; `NotFound` for an unknown
    /// id; `Invalid` when graphics are disabled. Operands: `[id][rect
    /// reference]`.
    pub const WINDOW_FRAME_RECT: u32 = 36;

    /// `syslib.window_content_rect(id, var rect) -> Status`: write the content
    /// rectangle (where the application draws) through the `Rect` reference.
    /// Same results as [`WINDOW_FRAME_RECT`].
    pub const WINDOW_CONTENT_RECT: u32 = 37;

    /// `syslib.window_focus() -> int`: push the focused window id, or `0` when
    /// no window exists or graphics are disabled. No operands.
    pub const WINDOW_FOCUS: u32 = 38;

    /// `syslib.window_present(id) -> Status`: present a window's content
    /// rectangle from the shadow, clipped to the window's visible parts (its
    /// content minus the frames of higher windows), so a client's presentation
    /// cannot cover a higher window or its chrome (spec 0011, W17). Owner-only
    /// like `close`/`move`: `NotFound` for an unknown id, `Invalid` for another
    /// task's window or disabled graphics.
    pub const WINDOW_PRESENT: u32 = 39;

    /// `syslib.window_info(index, var id, var rect, var title) -> Status`: the
    /// read-only window enumeration (spec 0012, D15; spec 0011, W19). Index `0`
    /// is the **topmost** live window and indices increase down the z-order.
    /// `Ok` writes the id, the frame rectangle (a `Rect` image) and the title
    /// (at most `layout::window::TITLE_CAP` bytes) and pushes `Ok`; an index at
    /// or past the live count, or disabled graphics, is `NotFound` and writes
    /// nothing. Operands: `[index][id reference][rect reference][title
    /// reference]`; the title reference names a `var string` whose
    /// compile-checked capacity is at least the title cap.
    pub const WINDOW_INFO: u32 = 40;

    // ---- application files (spec 0013, spec 0018) -----------------------
    //
    // The read-only ext2 image tree is reachable through three `core`
    // builtins. Paths are image-relative (`base/shell`, `lib/ui/menu.ig`);
    // `fs_list` takes the root (`""`/`"."`) or a directory path and
    // `fs_read`/`fs_size` take a file path. No opcode is added: the machine
    // validates the path syntax and the destinations, the host walks the path
    // and copies the bytes.

    /// `core.fs_list(path, index, var name, var kind, var size) -> Status`: the
    /// `index`-th named entry of the directory at `path` (`0`-based, directory
    /// order, `.` and `..` excluded). `Ok` writes the name (at most
    /// [`crate::fs::NAME_CAP`] bytes), the kind (`0` regular file, `1`
    /// directory, `2` other) and the inode size, and pushes `Ok`; an index past
    /// the end or a missing directory is `NotFound`; a malformed path is
    /// `Invalid`; a name longer than the destination is `CapacityExceeded`.
    /// Every destination is untouched on failure.
    /// Operands: `[path reference][index][name reference][kind reference][size
    /// reference]`.
    pub const FS_LIST: u32 = 41;

    /// `core.fs_read(path, offset, var dst, var read) -> Status`: copy up to
    /// [`crate::fs::READ_CHUNK`] bytes of the regular root file at byte
    /// `offset` into a `var bytes[N]` destination (`N >= READ_CHUNK`).
    /// `Ok` sets the destination's runtime length and `read` to the copied
    /// count; at or past the file's end it writes `0` bytes and pushes `Ok`.
    /// A failure leaves `dst` and `read` untouched: `NotFound` for a missing
    /// name, `Invalid` for a directory/other entry or a path with a separator,
    /// `CapacityExceeded` for a path longer than 256 bytes, `IoError` for a
    /// corrupt image. Operands: `[path reference][offset][dst reference][read
    /// reference]`.
    pub const FS_READ: u32 = 42;

    /// `core.fs_size(path, var size) -> Status`: the byte size of a regular
    /// root file. `Ok` writes `size`; the errors are `FS_READ`'s (a missing
    /// name is `NotFound`, a directory or bad path `Invalid`). Operands:
    /// `[path reference][size reference]`.
    pub const FS_SIZE: u32 = 43;

    // ---- the writable home filesystem (spec 0019, A1–A3) --------------
    //
    // `core.fs_write` is the first write primitive: only a path whose first
    // component is `home` is writable, and only when the home disk is mounted
    // (otherwise the mount is read-only). No opcode is added: the machine
    // validates the path and the data buffer, and the host writes through the
    // ext2 writer. The remaining write primitives (remove/mkdir/rename) are
    // builtins 54–56 in M6.3.

    /// `core.fs_write(path, offset, data, var written) -> Status`: write
    /// `data` at byte `offset` of the regular file at `path` (spec 0019, A1/A3).
    /// Only a path whose first component is `home` is writable; every other path
    /// and a read-only mount are `Invalid`. A missing file is created
    /// implicitly when `offset` is `0` and only then; a missing parent is
    /// `NotFound` and a directory target is `Invalid`. `Ok` sets `written` to
    /// the copied byte count; every failure leaves `written` untouched. A write
    /// that would leave the double-indirect reach, or that needs more blocks
    /// than are free, is `CapacityExceeded`. The data buffer is read-only.
    /// Operands: `[path reference][offset][data reference][written reference]`.
    pub const FS_WRITE: u32 = 53;

    // ---- terminal window (spec 0014) ------------------------------------
    //
    // `core.terminal_attach` binds the machine-wide console terminal grid to a
    // window owned by the calling task. The kernel host supplies the caller's
    // task id; there is no application-visible task id or window-content
    // primitive. No opcode is added: the machine only stages the window id and
    // the kernel performs the ownership check and the geometry derivation.

    /// `core.terminal_attach(window_id) -> Status`: attach the console's
    /// machine-wide terminal grid to a window owned by the calling task
    /// (spec 0014). `Ok` on success; `NotFound` for an unknown window;
    /// `Invalid` for a foreign window, disabled graphics or a content rectangle
    /// too small for one 8×8 cell; `Full` when another task already holds the
    /// one terminal. A repeated attach by the current owner is `Ok`.
    /// Operands: `[window id]`.
    pub const TERMINAL_ATTACH: u32 = 44;

    // ---- proportional font (spec 0015) ----------------------------------
    //
    // The font lives in the kernel (`kernel/src/font.rs`); these three
    // builtins expose its metrics and its drawing path. `draw_text` stays the
    // 8×8 fallback and is unchanged. No opcode is added: the machine resolves
    // the text reference and the host owns the rasterisation.

    /// `syslib.text_width_prop(text) -> uint`: the measured advance width of
    /// `text` in the proportional font, in pixels (spec 0015). Printable ASCII
    /// contributes its variable glyph advance; every other byte contributes
    /// the 8×8 fallback advance of 8. `0` when graphics are disabled.
    /// Operands: `[text reference]`.
    pub const TEXT_WIDTH_PROP: u32 = 45;

    /// `syslib.text_height_prop() -> uint`: the proportional font's line
    /// height in pixels (12), or `0` when graphics are disabled. No operands.
    pub const TEXT_HEIGHT_PROP: u32 = 46;

    /// `syslib.draw_text_prop(x, y, text, color) -> Status`: draw proportional
    /// text with a transparent background (spec 0015). `y` is the top of the
    /// 12-pixel line box; printable ASCII uses the variable-width glyphs,
    /// other bytes keep an 8×8 cell with an 8-pixel advance, and bytes at or
    /// above `0x80` are blank. A colour above `0x00FF_FFFF` is `Invalid`; the
    /// screen and the clip bound every pixel, so an off-screen origin is not
    /// an error. Operands: `[x][y][text reference][color]`.
    pub const DRAW_TEXT_PROP: u32 = 47;

    // ---- system control (spec 0017) -------------------------------------
    //
    // `core.halt` and `core.reboot` are the desktop's system-control
    // primitives. They have no application-visible capability check (spec
    // 0001 R14): every task may stop or reset the machine. The kernel host
    // owns the stop/reset path; a bare host (`langhost`) reports `Invalid`.

    /// `core.halt() -> Status`: request a machine-wide halt (spec 0017). The
    /// kernel host stops every other online core through the existing
    /// shutdown path and halts the calling core, so it never returns on a
    /// live kernel; `Invalid` is the disabled-service default of a bare host.
    /// No operands.
    pub const HALT: u32 = 48;

    /// `core.reboot() -> Status`: request a system reset (spec 0017). The
    /// kernel host requests the shutdown path (bounded AP stop), then resets
    /// through the platform reset channels; if the firmware ignores them the
    /// core halts instead of returning. `Invalid` is the bare-host default.
    /// No operands.
    pub const REBOOT: u32 = 49;

    // ---- correctly rounded float parsing (spec 0003, Q41) ----------------
    //
    // The decimal reader lives in the interpreter, not in `stdlib` language
    // code: a correctly rounded conversion needs exact integer arithmetic over
    // the whole mantissa, which `core`'s `f64: FromStr` already implements
    // allocation-free. The machine checks the spec grammar and publishes the
    // bits, so there is no host call and no `Host` method.

    /// `core.parse_f64_into(src, var value) -> Status`: parse decimal text
    /// into a binary64 `f64` with correct rounding (round half to even),
    /// including long mantissas, subnormals and the overflow/underflow
    /// boundaries (spec 0003, "Floating-point text"). The grammar is
    /// `[+-]? ( digits [ '.' digits? ] | '.' digits ) ( [eE] [+-]? digits )?`
    /// plus the exact strings `inf`, `+inf`, `-inf` and `nan`. `Ok` writes the
    /// 8-byte bit pattern through `value` and pushes `Ok`; a malformed string
    /// (including one that is not UTF-8) pushes `Invalid` and leaves `value`
    /// untouched. An overflowing magnitude is `±Inf` and an underflow `±0`,
    /// both with `Ok`; `-0.0` and `-inf` keep their sign. Operands:
    /// `[src reference][value reference]`.
    pub const PARSE_F64: u32 = 50;

    // ---- multi-source waits (spec 0003, spec 0007; Q39, ADR 0036) ----
    //
    // `poll_any` and `next_any` inspect a `const array[uint, 8]` of source
    // values and report the index of the first ready one. Source `0` is one
    // console input byte being available, `1` is one machine-wide input event (including a
    // pending window `Expose`) and any value `>= 2` is a stream endpoint handle
    // whose data or close is ready (a read select). The array reference and the
    // caller's `count` are staged; the interpreter validates the count and the
    // region, then calls one `Host` readiness probe per source in order.
    //
    // `WouldBlock` means no source is ready and leaves the `ready`
    // out-parameter untouched. `Ok` writes the zero-based index of the source
    // that was ready *at the probe*; another task may consume it before the
    // caller performs the corresponding read, so that read's status is
    // authoritative. `next_any` blocks on the same set instead of returning
    // `WouldBlock`; the wait is one `WaitFor::Any` condition and every producer
    // wakes a set that contains its source.

    /// `core.poll_any(sources, count, var ready) -> Status`: non-blocking
    /// multi-source readiness probe. Pops `[sources reference][count][ready
    /// reference]`; on `Ok` writes the zero-based index of the first ready
    /// source through `ready`. `WouldBlock` (nothing ready) leaves `ready`
    /// untouched and is the only non-`Ok` result for a well-formed call;
    /// `count` outside `1..=8` is `Invalid`, also leaving `ready` untouched.
    pub const POLL_ANY: u32 = 51;

    /// `core.next_any(sources, count, var ready) -> Status`: the blocking form
    /// of [`POLL_ANY`]. It reports a ready source exactly like `poll_any`; when
    /// none is ready it blocks the task on the same source set and retries the
    /// instruction after a wake-up. The operand boundary is unchanged on the
    /// block, so the retry re-reads the caller's array and `count`.
    pub const NEXT_ANY: u32 = 52;

    /// Returns the number of operands popped and pushed for a builtin.
    pub const fn effect(id: u32) -> Option<(u8, u8)> {
        match id {
            WRITE => Some((1, 0)),
            CLOCK => Some((0, 1)),
            SPAWN => Some((4, 1)),
            WAIT => Some((2, 1)),
            KILL => Some((1, 1)),
            RUN => Some((4, 1)),
            LEN => Some((1, 1)),
            PORT_IN8 => Some((1, 1)),
            PORT_OUT8 => Some((2, 1)),
            CPU_ID => Some((0, 1)),
            REALTIME_ON => Some((0, 1)),
            HEARTBEAT => Some((0, 0)),
            SCREEN_WIDTH => Some((0, 1)),
            SCREEN_HEIGHT => Some((0, 1)),
            FILL_RECT => Some((5, 1)),
            FILL_DITHER => Some((5, 1)),
            DRAW_RECT => Some((5, 1)),
            DRAW_TEXT => Some((4, 1)),
            GET_PIXEL => Some((2, 1)),
            PRESENT => Some((4, 1)),
            CLEAR => Some((1, 1)),
            POINTER_X => Some((0, 1)),
            POINTER_Y => Some((0, 1)),
            POINTER_BUTTONS => Some((0, 1)),
            INJECT_POINTER => Some((3, 1)),
            POINTER_SOURCE => Some((0, 1)),
            NEXT_EVENT => Some((1, 1)),
            POLL_EVENT => Some((1, 1)),
            INJECT_EVENT => Some((1, 1)),
            EVENT_DROPS => Some((0, 1)),
            EVENT_FLUSH => Some((0, 1)),
            WINDOW_OPEN => Some((6, 1)),
            WINDOW_CLOSE => Some((1, 1)),
            WINDOW_RAISE => Some((1, 1)),
            WINDOW_MOVE => Some((3, 1)),
            WINDOW_FRAME_RECT => Some((2, 1)),
            WINDOW_CONTENT_RECT => Some((2, 1)),
            WINDOW_FOCUS => Some((0, 1)),
            WINDOW_PRESENT => Some((1, 1)),
            WINDOW_INFO => Some((4, 1)),
            FS_LIST => Some((5, 1)),
            FS_READ => Some((4, 1)),
            FS_SIZE => Some((2, 1)),
            FS_WRITE => Some((4, 1)),
            TERMINAL_ATTACH => Some((1, 1)),
            TEXT_WIDTH_PROP => Some((1, 1)),
            TEXT_HEIGHT_PROP => Some((0, 1)),
            DRAW_TEXT_PROP => Some((4, 1)),
            HALT => Some((0, 1)),
            REBOOT => Some((0, 1)),
            PARSE_F64 => Some((2, 1)),
            POLL_ANY => Some((3, 1)),
            NEXT_ANY => Some((3, 1)),
            _ => None,
        }
    }
}

/// Number of 4-byte words in the instruction starting with `op`, including the
/// opcode word. Returns `None` for an unknown opcode.
pub const fn instruction_words(op: u8) -> Option<u32> {
    match op {
        DROP | RET | LOAD_REF | STORE_REF | NOT | CLEAR | CONVERT | FADD | FSUB | FMUL | FDIV
        | FNEG | FEQ | FNE | FLT | FLE | FGT | FGE => Some(1),
        ADD | SUB | MUL | DIV | MOD | NEG | ADD_U | SUB_U | MUL_U | DIV_U | MOD_U | EQ | NE
        | LT | LE | GT | GE | LT_U | LE_U | GT_U | GE_U | LOAD_U8 | CMP_BUFFERS | FIND
        | COPY_REFS | STREAM_SEND | STREAM_TRY_SEND => Some(1),
        CONST => Some(3),
        JMP | JZ | JNZ | CALL | LOAD_LOCAL | STORE_LOCAL | REF_LOCAL | LOAD_STATE | STORE_STATE
        | REF_STATE | REF_CONST | APPEND | READ_INPUT | STORE_U8 | BUILTIN | SUBSTR
        | FORMAT_INT | ENUM_CAST | REF_OFFSET | STREAM_RECV | STREAM_TRY_RECV | FORMAT_F64
        | FORMAT_F64_SHORT => Some(2),
        REF_ELEMENT => Some(3),
        MAP_PUT | MAP_GET | MAP_HAS | MAP_REMOVE => Some(5),
        MAP_COUNT => Some(1),
        _ => None,
    }
}

/// Returns the mnemonic of `op`, or `None` for an unknown opcode.
pub const fn mnemonic(op: u8) -> Option<&'static str> {
    match op {
        DROP => Some("drop"),
        CONST => Some("const"),
        JMP => Some("jmp"),
        JZ => Some("jz"),
        JNZ => Some("jnz"),
        CALL => Some("call"),
        RET => Some("ret"),
        LOAD_LOCAL => Some("load_local"),
        STORE_LOCAL => Some("store_local"),
        REF_LOCAL => Some("ref_local"),
        LOAD_STATE => Some("load_state"),
        STORE_STATE => Some("store_state"),
        REF_STATE => Some("ref_state"),
        LOAD_REF => Some("load_ref"),
        STORE_REF => Some("store_ref"),
        REF_CONST => Some("ref_const"),
        LOAD_U8 => Some("load_u8"),
        STORE_U8 => Some("store_u8"),
        REF_OFFSET => Some("ref_offset"),
        COPY_REFS => Some("copy_refs"),
        REF_ELEMENT => Some("ref_element"),
        ADD => Some("add"),
        SUB => Some("sub"),
        MUL => Some("mul"),
        DIV => Some("div"),
        MOD => Some("mod"),
        NEG => Some("neg"),
        ADD_U => Some("add_u"),
        SUB_U => Some("sub_u"),
        MUL_U => Some("mul_u"),
        DIV_U => Some("div_u"),
        MOD_U => Some("mod_u"),
        EQ => Some("eq"),
        NE => Some("ne"),
        LT => Some("lt"),
        LE => Some("le"),
        GT => Some("gt"),
        GE => Some("ge"),
        LT_U => Some("lt_u"),
        LE_U => Some("le_u"),
        GT_U => Some("gt_u"),
        GE_U => Some("ge_u"),
        NOT => Some("not"),
        CMP_BUFFERS => Some("cmp_buffers"),
        CLEAR => Some("clear"),
        APPEND => Some("append"),
        READ_INPUT => Some("read_input"),
        FIND => Some("find"),
        SUBSTR => Some("substr"),
        FORMAT_INT => Some("format_int"),
        FORMAT_F64 => Some("format_f64"),
        FORMAT_F64_SHORT => Some("format_f64_short"),
        ENUM_CAST => Some("enum_cast"),
        MAP_PUT => Some("map_put"),
        MAP_GET => Some("map_get"),
        MAP_HAS => Some("map_has"),
        MAP_REMOVE => Some("map_remove"),
        MAP_COUNT => Some("map_count"),
        STREAM_SEND => Some("stream_send"),
        STREAM_TRY_SEND => Some("stream_try_send"),
        STREAM_RECV => Some("stream_recv"),
        STREAM_TRY_RECV => Some("stream_try_recv"),
        BUILTIN => Some("builtin"),
        FADD => Some("fadd"),
        FSUB => Some("fsub"),
        FMUL => Some("fmul"),
        FDIV => Some("fdiv"),
        FNEG => Some("fneg"),
        FEQ => Some("feq"),
        FNE => Some("fne"),
        FLT => Some("flt"),
        FLE => Some("fle"),
        FGT => Some("fgt"),
        FGE => Some("fge"),
        CONVERT => Some("convert"),
        _ => None,
    }
}

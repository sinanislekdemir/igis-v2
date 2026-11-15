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
/// Pop a `string` buffer reference and ask the host for one finished console
/// line (`core.read_line`). Immediate: `capacity: u32`, the destination's
/// declared capacity. Pushes the copied length, or blocks the task with
/// `WaitKind::Console` when no line is complete. The copied length is at most
/// the capacity; a longer line is truncated and the rest discarded.
pub const READ_LINE: u8 = 0x42;
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

/// Invoke a host builtin. Immediate: `builtin: u32` (see [`builtin`]).
pub const BUILTIN: u8 = 0x50;

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
        | COPY_REFS => Some(1),
        CONST => Some(3),
        JMP | JZ | JNZ | CALL | LOAD_LOCAL | STORE_LOCAL | REF_LOCAL | LOAD_STATE | STORE_STATE
        | REF_STATE | REF_CONST | APPEND | READ_LINE | STORE_U8 | BUILTIN | SUBSTR | FORMAT_INT
        | ENUM_CAST | REF_OFFSET => Some(2),
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
        READ_LINE => Some("read_line"),
        FIND => Some("find"),
        SUBSTR => Some("substr"),
        FORMAT_INT => Some("format_int"),
        ENUM_CAST => Some("enum_cast"),
        MAP_PUT => Some("map_put"),
        MAP_GET => Some("map_get"),
        MAP_HAS => Some("map_has"),
        MAP_REMOVE => Some("map_remove"),
        MAP_COUNT => Some("map_count"),
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

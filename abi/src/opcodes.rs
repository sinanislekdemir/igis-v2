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
/// Push a reference to `locals[off]` (a buffer's data start). Immediate: `off: u32`.
pub const REF_LOCAL: u8 = 0x12;
/// Push the scalar 8 bytes at `state[off]`. Immediate: `off: u32`.
pub const LOAD_STATE: u8 = 0x13;
/// Pop a scalar into `state[off]`. Immediate: `off: u32`.
pub const STORE_STATE: u8 = 0x14;
/// Push a reference to `state[off]` (a buffer's data start). Immediate: `off: u32`.
pub const REF_STATE: u8 = 0x15;
/// Pop a reference, push the scalar 8 bytes it names.
pub const LOAD_REF: u8 = 0x16;
/// Pop a value, pop a reference, store the value through the reference.
pub const STORE_REF: u8 = 0x17;
/// Push a read-only reference into the constant blob. Immediate: `off: u32`.
pub const REF_CONST: u8 = 0x18;

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

/// Pop a destination reference and set its buffer length to zero.
pub const CLEAR: u8 = 0x40;
/// Pop a source reference, pop a destination reference, append the source
/// bytes to the destination. Immediate: `capacity: u32`, the destination's
/// declared capacity. Overflow faults `CapacityExceeded`.
pub const APPEND: u8 = 0x41;

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

/// Host builtin identifiers used by [`BUILTIN`].
pub mod builtin {
    /// `core.write(bytes)`: pop a buffer reference and write its bytes to the
    /// host console.
    pub const WRITE: u32 = 1;
    /// `core.clock()`: push the host's monotonic nanosecond counter.
    pub const CLOCK: u32 = 2;

    /// Returns the number of operands popped and pushed for a builtin.
    pub const fn effect(id: u32) -> Option<(u8, u8)> {
        match id {
            WRITE => Some((1, 0)),
            CLOCK => Some((0, 1)),
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
        | LT | LE | GT | GE | LT_U | LE_U | GT_U | GE_U => Some(1),
        CONST => Some(3),
        JMP | JZ | JNZ | CALL | LOAD_LOCAL | STORE_LOCAL | REF_LOCAL | LOAD_STATE | STORE_STATE
        | REF_STATE | REF_CONST | APPEND | BUILTIN => Some(2),
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
        CLEAR => Some("clear"),
        APPEND => Some("append"),
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

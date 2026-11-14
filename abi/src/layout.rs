//! Fixed record layouts shared by the compiler, the interpreter and the kernel.
//!
//! The language declares two records before user code (`Diagnostic` and
//! `ExitReason`), and the kernel writes their fields directly. The offsets here
//! are the single source of truth on both sides of the boundary: the compiler
//! uses them when it builds the predeclared record table, the interpreter uses
//! them when it stores a wait result, and the kernel uses them when it fills a
//! spawn diagnostic.
//!
//! Both records follow the language layout rule (spec 0003, "Records"): fields
//! in declaration order, each aligned to 8 bytes, a buffer field occupying
//! `[len: u64][data: cap]`, and the record size rounded to 8.
//!
//! `ExitReason` is the staged record form of the spec's sum type: `kind` is
//! `0` Ok, `1` Killed, `2` Faulted, and `code` is the `FaultCode` ordinal
//! (zero unless faulted). It becomes a real sum type when the language has one.

/// `Diagnostic`: the caller-owned compile/load error record (spec 0003).
pub mod diagnostic {
    use super::{write_i64, write_text};

    /// Record size in bytes.
    pub const SIZE: u32 = 192;

    /// Record alignment in bytes.
    pub const ALIGN: u32 = 8;

    /// Offset of `unit_name` (a `string[32]`, so the length word is here).
    pub const UNIT_NAME_OFFSET: u32 = 0;

    /// Capacity of `unit_name` in bytes.
    pub const UNIT_NAME_CAP: u32 = 32;

    /// Offset of `message` (a `string[96]`).
    pub const MESSAGE_OFFSET: u32 = 40;

    /// Capacity of `message` in bytes.
    pub const MESSAGE_CAP: u32 = 96;

    /// Offset of `line`.
    pub const LINE_OFFSET: u32 = 144;

    /// Offset of `column`.
    pub const COLUMN_OFFSET: u32 = 152;

    /// Offset of `handle`.
    pub const HANDLE_OFFSET: u32 = 160;

    /// Offset of `offset`.
    pub const OFFSET_OFFSET: u32 = 168;

    /// Offset of `length`.
    pub const LENGTH_OFFSET: u32 = 176;

    /// Offset of `task_id`.
    pub const TASK_ID_OFFSET: u32 = 184;

    /// Clears a diagnostic: empty text, zero line/column/task id and `-1` for
    /// the "not involved" fields, as spec 0003 documents.
    ///
    /// Does nothing when `dst` is shorter than [`SIZE`], so a corrupt reference
    /// can never panic the kernel.
    pub fn clear(dst: &mut [u8]) {
        let Some(dst) = dst.get_mut(..SIZE as usize) else {
            return;
        };
        dst.fill(0);
        write_i64(dst, HANDLE_OFFSET, -1);
        write_i64(dst, OFFSET_OFFSET, -1);
        write_i64(dst, LENGTH_OFFSET, -1);
    }

    /// Fills a diagnostic with a compile/load error.
    ///
    /// `unit` and `message` are truncated to their field capacities. The
    /// `handle`, `offset` and `length` fields are set to `-1`; `task_id` is
    /// zero. An empty `unit` is recorded as an empty `unit_name`.
    pub fn write(dst: &mut [u8], unit: &[u8], message: &[u8], line: i64, column: i64) {
        clear(dst);
        let Some(dst) = dst.get_mut(..SIZE as usize) else {
            return;
        };
        write_text(dst, UNIT_NAME_OFFSET, UNIT_NAME_CAP, unit);
        write_text(dst, MESSAGE_OFFSET, MESSAGE_CAP, message);
        write_i64(dst, LINE_OFFSET, line);
        write_i64(dst, COLUMN_OFFSET, column);
    }
}

/// `ExitReason`: the staged record form of the exit reason.
pub mod exit_reason {
    use super::write_i64;
    use crate::ExitReason;

    /// Record size in bytes.
    pub const SIZE: u32 = 16;

    /// Record alignment in bytes.
    pub const ALIGN: u32 = 8;

    /// Offset of `kind` (see [`super::exit_kind`]).
    pub const KIND_OFFSET: u32 = 0;

    /// Offset of `code` (a `FaultCode` ordinal, zero unless faulted).
    pub const CODE_OFFSET: u32 = 8;

    /// Writes `kind` and `code` into a record-sized slice.
    ///
    /// A slice shorter than [`SIZE`] is left untouched.
    pub fn write(dst: &mut [u8], reason: ExitReason) {
        let Some(dst) = dst.get_mut(..SIZE as usize) else {
            return;
        };
        let (kind, code) = match reason {
            ExitReason::Ok => (super::exit_kind::OK, 0),
            ExitReason::Killed => (super::exit_kind::KILLED, 0),
            ExitReason::Faulted(code) => (super::exit_kind::FAULTED, code as i64),
        };
        write_i64(dst, KIND_OFFSET, kind);
        write_i64(dst, CODE_OFFSET, code);
    }
}

/// `ExitReason.kind` values.
pub mod exit_kind {
    /// `main` returned normally.
    pub const OK: i64 = 0;
    /// The kernel or REPL ended the task.
    pub const KILLED: i64 = 1;
    /// The task faulted; `code` holds the `FaultCode` ordinal.
    pub const FAULTED: i64 = 2;
}

/// Writes one `int` field at a fixed offset; a short slice is a no-op.
fn write_i64(dst: &mut [u8], offset: u32, value: i64) {
    let start = offset as usize;
    let Some(slot) = dst.get_mut(start..start + 8) else {
        return;
    };
    slot.copy_from_slice(&value.to_le_bytes());
}

/// Writes a `[len: u64][data: cap]` buffer field at a fixed offset.
///
/// The copied length is `min(text.len(), cap)`, the rest of the data area is
/// zeroed, and the length word holds the copied length. A short slice is a
/// no-op.
fn write_text(dst: &mut [u8], offset: u32, cap: u32, text: &[u8]) {
    let start = offset as usize;
    let cap = cap as usize;
    let Some(field) = dst.get_mut(start..start + 8 + cap) else {
        return;
    };
    let copied = text.len().min(cap);
    field[..8].copy_from_slice(&(copied as u64).to_le_bytes());
    field[8..8 + copied].copy_from_slice(&text[..copied]);
    field[8 + copied..].fill(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExitReason, FaultCode};

    #[test]
    fn diagnostic_offsets_match_the_spec() {
        assert_eq!(diagnostic::UNIT_NAME_OFFSET, 0);
        assert_eq!(diagnostic::MESSAGE_OFFSET, 40);
        assert_eq!(diagnostic::LINE_OFFSET, 144);
        assert_eq!(diagnostic::COLUMN_OFFSET, 152);
        assert_eq!(diagnostic::HANDLE_OFFSET, 160);
        assert_eq!(diagnostic::OFFSET_OFFSET, 168);
        assert_eq!(diagnostic::LENGTH_OFFSET, 176);
        assert_eq!(diagnostic::TASK_ID_OFFSET, 184);
        assert_eq!(diagnostic::SIZE, 192);
        assert_eq!(exit_reason::KIND_OFFSET, 0);
        assert_eq!(exit_reason::CODE_OFFSET, 8);
        assert_eq!(exit_reason::SIZE, 16);
    }

    #[test]
    fn diagnostic_write_truncates_and_clears() {
        let mut bytes = [0xAAu8; diagnostic::SIZE as usize];
        let message = [b'x'; 200];
        diagnostic::write(&mut bytes, b"shell", &message, 3, 7);
        assert_eq!(u64::from_le_bytes(bytes[0..8].try_into().unwrap()), 5);
        assert_eq!(&bytes[8..13], b"shell");
        assert_eq!(
            u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
            diagnostic::MESSAGE_CAP as u64
        );
        assert_eq!(
            bytes[48..48 + diagnostic::MESSAGE_CAP as usize],
            message[..diagnostic::MESSAGE_CAP as usize]
        );
        assert_eq!(i64::from_le_bytes(bytes[144..152].try_into().unwrap()), 3);
        assert_eq!(i64::from_le_bytes(bytes[160..168].try_into().unwrap()), -1);

        diagnostic::clear(&mut bytes);
        assert_eq!(bytes[0..8], [0u8; 8]);
        assert_eq!(i64::from_le_bytes(bytes[184..192].try_into().unwrap()), 0);
    }

    #[test]
    fn exit_reason_maps_the_staged_kinds() {
        let mut bytes = [0u8; exit_reason::SIZE as usize];
        exit_reason::write(&mut bytes, ExitReason::Ok);
        assert_eq!(bytes, [0u8; 16]);
        exit_reason::write(&mut bytes, ExitReason::Killed);
        assert_eq!(i64::from_le_bytes(bytes[0..8].try_into().unwrap()), 1);
        assert_eq!(i64::from_le_bytes(bytes[8..16].try_into().unwrap()), 0);
        exit_reason::write(&mut bytes, ExitReason::Faulted(FaultCode::Overflow));
        assert_eq!(i64::from_le_bytes(bytes[0..8].try_into().unwrap()), 2);
        assert_eq!(
            i64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            FaultCode::Overflow as i64
        );
    }
}

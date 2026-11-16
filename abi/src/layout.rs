//! Fixed record layouts shared by the compiler, the interpreter and the kernel.
//!
//! Both are predeclared before user code. `Diagnostic` is a record; the kernel
//! writes its fields directly. `ExitReason` is the language's first sum type
//! (`Ok | Killed | Faulted(code: FaultCode)`), and the kernel writes its tag
//! and payload directly.
//!
//! `Diagnostic` follows the language layout rule (spec 0003, "Records"): fields
//! in declaration order, each aligned to 8 bytes, a buffer field occupying
//! `[len: u64][data: cap]`, and the record size rounded to 8.
//!
//! `ExitReason` follows the sum layout rule (spec 0003, "Sum types"): an 8-byte
//! tag at offset 0 and the payload area at offset 8. The tag is `0` Ok, `1`
//! Killed, `2` Faulted; `Faulted`'s payload is the `FaultCode` ordinal.

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

/// `Event`: the predeclared input record (spec 0010, I19; routing in spec
/// 0011, W16).
///
/// Layout follows the language record rule: fields in declaration order, each
/// aligned to 8 bytes. The compiler's predeclared `Event` type uses these
/// offsets, and the kernel writes the same bytes directly. Producers leave
/// `window` at 0; the kernel host fills it when the event is polled.
pub mod event {
    use super::write_i64;
    use crate::{Event, EventKind};

    /// Record size in bytes.
    pub const SIZE: u32 = 40;

    /// Record alignment in bytes.
    pub const ALIGN: u32 = 8;

    /// Offset of `kind`.
    pub const KIND_OFFSET: u32 = 0;

    /// Offset of `x`.
    pub const X_OFFSET: u32 = 8;

    /// Offset of `y`.
    pub const Y_OFFSET: u32 = 16;

    /// Offset of `code`.
    pub const CODE_OFFSET: u32 = 24;

    /// Offset of `window`.
    pub const WINDOW_OFFSET: u32 = 32;

    /// Writes an event into a record-sized slice. A short slice is untouched.
    pub fn write(dst: &mut [u8], event: Event) {
        let Some(dst) = dst.get_mut(..SIZE as usize) else {
            return;
        };
        write_i64(dst, KIND_OFFSET, event.kind as i64);
        write_i64(dst, X_OFFSET, event.x);
        write_i64(dst, Y_OFFSET, event.y);
        write_i64(dst, CODE_OFFSET, event.code as i64);
        write_i64(dst, WINDOW_OFFSET, event.window);
    }

    /// Reads an event from a record-sized slice. Returns `None` for a short
    /// slice or an unknown kind ordinal, so a corrupt injection is refused
    /// rather than interpreted.
    pub fn read(src: &[u8]) -> Option<Event> {
        let src = src.get(..SIZE as usize)?;
        let kind = EventKind::from_ordinal(i64::from_le_bytes(src[0..8].try_into().ok()?) as u64)?;
        Some(Event {
            kind,
            x: i64::from_le_bytes(src[8..16].try_into().ok()?),
            y: i64::from_le_bytes(src[16..24].try_into().ok()?),
            code: u64::from_le_bytes(src[24..32].try_into().ok()?),
            window: i64::from_le_bytes(src[32..40].try_into().ok()?),
        })
    }
}

/// `Rect`: the predeclared window-rectangle record (spec 0011, W3).
///
/// Four `int` fields in declaration order, each 8-byte aligned; the size is 32
/// bytes. The compiler's predeclared `Rect` type and the window service write
/// exactly this image.
pub mod rect {
    use super::write_i64;
    use crate::Rect;

    /// Record size in bytes.
    pub const SIZE: u32 = 32;

    /// Record alignment in bytes.
    pub const ALIGN: u32 = 8;

    /// Offset of `x`.
    pub const X_OFFSET: u32 = 0;

    /// Offset of `y`.
    pub const Y_OFFSET: u32 = 8;

    /// Offset of `w`.
    pub const W_OFFSET: u32 = 16;

    /// Offset of `h`.
    pub const H_OFFSET: u32 = 24;

    /// Writes a rectangle into a record-sized slice. A short slice is untouched.
    pub fn write(dst: &mut [u8], rect: Rect) {
        let Some(dst) = dst.get_mut(..SIZE as usize) else {
            return;
        };
        write_i64(dst, X_OFFSET, rect.x);
        write_i64(dst, Y_OFFSET, rect.y);
        write_i64(dst, W_OFFSET, rect.w);
        write_i64(dst, H_OFFSET, rect.h);
    }

    /// Reads a rectangle from a record-sized slice. Returns `None` for a short
    /// slice.
    pub fn read(src: &[u8]) -> Option<Rect> {
        let src = src.get(..SIZE as usize)?;
        Some(Rect {
            x: i64::from_le_bytes(src[0..8].try_into().ok()?),
            y: i64::from_le_bytes(src[8..16].try_into().ok()?),
            w: i64::from_le_bytes(src[16..24].try_into().ok()?),
            h: i64::from_le_bytes(src[24..32].try_into().ok()?),
        })
    }
}

/// `ExitReason`: the predeclared sum type `Ok | Killed | Faulted(code)`.
pub mod exit_reason {
    use super::write_i64;
    use crate::ExitReason;

    /// Size in bytes: an 8-byte tag plus the largest payload (`Faulted`'s
    /// `FaultCode`, one 8-byte slot).
    pub const SIZE: u32 = 16;

    /// Alignment in bytes.
    pub const ALIGN: u32 = 8;

    /// Offset of the variant tag (see [`super::exit_tag`]).
    pub const TAG_OFFSET: u32 = 0;

    /// Offset of the payload area. `Faulted` stores the `FaultCode` ordinal
    /// here; the other variants have no payload.
    pub const PAYLOAD_OFFSET: u32 = 8;

    /// Writes the tag and payload into a sum-sized slice.
    ///
    /// A slice shorter than [`SIZE`] is left untouched.
    pub fn write(dst: &mut [u8], reason: ExitReason) {
        let Some(dst) = dst.get_mut(..SIZE as usize) else {
            return;
        };
        let (tag, payload) = match reason {
            ExitReason::Ok => (super::exit_tag::OK, 0),
            ExitReason::Killed => (super::exit_tag::KILLED, 0),
            ExitReason::Faulted(code) => (super::exit_tag::FAULTED, code as i64),
        };
        write_i64(dst, TAG_OFFSET, tag);
        write_i64(dst, PAYLOAD_OFFSET, payload);
    }
}

/// `ExitReason` variant tags.
pub mod exit_tag {
    /// `main` returned normally.
    pub const OK: i64 = 0;
    /// The kernel or REPL ended the task.
    pub const KILLED: i64 = 1;
    /// The task faulted; the payload holds the `FaultCode` ordinal.
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
    use crate::key;
    use crate::{Event, EventKind, ExitReason, FaultCode};

    #[test]
    fn rect_offsets_and_round_trip_match_the_spec() {
        assert_eq!(rect::X_OFFSET, 0);
        assert_eq!(rect::Y_OFFSET, 8);
        assert_eq!(rect::W_OFFSET, 16);
        assert_eq!(rect::H_OFFSET, 24);
        assert_eq!(rect::SIZE, 32);

        let value = crate::Rect {
            x: -3,
            y: 4,
            w: 200,
            h: 120,
        };
        let mut bytes = [0xAAu8; rect::SIZE as usize];
        rect::write(&mut bytes, value);
        assert_eq!(rect::read(&bytes), Some(value));
        assert_eq!(rect::read(&bytes[..24]), None);
    }

    #[test]
    fn event_offsets_match_the_spec() {
        assert_eq!(event::KIND_OFFSET, 0);
        assert_eq!(event::X_OFFSET, 8);
        assert_eq!(event::Y_OFFSET, 16);
        assert_eq!(event::CODE_OFFSET, 24);
        assert_eq!(event::WINDOW_OFFSET, 32);
        assert_eq!(event::SIZE, 40);
    }

    #[test]
    fn event_round_trips_and_rejects_unknown_kinds() {
        let event = Event {
            kind: EventKind::PointerDown,
            x: -4,
            y: 768,
            code: 0b101,
            window: 2,
        };
        let mut bytes = [0u8; event::SIZE as usize];
        event::write(&mut bytes, event);
        assert_eq!(event::read(&bytes), Some(event));

        bytes[0..8].copy_from_slice(&9i64.to_le_bytes());
        assert_eq!(event::read(&bytes), None);
        assert_eq!(event::read(&bytes[..16]), None);
    }

    #[test]
    fn key_codes_pack_text_modifiers_and_identity() {
        let code = key::pack(key::LEFT, 0, key::MOD_SHIFT | key::MOD_CTRL);
        assert_eq!(key::text(code), 0);
        assert_eq!(key::modifiers(code), key::MOD_SHIFT | key::MOD_CTRL);
        assert_eq!(key::code(code), key::LEFT);

        let code = key::pack(u16::from(b'A'), b'A', 0);
        assert_eq!(key::text(code), b'A');
        assert_eq!(key::modifiers(code), 0);
        assert_eq!(key::code(code), u16::from(b'A'));
    }

    #[test]
    fn event_kind_ordinals_are_stable() {
        assert_eq!(EventKind::PointerMove as u8, 0);
        assert_eq!(EventKind::PointerDown as u8, 1);
        assert_eq!(EventKind::PointerUp as u8, 2);
        assert_eq!(EventKind::KeyDown as u8, 3);
        assert_eq!(EventKind::KeyUp as u8, 4);
        assert_eq!(EventKind::Expose as u8, 5);
        assert_eq!(EventKind::from_ordinal(5), Some(EventKind::Expose));
        assert_eq!(EventKind::from_ordinal(6), None);
        assert_eq!(EventKind::from_ordinal(4), Some(EventKind::KeyUp));
    }

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
        assert_eq!(exit_reason::TAG_OFFSET, 0);
        assert_eq!(exit_reason::PAYLOAD_OFFSET, 8);
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
    fn exit_reason_maps_the_sum_variants() {
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

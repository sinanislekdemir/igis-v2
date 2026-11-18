//! Resolved value types.
//!
//! Capacities are part of the type (`string[64]` and `string[128]` are
//! distinct), so a [`Type`] is just a kind and a capacity. Scalars ignore the
//! capacity field.

use core::fmt;

/// Maximum capacity accepted for a buffer, in bytes. This bounds both `state`
/// and frame sizes; the first-milestone tuning cap is 1 MiB per value.
pub const MAX_CAPACITY: u32 = 1 << 20;

/// Enum id of the predeclared `Status` type (spec 0003, "Errors").
///
/// Enum ids are assigned in declaration order with `Status` first, so the
/// predeclared ids are identical in every unit of a package (spec 0008,
/// "Types"; ADR 0039). A user id never reaches the artifact: an enum value is
/// its ordinal in one 8-byte slot, and casts carry the variant count as an
/// immediate.
pub const STATUS_ENUM: u16 = 0;

/// Value kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Signed 64-bit integer.
    Int,
    /// Unsigned 64-bit integer.
    Uint,
    /// Unsigned 8-bit integer.
    Byte,
    /// Boolean.
    Bool,
    /// IEEE-754 binary64.
    Float,
    /// UTF-8 text with a fixed capacity.
    String,
    /// Raw bytes with a fixed capacity.
    Bytes,
    /// A nominal enum type, by compiler-local enum id. The value is the
    /// variant ordinal in one 8-byte slot (spec 0003). The predeclared
    /// `Status` and `FaultCode` occupy fixed ids in every unit.
    Enum(u16),
    /// A nominal record type, by compiler-local record id. The layout (field
    /// offsets and size) lives in the compiler's record table; [`Type::cap`]
    /// carries the size in bytes so frame and state layout need no extra
    /// table lookup. The predeclared `Diagnostic`, `Event` and `Rect` occupy
    /// fixed ids in every unit.
    Record(u16),
    /// A nominal sum type, by compiler-local sum id. The layout (variant
    /// payloads and size) lives in the compiler's sum table; [`Type::cap`]
    /// carries the size in bytes. The predeclared `ExitReason` occupies the
    /// first id in every unit.
    Sum(u16),
    /// A fixed-size array, by compiler-local array id. The element type and
    /// count live in the compiler's array table; [`Type::cap`] carries the
    /// aligned size in bytes (spec 0003, "Arrays").
    Array(u16),
    /// A fixed-capacity FIFO, by compiler-local ring id. The element type and
    /// count live in the compiler's ring table; [`Type::cap`] carries the
    /// aligned size in bytes, header included (spec 0003, "Rings").
    Ring(u16),
    /// A fixed-capacity open-addressing map, by compiler-local map id. The key
    /// type, value type, entry count and table capacity live in the compiler's
    /// map table; [`Type::cap`] carries the total size in bytes, header
    /// included (spec 0003, "Maps").
    Map(u16),
    /// A receive stream endpoint (`in_stream[N]`). The value is an opaque
    /// 8-byte kernel handle; [`Type::cap`] carries the queue capacity in bytes
    /// (spec 0006).
    InStream,
    /// A send stream endpoint (`out_stream[N]`). Distinct from
    /// [`Kind::InStream`], so sending on a receive endpoint is a compile error.
    OutStream,
}

impl Kind {
    /// Stable name used in diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Int => "int",
            Kind::Uint => "uint",
            Kind::Byte => "byte",
            Kind::Bool => "bool",
            Kind::Float => "f64",
            Kind::String => "string",
            Kind::Bytes => "bytes",
            Kind::Enum(_) => "enum",
            Kind::Record(_) => "record",
            Kind::Sum(_) => "sum",
            Kind::Array(_) => "array",
            Kind::Ring(_) => "ring",
            Kind::Map(_) => "map",
            Kind::InStream => "in_stream",
            Kind::OutStream => "out_stream",
        }
    }

    /// Returns true for `string` and `bytes`.
    pub const fn is_buffer(self) -> bool {
        matches!(self, Kind::String | Kind::Bytes)
    }

    /// Returns true for a nominal record type.
    pub const fn is_record(self) -> bool {
        matches!(self, Kind::Record(_))
    }

    /// Returns true for a nominal sum type.
    pub const fn is_sum(self) -> bool {
        matches!(self, Kind::Sum(_))
    }

    /// Returns true for a fixed-size array type.
    pub const fn is_array(self) -> bool {
        matches!(self, Kind::Array(_))
    }

    /// Returns true for a fixed-capacity ring type.
    pub const fn is_ring(self) -> bool {
        matches!(self, Kind::Ring(_))
    }

    /// Returns true for a fixed-capacity map type.
    pub const fn is_map(self) -> bool {
        matches!(self, Kind::Map(_))
    }

    /// Returns true for a stream endpoint type.
    pub const fn is_stream(self) -> bool {
        matches!(self, Kind::InStream | Kind::OutStream)
    }
}

/// A resolved type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Type {
    /// Value kind.
    pub kind: Kind,
    /// Buffer capacity in bytes; zero for scalars.
    pub cap: u32,
}

impl Type {
    /// A scalar type with no capacity.
    pub const fn scalar(kind: Kind) -> Self {
        Type { kind, cap: 0 }
    }

    /// An enum type by compiler-local id.
    pub const fn enumeration(id: u16) -> Self {
        Type {
            kind: Kind::Enum(id),
            cap: 0,
        }
    }

    /// A buffer type with a capacity.
    pub const fn buffer(kind: Kind, cap: u32) -> Self {
        Type { kind, cap }
    }

    /// A nominal record type, with its layout size in bytes.
    pub const fn record(id: u16, size: u32) -> Self {
        Type {
            kind: Kind::Record(id),
            cap: size,
        }
    }

    /// A nominal sum type, with its layout size in bytes.
    pub const fn sum(id: u16, size: u32) -> Self {
        Type {
            kind: Kind::Sum(id),
            cap: size,
        }
    }

    /// A fixed-size array, with its aligned size in bytes.
    pub const fn array(id: u16, size: u32) -> Self {
        Type {
            kind: Kind::Array(id),
            cap: size,
        }
    }

    /// A fixed-capacity ring, with its aligned size in bytes (header included).
    pub const fn ring(id: u16, size: u32) -> Self {
        Type {
            kind: Kind::Ring(id),
            cap: size,
        }
    }

    /// A fixed-capacity map, with its total size in bytes (header included).
    pub const fn map(id: u16, size: u32) -> Self {
        Type {
            kind: Kind::Map(id),
            cap: size,
        }
    }

    /// A stream endpoint of `kind` (`InStream` or `OutStream`) with a queue
    /// capacity in bytes.
    pub const fn stream(kind: Kind, capacity: u32) -> Self {
        Type {
            kind,
            cap: capacity,
        }
    }

    /// True for `string`/`bytes`.
    pub const fn is_buffer(self) -> bool {
        self.kind.is_buffer()
    }

    /// True for a nominal record.
    pub const fn is_record(self) -> bool {
        self.kind.is_record()
    }

    /// True for a nominal sum type.
    pub const fn is_sum(self) -> bool {
        self.kind.is_sum()
    }

    /// True for a fixed-size array.
    pub const fn is_array(self) -> bool {
        self.kind.is_array()
    }

    /// True for a fixed-capacity ring.
    pub const fn is_ring(self) -> bool {
        self.kind.is_ring()
    }

    /// True for a fixed-capacity map.
    pub const fn is_map(self) -> bool {
        self.kind.is_map()
    }

    /// True for a stream endpoint.
    pub const fn is_stream(self) -> bool {
        self.kind.is_stream()
    }

    /// True for every aggregate value: a record, sum, array, ring or map.
    pub const fn is_aggregate(self) -> bool {
        self.kind.is_record()
            || self.kind.is_sum()
            || self.kind.is_array()
            || self.kind.is_ring()
            || self.kind.is_map()
    }

    /// True for every scalar kind (not a buffer, record, sum, array, ring,
    /// map or stream endpoint).
    pub const fn is_scalar(self) -> bool {
        !self.kind.is_buffer() && !self.is_aggregate() && !self.kind.is_stream()
    }

    /// Number of bytes this type occupies in `state` or in a frame, including
    /// the buffer length word and 8-byte alignment.
    pub const fn size(self) -> u32 {
        match self.kind {
            Kind::String | Kind::Bytes => align8(8 + self.cap),
            // A record's, sum's, array's, ring's or map's `cap` already holds
            // its aligned size.
            Kind::Record(_) | Kind::Sum(_) | Kind::Array(_) | Kind::Ring(_) | Kind::Map(_) => {
                self.cap
            }
            // A stream endpoint is one opaque 8-byte handle.
            _ => 8,
        }
    }

    /// The opcode [`value_kind`](abi::opcodes::value_kind) tag, for scalars.
    pub fn value_kind(self) -> Option<u8> {
        use abi::opcodes::value_kind;
        match self.kind {
            Kind::Int => Some(value_kind::INT),
            Kind::Uint => Some(value_kind::UINT),
            Kind::Byte => Some(value_kind::BYTE),
            Kind::Float => Some(value_kind::F64),
            Kind::Bool
            | Kind::String
            | Kind::Bytes
            | Kind::Enum(_)
            | Kind::Record(_)
            | Kind::Sum(_)
            | Kind::Array(_)
            | Kind::Ring(_)
            | Kind::Map(_)
            | Kind::InStream
            | Kind::OutStream => None,
        }
    }
}

impl fmt::Display for Type {
    /// Renders the type the way source writes it. An enum has no name here
    /// because [`Type`] carries only its id; codegen renders diagnostics
    /// through its enum table.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Kind::String | Kind::Bytes => write!(f, "{}[{}]", self.kind.name(), self.cap),
            // A record, sum, array, ring or map renders as its kind; codegen
            // resolves the name or element type.
            Kind::Record(_) | Kind::Sum(_) | Kind::Array(_) | Kind::Ring(_) | Kind::Map(_) => {
                f.write_str(self.kind.name())
            }
            Kind::InStream | Kind::OutStream => write!(f, "{}[{}]", self.kind.name(), self.cap),
            _ => f.write_str(self.kind.name()),
        }
    }
}

/// Rounds `value` up to a multiple of 8.
pub const fn align8(value: u32) -> u32 {
    (value + 7) & !7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_aligned() {
        assert_eq!(Type::scalar(Kind::Int).size(), 8);
        assert_eq!(Type::buffer(Kind::String, 1).size(), 16);
        assert_eq!(Type::buffer(Kind::String, 8).size(), 16);
        assert_eq!(Type::buffer(Kind::Bytes, 9).size(), 24);
    }
}

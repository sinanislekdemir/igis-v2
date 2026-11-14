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
/// Enum ids are local to one compilation and assigned in declaration order,
/// with `Status` first. The id never reaches the artifact: an enum value is its
/// ordinal in one 8-byte slot, and casts carry the variant count as an
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
    /// variant ordinal in one 8-byte slot (spec 0003).
    Enum(u16),
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
        }
    }

    /// Returns true for `string` and `bytes`.
    pub const fn is_buffer(self) -> bool {
        matches!(self, Kind::String | Kind::Bytes)
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

    /// True for `string`/`bytes`.
    pub const fn is_buffer(self) -> bool {
        self.kind.is_buffer()
    }

    /// True for every scalar kind.
    pub const fn is_scalar(self) -> bool {
        !self.kind.is_buffer()
    }

    /// Number of bytes this type occupies in `state` or in a frame, including
    /// the buffer length word and 8-byte alignment.
    pub const fn size(self) -> u32 {
        match self.kind {
            Kind::String | Kind::Bytes => align8(8 + self.cap),
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
            Kind::Bool | Kind::String | Kind::Bytes | Kind::Enum(_) => None,
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

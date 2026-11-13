//! 64-bit value references.
//!
//! Every application-visible reference is a `(region: 2 bits, offset: 62 bits)`
//! pair (spec 0004, C3). The interpreter validates the region and the offset
//! against the task's own region table on every access; nothing carries a raw
//! address.
//!
//! The four regions are fixed by this ABI version:
//!
//! | id | region | access |
//! |----|--------|--------|
//! | 0 | `state` | read/write |
//! | 1 | `stack` | read/write |
//! | 2 | constant blob inside `code` | read-only |
//! | 3 | `args` buffer supplied by the loader | read-only |

/// Region id for the application's `state` region.
pub const REGION_STATE: u8 = 0;
/// Region id for the task's `stack` region.
pub const REGION_STACK: u8 = 1;
/// Region id for the constant blob inside the `code` region.
pub const REGION_CODE: u8 = 2;
/// Region id for the loader-provided `main(args)` buffer.
pub const REGION_ARGS: u8 = 3;

/// Number of low bits that carry the offset.
pub const OFFSET_BITS: u32 = 62;

/// Mask selecting the offset bits of an encoded reference.
pub const OFFSET_MASK: u64 = (1u64 << OFFSET_BITS) - 1;

/// A decoded reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    /// One of the `REGION_*` constants.
    pub region: u8,
    /// Byte offset inside the region.
    pub offset: u64,
}

/// Returns true when `offset` fits the 62-bit offset field.
pub const fn fits(offset: u64) -> bool {
    offset <= OFFSET_MASK
}

/// Encodes `(region, offset)` into one 64-bit word.
///
/// Regions are two bits, so only the low two bits of `region` are kept. Use
/// [`try_encode`] when the inputs are not already known to be valid.
pub const fn encode(region: u8, offset: u64) -> u64 {
    (((region as u64) & 0b11) << OFFSET_BITS) | (offset & OFFSET_MASK)
}

/// Encodes a reference, returning `None` when the offset does not fit.
pub const fn try_encode(region: u8, offset: u64) -> Option<u64> {
    if fits(offset) {
        Some(encode(region, offset))
    } else {
        None
    }
}

/// Decodes a 64-bit reference word.
pub const fn decode(bits: u64) -> Reference {
    Reference {
        region: ((bits >> OFFSET_BITS) & 0b11) as u8,
        offset: bits & OFFSET_MASK,
    }
}

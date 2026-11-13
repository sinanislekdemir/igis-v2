//! Shared ABI for the igis-v2 application language.
//!
//! This crate is the contract between `compiler`, `interpreter` and (later) the
//! kernel. It has no dependencies and is `#![no_std]`, so the same definitions
//! compile for the host and for `x86_64-unknown-none`.
//!
//! # Code blob layout (ABI v1)
//!
//! ```text
//! offset                                   size
//! 0                                        CodeHeader (40 bytes)
//! 40                                       24 bytes per Function
//! align8(40 + 24 * function_count)         header.code_len bytes of instructions
//! align8(previous + code_len)              header.const_len bytes of constants
//! align8(previous + const_len)             header.line_len bytes of line table
//! ```
//!
//! Every section starts on an 8-byte boundary and the pad bytes are zero. The
//! code blob ends exactly at the end of the line table. Section offsets are
//! derived from the header, so no absolute address is stored anywhere.
//!
//! The constant blob holds buffer images:
//!
//! ```text
//! [len: u64 little-endian][data: len bytes][zero padding to 8]
//! ```
//!
//! A [`refs`] reference into the constant blob points at `data`, which is why
//! [`REF_CONST`](opcodes::REF_CONST) offsets are always 8-byte aligned and at
//! least 8. The header of a buffer is therefore always at `offset - 8`, in
//! every region (spec 0008, "Values in the VM").
//!
//! The line table has 16-byte entries, sorted by code offset:
//!
//! ```text
//! [code_off: u32][unit: u32][line: u32][column: u32]
//! ```
//!
//! `line_len` is a byte length, like `code_len` and `const_len`. ABI v1 has
//! exactly one unit per package (`unit == 0`).

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod opcodes;
pub mod refs;

/// ABI version carried by every code blob and manifest.
pub const ABI_VERSION: u16 = 1;

/// Target profile: x86_64, little-endian, 8-byte words, SSE2 doubles.
pub const PROFILE_X86_64_LE: u16 = 1;

/// Magic at the start of a code blob.
pub const CODE_MAGIC: [u8; 4] = *b"IGC1";

/// Magic at the start of a manifest.
pub const MANIFEST_MAGIC: [u8; 4] = *b"IGM1";

/// Size of [`CodeHeader`] in bytes.
pub const CODE_HEADER_SIZE: usize = 40;

/// Size of [`Function`] in bytes.
pub const FUNCTION_SIZE: usize = 24;

/// Size of [`ManifestHeader`] in bytes.
pub const MANIFEST_HEADER_SIZE: usize = 32;

/// Size of [`Endpoint`] in bytes.
pub const ENDPOINT_SIZE: usize = 8;

/// Size of one line-table entry in bytes.
pub const LINE_ENTRY_SIZE: usize = 16;

/// Value of `CodeHeader::diag_offset` when no hidden diagnostic slot is
/// reserved. ABI v1 has no `fault`/`exit` handlers, so the compiler always
/// emits this value.
pub const NO_DIAGNOSTIC: u32 = u32::MAX;

/// Value of `Function::name_off` when the function name is not stored in the
/// constant blob. ABI v1 keeps names compile-time only.
pub const NO_NAME: u32 = u32::MAX;

/// Non-fatal operation result (spec 0003, "Errors").
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    /// Success.
    Ok = 0,
    /// The operation would block; retry later.
    WouldBlock = 1,
    /// The peer endpoint is closed.
    Closed = 2,
    /// The named thing does not exist.
    NotFound = 3,
    /// The input is malformed.
    Invalid = 4,
    /// A fixed-capacity resource is full.
    Full = 5,
    /// The result does not fit its declared capacity.
    CapacityExceeded = 6,
    /// A device or loader failure.
    IoError = 7,
}

impl Status {
    /// Stable assembler-style name, used by diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            Status::Ok => "Ok",
            Status::WouldBlock => "WouldBlock",
            Status::Closed => "Closed",
            Status::NotFound => "NotFound",
            Status::Invalid => "Invalid",
            Status::Full => "Full",
            Status::CapacityExceeded => "CapacityExceeded",
            Status::IoError => "IoError",
        }
    }
}

/// Fatal interpreter fault (spec 0003, "Fault codes").
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaultCode {
    /// Unclassified interpreter fault.
    GenericFault = 0,
    /// Access outside an owned region, or a bad reference.
    OutOfBounds = 1,
    /// A buffer or collection operation would exceed its capacity.
    CapacityExceeded = 2,
    /// Integer arithmetic overflow, or a float that does not fit an integer.
    Overflow = 3,
    /// Integer division or remainder by zero.
    DivideByZero = 4,
    /// The frame would not fit in the stack region.
    StackOverflow = 5,
    /// The watchdog budget was exceeded.
    BudgetExceeded = 6,
    /// A realtime heartbeat was missed.
    WatchdogTimeout = 7,
    /// Invalid opcode, invalid operand, or an illegal operation.
    IllegalOperation = 8,
    /// Invalid or closed stream endpoint.
    StreamError = 9,
    /// Loader or device failure.
    IoError = 10,
    /// Terminal reason recorded when `exit` itself failed.
    ExitFault = 11,
}

impl FaultCode {
    /// Stable assembler-style name, used by diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            FaultCode::GenericFault => "GenericFault",
            FaultCode::OutOfBounds => "OutOfBounds",
            FaultCode::CapacityExceeded => "CapacityExceeded",
            FaultCode::Overflow => "Overflow",
            FaultCode::DivideByZero => "DivideByZero",
            FaultCode::StackOverflow => "StackOverflow",
            FaultCode::BudgetExceeded => "BudgetExceeded",
            FaultCode::WatchdogTimeout => "WatchdogTimeout",
            FaultCode::IllegalOperation => "IllegalOperation",
            FaultCode::StreamError => "StreamError",
            FaultCode::IoError => "IoError",
            FaultCode::ExitFault => "ExitFault",
        }
    }
}

/// Why a task stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// `main` returned normally.
    Ok,
    /// The kernel or REPL ended the task.
    Killed,
    /// The task faulted.
    Faulted(FaultCode),
}

/// Header of the code blob.
///
/// The struct is `#[repr(C)]` with no padding; the byte layout is fixed by
/// [`CodeHeader::to_bytes`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeHeader {
    /// Always [`CODE_MAGIC`].
    pub magic: [u8; 4],
    /// Always [`ABI_VERSION`].
    pub abi_version: u16,
    /// Always [`PROFILE_X86_64_LE`] for this interpreter.
    pub profile: u16,
    /// Number of [`Function`] descriptors.
    pub function_count: u32,
    /// Function index of the entry point (`main`).
    pub entry: u32,
    /// Instruction section length in bytes.
    pub code_len: u32,
    /// Constant section length in bytes.
    pub const_len: u32,
    /// Line table length in bytes (a multiple of [`LINE_ENTRY_SIZE`]).
    pub line_len: u32,
    /// Size of the `state` region in bytes.
    pub state_size: u32,
    /// Size of the `stack` region in bytes.
    pub stack_size: u32,
    /// State offset of the hidden `Diagnostic` slot, or [`NO_DIAGNOSTIC`].
    pub diag_offset: u32,
}

impl CodeHeader {
    /// Encodes the header in little-endian order.
    pub fn to_bytes(&self) -> [u8; CODE_HEADER_SIZE] {
        let mut out = [0u8; CODE_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.abi_version.to_le_bytes());
        out[6..8].copy_from_slice(&self.profile.to_le_bytes());
        out[8..12].copy_from_slice(&self.function_count.to_le_bytes());
        out[12..16].copy_from_slice(&self.entry.to_le_bytes());
        out[16..20].copy_from_slice(&self.code_len.to_le_bytes());
        out[20..24].copy_from_slice(&self.const_len.to_le_bytes());
        out[24..28].copy_from_slice(&self.line_len.to_le_bytes());
        out[28..32].copy_from_slice(&self.state_size.to_le_bytes());
        out[32..36].copy_from_slice(&self.stack_size.to_le_bytes());
        out[36..40].copy_from_slice(&self.diag_offset.to_le_bytes());
        out
    }

    /// Decodes a header. Returns `None` when `src` is not exactly
    /// [`CODE_HEADER_SIZE`] bytes.
    pub fn from_bytes(src: &[u8]) -> Option<Self> {
        if src.len() != CODE_HEADER_SIZE {
            return None;
        }
        Some(CodeHeader {
            magic: [src[0], src[1], src[2], src[3]],
            abi_version: u16::from_le_bytes([src[4], src[5]]),
            profile: u16::from_le_bytes([src[6], src[7]]),
            function_count: u32::from_le_bytes([src[8], src[9], src[10], src[11]]),
            entry: u32::from_le_bytes([src[12], src[13], src[14], src[15]]),
            code_len: u32::from_le_bytes([src[16], src[17], src[18], src[19]]),
            const_len: u32::from_le_bytes([src[20], src[21], src[22], src[23]]),
            line_len: u32::from_le_bytes([src[24], src[25], src[26], src[27]]),
            state_size: u32::from_le_bytes([src[28], src[29], src[30], src[31]]),
            stack_size: u32::from_le_bytes([src[32], src[33], src[34], src[35]]),
            diag_offset: u32::from_le_bytes([src[36], src[37], src[38], src[39]]),
        })
    }
}

/// One function descriptor.
///
/// `code_off` is relative to the start of the instruction section.
/// `operand_off` is the distance from the frame base to the operand stack; the
/// locals area occupies `32..operand_off` (see the interpreter's frame layout).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    /// Byte offset of the first instruction in the instruction section.
    pub code_off: u32,
    /// Total frame size in bytes, including the 32-byte frame header.
    pub frame_size: u32,
    /// Distance from the frame base to the operand stack, in bytes.
    pub operand_off: u32,
    /// Index of this function's first line-table entry.
    pub line_base: u32,
    /// Offset of the function name in the constant blob, or [`NO_NAME`].
    pub name_off: u32,
    /// Number of parameters; each occupies the first 8 bytes of the locals
    /// area, in declaration order.
    pub param_count: u8,
    /// True when the callee writes one result through a caller-provided
    /// reference.
    pub has_destination: bool,
    /// Reserved; must be zero.
    pub _reserved: [u8; 2],
}

impl Function {
    /// Encodes the descriptor in little-endian order.
    pub fn to_bytes(&self) -> [u8; FUNCTION_SIZE] {
        let mut out = [0u8; FUNCTION_SIZE];
        out[0..4].copy_from_slice(&self.code_off.to_le_bytes());
        out[4..8].copy_from_slice(&self.frame_size.to_le_bytes());
        out[8..12].copy_from_slice(&self.operand_off.to_le_bytes());
        out[12..16].copy_from_slice(&self.line_base.to_le_bytes());
        out[16..20].copy_from_slice(&self.name_off.to_le_bytes());
        out[20] = self.param_count;
        out[21] = self.has_destination as u8;
        out
    }

    /// Decodes a descriptor. Returns `None` when `src` has the wrong length or
    /// `has_destination` is not a valid boolean.
    pub fn from_bytes(src: &[u8]) -> Option<Self> {
        if src.len() != FUNCTION_SIZE {
            return None;
        }
        let has_destination = match src[21] {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Function {
            code_off: u32::from_le_bytes([src[0], src[1], src[2], src[3]]),
            frame_size: u32::from_le_bytes([src[4], src[5], src[6], src[7]]),
            operand_off: u32::from_le_bytes([src[8], src[9], src[10], src[11]]),
            line_base: u32::from_le_bytes([src[12], src[13], src[14], src[15]]),
            name_off: u32::from_le_bytes([src[16], src[17], src[18], src[19]]),
            param_count: src[20],
            has_destination,
            _reserved: [src[22], src[23]],
        })
    }
}

/// Stream endpoint direction, used by [`Endpoint`].
pub mod direction {
    /// Receive endpoint (`in_stream`).
    pub const IN: u8 = 0;
    /// Send endpoint (`out_stream`).
    pub const OUT: u8 = 1;
}

/// One declared stream endpoint in the manifest. ABI v1 never declares any;
/// the type exists so the manifest does not need a version bump when streams
/// arrive.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// Length of the endpoint name in bytes.
    pub name_len: u16,
    /// One of [`direction::IN`] or [`direction::OUT`].
    pub direction: u8,
    /// Reserved; must be zero.
    pub _reserved: u8,
    /// Queue capacity in bytes.
    pub capacity: u32,
}

impl Endpoint {
    /// Encodes the descriptor in little-endian order.
    pub fn to_bytes(&self) -> [u8; ENDPOINT_SIZE] {
        let mut out = [0u8; ENDPOINT_SIZE];
        out[0..2].copy_from_slice(&self.name_len.to_le_bytes());
        out[2] = self.direction;
        out[3] = self._reserved;
        out[4..8].copy_from_slice(&self.capacity.to_le_bytes());
        out
    }

    /// Decodes a descriptor. Returns `None` on a wrong length or an unknown
    /// direction.
    pub fn from_bytes(src: &[u8]) -> Option<Self> {
        if src.len() != ENDPOINT_SIZE {
            return None;
        }
        if src[2] != direction::IN && src[2] != direction::OUT {
            return None;
        }
        Some(Endpoint {
            name_len: u16::from_le_bytes([src[0], src[1]]),
            direction: src[2],
            _reserved: src[3],
            capacity: u32::from_le_bytes([src[4], src[5], src[6], src[7]]),
        })
    }
}

/// Header of an application manifest.
///
/// Manifest layout: this header, then `unit_name_len` bytes of unit name, then
/// `endpoint_count` times (`Endpoint` descriptor followed by `name_len` bytes
/// of name).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestHeader {
    /// Always [`MANIFEST_MAGIC`].
    pub magic: [u8; 4],
    /// Always [`ABI_VERSION`].
    pub abi_version: u16,
    /// Target profile, as in [`CodeHeader::profile`].
    pub profile: u16,
    /// Length of the unit name that follows the header.
    pub unit_name_len: u16,
    /// Number of endpoint descriptors that follow the unit name.
    pub endpoint_count: u16,
    /// Byte length of the code blob that accompanies this manifest.
    pub code_len: u32,
    /// Size of the `state` region in bytes.
    pub state_size: u32,
    /// Size of the `stack` region in bytes.
    pub stack_size: u32,
    /// Function index of the entry point.
    pub entry: u32,
    /// Build-parameter profile identifier; zero for the default profile.
    pub capacity_profile: u32,
}

impl ManifestHeader {
    /// Encodes the header in little-endian order.
    pub fn to_bytes(&self) -> [u8; MANIFEST_HEADER_SIZE] {
        let mut out = [0u8; MANIFEST_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.abi_version.to_le_bytes());
        out[6..8].copy_from_slice(&self.profile.to_le_bytes());
        out[8..10].copy_from_slice(&self.unit_name_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.endpoint_count.to_le_bytes());
        out[12..16].copy_from_slice(&self.code_len.to_le_bytes());
        out[16..20].copy_from_slice(&self.state_size.to_le_bytes());
        out[20..24].copy_from_slice(&self.stack_size.to_le_bytes());
        out[24..28].copy_from_slice(&self.entry.to_le_bytes());
        out[28..32].copy_from_slice(&self.capacity_profile.to_le_bytes());
        out
    }

    /// Decodes a header. Returns `None` when `src` is not exactly
    /// [`MANIFEST_HEADER_SIZE`] bytes.
    pub fn from_bytes(src: &[u8]) -> Option<Self> {
        if src.len() != MANIFEST_HEADER_SIZE {
            return None;
        }
        Some(ManifestHeader {
            magic: [src[0], src[1], src[2], src[3]],
            abi_version: u16::from_le_bytes([src[4], src[5]]),
            profile: u16::from_le_bytes([src[6], src[7]]),
            unit_name_len: u16::from_le_bytes([src[8], src[9]]),
            endpoint_count: u16::from_le_bytes([src[10], src[11]]),
            code_len: u32::from_le_bytes([src[12], src[13], src[14], src[15]]),
            state_size: u32::from_le_bytes([src[16], src[17], src[18], src[19]]),
            stack_size: u32::from_le_bytes([src[20], src[21], src[22], src[23]]),
            entry: u32::from_le_bytes([src[24], src[25], src[26], src[27]]),
            capacity_profile: u32::from_le_bytes([src[28], src[29], src[30], src[31]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn header_layout_is_fixed() {
        assert_eq!(size_of::<CodeHeader>(), CODE_HEADER_SIZE);
        assert_eq!(size_of::<Function>(), FUNCTION_SIZE);
        assert_eq!(size_of::<ManifestHeader>(), MANIFEST_HEADER_SIZE);
        assert_eq!(size_of::<Endpoint>(), ENDPOINT_SIZE);
    }

    #[test]
    fn code_header_round_trips() {
        let header = CodeHeader {
            magic: CODE_MAGIC,
            abi_version: ABI_VERSION,
            profile: PROFILE_X86_64_LE,
            function_count: 3,
            entry: 1,
            code_len: 128,
            const_len: 64,
            line_len: 32,
            state_size: 256,
            stack_size: 4096,
            diag_offset: NO_DIAGNOSTIC,
        };
        let bytes = header.to_bytes();
        assert_eq!(CodeHeader::from_bytes(&bytes), Some(header));
        assert_eq!(bytes[0..4], CODE_MAGIC);
        assert_eq!(CodeHeader::from_bytes(&bytes[..39]), None);
    }

    #[test]
    fn function_round_trips() {
        let function = Function {
            code_off: 12,
            frame_size: 96,
            operand_off: 64,
            line_base: 2,
            name_off: NO_NAME,
            param_count: 3,
            has_destination: true,
            _reserved: [0, 0],
        };
        let bytes = function.to_bytes();
        assert_eq!(Function::from_bytes(&bytes), Some(function));

        let mut bad = bytes;
        bad[21] = 2;
        assert_eq!(Function::from_bytes(&bad), None);
    }

    #[test]
    fn manifest_round_trips() {
        let manifest = ManifestHeader {
            magic: MANIFEST_MAGIC,
            abi_version: ABI_VERSION,
            profile: PROFILE_X86_64_LE,
            unit_name_len: 5,
            endpoint_count: 0,
            code_len: 512,
            state_size: 16,
            stack_size: 1024,
            entry: 0,
            capacity_profile: 0,
        };
        let bytes = manifest.to_bytes();
        assert_eq!(ManifestHeader::from_bytes(&bytes), Some(manifest));
    }

    #[test]
    fn endpoint_round_trips() {
        let endpoint = Endpoint {
            name_len: 4,
            direction: direction::OUT,
            _reserved: 0,
            capacity: 4096,
        };
        let bytes = endpoint.to_bytes();
        assert_eq!(Endpoint::from_bytes(&bytes), Some(endpoint));
        let mut bad = bytes;
        bad[2] = 7;
        assert_eq!(Endpoint::from_bytes(&bad), None);
    }

    #[test]
    fn status_discriminants_are_stable() {
        assert_eq!(Status::Ok as u8, 0);
        assert_eq!(Status::WouldBlock as u8, 1);
        assert_eq!(Status::Closed as u8, 2);
        assert_eq!(Status::NotFound as u8, 3);
        assert_eq!(Status::Invalid as u8, 4);
        assert_eq!(Status::Full as u8, 5);
        assert_eq!(Status::CapacityExceeded as u8, 6);
        assert_eq!(Status::IoError as u8, 7);
    }

    #[test]
    fn fault_codes_have_names() {
        assert_eq!(FaultCode::OutOfBounds.name(), "OutOfBounds");
        assert_eq!(FaultCode::ExitFault as u8, 11);
    }
}

//! Name resolution, type/capacity checking and bytecode emission.
//!
//! The compiler is a single pass over the AST after the declaration pass:
//! procedures are compiled in declaration order, which fixes their indices and
//! their order in the instruction section. Every buffer write goes through the
//! destination-construction form (`clear` then `append`), so no temporary
//! buffer is ever needed (spec 0004, "Construction").
//!
//! Frame layout, relative to the frame base (all fields 8-byte aligned):
//!
//! ```text
//!   0  prev_fp: u64
//!   8  return_pc: u32
//!  12  function: u32
//!  16  return_sp: u64
//!  24  dest_ref: u64
//!  32  locals: parameters first, then locals, then inline buffers
//!      operand stack (max_depth slots)
//! ```
//!
//! A buffer local occupies `[len: u64][data: cap]`; a reference to its data is
//! formed eight bytes past the value start, and every buffer instruction reads
//! `len` at `data - 8`. A `const`/`var` buffer parameter slot already holds a
//! data reference.
//!
//! A sum value is an 8-byte tag at offset 0 and the payload area at offset 8.
//! Sum types add no opcode: construction stores the tag with `store_ref` and
//! the payload with the payload type's ordinary instructions, and `case`
//! compares the tag and copies the payload into an arm-local slot.
//!
//! Arrays add one opcode, `ref_element`: an element reference is the array's
//! base plus `index * stride`, checked against the element count. Elements
//! then use the same load/store/copy instructions as record fields.
//!
//! Name resolution is table-driven, not hash-driven: a unit is small, so
//! constants, state variables, procedures and locals are looked up by a linear
//! scan of an arena-backed slice. That keeps declaration order observable and
//! removes every hash map from the pipeline (spec 0008, B9).

use core::fmt;

use abi::layout;
use abi::opcodes::{self, builtin};

use crate::arena::{Bump, OutOfSpace, SliceVec};
use crate::ast::*;
use crate::package::LineEntry;
use crate::types::{align8, Kind, Type, MAX_CAPACITY, STATUS_ENUM};
use crate::{diag, out_of_space, package, Diagnostic, Package, Resource, MAX_NESTING};

/// Bytes reserved at the bottom of every frame (spec 0008, "Frames" plus the
/// saved `return_sp` and the destination reference this ABI version adds).
pub const FRAME_HEADER: u32 = 32;

/// Extra stack kept above the sum of all frame sizes, so a shallow call chain
/// has room even when a caller's operand stack is not the deepest one.
const STACK_MARGIN: u32 = 128;

/// Most stream endpoints one application may declare. It matches the kernel's
/// static stream table (spec 0006): a package that declared more could never
/// be loaded, so the compiler reports it instead.
pub const MAX_STREAM_ENDPOINTS: usize = 32;

/// Longest stream name, matching spec 0006 S12 and the manifest's fixed
/// `string[32]` image.
const MAX_STREAM_NAME: usize = 31;

/// True when `name` is a valid stream name: `[a-z][a-z0-9_]*`, at most
/// [`MAX_STREAM_NAME`] bytes.
fn valid_stream_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_STREAM_NAME
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn arena_full(path: &str, unit: &str, line: u32, column: u32) -> Diagnostic {
    out_of_space(path, unit, line, column, Resource::Arena)
}

#[derive(Debug, Clone, Copy)]
struct Const<'a> {
    name: &'a str,
    value: i64,
}

/// One enum type known to this compilation: the predeclared `Status` and
/// `FaultCode` first, then every `type` declaration in source order. The index
/// in this table is the compiler-local enum id carried by [`Kind::Enum`].
#[derive(Debug, Clone, Copy)]
struct EnumDef<'a> {
    name: &'a str,
    variants: &'a [&'a str],
    /// True when the variants are global constants (`Status` and every user
    /// enum). `FaultCode` sets this false because several of its names collide
    /// with `Status`; it is named qualified (`FaultCode.IoError`).
    global_variants: bool,
}

/// One record field after layout: name, resolved type and byte offset from the
/// record start.
#[derive(Debug, Clone, Copy)]
struct RecordField<'a> {
    name: &'a str,
    ty: Type,
    off: u32,
}

/// One nominal record type known to this compilation. The index in this table
/// is the compiler-local record id carried by [`Kind::Record`].
#[derive(Debug, Clone, Copy)]
struct RecordDef<'a> {
    name: &'a str,
    fields: &'a [RecordField<'a>],
    /// Size in bytes, already rounded to 8.
    size: u32,
}

/// One sum-type variant after checking: its name and optional payload type.
#[derive(Debug, Clone, Copy)]
struct SumVariantDef<'a> {
    name: &'a str,
    payload: Option<Type>,
}

/// One nominal sum type known to this compilation. The index in this table is
/// the compiler-local sum id carried by [`Kind::Sum`].
///
/// Layout (spec 0003, "Sum types"): an 8-byte tag at offset 0 and the payload
/// area at offset 8, sized by the largest variant payload and rounded to 8.
#[derive(Debug, Clone, Copy)]
struct SumDef<'a> {
    name: &'a str,
    variants: &'a [SumVariantDef<'a>],
    /// Size in bytes, already rounded to 8.
    size: u32,
}

/// One fixed-size array type known to this compilation. The index in this
/// table is the compiler-local array id carried by [`Kind::Array`].
///
/// Layout (spec 0003, "Arrays"): `count` elements at `element_stride` bytes
/// each, where the stride is the element size rounded to 8; the whole array is
/// rounded to 8. Since every current element type already has an 8-byte size,
/// `stride == size` and `count * stride == size`.
#[derive(Debug, Clone, Copy)]
struct ArrayDef {
    /// Element type.
    elem: Type,
    /// Element count, always at least 1.
    count: u32,
    /// Element stride in bytes, always a multiple of 8.
    stride: u32,
    /// Total size in bytes, already rounded to 8.
    size: u32,
}

/// One fixed-capacity ring type known to this compilation. The index in this
/// table is the compiler-local ring id carried by [`Kind::Ring`].
///
/// Layout (spec 0003, "Rings"): the `count` elements (each `element_stride`
/// bytes) come first, so an element reference uses the ring base directly;
/// then `head: u64` at `count * stride` and `count: u64` eight bytes later.
/// The total is `align8(count * stride + 16)`.
#[derive(Debug, Clone, Copy)]
struct RingDef {
    /// Element type.
    elem: Type,
    /// Capacity, always at least 1.
    count: u32,
    /// Element stride in bytes, always a multiple of 8.
    stride: u32,
    /// Offset of the `head` word: `count * stride`.
    head_off: u32,
    /// Offset of the `count` word: `head_off + 8`.
    count_off: u32,
    /// Total size in bytes, header included, already rounded to 8.
    size: u32,
}

/// One fixed-capacity map type known to this compilation. The index in this
/// table is the compiler-local map id carried by [`Kind::Map`].
///
/// Layout (spec 0003, "Maps"): `count: u64` at offset 0, then `capacity` slots
/// at offset 8. A slot is `state: u64` (0 empty, 1 occupied), `hash: u64`, the
/// key image and the value image; `16 + key_size + value_size` bytes, always a
/// multiple of 8. The table capacity is the smallest power of two at least
/// twice `count`, so an empty slot always exists while `count <= N`.
#[derive(Debug, Clone, Copy)]
struct MapDef {
    /// Key type: `int`, `string[M]` or `bytes[M]`.
    key: Type,
    /// Value type: any fixed-size type.
    value: Type,
    /// Entry count `N`, always at least 1.
    count: u32,
    /// Table capacity: a power of two, at least `2 * count`.
    capacity: u32,
    /// Key image size in bytes (`8` for `int`, `align8(8 + cap)` for buffers).
    key_size: u32,
    /// Value image size in bytes.
    value_size: u32,
    /// Total size in bytes: `8 + capacity * slot_stride`, where
    /// `slot_stride = 16 + key_size + value_size`.
    size: u32,
}

/// Compiler-local id of the predeclared `Diagnostic` record.
const DIAGNOSTIC_RECORD: u16 = 0;

/// Compiler-local id of the predeclared `ExitReason` sum type.
const EXIT_REASON_SUM: u16 = 0;

/// Compiler-local id of the predeclared `FaultCode` enum. It follows `Status`.
const FAULT_CODE_ENUM: u16 = 1;

/// Fields of the predeclared `Diagnostic` record, at the offsets `abi::layout`
/// fixes. The kernel writes the same fields directly.
const DIAGNOSTIC_FIELDS: [RecordField<'static>; 8] = [
    RecordField {
        name: "unit_name",
        ty: Type::buffer(Kind::String, layout::diagnostic::UNIT_NAME_CAP),
        off: layout::diagnostic::UNIT_NAME_OFFSET,
    },
    RecordField {
        name: "message",
        ty: Type::buffer(Kind::String, layout::diagnostic::MESSAGE_CAP),
        off: layout::diagnostic::MESSAGE_OFFSET,
    },
    RecordField {
        name: "line",
        ty: Type::scalar(Kind::Int),
        off: layout::diagnostic::LINE_OFFSET,
    },
    RecordField {
        name: "column",
        ty: Type::scalar(Kind::Int),
        off: layout::diagnostic::COLUMN_OFFSET,
    },
    RecordField {
        name: "handle",
        ty: Type::scalar(Kind::Int),
        off: layout::diagnostic::HANDLE_OFFSET,
    },
    RecordField {
        name: "offset",
        ty: Type::scalar(Kind::Int),
        off: layout::diagnostic::OFFSET_OFFSET,
    },
    RecordField {
        name: "length",
        ty: Type::scalar(Kind::Int),
        off: layout::diagnostic::LENGTH_OFFSET,
    },
    RecordField {
        name: "task_id",
        ty: Type::scalar(Kind::Int),
        off: layout::diagnostic::TASK_ID_OFFSET,
    },
];

/// Variants of the predeclared `FaultCode` enum, in the order `abi::FaultCode`
/// fixes. They are not global constants: `FaultCode` names them qualified
/// (`FaultCode.DivideByZero`) because several collide with `Status` variants.
const FAULT_CODE_VARIANTS: [&str; 12] = [
    "GenericFault",
    "OutOfBounds",
    "CapacityExceeded",
    "Overflow",
    "DivideByZero",
    "StackOverflow",
    "BudgetExceeded",
    "WatchdogTimeout",
    "IllegalOperation",
    "StreamError",
    "IoError",
    "ExitFault",
];

/// Variants of the predeclared `ExitReason` sum type, in the order
/// `abi::layout::exit_tag` fixes. `Faulted` carries the `FaultCode` enum.
const EXIT_REASON_VARIANTS: [SumVariantDef<'static>; 3] = [
    SumVariantDef {
        name: "Ok",
        payload: None,
    },
    SumVariantDef {
        name: "Killed",
        payload: None,
    },
    SumVariantDef {
        name: "Faulted",
        payload: Some(Type::enumeration(FAULT_CODE_ENUM)),
    },
];

/// Variants of the predeclared `Status` type, in the order spec 0003 fixes.
/// Their ordinals are `0..7` and match `abi::Status`.
const STATUS_VARIANTS: [&str; 8] = [
    "Ok",
    "WouldBlock",
    "Closed",
    "NotFound",
    "Invalid",
    "Full",
    "CapacityExceeded",
    "IoError",
];

/// Ordinals used by the compiler-lowered ring operations. They follow
/// [`STATUS_VARIANTS`] (and `abi::Status`): `Ok` 0, `NotFound` 3, `Full` 5.
const STATUS_OK: i64 = 0;
const STATUS_NOT_FOUND: i64 = 3;
const STATUS_FULL: i64 = 5;

#[derive(Debug, Clone, Copy)]
struct StateVar<'a> {
    name: &'a str,
    ty: Type,
    off: u32,
}

/// One declared stream endpoint, in `state` declaration order. The manifest is
/// emitted from this list, so the descriptor order and the handle offsets
/// (`state[i * 8]`) always agree (spec 0006, spec 0008).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EndpointDecl<'a> {
    pub(crate) name: &'a str,
    /// `abi::direction::IN` or `abi::direction::OUT`.
    pub(crate) direction: u8,
    pub(crate) capacity: u32,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

#[derive(Debug, Clone, Copy)]
struct Param<'a> {
    name: &'a str,
    mode: ParamMode,
    ty: Type,
}

#[derive(Debug, Clone, Copy)]
struct Sig<'a> {
    name: &'a str,
    params: &'a [Param<'a>],
    /// `Some` for a function: the scalar result type. `None` for a procedure.
    ret: Option<Type>,
    line: u32,
    column: u32,
}

#[derive(Clone, Copy)]
struct CompiledFn<'a> {
    code: &'a [u8],
    frame_size: u32,
    operand_off: u32,
    param_count: u8,
    has_destination: bool,
    lines: &'a [LineEntry],
}

#[derive(Debug, Clone, Copy)]
struct ConstRef {
    offset: u32,
    len: u32,
}

/// Interns buffer literals into the constant blob, one entry per distinct byte
/// string, in first-use order. Interning scans the entries already emitted, so
/// constant order is first-use order and nothing depends on a hash seed.
struct ConstantPool<'a, 'b> {
    data: SliceVec<'a, 'b, u8>,
    index: SliceVec<'a, 'b, ConstRef>,
}

impl<'a, 'b> ConstantPool<'a, 'b> {
    fn new(path: &str, unit: &str, bump: &'b Bump<'a>) -> Result<Self, Diagnostic> {
        let data = SliceVec::new(bump).map_err(|_| arena_full(path, unit, 1, 1))?;
        let index = SliceVec::new(bump).map_err(|_| arena_full(path, unit, 1, 1))?;
        Ok(ConstantPool { data, index })
    }

    fn intern(&mut self, bytes: &[u8]) -> Result<u32, OutOfSpace> {
        for entry in self.index.as_slice() {
            let start = entry.offset as usize;
            let end = start + entry.len as usize;
            if entry.len as usize == bytes.len() && self.data.as_slice()[start..end] == *bytes {
                return Ok(entry.offset);
            }
        }
        let offset = u32::try_from(self.data.len() + 8).map_err(|_| OutOfSpace)?;
        for byte in (bytes.len() as u64).to_le_bytes() {
            self.data.push(byte)?;
        }
        self.data.extend_from_slice(bytes)?;
        while !self.data.len().is_multiple_of(8) {
            self.data.push(0)?;
        }
        self.index.push(ConstRef {
            offset,
            len: bytes.len() as u32,
        })?;
        Ok(offset)
    }
}

/// True when a type can cross the library/application unit boundary.
///
/// Scalars, buffers and the predeclared enums have representation-only
/// identity; records, sums and user enums are compiler-local ids. Array, ring
/// and map ids are shared by the one interning table, but their element and
/// key/value types must themselves be safe.
fn cross_unit_type_ok(ty: Type, arrays: &[ArrayDef], rings: &[RingDef], maps: &[MapDef]) -> bool {
    match ty.kind {
        Kind::Record(_) | Kind::Sum(_) => false,
        Kind::Enum(id) => id <= FAULT_CODE_ENUM,
        Kind::Array(id) => arrays
            .get(id as usize)
            .is_some_and(|array| cross_unit_type_ok(array.elem, arrays, rings, maps)),
        Kind::Ring(id) => rings
            .get(id as usize)
            .is_some_and(|ring| cross_unit_type_ok(ring.elem, arrays, rings, maps)),
        Kind::Map(id) => maps.get(id as usize).is_some_and(|map| {
            cross_unit_type_ok(map.key, arrays, rings, maps)
                && cross_unit_type_ok(map.value, arrays, rings, maps)
        }),
        _ => true,
    }
}

fn find_const<'a>(consts: &[Const<'a>], name: &str) -> Option<i64> {
    consts.iter().find(|c| c.name == name).map(|c| c.value)
}

fn find_state<'a>(vars: &[StateVar<'a>], name: &str) -> Option<StateVar<'a>> {
    vars.iter().find(|v| v.name == name).copied()
}

fn find_sig<'a>(sigs: &[Sig<'a>], name: &str) -> Option<usize> {
    sigs.iter().position(|s| s.name == name)
}

/// Finds an enum type by name.
fn find_enum(enums: &[EnumDef<'_>], name: &str) -> Option<u16> {
    enums
        .iter()
        .position(|e| e.name == name)
        .and_then(|index| u16::try_from(index).ok())
}

/// Finds a record type by name.
fn find_record(records: &[RecordDef<'_>], name: &str) -> Option<u16> {
    records
        .iter()
        .position(|record| record.name == name)
        .and_then(|index| u16::try_from(index).ok())
}

/// Finds a sum type by name.
fn find_sum(sums: &[SumDef<'_>], name: &str) -> Option<u16> {
    sums.iter()
        .position(|sum| sum.name == name)
        .and_then(|index| u16::try_from(index).ok())
}

/// Finds an array type by its compiler-local id, returning its layout.
fn array_of(arrays: &[ArrayDef], ty: Type) -> Option<ArrayDef> {
    match ty.kind {
        Kind::Array(id) => arrays.get(id as usize).copied(),
        _ => None,
    }
}

/// Interns `elem × count` structurally and returns the array type.
///
/// The table is shared by both units of one compilation (the application and
/// the embedded library), so the same written `array[T, N]` resolves to the
/// same id on either side of a call. Interning scans existing entries, so the
/// numbering is first-use order and deterministic.
fn intern_array<'a>(
    arrays: &mut SliceVec<'a, '_, ArrayDef>,
    elem: Type,
    count: u32,
    line: u32,
    column: u32,
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
    for (index, entry) in arrays.as_slice().iter().enumerate() {
        if entry.elem == elem && entry.count == count {
            return Ok(Type::array(index as u16, entry.size));
        }
    }
    let stride = align8(elem.size());
    let size = count.checked_mul(stride).ok_or_else(|| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("array size overflows"),
        )
    })?;
    if size > MAX_CAPACITY {
        return Err(diag(
            path,
            unit,
            line,
            column,
            format_args!(
                "array of {count} elements is {size} bytes, over the {MAX_CAPACITY}-byte cap"
            ),
        ));
    }
    let size = align8(size);
    let index = u16::try_from(arrays.len()).map_err(|_| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("too many distinct array types"),
        )
    })?;
    arrays
        .push(ArrayDef {
            elem,
            count,
            stride,
            size,
        })
        .map_err(|_| arena_full(path, unit, line, column))?;
    Ok(Type::array(index, size))
}

/// Finds a ring type by its compiler-local id, returning its layout.
fn ring_of(rings: &[RingDef], ty: Type) -> Option<RingDef> {
    match ty.kind {
        Kind::Ring(id) => rings.get(id as usize).copied(),
        _ => None,
    }
}

/// Interns `elem × count` structurally and returns the ring type.
///
/// The table is shared by both units of one compilation, so the same written
/// `ring[T, N]` resolves to the same id on either side of a call. Interning
/// scans existing entries, so the numbering is first-use order and
/// deterministic.
fn intern_ring<'a>(
    rings: &mut SliceVec<'a, '_, RingDef>,
    elem: Type,
    count: u32,
    line: u32,
    column: u32,
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
    for (index, entry) in rings.as_slice().iter().enumerate() {
        if entry.elem == elem && entry.count == count {
            return Ok(Type::ring(index as u16, entry.size));
        }
    }
    let stride = align8(elem.size());
    let storage = count.checked_mul(stride).ok_or_else(|| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("ring size overflows"),
        )
    })?;
    let size = storage.checked_add(16).ok_or_else(|| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("ring size overflows"),
        )
    })?;
    if size > MAX_CAPACITY {
        return Err(diag(
            path,
            unit,
            line,
            column,
            format_args!(
                "ring of {count} elements is {size} bytes, over the {MAX_CAPACITY}-byte cap"
            ),
        ));
    }
    let index = u16::try_from(rings.len()).map_err(|_| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("too many distinct ring types"),
        )
    })?;
    rings
        .push(RingDef {
            elem,
            count,
            stride,
            head_off: storage,
            count_off: storage + 8,
            size: align8(size),
        })
        .map_err(|_| arena_full(path, unit, line, column))?;
    Ok(Type::ring(index, align8(size)))
}

/// Finds a map type by its compiler-local id, returning its layout.
fn map_of(maps: &[MapDef], ty: Type) -> Option<MapDef> {
    match ty.kind {
        Kind::Map(id) => maps.get(id as usize).copied(),
        _ => None,
    }
}

/// The smallest power of two at least `2 * count` (spec 0003, "Maps").
fn map_table_capacity(count: u32) -> Option<u32> {
    let wanted = count.checked_mul(2)?;
    let capacity = wanted.checked_next_power_of_two()?;
    // `u32::MAX` is not a power of two, so `next_power_of_two` cannot return
    // a value that is too small; the caller bounds `count`.
    Some(capacity)
}

/// Interns `(key, value, count)` structurally and returns the map type.
///
/// The table is shared by both units of one compilation, so the same written
/// `map[K, V, N]` resolves to the same id on either side of a call. Interning
/// scans existing entries, so the numbering is first-use order and
/// deterministic.
#[allow(clippy::too_many_arguments)]
fn intern_map<'a>(
    maps: &mut SliceVec<'a, '_, MapDef>,
    key: Type,
    value: Type,
    count: u32,
    line: u32,
    column: u32,
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
    for (index, entry) in maps.as_slice().iter().enumerate() {
        if entry.key == key && entry.value == value && entry.count == count {
            return Ok(Type::map(index as u16, entry.size));
        }
    }
    let capacity = map_table_capacity(count).ok_or_else(|| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("map table capacity overflows"),
        )
    })?;
    let key_size = key.size();
    let value_size = value.size();
    let slot_stride = 16u32
        .checked_add(key_size)
        .and_then(|size| size.checked_add(value_size))
        .ok_or_else(|| {
            diag(
                path,
                unit,
                line,
                column,
                format_args!("map slot size overflows"),
            )
        })?;
    let size = capacity
        .checked_mul(slot_stride)
        .and_then(|size| size.checked_add(8))
        .ok_or_else(|| diag(path, unit, line, column, format_args!("map size overflows")))?;
    if size > MAX_CAPACITY {
        return Err(diag(
            path,
            unit,
            line,
            column,
            format_args!(
                "map of {count} entries is {size} bytes ({capacity} slots of {slot_stride}), over the {MAX_CAPACITY}-byte cap"
            ),
        ));
    }
    let index = u16::try_from(maps.len()).map_err(|_| {
        diag(
            path,
            unit,
            line,
            column,
            format_args!("too many distinct map types"),
        )
    })?;
    maps.push(MapDef {
        key,
        value,
        count,
        capacity,
        key_size,
        value_size,
        size,
    })
    .map_err(|_| arena_full(path, unit, line, column))?;
    Ok(Type::map(index, size))
}

/// Returns the sum definition a type names, if it is a sum type.
fn sum_of<'a>(sums: &[SumDef<'a>], ty: Type) -> Option<SumDef<'a>> {
    match ty.kind {
        Kind::Sum(id) => sums.get(id as usize).copied(),
        _ => None,
    }
}

/// Returns a variant of a sum type by name.
fn variant_of<'a>(sum: &SumDef<'a>, name: &str) -> Option<SumVariantDef<'a>> {
    sum.variants
        .iter()
        .find(|variant| variant.name == name)
        .copied()
}

/// True when any sum type already uses `name` as a variant.
fn has_sum_variant(sums: &[SumDef<'_>], name: &str) -> bool {
    sums.iter()
        .any(|sum| sum.variants.iter().any(|variant| variant.name == name))
}

/// Returns the record definition a type names, if it is a record.
fn record_of<'a>(records: &[RecordDef<'a>], ty: Type) -> Option<RecordDef<'a>> {
    match ty.kind {
        Kind::Record(id) => records.get(id as usize).copied(),
        _ => None,
    }
}

/// Returns the field named `name` of a record type.
fn field_of<'a>(records: &[RecordDef<'a>], ty: Type, name: &str) -> Option<RecordField<'a>> {
    record_of(records, ty)?
        .fields
        .iter()
        .find(|field| field.name == name)
        .copied()
}

/// Finds a variant in any enum, returning its enum id and ordinal.
///
/// Only enums whose variants are global constants take part; `FaultCode` is
/// named qualified because its variant names overlap `Status`.
fn find_variant(enums: &[EnumDef<'_>], name: &str) -> Option<(u16, u32)> {
    for (index, enumeration) in enums.iter().enumerate() {
        if !enumeration.global_variants {
            continue;
        }
        if let Some(ordinal) = enumeration
            .variants
            .iter()
            .position(|variant| *variant == name)
        {
            return Some((u16::try_from(index).ok()?, ordinal as u32));
        }
    }
    None
}

/// Finds a qualified enum variant, `TypeName.Variant`.
fn find_qualified_variant(
    enums: &[EnumDef<'_>],
    type_name: &str,
    name: &str,
) -> Option<(u16, u32)> {
    let id = find_enum(enums, type_name)?;
    let ordinal = enums[id as usize]
        .variants
        .iter()
        .position(|variant| *variant == name)?;
    Some((id, ordinal as u32))
}

/// True when any enum already uses `name` as a variant.
fn has_variant(enums: &[EnumDef<'_>], name: &str) -> bool {
    find_variant(enums, name).is_some()
}

/// True for the built-in type names an enum may not shadow.
fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "uint"
            | "byte"
            | "bool"
            | "f64"
            | "string"
            | "bytes"
            | "array"
            | "ring"
            | "map"
            | "in_stream"
            | "out_stream"
    )
}

/// Builds one unit's symbol tables: constants, enums, records, sums, state and
/// signatures. Type ids are local to the unit, so two units can be compiled
/// side by side without a shared declaration namespace.
struct UnitTables<'a> {
    name: &'a str,
    consts: &'a [Const<'a>],
    enums: &'a [EnumDef<'a>],
    records: &'a [RecordDef<'a>],
    sums: &'a [SumDef<'a>],
    state_vars: &'a [StateVar<'a>],
    state_size: u32,
    /// Stream endpoints declared by this unit, in declaration order. Only an
    /// application may declare stream variables.
    endpoints: &'a [EndpointDecl<'a>],
    sigs: &'a [Sig<'a>],
}

/// Rejects a `uses` name the unit's role does not provide.
fn check_uses<'a>(unit: &Unit<'a>, path: &'a str, is_application: bool) -> Result<(), Diagnostic> {
    let name = unit.name;
    for used in unit.uses {
        let known = match *used {
            "core" | "syslib" => true,
            // Only an application may import the library; the library may not
            // import the application or itself (no cycles, spec 0003 M4).
            "stdlib" => is_application,
            _ => false,
        };
        if !known {
            return Err(diag(
                path,
                name,
                1,
                1,
                format_args!(
                    "unknown unit `{used}`; this slice provides `core` and `syslib`{}",
                    if is_application { " and `stdlib`" } else { "" }
                ),
            ));
        }
    }
    Ok(())
}

/// Builds one parsed unit's tables.
fn build_tables<'a>(
    unit: &Unit<'a>,
    path: &'a str,
    allow_streams: bool,
    bump: &Bump<'a>,
    arrays: &mut SliceVec<'a, '_, ArrayDef>,
    rings: &mut SliceVec<'a, '_, RingDef>,
    maps: &mut SliceVec<'a, '_, MapDef>,
) -> Result<UnitTables<'a>, Diagnostic> {
    let name = unit.name;
    if name.is_empty() {
        return Err(diag(path, "", 1, 1, format_args!("unit has no name")));
    }

    // Constants come first: they may size a record field's buffer, and their
    // values depend on nothing but other constants. The type table is built
    // next and rejects a type name that collides with a constant.
    let mut consts = SliceVec::<Const<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    for decl in unit.consts {
        if find_const(consts.as_slice(), decl.name).is_some() {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!("duplicate declaration of `{}`", decl.name),
            ));
        }
        let value = eval_const(decl.value, consts.as_slice(), path, name, 0)?;
        consts
            .push(Const {
                name: decl.name,
                value,
            })
            .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
    }

    // The predeclared `Status` comes first, so its id is `STATUS_ENUM` and its
    // ordinals match `abi::Status`. `FaultCode` follows, with the ordinals of
    // `abi::FaultCode`; its variant names are qualified because some collide
    // with `Status`. User `type` declarations follow in source order; the index
    // in this table is the compiler-local enum id.
    let mut enums = SliceVec::<EnumDef<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    enums
        .push(EnumDef {
            name: "Status",
            variants: &STATUS_VARIANTS,
            global_variants: true,
        })
        .map_err(|_| arena_full(path, name, 1, 1))?;
    enums
        .push(EnumDef {
            name: "FaultCode",
            variants: &FAULT_CODE_VARIANTS,
            global_variants: false,
        })
        .map_err(|_| arena_full(path, name, 1, 1))?;

    // The predeclared `Diagnostic` is the first record entry; its layout is
    // fixed by `abi::layout`, so the kernel and the compiler cannot disagree.
    let mut records =
        SliceVec::<RecordDef<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    records
        .push(RecordDef {
            name: "Diagnostic",
            fields: &DIAGNOSTIC_FIELDS,
            size: layout::diagnostic::SIZE,
        })
        .map_err(|_| arena_full(path, name, 1, 1))?;

    // The predeclared `ExitReason` is the first sum type
    // (`Ok | Killed | Faulted(code: FaultCode)`); its tag/payload layout is
    // `abi::layout::exit_reason`.
    let mut sums = SliceVec::<SumDef<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    sums.push(SumDef {
        name: "ExitReason",
        variants: &EXIT_REASON_VARIANTS,
        size: layout::exit_reason::SIZE,
    })
    .map_err(|_| arena_full(path, name, 1, 1))?;

    for decl in unit.types {
        if matches!(
            decl.name,
            "Status" | "FaultCode" | "Diagnostic" | "ExitReason"
        ) {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!(
                    "`{}` is predeclared by the language and cannot be declared",
                    decl.name
                ),
            ));
        }
        if is_builtin_type_name(decl.name) {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!(
                    "type name `{}` shadows a built-in type; choose another name",
                    decl.name
                ),
            ));
        }
        if find_enum(enums.as_slice(), decl.name).is_some()
            || find_record(records.as_slice(), decl.name).is_some()
            || find_sum(sums.as_slice(), decl.name).is_some()
            || find_const(consts.as_slice(), decl.name).is_some()
        {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!("duplicate declaration of type `{}`", decl.name),
            ));
        }
        match decl.kind {
            TypeKind::Enum(variants) => {
                for (index, variant) in variants.iter().enumerate() {
                    if has_variant(enums.as_slice(), variant)
                        || variants[..index].contains(variant)
                        || find_const(consts.as_slice(), variant).is_some()
                        || has_sum_variant(sums.as_slice(), variant)
                    {
                        return Err(diag(
                            path,
                            name,
                            decl.line,
                            decl.column,
                            format_args!(
                                "duplicate variant `{variant}`; variants share one top-level namespace"
                            ),
                        ));
                    }
                }
                enums
                    .push(EnumDef {
                        name: decl.name,
                        variants,
                        global_variants: true,
                    })
                    .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
            }
            TypeKind::Record(fields) => {
                // Records are nominal and every field type must already be
                // declared: declaration before use keeps layout a single pass.
                let mut laid_out = SliceVec::<RecordField<'a>>::new(bump)
                    .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
                let mut size = 0u32;
                for field in fields {
                    if laid_out
                        .as_slice()
                        .iter()
                        .any(|existing| existing.name == field.name)
                    {
                        return Err(diag(
                            path,
                            name,
                            field.line,
                            field.column,
                            format_args!("duplicate field `{}`", field.name),
                        ));
                    }
                    let ty = resolve_type(
                        &field.ty,
                        consts.as_slice(),
                        enums.as_slice(),
                        records.as_slice(),
                        sums.as_slice(),
                        arrays,
                        rings,
                        maps,
                        path,
                        name,
                    )?;
                    if ty.is_buffer() && ty.cap == 0 {
                        return Err(diag(
                            path,
                            name,
                            field.line,
                            field.column,
                            format_args!(
                                "buffer field `{}` needs a capacity of at least 1",
                                field.name
                            ),
                        ));
                    }
                    if ty.is_record() && ty.cap == 0 {
                        return Err(diag(
                            path,
                            name,
                            field.line,
                            field.column,
                            format_args!("record field `{}` is empty", field.name),
                        ));
                    }
                    let off = align8(size);
                    size = off.checked_add(ty.size()).ok_or_else(|| {
                        diag(
                            path,
                            name,
                            field.line,
                            field.column,
                            format_args!("record `{}` is too large", decl.name),
                        )
                    })?;
                    laid_out
                        .push(RecordField {
                            name: field.name,
                            ty,
                            off,
                        })
                        .map_err(|_| arena_full(path, name, field.line, field.column))?;
                }
                let size = align8(size);
                if size > MAX_CAPACITY {
                    return Err(diag(
                        path,
                        name,
                        decl.line,
                        decl.column,
                        format_args!("record `{}` exceeds the {MAX_CAPACITY}-byte cap", decl.name),
                    ));
                }
                records
                    .push(RecordDef {
                        name: decl.name,
                        fields: laid_out.into_slice(),
                        size,
                    })
                    .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
            }
            TypeKind::Sum(variants) => {
                // Sum types are nominal. Each variant has at most one payload,
                // which must already be declared; the layout is an 8-byte tag
                // followed by the largest payload, rounded to 8 (spec 0003).
                let mut laid_out = SliceVec::<SumVariantDef<'a>>::new(bump)
                    .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
                let mut largest = 0u32;
                for variant in variants {
                    if laid_out
                        .as_slice()
                        .iter()
                        .any(|existing| existing.name == variant.name)
                    {
                        return Err(diag(
                            path,
                            name,
                            variant.line,
                            variant.column,
                            format_args!("duplicate variant `{}`", variant.name),
                        ));
                    }
                    if has_variant(enums.as_slice(), variant.name)
                        || has_sum_variant(sums.as_slice(), variant.name)
                        || find_const(consts.as_slice(), variant.name).is_some()
                        || find_enum(enums.as_slice(), variant.name).is_some()
                        || find_record(records.as_slice(), variant.name).is_some()
                    {
                        return Err(diag(
                            path,
                            name,
                            variant.line,
                            variant.column,
                            format_args!(
                                "duplicate variant `{}`; variants share one top-level namespace",
                                variant.name
                            ),
                        ));
                    }
                    let payload = match &variant.payload {
                        Some((field, ty)) => {
                            let ty = resolve_type(
                                ty,
                                consts.as_slice(),
                                enums.as_slice(),
                                records.as_slice(),
                                sums.as_slice(),
                                arrays,
                                rings,
                                maps,
                                path,
                                name,
                            )?;
                            if ty.is_buffer() && ty.cap == 0 {
                                return Err(diag(
                                    path,
                                    name,
                                    variant.line,
                                    variant.column,
                                    format_args!(
                                        "payload `{field}` needs a capacity of at least 1"
                                    ),
                                ));
                            }
                            largest = largest.max(ty.size());
                            Some(ty)
                        }
                        None => None,
                    };
                    laid_out
                        .push(SumVariantDef {
                            name: variant.name,
                            payload,
                        })
                        .map_err(|_| arena_full(path, name, variant.line, variant.column))?;
                }
                let size = 8u32.checked_add(largest).ok_or_else(|| {
                    diag(
                        path,
                        name,
                        decl.line,
                        decl.column,
                        format_args!("sum type `{}` is too large", decl.name),
                    )
                })?;
                if size > MAX_CAPACITY {
                    return Err(diag(
                        path,
                        name,
                        decl.line,
                        decl.column,
                        format_args!(
                            "sum type `{}` exceeds the {MAX_CAPACITY}-byte cap",
                            decl.name
                        ),
                    ));
                }
                sums.push(SumDef {
                    name: decl.name,
                    variants: laid_out.into_slice(),
                    size: align8(size),
                })
                .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
            }
        }
    }

    // Constants are declared before the type table is built, so a constant
    // that collides with a type or variant is rejected here, once the type
    // names exist.
    for decl in unit.consts {
        if find_enum(enums.as_slice(), decl.name).is_some()
            || find_record(records.as_slice(), decl.name).is_some()
            || find_sum(sums.as_slice(), decl.name).is_some()
            || has_variant(enums.as_slice(), decl.name)
            || has_sum_variant(sums.as_slice(), decl.name)
        {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!("duplicate declaration of `{}`", decl.name),
            ));
        }
    }

    let mut state_vars =
        SliceVec::<StateVar<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    let mut endpoints =
        SliceVec::<EndpointDecl<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    // Stream handles occupy the first `8 * endpoint_count` bytes of `state`,
    // in declaration order; every other state variable follows. The syntactic
    // name check is enough to reserve the prefix: `resolve_type` rejects every
    // other spelling of a stream type.
    let stream_count = unit
        .vars
        .iter()
        .filter(|decl| matches!(decl.ty.name, "in_stream" | "out_stream"))
        .count();
    if stream_count > MAX_STREAM_ENDPOINTS {
        return Err(diag(
            path,
            name,
            1,
            1,
            format_args!(
                "an application may declare at most {MAX_STREAM_ENDPOINTS} stream endpoints, found {stream_count}"
            ),
        ));
    }
    let state_prefix = u32::try_from(stream_count)
        .ok()
        .and_then(|count| count.checked_mul(8))
        .ok_or_else(|| diag(path, name, 1, 1, format_args!("state region is too large")))?;
    let mut state_size = state_prefix;
    for decl in unit.vars {
        if find_const(consts.as_slice(), decl.name).is_some()
            || find_state(state_vars.as_slice(), decl.name).is_some()
            || find_enum(enums.as_slice(), decl.name).is_some()
            || find_record(records.as_slice(), decl.name).is_some()
            || find_sum(sums.as_slice(), decl.name).is_some()
            || has_variant(enums.as_slice(), decl.name)
            || has_sum_variant(sums.as_slice(), decl.name)
        {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!("duplicate declaration of `{}`", decl.name),
            ));
        }
        let ty = resolve_type_with(
            &decl.ty,
            consts.as_slice(),
            enums.as_slice(),
            records.as_slice(),
            sums.as_slice(),
            arrays,
            rings,
            maps,
            allow_streams,
            path,
            name,
        )?;
        if ty.is_buffer() && ty.cap == 0 {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!(
                    "buffer variable `{}` needs a capacity of at least 1",
                    decl.name
                ),
            ));
        }
        let off = if ty.is_stream() {
            if !valid_stream_name(decl.name) {
                return Err(diag(
                    path,
                    name,
                    decl.line,
                    decl.column,
                    format_args!(
                        "stream name `{}` must match `[a-z][a-z0-9_]*` and be at most {MAX_STREAM_NAME} bytes",
                        decl.name
                    ),
                ));
            }
            let off = u32::try_from(endpoints.len())
                .ok()
                .and_then(|index| index.checked_mul(8))
                .ok_or_else(|| {
                    diag(
                        path,
                        name,
                        decl.line,
                        decl.column,
                        format_args!("state region is too large"),
                    )
                })?;
            let direction = match ty.kind {
                Kind::InStream => abi::direction::IN,
                Kind::OutStream => abi::direction::OUT,
                _ => unreachable!("only stream types enter this branch"),
            };
            endpoints
                .push(EndpointDecl {
                    name: decl.name,
                    direction,
                    capacity: ty.cap,
                    line: decl.line,
                    column: decl.column,
                })
                .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
            off
        } else {
            let off = align8(state_size);
            state_size = off.checked_add(ty.size()).ok_or_else(|| {
                diag(
                    path,
                    name,
                    decl.line,
                    decl.column,
                    format_args!("state region is too large"),
                )
            })?;
            off
        };
        if state_size > MAX_CAPACITY {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!("state region exceeds the {MAX_CAPACITY}-byte cap"),
            ));
        }
        state_vars
            .push(StateVar {
                name: decl.name,
                ty,
                off,
            })
            .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
    }

    let mut sigs = SliceVec::<Sig<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    for proc in unit.procs {
        if find_const(consts.as_slice(), proc.name).is_some()
            || find_state(state_vars.as_slice(), proc.name).is_some()
            || find_sig(sigs.as_slice(), proc.name).is_some()
            || find_enum(enums.as_slice(), proc.name).is_some()
            || find_record(records.as_slice(), proc.name).is_some()
            || find_sum(sums.as_slice(), proc.name).is_some()
            || has_variant(enums.as_slice(), proc.name)
            || has_sum_variant(sums.as_slice(), proc.name)
        {
            return Err(diag(
                path,
                name,
                proc.line,
                proc.column,
                format_args!("duplicate declaration of `{}`", proc.name),
            ));
        }
        if proc.params.len() > u8::MAX as usize {
            return Err(diag(
                path,
                name,
                proc.line,
                proc.column,
                format_args!("too many parameters (limit is 255)"),
            ));
        }
        let mut params = SliceVec::<Param<'a>>::new(bump)
            .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
        for param in proc.params {
            if params.as_slice().iter().any(|p| p.name == param.name) {
                return Err(diag(
                    path,
                    name,
                    param.line,
                    param.column,
                    format_args!("duplicate parameter `{}`", param.name),
                ));
            }
            let ty = resolve_type_with(
                &param.ty,
                consts.as_slice(),
                enums.as_slice(),
                records.as_slice(),
                sums.as_slice(),
                arrays,
                rings,
                maps,
                true,
                path,
                name,
            )?;
            if ty.is_stream() && param.mode != ParamMode::Const {
                return Err(diag(
                    path,
                    name,
                    param.line,
                    param.column,
                    format_args!(
                        "stream parameter `{}` must be declared `const`; a `value` copy or a `var` reference to an endpoint is not meaningful",
                        param.name
                    ),
                ));
            }
            if ty.is_buffer() {
                if param.mode == ParamMode::Value {
                    return Err(diag(
                        path,
                        name,
                        param.line,
                        param.column,
                        format_args!(
                            "buffer parameter `{}` must be declared `const` or `var`",
                            param.name
                        ),
                    ));
                }
                if param.mode == ParamMode::Var && ty.cap == 0 {
                    return Err(diag(
                        path,
                        name,
                        param.line,
                        param.column,
                        format_args!(
                            "`var` buffer parameter `{}` needs a capacity of at least 1",
                            param.name
                        ),
                    ));
                }
            }
            params
                .push(Param {
                    name: param.name,
                    mode: param.mode,
                    ty,
                })
                .map_err(|_| arena_full(path, name, param.line, param.column))?;
        }
        let params = params.into_slice();
        let ret = match &proc.ret {
            Some(ty) => {
                let resolved = resolve_type(
                    ty,
                    consts.as_slice(),
                    enums.as_slice(),
                    records.as_slice(),
                    sums.as_slice(),
                    arrays,
                    rings,
                    maps,
                    path,
                    name,
                )?;
                if !resolved.is_scalar() {
                    return Err(diag(
                        path,
                        name,
                        ty.line,
                        ty.column,
                        format_args!(
                            "function `{}` must return a scalar type; buffer, record, sum, array, ring and map results are not implemented",
                            proc.name
                        ),
                    ));
                }
                Some(resolved)
            }
            None => None,
        };
        sigs.push(Sig {
            name: proc.name,
            params,
            ret,
            line: proc.line,
            column: proc.column,
        })
        .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
    }

    Ok(UnitTables {
        name,
        consts: consts.into_slice(),
        enums: enums.into_slice(),
        records: records.into_slice(),
        sums: sums.into_slice(),
        state_vars: state_vars.into_slice(),
        state_size,
        endpoints: endpoints.into_slice(),
        sigs: sigs.into_slice(),
    })
}

/// Compiles every procedure of one unit, returning the function images in
/// declaration order. `call_base` is the unit's first index in the shared
/// function table; `library_sigs` is `Some` only for the application, whose
/// qualified `stdlib.<name>` calls resolve there.
#[allow(clippy::too_many_arguments)]
fn compile_procs<'a, 'b>(
    unit: &Unit<'a>,
    tables: &UnitTables<'a>,
    path: &'a str,
    call_base: u32,
    library_sigs: Option<&'a [Sig<'a>]>,
    state_vars: &'a [StateVar<'a>],
    arrays: &mut SliceVec<'a, 'b, ArrayDef>,
    rings: &mut SliceVec<'a, 'b, RingDef>,
    maps: &mut SliceVec<'a, 'b, MapDef>,
    unit_id: u32,
    pool: &mut ConstantPool<'a, 'b>,
    bump: &'b Bump<'a>,
) -> Result<&'a [CompiledFn<'a>], Diagnostic> {
    let name = tables.name;
    let mut compiled =
        SliceVec::<CompiledFn<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    for (index, proc) in unit.procs.iter().enumerate() {
        let current = tables.sigs[index];
        let mut compiler = FnCompiler::new(
            path,
            name,
            tables.sigs,
            tables.consts,
            state_vars,
            tables.enums,
            tables.records,
            tables.sums,
            &mut *arrays,
            &mut *rings,
            &mut *maps,
            call_base,
            library_sigs,
            unit_id,
            pool,
            current,
            bump,
        )?;
        compiler.compile_procedure(proc)?;
        let function = compiler.finish()?;
        compiled
            .push(function)
            .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
    }
    Ok(compiled.into_slice())
}

/// Appends one compiled function to the shared instruction, function and line
/// tables, rebasing its code offsets and line positions.
#[allow(clippy::too_many_arguments)]
fn append_function<'a>(
    function: &CompiledFn<'a>,
    path: &'a str,
    unit_name: &'a str,
    line: u32,
    column: u32,
    instructions: &mut SliceVec<'a, '_, u8>,
    functions: &mut SliceVec<'a, '_, abi::Function>,
    lines: &mut SliceVec<'a, '_, LineEntry>,
    stack_size: &mut u32,
) -> Result<(), Diagnostic> {
    let code_off = u32::try_from(instructions.len()).map_err(|_| {
        diag(
            path,
            unit_name,
            line,
            column,
            format_args!("instruction section exceeds 4 GiB"),
        )
    })?;
    let line_base = u32::try_from(lines.len()).map_err(|_| {
        diag(
            path,
            unit_name,
            line,
            column,
            format_args!("line table is too large"),
        )
    })?;
    for entry in function.lines {
        lines
            .push(LineEntry {
                off: entry.off + code_off,
                unit: entry.unit,
                line: entry.line,
                column: entry.column,
            })
            .map_err(|_| arena_full(path, unit_name, line, column))?;
    }
    functions
        .push(abi::Function {
            code_off,
            frame_size: function.frame_size,
            operand_off: function.operand_off,
            line_base,
            name_off: abi::NO_NAME,
            param_count: function.param_count,
            has_destination: function.has_destination,
            _reserved: [0, 0],
        })
        .map_err(|_| arena_full(path, unit_name, line, column))?;
    instructions
        .extend_from_slice(function.code)
        .map_err(|_| arena_full(path, unit_name, line, column))?;
    *stack_size = stack_size.checked_add(function.frame_size).ok_or_else(|| {
        diag(
            path,
            unit_name,
            line,
            column,
            format_args!("stack size overflows"),
        )
    })?;
    Ok(())
}

/// Compiles the application unit and, when it imports `stdlib`, the embedded
/// library unit into one package (spec 0008, "One artifact per application").
///
/// Function table order is library first, then application, so an application
/// call and an intra-library call resolve to one index space. The application's
/// `main` is the entry point; the manifest keeps the application's unit name.
/// A `library` of `None` is how the host checks the library source in
/// isolation: the unit then needs no `main` and its first function is the
/// entry.
pub(crate) fn compile_units<'a>(
    application: &Unit<'a>,
    application_path: &'a str,
    library: Option<&Unit<'a>>,
    library_path: &'a str,
    bump: &Bump<'a>,
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    assemble_units(
        application,
        application_path,
        library,
        library_path,
        true,
        bump,
        code,
        manifest,
    )
}

/// Compiles one unit on its own without requiring `main`. This is the
/// host-side self-check for the embedded `stdlib` source (spec 0008, B13).
pub(crate) fn compile_library<'a>(
    unit: &Unit<'a>,
    path: &'a str,
    bump: &Bump<'a>,
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    assemble_units(unit, path, None, path, false, bump, code, manifest)
}

#[allow(clippy::too_many_arguments)]
fn assemble_units<'a>(
    application: &Unit<'a>,
    application_path: &'a str,
    library: Option<&Unit<'a>>,
    library_path: &'a str,
    is_application: bool,
    bump: &Bump<'a>,
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    check_uses(application, application_path, is_application)?;
    // One array, ring and map table serve both units: interning is
    // structural, so an `array[T, N]`, `ring[T, N]` or `map[K, V, N]` written
    // on either side of a call resolves to the same id.
    let mut arrays = SliceVec::<ArrayDef>::new(bump)
        .map_err(|_| arena_full(application_path, application.name, 1, 1))?;
    let mut rings = SliceVec::<RingDef>::new(bump)
        .map_err(|_| arena_full(application_path, application.name, 1, 1))?;
    let mut maps = SliceVec::<MapDef>::new(bump)
        .map_err(|_| arena_full(application_path, application.name, 1, 1))?;
    let app = build_tables(
        application,
        application_path,
        is_application,
        bump,
        &mut arrays,
        &mut rings,
        &mut maps,
    )?;
    // A library unit may not declare a stream endpoint: the endpoint name is
    // application-wide and the handle belongs in the application's state
    // prefix. `const in_stream`/`out_stream` parameters are allowed for
    // framing helpers.
    let lib = match library {
        Some(unit) => {
            check_uses(unit, library_path, false)?;
            Some(build_tables(
                unit,
                library_path,
                false,
                bump,
                &mut arrays,
                &mut rings,
                &mut maps,
            )?)
        }
        None => None,
    };

    let entry = if is_application {
        match find_sig(app.sigs, "main") {
            Some(index) => {
                let main_sig = app.sigs[index];
                let main_ok = main_sig.ret.is_none()
                    && main_sig.params.len() == 1
                    && main_sig.params[0].mode == ParamMode::Const
                    && main_sig.params[0].ty == Type::buffer(Kind::String, 1024);
                if !main_ok {
                    return Err(diag(
                        application_path,
                        app.name,
                        main_sig.line,
                        main_sig.column,
                        format_args!(
                            "main must be declared as `procedure main(args: const string[1024]);`"
                        ),
                    ));
                }
                index
            }
            None => {
                return Err(diag(
                    application_path,
                    app.name,
                    1,
                    1,
                    format_args!(
                        "missing `main` procedure; an application needs `procedure main(args: const string[1024]);`"
                    ),
                ))
            }
        }
    } else {
        0
    };
    let library_len = lib.as_ref().map_or(0, |tables| tables.sigs.len());
    let entry = u32::try_from(library_len + entry).map_err(|_| {
        diag(
            application_path,
            app.name,
            1,
            1,
            format_args!("function table is too large"),
        )
    })?;

    // One constant pool and one instruction section serve both units.
    let mut pool = ConstantPool::new(application_path, app.name, bump)?;
    let library_compiled = match &lib {
        Some(tables) => {
            // The library's `state` follows the application's in one region.
            let mut rebased = SliceVec::<StateVar<'a>>::new(bump)
                .map_err(|_| arena_full(library_path, tables.name, 1, 1))?;
            for var in tables.state_vars {
                rebased
                    .push(StateVar {
                        name: var.name,
                        ty: var.ty,
                        off: var.off + app.state_size,
                    })
                    .map_err(|_| arena_full(library_path, tables.name, 1, 1))?;
            }
            let rebased: &'a [StateVar<'a>] = rebased.into_slice();
            Some(compile_procs(
                library.expect("library tables imply a unit"),
                tables,
                library_path,
                0,
                None,
                rebased,
                &mut arrays,
                &mut rings,
                &mut maps,
                1,
                &mut pool,
                bump,
            )?)
        }
        None => None,
    };
    let library_sigs = lib.as_ref().map(|tables| tables.sigs);
    let app_compiled = compile_procs(
        application,
        &app,
        application_path,
        u32::try_from(library_len).unwrap_or(u32::MAX),
        library_sigs,
        app.state_vars,
        &mut arrays,
        &mut rings,
        &mut maps,
        0,
        &mut pool,
        bump,
    )?;

    let mut instructions =
        SliceVec::<u8>::new(bump).map_err(|_| arena_full(application_path, app.name, 1, 1))?;
    let mut functions = SliceVec::<abi::Function>::new(bump)
        .map_err(|_| arena_full(application_path, app.name, 1, 1))?;
    let mut lines = SliceVec::<LineEntry>::new(bump)
        .map_err(|_| arena_full(application_path, app.name, 1, 1))?;
    let mut stack_size = STACK_MARGIN;
    if let (Some(tables), Some(compiled)) = (&lib, library_compiled) {
        for (index, function) in compiled.iter().enumerate() {
            let proc = library.expect("library tables imply a unit").procs[index];
            append_function(
                function,
                library_path,
                tables.name,
                proc.line,
                proc.column,
                &mut instructions,
                &mut functions,
                &mut lines,
                &mut stack_size,
            )?;
        }
    }
    for (index, function) in app_compiled.iter().enumerate() {
        let proc = application.procs[index];
        append_function(
            function,
            application_path,
            app.name,
            proc.line,
            proc.column,
            &mut instructions,
            &mut functions,
            &mut lines,
            &mut stack_size,
        )?;
    }
    stack_size = align8(stack_size);
    if stack_size > MAX_CAPACITY {
        return Err(diag(
            application_path,
            app.name,
            1,
            1,
            format_args!("stack size {stack_size} exceeds the {MAX_CAPACITY}-byte cap"),
        ));
    }
    let state_size = app.state_size + lib.as_ref().map_or(0, |tables| tables.state_size);
    if state_size > MAX_CAPACITY {
        return Err(diag(
            application_path,
            app.name,
            1,
            1,
            format_args!("state size {state_size} exceeds the {MAX_CAPACITY}-byte cap"),
        ));
    }

    package::assemble(
        application_path,
        app.name,
        functions.as_slice(),
        instructions.as_slice(),
        pool.data.as_slice(),
        lines.as_slice(),
        app.endpoints,
        state_size,
        stack_size,
        entry,
        code,
        manifest,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_type<'a>(
    ty: &TypeRef<'a>,
    consts: &[Const<'a>],
    enums: &[EnumDef<'a>],
    records: &[RecordDef<'a>],
    sums: &[SumDef<'a>],
    arrays: &mut SliceVec<'a, '_, ArrayDef>,
    rings: &mut SliceVec<'a, '_, RingDef>,
    maps: &mut SliceVec<'a, '_, MapDef>,
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
    resolve_type_with(
        ty, consts, enums, records, sums, arrays, rings, maps, false, path, unit,
    )
}

/// Resolves a type. `allow_stream` is true only for a top-level `var` of an
/// application and for a parameter declaration; every other context rejects a
/// stream endpoint, because the handle is meaningful only in `state`.
#[allow(clippy::too_many_arguments)]
fn resolve_type_with<'a>(
    ty: &TypeRef<'a>,
    consts: &[Const<'a>],
    enums: &[EnumDef<'a>],
    records: &[RecordDef<'a>],
    sums: &[SumDef<'a>],
    arrays: &mut SliceVec<'a, '_, ArrayDef>,
    rings: &mut SliceVec<'a, '_, RingDef>,
    maps: &mut SliceVec<'a, '_, MapDef>,
    allow_stream: bool,
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
    if ty.name == "in_stream" || ty.name == "out_stream" {
        if !allow_stream {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!(
                    "`{}` is a stream endpoint; it may only be declared as a top-level `var` or a `const` parameter",
                    ty.name
                ),
            ));
        }
        let Some(expr) = ty.cap else {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("`{}` needs a capacity, e.g. `{}[64]`", ty.name, ty.name),
            ));
        };
        let value = eval_const(expr, consts, path, unit, 0)?;
        if !(1..=65536).contains(&value) {
            return Err(diag(
                path,
                unit,
                expr.line,
                expr.column,
                format_args!("stream capacity must be between 1 and 65536 bytes, found {value}"),
            ));
        }
        let kind = if ty.name == "in_stream" {
            Kind::InStream
        } else {
            Kind::OutStream
        };
        return Ok(Type::stream(kind, value as u32));
    }
    if ty.name == "map" {
        let Some(key_ref) = ty.elem else {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("`map` needs a key type, e.g. `map[int, string[8], 4]`"),
            ));
        };
        let Some(value_ref) = ty.value else {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("`map` needs a value type, e.g. `map[int, string[8], 4]`"),
            ));
        };
        let Some(count_expr) = ty.count else {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("`map` needs an entry count, e.g. `map[int, string[8], 4]`"),
            ));
        };
        let key = resolve_type(
            key_ref, consts, enums, records, sums, arrays, rings, maps, path, unit,
        )?;
        if !matches!(key.kind, Kind::Int | Kind::String | Kind::Bytes) {
            return Err(diag(
                path,
                unit,
                key_ref.line,
                key_ref.column,
                format_args!("a map key must be `int`, `string[M]` or `bytes[M]`, found `{key}`"),
            ));
        }
        if key.is_buffer() && key.cap == 0 {
            return Err(diag(
                path,
                unit,
                key_ref.line,
                key_ref.column,
                format_args!("a map key buffer needs a capacity of at least 1"),
            ));
        }
        let value = resolve_type(
            value_ref, consts, enums, records, sums, arrays, rings, maps, path, unit,
        )?;
        if value.is_buffer() && value.cap == 0 {
            return Err(diag(
                path,
                unit,
                value_ref.line,
                value_ref.column,
                format_args!("a map value buffer needs a capacity of at least 1"),
            ));
        }
        let count = eval_const(count_expr, consts, path, unit, 0)?;
        if count <= 0 {
            return Err(diag(
                path,
                unit,
                count_expr.line,
                count_expr.column,
                format_args!("map entry count must be a positive constant, found {count}"),
            ));
        }
        if count > MAX_CAPACITY as i64 {
            return Err(diag(
                path,
                unit,
                count_expr.line,
                count_expr.column,
                format_args!("map entry count {count} exceeds the {MAX_CAPACITY}-entry cap"),
            ));
        }
        return intern_map(
            maps,
            key,
            value,
            count as u32,
            ty.line,
            ty.column,
            path,
            unit,
        );
    }
    if matches!(ty.name, "array" | "ring") {
        let word = ty.name;
        let example = if word == "array" {
            "array[int, 4]"
        } else {
            "ring[int, 4]"
        };
        let Some(elem_ref) = ty.elem else {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("`{word}` needs an element type, e.g. `{example}`"),
            ));
        };
        let Some(count_expr) = ty.count else {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("`{word}` needs a count, e.g. `{example}`"),
            ));
        };
        let elem = resolve_type(
            elem_ref, consts, enums, records, sums, arrays, rings, maps, path, unit,
        )?;
        if elem.is_buffer() && elem.cap == 0 {
            return Err(diag(
                path,
                unit,
                elem_ref.line,
                elem_ref.column,
                format_args!("a `{word}` element buffer needs a capacity of at least 1"),
            ));
        }
        if elem.size() == 0 {
            return Err(diag(
                path,
                unit,
                elem_ref.line,
                elem_ref.column,
                format_args!("the `{word}` element type has no size"),
            ));
        }
        let count = eval_const(count_expr, consts, path, unit, 0)?;
        if count <= 0 {
            return Err(diag(
                path,
                unit,
                count_expr.line,
                count_expr.column,
                format_args!("{word} count must be a positive constant, found {count}"),
            ));
        }
        if count > MAX_CAPACITY as i64 {
            return Err(diag(
                path,
                unit,
                count_expr.line,
                count_expr.column,
                format_args!("{word} count {count} exceeds the {MAX_CAPACITY}-element cap"),
            ));
        }
        if word == "array" {
            return intern_array(arrays, elem, count as u32, ty.line, ty.column, path, unit);
        }
        return intern_ring(rings, elem, count as u32, ty.line, ty.column, path, unit);
    }
    if ty.elem.is_some() || ty.value.is_some() || ty.count.is_some() {
        return Err(diag(
            path,
            unit,
            ty.line,
            ty.column,
            format_args!("`{}` is not a parametric type", ty.name),
        ));
    }
    if let Some(id) = find_enum(enums, ty.name) {
        if ty.cap.is_some() {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("enum type `{}` does not take a capacity", ty.name),
            ));
        }
        return Ok(Type::enumeration(id));
    }
    if let Some(id) = find_record(records, ty.name) {
        if ty.cap.is_some() {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("record type `{}` does not take a capacity", ty.name),
            ));
        }
        let size = records[id as usize].size;
        return Ok(Type::record(id, size));
    }
    if let Some(id) = find_sum(sums, ty.name) {
        if ty.cap.is_some() {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("sum type `{}` does not take a capacity", ty.name),
            ));
        }
        let size = sums[id as usize].size;
        return Ok(Type::sum(id, size));
    }
    let kind = match ty.name {
        "int" => Kind::Int,
        "uint" => Kind::Uint,
        "byte" => Kind::Byte,
        "bool" => Kind::Bool,
        "f64" => Kind::Float,
        "string" => Kind::String,
        "bytes" => Kind::Bytes,
        other => {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("unknown type `{other}`"),
            ))
        }
    };
    if kind.is_buffer() {
        let cap = match ty.cap {
            Some(expr) => {
                let value = eval_const(expr, consts, path, unit, 0)?;
                if value < 0 {
                    return Err(diag(
                        path,
                        unit,
                        expr.line,
                        expr.column,
                        format_args!("capacity {value} is negative"),
                    ));
                }
                if value > MAX_CAPACITY as i64 {
                    return Err(diag(
                        path,
                        unit,
                        expr.line,
                        expr.column,
                        format_args!("capacity {value} exceeds the {MAX_CAPACITY}-byte cap"),
                    ));
                }
                value as u32
            }
            None => {
                return Err(diag(
                    path,
                    unit,
                    ty.line,
                    ty.column,
                    format_args!(
                        "`{}` needs a capacity, e.g. `{}[64]`",
                        kind.name(),
                        kind.name()
                    ),
                ))
            }
        };
        Ok(Type::buffer(kind, cap))
    } else {
        if ty.cap.is_some() {
            return Err(diag(
                path,
                unit,
                ty.line,
                ty.column,
                format_args!("scalar type `{}` does not take a capacity", kind.name()),
            ));
        }
        Ok(Type::scalar(kind))
    }
}

fn eval_const<'a>(
    expr: &Expr<'a>,
    consts: &[Const<'a>],
    path: &'a str,
    unit: &'a str,
    depth: u32,
) -> Result<i64, Diagnostic> {
    let error = |message: fmt::Arguments<'_>| diag(path, unit, expr.line, expr.column, message);
    if depth >= MAX_NESTING {
        return Err(error(format_args!(
            "nesting too deep (limit {MAX_NESTING})"
        )));
    }
    match expr.kind {
        ExprKind::Int(value) => Ok(value),
        ExprKind::Designator(designator) if designator.is_name() => {
            find_const(consts, designator.base).ok_or_else(|| {
                error(format_args!(
                    "unknown constant `{}` in a constant expression",
                    designator.base
                ))
            })
        }
        ExprKind::Unary(UnOp::Neg, inner) => {
            let value = eval_const(inner, consts, path, unit, depth + 1)?;
            value
                .checked_neg()
                .ok_or_else(|| error(format_args!("constant expression overflows")))
        }
        ExprKind::Binary(op, left, right) => {
            let a = eval_const(left, consts, path, unit, depth + 1)?;
            let b = eval_const(right, consts, path, unit, depth + 1)?;
            let overflow = || error(format_args!("constant expression overflows"));
            match op {
                BinOp::Add => a.checked_add(b).ok_or_else(overflow),
                BinOp::Sub => a.checked_sub(b).ok_or_else(overflow),
                BinOp::Mul => a.checked_mul(b).ok_or_else(overflow),
                BinOp::Div => {
                    if b == 0 {
                        return Err(error(format_args!(
                            "division by zero in constant expression"
                        )));
                    }
                    a.checked_div(b).ok_or_else(overflow)
                }
                BinOp::Mod => {
                    if b == 0 {
                        return Err(error(format_args!(
                            "division by zero in constant expression"
                        )));
                    }
                    a.checked_rem(b).ok_or_else(overflow)
                }
                _ => Err(error(format_args!(
                    "only `+`, `-`, `*`, `div` and `mod` are constant expressions"
                ))),
            }
        }
        _ => Err(error(format_args!("constant expression required"))),
    }
}

#[derive(Clone, Copy)]
struct Slot<'a> {
    name: &'a str,
    base: u32,
    ty: Type,
    writable: bool,
    is_ref: bool,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    label: usize,
    imm_pos: usize,
    next: usize,
    op_pos: usize,
}

#[derive(Debug, Clone, Copy)]
enum Target {
    LocalScalar { off: u32, ty: Type, is_ref: bool },
    StateScalar { off: u32, ty: Type },
    LocalBuffer { base: u32, ty: Type, is_ref: bool },
    StateBuffer { off: u32, ty: Type },
    LocalRecord { base: u32, ty: Type, is_ref: bool },
    StateRecord { off: u32, ty: Type },
    LocalSum { base: u32, ty: Type, is_ref: bool },
    StateSum { off: u32, ty: Type },
    LocalArray { base: u32, ty: Type, is_ref: bool },
    StateArray { off: u32, ty: Type },
    LocalRing { base: u32, ty: Type, is_ref: bool },
    StateRing { off: u32, ty: Type },
    LocalMap { base: u32, ty: Type, is_ref: bool },
    StateMap { off: u32, ty: Type },
}

/// Where a designator's storage starts, before any field path is applied.
#[derive(Debug, Clone, Copy)]
enum Base {
    /// A frame local. `is_ref` means the slot holds a call-scoped reference
    /// (a `const`/`var` parameter) instead of the value.
    Local { off: u32, is_ref: bool },
    /// A `state` region variable.
    State { off: u32 },
}

/// A resolved designator: a base variable plus a path of field selections and
/// element indexes.
///
/// `base_ty` is the declared type of the base variable; each path element
/// selects a record field or an array element. With an empty path this is the
/// variable itself.
#[derive(Debug, Clone, Copy)]
struct Place<'a> {
    base: Base,
    base_ty: Type,
    path: &'a [Access<'a>],
    ty: Type,
    writable: bool,
    /// True when the last path step is `[i]` on a buffer: the result is one
    /// `byte` (read with `load_u8`, written with `store_u8`), and this records
    /// the buffer's type for the capacity immediate.
    byte_buffer: Option<Type>,
}

struct FnCompiler<'a, 'b, 'p> {
    path: &'a str,
    unit: &'a str,
    sigs: &'b [Sig<'a>],
    consts: &'b [Const<'a>],
    state_vars: &'b [StateVar<'a>],
    enums: &'b [EnumDef<'a>],
    records: &'b [RecordDef<'a>],
    sums: &'b [SumDef<'a>],
    /// Every array type of this compilation, shared with the library unit.
    /// Codegen may intern a local's element type here, so the table stays
    /// mutable until both units are compiled.
    arrays: &'p mut SliceVec<'a, 'b, ArrayDef>,
    /// Every ring type of this compilation, shared with the library unit.
    rings: &'p mut SliceVec<'a, 'b, RingDef>,
    /// Every map type of this compilation, shared with the library unit.
    maps: &'p mut SliceVec<'a, 'b, MapDef>,
    /// Index of this unit's first function in the shared function table.
    call_base: u32,
    /// Top-level procedures of the imported `stdlib` unit, reachable only as
    /// `stdlib.<name>` from the application. `None` inside the stdlib itself,
    /// which resolves its own names unqualified.
    library_sigs: Option<&'b [Sig<'a>]>,
    /// Line-table unit id (0 application, 1 stdlib).
    unit_id: u32,
    pool: &'p mut ConstantPool<'a, 'b>,
    current: Sig<'a>,
    bump: &'b Bump<'a>,
    code: SliceVec<'a, 'b, u8>,
    slots: SliceVec<'a, 'b, Slot<'a>>,
    next_local: u32,
    depth: i32,
    max_depth: i32,
    /// Recursive codegen depth (spec 0002, Q42). The parser bounds nesting
    /// while it descends, but a left-associative operator chain is parsed by a
    /// loop and builds a deep AST, so `expr_scalar` bounds its own walk too.
    nesting: u32,
    labels: SliceVec<'a, 'b, Option<usize>>,
    pending: SliceVec<'a, 'b, Pending>,
    lines: SliceVec<'a, 'b, LineEntry>,
}

/// Renders a resolved type for a diagnostic. [`Type`]'s own `Display` carries
/// only the enum id, so codegen resolves the name through this table.
struct TypeName<'a, 'b, 'c> {
    ty: Type,
    enums: &'b [EnumDef<'a>],
    records: &'b [RecordDef<'a>],
    sums: &'b [SumDef<'a>],
    arrays: &'c [ArrayDef],
    rings: &'c [RingDef],
    maps: &'c [MapDef],
}

impl fmt::Display for TypeName<'_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ty.kind {
            Kind::Enum(id) => match self.enums.get(id as usize) {
                Some(enumeration) => f.write_str(enumeration.name),
                None => f.write_str("enum"),
            },
            Kind::Record(id) => match self.records.get(id as usize) {
                Some(record) => f.write_str(record.name),
                None => f.write_str("record"),
            },
            Kind::Sum(id) => match self.sums.get(id as usize) {
                Some(sum) => f.write_str(sum.name),
                None => f.write_str("sum"),
            },
            Kind::Array(id) => match self.arrays.get(id as usize) {
                Some(array) => write!(
                    f,
                    "array[{}, {}]",
                    TypeName {
                        ty: array.elem,
                        enums: self.enums,
                        records: self.records,
                        sums: self.sums,
                        arrays: self.arrays,
                        rings: self.rings,
                        maps: self.maps,
                    },
                    array.count
                ),
                None => f.write_str("array"),
            },
            Kind::Ring(id) => match self.rings.get(id as usize) {
                Some(ring) => write!(
                    f,
                    "ring[{}, {}]",
                    TypeName {
                        ty: ring.elem,
                        enums: self.enums,
                        records: self.records,
                        sums: self.sums,
                        arrays: self.arrays,
                        rings: self.rings,
                        maps: self.maps,
                    },
                    ring.count
                ),
                None => f.write_str("ring"),
            },
            Kind::Map(id) => match self.maps.get(id as usize) {
                Some(map) => write!(
                    f,
                    "map[{}, {}, {}]",
                    TypeName {
                        ty: map.key,
                        enums: self.enums,
                        records: self.records,
                        sums: self.sums,
                        arrays: self.arrays,
                        rings: self.rings,
                        maps: self.maps,
                    },
                    TypeName {
                        ty: map.value,
                        enums: self.enums,
                        records: self.records,
                        sums: self.sums,
                        arrays: self.arrays,
                        rings: self.rings,
                        maps: self.maps,
                    },
                    map.count
                ),
                None => f.write_str("map"),
            },
            _ => write!(f, "{}", self.ty),
        }
    }
}

/// Where a buffer construction writes: a whole buffer variable/field, or the
/// payload area of a sum type. The latter is referenced at `sum + 16` (its
/// `[len][data]` image starts at the payload offset 8), so `clear` and `append`
/// find the length word at `sum + 8`.
#[derive(Clone, Copy)]
enum BufferDest<'a> {
    Place(Place<'a>),
    SumPayload { sum: Place<'a>, ty: Type },
}

impl BufferDest<'_> {
    fn ty(self) -> Type {
        match self {
            BufferDest::Place(place) => place.ty,
            BufferDest::SumPayload { ty, .. } => ty,
        }
    }
}

impl<'a, 'b, 'p> FnCompiler<'a, 'b, 'p> {
    /// Renders a type for a diagnostic. [`Type`]'s own `Display` knows only
    /// that an enum is an enum; this resolves the name through the table.
    fn type_name(&self, ty: Type) -> TypeName<'a, '_, '_> {
        TypeName {
            ty,
            enums: self.enums,
            records: self.records,
            sums: self.sums,
            arrays: self.arrays.as_slice(),
            rings: self.rings.as_slice(),
            maps: self.maps.as_slice(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        path: &'a str,
        unit: &'a str,
        sigs: &'b [Sig<'a>],
        consts: &'b [Const<'a>],
        state_vars: &'b [StateVar<'a>],
        enums: &'b [EnumDef<'a>],
        records: &'b [RecordDef<'a>],
        sums: &'b [SumDef<'a>],
        arrays: &'p mut SliceVec<'a, 'b, ArrayDef>,
        rings: &'p mut SliceVec<'a, 'b, RingDef>,
        maps: &'p mut SliceVec<'a, 'b, MapDef>,
        call_base: u32,
        library_sigs: Option<&'b [Sig<'a>]>,
        unit_id: u32,
        pool: &'p mut ConstantPool<'a, 'b>,
        current: Sig<'a>,
        bump: &'b Bump<'a>,
    ) -> Result<Self, Diagnostic> {
        let line = current.line;
        let column = current.column;
        let code = SliceVec::new(bump).map_err(|_| arena_full(path, unit, line, column))?;
        let slots = SliceVec::new(bump).map_err(|_| arena_full(path, unit, line, column))?;
        let labels = SliceVec::new(bump).map_err(|_| arena_full(path, unit, line, column))?;
        let pending = SliceVec::new(bump).map_err(|_| arena_full(path, unit, line, column))?;
        let lines = SliceVec::new(bump).map_err(|_| arena_full(path, unit, line, column))?;
        Ok(FnCompiler {
            path,
            unit,
            sigs,
            consts,
            state_vars,
            enums,
            records,
            sums,
            arrays,
            rings,
            maps,
            call_base,
            library_sigs,
            unit_id,
            pool,
            current,
            bump,
            code,
            slots,
            next_local: 0,
            depth: 0,
            max_depth: 0,
            nesting: 0,
            labels,
            pending,
            lines,
        })
    }

    fn arena_error(&self, line: u32, column: u32) -> Diagnostic {
        arena_full(self.path, self.unit, line, column)
    }

    fn err(&self, line: u32, column: u32, message: fmt::Arguments<'_>) -> Diagnostic {
        diag(self.path, self.unit, line, column, message)
    }

    fn err_at(&self, expr: &Expr<'_>, message: fmt::Arguments<'_>) -> Diagnostic {
        self.err(expr.line, expr.column, message)
    }

    /// Enters one recursive expression level, or rejects the source when the
    /// fixed limit is reached. The caller must pair every success with
    /// [`Self::leave_nesting`], including on error paths.
    fn enter_nesting(&mut self, line: u32, column: u32) -> Result<(), Diagnostic> {
        if self.nesting >= MAX_NESTING {
            return Err(self.err(
                line,
                column,
                format_args!("nesting too deep (limit {MAX_NESTING})"),
            ));
        }
        self.nesting += 1;
        Ok(())
    }

    fn leave_nesting(&mut self) {
        debug_assert!(self.nesting > 0);
        self.nesting -= 1;
    }

    fn internal(&self, message: &str) -> Diagnostic {
        diag(
            self.path,
            self.unit,
            self.current.line,
            self.current.column,
            format_args!(
                "internal compiler error in `{}`: {message}",
                self.current.name
            ),
        )
    }

    fn code_extend(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
        let line = self.current.line;
        let column = self.current.column;
        match self.code.extend_from_slice(bytes) {
            Ok(()) => Ok(()),
            Err(_) => Err(arena_full(self.path, self.unit, line, column)),
        }
    }

    fn compile_procedure(&mut self, proc: &ProcDecl<'a>) -> Result<(), Diagnostic> {
        // Record and sum parameters are call-scoped references like buffers. A
        // `value` parameter is a copy: the slot holds the caller's reference
        // and the prologue copies the whole value into a local area, so the
        // callee can assign without touching the caller (spec 0003).
        let mut copies = SliceVec::<(usize, u32)>::new(self.bump)
            .map_err(|_| self.arena_error(self.current.line, self.current.column))?;
        for (index, param) in self.current.params.iter().enumerate() {
            let base = self.next_local;
            self.next_local += 8;
            let aggregate = param.ty.is_record()
                || param.ty.is_sum()
                || param.ty.is_array()
                || param.ty.is_ring()
                || param.ty.is_map();
            let is_ref = param.mode == ParamMode::Var
                || param.ty.is_buffer()
                || param.ty.is_stream()
                || (aggregate && param.mode != ParamMode::Value);
            let mut slot_base = base;
            if aggregate && param.mode == ParamMode::Value {
                let copy_base = self.next_local;
                self.next_local += param.ty.size();
                if self.next_local > MAX_CAPACITY {
                    return Err(self.err(
                        self.current.line,
                        self.current.column,
                        format_args!(
                            "procedure `{}` needs more than {MAX_CAPACITY} bytes of locals",
                            self.current.name
                        ),
                    ));
                }
                slot_base = copy_base;
                copies
                    .push((index, base))
                    .map_err(|_| self.arena_error(self.current.line, self.current.column))?;
            }
            let slot = Slot {
                name: param.name,
                base: slot_base,
                ty: param.ty,
                writable: param.mode != ParamMode::Const,
                is_ref,
            };
            self.slots
                .push(slot)
                .map_err(|_| self.arena_error(self.current.line, self.current.column))?;
        }
        // Prologue: copy each `value` record/sum parameter out of the caller's
        // value, so all later accesses use the local copy.
        for index in 0..copies.len() {
            let (parameter, copied) = copies
                .get(index)
                .ok_or_else(|| self.internal("missing copy slot"))?;
            let slot = self
                .slot(self.current.params[parameter].name)
                .ok_or_else(|| self.internal("missing parameter slot"))?;
            self.aggregate_copy_from_ref(slot.base, copied, slot.ty)?;
        }
        for local in proc.locals {
            if self.slot(local.name).is_some() {
                return Err(self.err(
                    local.line,
                    local.column,
                    format_args!("duplicate declaration of `{}`", local.name),
                ));
            }
            let ty = resolve_type(
                &local.ty,
                self.consts,
                self.enums,
                self.records,
                self.sums,
                &mut *self.arrays,
                &mut *self.rings,
                &mut *self.maps,
                self.path,
                self.unit,
            )?;
            if ty.is_buffer() && ty.cap == 0 {
                return Err(self.err(
                    local.line,
                    local.column,
                    format_args!(
                        "buffer variable `{}` needs a capacity of at least 1",
                        local.name
                    ),
                ));
            }
            let base = self.next_local;
            self.next_local += ty.size();
            if self.next_local > MAX_CAPACITY {
                return Err(self.err(
                    local.line,
                    local.column,
                    format_args!(
                        "procedure `{}` needs more than {MAX_CAPACITY} bytes of locals",
                        proc.name
                    ),
                ));
            }
            let slot = Slot {
                name: local.name,
                base,
                ty,
                writable: true,
                is_ref: false,
            };
            self.slots
                .push(slot)
                .map_err(|_| self.arena_error(local.line, local.column))?;
        }

        for stmt in proc.body {
            self.stmt(stmt)?;
        }
        if self.current.ret.is_some() && !proc.body.last().is_some_and(Self::closes) {
            return Err(self.err(
                self.current.line,
                self.current.column,
                format_args!(
                    "function `{}` must end with `return value;` on every path",
                    self.current.name
                ),
            ));
        }
        if self.depth != 0 {
            return Err(self.internal("operand stack is not empty at the end of a procedure"));
        }
        self.simple(opcodes::RET)?;
        self.finish_jumps()?;
        Ok(())
    }

    /// True when a statement always leaves the enclosing subprogram, which is
    /// what a `function` needs at the end of its body.
    fn closes(stmt: &Stmt<'_>) -> bool {
        match stmt {
            Stmt::Return { .. } => true,
            Stmt::Block(body) => body.last().is_some_and(Self::closes),
            Stmt::If {
                then_branch,
                else_branch: Some(else_branch),
                ..
            } => Self::closes(then_branch) && Self::closes(else_branch),
            _ => false,
        }
    }

    fn finish(self) -> Result<CompiledFn<'a>, Diagnostic> {
        let line = self.current.line;
        let column = self.current.column;
        let proc_name = self.current.name;
        let operand_off = FRAME_HEADER
            .checked_add(self.next_local)
            .ok_or_else(|| self.internal("frame size overflows"))?;
        let operands = u32::try_from(self.max_depth)
            .map_err(|_| self.internal("operand depth overflows"))?
            .checked_mul(8)
            .ok_or_else(|| self.internal("frame size overflows"))?;
        let frame_size = operand_off
            .checked_add(operands)
            .ok_or_else(|| self.internal("frame size overflows"))?;
        if frame_size > MAX_CAPACITY {
            return Err(diag(
                self.path,
                self.unit,
                line,
                column,
                format_args!(
                    "procedure `{proc_name}` needs a {frame_size}-byte frame, over the {MAX_CAPACITY}-byte cap"
                ),
            ));
        }
        let param_count = self.current.params.len() as u8;
        Ok(CompiledFn {
            code: self.code.into_slice(),
            frame_size,
            operand_off,
            param_count,
            has_destination: self.current.ret.is_some(),
            lines: self.lines.into_slice(),
        })
    }

    fn slot(&self, name: &str) -> Option<Slot<'a>> {
        self.slots
            .as_slice()
            .iter()
            .find(|s| s.name == name)
            .copied()
    }

    // ---- emission ------------------------------------------------------

    fn word(&mut self, op: u8, a: u8, b: u8, c: u8) -> Result<usize, Diagnostic> {
        let pos = self.code.len();
        self.code_extend(&[op, a, b, c])?;
        Ok(pos)
    }

    fn imm32(&mut self, value: u32) -> Result<(), Diagnostic> {
        self.code_extend(&value.to_le_bytes())
    }

    fn simple(&mut self, op: u8) -> Result<(), Diagnostic> {
        self.word(op, 0, 0, 0).map(|_| ())
    }

    fn one(&mut self, op: u8, imm: u32) -> Result<(), Diagnostic> {
        self.word(op, 0, 0, 0)?;
        self.imm32(imm)
    }

    fn push(&mut self) {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.max_depth = self.depth;
        }
    }

    fn pop(&mut self, count: i32) -> Result<(), Diagnostic> {
        if self.depth < count {
            return Err(self.internal("operand stack underflow"));
        }
        self.depth -= count;
        Ok(())
    }

    fn mark(&mut self, line: u32, column: u32) -> Result<(), Diagnostic> {
        let off = self.code.len() as u32;
        if self.lines.last().is_none_or(|entry| entry.off != off) {
            let entry = LineEntry {
                off,
                unit: self.unit_id,
                line,
                column,
            };
            let at_line = self.current.line;
            let at_column = self.current.column;
            self.lines
                .push(entry)
                .map_err(|_| self.arena_error(at_line, at_column))?;
        }
        Ok(())
    }

    fn label(&mut self) -> Result<usize, Diagnostic> {
        let at_line = self.current.line;
        let at_column = self.current.column;
        self.labels
            .push(None)
            .map_err(|_| self.arena_error(at_line, at_column))?;
        Ok(self.labels.len() - 1)
    }

    fn bind(&mut self, label: usize) {
        self.labels.set(label, Some(self.code.len()));
    }

    fn jump(&mut self, op: u8, label: usize, pops: bool) -> Result<(), Diagnostic> {
        let pos = self.word(op, 0, 0, 0)?;
        self.imm32(0)?;
        if pops {
            self.pop(1)?;
        }
        let pending = Pending {
            label,
            imm_pos: pos + 4,
            next: self.code.len(),
            op_pos: pos,
        };
        let at_line = self.current.line;
        let at_column = self.current.column;
        self.pending
            .push(pending)
            .map_err(|_| self.arena_error(at_line, at_column))
    }

    fn finish_jumps(&mut self) -> Result<(), Diagnostic> {
        let count = self.pending.len();
        for index in 0..count {
            let pending = self.pending.get(index).expect("pending entry exists");
            let target = match self.labels.get(pending.label) {
                Some(Some(target)) => target,
                _ => return Err(self.internal("unbound label")),
            };
            let delta = target as i64 - pending.next as i64;
            if delta % 4 != 0 {
                return Err(self.internal("jump target is not word aligned"));
            }
            let words = delta / 4;
            if words < i32::MIN as i64 || words > i32::MAX as i64 {
                return Err(self.internal("jump offset overflows"));
            }
            self.code.as_mut_slice()[pending.imm_pos..pending.imm_pos + 4]
                .copy_from_slice(&(words as i32).to_le_bytes());
            if target < pending.op_pos {
                // Explicit poll hint on backward branches (spec 0008, B3).
                self.code.as_mut_slice()[pending.op_pos + 3] = 1;
            }
        }
        Ok(())
    }

    fn const_i64(&mut self, value: i64) -> Result<(), Diagnostic> {
        self.simple(opcodes::CONST)?;
        self.imm32(value as u32)?;
        self.imm32((value >> 32) as u32)?;
        self.push();
        Ok(())
    }

    // ---- statements ----------------------------------------------------

    fn stmt(&mut self, stmt: &'a Stmt<'a>) -> Result<(), Diagnostic> {
        match *stmt {
            Stmt::Assign {
                target,
                value,
                line,
                column,
            } => {
                self.mark(line, column)?;
                self.assign_designator(&target, value, line, column)
            }
            Stmt::Call(call) => {
                self.mark(call.line, call.column)?;
                let result = self.call(&call)?;
                if result.is_some() {
                    self.simple(opcodes::DROP)?;
                    self.pop(1)?;
                }
                Ok(())
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let start_depth = self.depth;
                match else_branch {
                    Some(else_branch) => {
                        let else_label = self.label()?;
                        let end_label = self.label()?;
                        self.condition(cond)?;
                        self.jump(opcodes::JZ, else_label, true)?;
                        self.stmt(then_branch)?;
                        self.jump(opcodes::JMP, end_label, false)?;
                        self.bind(else_label);
                        self.stmt(else_branch)?;
                        self.bind(end_label);
                    }
                    None => {
                        let end_label = self.label()?;
                        self.condition(cond)?;
                        self.jump(opcodes::JZ, end_label, true)?;
                        self.stmt(then_branch)?;
                        self.bind(end_label);
                    }
                }
                if self.depth != start_depth {
                    return Err(self.internal("branches leave different operand depths"));
                }
                Ok(())
            }
            Stmt::While { cond, body } => {
                let start_depth = self.depth;
                let start = self.label()?;
                let end = self.label()?;
                self.bind(start);
                self.condition(cond)?;
                self.jump(opcodes::JZ, end, true)?;
                self.stmt(body)?;
                self.jump(opcodes::JMP, start, false)?;
                self.bind(end);
                if self.depth != start_depth {
                    return Err(self.internal("loop leaves a different operand depth"));
                }
                Ok(())
            }
            Stmt::Case { value, arms } => {
                self.mark(value.line, value.column)?;
                self.case_stmt(value, arms)
            }
            Stmt::Return {
                value,
                line,
                column,
            } => {
                self.mark(line, column)?;
                match (self.current.ret, value) {
                    (Some(expected), Some(expr)) => {
                        let found = self.expr_scalar(expr)?;
                        if found != expected {
                            let expected = self.type_name(expected);
                            let found = self.type_name(found);
                            return Err(self.err_at(
                                expr,
                                format_args!(
                                    "function `{}` returns `{expected}`, found `{found}`",
                                    self.current.name
                                ),
                            ));
                        }
                    }
                    (Some(_), None) => {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("function `{}` must return a value", self.current.name),
                        ))
                    }
                    (None, Some(_)) => {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("`return` with a value is only allowed in a function"),
                        ))
                    }
                    (None, None) => {}
                }
                self.simple(opcodes::RET)?;
                // The interpreter restores the caller's operand top; dead
                // statements after this point still compile with an empty
                // operand stack.
                self.depth = 0;
                Ok(())
            }
            Stmt::Block(stmts) => {
                for stmt in stmts {
                    self.stmt(stmt)?;
                }
                Ok(())
            }
        }
    }

    fn condition(&mut self, expr: &'a Expr<'a>) -> Result<(), Diagnostic> {
        let ty = self.expr_scalar(expr)?;
        if ty.kind != Kind::Bool {
            return Err(self.err_at(expr, format_args!("condition must be `bool`, found `{ty}`")));
        }
        Ok(())
    }

    /// Compiles `case value of ... end`.
    ///
    /// The scrutinee is a variable or field of a sum type (a designator, so it
    /// can be referenced repeatedly without a temporary). Every variant must
    /// have exactly one arm; a missing, duplicate or unknown variant is a
    /// compile error. An arm's payload is copied into a fresh local for the
    /// arm's scope, so a buffer payload is an ordinary writable copy and a
    /// scalar payload an ordinary local (spec 0003, "Sum types").
    fn case_stmt(
        &mut self,
        value: &'a Expr<'a>,
        arms: &'a [CaseArm<'a>],
    ) -> Result<(), Diagnostic> {
        let start_depth = self.depth;
        let Some(designator) = Self::designator(value) else {
            return Err(self.err_at(
                value,
                format_args!("a `case` subject must be a variable, field or element"),
            ));
        };
        let place = self.place(designator.base, designator.path, value.line, value.column)?;
        let Some(sum) = sum_of(self.sums, place.ty) else {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                value,
                format_args!("`case` needs a sum value, found `{found}`"),
            ));
        };

        // Validate the arms before emitting: variant membership, one arm per
        // variant, and a binding exactly when the variant has a payload.
        for variant in sum.variants {
            let mut arms_for_variant = 0u32;
            for arm in arms {
                if arm.variant == variant.name {
                    arms_for_variant += 1;
                }
            }
            if arms_for_variant == 0 {
                return Err(self.err_at(
                    value,
                    format_args!(
                        "non-exhaustive `case`: `{}` has no arm for variant `{}`",
                        sum.name, variant.name
                    ),
                ));
            }
            if arms_for_variant > 1 {
                return Err(self.err_at(
                    value,
                    format_args!(
                        "duplicate arm for variant `{}`; each variant needs exactly one arm",
                        variant.name
                    ),
                ));
            }
        }
        for arm in arms {
            let Some(variant) = variant_of(&sum, arm.variant) else {
                if let Some((owner, _)) = self.sum_variant_owner(arm.variant) {
                    return Err(self.err(
                        arm.line,
                        arm.column,
                        format_args!(
                            "`{}` has no variant `{}`; it belongs to `{owner}`",
                            sum.name, arm.variant
                        ),
                    ));
                }
                return Err(self.err(
                    arm.line,
                    arm.column,
                    format_args!("`{}` has no variant `{}`", sum.name, arm.variant),
                ));
            };
            match (variant.payload, arm.binding) {
                (Some(_), None) => {
                    return Err(self.err(
                        arm.line,
                        arm.column,
                        format_args!(
                            "variant `{}` carries a payload; bind it as `{}(name)`",
                            variant.name, variant.name
                        ),
                    ))
                }
                (None, Some(_)) => {
                    return Err(self.err(
                        arm.line,
                        arm.column,
                        format_args!(
                            "variant `{}` carries no payload; write `{}` without a binding",
                            variant.name, variant.name
                        ),
                    ))
                }
                _ => {}
            }
            if let Some(binding) = arm.binding {
                if self.slot(binding).is_some() {
                    return Err(self.err(
                        arm.line,
                        arm.column,
                        format_args!(
                            "duplicate declaration of `{binding}`; the payload binding is a new local"
                        ),
                    ));
                }
            }
        }

        // One payload scratch area serves every arm: arms are mutually
        // exclusive, so the largest payload covers each binding.
        let largest = sum
            .variants
            .iter()
            .filter_map(|variant| variant.payload)
            .map(Type::size)
            .max()
            .unwrap_or(0);
        let payload_base = self.next_local;
        self.next_local = self
            .next_local
            .checked_add(largest)
            .ok_or_else(|| self.err_at(value, format_args!("`case` payload area overflows")))?;
        if self.next_local > MAX_CAPACITY {
            return Err(self.err_at(
                value,
                format_args!(
                    "procedure `{}` needs more than {MAX_CAPACITY} bytes of locals",
                    self.current.name
                ),
            ));
        }

        let end = self.label()?;
        for arm in arms {
            let variant = variant_of(&sum, arm.variant)
                .ok_or_else(|| self.internal("validated arm has no variant"))?;
            let next = self.label()?;
            let ordinal = sum
                .variants
                .iter()
                .position(|candidate| candidate.name == variant.name)
                .ok_or_else(|| self.internal("variant is not a member of its sum"))?;

            // if tag <> ordinal then next
            self.push_value_ref(&place)?;
            self.simple(opcodes::LOAD_REF)?;
            self.const_i64(ordinal as i64)?;
            self.simple(opcodes::EQ)?;
            self.pop(1)?;
            self.jump(opcodes::JZ, next, true)?;

            let slot_count = self.slots.len();
            if let (Some(payload_ty), Some(binding)) = (variant.payload, arm.binding) {
                self.one(opcodes::REF_LOCAL, payload_base)?;
                self.push();
                self.push_value_ref(&place)?;
                self.one(opcodes::REF_OFFSET, 8)?;
                self.const_i64(payload_ty.size() as i64)?;
                self.simple(opcodes::COPY_REFS)?;
                self.pop(3)?;
                self.slots
                    .push(Slot {
                        name: binding,
                        base: payload_base,
                        ty: payload_ty,
                        writable: true,
                        is_ref: false,
                    })
                    .map_err(|_| self.arena_error(arm.line, arm.column))?;
            }
            for stmt in arm.body {
                self.stmt(stmt)?;
            }
            self.slots.truncate(slot_count);
            if self.depth != start_depth {
                return Err(self.internal("a `case` arm leaves a different operand depth"));
            }
            self.jump(opcodes::JMP, end, false)?;
            self.bind(next);
        }
        self.bind(end);
        if self.depth != start_depth {
            return Err(self.internal("`case` leaves a different operand depth"));
        }
        Ok(())
    }

    /// Assigns a value to a designator.
    ///
    /// A bare name takes the direct scalar/state store; any field or element
    /// path is resolved to a [`Place`] and dispatched by its type. The base
    /// must be writable, so a `const` parameter and everything reachable
    /// through it is read-only.
    fn assign_designator(
        &mut self,
        target: &Designator<'a>,
        value: &'a Expr<'a>,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        if target.path.is_empty() {
            return self.assign_name(target.base, value, line, column);
        }
        let place = self.place(target.base, target.path, line, column)?;
        if !place.writable {
            let what = if target
                .path
                .iter()
                .any(|access| matches!(access, Access::Field(_)))
            {
                "field"
            } else {
                "element"
            };
            return Err(self.err(
                line,
                column,
                format_args!(
                    "cannot assign to a {what} of `const` parameter `{}`",
                    target.base
                ),
            ));
        }
        if let Some(buffer_ty) = place.byte_buffer {
            return self.assign_byte(&place, buffer_ty, value, line, column);
        }
        if place.ty.is_record() {
            return self.assign_record(&place, value);
        }
        if place.ty.is_sum() {
            return self.assign_sum(&place, value);
        }
        if place.ty.is_array() {
            return self.assign_array(&place, value);
        }
        if place.ty.is_ring() {
            return self.assign_ring(&place, value);
        }
        if place.ty.is_map() {
            return self.assign_map(&place, value);
        }
        if place.ty.is_buffer() {
            return self.buffer_assign(BufferDest::Place(place), Some(target), value);
        }
        self.push_value_ref(&place)?;
        let value_ty = self.expr_scalar(value)?;
        self.check_exact(place.ty, value_ty, value)?;
        self.simple(opcodes::STORE_REF)?;
        self.pop(2)?;
        Ok(())
    }

    fn assign_name(
        &mut self,
        name: &'a str,
        value: &'a Expr<'a>,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        let target = self.resolve_target(name, line, column)?;
        match target {
            Target::LocalScalar {
                off,
                ty,
                is_ref: false,
            } => {
                let value_ty = self.expr_scalar(value)?;
                self.check_exact(ty, value_ty, value)?;
                self.one(opcodes::STORE_LOCAL, off)?;
                self.pop(1)?;
            }
            Target::LocalScalar {
                off,
                ty,
                is_ref: true,
            } => {
                // A `var` scalar parameter aliases the caller's variable: push
                // the reference first, then the value, then store through it.
                self.one(opcodes::LOAD_LOCAL, off)?;
                self.push();
                let value_ty = self.expr_scalar(value)?;
                self.check_exact(ty, value_ty, value)?;
                self.simple(opcodes::STORE_REF)?;
                self.pop(2)?;
            }
            Target::StateScalar { off, ty } => {
                let value_ty = self.expr_scalar(value)?;
                self.check_exact(ty, value_ty, value)?;
                self.one(opcodes::STORE_STATE, off)?;
                self.pop(1)?;
            }
            Target::LocalBuffer { .. } | Target::StateBuffer { .. } => {
                let place = self.place_of_target(&target);
                let home = Designator {
                    base: name,
                    path: &[],
                };
                self.buffer_assign(BufferDest::Place(place), Some(&home), value)?;
            }
            Target::LocalRecord { .. } | Target::StateRecord { .. } => {
                let place = self.place_of_target(&target);
                self.assign_record(&place, value)?;
            }
            Target::LocalSum { .. } | Target::StateSum { .. } => {
                let place = self.place_of_target(&target);
                self.assign_sum(&place, value)?;
            }
            Target::LocalArray { .. } | Target::StateArray { .. } => {
                let place = self.place_of_target(&target);
                self.assign_array(&place, value)?;
            }
            Target::LocalRing { .. } | Target::StateRing { .. } => {
                let place = self.place_of_target(&target);
                self.assign_ring(&place, value)?;
            }
            Target::LocalMap { .. } | Target::StateMap { .. } => {
                let place = self.place_of_target(&target);
                self.assign_map(&place, value)?;
            }
        }
        Ok(())
    }

    /// Assigns one byte of a buffer element: `buf[i] := value`, including an
    /// element reached through a record field or array (`rec.s[i]`,
    /// `rows[i][j]`).
    ///
    /// Only `bytes` is writable by index. Writing into a `string` would break
    /// the UTF-8 invariant one byte at a time, so it is a compile error
    /// (spec 0003). The declared capacity is carried in the instruction; the
    /// interpreter extends the runtime length to `index + 1` when the index is
    /// at or past the current length.
    fn assign_byte(
        &mut self,
        place: &Place<'a>,
        buffer_ty: Type,
        value: &'a Expr<'a>,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        if buffer_ty.kind != Kind::Bytes {
            let found = self.type_name(buffer_ty);
            return Err(self.err(
                line,
                column,
                format_args!(
                    "cannot assign into `{found}` by index; `string` is read-only by index (UTF-8)"
                ),
            ));
        }
        let Some(Access::Index(index)) = place.path.last() else {
            return Err(self.internal("byte place does not end in an index"));
        };
        let prefix = Place {
            path: &place.path[..place.path.len() - 1],
            ty: buffer_ty,
            byte_buffer: None,
            ..*place
        };
        self.push_buffer_data_ref(&prefix)?;
        self.check_index_type(index)?;
        let value_ty = self.expr_scalar(value)?;
        if value_ty.kind != Kind::Byte {
            return Err(self.err_at(
                value,
                format_args!(
                    "a buffer element needs a `byte`, found `{value_ty}`; convert the value with `byte(x)`"
                ),
            ));
        }
        self.one(opcodes::STORE_U8, buffer_ty.cap)?;
        self.pop(3)?;
        Ok(())
    }

    /// Checks that an index expression is an integer type and emits it.
    ///
    /// Negative and out-of-range values are not compile errors: the interpreter
    /// bounds-checks every index at run time (spec 0008, B4).
    fn check_index_type(&mut self, index: &'a Expr<'a>) -> Result<(), Diagnostic> {
        let ty = self.expr_scalar(index)?;
        if !matches!(ty.kind, Kind::Int | Kind::Uint | Kind::Byte) {
            return Err(self.err_at(
                index,
                format_args!("a buffer index must be `int`, `uint` or `byte`, found `{ty}`"),
            ));
        }
        Ok(())
    }

    fn check_exact(&self, expected: Type, found: Type, expr: &Expr<'_>) -> Result<(), Diagnostic> {
        if expected == found {
            Ok(())
        } else {
            let expected = self.type_name(expected);
            let found = self.type_name(found);
            Err(self.err_at(
                expr,
                format_args!("type mismatch: expected `{expected}`, found `{found}`"),
            ))
        }
    }

    fn resolve_target(&self, name: &str, line: u32, column: u32) -> Result<Target, Diagnostic> {
        if let Some(slot) = self.slot(name) {
            if slot.ty.is_stream() {
                return Err(self.err(
                    line,
                    column,
                    format_args!(
                        "stream endpoint `{name}` cannot be assigned; it is an opaque handle"
                    ),
                ));
            }
            if !slot.writable {
                return Err(self.err(
                    line,
                    column,
                    format_args!("cannot assign to `const` parameter `{name}`"),
                ));
            }
            return Ok(if slot.ty.is_buffer() {
                Target::LocalBuffer {
                    base: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            } else if slot.ty.is_record() {
                Target::LocalRecord {
                    base: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            } else if slot.ty.is_sum() {
                Target::LocalSum {
                    base: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            } else if slot.ty.is_array() {
                Target::LocalArray {
                    base: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            } else if slot.ty.is_ring() {
                Target::LocalRing {
                    base: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            } else if slot.ty.is_map() {
                Target::LocalMap {
                    base: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            } else {
                Target::LocalScalar {
                    off: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            });
        }
        if let Some(var) = find_state(self.state_vars, name) {
            if var.ty.is_stream() {
                return Err(self.err(
                    line,
                    column,
                    format_args!(
                        "stream endpoint `{name}` cannot be assigned; it is an opaque handle"
                    ),
                ));
            }
            return Ok(if var.ty.is_buffer() {
                Target::StateBuffer {
                    off: var.off,
                    ty: var.ty,
                }
            } else if var.ty.is_record() {
                Target::StateRecord {
                    off: var.off,
                    ty: var.ty,
                }
            } else if var.ty.is_sum() {
                Target::StateSum {
                    off: var.off,
                    ty: var.ty,
                }
            } else if var.ty.is_array() {
                Target::StateArray {
                    off: var.off,
                    ty: var.ty,
                }
            } else if var.ty.is_ring() {
                Target::StateRing {
                    off: var.off,
                    ty: var.ty,
                }
            } else if var.ty.is_map() {
                Target::StateMap {
                    off: var.off,
                    ty: var.ty,
                }
            } else {
                Target::StateScalar {
                    off: var.off,
                    ty: var.ty,
                }
            });
        }
        if find_const(self.consts, name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to constant `{name}`"),
            ));
        }
        if find_enum(self.enums, name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to enum type `{name}`"),
            ));
        }
        if find_record(self.records, name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to record type `{name}`"),
            ));
        }
        if find_sum(self.sums, name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to sum type `{name}`"),
            ));
        }
        if let Some((_, ordinal)) = find_variant(self.enums, name) {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to enum variant `{name}` (ordinal {ordinal})"),
            ));
        }
        if let Some((sum, _)) = self.sum_variant_owner(name) {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to sum variant `{name}` of `{sum}`"),
            ));
        }
        Err(self.err(line, column, format_args!("unknown identifier `{name}`")))
    }

    /// Finds which sum type declares `name` as a variant.
    fn sum_variant_owner(&self, name: &str) -> Option<(&'a str, SumVariantDef<'a>)> {
        for sum in self.sums {
            if let Some(variant) = variant_of(sum, name) {
                return Some((sum.name, variant));
            }
        }
        None
    }

    // ---- places (variables, fields and array elements) -----------------

    /// A designator names storage a reference can point at; it is either a
    /// bare name or a chain of field selections and element indexes.
    fn designator(expr: &'a Expr<'a>) -> Option<Designator<'a>> {
        match expr.kind {
            ExprKind::Designator(designator) => Some(designator),
            _ => None,
        }
    }

    /// Resolves a designator to a [`Place`], checking every field against the
    /// record table and every index against an array.
    ///
    /// The last step may index a buffer (`buf[i]`), which yields one `byte`;
    /// [`Place::byte_buffer`] records the buffer type. An index earlier in the
    /// path must be an array element, so nested indexing composes (`a[i][j]`,
    /// `a[i].field`).
    fn place(
        &self,
        base_name: &'a str,
        path: &'a [Access<'a>],
        line: u32,
        column: u32,
    ) -> Result<Place<'a>, Diagnostic> {
        let (base, base_ty, writable) = if let Some(slot) = self.slot(base_name) {
            (
                Base::Local {
                    off: slot.base,
                    is_ref: slot.is_ref,
                },
                slot.ty,
                slot.writable,
            )
        } else if let Some(var) = find_state(self.state_vars, base_name) {
            (Base::State { off: var.off }, var.ty, true)
        } else if find_const(self.consts, base_name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("`{base_name}` is a constant, not a value"),
            ));
        } else if find_enum(self.enums, base_name).is_some()
            || find_record(self.records, base_name).is_some()
            || find_sum(self.sums, base_name).is_some()
        {
            return Err(self.err(
                line,
                column,
                format_args!("`{base_name}` is a type, not a value"),
            ));
        } else if find_variant(self.enums, base_name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("`{base_name}` is an enum variant, not a value"),
            ));
        } else if self.sum_variant_owner(base_name).is_some() {
            return Err(self.err(
                line,
                column,
                format_args!("`{base_name}` is a sum variant, not a value"),
            ));
        } else {
            return Err(self.err(
                line,
                column,
                format_args!("unknown identifier `{base_name}`"),
            ));
        };

        let mut ty = base_ty;
        let mut byte_buffer = None;
        let count = path.len();
        for (index, access) in path.iter().enumerate() {
            match *access {
                Access::Field(name) => {
                    if byte_buffer.is_some() {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("a `byte` has no field `{name}`"),
                        ));
                    }
                    let Some(record) = record_of(self.records, ty) else {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("`{base_name}` is `{ty}`, which has no field `{name}`"),
                        ));
                    };
                    let Some(field) = record.fields.iter().find(|field| field.name == name) else {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("`{}` has no field `{name}`", record.name),
                        ));
                    };
                    ty = field.ty;
                }
                Access::Index(_) => {
                    if byte_buffer.is_some() {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("a `byte` cannot be indexed"),
                        ));
                    }
                    if ty.is_buffer() {
                        if index + 1 != count {
                            return Err(self.err(
                                line,
                                column,
                                format_args!(
                                    "a buffer element is a `byte`; it cannot be indexed or selected further"
                                ),
                            ));
                        }
                        byte_buffer = Some(ty);
                        ty = Type::scalar(Kind::Byte);
                    } else if let Some(array) = array_of(self.arrays.as_slice(), ty) {
                        ty = array.elem;
                    } else {
                        let found = self.type_name(ty);
                        return Err(self.err(
                            line,
                            column,
                            format_args!("`{base_name}` is `{found}`, which cannot be indexed"),
                        ));
                    }
                }
            }
        }
        Ok(Place {
            base,
            base_ty,
            path,
            ty,
            writable,
            byte_buffer,
        })
    }

    /// Builds the [`Place`] of a local/state variable with no path.
    fn place_of_target(&self, target: &Target) -> Place<'a> {
        match *target {
            Target::LocalBuffer { base, ty, is_ref } | Target::LocalRecord { base, ty, is_ref } => {
                Place {
                    base: Base::Local { off: base, is_ref },
                    base_ty: ty,
                    path: &[],
                    ty,
                    writable: true,
                    byte_buffer: None,
                }
            }
            Target::LocalSum { base, ty, is_ref } | Target::LocalArray { base, ty, is_ref } => {
                Place {
                    base: Base::Local { off: base, is_ref },
                    base_ty: ty,
                    path: &[],
                    ty,
                    writable: true,
                    byte_buffer: None,
                }
            }
            Target::LocalRing { base, ty, is_ref } => Place {
                base: Base::Local { off: base, is_ref },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
                byte_buffer: None,
            },
            Target::LocalMap { base, ty, is_ref } => Place {
                base: Base::Local { off: base, is_ref },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
                byte_buffer: None,
            },
            Target::StateBuffer { off, ty } | Target::StateRecord { off, ty } => Place {
                base: Base::State { off },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
                byte_buffer: None,
            },
            Target::StateSum { off, ty } | Target::StateArray { off, ty } => Place {
                base: Base::State { off },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
                byte_buffer: None,
            },
            Target::StateRing { off, ty } => Place {
                base: Base::State { off },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
                byte_buffer: None,
            },
            Target::StateMap { off, ty } => Place {
                base: Base::State { off },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
                byte_buffer: None,
            },
            Target::LocalScalar { .. } | Target::StateScalar { .. } => {
                unreachable!("scalars are not places")
            }
        }
    }

    /// Pushes a reference to the first byte of the value a [`Place`] names.
    ///
    /// A field step is `ref_offset` with its declared offset; an array element
    /// step is `ref_element` with the array's stride and count. The interpreter
    /// bounds-checks every intermediate reference.
    fn push_value_ref(&mut self, place: &Place<'a>) -> Result<(), Diagnostic> {
        match place.base {
            Base::Local { off, is_ref: true } => self.one(opcodes::LOAD_LOCAL, off)?,
            Base::Local { off, is_ref: false } => self.one(opcodes::REF_LOCAL, off)?,
            Base::State { off } => self.one(opcodes::REF_STATE, off)?,
        }
        self.push();

        let mut ty = place.base_ty;
        for access in place.path {
            match *access {
                Access::Field(name) => {
                    let field = field_of(self.records, ty, name)
                        .ok_or_else(|| self.internal("unresolved field in a place"))?;
                    self.one(opcodes::REF_OFFSET, field.off)?;
                    ty = field.ty;
                }
                Access::Index(index) => {
                    let array = array_of(self.arrays.as_slice(), ty)
                        .ok_or_else(|| self.internal("unresolved index in a place"))?;
                    self.check_index_type(index)?;
                    self.word(opcodes::REF_ELEMENT, 0, 0, 0)?;
                    self.imm32(array.stride)?;
                    self.imm32(array.count)?;
                    self.pop(1)?;
                    ty = array.elem;
                }
            }
        }
        Ok(())
    }

    /// Pushes a reference to a buffer's data area (its runtime length sits at
    /// `ref - 8`), for the ordinary buffer opcodes.
    ///
    /// A `const`/`var` buffer parameter already holds a data reference in its
    /// slot; every other buffer names its `[len][data]` image, so the data is
    /// eight bytes past the value.
    fn push_buffer_data_ref(&mut self, place: &Place<'a>) -> Result<(), Diagnostic> {
        if place.path.is_empty() && place.ty.is_buffer() {
            if let Base::Local { off, is_ref: true } = place.base {
                self.one(opcodes::LOAD_LOCAL, off)?;
                self.push();
                return Ok(());
            }
        }
        self.push_value_ref(place)?;
        if place.ty.is_buffer() {
            self.one(opcodes::REF_OFFSET, 8)?;
        }
        Ok(())
    }

    /// Copies a whole record or sum value from the reference in `src_slot`
    /// into the local area at `dst_base` (a `value` parameter's prologue).
    fn aggregate_copy_from_ref(
        &mut self,
        dst_base: u32,
        src_slot: u32,
        ty: Type,
    ) -> Result<(), Diagnostic> {
        self.one(opcodes::REF_LOCAL, dst_base)?;
        self.push();
        self.one(opcodes::LOAD_LOCAL, src_slot)?;
        self.push();
        self.const_i64(ty.size() as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// Emits a reference to a record, sum or array expression and returns its
    /// type.
    ///
    /// `writable` is required for a `var` parameter or an out-parameter; a
    /// `value` copy and a `const` borrow accept a read-only source.
    fn emit_aggregate_ref(
        &mut self,
        expr: &'a Expr<'a>,
        writable: bool,
        what: &str,
    ) -> Result<Type, Diagnostic> {
        if let ExprKind::Cast(name, _) = expr.kind {
            if find_record(self.records, name).is_some() {
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "`{name}(x)` is not a record constructor; declare a variable and assign its fields"
                    ),
                ));
            }
            if find_sum(self.sums, name).is_some() {
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "`{name}(x)` is not a sum constructor; write `Variant` or `Variant(payload)`"
                    ),
                ));
            }
        }
        let Some(designator) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("{what} must be a variable, field or element"),
            ));
        };
        let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
        if !place.ty.is_aggregate() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!(
                    "{what} expects a record, sum, array, ring or map value, found `{found}`"
                ),
            ));
        }
        if writable && !place.writable {
            return Err(self.err_at(
                expr,
                format_args!("cannot pass `const` value `{}` as `var`", designator.base),
            ));
        }
        self.push_value_ref(&place)?;
        Ok(place.ty)
    }

    /// Assigns a whole record: a byte copy of the destination-size region.
    fn assign_record(
        &mut self,
        destination: &Place<'a>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        self.push_value_ref(destination)?;
        let source = self.emit_aggregate_ref(value, false, "a record assignment")?;
        self.check_exact(destination.ty, source, value)?;
        self.const_i64(destination.ty.size() as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// Assigns a whole array: a byte copy of `N * stride` bytes, exactly like a
    /// record copy. The types must match exactly (element type and count).
    fn assign_array(
        &mut self,
        destination: &Place<'a>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        self.push_value_ref(destination)?;
        let source = self.emit_aggregate_ref(value, false, "an array assignment")?;
        self.check_exact(destination.ty, source, value)?;
        self.const_i64(destination.ty.size() as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// Assigns a whole ring: a byte copy of the whole image, header included,
    /// so `head` and `count` travel with the storage (spec 0003, "Rings").
    fn assign_ring(
        &mut self,
        destination: &Place<'a>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        self.push_value_ref(destination)?;
        let source = self.emit_aggregate_ref(value, false, "a ring assignment")?;
        self.check_exact(destination.ty, source, value)?;
        self.const_i64(destination.ty.size() as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// Assigns a whole map: a byte copy of the whole image, header and slots
    /// included, exactly like a ring copy (spec 0003, "Maps").
    fn assign_map(
        &mut self,
        destination: &Place<'a>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        self.push_value_ref(destination)?;
        let source = self.emit_aggregate_ref(value, false, "a map assignment")?;
        self.check_exact(destination.ty, source, value)?;
        self.const_i64(destination.ty.size() as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// Assigns a whole sum value.
    ///
    /// A sum value is either a construction of a variant of the destination's
    /// own type (`Ok`, `Faulted(code)`) or a copy of a value of that exact
    /// type. Construction is destination-driven: the destination names the sum
    /// type, so variant names never enter the global namespace. That is what
    /// lets `ExitReason.Ok` coexist with `Status.Ok` (spec 0003, "Sum types").
    fn assign_sum(
        &mut self,
        destination: &Place<'a>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        let sum = sum_of(self.sums, destination.ty)
            .ok_or_else(|| self.internal("sum destination has no sum definition"))?;
        match value.kind {
            ExprKind::Designator(designator) if designator.is_name() => {
                let name = designator.base;
                if let Some(variant) = variant_of(&sum, name) {
                    if variant.payload.is_some() {
                        return Err(self.err_at(
                            value,
                            format_args!(
                                "variant `{name}` carries a payload; construct it as `{name}(value)`"
                            ),
                        ));
                    }
                    self.construct_sum(destination, &sum, variant, None)
                } else if let Some((owner, _)) = self.sum_variant_owner(name) {
                    Err(self.err_at(
                        value,
                        format_args!("`{name}` is a variant of `{owner}`, not of `{}`", sum.name),
                    ))
                } else {
                    // Not a variant of this sum: copy it from a value of the
                    // same type (the type check reports a real mismatch).
                    self.copy_sum(destination, value)
                }
            }
            ExprKind::Designator(_) => self.copy_sum(destination, value),
            ExprKind::Call(call) => {
                if call.target.contains('.') {
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "a sum value is constructed as `Variant` or `Variant(payload)`"
                        ),
                    ));
                }
                let Some(variant) = variant_of(&sum, call.target) else {
                    if let Some((owner, _)) = self.sum_variant_owner(call.target) {
                        return Err(self.err_at(
                            value,
                            format_args!(
                                "`{}` is a variant of `{owner}`, not of `{}`",
                                call.target, sum.name
                            ),
                        ));
                    }
                    return Err(self.err_at(
                        value,
                        format_args!("`{}` is not a variant of `{}`", call.target, sum.name),
                    ));
                };
                if variant.payload.is_none() {
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "variant `{}` carries no payload; write `{}` without `()`",
                            variant.name, variant.name
                        ),
                    ));
                }
                if call.args.len() != 1 {
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "variant `{}` expects 1 payload argument, got {}",
                            variant.name,
                            call.args.len()
                        ),
                    ));
                }
                self.construct_sum(destination, &sum, variant, Some(call.args[0]))
            }
            _ => Err(self.err_at(
                value,
                format_args!(
                    "a sum value is constructed as `Variant(payload)` or copied from a `{}` value",
                    sum.name
                ),
            )),
        }
    }

    /// Copies a whole sum value from another value of the same type.
    fn copy_sum(&mut self, destination: &Place<'a>, value: &'a Expr<'a>) -> Result<(), Diagnostic> {
        self.push_value_ref(destination)?;
        let source = self.emit_aggregate_ref(value, false, "a sum assignment")?;
        self.check_exact(destination.ty, source, value)?;
        self.const_i64(destination.ty.size() as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// Writes a tag and (optionally) a payload into a sum destination.
    ///
    /// The tag is one 8-byte slot at offset 0. The payload area starts at
    /// offset 8: a scalar or enum is stored there directly, a buffer is
    /// constructed as `[len][data]` through the ordinary buffer opcodes, and a
    /// record or nested sum is copied whole.
    fn construct_sum(
        &mut self,
        destination: &Place<'a>,
        sum: &SumDef<'a>,
        variant: SumVariantDef<'a>,
        payload: Option<&'a Expr<'a>>,
    ) -> Result<(), Diagnostic> {
        let ordinal = sum
            .variants
            .iter()
            .position(|candidate| candidate.name == variant.name)
            .ok_or_else(|| self.internal("variant is not a member of its sum"))?;
        self.push_value_ref(destination)?;
        self.const_i64(ordinal as i64)?;
        self.simple(opcodes::STORE_REF)?;
        self.pop(2)?;

        let Some(payload_ty) = variant.payload else {
            return Ok(());
        };
        let payload = payload.ok_or_else(|| self.internal("payload variant without a value"))?;
        if payload_ty.is_buffer() {
            return self.buffer_assign(
                BufferDest::SumPayload {
                    sum: *destination,
                    ty: payload_ty,
                },
                None,
                payload,
            );
        }
        self.push_value_ref(destination)?;
        self.one(opcodes::REF_OFFSET, 8)?;
        if payload_ty.is_record()
            || payload_ty.is_sum()
            || payload_ty.is_array()
            || payload_ty.is_ring()
            || payload_ty.is_map()
        {
            let source = self.emit_aggregate_ref(payload, false, "a sum payload")?;
            self.check_exact(payload_ty, source, payload)?;
            self.const_i64(payload_ty.size() as i64)?;
            self.simple(opcodes::COPY_REFS)?;
            self.pop(3)?;
        } else {
            let found = self.expr_scalar(payload)?;
            self.check_exact(payload_ty, found, payload)?;
            self.simple(opcodes::STORE_REF)?;
            self.pop(2)?;
        }
        Ok(())
    }

    /// Pushes a reference to the data area a buffer construction writes.
    fn push_buffer_dest_ref(&mut self, destination: BufferDest<'a>) -> Result<(), Diagnostic> {
        match destination {
            BufferDest::Place(place) => self.push_buffer_data_ref(&place),
            BufferDest::SumPayload { sum, .. } => {
                self.push_value_ref(&sum)?;
                self.one(opcodes::REF_OFFSET, 16)
            }
        }
    }

    /// Assigns a whole buffer field or variable: a construction from literals
    /// and other buffers (`clear` then `append`), never a temporary.
    ///
    /// `home` names the destination designator for the aliasing check.
    fn buffer_assign(
        &mut self,
        destination: BufferDest<'a>,
        home: Option<&Designator<'a>>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        let ty = destination.ty();
        let bump = self.bump;
        let mut parts = SliceVec::<&'a Expr<'a>>::new(bump)
            .map_err(|_| self.arena_error(value.line, value.column))?;
        collect_concat(value, &mut parts, 0, self.path, self.unit)?;

        for index in 0..parts.len() {
            let part = parts.as_slice()[index];
            if Self::aliases(home, part) {
                return Err(self.err(
                    part.line,
                    part.column,
                    format_args!(
                        "destination aliases an operand; a buffer cannot be appended to itself"
                    ),
                ));
            }
            match part.kind {
                ExprKind::Str(bytes) => {
                    if bytes.len() as u64 > ty.cap as u64 {
                        return Err(self.err(
                            part.line,
                            part.column,
                            format_args!(
                                "string literal ({} bytes) does not fit {ty}",
                                bytes.len()
                            ),
                        ));
                    }
                    if ty.kind == Kind::String && core::str::from_utf8(bytes).is_err() {
                        return Err(self.err(
                            part.line,
                            part.column,
                            format_args!(
                                "string literal is not valid UTF-8; use a `bytes` destination for raw data"
                            ),
                        ));
                    }
                }
                ExprKind::Designator(designator) => {
                    let place =
                        self.place(designator.base, designator.path, part.line, part.column)?;
                    if !place.ty.is_buffer() {
                        let found = self.type_name(place.ty);
                        return Err(self.err(
                            part.line,
                            part.column,
                            format_args!("`{}` is `{found}`, not a buffer", designator.base),
                        ));
                    }
                    if place.ty.kind != ty.kind {
                        let found = self.type_name(place.ty);
                        return Err(self.err(
                            part.line,
                            part.column,
                            format_args!("cannot use `{found}` in a `{ty}` construction"),
                        ));
                    }
                    // Direct assignment between two inline buffers is a type
                    // assignment, so capacities must match exactly (spec 0003,
                    // D11). A `const`/`var` buffer parameter is a call-scoped
                    // reference whose declared capacity is only a lower bound,
                    // so copying one is a runtime-checked construction.
                    let is_ref = designator.path.is_empty()
                        && matches!(place.base, Base::Local { is_ref: true, .. });
                    if parts.len() == 1 && !is_ref && place.ty.cap != ty.cap {
                        return Err(self.err(
                            part.line,
                            part.column,
                            format_args!(
                                "capacity mismatch: `{}` cannot be assigned to `{ty}`",
                                self.type_name(place.ty)
                            ),
                        ));
                    }
                }
                _ => {
                    return Err(self.err(
                        part.line,
                        part.column,
                        format_args!("buffer construction accepts only literals, buffers and `+`"),
                    ))
                }
            }
        }

        self.push_buffer_dest_ref(destination)?;
        self.simple(opcodes::CLEAR)?;
        self.pop(1)?;

        for index in 0..parts.len() {
            let part = parts.as_slice()[index];
            // An empty literal adds nothing after `clear`; pushing the
            // destination reference for an append that is never emitted would
            // leave one value on the run-time operand stack.
            if matches!(part.kind, ExprKind::Str(bytes) if bytes.is_empty()) {
                continue;
            }
            self.push_buffer_dest_ref(destination)?;
            match part.kind {
                ExprKind::Str(bytes) => {
                    let offset = match self.pool.intern(bytes) {
                        Ok(offset) => offset,
                        Err(_) => return Err(self.arena_error(part.line, part.column)),
                    };
                    self.one(opcodes::REF_CONST, offset)?;
                    self.push();
                }
                ExprKind::Designator(designator) => {
                    let place =
                        self.place(designator.base, designator.path, part.line, part.column)?;
                    self.push_buffer_data_ref(&place)?;
                }
                _ => {
                    return Err(self.err_at(
                        part,
                        format_args!("buffer construction accepts only literals, buffers and `+`"),
                    ))
                }
            }
            self.one(opcodes::APPEND, ty.cap)?;
            self.pop(2)?;
        }

        Ok(())
    }

    /// True when `part` names the same storage as the destination designator.
    ///
    /// Field and index steps are compared structurally; an index whose
    /// expression is not literally the same shape is treated as different,
    /// which matches the old field-path rule and keeps `rec.a := rec.b`
    /// valid. (`a[i] := a[i]` is caught; `a[i] := a[j]` is allowed.)
    fn aliases(home: Option<&Designator<'a>>, part: &Expr<'a>) -> bool {
        let Some(home) = home else {
            return false;
        };
        let ExprKind::Designator(other) = part.kind else {
            return false;
        };
        home.base == other.base && same_accesses(home.path, other.path, 0)
    }

    // ---- expressions ---------------------------------------------------

    fn expr_scalar(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        self.enter_nesting(expr.line, expr.column)?;
        let result = self.expr_scalar_inner(expr);
        self.leave_nesting();
        result
    }

    fn expr_scalar_inner(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        match expr.kind {
            ExprKind::Int(value) => {
                self.const_i64(value)?;
                Ok(Type::scalar(Kind::Int))
            }
            ExprKind::Float(value) => {
                self.const_i64(value.to_bits() as i64)?;
                Ok(Type::scalar(Kind::Float))
            }
            ExprKind::Bool(value) => {
                self.const_i64(u64::from(value) as i64)?;
                Ok(Type::scalar(Kind::Bool))
            }
            ExprKind::Str(_) => Err(self.err_at(
                expr,
                format_args!(
                    "a string literal is a buffer; assign it into a buffer or pass it to a buffer parameter"
                ),
            )),
            ExprKind::Designator(designator) => self.load_designator(&designator, expr),
            ExprKind::Call(call) => match self.call(&call)? {
                Some(ty) => Ok(ty),
                None => Err(self.err_at(
                    expr,
                    format_args!("procedure `{}` does not return a value", call.target),
                )),
            },
            ExprKind::Cast(name, inner) => self.cast(name, inner, expr),
            ExprKind::Unary(op, inner) => self.unary(op, inner, expr),
            ExprKind::Binary(op, left, right) => self.binary(op, left, right, expr),
        }
    }

    /// Returns the buffer type of a buffer expression without emitting code.
    ///
    /// A string literal reports `string[len]`; that kind is only a default,
    /// because a literal is typed by its destination (spec 0003, T1). A
    /// designator may be a variable, a record field or an array element.
    fn probe_buffer(&self, expr: &Expr<'a>) -> Option<Type> {
        match expr.kind {
            ExprKind::Str(bytes) => Some(Type::buffer(Kind::String, bytes.len() as u32)),
            ExprKind::Designator(designator) => {
                let place = self
                    .place(designator.base, designator.path, expr.line, expr.column)
                    .ok()?;
                place.ty.is_buffer().then_some(place.ty)
            }
            _ => None,
        }
    }

    /// Emits `left = right` (or `<>`) when both operands are buffers, and
    /// returns `None` when neither is.
    ///
    /// Capacities may differ: the comparison reads both runtime lengths and
    /// then the bytes. A literal takes its kind from the other operand, so
    /// `line = "help"` compares `string` with `string` and `frame = "\x00"`
    /// compares `bytes` with `bytes`.
    fn compare_buffers(
        &mut self,
        op: BinOp,
        left: &'a Expr<'a>,
        right: &'a Expr<'a>,
        expr: &'a Expr<'a>,
    ) -> Result<Option<Type>, Diagnostic> {
        let (left_ty, right_ty) = match (self.probe_buffer(left), self.probe_buffer(right)) {
            (None, None) => return Ok(None),
            (Some(left_ty), Some(right_ty)) => (left_ty, right_ty),
            (Some(left_ty), None) => {
                return Err(self.err_at(
                    right,
                    format_args!(
                        "cannot compare `{left_ty}` with a scalar; both operands must be buffers of the same kind"
                    ),
                ))
            }
            (None, Some(right_ty)) => {
                return Err(self.err_at(
                    left,
                    format_args!(
                        "cannot compare a scalar with `{right_ty}`; both operands must be buffers of the same kind"
                    ),
                ))
            }
        };
        let left_literal = matches!(left.kind, ExprKind::Str(_));
        let right_literal = matches!(right.kind, ExprKind::Str(_));
        let kind = if left_ty.kind == right_ty.kind {
            left_ty.kind
        } else if left_literal {
            right_ty.kind
        } else if right_literal {
            left_ty.kind
        } else {
            return Err(self.err_at(
                expr,
                format_args!(
                    "cannot compare `{left_ty}` with `{right_ty}`; `string` and `bytes` are distinct types"
                ),
            ));
        };

        self.emit_ref(left, Some(kind))?;
        self.emit_ref(right, Some(kind))?;
        self.simple(opcodes::CMP_BUFFERS)?;
        self.pop(1)?;
        if op == BinOp::Ne {
            self.simple(opcodes::NOT)?;
        }
        Ok(Some(Type::scalar(Kind::Bool)))
    }

    /// Reads a designator as a scalar value.
    ///
    /// A bare name resolves to a constant, an enum variant, a local or a state
    /// variable. A field or element path resolves to a [`Place`]; a buffer
    /// index yields one raw `byte` (spec 0003, T4), and every other scalar or
    /// enum element is loaded through its reference. Records, sums, arrays and
    /// buffers are not scalar values.
    fn load_designator(
        &mut self,
        designator: &Designator<'a>,
        expr: &'a Expr<'a>,
    ) -> Result<Type, Diagnostic> {
        let base = designator.base;
        let path = designator.path;
        if path.is_empty() {
            return self.load_scalar(base, expr);
        }
        // A qualified enum variant, `TypeName.Variant`. `FaultCode` requires
        // this form because its names overlap `Status`.
        if path.len() == 1 {
            if let Access::Field(name) = path[0] {
                if let Some((id, ordinal)) = find_qualified_variant(self.enums, base, name) {
                    self.const_i64(ordinal as i64)?;
                    return Ok(Type::enumeration(id));
                }
            }
        }
        let place = self.place(base, path, expr.line, expr.column)?;
        if let Some(buffer_ty) = place.byte_buffer {
            let Some(Access::Index(index)) = place.path.last() else {
                return Err(self.internal("byte place does not end in an index"));
            };
            let prefix = Place {
                path: &place.path[..place.path.len() - 1],
                ty: buffer_ty,
                byte_buffer: None,
                ..place
            };
            self.push_buffer_data_ref(&prefix)?;
            self.check_index_type(index)?;
            self.simple(opcodes::LOAD_U8)?;
            self.pop(1)?;
            return Ok(Type::scalar(Kind::Byte));
        }
        if place.ty.is_stream() {
            return Err(self.err_at(
                expr,
                format_args!(
                    "stream endpoint `{base}` cannot be used as a value; pass it to `core.send`/`core.recv`"
                ),
            ));
        }
        if place.ty.is_buffer() {
            return Err(self.err_at(
                expr,
                format_args!("buffer `{base}` cannot be used as a scalar value"),
            ));
        }
        if place.ty.is_record() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!("record `{found}` cannot be used as a scalar value; select a field"),
            ));
        }
        if place.ty.is_sum() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!(
                    "sum `{found}` cannot be used as a scalar value; match it with `case`"
                ),
            ));
        }
        if place.ty.is_array() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!(
                    "array `{found}` cannot be used as a scalar value; index it or copy it whole"
                ),
            ));
        }
        if place.ty.is_ring() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!(
                    "ring `{found}` cannot be used as a scalar value; use `core.ring_push`/`pop`/`peek` and copy it whole"
                ),
            ));
        }
        if place.ty.is_map() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!(
                    "map `{found}` cannot be used as a scalar value; use `core.map_get`/`map_put` and copy it whole"
                ),
            ));
        }
        self.push_value_ref(&place)?;
        self.simple(opcodes::LOAD_REF)?;
        // `push_value_ref` counts the reference; `load_ref` replaces it with
        // the value, so the depth does not change.
        Ok(place.ty)
    }

    fn load_scalar(&mut self, name: &'a str, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        if let Some(value) = find_const(self.consts, name) {
            self.const_i64(value)?;
            return Ok(Type::scalar(Kind::Int));
        }
        if let Some(slot) = self.slot(name) {
            if slot.ty.is_stream() {
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "stream endpoint `{name}` cannot be used as a value; pass it to `core.send`/`core.recv`"
                    ),
                ));
            }
            if slot.ty.is_buffer() {
                return Err(self.err_at(
                    expr,
                    format_args!("buffer `{name}` cannot be used as a scalar value"),
                ));
            }
            if slot.ty.is_record() {
                let found = self.type_name(slot.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "record `{name}` of type `{found}` cannot be used as a scalar value; select a field"
                    ),
                ));
            }
            if slot.ty.is_sum() {
                let found = self.type_name(slot.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "sum `{name}` of type `{found}` cannot be used as a scalar value; match it with `case`"
                    ),
                ));
            }
            if slot.ty.is_array() {
                let found = self.type_name(slot.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "array `{name}` of type `{found}` cannot be used as a scalar value; index it or copy it whole"
                    ),
                ));
            }
            if slot.ty.is_ring() {
                let found = self.type_name(slot.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "ring `{name}` of type `{found}` cannot be used as a scalar value; use `core.ring_push`/`pop`/`peek` and copy it whole"
                    ),
                ));
            }
            if slot.ty.is_map() {
                let found = self.type_name(slot.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "map `{name}` of type `{found}` cannot be used as a scalar value; use `core.map_get`/`map_put` and copy it whole"
                    ),
                ));
            }
            if slot.is_ref {
                self.one(opcodes::LOAD_LOCAL, slot.base)?;
                self.simple(opcodes::LOAD_REF)?;
            } else {
                self.one(opcodes::LOAD_LOCAL, slot.base)?;
            }
            self.push();
            return Ok(slot.ty);
        }
        if let Some(var) = find_state(self.state_vars, name) {
            if var.ty.is_stream() {
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "stream endpoint `{name}` cannot be used as a value; pass it to `core.send`/`core.recv`"
                    ),
                ));
            }
            if var.ty.is_buffer() {
                return Err(self.err_at(
                    expr,
                    format_args!("buffer `{name}` cannot be used as a scalar value"),
                ));
            }
            if var.ty.is_record() {
                let found = self.type_name(var.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "record `{name}` of type `{found}` cannot be used as a scalar value; select a field"
                    ),
                ));
            }
            if var.ty.is_sum() {
                let found = self.type_name(var.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "sum `{name}` of type `{found}` cannot be used as a scalar value; match it with `case`"
                    ),
                ));
            }
            if var.ty.is_array() {
                let found = self.type_name(var.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "array `{name}` of type `{found}` cannot be used as a scalar value; index it or copy it whole"
                    ),
                ));
            }
            if var.ty.is_ring() {
                let found = self.type_name(var.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "ring `{name}` of type `{found}` cannot be used as a scalar value; use `core.ring_push`/`pop`/`peek` and copy it whole"
                    ),
                ));
            }
            if var.ty.is_map() {
                let found = self.type_name(var.ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "map `{name}` of type `{found}` cannot be used as a scalar value; use `core.map_get`/`map_put` and copy it whole"
                    ),
                ));
            }
            self.one(opcodes::LOAD_STATE, var.off)?;
            self.push();
            return Ok(var.ty);
        }
        // A variant is a global constant of its enum type with its ordinal as
        // the value (spec 0003, "Enums").
        if let Some((id, ordinal)) = find_variant(self.enums, name) {
            self.const_i64(ordinal as i64)?;
            return Ok(Type::enumeration(id));
        }
        let suggestion = if find_sig(self.sigs, name).is_some() {
            format_args!("`{name}` is a procedure; call it with `()`")
        } else {
            format_args!("unknown identifier `{name}`")
        };
        Err(self.err_at(expr, suggestion))
    }

    fn cast(
        &mut self,
        name: &'a str,
        inner: &'a Expr<'a>,
        expr: &'a Expr<'a>,
    ) -> Result<Type, Diagnostic> {
        // An enum name is a checked construction from an `int` ordinal:
        // `Status(int_value)` fault(Overflow) outside `0..variant_count`
        // (spec 0003, "Enums").
        if let Some(id) = find_enum(self.enums, name) {
            let source = self.expr_scalar(inner)?;
            let target = Type::enumeration(id);
            if source == target {
                return Ok(target);
            }
            if source.kind != Kind::Int {
                let found = self.type_name(source);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "`{name}(x)` needs an `int` ordinal, found `{found}`; convert the value with `int(x)` first"
                    ),
                ));
            }
            let count = self
                .enums
                .get(id as usize)
                .map(|enumeration| enumeration.variants.len() as u32)
                .unwrap_or(0);
            self.one(opcodes::ENUM_CAST, count)?;
            return Ok(target);
        }
        // A record or sum name in `Name(x)` position is not a constructor:
        // records are built by assigning fields and sums by writing a variant
        // (spec 0003).
        if find_record(self.records, name).is_some() {
            return Err(self.err_at(
                expr,
                format_args!(
                    "`{name}(x)` is not a record constructor; declare a variable and assign its fields"
                ),
            ));
        }
        if find_sum(self.sums, name).is_some() {
            return Err(self.err_at(
                expr,
                format_args!(
                    "`{name}(x)` is not a sum constructor; write `Variant` or `Variant(payload)`"
                ),
            ));
        }
        let target = match name {
            "int" => Kind::Int,
            "uint" => Kind::Uint,
            "byte" => Kind::Byte,
            "f64" => Kind::Float,
            _ => unreachable!("the parser only builds known casts"),
        };
        let source = self.expr_scalar(inner)?;
        if !source.is_scalar() {
            return Err(self.err_at(
                expr,
                format_args!("buffer conversions are not casts; they are `stdlib` functions"),
            ));
        }
        // `int(enum_value)` is the ordinal; it needs no instruction because the
        // slot already holds the ordinal. Every other numeric cast from an enum
        // has to go through `int` first.
        if matches!(source.kind, Kind::Enum(_)) {
            if target == Kind::Int {
                return Ok(Type::scalar(Kind::Int));
            }
            let found = self.type_name(source);
            return Err(self.err_at(
                expr,
                format_args!(
                    "cannot convert `{found}` to `{}`; convert it with `int(x)` first",
                    target.name()
                ),
            ));
        }
        if source.kind == Kind::Bool {
            return Err(self.err_at(
                expr,
                format_args!("cannot convert `bool` to `{}`", target.name()),
            ));
        }
        if source.kind == target {
            return Ok(Type::scalar(target));
        }
        let from = source
            .value_kind()
            .ok_or_else(|| self.err_at(expr, format_args!("unsupported conversion")))?;
        let to = Type::scalar(target)
            .value_kind()
            .ok_or_else(|| self.err_at(expr, format_args!("unsupported conversion")))?;
        self.word(opcodes::CONVERT, from, to, 0)?;
        Ok(Type::scalar(target))
    }

    fn unary(
        &mut self,
        op: UnOp,
        inner: &'a Expr<'a>,
        expr: &'a Expr<'a>,
    ) -> Result<Type, Diagnostic> {
        match op {
            UnOp::Neg => {
                let ty = self.expr_scalar(inner)?;
                match ty.kind {
                    Kind::Int => {
                        self.simple(opcodes::NEG)?;
                        Ok(ty)
                    }
                    Kind::Float => {
                        self.simple(opcodes::FNEG)?;
                        Ok(ty)
                    }
                    Kind::Uint => Err(self.err_at(
                        expr,
                        format_args!(
                            "unary `-` is not defined for `uint`; convert with `int(x)` first"
                        ),
                    )),
                    other => Err(self.err_at(
                        expr,
                        format_args!("unary `-` is not defined for `{}`", other.name()),
                    )),
                }
            }
            UnOp::Not => {
                let ty = self.expr_scalar(inner)?;
                if ty.kind != Kind::Bool {
                    return Err(
                        self.err_at(expr, format_args!("`not` needs a `bool`, found `{ty}`"))
                    );
                }
                self.simple(opcodes::NOT)?;
                Ok(ty)
            }
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        left: &'a Expr<'a>,
        right: &'a Expr<'a>,
        expr: &'a Expr<'a>,
    ) -> Result<Type, Diagnostic> {
        // `and`/`or` short-circuit, so they are lowered to branches before the
        // right operand is evaluated.
        if matches!(op, BinOp::And | BinOp::Or) {
            let left_ty = self.expr_scalar(left)?;
            if left_ty.kind != Kind::Bool {
                return Err(self.err_at(
                    left,
                    format_args!(
                        "`{}` needs `bool` operands, found `{left_ty}`",
                        if op == BinOp::And { "and" } else { "or" }
                    ),
                ));
            }
            let end = self.label()?;
            let short = self.label()?;
            if op == BinOp::And {
                self.jump(opcodes::JZ, short, true)?;
            } else {
                self.jump(opcodes::JNZ, short, true)?;
            }
            // The short-circuit branch starts with an empty operand stack; the
            // right operand path starts with the same depth after the pop.
            let short_depth = self.depth;
            let right_ty = self.expr_scalar(right)?;
            if right_ty.kind != Kind::Bool {
                return Err(self.err_at(
                    right,
                    format_args!(
                        "`{}` needs `bool` operands, found `{right_ty}`",
                        if op == BinOp::And { "and" } else { "or" }
                    ),
                ));
            }
            self.jump(opcodes::JMP, end, false)?;
            self.bind(short);
            self.depth = short_depth;
            self.const_i64(u64::from(op == BinOp::Or) as i64)?;
            self.bind(end);
            return Ok(Type::scalar(Kind::Bool));
        }

        // Equality between two buffers is a byte comparison over the runtime
        // lengths, not a value copy: `line = "help"` must compile. `<>` is the
        // same comparison negated. Ordering stays scalar-only.
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            if let Some(result) = self.compare_buffers(op, left, right, expr)? {
                return Ok(result);
            }
        }
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
            && (self.probe_buffer(left).is_some() || self.probe_buffer(right).is_some())
        {
            return Err(self.err_at(
                expr,
                format_args!(
                    "buffer ordering is not implemented; buffers compare with `=` and `<>` only"
                ),
            ));
        }

        let left_ty = self.expr_scalar(left)?;
        let right_ty = self.expr_scalar(right)?;
        if left_ty != right_ty {
            if matches!(left_ty.kind, Kind::Enum(_)) || matches!(right_ty.kind, Kind::Enum(_)) {
                let left_name = self.type_name(left_ty);
                let right_name = self.type_name(right_ty);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "type mismatch: `{left_name}` and `{right_name}`; an enum compares only with the same enum type (use `int(x)` for its ordinal)"
                    ),
                ));
            }
            let left_name = self.type_name(left_ty);
            let right_name = self.type_name(right_ty);
            return Err(self.err_at(
                expr,
                format_args!(
                    "type mismatch: `{left_name}` and `{right_name}`; there are no implicit conversions"
                ),
            ));
        }

        let arithmetic = matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Slash | BinOp::Div | BinOp::Mod
        );
        let comparison = matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );

        let encoded = match left_ty.kind {
            Kind::Int => match op {
                BinOp::Add => opcodes::ADD,
                BinOp::Sub => opcodes::SUB,
                BinOp::Mul => opcodes::MUL,
                BinOp::Div => opcodes::DIV,
                BinOp::Mod => opcodes::MOD,
                BinOp::Eq => opcodes::EQ,
                BinOp::Ne => opcodes::NE,
                BinOp::Lt => opcodes::LT,
                BinOp::Le => opcodes::LE,
                BinOp::Gt => opcodes::GT,
                BinOp::Ge => opcodes::GE,
                BinOp::Slash => {
                    return Err(self.err_at(
                        expr,
                        format_args!("`/` is float division; use `div` for integers"),
                    ))
                }
                BinOp::And | BinOp::Or => unreachable!(),
            },
            Kind::Uint => match op {
                BinOp::Add => opcodes::ADD_U,
                BinOp::Sub => opcodes::SUB_U,
                BinOp::Mul => opcodes::MUL_U,
                BinOp::Div => opcodes::DIV_U,
                BinOp::Mod => opcodes::MOD_U,
                BinOp::Eq => opcodes::EQ,
                BinOp::Ne => opcodes::NE,
                BinOp::Lt => opcodes::LT_U,
                BinOp::Le => opcodes::LE_U,
                BinOp::Gt => opcodes::GT_U,
                BinOp::Ge => opcodes::GE_U,
                BinOp::Slash => {
                    return Err(self.err_at(
                        expr,
                        format_args!("`/` is float division; use `div` for integers"),
                    ))
                }
                BinOp::And | BinOp::Or => unreachable!(),
            },
            Kind::Byte => {
                if arithmetic {
                    return Err(self.err_at(
                        expr,
                        format_args!(
                            "arithmetic on `byte` is not defined; convert with `int(x)` first"
                        ),
                    ));
                }
                match op {
                    BinOp::Eq => opcodes::EQ,
                    BinOp::Ne => opcodes::NE,
                    BinOp::Lt => opcodes::LT_U,
                    BinOp::Le => opcodes::LE_U,
                    BinOp::Gt => opcodes::GT_U,
                    BinOp::Ge => opcodes::GE_U,
                    _ => unreachable!(),
                }
            }
            Kind::Bool => match op {
                BinOp::Eq => opcodes::EQ,
                BinOp::Ne => opcodes::NE,
                _ => {
                    return Err(self.err_at(expr, format_args!("`bool` supports only `=` and `<>`")))
                }
            },
            Kind::Float => match op {
                BinOp::Add => opcodes::FADD,
                BinOp::Sub => opcodes::FSUB,
                BinOp::Mul => opcodes::FMUL,
                BinOp::Slash => opcodes::FDIV,
                BinOp::Div | BinOp::Mod => {
                    return Err(self.err_at(
                        expr,
                        format_args!("use `/` for `f64`; `div` and `mod` are integer operations"),
                    ))
                }
                BinOp::Eq => opcodes::FEQ,
                BinOp::Ne => opcodes::FNE,
                BinOp::Lt => opcodes::FLT,
                BinOp::Le => opcodes::FLE,
                BinOp::Gt => opcodes::FGT,
                BinOp::Ge => opcodes::FGE,
                BinOp::And | BinOp::Or => unreachable!(),
            },
            Kind::String | Kind::Bytes => unreachable!(),
            Kind::Record(_) | Kind::Sum(_) | Kind::Array(_) | Kind::Ring(_) | Kind::Map(_)
            | Kind::InStream | Kind::OutStream => unreachable!(),
            Kind::Enum(_) => match op {
                // Ordinals are non-negative, so unsigned ordering is the
                // declaration order (spec 0003, "Enums").
                BinOp::Eq => opcodes::EQ,
                BinOp::Ne => opcodes::NE,
                BinOp::Lt => opcodes::LT_U,
                BinOp::Le => opcodes::LE_U,
                BinOp::Gt => opcodes::GT_U,
                BinOp::Ge => opcodes::GE_U,
                _ => {
                    return Err(self.err_at(
                        expr,
                        format_args!(
                            "an enum supports `=`, `<>` and ordinal ordering; use `int(x)` for arithmetic"
                        ),
                    ))
                }
            },
        };

        self.simple(encoded)?;
        self.pop(1)?;
        Ok(if comparison {
            Type::scalar(Kind::Bool)
        } else {
            left_ty
        })
    }

    // ---- calls ---------------------------------------------------------

    fn call(&mut self, call: &Call<'a>) -> Result<Option<Type>, Diagnostic> {
        let target = call.target;
        let args = call.args;
        let line = call.line;
        let column = call.column;
        match target {
            "core.write" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.write` expects 1 argument, got {}", args.len()),
                    ));
                }
                self.emit_ref(args[0], None)?;
                self.one(opcodes::BUILTIN, builtin::WRITE)?;
                self.pop(1)?;
                Ok(None)
            }
            // Staged line discipline (spec 0005, Q44): the kernel assembles
            // one line and hands it to the caller. `var` gives the callee the
            // caller's buffer; the declared capacity is carried in the
            // instruction so the interpreter can bound the copy.
            "core.read_line" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.read_line` expects 1 argument, got {}", args.len()),
                    ));
                }
                let found = self.emit_var_buffer_ref(args[0])?;
                if found.kind != Kind::String {
                    return Err(self.err_at(
                        args[0],
                        format_args!(
                            "`core.read_line` expects a `var string` buffer, found `{found}`"
                        ),
                    ));
                }
                self.one(opcodes::READ_LINE, found.cap)?;
                self.pop(1)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // `core.len` accepts both buffer kinds; one builtin serves both
            // because the length lives in the same header (spec 0003).
            "core.len" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.len` expects 1 argument, got {}", args.len()),
                    ));
                }
                self.emit_ref(args[0], None)?;
                self.one(opcodes::BUILTIN, builtin::LEN)?;
                self.pop(1)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            // `core.find(buf, value: byte) -> int`: first index of the byte, or
            // -1. Both buffer kinds share the byte scan (spec 0003).
            "core.find" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.find` expects 2 arguments, got {}", args.len()),
                    ));
                }
                self.emit_ref(args[0], None)?;
                let value = self.expr_scalar(args[1])?;
                if value.kind != Kind::Byte {
                    return Err(self.err_at(
                        args[1],
                        format_args!(
                            "`core.find` expects a `byte` value, found `{value}`; convert with `byte(x)`"
                        ),
                    ));
                }
                self.simple(opcodes::FIND)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // `core.substr_into(dst, src, start: int, len: int) -> int`. It stays
            // in `core`: the destination capacity must be a call-site
            // immediate. All four kind
            // combinations are legal; the **destination** kind decides the
            // check (string→string boundaries, bytes→string strict UTF-8,
            // bytes destination raw).
            "core.substr_into" => {
                if args.len() != 4 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.substr_into` expects 4 arguments, got {}", args.len()),
                    ));
                }
                let destination = self.emit_var_buffer_ref(args[0])?;
                // A literal takes its kind from the destination (spec 0003,
                // T1); a named source contributes its own kind, and mixing
                // `string` with `bytes` is allowed.
                let source_kind = if matches!(args[1].kind, ExprKind::Str(_)) {
                    destination.kind
                } else {
                    self.probe_buffer(args[1])
                        .map_or(destination.kind, |source| source.kind)
                };
                let source = self.emit_ref(args[1], Some(source_kind))?;
                let start = self.expr_scalar(args[2])?;
                if start.kind != Kind::Int {
                    return Err(self.err_at(
                        args[2],
                        format_args!("`core.substr_into` expects an `int` start, found `{start}`"),
                    ));
                }
                let length = self.expr_scalar(args[3])?;
                if length.kind != Kind::Int {
                    return Err(self.err_at(
                        args[3],
                        format_args!(
                            "`core.substr_into` expects an `int` length, found `{length}`"
                        ),
                    ));
                }
                let kind = match destination.kind {
                    Kind::String => opcodes::buffer_kind::STRING,
                    Kind::Bytes => opcodes::buffer_kind::BYTES,
                    _ => unreachable!("the destination is a buffer"),
                };
                let source_kind = match source.kind {
                    Kind::String => opcodes::buffer_kind::STRING,
                    Kind::Bytes => opcodes::buffer_kind::BYTES,
                    _ => unreachable!("the source is a buffer"),
                };
                self.word(opcodes::SUBSTR, kind, source_kind, 0)?;
                self.imm32(destination.cap)?;
                self.pop(4)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            "core.clock" => {
                if !args.is_empty() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.clock` expects 0 arguments, got {}", args.len()),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::CLOCK)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            // Staged formatting primitive (spec 0003, "Standard library"):
            // integer formatting ultimately lives in `core`/`stdlib`, and the
            // opcode keeps it a pure buffer operation with no host call. Both
            // buffer kinds accept the ASCII output.
            "core.format_int_into" => {
                if args.len() != 4 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!(
                            "`core.format_int_into` expects 4 arguments, got {}",
                            args.len()
                        ),
                    ));
                }
                let destination = self.emit_var_buffer_ref(args[0])?;
                let value = self.expr_scalar(args[1])?;
                if value.kind != Kind::Int {
                    return Err(self.err_at(
                        args[1],
                        format_args!(
                            "`core.format_int_into` expects an `int` value, found `{value}`"
                        ),
                    ));
                }
                let base = self.expr_scalar(args[2])?;
                if base.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[2],
                        format_args!(
                            "`core.format_int_into` expects a `uint` base, found `{base}`"
                        ),
                    ));
                }
                let width = self.expr_scalar(args[3])?;
                if width.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[3],
                        format_args!(
                            "`core.format_int_into` expects a `uint` width, found `{width}`"
                        ),
                    ));
                }
                let kind = match destination.kind {
                    Kind::String => opcodes::buffer_kind::STRING,
                    Kind::Bytes => opcodes::buffer_kind::BYTES,
                    _ => unreachable!("the destination is a buffer"),
                };
                self.word(opcodes::FORMAT_INT, kind, 0, 0)?;
                self.imm32(destination.cap)?;
                self.pop(4)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // `core.format_f64_into(dst, value: f64, frac_digits: uint) -> int`
            // (spec 0003, "Floating point"). Fixed-point text in a writable
            // `string` or `bytes` buffer; the interpreter builds it, so no host
            // call and no formatting library.
            "core.format_f64_into" => {
                if args.len() != 3 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!(
                            "`core.format_f64_into` expects 3 arguments, got {}",
                            args.len()
                        ),
                    ));
                }
                let destination = self.emit_var_buffer_ref(args[0])?;
                let value = self.expr_scalar(args[1])?;
                if value.kind != Kind::Float {
                    return Err(self.err_at(
                        args[1],
                        format_args!(
                            "`core.format_f64_into` expects an `f64` value, found `{value}`"
                        ),
                    ));
                }
                let frac_digits = self.expr_scalar(args[2])?;
                if frac_digits.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[2],
                        format_args!(
                            "`core.format_f64_into` expects a `uint` fraction width, found `{frac_digits}`"
                        ),
                    ));
                }
                let kind = match destination.kind {
                    Kind::String => opcodes::buffer_kind::STRING,
                    Kind::Bytes => opcodes::buffer_kind::BYTES,
                    _ => unreachable!("the destination is a buffer"),
                };
                self.word(opcodes::FORMAT_F64, kind, 0, 0)?;
                self.imm32(destination.cap)?;
                self.pop(3)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // `core.format_f64_short_into(dst, value: f64) -> int`
            // (spec 0003, Q41). The shortest round-trip text in a writable
            // `string` or `bytes` buffer; no width, no magnitude bound, and no
            // host call. The interpreter owns the digit search.
            "core.format_f64_short_into" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!(
                            "`core.format_f64_short_into` expects 2 arguments, got {}",
                            args.len()
                        ),
                    ));
                }
                let destination = self.emit_var_buffer_ref(args[0])?;
                let value = self.expr_scalar(args[1])?;
                if value.kind != Kind::Float {
                    return Err(self.err_at(
                        args[1],
                        format_args!(
                            "`core.format_f64_short_into` expects an `f64` value, found `{value}`"
                        ),
                    ));
                }
                let kind = match destination.kind {
                    Kind::String => opcodes::buffer_kind::STRING,
                    Kind::Bytes => opcodes::buffer_kind::BYTES,
                    _ => unreachable!("the destination is a buffer"),
                };
                self.word(opcodes::FORMAT_F64_SHORT, kind, 0, 0)?;
                self.imm32(destination.cap)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // First `syslib` primitives: x86 port I/O (spec 0003, "Hardware
            // access"). `Port` is staged as a plain `uint` (Q45); the namespace
            // is still separate from `core`, so a hardware access stays
            // visible as such. MMIO and DMA are not implemented.
            //
            // `syslib.cpu_id()` reports the APIC id of the core that executes
            // the call. A task is migratable, so this is "where am I now", not
            // an affinity (ADR 0011).
            "syslib.cpu_id" => {
                if !args.is_empty() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.cpu_id` expects 0 arguments, got {}", args.len()),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::CPU_ID)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            // The realtime class (spec 0007, ADR 0016). `realtime_on` is a
            // one-way promotion that returns `Status`; `heartbeat` is a bare
            // progress claim with no operands and no result. Neither has a
            // host-visible parameter, so the only compile-time check is arity.
            "core.realtime_on" => {
                if !args.is_empty() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.realtime_on` expects 0 arguments, got {}", args.len()),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::REALTIME_ON)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.heartbeat" => {
                if !args.is_empty() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.heartbeat` expects 0 arguments, got {}", args.len()),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::HEARTBEAT)?;
                Ok(None)
            }
            "syslib.port_in8" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.port_in8` expects 1 argument, got {}", args.len()),
                    ));
                }
                let port = self.expr_scalar(args[0])?;
                if port.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[0],
                        format_args!("`syslib.port_in8` expects a `uint` port, found `{port}`"),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::PORT_IN8)?;
                self.pop(1)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            "syslib.port_out8" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.port_out8` expects 2 arguments, got {}", args.len()),
                    ));
                }
                let port = self.expr_scalar(args[0])?;
                if port.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[0],
                        format_args!("`syslib.port_out8` expects a `uint` port, found `{port}`"),
                    ));
                }
                let value = self.expr_scalar(args[1])?;
                if value.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[1],
                        format_args!("`syslib.port_out8` expects a `uint` value, found `{value}`"),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::PORT_OUT8)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // `syslib` graphics (spec 0009). The primitives are host builtins,
            // so there is no opcode and no rasterisation in bytecode. The
            // compiler only checks the staged signatures: `int` coordinates,
            // `uint` colour, and a `string` for the text. Runtime rules
            // (non-positive extents, a colour above 0x00FF_FFFF, clipping)
            // belong to the interpreter and the host.
            "syslib.screen_width" | "syslib.screen_height" => {
                if !args.is_empty() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`{target}` expects 0 arguments, got {}", args.len()),
                    ));
                }
                let id = if target == "syslib.screen_width" {
                    builtin::SCREEN_WIDTH
                } else {
                    builtin::SCREEN_HEIGHT
                };
                self.one(opcodes::BUILTIN, id)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            "syslib.fill_rect" | "syslib.fill_dither" | "syslib.draw_rect" => {
                self.gfx_rect_operands(target, line, column, args)?;
                let id = match target {
                    "syslib.fill_rect" => builtin::FILL_RECT,
                    "syslib.fill_dither" => builtin::FILL_DITHER,
                    _ => builtin::DRAW_RECT,
                };
                self.one(opcodes::BUILTIN, id)?;
                self.pop(5)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "syslib.draw_text" => {
                if args.len() != 4 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.draw_text` expects 4 arguments, got {}", args.len()),
                    ));
                }
                for (index, name) in ["x", "y"].iter().enumerate() {
                    let value = self.expr_scalar(args[index])?;
                    if value.kind != Kind::Int {
                        return Err(self.err_at(
                            args[index],
                            format_args!(
                                "`syslib.draw_text` expects an `int` {name}, found `{value}`"
                            ),
                        ));
                    }
                }
                let text = self.emit_ref(args[2], Some(Kind::String))?;
                if text.kind != Kind::String {
                    let found = self.type_name(text);
                    return Err(self.err_at(
                        args[2],
                        format_args!("`syslib.draw_text` expects a `string`, found `{found}`"),
                    ));
                }
                let color = self.expr_scalar(args[3])?;
                if color.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[3],
                        format_args!("`syslib.draw_text` expects a `uint` color, found `{color}`"),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::DRAW_TEXT)?;
                self.pop(4)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "syslib.get_pixel" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.get_pixel` expects 2 arguments, got {}", args.len()),
                    ));
                }
                for (index, name) in ["x", "y"].iter().enumerate() {
                    let value = self.expr_scalar(args[index])?;
                    if value.kind != Kind::Int {
                        return Err(self.err_at(
                            args[index],
                            format_args!(
                                "`syslib.get_pixel` expects an `int` {name}, found `{value}`"
                            ),
                        ));
                    }
                }
                self.one(opcodes::BUILTIN, builtin::GET_PIXEL)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            "syslib.present" => {
                if args.len() != 4 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.present` expects 4 arguments, got {}", args.len()),
                    ));
                }
                for (index, name) in ["x", "y", "w", "h"].iter().enumerate() {
                    let value = self.expr_scalar(args[index])?;
                    if value.kind != Kind::Int {
                        return Err(self.err_at(
                            args[index],
                            format_args!(
                                "`syslib.present` expects an `int` {name}, found `{value}`"
                            ),
                        ));
                    }
                }
                self.one(opcodes::BUILTIN, builtin::PRESENT)?;
                self.pop(4)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "syslib.clear" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`syslib.clear` expects 1 argument, got {}", args.len()),
                    ));
                }
                let color = self.expr_scalar(args[0])?;
                if color.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[0],
                        format_args!("`syslib.clear` expects a `uint` color, found `{color}`"),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::CLEAR)?;
                self.pop(1)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            // Input (spec 0010). The pointer is a read-only machine value:
            // `int` screen coordinates and a `uint` button mask. Injection is
            // the debug-only hook that moves the pointer by screen deltas; the
            // runtime `buttons > 0b111` rule belongs to the interpreter.
            "syslib.pointer_x"
            | "syslib.pointer_y"
            | "syslib.pointer_buttons"
            | "syslib.pointer_source" => {
                if !args.is_empty() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`{target}` expects 0 arguments, got {}", args.len()),
                    ));
                }
                let (id, result) = match target {
                    "syslib.pointer_x" => (builtin::POINTER_X, Kind::Int),
                    "syslib.pointer_y" => (builtin::POINTER_Y, Kind::Int),
                    "syslib.pointer_buttons" => (builtin::POINTER_BUTTONS, Kind::Uint),
                    _ => (builtin::POINTER_SOURCE, Kind::Uint),
                };
                self.one(opcodes::BUILTIN, id)?;
                self.push();
                Ok(Some(Type::scalar(result)))
            }
            "syslib.inject_pointer" => {
                if args.len() != 3 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!(
                            "`syslib.inject_pointer` expects 3 arguments, got {}",
                            args.len()
                        ),
                    ));
                }
                for (index, name) in ["dx", "dy"].iter().enumerate() {
                    let value = self.expr_scalar(args[index])?;
                    if value.kind != Kind::Int {
                        return Err(self.err_at(
                            args[index],
                            format_args!(
                                "`syslib.inject_pointer` expects an `int` {name}, found `{value}`"
                            ),
                        ));
                    }
                }
                let buttons = self.expr_scalar(args[2])?;
                if buttons.kind != Kind::Uint {
                    return Err(self.err_at(
                        args[2],
                        format_args!(
                            "`syslib.inject_pointer` expects a `uint` buttons, found `{buttons}`"
                        ),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::INJECT_POINTER)?;
                self.pop(3)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            // Ring operations (spec 0003, "Rings") are compiler-recognised
            // built-in procedures, not host builtins: they lower to the
            // ordinary reference/copy/branch instructions over the ring's
            // layout, so no opcode and no interpreter builtin id is involved.
            // `push` and `pop` mutate the receiver; `peek`, `full` and
            // `empty` read it.
            "core.ring_push" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.ring_push` expects 2 arguments, got {}", args.len()),
                    ));
                }
                let (place, ring) = self.ring_receiver(args[0], "core.ring_push", true)?;
                let start_depth = self.depth;
                let full = self.label()?;
                let end = self.label()?;
                self.push_ring_header(&place, ring.count_off)?;
                self.const_i64(ring.count as i64)?;
                self.simple(opcodes::EQ)?;
                self.pop(1)?;
                self.jump(opcodes::JNZ, full, true)?;
                self.write_ring_element(&place, &ring, args[1])?;
                self.adjust_ring_count(&place, &ring, 1)?;
                self.const_i64(STATUS_OK)?;
                self.jump(opcodes::JMP, end, false)?;
                // Both paths leave one status operand; the join restarts from
                // the depth the call began with.
                self.depth = start_depth;
                self.bind(full);
                self.const_i64(STATUS_FULL)?;
                self.bind(end);
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.ring_pop" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.ring_pop` expects 2 arguments, got {}", args.len()),
                    ));
                }
                let (place, ring) = self.ring_receiver(args[0], "core.ring_pop", true)?;
                let start_depth = self.depth;
                let empty = self.label()?;
                let end = self.label()?;
                self.push_ring_header(&place, ring.count_off)?;
                self.const_i64(0)?;
                self.simple(opcodes::EQ)?;
                self.pop(1)?;
                self.jump(opcodes::JNZ, empty, true)?;
                self.read_ring_element_into(&place, &ring, args[1], "core.ring_pop")?;
                self.advance_ring_head(&place, &ring)?;
                self.adjust_ring_count(&place, &ring, -1)?;
                self.const_i64(STATUS_OK)?;
                self.jump(opcodes::JMP, end, false)?;
                self.depth = start_depth;
                self.bind(empty);
                self.const_i64(STATUS_NOT_FOUND)?;
                self.bind(end);
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.ring_peek" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.ring_peek` expects 2 arguments, got {}", args.len()),
                    ));
                }
                let (place, ring) = self.ring_receiver(args[0], "core.ring_peek", false)?;
                let start_depth = self.depth;
                let empty = self.label()?;
                let end = self.label()?;
                self.push_ring_header(&place, ring.count_off)?;
                self.const_i64(0)?;
                self.simple(opcodes::EQ)?;
                self.pop(1)?;
                self.jump(opcodes::JNZ, empty, true)?;
                self.read_ring_element_into(&place, &ring, args[1], "core.ring_peek")?;
                self.const_i64(STATUS_OK)?;
                self.jump(opcodes::JMP, end, false)?;
                self.depth = start_depth;
                self.bind(empty);
                self.const_i64(STATUS_NOT_FOUND)?;
                self.bind(end);
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.ring_full" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.ring_full` expects 1 argument, got {}", args.len()),
                    ));
                }
                let (place, ring) = self.ring_receiver(args[0], "core.ring_full", false)?;
                self.push_ring_header(&place, ring.count_off)?;
                self.const_i64(ring.count as i64)?;
                self.simple(opcodes::EQ)?;
                self.pop(1)?;
                Ok(Some(Type::scalar(Kind::Bool)))
            }
            "core.ring_empty" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.ring_empty` expects 1 argument, got {}", args.len()),
                    ));
                }
                let (place, ring) = self.ring_receiver(args[0], "core.ring_empty", false)?;
                self.push_ring_header(&place, ring.count_off)?;
                self.const_i64(0)?;
                self.simple(opcodes::EQ)?;
                self.pop(1)?;
                Ok(Some(Type::scalar(Kind::Bool)))
            }
            // Map operations (spec 0003, "Maps") are built-in procedures that
            // emit one `map_*` opcode carrying the layout: hashing, probing
            // and backward-shift deletion live in the interpreter, once,
            // instead of inlined at every call site. `put`/`remove` mutate the
            // receiver; `get`/`has`/`count` read it.
            "core.map_put" => {
                if args.len() != 3 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.map_put` expects 3 arguments, got {}", args.len()),
                    ));
                }
                let (place, map) = self.map_receiver(args[0], "core.map_put", true)?;
                self.push_value_ref(&place)?;
                self.emit_map_key(&map, args[1], "core.map_put")?;
                self.emit_map_put_value(&map, args[2])?;
                self.emit_map_op(opcodes::MAP_PUT, &map)?;
                self.pop(3)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.map_get" => {
                if args.len() != 3 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.map_get` expects 3 arguments, got {}", args.len()),
                    ));
                }
                let (place, map) = self.map_receiver(args[0], "core.map_get", false)?;
                self.push_value_ref(&place)?;
                self.emit_map_key(&map, args[1], "core.map_get")?;
                self.emit_map_get_value(&map, args[2])?;
                self.emit_map_op(opcodes::MAP_GET, &map)?;
                self.pop(3)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.map_has" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.map_has` expects 2 arguments, got {}", args.len()),
                    ));
                }
                let (place, map) = self.map_receiver(args[0], "core.map_has", false)?;
                self.push_value_ref(&place)?;
                self.emit_map_key(&map, args[1], "core.map_has")?;
                self.emit_map_op(opcodes::MAP_HAS, &map)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Bool)))
            }
            "core.map_remove" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.map_remove` expects 2 arguments, got {}", args.len()),
                    ));
                }
                let (place, map) = self.map_receiver(args[0], "core.map_remove", true)?;
                self.push_value_ref(&place)?;
                self.emit_map_key(&map, args[1], "core.map_remove")?;
                self.emit_map_op(opcodes::MAP_REMOVE, &map)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.map_count" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.map_count` expects 1 argument, got {}", args.len()),
                    ));
                }
                let (place, _map) = self.map_receiver(args[0], "core.map_count", false)?;
                self.push_value_ref(&place)?;
                self.simple(opcodes::MAP_COUNT)?;
                self.pop(1)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // Stream endpoints (spec 0006). A send copies a whole buffer
            // atomically; a receive fills a `bytes` destination and returns
            // the copied count. `try_*` never blocks. All four return the
            // predeclared `Status`/`int` staged result described in spec 0006.
            "core.send" | "core.try_send" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`{target}` expects 2 arguments, got {}", args.len()),
                    ));
                }
                self.emit_stream_handle(args[0], Kind::OutStream, target)?;
                // Any buffer kind is valid data; `emit_ref` also accepts a
                // literal, which is typed by its destination here.
                self.emit_ref(args[1], None)?;
                let op = if target == "core.send" {
                    opcodes::STREAM_SEND
                } else {
                    opcodes::STREAM_TRY_SEND
                };
                self.simple(op)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.recv" | "core.try_recv" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`{target}` expects 2 arguments, got {}", args.len()),
                    ));
                }
                self.emit_stream_handle(args[0], Kind::InStream, target)?;
                let destination = self.emit_var_buffer_ref(args[1])?;
                if destination.kind != Kind::Bytes || destination.cap == 0 {
                    let found = self.type_name(destination);
                    return Err(self.err_at(
                        args[1],
                        format_args!(
                            "`{target}` needs a writable `var bytes[N]` destination, found `{found}`; a partial read into a `string` could split a code point"
                        ),
                    ));
                }
                let op = if target == "core.recv" {
                    opcodes::STREAM_RECV
                } else {
                    opcodes::STREAM_TRY_RECV
                };
                self.one(op, destination.cap)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::scalar(Kind::Int)))
            }
            // `Spawn`/`wait`/`kill` return the predeclared `Status` enum and
            // write their results through `var` out-parameters (spec 0003,
            // E1; spec 0005). `Status` ordinals are the `abi::Status`
            // discriminants, so the interpreter pushes them directly.
            "core.spawn" => {
                if args.len() != 4 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.spawn` expects 4 arguments, got {}", args.len()),
                    ));
                }
                self.emit_ref(args[0], Some(Kind::String))?;
                self.emit_ref(args[1], Some(Kind::String))?;
                let diagnostic = Type::record(DIAGNOSTIC_RECORD, abi::layout::diagnostic::SIZE);
                let diag = self.emit_aggregate_ref(args[2], true, "`var diag`")?;
                if diag != diagnostic {
                    let found = self.type_name(diag);
                    return Err(self.err_at(
                        args[2],
                        format_args!(
                            "`core.spawn` expects `var diag: Diagnostic`, found `{found}`"
                        ),
                    ));
                }
                let id = self.emit_scalar_ref(args[3])?;
                if id.kind != Kind::Int {
                    let found = self.type_name(id);
                    return Err(self.err_at(
                        args[3],
                        format_args!("`core.spawn` expects `var id: int`, found `{found}`"),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::SPAWN)?;
                self.pop(4)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.run" => {
                if args.len() != 4 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.run` expects 4 arguments, got {}", args.len()),
                    ));
                }
                self.emit_ref(args[0], Some(Kind::String))?;
                self.emit_ref(args[1], Some(Kind::String))?;
                let diagnostic = Type::record(DIAGNOSTIC_RECORD, abi::layout::diagnostic::SIZE);
                let diag = self.emit_aggregate_ref(args[2], true, "`var diag`")?;
                if diag != diagnostic {
                    let found = self.type_name(diag);
                    return Err(self.err_at(
                        args[2],
                        format_args!("`core.run` expects `var diag: Diagnostic`, found `{found}`"),
                    ));
                }
                let exit_reason = Type::sum(EXIT_REASON_SUM, abi::layout::exit_reason::SIZE);
                let reason = self.emit_aggregate_ref(args[3], true, "`var reason`")?;
                if reason != exit_reason {
                    let found = self.type_name(reason);
                    return Err(self.err_at(
                        args[3],
                        format_args!(
                            "`core.run` expects `var reason: ExitReason`, found `{found}`"
                        ),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::RUN)?;
                self.pop(4)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.wait" => {
                if args.len() != 2 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.wait` expects 2 arguments, got {}", args.len()),
                    ));
                }
                self.builtin_id(args[0])?;
                let exit_reason = Type::sum(EXIT_REASON_SUM, abi::layout::exit_reason::SIZE);
                let reason = self.emit_aggregate_ref(args[1], true, "`var reason`")?;
                if reason != exit_reason {
                    let found = self.type_name(reason);
                    return Err(self.err_at(
                        args[1],
                        format_args!(
                            "`core.wait` expects `var reason: ExitReason`, found `{found}`"
                        ),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::WAIT)?;
                self.pop(2)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            "core.kill" => {
                if args.len() != 1 {
                    return Err(self.err(
                        line,
                        column,
                        format_args!("`core.kill` expects 1 argument, got {}", args.len()),
                    ));
                }
                self.builtin_id(args[0])?;
                self.one(opcodes::BUILTIN, builtin::KILL)?;
                self.pop(1)?;
                self.push();
                Ok(Some(Type::enumeration(STATUS_ENUM)))
            }
            target => {
                let (signature, function_index) = if let Some(name) = target.strip_prefix("stdlib.")
                {
                    // Qualified library names (`stdlib.trim_into`) resolve in
                    // the embedded unit; the application cannot see the
                    // library's names unqualified (spec 0003, M3).
                    let Some(sigs) = self.library_sigs else {
                        return Err(self.err(
                            line,
                            column,
                            format_args!(
                                "`stdlib` is not imported; add `uses stdlib;` to the `uses` clause"
                            ),
                        ));
                    };
                    let index = find_sig(sigs, name).ok_or_else(|| {
                        self.err(
                            line,
                            column,
                            format_args!("undefined procedure `{target}` in `stdlib`"),
                        )
                    })?;
                    (sigs[index], u32::try_from(index).unwrap_or(u32::MAX))
                } else {
                    if target.contains('.') {
                        return Err(self.err(
                            line,
                            column,
                            format_args!("undefined procedure `{target}`"),
                        ));
                    }
                    let index = find_sig(self.sigs, target).ok_or_else(|| {
                        self.err(line, column, format_args!("undefined procedure `{target}`"))
                    })?;
                    (
                        self.sigs[index],
                        self.call_base
                            .checked_add(u32::try_from(index).unwrap_or(u32::MAX))
                            .ok_or_else(|| self.internal("function index overflows"))?,
                    )
                };
                // Types are compiler-local per unit, so a cross-unit
                // parameter or result that names a record, sum or user enum
                // has no shared identity yet. Scalars, buffers, arrays of
                // those and the two predeclared enums (`Status`, `FaultCode`)
                // are safe; the array table itself is shared by both units.
                if target.starts_with("stdlib.") {
                    let arrays = self.arrays.as_slice();
                    let rings = self.rings.as_slice();
                    let maps = self.maps.as_slice();
                    let incompatible = signature
                        .params
                        .iter()
                        .any(|param| !cross_unit_type_ok(param.ty, arrays, rings, maps))
                        || signature
                            .ret
                            .is_some_and(|ty| !cross_unit_type_ok(ty, arrays, rings, maps));
                    if incompatible {
                        return Err(self.err(
                            line,
                            column,
                            format_args!(
                                "`{target}` uses a record, sum or user enum across units; cross-unit aggregate types are not implemented"
                            ),
                        ));
                    }
                }
                let params = signature.params;
                if args.len() != params.len() {
                    return Err(self.err(
                        line,
                        column,
                        format_args!(
                            "procedure `{target}` expects {} argument(s), got {}",
                            params.len(),
                            args.len()
                        ),
                    ));
                }
                // A function result arrives through a destination reference
                // pushed before the arguments: `[dest][arg...] CALL`.
                let destination = match signature.ret {
                    Some(ty) => {
                        let slot = self.next_local;
                        self.next_local = slot
                            .checked_add(8)
                            .ok_or_else(|| self.internal("frame size overflows"))?;
                        if self.next_local > MAX_CAPACITY {
                            return Err(self.err(
                                line,
                                column,
                                format_args!(
                                    "procedure `{target}` needs more than {MAX_CAPACITY} bytes of locals"
                                ),
                            ));
                        }
                        self.one(opcodes::REF_LOCAL, slot)?;
                        self.push();
                        Some((slot, ty))
                    }
                    None => None,
                };
                for (position, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
                    self.argument(arg, param, position + 1)?;
                }
                self.word(opcodes::CALL, 0, 0, 1)?;
                self.imm32(function_index)?;
                self.pop(params.len() as i32)?;
                match destination {
                    Some((slot, ty)) => {
                        // The destination reference is still below the callee's
                        // arguments; drop it, then load the result.
                        self.simple(opcodes::DROP)?;
                        self.pop(1)?;
                        self.one(opcodes::LOAD_LOCAL, slot)?;
                        self.push();
                        Ok(Some(ty))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    fn argument(
        &mut self,
        arg: &'a Expr<'a>,
        param: &Param<'a>,
        position: usize,
    ) -> Result<(), Diagnostic> {
        if param.ty.is_stream() {
            let found = self.emit_stream_ref(arg)?;
            if found.kind != param.ty.kind {
                let expected = self.type_name(param.ty);
                let found = self.type_name(found);
                return Err(self.err_at(
                    arg,
                    format_args!("argument {position} expects `{expected}`, found `{found}`"),
                ));
            }
            return Ok(());
        }
        if param.ty.is_aggregate() {
            let writable = param.mode == ParamMode::Var;
            let found = self.emit_aggregate_ref(arg, writable, "argument")?;
            if found != param.ty {
                let expected = self.type_name(param.ty);
                let found = self.type_name(found);
                return Err(self.err_at(
                    arg,
                    format_args!("argument {position} expects `{expected}`, found `{found}`"),
                ));
            }
            Ok(())
        } else if param.ty.is_buffer() {
            let found = if param.mode == ParamMode::Var {
                self.emit_var_buffer_ref(arg)?
            } else {
                self.emit_ref(arg, Some(param.ty.kind))?
            };
            if found.kind != param.ty.kind {
                let expected = self.type_name(param.ty);
                let found = self.type_name(found);
                return Err(self.err_at(
                    arg,
                    format_args!("argument {position} expects `{expected}`, found `{found}`"),
                ));
            }
            if found.cap < param.ty.cap {
                return Err(self.err_at(
                    arg,
                    format_args!(
                        "argument {position} has capacity {}, but parameter `{}` requires at least {}",
                        found.cap, param.name, param.ty.cap
                    ),
                ));
            }
            Ok(())
        } else {
            match param.mode {
                ParamMode::Var => {
                    let found = self.emit_scalar_ref(arg)?;
                    if found != param.ty {
                        let expected = self.type_name(param.ty);
                        let found = self.type_name(found);
                        return Err(self.err_at(
                            arg,
                            format_args!(
                                "argument {position} expects `{expected}`, found `{found}`"
                            ),
                        ));
                    }
                    Ok(())
                }
                _ => {
                    let found = self.expr_scalar(arg)?;
                    if found != param.ty {
                        let expected = self.type_name(param.ty);
                        let found = self.type_name(found);
                        return Err(self.err_at(
                            arg,
                            format_args!(
                                "argument {position} expects `{expected}`, found `{found}`"
                            ),
                        ));
                    }
                    Ok(())
                }
            }
        }
    }

    /// Emits a scalar expression and requires it to be an `int` task id.
    /// `TaskId` is still an `int` (spec 0003, "Task ids").
    fn builtin_id(&mut self, expr: &'a Expr<'a>) -> Result<(), Diagnostic> {
        let found = self.expr_scalar(expr)?;
        if found.kind != Kind::Int {
            let found = self.type_name(found);
            return Err(self.err_at(
                expr,
                format_args!("expected an `int` task id, found `{found}`"),
            ));
        }
        Ok(())
    }

    /// Emits the five operands of a `syslib` rectangle primitive
    /// (`fill_rect`, `fill_dither`, `draw_rect`) in declaration order and
    /// checks their staged types (spec 0009, G10).
    fn gfx_rect_operands(
        &mut self,
        what: &str,
        line: u32,
        column: u32,
        args: &'a [&'a Expr<'a>],
    ) -> Result<(), Diagnostic> {
        if args.len() != 5 {
            return Err(self.err(
                line,
                column,
                format_args!("`{what}` expects 5 arguments, got {}", args.len()),
            ));
        }
        for (index, name) in ["x", "y", "w", "h"].iter().enumerate() {
            let value = self.expr_scalar(args[index])?;
            if value.kind != Kind::Int {
                return Err(self.err_at(
                    args[index],
                    format_args!("`{what}` expects an `int` {name}, found `{value}`"),
                ));
            }
        }
        let color = self.expr_scalar(args[4])?;
        if color.kind != Kind::Uint {
            return Err(self.err_at(
                args[4],
                format_args!("`{what}` expects a `uint` color, found `{color}`"),
            ));
        }
        Ok(())
    }

    /// Pushes a reference to a buffer expression. `want` selects the nominal
    /// kind (`string` vs `bytes`) and enables UTF-8 validation of literals.
    fn emit_ref(&mut self, expr: &'a Expr<'a>, want: Option<Kind>) -> Result<Type, Diagnostic> {
        match expr.kind {
            ExprKind::Str(bytes) => {
                if let Some(kind) = want {
                    if kind == Kind::String && core::str::from_utf8(bytes).is_err() {
                        return Err(self.err_at(
                            expr,
                            format_args!(
                                "string literal is not valid UTF-8; use a `bytes` destination for raw data"
                            ),
                        ));
                    }
                    if !kind.is_buffer() {
                        return Err(self.err_at(
                            expr,
                            format_args!("expected a scalar, found a string literal"),
                        ));
                    }
                }
                let kind = match want {
                    Some(kind) if kind.is_buffer() => kind,
                    _ => Kind::String,
                };
                let offset = match self.pool.intern(bytes) {
                    Ok(offset) => offset,
                    Err(_) => return Err(self.arena_error(expr.line, expr.column)),
                };
                self.one(opcodes::REF_CONST, offset)?;
                self.push();
                Ok(Type::buffer(kind, bytes.len() as u32))
            }
            ExprKind::Designator(designator) => {
                let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
                if !place.ty.is_buffer() {
                    let found = self.type_name(place.ty);
                    return Err(self.err_at(
                        expr,
                        format_args!("`{}` is `{found}`, not a buffer", designator.base),
                    ));
                }
                if let Some(kind) = want {
                    if kind.is_buffer() && kind != place.ty.kind {
                        let found = self.type_name(place.ty);
                        return Err(self.err_at(
                            expr,
                            format_args!("expected `{}`, found `{found}`", kind.name()),
                        ));
                    }
                }
                self.push_buffer_data_ref(&place)?;
                Ok(place.ty)
            }
            _ => Err(self.err_at(expr, format_args!("expected a buffer expression"))),
        }
    }

    fn emit_var_buffer_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let place = self.var_buffer_place(expr)?;
        self.push_buffer_data_ref(&place)?;
        Ok(place.ty)
    }

    /// Resolves a stream endpoint designator without emitting code.
    fn stream_place(&self, expr: &'a Expr<'a>, what: &str) -> Result<Place<'a>, Diagnostic> {
        let Some(designator) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects a stream endpoint as its first argument"),
            ));
        };
        let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
        if !place.ty.is_stream() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects a stream endpoint, found `{found}`"),
            ));
        }
        if !place.path.is_empty() {
            return Err(self.err_at(
                expr,
                format_args!("a stream endpoint has no fields or elements"),
            ));
        }
        Ok(place)
    }

    /// Pushes a reference to a stream endpoint's handle. This is how a `const`
    /// stream parameter is passed: the callee borrows the caller's handle.
    fn emit_stream_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let place = self.stream_place(expr, "a stream argument")?;
        self.push_value_ref(&place)?;
        Ok(place.ty)
    }

    /// Pushes the endpoint handle value, checking that the operation uses the
    /// endpoint in its own direction. The compiler's distinct nominal types
    /// already guarantee this; the check keeps the diagnostic specific.
    fn emit_stream_handle(
        &mut self,
        expr: &'a Expr<'a>,
        want: Kind,
        what: &str,
    ) -> Result<Type, Diagnostic> {
        let place = self.stream_place(expr, what)?;
        if place.ty.kind != want {
            let direction = if want == Kind::OutStream {
                "`core.send`/`core.try_send` need an `out_stream`"
            } else {
                "`core.recv`/`core.try_recv` need an `in_stream`"
            };
            let found = self.type_name(place.ty);
            return Err(self.err_at(expr, format_args!("{direction}, found `{found}`")));
        }
        self.push_value_ref(&place)?;
        // The reference names the 8-byte handle; `load_ref` replaces it with
        // the value the kernel wrote at load time.
        self.simple(opcodes::LOAD_REF)?;
        Ok(place.ty)
    }

    /// Resolves a writable buffer designator without emitting its reference.
    fn var_buffer_place(&self, expr: &'a Expr<'a>) -> Result<Place<'a>, Diagnostic> {
        let Some(designator) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("a `var` buffer argument must be a variable or parameter"),
            ));
        };
        let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
        if !place.ty.is_buffer() {
            return Err(self.err_at(expr, format_args!("`{}` is not a buffer", designator.base)));
        }
        if !place.writable {
            return Err(self.err_at(
                expr,
                format_args!(
                    "cannot pass `const` parameter `{}` as `var`",
                    designator.base
                ),
            ));
        }
        Ok(place)
    }

    fn emit_scalar_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let Some(designator) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("a `var` argument must be a variable or parameter"),
            ));
        };
        let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
        if !place.writable {
            return Err(self.err_at(
                expr,
                format_args!(
                    "cannot pass `const` parameter `{}` as `var`",
                    designator.base
                ),
            ));
        }
        if !place.ty.is_scalar() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!("`{}` is `{found}`, not a scalar", designator.base),
            ));
        }
        self.push_value_ref(&place)?;
        Ok(place.ty)
    }

    // ---- rings (spec 0003, "Rings") ------------------------------------

    /// Resolves a `core.ring_*` receiver: a designator of a ring type, and
    /// writable storage when the operation mutates the header (`push`/`pop`).
    ///
    /// A `value` parameter is an ordinary writable copy everywhere in the
    /// language, so pushing/popping through one mutates the callee's copy only,
    /// exactly like a `value` record or array parameter. A `const` parameter is
    /// read-only.
    fn ring_receiver(
        &mut self,
        expr: &'a Expr<'a>,
        what: &str,
        mutate: bool,
    ) -> Result<(Place<'a>, RingDef), Diagnostic> {
        let Some(designator) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects a ring variable as its first argument"),
            ));
        };
        let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
        let Some(ring) = ring_of(self.rings.as_slice(), place.ty) else {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects a `ring[T, N]` first argument, found `{found}`"),
            ));
        };
        if mutate && !place.writable {
            return Err(self.err_at(
                expr,
                format_args!(
                    "cannot mutate `const` parameter `{}`; declare the ring parameter `var`",
                    designator.base
                ),
            ));
        }
        Ok((place, ring))
    }

    /// Pushes the `u64` header word at `off` of a ring (`head` or `count`).
    fn push_ring_header(&mut self, place: &Place<'a>, off: u32) -> Result<(), Diagnostic> {
        self.push_value_ref(place)?;
        self.one(opcodes::REF_OFFSET, off)?;
        self.simple(opcodes::LOAD_REF)?;
        Ok(())
    }

    /// Pushes a reference to the element at `(head + count) % N` (the next
    /// push slot) or at `head` (the oldest element).
    fn push_ring_element(
        &mut self,
        place: &Place<'a>,
        ring: &RingDef,
        at_head: bool,
    ) -> Result<(), Diagnostic> {
        self.push_value_ref(place)?;
        if at_head {
            self.push_ring_header(place, ring.head_off)?;
        } else {
            self.push_ring_header(place, ring.head_off)?;
            self.push_ring_header(place, ring.count_off)?;
            self.simple(opcodes::ADD_U)?;
            self.pop(1)?;
            self.const_i64(ring.count as i64)?;
            self.simple(opcodes::MOD_U)?;
            self.pop(1)?;
        }
        self.word(opcodes::REF_ELEMENT, 0, 0, 0)?;
        self.imm32(ring.stride)?;
        self.imm32(ring.count)?;
        self.pop(1)?;
        Ok(())
    }

    /// Writes the next push slot: scalars and enums store one slot, every
    /// other element is a byte copy of `stride` bytes; a buffer element is
    /// rebuilt with `clear`/`append`, which is the same `[len][data]` image
    /// and needs no padding when the source is shorter.
    fn write_ring_element(
        &mut self,
        place: &Place<'a>,
        ring: &RingDef,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        let elem = ring.elem;
        if elem.is_buffer() {
            return self.write_ring_buffer_element(place, ring, value);
        }
        self.push_ring_element(place, ring, false)?;
        if elem.is_scalar() {
            let found = self.expr_scalar(value)?;
            if found != elem {
                let expected = self.type_name(elem);
                let found = self.type_name(found);
                return Err(self.err_at(
                    value,
                    format_args!("`core.ring_push` expects `{expected}`, found `{found}`"),
                ));
            }
            self.simple(opcodes::STORE_REF)?;
            self.pop(2)?;
        } else {
            let source = self.emit_aggregate_ref(value, false, "a ring element")?;
            if source != elem {
                let expected = self.type_name(elem);
                let found = self.type_name(source);
                return Err(self.err_at(
                    value,
                    format_args!("`core.ring_push` expects `{expected}`, found `{found}`"),
                ));
            }
            self.const_i64(ring.stride as i64)?;
            self.simple(opcodes::COPY_REFS)?;
            self.pop(3)?;
        }
        Ok(())
    }

    /// Writes a buffer element: `clear` then `append`, exactly like a whole
    /// buffer assignment, so a shorter source never over-reads.
    fn write_ring_buffer_element(
        &mut self,
        place: &Place<'a>,
        ring: &RingDef,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        let elem = ring.elem;
        let mut literal: Option<u32> = None;
        match value.kind {
            ExprKind::Str(bytes) => {
                if bytes.len() as u64 > elem.cap as u64 {
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "string literal ({} bytes) does not fit `{}`",
                            bytes.len(),
                            self.type_name(elem)
                        ),
                    ));
                }
                if elem.kind == Kind::String && core::str::from_utf8(bytes).is_err() {
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "string literal is not valid UTF-8; use a `bytes` element for raw data"
                        ),
                    ));
                }
                let offset = match self.pool.intern(bytes) {
                    Ok(offset) => offset,
                    Err(_) => return Err(self.arena_error(value.line, value.column)),
                };
                literal = Some(offset);
            }
            ExprKind::Designator(designator) => {
                let source =
                    self.place(designator.base, designator.path, value.line, value.column)?;
                if !source.ty.is_buffer() {
                    let found = self.type_name(source.ty);
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "`core.ring_push` expects `{}`, found `{found}`",
                            self.type_name(elem)
                        ),
                    ));
                }
                if source.ty.kind != elem.kind {
                    let found = self.type_name(source.ty);
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "cannot use `{found}` in a `{}` element",
                            self.type_name(elem)
                        ),
                    ));
                }
                // A call-scoped buffer parameter's declared capacity is only a
                // lower bound, so that copy is runtime-checked; an inline
                // buffer must match exactly (spec 0003, D11).
                let is_ref = designator.path.is_empty()
                    && matches!(source.base, Base::Local { is_ref: true, .. });
                if !is_ref && source.ty.cap != elem.cap {
                    return Err(self.err_at(
                        value,
                        format_args!(
                            "capacity mismatch: `{}` cannot be pushed into a `{}` element",
                            self.type_name(source.ty),
                            self.type_name(elem)
                        ),
                    ));
                }
            }
            _ => {
                return Err(self.err_at(
                    value,
                    format_args!(
                    "`core.ring_push` accepts a literal or a buffer variable for a buffer element"
                ),
                ))
            }
        }

        // Clear the destination, then append the source data (the element
        // reference points at the `[len][data]` image, so +8 is its data).
        self.push_ring_element(place, ring, false)?;
        self.one(opcodes::REF_OFFSET, 8)?;
        self.simple(opcodes::CLEAR)?;
        self.pop(1)?;
        match literal {
            Some(offset) => {
                if let ExprKind::Str(bytes) = value.kind {
                    if bytes.is_empty() {
                        return Ok(());
                    }
                }
                self.push_ring_element(place, ring, false)?;
                self.one(opcodes::REF_OFFSET, 8)?;
                self.one(opcodes::REF_CONST, offset)?;
                self.push();
            }
            None => {
                self.push_ring_element(place, ring, false)?;
                self.one(opcodes::REF_OFFSET, 8)?;
                let ExprKind::Designator(designator) = value.kind else {
                    return Err(self.internal("ring buffer source is not a designator"));
                };
                let source =
                    self.place(designator.base, designator.path, value.line, value.column)?;
                self.push_buffer_data_ref(&source)?;
            }
        }
        self.one(opcodes::APPEND, elem.cap)?;
        self.pop(2)?;
        Ok(())
    }

    /// Copies the oldest element into a caller-owned `var` out-parameter. The
    /// caller has already proved the ring non-empty, so a failure leaves the
    /// out-parameter untouched.
    fn read_ring_element_into(
        &mut self,
        place: &Place<'a>,
        ring: &RingDef,
        value: &'a Expr<'a>,
        what: &str,
    ) -> Result<(), Diagnostic> {
        let elem = ring.elem;
        let Some(designator) = Self::designator(value) else {
            let expected = self.type_name(elem);
            return Err(self.err_at(
                value,
                format_args!("`{what}` expects `var value: {expected}` as its second argument"),
            ));
        };
        let destination = self.place(designator.base, designator.path, value.line, value.column)?;
        if destination.ty != elem {
            let expected = self.type_name(elem);
            let found = self.type_name(destination.ty);
            return Err(self.err_at(
                value,
                format_args!("`{what}` expects `var value: {expected}`, found `{found}`"),
            ));
        }
        if !destination.writable {
            return Err(self.err_at(
                value,
                format_args!(
                    "cannot pass `const` parameter `{}` as `var`",
                    designator.base
                ),
            ));
        }
        if elem.is_buffer() {
            self.push_buffer_dest_ref(BufferDest::Place(destination))?;
            self.simple(opcodes::CLEAR)?;
            self.pop(1)?;
            self.push_buffer_dest_ref(BufferDest::Place(destination))?;
            self.push_ring_element(place, ring, true)?;
            self.one(opcodes::REF_OFFSET, 8)?;
            self.one(opcodes::APPEND, elem.cap)?;
            self.pop(2)?;
            return Ok(());
        }
        if elem.is_scalar() {
            self.push_value_ref(&destination)?;
            self.push_ring_element(place, ring, true)?;
            self.simple(opcodes::LOAD_REF)?;
            self.simple(opcodes::STORE_REF)?;
            self.pop(2)?;
            return Ok(());
        }
        self.push_value_ref(&destination)?;
        self.push_ring_element(place, ring, true)?;
        self.const_i64(ring.stride as i64)?;
        self.simple(opcodes::COPY_REFS)?;
        self.pop(3)?;
        Ok(())
    }

    /// `head := (head + 1) % N`, after a `pop` removed the oldest element.
    fn advance_ring_head(&mut self, place: &Place<'a>, ring: &RingDef) -> Result<(), Diagnostic> {
        self.push_value_ref(place)?;
        self.one(opcodes::REF_OFFSET, ring.head_off)?;
        self.push_ring_header(place, ring.head_off)?;
        self.const_i64(1)?;
        self.simple(opcodes::ADD_U)?;
        self.pop(1)?;
        self.const_i64(ring.count as i64)?;
        self.simple(opcodes::MOD_U)?;
        self.pop(1)?;
        self.simple(opcodes::STORE_REF)?;
        self.pop(2)?;
        Ok(())
    }

    /// `count := count ± 1` after a successful `push`/`pop`.
    fn adjust_ring_count(
        &mut self,
        place: &Place<'a>,
        ring: &RingDef,
        delta: i64,
    ) -> Result<(), Diagnostic> {
        self.push_value_ref(place)?;
        self.one(opcodes::REF_OFFSET, ring.count_off)?;
        self.push_ring_header(place, ring.count_off)?;
        if delta > 0 {
            self.const_i64(delta)?;
            self.simple(opcodes::ADD_U)?;
        } else {
            self.const_i64(-delta)?;
            self.simple(opcodes::SUB_U)?;
        }
        self.pop(1)?;
        self.simple(opcodes::STORE_REF)?;
        self.pop(2)?;
        Ok(())
    }

    // ---- maps (spec 0003, "Maps") --------------------------------------

    /// Resolves a `core.map_*` receiver: a designator of a map type, and
    /// writable storage when the operation mutates the map (`put`/`remove`).
    ///
    /// A `value` parameter is an ordinary writable copy, so mutating through
    /// one changes the callee's copy only. A `const` parameter is read-only.
    fn map_receiver(
        &mut self,
        expr: &'a Expr<'a>,
        what: &str,
        mutate: bool,
    ) -> Result<(Place<'a>, MapDef), Diagnostic> {
        let Some(designator) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects a map variable as its first argument"),
            ));
        };
        let place = self.place(designator.base, designator.path, expr.line, expr.column)?;
        let Some(map) = map_of(self.maps.as_slice(), place.ty) else {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects a `map[K, V, N]` first argument, found `{found}`"),
            ));
        };
        if mutate && !place.writable {
            return Err(self.err_at(
                expr,
                format_args!(
                    "cannot mutate `const` parameter `{}`; declare the map parameter `var`",
                    designator.base
                ),
            ));
        }
        Ok((place, map))
    }

    /// Emits one `map_*` opcode with the layout immediates: capacity, key
    /// image size and value image size. The key and value kinds ride in the
    /// opcode word (spec 0008, "Maps").
    fn emit_map_op(&mut self, op: u8, map: &MapDef) -> Result<(), Diagnostic> {
        let key_kind = match map.key.kind {
            Kind::Int => opcodes::map_key_kind::INT,
            Kind::Bytes => opcodes::map_key_kind::BYTES,
            Kind::String => opcodes::map_key_kind::STRING,
            _ => return Err(self.internal("a map key has an unsupported kind")),
        };
        let value_kind = if map.value.is_buffer() {
            opcodes::map_value_kind::BUFFER
        } else if map.value.is_scalar() {
            opcodes::map_value_kind::SCALAR
        } else {
            opcodes::map_value_kind::FIXED
        };
        self.word(op, key_kind, value_kind, 0)?;
        self.imm32(map.capacity)?;
        self.imm32(map.count)?;
        self.imm32(map.key_size)?;
        self.imm32(map.value_size)?;
        Ok(())
    }

    /// Pushes one key operand: the 8-byte value of an `int` key, or a data
    /// reference for a buffer key. A buffer key follows the at-least-N rule
    /// (the map only reads it); a literal is typed by the key type.
    fn emit_map_key(
        &mut self,
        map: &MapDef,
        expr: &'a Expr<'a>,
        what: &str,
    ) -> Result<(), Diagnostic> {
        if map.key.kind == Kind::Int {
            // A buffer expression is a clearer diagnostic than the generic
            // "string literal is a buffer" message.
            if let Some(found) = self.probe_buffer(expr) {
                let expected = self.type_name(map.key);
                let found = self.type_name(found);
                return Err(self.err_at(
                    expr,
                    format_args!("`{what}` expects key `{expected}`, found `{found}`"),
                ));
            }
            let found = self.expr_scalar(expr)?;
            if found != map.key {
                let expected = self.type_name(map.key);
                let found = self.type_name(found);
                return Err(self.err_at(
                    expr,
                    format_args!("`{what}` expects key `{expected}`, found `{found}`"),
                ));
            }
            return Ok(());
        }
        let literal = matches!(expr.kind, ExprKind::Str(_));
        if let ExprKind::Str(bytes) = expr.kind {
            if bytes.len() as u64 > map.key.cap as u64 {
                let expected = self.type_name(map.key);
                return Err(self.err_at(
                    expr,
                    format_args!(
                        "string literal ({} bytes) does not fit map key `{expected}`",
                        bytes.len()
                    ),
                ));
            }
        }
        let found = self.emit_ref(expr, Some(map.key.kind))?;
        if found.kind != map.key.kind {
            let expected = self.type_name(map.key);
            let found = self.type_name(found);
            return Err(self.err_at(
                expr,
                format_args!("`{what}` expects key `{expected}`, found `{found}`"),
            ));
        }
        if !literal && found.cap < map.key.cap {
            let expected = self.type_name(map.key);
            let found = self.type_name(found);
            return Err(self.err_at(
                expr,
                format_args!(
                    "`{what}` expects key `{expected}` (at least {} bytes), found `{found}`",
                    map.key.cap
                ),
            ));
        }
        Ok(())
    }

    /// Pushes one value operand for `map_put`: an 8-byte scalar, a reference
    /// to a fixed aggregate, or a data reference for a buffer value. The value
    /// must be exactly `V`; a literal is typed by `V`.
    fn emit_map_put_value(&mut self, map: &MapDef, expr: &'a Expr<'a>) -> Result<(), Diagnostic> {
        let value = map.value;
        if value.is_buffer() {
            let literal = matches!(expr.kind, ExprKind::Str(_));
            if let ExprKind::Str(bytes) = expr.kind {
                if bytes.len() as u64 > value.cap as u64 {
                    let expected = self.type_name(value);
                    return Err(self.err_at(
                        expr,
                        format_args!(
                            "string literal ({} bytes) does not fit map value `{expected}`",
                            bytes.len()
                        ),
                    ));
                }
            }
            let found = self.emit_ref(expr, Some(value.kind))?;
            if found != value && !literal {
                let expected = self.type_name(value);
                let found = self.type_name(found);
                return Err(self.err_at(
                    expr,
                    format_args!("`core.map_put` expects value `{expected}`, found `{found}`"),
                ));
            }
            return Ok(());
        }
        if value.is_scalar() {
            if let Some(found) = self.probe_buffer(expr) {
                let expected = self.type_name(value);
                let found = self.type_name(found);
                return Err(self.err_at(
                    expr,
                    format_args!("`core.map_put` expects value `{expected}`, found `{found}`"),
                ));
            }
            let found = self.expr_scalar(expr)?;
            if found != value {
                let expected = self.type_name(value);
                let found = self.type_name(found);
                return Err(self.err_at(
                    expr,
                    format_args!("`core.map_put` expects value `{expected}`, found `{found}`"),
                ));
            }
            return Ok(());
        }
        let found = self.emit_aggregate_ref(expr, false, "a map value")?;
        if found != value {
            let expected = self.type_name(value);
            let found = self.type_name(found);
            return Err(self.err_at(
                expr,
                format_args!("`core.map_put` expects value `{expected}`, found `{found}`"),
            ));
        }
        Ok(())
    }

    /// Pushes the destination of `map_get`: a writable place of exactly the
    /// value type, referenced the way a value operand is.
    fn emit_map_get_value(&mut self, map: &MapDef, expr: &'a Expr<'a>) -> Result<(), Diagnostic> {
        let value = map.value;
        let found = if value.is_buffer() {
            self.emit_var_buffer_ref(expr)?
        } else if value.is_scalar() {
            self.emit_scalar_ref(expr)?
        } else {
            self.emit_aggregate_ref(expr, true, "`var value`")?
        };
        if found != value {
            let expected = self.type_name(value);
            let found = self.type_name(found);
            return Err(self.err_at(
                expr,
                format_args!("`core.map_get` expects `var value: {expected}`, found `{found}`"),
            ));
        }
        Ok(())
    }
}

/// Structural equality of two access paths, used only by the buffer
/// self-append check. Depth is bounded by the compiler's nesting limit.
fn same_accesses(left: &[Access<'_>], right: &[Access<'_>], depth: u32) -> bool {
    if left.len() != right.len() || depth >= MAX_NESTING {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (Access::Field(a), Access::Field(b)) => a == b,
            (Access::Index(a), Access::Index(b)) => same_expr(a, b, depth + 1),
            _ => false,
        })
}

/// Structural equality of two expressions, sufficient for the aliasing check.
/// A call is never considered equal to anything else.
fn same_expr(left: &Expr<'_>, right: &Expr<'_>, depth: u32) -> bool {
    if depth >= MAX_NESTING {
        return false;
    }
    match (&left.kind, &right.kind) {
        (ExprKind::Int(a), ExprKind::Int(b)) => a == b,
        (ExprKind::Float(a), ExprKind::Float(b)) => a.to_bits() == b.to_bits(),
        (ExprKind::Str(a), ExprKind::Str(b)) => a == b,
        (ExprKind::Bool(a), ExprKind::Bool(b)) => a == b,
        (ExprKind::Designator(a), ExprKind::Designator(b)) => {
            a.base == b.base && same_accesses(a.path, b.path, depth + 1)
        }
        (ExprKind::Cast(a, ae), ExprKind::Cast(b, be)) => a == b && same_expr(ae, be, depth + 1),
        (ExprKind::Unary(aop, ae), ExprKind::Unary(bop, be)) => {
            aop == bop && same_expr(ae, be, depth + 1)
        }
        (ExprKind::Binary(aop, al, ar), ExprKind::Binary(bop, bl, br)) => {
            aop == bop && same_expr(al, bl, depth + 1) && same_expr(ar, br, depth + 1)
        }
        _ => false,
    }
}

fn collect_concat<'a, 'b>(
    expr: &'a Expr<'a>,
    parts: &mut SliceVec<'a, 'b, &'a Expr<'a>>,
    depth: u32,
    path: &str,
    unit: &str,
) -> Result<(), Diagnostic> {
    if depth >= MAX_NESTING {
        return Err(diag(
            path,
            unit,
            expr.line,
            expr.column,
            format_args!("nesting too deep (limit {MAX_NESTING})"),
        ));
    }
    if let ExprKind::Binary(BinOp::Add, left, right) = expr.kind {
        collect_concat(left, parts, depth + 1, path, unit)?;
        collect_concat(right, parts, depth + 1, path, unit)?;
    } else {
        parts
            .push(expr)
            .map_err(|_| out_of_space(path, unit, expr.line, expr.column, Resource::Arena))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predeclared tables must agree with `abi::layout` field for field; a
    /// drift here would make the kernel and the compiler disagree about where
    /// a diagnostic field lives.
    fn assert_layout(fields: &[RecordField<'_>], size: u32) {
        let mut next = 0u32;
        for field in fields {
            next = align8(next);
            assert_eq!(field.off, next, "field `{}` offset", field.name);
            next += field.ty.size();
        }
        assert_eq!(align8(next), size, "record size");
    }

    #[test]
    fn predeclared_layouts_match_the_shared_layout() {
        assert_layout(&DIAGNOSTIC_FIELDS, layout::diagnostic::SIZE);
        assert_eq!(find_record(&predeclared_records(), "Diagnostic"), Some(0));

        // `ExitReason` is a sum: 8-byte tag at 0, the `FaultCode` payload at 8,
        // and a size fixed by `abi::layout` (spec 0003, "Sum types").
        assert_eq!(layout::exit_reason::TAG_OFFSET, 0);
        assert_eq!(layout::exit_reason::PAYLOAD_OFFSET, 8);
        assert_eq!(FAULT_CODE_ENUM, 1);
        assert_eq!(EXIT_REASON_SUM, 0);
        let sum = SumDef {
            name: "ExitReason",
            variants: &EXIT_REASON_VARIANTS,
            size: layout::exit_reason::SIZE,
        };
        assert_eq!(sum.size, 8 + Type::enumeration(FAULT_CODE_ENUM).size());
        assert_eq!(find_sum(&predeclared_sums(), "ExitReason"), Some(0));
        assert_eq!(
            variant_of(&sum, "Faulted").and_then(|variant| variant.payload),
            Some(Type::enumeration(FAULT_CODE_ENUM))
        );
        assert_eq!(
            variant_of(&sum, "Ok").and_then(|variant| variant.payload),
            None
        );
        assert_eq!(
            variant_of(&sum, "Killed").and_then(|variant| variant.payload),
            None
        );
    }

    fn predeclared_records() -> [RecordDef<'static>; 1] {
        [RecordDef {
            name: "Diagnostic",
            fields: &DIAGNOSTIC_FIELDS,
            size: layout::diagnostic::SIZE,
        }]
    }

    fn predeclared_sums() -> [SumDef<'static>; 1] {
        [SumDef {
            name: "ExitReason",
            variants: &EXIT_REASON_VARIANTS,
            size: layout::exit_reason::SIZE,
        }]
    }
}

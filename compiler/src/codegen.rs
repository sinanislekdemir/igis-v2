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
//! A buffer local occupies `[len: u64][data: cap]`; `ref_local`/`ref_state`
//! point at `data`, and every buffer instruction reads `len` at `data - 8`.
//!
//! A sum value is an 8-byte tag at offset 0 and the payload area at offset 8.
//! Sum types add no opcode: construction stores the tag with `store_ref` and
//! the payload with the payload type's ordinary instructions, and `case`
//! compares the tag and copies the payload into an arm-local slot.
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

#[derive(Debug, Clone, Copy)]
struct StateVar<'a> {
    name: &'a str,
    ty: Type,
    off: u32,
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
    line: u32,
    column: u32,
}

#[derive(Clone, Copy)]
struct CompiledFn<'a> {
    code: &'a [u8],
    frame_size: u32,
    operand_off: u32,
    param_count: u8,
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
        "int" | "uint" | "byte" | "bool" | "f64" | "string" | "bytes"
    )
}

/// Compiles one parsed unit into a package.
pub(crate) fn compile_unit<'a>(
    unit: &Unit<'a>,
    path: &'a str,
    bump: &Bump<'a>,
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    let name = unit.name;
    if name.is_empty() {
        return Err(diag(path, "", 1, 1, format_args!("unit has no name")));
    }
    for used in unit.uses {
        if *used != "core" && *used != "syslib" {
            return Err(diag(
                path,
                name,
                1,
                1,
                format_args!("unknown unit `{used}`; this slice provides only `core` and `syslib`"),
            ));
        }
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
    let mut state_size = 0u32;
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
        let ty = resolve_type(
            &decl.ty,
            consts.as_slice(),
            enums.as_slice(),
            records.as_slice(),
            sums.as_slice(),
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
            let ty = resolve_type(
                &param.ty,
                consts.as_slice(),
                enums.as_slice(),
                records.as_slice(),
                sums.as_slice(),
                path,
                name,
            )?;
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
        sigs.push(Sig {
            name: proc.name,
            params,
            line: proc.line,
            column: proc.column,
        })
        .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
    }

    let main_index = match find_sig(sigs.as_slice(), "main") {
        Some(index) => index,
        None => {
            return Err(diag(
                path,
                name,
                1,
                1,
                format_args!(
                    "missing `main` procedure; an application needs `procedure main(args: const string[1024]);`"
                ),
            ))
        }
    };
    let main_sig = sigs.as_slice()[main_index];
    let main_ok = main_sig.params.len() == 1
        && main_sig.params[0].mode == ParamMode::Const
        && main_sig.params[0].ty == Type::buffer(Kind::String, 1024);
    if !main_ok {
        return Err(diag(
            path,
            name,
            main_sig.line,
            main_sig.column,
            format_args!("main must be declared as `procedure main(args: const string[1024]);`"),
        ));
    }

    let mut pool = ConstantPool::new(path, name, bump)?;
    let mut compiled =
        SliceVec::<CompiledFn<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    for (index, proc) in unit.procs.iter().enumerate() {
        let current = sigs.as_slice()[index];
        let mut compiler = FnCompiler::new(
            path,
            name,
            sigs.as_slice(),
            consts.as_slice(),
            state_vars.as_slice(),
            enums.as_slice(),
            records.as_slice(),
            sums.as_slice(),
            &mut pool,
            current,
            bump,
        )?;
        compiler.compile_procedure(proc)?;
        let function = compiler.finish()?;
        compiled
            .push(function)
            .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
    }

    // Lay the functions out in declaration order and rebase their line tables.
    let mut instructions = SliceVec::<u8>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    let mut functions =
        SliceVec::<abi::Function>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    let mut lines = SliceVec::<LineEntry>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    let mut stack_size = STACK_MARGIN;
    for (index, function) in compiled.as_slice().iter().enumerate() {
        let proc = unit.procs[index];
        let code_off = u32::try_from(instructions.len()).map_err(|_| {
            diag(
                path,
                name,
                proc.line,
                proc.column,
                format_args!("instruction section exceeds 4 GiB"),
            )
        })?;
        let line_base = u32::try_from(lines.len()).map_err(|_| {
            diag(
                path,
                name,
                proc.line,
                proc.column,
                format_args!("line table is too large"),
            )
        })?;
        for entry in function.lines {
            lines
                .push(LineEntry {
                    off: entry.off + code_off,
                    line: entry.line,
                    column: entry.column,
                })
                .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
        }
        functions
            .push(abi::Function {
                code_off,
                frame_size: function.frame_size,
                operand_off: function.operand_off,
                line_base,
                name_off: abi::NO_NAME,
                param_count: function.param_count,
                has_destination: false,
                _reserved: [0, 0],
            })
            .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
        instructions
            .extend_from_slice(function.code)
            .map_err(|_| arena_full(path, name, proc.line, proc.column))?;
        stack_size = stack_size.checked_add(function.frame_size).ok_or_else(|| {
            diag(
                path,
                name,
                proc.line,
                proc.column,
                format_args!("stack size overflows"),
            )
        })?;
    }
    stack_size = align8(stack_size);
    if stack_size > MAX_CAPACITY {
        return Err(diag(
            path,
            name,
            1,
            1,
            format_args!("stack size {stack_size} exceeds the {MAX_CAPACITY}-byte cap"),
        ));
    }

    package::assemble(
        path,
        name,
        functions.as_slice(),
        instructions.as_slice(),
        pool.data.as_slice(),
        lines.as_slice(),
        state_size,
        stack_size,
        main_index as u32,
        code,
        manifest,
    )
}

fn resolve_type<'a>(
    ty: &TypeRef<'a>,
    consts: &[Const<'a>],
    enums: &[EnumDef<'a>],
    records: &[RecordDef<'a>],
    sums: &[SumDef<'a>],
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
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
        ExprKind::Ident(name) => find_const(consts, name).ok_or_else(|| {
            error(format_args!(
                "unknown constant `{name}` in a constant expression"
            ))
        }),
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
}

impl Target {
    fn ty(&self) -> Type {
        match self {
            Target::LocalScalar { ty, .. }
            | Target::StateScalar { ty, .. }
            | Target::LocalBuffer { ty, .. }
            | Target::StateBuffer { ty, .. }
            | Target::LocalRecord { ty, .. }
            | Target::StateRecord { ty, .. }
            | Target::LocalSum { ty, .. }
            | Target::StateSum { ty, .. } => *ty,
        }
    }
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

/// A resolved designator: a base variable plus a field path.
///
/// `base_ty` is the declared type of the base variable; each path element
/// selects a record field. With an empty path this is the variable itself.
#[derive(Debug, Clone, Copy)]
struct Place<'a> {
    base: Base,
    base_ty: Type,
    path: &'a [&'a str],
    ty: Type,
    writable: bool,
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
struct TypeName<'a, 'b> {
    ty: Type,
    enums: &'b [EnumDef<'a>],
    records: &'b [RecordDef<'a>],
    sums: &'b [SumDef<'a>],
}

impl fmt::Display for TypeName<'_, '_> {
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
    fn type_name(&self, ty: Type) -> TypeName<'a, '_> {
        TypeName {
            ty,
            enums: self.enums,
            records: self.records,
            sums: self.sums,
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
            let aggregate = param.ty.is_record() || param.ty.is_sum();
            let is_ref = param.mode == ParamMode::Var
                || param.ty.is_buffer()
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
        if self.depth != 0 {
            return Err(self.internal("operand stack is not empty at the end of a procedure"));
        }
        self.simple(opcodes::RET)?;
        self.finish_jumps()?;
        Ok(())
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
            let entry = LineEntry { off, line, column };
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
                match target {
                    AssignTarget::Name(name) => self.assign(name, value, line, column),
                    AssignTarget::Field { base, path } => {
                        self.assign_field(base, path, value, line, column)
                    }
                    AssignTarget::Index(name, index) => {
                        self.assign_index(name, index, value, line, column)
                    }
                }
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
        let Some((base, path)) = Self::designator(value) else {
            return Err(self.err_at(
                value,
                format_args!("a `case` subject must be a variable or a field"),
            ));
        };
        let place = self.place(base, path, value.line, value.column)?;
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
            self.push_place_ref(&place)?;
            self.simple(opcodes::LOAD_REF)?;
            self.const_i64(ordinal as i64)?;
            self.simple(opcodes::EQ)?;
            self.pop(1)?;
            self.jump(opcodes::JZ, next, true)?;

            let slot_count = self.slots.len();
            if let (Some(payload_ty), Some(binding)) = (variant.payload, arm.binding) {
                self.one(opcodes::REF_LOCAL, payload_base)?;
                self.push();
                self.push_place_ref(&place)?;
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

    fn assign(
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
                self.buffer_assign(BufferDest::Place(place), Some((name, &[])), value)?;
            }
            Target::LocalRecord { .. } | Target::StateRecord { .. } => {
                let place = self.place_of_target(&target);
                self.assign_record(&place, value)?;
            }
            Target::LocalSum { .. } | Target::StateSum { .. } => {
                let place = self.place_of_target(&target);
                self.assign_sum(&place, value)?;
            }
        }
        Ok(())
    }

    /// Assigns one byte of a buffer: `name[index] := value`.
    ///
    /// Only `bytes` is writable by index. Writing into a `string` would break
    /// the UTF-8 invariant one byte at a time, so it is a compile error
    /// (spec 0003). The declared capacity is carried in the instruction; the
    /// interpreter extends the runtime length to `index + 1` when the index is
    /// at or past the current length.
    fn assign_index(
        &mut self,
        name: &'a str,
        index: &'a Expr<'a>,
        value: &'a Expr<'a>,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        let target = self.resolve_target(name, line, column)?;
        let ty = match target {
            Target::LocalBuffer { .. } | Target::StateBuffer { .. } => target.ty(),
            _ => {
                return Err(self.err(
                    line,
                    column,
                    format_args!("cannot index `{name}`; it is not a buffer"),
                ))
            }
        };
        if ty.kind != Kind::Bytes {
            return Err(self.err(
                line,
                column,
                format_args!(
                    "cannot assign into `{ty}` by index; `string` is read-only by index (UTF-8)"
                ),
            ));
        }
        self.push_target_ref(&target)?;
        self.check_index_type(index)?;
        let value_ty = self.expr_scalar(value)?;
        if value_ty.kind != Kind::Byte {
            return Err(self.err_at(
                value,
                format_args!(
                    "`{name}[...] := ...` needs a `byte`, found `{value_ty}`; convert the value with `byte(x)`"
                ),
            ));
        }
        self.one(opcodes::STORE_U8, ty.cap)?;
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
            } else {
                Target::LocalScalar {
                    off: slot.base,
                    ty: slot.ty,
                    is_ref: slot.is_ref,
                }
            });
        }
        if let Some(var) = find_state(self.state_vars, name) {
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

    fn push_target_ref(&mut self, target: &Target) -> Result<(), Diagnostic> {
        match *target {
            Target::LocalBuffer {
                base, is_ref: true, ..
            } => self.one(opcodes::LOAD_LOCAL, base)?,
            Target::LocalBuffer {
                base,
                is_ref: false,
                ..
            } => self.one(opcodes::REF_LOCAL, base + 8)?,
            Target::StateBuffer { off, .. } => self.one(opcodes::REF_STATE, off + 8)?,
            _ => unreachable!("push_target_ref is only used for buffers"),
        }
        self.push();
        Ok(())
    }

    // ---- places (variables and record fields) ---------------------------

    /// A designator is an identifier or a field path; both name storage that a
    /// reference can point at.
    fn designator(expr: &'a Expr<'a>) -> Option<(&'a str, &'a [&'a str])> {
        match expr.kind {
            ExprKind::Ident(name) => Some((name, &[])),
            ExprKind::Field { base, path } => Some((base, path)),
            _ => None,
        }
    }

    /// Resolves a base name and a field path to a [`Place`], checking every
    /// field against the record table.
    fn place(
        &self,
        base_name: &'a str,
        path: &'a [&'a str],
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
        for name in path {
            let Some(record) = record_of(self.records, ty) else {
                return Err(self.err(
                    line,
                    column,
                    format_args!("`{base_name}` is `{ty}`, which has no field `{name}`"),
                ));
            };
            let Some(field) = record.fields.iter().find(|field| field.name == *name) else {
                return Err(self.err(
                    line,
                    column,
                    format_args!("`{}` has no field `{name}`", record.name),
                ));
            };
            ty = field.ty;
        }
        Ok(Place {
            base,
            base_ty,
            path,
            ty,
            writable,
        })
    }

    /// Builds the [`Place`] of a local/state variable with no field path.
    fn place_of_target(&self, target: &Target) -> Place<'a> {
        match *target {
            Target::LocalBuffer { base, ty, is_ref } | Target::LocalRecord { base, ty, is_ref } => {
                Place {
                    base: Base::Local { off: base, is_ref },
                    base_ty: ty,
                    path: &[],
                    ty,
                    writable: true,
                }
            }
            Target::LocalSum { base, ty, is_ref } => Place {
                base: Base::Local { off: base, is_ref },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
            },
            Target::StateBuffer { off, ty } | Target::StateRecord { off, ty } => Place {
                base: Base::State { off },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
            },
            Target::StateSum { off, ty } => Place {
                base: Base::State { off },
                base_ty: ty,
                path: &[],
                ty,
                writable: true,
            },
            Target::LocalScalar { .. } | Target::StateScalar { .. } => {
                unreachable!("scalars are not places")
            }
        }
    }

    /// Pushes a reference to the storage a [`Place`] names.
    ///
    /// A buffer is referenced at its data (the length word sits just before);
    /// a scalar or record is referenced at its first byte. A field reference is
    /// formed with `ref_offset`, one step per field; the interpreter
    /// bounds-checks every intermediate offset.
    fn push_place_ref(&mut self, place: &Place<'a>) -> Result<(), Diagnostic> {
        let extra = if place.path.is_empty() && place.ty.is_buffer() {
            8
        } else {
            0
        };
        match place.base {
            Base::Local { off, is_ref: true } => self.one(opcodes::LOAD_LOCAL, off)?,
            Base::Local { off, is_ref: false } => self.one(opcodes::REF_LOCAL, off + extra)?,
            Base::State { off } => self.one(opcodes::REF_STATE, off + extra)?,
        }
        self.push();

        let mut ty = place.base_ty;
        let count = place.path.len();
        for (index, name) in place.path.iter().enumerate() {
            let field = field_of(self.records, ty, name)
                .ok_or_else(|| self.internal("unresolved field in a place"))?;
            let last = index + 1 == count;
            let extra = if last && field.ty.is_buffer() { 8 } else { 0 };
            self.one(opcodes::REF_OFFSET, field.off + extra)?;
            ty = field.ty;
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

    /// Emits a reference to a record or sum expression and returns its type.
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
        let Some((base, path)) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("{what} must be a variable or a field of a record or sum type"),
            ));
        };
        let place = self.place(base, path, expr.line, expr.column)?;
        if !place.ty.is_record() && !place.ty.is_sum() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(
                expr,
                format_args!("{what} expects a record or sum value, found `{found}`"),
            ));
        }
        if writable && !place.writable {
            return Err(self.err_at(
                expr,
                format_args!("cannot pass `const` value `{base}` as `var`"),
            ));
        }
        self.push_place_ref(&place)?;
        Ok(place.ty)
    }

    /// Assigns a whole record: a byte copy of the destination-size region.
    fn assign_record(
        &mut self,
        destination: &Place<'a>,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        self.push_place_ref(destination)?;
        let source = self.emit_aggregate_ref(value, false, "a record assignment")?;
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
            ExprKind::Ident(name) => {
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
                } else if Self::designator(value).is_some() {
                    self.copy_sum(destination, value)
                } else {
                    Err(self.err_at(
                        value,
                        format_args!("`{name}` is not a variant of `{}`", sum.name),
                    ))
                }
            }
            ExprKind::Field { .. } => self.copy_sum(destination, value),
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
        self.push_place_ref(destination)?;
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
        self.push_place_ref(destination)?;
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
        self.push_place_ref(destination)?;
        self.one(opcodes::REF_OFFSET, 8)?;
        if payload_ty.is_record() || payload_ty.is_sum() {
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

    /// Assigns one field: a whole-record copy, a buffer construction, or a
    /// scalar store through the field reference.
    fn assign_field(
        &mut self,
        base: &'a str,
        path: &'a [&'a str],
        value: &'a Expr<'a>,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        let place = self.place(base, path, line, column)?;
        if !place.writable {
            return Err(self.err(
                line,
                column,
                format_args!("cannot assign to a field of `const` parameter `{base}`"),
            ));
        }
        if place.ty.is_record() {
            return self.assign_record(&place, value);
        }
        if place.ty.is_sum() {
            return self.assign_sum(&place, value);
        }
        if place.ty.is_buffer() {
            return self.buffer_assign(BufferDest::Place(place), Some((base, path)), value);
        }
        self.push_place_ref(&place)?;
        let value_ty = self.expr_scalar(value)?;
        self.check_exact(place.ty, value_ty, value)?;
        self.simple(opcodes::STORE_REF)?;
        self.pop(2)?;
        Ok(())
    }

    /// Pushes a reference to the data area a buffer construction writes.
    fn push_buffer_dest_ref(&mut self, destination: BufferDest<'a>) -> Result<(), Diagnostic> {
        match destination {
            BufferDest::Place(place) => self.push_place_ref(&place),
            BufferDest::SumPayload { sum, .. } => {
                self.push_place_ref(&sum)?;
                self.one(opcodes::REF_OFFSET, 16)
            }
        }
    }

    /// Assigns a whole buffer field or variable: a construction from literals
    /// and other buffers (`clear` then `append`), never a temporary.
    ///
    /// `home` names the destination designator for the aliasing check; a
    /// variable passes its name, a field passes its base and path.
    fn buffer_assign(
        &mut self,
        destination: BufferDest<'a>,
        home: Option<(&'a str, &'a [&'a str])>,
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
                ExprKind::Ident(operand) => {
                    let found = match self.slot(operand) {
                        Some(slot) => Some((slot.ty, slot.is_ref)),
                        None => find_state(self.state_vars, operand).map(|var| (var.ty, false)),
                    };
                    match found {
                        Some((part_ty, is_ref)) if part_ty.is_buffer() => {
                            if part_ty.kind != ty.kind {
                                return Err(self.err(
                                    part.line,
                                    part.column,
                                    format_args!("cannot use `{part_ty}` in a `{ty}` construction"),
                                ));
                            }
                            // Direct assignment between two inline buffers is a
                            // type assignment, so the capacities must match
                            // exactly (spec 0003, D11). A `const`/`var` buffer
                            // parameter is a call-scoped reference whose declared
                            // capacity is only a lower bound, so copying one is a
                            // runtime-checked construction.
                            if parts.len() == 1 && !is_ref && part_ty.cap != ty.cap {
                                return Err(self.err(
                                    part.line,
                                    part.column,
                                    format_args!(
                                        "capacity mismatch: `{part_ty}` cannot be assigned to `{ty}`"
                                    ),
                                ));
                            }
                        }
                        Some(_) => {
                            return Err(self.err(
                                part.line,
                                part.column,
                                format_args!("`{operand}` is not a buffer"),
                            ))
                        }
                        None => {
                            return Err(self.err(
                                part.line,
                                part.column,
                                format_args!("unknown identifier `{operand}`"),
                            ))
                        }
                    }
                }
                ExprKind::Field { base, path } => {
                    let place = self.place(base, path, part.line, part.column)?;
                    if !place.ty.is_buffer() {
                        let found = self.type_name(place.ty);
                        return Err(self.err(
                            part.line,
                            part.column,
                            format_args!("`{base}.` field is `{found}`, not a buffer"),
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
                    // A record field has its nominal capacity; there is no
                    // at-least-N relaxation for fields.
                    if parts.len() == 1 && place.ty.cap != ty.cap {
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
            self.push_buffer_dest_ref(destination)?;
            match part.kind {
                ExprKind::Str(bytes) => {
                    if bytes.is_empty() {
                        self.pop(1)?;
                        continue;
                    }
                    let offset = match self.pool.intern(bytes) {
                        Ok(offset) => offset,
                        Err(_) => return Err(self.arena_error(part.line, part.column)),
                    };
                    self.one(opcodes::REF_CONST, offset)?;
                    self.push();
                }
                ExprKind::Ident(operand) => {
                    self.push_named_ref(operand, part)?;
                }
                ExprKind::Field { base, path } => {
                    let place = self.place(base, path, part.line, part.column)?;
                    self.push_place_ref(&place)?;
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
    fn aliases(home: Option<(&'a str, &'a [&'a str])>, part: &Expr<'a>) -> bool {
        let Some((home_base, home_path)) = home else {
            return false;
        };
        match part.kind {
            ExprKind::Ident(name) => name == home_base && home_path.is_empty(),
            ExprKind::Field { base, path } => base == home_base && path == home_path,
            _ => false,
        }
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
            ExprKind::Ident(name) => self.load_scalar(name, expr),
            ExprKind::Field { base, path } => {
                // A qualified enum variant, `TypeName.Variant`. `FaultCode`
                // requires this form because its names overlap `Status`.
                if path.len() == 1 {
                    if let Some((id, ordinal)) = find_qualified_variant(self.enums, base, path[0]) {
                        self.const_i64(ordinal as i64)?;
                        return Ok(Type::enumeration(id));
                    }
                }
                let place = self.place(base, path, expr.line, expr.column)?;
                if place.ty.is_buffer() {
                    return Err(self.err_at(
                        expr,
                        format_args!("buffer `{base}` field cannot be used as a scalar value"),
                    ));
                }
                if place.ty.is_record() {
                    let found = self.type_name(place.ty);
                    return Err(self.err_at(
                        expr,
                        format_args!(
                            "record `{found}` cannot be used as a scalar value; select a field"
                        ),
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
                self.push_place_ref(&place)?;
                self.simple(opcodes::LOAD_REF)?;
                // `push_place_ref` counts the reference; `load_ref` replaces it
                // with the value, so the depth does not change.
                Ok(place.ty)
            }
            ExprKind::Index(name, index) => self.load_index(name, index, expr),
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

    /// Loads one byte of a buffer: `name[index]`.
    ///
    /// The result is `byte` for both `string` and `bytes`; a `string` index
    /// yields the raw byte at that offset, not a code point (spec 0003, T4).
    /// The interpreter checks the index against the runtime length and faults
    /// `OutOfBounds` past it. A literal index is not folded; it is checked at
    /// run time like any other.
    fn load_index(
        &mut self,
        name: &'a str,
        index: &'a Expr<'a>,
        expr: &'a Expr<'a>,
    ) -> Result<Type, Diagnostic> {
        self.buffer_type(name, expr)?;
        self.push_named_ref(name, expr)?;
        self.check_index_type(index)?;
        self.simple(opcodes::LOAD_U8)?;
        self.pop(1)?;
        Ok(Type::scalar(Kind::Byte))
    }

    /// Returns the buffer type of a buffer expression without emitting code.
    ///
    /// A string literal reports `string[len]`; that kind is only a default,
    /// because a literal is typed by its destination (spec 0003, T1).
    fn probe_buffer(&self, expr: &Expr<'a>) -> Option<Type> {
        match expr.kind {
            ExprKind::Str(bytes) => Some(Type::buffer(Kind::String, bytes.len() as u32)),
            ExprKind::Ident(name) => {
                if let Some(slot) = self.slot(name) {
                    if slot.ty.is_buffer() {
                        return Some(slot.ty);
                    }
                }
                find_state(self.state_vars, name)
                    .filter(|var| var.ty.is_buffer())
                    .map(|var| var.ty)
            }
            ExprKind::Field { base, path } => {
                let place = self.place(base, path, expr.line, expr.column).ok()?;
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

    fn load_scalar(&mut self, name: &'a str, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        if let Some(value) = find_const(self.consts, name) {
            self.const_i64(value)?;
            return Ok(Type::scalar(Kind::Int));
        }
        if let Some(slot) = self.slot(name) {
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
            Kind::Record(_) | Kind::Sum(_) => unreachable!(),
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
            // `core.substr_into(dst, src, start: int, len: int) -> int`. Staged:
            // spec 0003 T4 targets `stdlib.substr_into`; until `stdlib` exists
            // the primitive is a `core` opcode. Same kind only; a `string`
            // rejects boundaries that split a code point at run time.
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
                // T1); a named source must match exactly.
                if !matches!(args[1].kind, ExprKind::Str(_)) {
                    if let Some(source) = self.probe_buffer(args[1]) {
                        if source.kind != destination.kind {
                            return Err(self.err_at(
                                args[1],
                                format_args!(
                                    "`core.substr_into` needs `src` and `dst` of the same kind; `{destination}` and `{source}` differ"
                                ),
                            ));
                        }
                    }
                }
                self.emit_ref(args[1], Some(destination.kind))?;
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
                self.word(opcodes::SUBSTR, kind, 0, 0)?;
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
            // First `syslib` primitives: x86 port I/O (spec 0003, "Hardware
            // access"). `Port` is staged as a plain `uint` (Q45); the namespace
            // is still separate from `core`, so a hardware access stays
            // visible as such. MMIO and DMA are not implemented.
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
                let params = self.sigs[index].params;
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
                for (position, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
                    self.argument(arg, param, position + 1)?;
                }
                self.word(opcodes::CALL, 0, 0, 1)?;
                self.imm32(index as u32)?;
                self.pop(params.len() as i32)?;
                Ok(None)
            }
        }
    }

    fn argument(
        &mut self,
        arg: &'a Expr<'a>,
        param: &Param<'a>,
        position: usize,
    ) -> Result<(), Diagnostic> {
        if param.ty.is_record() || param.ty.is_sum() {
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
            ExprKind::Ident(name) => {
                let ty = self.buffer_type(name, expr)?;
                if let Some(kind) = want {
                    if kind.is_buffer() && kind != ty.kind {
                        return Err(self.err_at(
                            expr,
                            format_args!("expected `{}`, found `{ty}`", kind.name()),
                        ));
                    }
                }
                self.push_named_ref(name, expr)?;
                Ok(ty)
            }
            ExprKind::Field { base, path } => {
                let place = self.place(base, path, expr.line, expr.column)?;
                if !place.ty.is_buffer() {
                    let found = self.type_name(place.ty);
                    return Err(self.err_at(
                        expr,
                        format_args!("field `{base}` is `{found}`, not a buffer"),
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
                self.push_place_ref(&place)?;
                Ok(place.ty)
            }
            _ => Err(self.err_at(expr, format_args!("expected a buffer expression"))),
        }
    }

    fn emit_var_buffer_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let Some((base, path)) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("a `var` buffer argument must be a variable or parameter"),
            ));
        };
        let place = self.place(base, path, expr.line, expr.column)?;
        if !place.ty.is_buffer() {
            return Err(self.err_at(expr, format_args!("`{base}` is not a buffer")));
        }
        if !place.writable {
            return Err(self.err_at(
                expr,
                format_args!("cannot pass `const` parameter `{base}` as `var`"),
            ));
        }
        self.push_place_ref(&place)?;
        Ok(place.ty)
    }

    fn emit_scalar_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let Some((base, path)) = Self::designator(expr) else {
            return Err(self.err_at(
                expr,
                format_args!("a `var` argument must be a variable or parameter"),
            ));
        };
        let place = self.place(base, path, expr.line, expr.column)?;
        if !place.writable {
            return Err(self.err_at(
                expr,
                format_args!("cannot pass `const` parameter `{base}` as `var`"),
            ));
        }
        if !place.ty.is_scalar() {
            let found = self.type_name(place.ty);
            return Err(self.err_at(expr, format_args!("`{base}` is `{found}`, not a scalar")));
        }
        self.push_place_ref(&place)?;
        Ok(place.ty)
    }

    fn buffer_type(&self, name: &str, expr: &Expr<'a>) -> Result<Type, Diagnostic> {
        if let Some(slot) = self.slot(name) {
            if slot.ty.is_buffer() {
                return Ok(slot.ty);
            }
            return Err(self.err_at(expr, format_args!("`{name}` is not a buffer")));
        }
        if let Some(var) = find_state(self.state_vars, name) {
            if var.ty.is_buffer() {
                return Ok(var.ty);
            }
            return Err(self.err_at(expr, format_args!("`{name}` is not a buffer")));
        }
        Err(self.err_at(expr, format_args!("unknown identifier `{name}`")))
    }

    fn push_named_ref(&mut self, name: &str, expr: &'a Expr<'a>) -> Result<(), Diagnostic> {
        if let Some(slot) = self.slot(name) {
            if !slot.ty.is_buffer() {
                return Err(self.err_at(expr, format_args!("`{name}` is not a buffer")));
            }
            if slot.is_ref {
                self.one(opcodes::LOAD_LOCAL, slot.base)?;
            } else {
                self.one(opcodes::REF_LOCAL, slot.base + 8)?;
            }
            self.push();
            return Ok(());
        }
        if let Some(var) = find_state(self.state_vars, name) {
            if !var.ty.is_buffer() {
                return Err(self.err_at(expr, format_args!("`{name}` is not a buffer")));
            }
            self.one(opcodes::REF_STATE, var.off + 8)?;
            self.push();
            return Ok(());
        }
        Err(self.err_at(expr, format_args!("unknown identifier `{name}`")))
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

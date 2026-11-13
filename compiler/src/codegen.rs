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
//! Name resolution is table-driven, not hash-driven: a unit is small, so
//! constants, state variables, procedures and locals are looked up by a linear
//! scan of an arena-backed slice. That keeps declaration order observable and
//! removes every hash map from the pipeline (spec 0008, B9).

use core::fmt;

use abi::opcodes::{self, builtin};

use crate::arena::{Bump, OutOfSpace, SliceVec};
use crate::ast::*;
use crate::package::LineEntry;
use crate::types::{align8, Kind, Type, MAX_CAPACITY};
use crate::{diag, out_of_space, package, Diagnostic, Package, Resource};

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
        if *used != "core" {
            return Err(diag(
                path,
                name,
                1,
                1,
                format_args!("unknown unit `{used}`; this slice provides only `core`"),
            ));
        }
    }

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
        let value = eval_const(decl.value, consts.as_slice(), path, name)?;
        consts
            .push(Const {
                name: decl.name,
                value,
            })
            .map_err(|_| arena_full(path, name, decl.line, decl.column))?;
    }

    let mut state_vars =
        SliceVec::<StateVar<'a>>::new(bump).map_err(|_| arena_full(path, name, 1, 1))?;
    let mut state_size = 0u32;
    for decl in unit.vars {
        if find_const(consts.as_slice(), decl.name).is_some()
            || find_state(state_vars.as_slice(), decl.name).is_some()
        {
            return Err(diag(
                path,
                name,
                decl.line,
                decl.column,
                format_args!("duplicate declaration of `{}`", decl.name),
            ));
        }
        let ty = resolve_type(&decl.ty, consts.as_slice(), path, name)?;
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
            let ty = resolve_type(&param.ty, consts.as_slice(), path, name)?;
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
    path: &'a str,
    unit: &'a str,
) -> Result<Type, Diagnostic> {
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
                let value = eval_const(expr, consts, path, unit)?;
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
) -> Result<i64, Diagnostic> {
    let error = |message: fmt::Arguments<'_>| diag(path, unit, expr.line, expr.column, message);
    match expr.kind {
        ExprKind::Int(value) => Ok(value),
        ExprKind::Ident(name) => find_const(consts, name).ok_or_else(|| {
            error(format_args!(
                "unknown constant `{name}` in a constant expression"
            ))
        }),
        ExprKind::Unary(UnOp::Neg, inner) => {
            let value = eval_const(inner, consts, path, unit)?;
            value
                .checked_neg()
                .ok_or_else(|| error(format_args!("constant expression overflows")))
        }
        ExprKind::Binary(op, left, right) => {
            let a = eval_const(left, consts, path, unit)?;
            let b = eval_const(right, consts, path, unit)?;
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
}

impl Target {
    fn ty(&self) -> Type {
        match self {
            Target::LocalScalar { ty, .. }
            | Target::StateScalar { ty, .. }
            | Target::LocalBuffer { ty, .. }
            | Target::StateBuffer { ty, .. } => *ty,
        }
    }
}

struct FnCompiler<'a, 'b, 'p> {
    path: &'a str,
    unit: &'a str,
    sigs: &'b [Sig<'a>],
    consts: &'b [Const<'a>],
    state_vars: &'b [StateVar<'a>],
    pool: &'p mut ConstantPool<'a, 'b>,
    current: Sig<'a>,
    bump: &'b Bump<'a>,
    code: SliceVec<'a, 'b, u8>,
    slots: SliceVec<'a, 'b, Slot<'a>>,
    next_local: u32,
    depth: i32,
    max_depth: i32,
    labels: SliceVec<'a, 'b, Option<usize>>,
    pending: SliceVec<'a, 'b, Pending>,
    lines: SliceVec<'a, 'b, LineEntry>,
}

impl<'a, 'b, 'p> FnCompiler<'a, 'b, 'p> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        path: &'a str,
        unit: &'a str,
        sigs: &'b [Sig<'a>],
        consts: &'b [Const<'a>],
        state_vars: &'b [StateVar<'a>],
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
            pool,
            current,
            bump,
            code,
            slots,
            next_local: 0,
            depth: 0,
            max_depth: 0,
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
        for param in self.current.params {
            let base = self.next_local;
            self.next_local += 8;
            let is_ref = param.mode == ParamMode::Var || param.ty.is_buffer();
            let slot = Slot {
                name: param.name,
                base,
                ty: param.ty,
                writable: param.mode != ParamMode::Const,
                is_ref,
            };
            self.slots
                .push(slot)
                .map_err(|_| self.arena_error(self.current.line, self.current.column))?;
        }
        for local in proc.locals {
            if self.slot(local.name).is_some() {
                return Err(self.err(
                    local.line,
                    local.column,
                    format_args!("duplicate declaration of `{}`", local.name),
                ));
            }
            let ty = resolve_type(&local.ty, self.consts, self.path, self.unit)?;
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
                self.assign(target, value, line, column)
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
                self.buffer_assign(name, &target, value)?;
            }
        }
        Ok(())
    }

    fn check_exact(&self, expected: Type, found: Type, expr: &Expr<'_>) -> Result<(), Diagnostic> {
        if expected == found {
            Ok(())
        } else {
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
        Err(self.err(line, column, format_args!("unknown identifier `{name}`")))
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

    fn buffer_assign(
        &mut self,
        name: &'a str,
        target: &Target,
        value: &'a Expr<'a>,
    ) -> Result<(), Diagnostic> {
        let ty = target.ty();
        let bump = self.bump;
        let mut parts = SliceVec::<&'a Expr<'a>>::new(bump)
            .map_err(|_| self.arena_error(value.line, value.column))?;
        collect_concat(value, &mut parts)
            .map_err(|_| self.arena_error(value.line, value.column))?;

        for index in 0..parts.len() {
            let part = parts.as_slice()[index];
            match part.kind {
                ExprKind::Ident(operand) if operand == name => {
                    return Err(self.err(
                        part.line,
                        part.column,
                        format_args!(
                            "destination `{name}` aliases an operand; `{name} := {name} + ...` is not allowed"
                        ),
                    ));
                }
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
                _ => {
                    return Err(self.err(
                        part.line,
                        part.column,
                        format_args!("buffer construction accepts only literals, buffers and `+`"),
                    ))
                }
            }
        }

        self.push_target_ref(target)?;
        self.simple(opcodes::CLEAR)?;
        self.pop(1)?;

        for index in 0..parts.len() {
            let part = parts.as_slice()[index];
            match part.kind {
                ExprKind::Str(bytes) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    let offset = match self.pool.intern(bytes) {
                        Ok(offset) => offset,
                        Err(_) => return Err(self.arena_error(part.line, part.column)),
                    };
                    self.push_target_ref(target)?;
                    self.one(opcodes::REF_CONST, offset)?;
                    self.push();
                }
                ExprKind::Ident(operand) => {
                    self.push_target_ref(target)?;
                    self.push_named_ref(operand, part)?;
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

    // ---- expressions ---------------------------------------------------

    fn expr_scalar(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
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
            self.one(opcodes::LOAD_STATE, var.off)?;
            self.push();
            return Ok(var.ty);
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

        let left_ty = self.expr_scalar(left)?;
        let right_ty = self.expr_scalar(right)?;
        if left_ty != right_ty {
            return Err(self.err_at(
                expr,
                format_args!(
                    "type mismatch: `{left_ty}` and `{right_ty}`; there are no implicit conversions"
                ),
            ));
        }
        if left_ty.is_buffer() {
            return Err(self.err_at(
                expr,
                format_args!("buffer comparison is not implemented in this slice"),
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
        if param.ty.is_buffer() {
            let found = if param.mode == ParamMode::Var {
                self.emit_var_buffer_ref(arg)?
            } else {
                self.emit_ref(arg, Some(param.ty.kind))?
            };
            if found.kind != param.ty.kind {
                return Err(self.err_at(
                    arg,
                    format_args!(
                        "argument {position} expects `{}`, found `{found}`",
                        param.ty
                    ),
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
                        return Err(self.err_at(
                            arg,
                            format_args!(
                                "argument {position} expects `{}`, found `{found}`",
                                param.ty
                            ),
                        ));
                    }
                    Ok(())
                }
                _ => {
                    let found = self.expr_scalar(arg)?;
                    if found != param.ty {
                        return Err(self.err_at(
                            arg,
                            format_args!(
                                "argument {position} expects `{}`, found `{found}`",
                                param.ty
                            ),
                        ));
                    }
                    Ok(())
                }
            }
        }
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
            _ => Err(self.err_at(expr, format_args!("expected a buffer expression"))),
        }
    }

    fn emit_var_buffer_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let name = match expr.kind {
            ExprKind::Ident(name) => name,
            _ => {
                return Err(self.err_at(
                    expr,
                    format_args!("a `var` buffer argument must be a variable or parameter"),
                ))
            }
        };
        let target = self.resolve_target(name, expr.line, expr.column)?;
        match target {
            Target::LocalBuffer { .. } | Target::StateBuffer { .. } => {
                let ty = target.ty();
                self.push_target_ref(&target)?;
                Ok(ty)
            }
            _ => Err(self.err_at(expr, format_args!("`{name}` is not a buffer"))),
        }
    }

    fn emit_scalar_ref(&mut self, expr: &'a Expr<'a>) -> Result<Type, Diagnostic> {
        let name = match expr.kind {
            ExprKind::Ident(name) => name,
            _ => {
                return Err(self.err_at(
                    expr,
                    format_args!("a `var` argument must be a variable or parameter"),
                ))
            }
        };
        if let Some(slot) = self.slot(name) {
            if !slot.writable {
                return Err(self.err_at(
                    expr,
                    format_args!("cannot pass `const` parameter `{name}` as `var`"),
                ));
            }
            if slot.ty.is_buffer() {
                return Err(self.err_at(expr, format_args!("`{name}` is not a scalar")));
            }
            if slot.is_ref {
                self.one(opcodes::LOAD_LOCAL, slot.base)?;
            } else {
                self.one(opcodes::REF_LOCAL, slot.base)?;
            }
            self.push();
            return Ok(slot.ty);
        }
        if let Some(var) = find_state(self.state_vars, name) {
            if var.ty.is_buffer() {
                return Err(self.err_at(expr, format_args!("`{name}` is not a scalar")));
            }
            self.one(opcodes::REF_STATE, var.off)?;
            self.push();
            return Ok(var.ty);
        }
        if find_const(self.consts, name).is_some() {
            return Err(self.err_at(expr, format_args!("cannot pass constant `{name}` as `var`")));
        }
        Err(self.err_at(expr, format_args!("unknown identifier `{name}`")))
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
) -> Result<(), OutOfSpace> {
    if let ExprKind::Binary(BinOp::Add, left, right) = expr.kind {
        collect_concat(left, parts)?;
        collect_concat(right, parts)?;
    } else {
        parts.push(expr)?;
    }
    Ok(())
}

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

use std::collections::{HashMap, HashSet};

use abi::opcodes::{self, builtin};

use crate::ast::*;
use crate::package::LineEntry;
use crate::types::{align8, Kind, Type, MAX_CAPACITY};
use crate::{diag, package, Diagnostic};

/// Bytes reserved at the bottom of every frame (spec 0008, "Frames" plus the
/// saved `return_sp` and the destination reference this ABI version adds).
pub const FRAME_HEADER: u32 = 32;

/// Extra stack kept above the sum of all frame sizes, so a shallow call chain
/// has room even when a caller's operand stack is not the deepest one.
const STACK_MARGIN: u32 = 128;

struct StateVar {
    ty: Type,
    off: u32,
}

#[derive(Clone)]
struct Param {
    name: String,
    mode: ParamMode,
    ty: Type,
}

struct Sig {
    name: String,
    params: Vec<Param>,
    line: u32,
    column: u32,
}

struct CompiledFn {
    code: Vec<u8>,
    frame_size: u32,
    operand_off: u32,
    param_count: u8,
    lines: Vec<LineEntry>,
}

/// Interns buffer literals into the constant blob, one entry per distinct byte
/// string, in first-use order.
#[derive(Default)]
struct ConstantPool {
    bytes: Vec<u8>,
    index: HashMap<Vec<u8>, u32>,
}

impl ConstantPool {
    fn intern(&mut self, data: &[u8]) -> u32 {
        if let Some(offset) = self.index.get(data) {
            return *offset;
        }
        let base = self.bytes.len() as u32;
        self.bytes
            .extend_from_slice(&(data.len() as u64).to_le_bytes());
        self.bytes.extend_from_slice(data);
        while !self.bytes.len().is_multiple_of(8) {
            self.bytes.push(0);
        }
        let offset = base + 8;
        self.index.insert(data.to_vec(), offset);
        offset
    }
}

/// Compiles one parsed unit into a package.
pub(crate) fn compile_unit(unit: &Unit, path: &str) -> Result<crate::Package, Diagnostic> {
    if unit.name.is_empty() {
        return Err(diag(path, "", 1, 1, "unit has no name"));
    }
    for used in &unit.uses {
        if used != "core" {
            return Err(diag(
                path,
                &unit.name,
                1,
                1,
                format!("unknown unit `{used}`; this slice provides only `core`"),
            ));
        }
    }

    let mut consts: HashMap<String, i64> = HashMap::new();
    for decl in &unit.consts {
        if consts.contains_key(&decl.name) {
            return Err(diag(
                path,
                &unit.name,
                decl.line,
                decl.column,
                format!("duplicate declaration of `{}`", decl.name),
            ));
        }
        let value = eval_const(&decl.value, &consts, path, &unit.name)?;
        consts.insert(decl.name.clone(), value);
    }

    let mut state_vars: HashMap<String, StateVar> = HashMap::new();
    let mut state_size = 0u32;
    for decl in &unit.vars {
        if consts.contains_key(&decl.name) || state_vars.contains_key(&decl.name) {
            return Err(diag(
                path,
                &unit.name,
                decl.line,
                decl.column,
                format!("duplicate declaration of `{}`", decl.name),
            ));
        }
        let ty = resolve_type(&decl.ty, &consts, path, &unit.name)?;
        if ty.is_buffer() && ty.cap == 0 {
            return Err(diag(
                path,
                &unit.name,
                decl.line,
                decl.column,
                format!(
                    "buffer variable `{}` needs a capacity of at least 1",
                    decl.name
                ),
            ));
        }
        let off = align8(state_size);
        state_size = off.checked_add(ty.size()).ok_or_else(|| {
            diag(
                path,
                &unit.name,
                decl.line,
                decl.column,
                "state region is too large",
            )
        })?;
        if state_size > MAX_CAPACITY {
            return Err(diag(
                path,
                &unit.name,
                decl.line,
                decl.column,
                format!("state region exceeds the {MAX_CAPACITY}-byte cap"),
            ));
        }
        state_vars.insert(decl.name.clone(), StateVar { ty, off });
    }

    let mut sigs: Vec<Sig> = Vec::new();
    let mut sig_index: HashMap<String, usize> = HashMap::new();
    for proc in &unit.procs {
        if consts.contains_key(&proc.name)
            || state_vars.contains_key(&proc.name)
            || sig_index.contains_key(&proc.name)
        {
            return Err(diag(
                path,
                &unit.name,
                proc.line,
                proc.column,
                format!("duplicate declaration of `{}`", proc.name),
            ));
        }
        if proc.params.len() > u8::MAX as usize {
            return Err(diag(
                path,
                &unit.name,
                proc.line,
                proc.column,
                "too many parameters (limit is 255)",
            ));
        }
        let mut params = Vec::new();
        let mut seen = HashSet::new();
        for param in &proc.params {
            if !seen.insert(param.name.clone()) {
                return Err(diag(
                    path,
                    &unit.name,
                    param.line,
                    param.column,
                    format!("duplicate parameter `{}`", param.name),
                ));
            }
            let ty = resolve_type(&param.ty, &consts, path, &unit.name)?;
            if ty.is_buffer() {
                if param.mode == ParamMode::Value {
                    return Err(diag(
                        path,
                        &unit.name,
                        param.line,
                        param.column,
                        format!(
                            "buffer parameter `{}` must be declared `const` or `var`",
                            param.name
                        ),
                    ));
                }
                if param.mode == ParamMode::Var && ty.cap == 0 {
                    return Err(diag(
                        path,
                        &unit.name,
                        param.line,
                        param.column,
                        format!(
                            "`var` buffer parameter `{}` needs a capacity of at least 1",
                            param.name
                        ),
                    ));
                }
            }
            params.push(Param {
                name: param.name.clone(),
                mode: param.mode,
                ty,
            });
        }
        sig_index.insert(proc.name.clone(), sigs.len());
        sigs.push(Sig {
            name: proc.name.clone(),
            params,
            line: proc.line,
            column: proc.column,
        });
    }

    let main_index = match sig_index.get("main") {
        Some(index) => *index,
        None => {
            return Err(diag(
                path,
                &unit.name,
                1,
                1,
                "missing `main` procedure; an application needs `procedure main(args: const string[1024]);`",
            ))
        }
    };
    let main_sig = &sigs[main_index];
    let main_ok = main_sig.params.len() == 1
        && main_sig.params[0].mode == ParamMode::Const
        && main_sig.params[0].ty == Type::buffer(Kind::String, 1024);
    if !main_ok {
        return Err(diag(
            path,
            &unit.name,
            main_sig.line,
            main_sig.column,
            "main must be declared as `procedure main(args: const string[1024]);`",
        ));
    }

    let mut pool = ConstantPool::default();
    let mut compiled: Vec<CompiledFn> = Vec::with_capacity(unit.procs.len());
    for (index, proc) in unit.procs.iter().enumerate() {
        let mut compiler = FnCompiler {
            path,
            unit: &unit.name,
            sigs: &sigs,
            sig_index: &sig_index,
            consts: &consts,
            state_vars: &state_vars,
            pool: &mut pool,
            current: &sigs[index],
            code: Vec::new(),
            slots: HashMap::new(),
            next_local: 0,
            depth: 0,
            max_depth: 0,
            labels: Vec::new(),
            pending: Vec::new(),
            lines: Vec::new(),
        };
        compiler.compile_procedure(proc)?;
        compiled.push(compiler.finish()?);
    }

    // Lay the functions out in declaration order and rebase their line tables.
    let mut instructions = Vec::new();
    let mut functions = Vec::new();
    let mut lines = Vec::new();
    let mut stack_size = STACK_MARGIN;
    for (index, function) in compiled.iter().enumerate() {
        let code_off = u32::try_from(instructions.len()).map_err(|_| {
            diag(
                path,
                &unit.name,
                unit.procs[index].line,
                unit.procs[index].column,
                "instruction section exceeds 4 GiB",
            )
        })?;
        let line_base = u32::try_from(lines.len()).map_err(|_| {
            diag(
                path,
                &unit.name,
                unit.procs[index].line,
                unit.procs[index].column,
                "line table is too large",
            )
        })?;
        for entry in &function.lines {
            lines.push(LineEntry {
                off: entry.off + code_off,
                line: entry.line,
                column: entry.column,
            });
        }
        functions.push(abi::Function {
            code_off,
            frame_size: function.frame_size,
            operand_off: function.operand_off,
            line_base,
            name_off: abi::NO_NAME,
            param_count: function.param_count,
            has_destination: false,
            _reserved: [0, 0],
        });
        instructions.extend_from_slice(&function.code);
        stack_size = stack_size.checked_add(function.frame_size).ok_or_else(|| {
            diag(
                path,
                &unit.name,
                unit.procs[index].line,
                unit.procs[index].column,
                "stack size overflows",
            )
        })?;
    }
    stack_size = align8(stack_size);
    if stack_size > MAX_CAPACITY {
        return Err(diag(
            path,
            &unit.name,
            1,
            1,
            format!("stack size {stack_size} exceeds the {MAX_CAPACITY}-byte cap"),
        ));
    }

    package::assemble(
        path,
        &unit.name,
        functions,
        instructions,
        pool.bytes,
        lines,
        state_size,
        stack_size,
        main_index as u32,
    )
}

fn resolve_type(
    ty: &TypeRef,
    consts: &HashMap<String, i64>,
    path: &str,
    unit: &str,
) -> Result<Type, Diagnostic> {
    let kind = match ty.name.as_str() {
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
                format!("unknown type `{other}`"),
            ))
        }
    };
    if kind.is_buffer() {
        let cap = match &ty.cap {
            Some(expr) => {
                let value = eval_const(expr, consts, path, unit)?;
                if value < 0 {
                    return Err(diag(
                        path,
                        unit,
                        expr.line,
                        expr.column,
                        format!("capacity {value} is negative"),
                    ));
                }
                if value > MAX_CAPACITY as i64 {
                    return Err(diag(
                        path,
                        unit,
                        expr.line,
                        expr.column,
                        format!("capacity {value} exceeds the {MAX_CAPACITY}-byte cap"),
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
                    format!(
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
                format!("scalar type `{}` does not take a capacity", kind.name()),
            ));
        }
        Ok(Type::scalar(kind))
    }
}

fn eval_const(
    expr: &Expr,
    consts: &HashMap<String, i64>,
    path: &str,
    unit: &str,
) -> Result<i64, Diagnostic> {
    let error = |message: String| diag(path, unit, expr.line, expr.column, message);
    match &expr.kind {
        ExprKind::Int(value) => Ok(*value),
        ExprKind::Ident(name) => consts.get(name).copied().ok_or_else(|| {
            error(format!(
                "unknown constant `{name}` in a constant expression"
            ))
        }),
        ExprKind::Unary(UnOp::Neg, inner) => {
            let value = eval_const(inner, consts, path, unit)?;
            value
                .checked_neg()
                .ok_or_else(|| error("constant expression overflows".to_string()))
        }
        ExprKind::Binary(op, left, right) => {
            let a = eval_const(left, consts, path, unit)?;
            let b = eval_const(right, consts, path, unit)?;
            let overflow = || error("constant expression overflows".to_string());
            match op {
                BinOp::Add => a.checked_add(b).ok_or_else(overflow),
                BinOp::Sub => a.checked_sub(b).ok_or_else(overflow),
                BinOp::Mul => a.checked_mul(b).ok_or_else(overflow),
                BinOp::Div => {
                    if b == 0 {
                        return Err(error("division by zero in constant expression".to_string()));
                    }
                    a.checked_div(b).ok_or_else(overflow)
                }
                BinOp::Mod => {
                    if b == 0 {
                        return Err(error("division by zero in constant expression".to_string()));
                    }
                    a.checked_rem(b).ok_or_else(overflow)
                }
                _ => Err(error(
                    "only `+`, `-`, `*`, `div` and `mod` are constant expressions".to_string(),
                )),
            }
        }
        _ => Err(error("constant expression required".to_string())),
    }
}

struct Slot {
    base: u32,
    ty: Type,
    writable: bool,
    is_ref: bool,
}

struct Pending {
    label: usize,
    imm_pos: usize,
    next: usize,
    op_pos: usize,
}

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

struct FnCompiler<'a> {
    path: &'a str,
    unit: &'a str,
    sigs: &'a [Sig],
    sig_index: &'a HashMap<String, usize>,
    consts: &'a HashMap<String, i64>,
    state_vars: &'a HashMap<String, StateVar>,
    pool: &'a mut ConstantPool,
    current: &'a Sig,
    code: Vec<u8>,
    slots: HashMap<String, Slot>,
    next_local: u32,
    depth: i32,
    max_depth: i32,
    labels: Vec<Option<usize>>,
    pending: Vec<Pending>,
    lines: Vec<LineEntry>,
}

impl<'a> FnCompiler<'a> {
    fn err(&self, line: u32, column: u32, message: impl Into<String>) -> Diagnostic {
        diag(self.path, self.unit, line, column, message)
    }

    fn err_at(&self, expr: &Expr, message: impl Into<String>) -> Diagnostic {
        self.err(expr.line, expr.column, message)
    }

    fn internal(&self, message: &str) -> Diagnostic {
        self.err(
            self.current.line,
            self.current.column,
            format!(
                "internal compiler error in `{}`: {message}",
                self.current.name
            ),
        )
    }

    fn compile_procedure(&mut self, proc: &ProcDecl) -> Result<(), Diagnostic> {
        for param in &self.current.params {
            let base = self.next_local;
            self.next_local += 8;
            let is_ref = param.mode == ParamMode::Var || param.ty.is_buffer();
            self.slots.insert(
                param.name.clone(),
                Slot {
                    base,
                    ty: param.ty,
                    writable: param.mode != ParamMode::Const,
                    is_ref,
                },
            );
        }
        for local in &proc.locals {
            if self.slots.contains_key(&local.name) {
                return Err(self.err(
                    local.line,
                    local.column,
                    format!("duplicate declaration of `{}`", local.name),
                ));
            }
            let ty = resolve_type(&local.ty, self.consts, self.path, self.unit)?;
            if ty.is_buffer() && ty.cap == 0 {
                return Err(self.err(
                    local.line,
                    local.column,
                    format!(
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
                    format!(
                        "procedure `{}` needs more than {MAX_CAPACITY} bytes of locals",
                        proc.name
                    ),
                ));
            }
            self.slots.insert(
                local.name.clone(),
                Slot {
                    base,
                    ty,
                    writable: true,
                    is_ref: false,
                },
            );
        }

        for stmt in &proc.body {
            self.stmt(stmt)?;
        }
        if self.depth != 0 {
            return Err(self.internal("operand stack is not empty at the end of a procedure"));
        }
        self.simple(opcodes::RET);
        self.finish_jumps()?;
        Ok(())
    }

    fn finish(self) -> Result<CompiledFn, Diagnostic> {
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
            return Err(self.err(
                self.current.line,
                self.current.column,
                format!(
                    "procedure `{}` needs a {frame_size}-byte frame, over the {MAX_CAPACITY}-byte cap",
                    self.current.name
                ),
            ));
        }
        Ok(CompiledFn {
            code: self.code,
            frame_size,
            operand_off,
            param_count: self.current.params.len() as u8,
            lines: self.lines,
        })
    }

    // ---- emission ------------------------------------------------------

    fn word(&mut self, op: u8, a: u8, b: u8, c: u8) -> usize {
        let pos = self.code.len();
        self.code.extend_from_slice(&[op, a, b, c]);
        pos
    }

    fn imm32(&mut self, value: u32) {
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    fn simple(&mut self, op: u8) {
        self.word(op, 0, 0, 0);
    }

    fn one(&mut self, op: u8, imm: u32) {
        self.word(op, 0, 0, 0);
        self.imm32(imm);
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

    fn mark(&mut self, line: u32, column: u32) {
        let off = self.code.len() as u32;
        if self.lines.last().is_none_or(|entry| entry.off != off) {
            self.lines.push(LineEntry { off, line, column });
        }
    }

    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }

    fn bind(&mut self, label: usize) {
        self.labels[label] = Some(self.code.len());
    }

    fn jump(&mut self, op: u8, label: usize, pops: bool) -> Result<(), Diagnostic> {
        let pos = self.word(op, 0, 0, 0);
        self.imm32(0);
        if pops {
            self.pop(1)?;
        }
        self.pending.push(Pending {
            label,
            imm_pos: pos + 4,
            next: self.code.len(),
            op_pos: pos,
        });
        Ok(())
    }

    fn finish_jumps(&mut self) -> Result<(), Diagnostic> {
        for pending in &self.pending {
            let target =
                self.labels[pending.label].ok_or_else(|| self.internal("unbound label"))?;
            let delta = target as i64 - pending.next as i64;
            if delta % 4 != 0 {
                return Err(self.internal("jump target is not word aligned"));
            }
            let words = delta / 4;
            if words < i32::MIN as i64 || words > i32::MAX as i64 {
                return Err(self.internal("jump offset overflows"));
            }
            self.code[pending.imm_pos..pending.imm_pos + 4]
                .copy_from_slice(&(words as i32).to_le_bytes());
            if target < pending.op_pos {
                // Explicit poll hint on backward branches (spec 0008, B3).
                self.code[pending.op_pos + 3] = 1;
            }
        }
        Ok(())
    }

    fn const_i64(&mut self, value: i64) {
        self.simple(opcodes::CONST);
        self.imm32(value as u32);
        self.imm32((value >> 32) as u32);
        self.push();
    }

    // ---- statements ----------------------------------------------------

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Assign {
                target,
                value,
                line,
                column,
            } => {
                self.mark(*line, *column);
                self.assign(target, value, *line, *column)
            }
            Stmt::Call(call) => {
                self.mark(call.line, call.column);
                let result = self.call(call)?;
                if result.is_some() {
                    self.simple(opcodes::DROP);
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
                        let else_label = self.label();
                        let end_label = self.label();
                        self.condition(cond)?;
                        self.jump(opcodes::JZ, else_label, true)?;
                        self.stmt(then_branch)?;
                        self.jump(opcodes::JMP, end_label, false)?;
                        self.bind(else_label);
                        self.stmt(else_branch)?;
                        self.bind(end_label);
                    }
                    None => {
                        let end_label = self.label();
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
                let start = self.label();
                let end = self.label();
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

    fn condition(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.expr_scalar(expr)?;
        if ty.kind != Kind::Bool {
            return Err(self.err_at(
                expr,
                format!("condition must be `bool`, found `{}`", ty.display()),
            ));
        }
        Ok(())
    }

    fn assign(
        &mut self,
        name: &str,
        value: &Expr,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        let target = self.resolve_target(name, line, column)?;
        match &target {
            Target::LocalScalar {
                off,
                ty,
                is_ref: false,
            } => {
                let value_ty = self.expr_scalar(value)?;
                self.check_exact(*ty, value_ty, value)?;
                self.one(opcodes::STORE_LOCAL, *off);
                self.pop(1)?;
            }
            Target::LocalScalar {
                off,
                ty,
                is_ref: true,
            } => {
                // A `var` scalar parameter aliases the caller's variable: push
                // the reference first, then the value, then store through it.
                self.one(opcodes::LOAD_LOCAL, *off);
                self.push();
                let value_ty = self.expr_scalar(value)?;
                self.check_exact(*ty, value_ty, value)?;
                self.simple(opcodes::STORE_REF);
                self.pop(2)?;
            }
            Target::StateScalar { off, ty } => {
                let value_ty = self.expr_scalar(value)?;
                self.check_exact(*ty, value_ty, value)?;
                self.one(opcodes::STORE_STATE, *off);
                self.pop(1)?;
            }
            Target::LocalBuffer { .. } | Target::StateBuffer { .. } => {
                self.buffer_assign(name, &target, value)?;
            }
        }
        Ok(())
    }

    fn check_exact(&self, expected: Type, found: Type, expr: &Expr) -> Result<(), Diagnostic> {
        if expected == found {
            Ok(())
        } else {
            Err(self.err_at(
                expr,
                format!(
                    "type mismatch: expected `{}`, found `{}`",
                    expected.display(),
                    found.display()
                ),
            ))
        }
    }

    fn resolve_target(&self, name: &str, line: u32, column: u32) -> Result<Target, Diagnostic> {
        if let Some(slot) = self.slots.get(name) {
            if !slot.writable {
                return Err(self.err(
                    line,
                    column,
                    format!("cannot assign to `const` parameter `{name}`"),
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
        if let Some(var) = self.state_vars.get(name) {
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
        if self.consts.contains_key(name) {
            return Err(self.err(line, column, format!("cannot assign to constant `{name}`")));
        }
        Err(self.err(line, column, format!("unknown identifier `{name}`")))
    }

    fn push_target_ref(&mut self, target: &Target) {
        match target {
            Target::LocalBuffer {
                base, is_ref: true, ..
            } => self.one(opcodes::LOAD_LOCAL, *base),
            Target::LocalBuffer {
                base,
                is_ref: false,
                ..
            } => self.one(opcodes::REF_LOCAL, *base + 8),
            Target::StateBuffer { off, .. } => self.one(opcodes::REF_STATE, *off + 8),
            _ => unreachable!("push_target_ref is only used for buffers"),
        }
        self.push();
    }

    fn buffer_assign(
        &mut self,
        name: &str,
        target: &Target,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let ty = target.ty();
        let mut parts = Vec::new();
        collect_concat(value, &mut parts);

        for part in &parts {
            match &part.kind {
                ExprKind::Ident(operand) if operand == name => {
                    return Err(self.err(
                        part.line,
                        part.column,
                        format!("destination `{name}` aliases an operand; `{name} := {name} + ...` is not allowed"),
                    ));
                }
                ExprKind::Str(bytes) => {
                    if bytes.len() as u64 > ty.cap as u64 {
                        return Err(self.err(
                            part.line,
                            part.column,
                            format!(
                                "string literal ({} bytes) does not fit {}",
                                bytes.len(),
                                ty.display()
                            ),
                        ));
                    }
                    if ty.kind == Kind::String && std::str::from_utf8(bytes).is_err() {
                        return Err(self.err(
                            part.line,
                            part.column,
                            "string literal is not valid UTF-8; use a `bytes` destination for raw data",
                        ));
                    }
                }
                ExprKind::Ident(operand) => {
                    let slot = self
                        .slots
                        .get(operand)
                        .map(|slot| (slot.ty, slot.is_ref, slot.base))
                        .or_else(|| {
                            self.state_vars
                                .get(operand)
                                .map(|var| (var.ty, false, var.off))
                        });
                    match slot {
                        Some((part_ty, is_ref, _)) if part_ty.is_buffer() => {
                            if part_ty.kind != ty.kind {
                                return Err(self.err(
                                    part.line,
                                    part.column,
                                    format!(
                                        "cannot use `{}` in a `{}` construction",
                                        part_ty.display(),
                                        ty.display()
                                    ),
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
                                    format!(
                                        "capacity mismatch: `{}` cannot be assigned to `{}`",
                                        part_ty.display(),
                                        ty.display()
                                    ),
                                ));
                            }
                        }
                        Some(_) => {
                            return Err(self.err(
                                part.line,
                                part.column,
                                format!("`{operand}` is not a buffer"),
                            ))
                        }
                        None => {
                            return Err(self.err(
                                part.line,
                                part.column,
                                format!("unknown identifier `{operand}`"),
                            ))
                        }
                    }
                }
                _ => {
                    return Err(self.err(
                        part.line,
                        part.column,
                        "buffer construction accepts only literals, buffers and `+`",
                    ))
                }
            }
        }

        self.push_target_ref(target);
        self.simple(opcodes::CLEAR);
        self.pop(1)?;

        for part in &parts {
            if let ExprKind::Str(bytes) = &part.kind {
                if bytes.is_empty() {
                    continue;
                }
                let offset = self.pool.intern(bytes);
                self.push_target_ref(target);
                self.one(opcodes::REF_CONST, offset);
                self.push();
            } else if let ExprKind::Ident(operand) = &part.kind {
                self.push_target_ref(target);
                self.push_named_ref(operand, part)?;
            } else {
                return Err(self.err_at(
                    part,
                    "buffer construction accepts only literals, buffers and `+`",
                ));
            }
            self.one(opcodes::APPEND, ty.cap);
            self.pop(2)?;
        }

        Ok(())
    }

    // ---- expressions ---------------------------------------------------

    fn expr_scalar(&mut self, expr: &Expr) -> Result<Type, Diagnostic> {
        match &expr.kind {
            ExprKind::Int(value) => {
                self.const_i64(*value);
                Ok(Type::scalar(Kind::Int))
            }
            ExprKind::Float(value) => {
                self.const_i64(value.to_bits() as i64);
                Ok(Type::scalar(Kind::Float))
            }
            ExprKind::Bool(value) => {
                self.const_i64(u64::from(*value) as i64);
                Ok(Type::scalar(Kind::Bool))
            }
            ExprKind::Str(_) => Err(self.err_at(
                expr,
                "a string literal is a buffer; assign it into a buffer or pass it to a buffer parameter",
            )),
            ExprKind::Ident(name) => self.load_scalar(name, expr),
            ExprKind::Call(call) => match self.call(call)? {
                Some(ty) => Ok(ty),
                None => Err(self.err_at(
                    expr,
                    format!("procedure `{}` does not return a value", call.target),
                )),
            },
            ExprKind::Cast(name, inner) => self.cast(name, inner, expr),
            ExprKind::Unary(op, inner) => self.unary(*op, inner, expr),
            ExprKind::Binary(op, left, right) => self.binary(*op, left, right, expr),
        }
    }

    fn load_scalar(&mut self, name: &str, expr: &Expr) -> Result<Type, Diagnostic> {
        if let Some(value) = self.consts.get(name) {
            let value = *value;
            self.const_i64(value);
            return Ok(Type::scalar(Kind::Int));
        }
        if let Some(slot) = self.slots.get(name) {
            let (base, ty, is_ref) = (slot.base, slot.ty, slot.is_ref);
            if ty.is_buffer() {
                return Err(self.err_at(
                    expr,
                    format!("buffer `{name}` cannot be used as a scalar value"),
                ));
            }
            if is_ref {
                self.one(opcodes::LOAD_LOCAL, base);
                self.simple(opcodes::LOAD_REF);
            } else {
                self.one(opcodes::LOAD_LOCAL, base);
            }
            self.push();
            return Ok(ty);
        }
        if let Some(var) = self.state_vars.get(name) {
            if var.ty.is_buffer() {
                return Err(self.err_at(
                    expr,
                    format!("buffer `{name}` cannot be used as a scalar value"),
                ));
            }
            self.one(opcodes::LOAD_STATE, var.off);
            self.push();
            return Ok(var.ty);
        }
        let suggestion = if self.sig_index.contains_key(name) {
            format!("`{name}` is a procedure; call it with `()`")
        } else {
            format!("unknown identifier `{name}`")
        };
        Err(self.err_at(expr, suggestion))
    }

    fn cast(&mut self, name: &str, inner: &Expr, expr: &Expr) -> Result<Type, Diagnostic> {
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
                "buffer conversions are not casts; they are `stdlib` functions",
            ));
        }
        if source.kind == Kind::Bool {
            return Err(self.err_at(
                expr,
                format!("cannot convert `bool` to `{}`", target.name()),
            ));
        }
        if source.kind == target {
            return Ok(Type::scalar(target));
        }
        let from = source
            .value_kind()
            .ok_or_else(|| self.err_at(expr, "unsupported conversion"))?;
        let to = Type::scalar(target)
            .value_kind()
            .ok_or_else(|| self.err_at(expr, "unsupported conversion"))?;
        self.word(opcodes::CONVERT, from, to, 0);
        Ok(Type::scalar(target))
    }

    fn unary(&mut self, op: UnOp, inner: &Expr, expr: &Expr) -> Result<Type, Diagnostic> {
        match op {
            UnOp::Neg => {
                let ty = self.expr_scalar(inner)?;
                match ty.kind {
                    Kind::Int => {
                        self.simple(opcodes::NEG);
                        Ok(ty)
                    }
                    Kind::Float => {
                        self.simple(opcodes::FNEG);
                        Ok(ty)
                    }
                    Kind::Uint => Err(self.err_at(
                        expr,
                        "unary `-` is not defined for `uint`; convert with `int(x)` first",
                    )),
                    other => Err(self.err_at(
                        expr,
                        format!("unary `-` is not defined for `{}`", other.name()),
                    )),
                }
            }
            UnOp::Not => {
                let ty = self.expr_scalar(inner)?;
                if ty.kind != Kind::Bool {
                    return Err(self.err_at(
                        expr,
                        format!("`not` needs a `bool`, found `{}`", ty.display()),
                    ));
                }
                self.simple(opcodes::NOT);
                Ok(ty)
            }
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        expr: &Expr,
    ) -> Result<Type, Diagnostic> {
        // `and`/`or` short-circuit, so they are lowered to branches before the
        // right operand is evaluated.
        if matches!(op, BinOp::And | BinOp::Or) {
            let left_ty = self.expr_scalar(left)?;
            if left_ty.kind != Kind::Bool {
                return Err(self.err_at(
                    left,
                    format!(
                        "`{}` needs `bool` operands, found `{}`",
                        if op == BinOp::And { "and" } else { "or" },
                        left_ty.display()
                    ),
                ));
            }
            let end = self.label();
            let short = self.label();
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
                    format!(
                        "`{}` needs `bool` operands, found `{}`",
                        if op == BinOp::And { "and" } else { "or" },
                        right_ty.display()
                    ),
                ));
            }
            self.jump(opcodes::JMP, end, false)?;
            self.bind(short);
            self.depth = short_depth;
            self.const_i64(u64::from(op == BinOp::Or) as i64);
            self.bind(end);
            return Ok(Type::scalar(Kind::Bool));
        }

        let left_ty = self.expr_scalar(left)?;
        let right_ty = self.expr_scalar(right)?;
        if left_ty != right_ty {
            return Err(self.err_at(
                expr,
                format!(
                    "type mismatch: `{}` and `{}`; there are no implicit conversions",
                    left_ty.display(),
                    right_ty.display()
                ),
            ));
        }
        if left_ty.is_buffer() {
            return Err(self.err_at(expr, "buffer comparison is not implemented in this slice"));
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
                    return Err(self.err_at(expr, "`/` is float division; use `div` for integers"))
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
                    return Err(self.err_at(expr, "`/` is float division; use `div` for integers"))
                }
                BinOp::And | BinOp::Or => unreachable!(),
            },
            Kind::Byte => {
                if arithmetic {
                    return Err(self.err_at(
                        expr,
                        "arithmetic on `byte` is not defined; convert with `int(x)` first",
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
                _ => return Err(self.err_at(expr, "`bool` supports only `=` and `<>`")),
            },
            Kind::Float => match op {
                BinOp::Add => opcodes::FADD,
                BinOp::Sub => opcodes::FSUB,
                BinOp::Mul => opcodes::FMUL,
                BinOp::Slash => opcodes::FDIV,
                BinOp::Div | BinOp::Mod => {
                    return Err(self.err_at(
                        expr,
                        "use `/` for `f64`; `div` and `mod` are integer operations",
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

        self.simple(encoded);
        self.pop(1)?;
        Ok(if comparison {
            Type::scalar(Kind::Bool)
        } else {
            left_ty
        })
    }

    // ---- calls ---------------------------------------------------------

    fn call(&mut self, call: &Call) -> Result<Option<Type>, Diagnostic> {
        match call.target.as_str() {
            "core.write" => {
                if call.args.len() != 1 {
                    return Err(self.err(
                        call.line,
                        call.column,
                        format!("`core.write` expects 1 argument, got {}", call.args.len()),
                    ));
                }
                self.emit_ref(&call.args[0], None)?;
                self.one(opcodes::BUILTIN, builtin::WRITE);
                self.pop(1)?;
                Ok(None)
            }
            "core.clock" => {
                if !call.args.is_empty() {
                    return Err(self.err(
                        call.line,
                        call.column,
                        format!("`core.clock` expects 0 arguments, got {}", call.args.len()),
                    ));
                }
                self.one(opcodes::BUILTIN, builtin::CLOCK);
                self.push();
                Ok(Some(Type::scalar(Kind::Uint)))
            }
            target => {
                if target.contains('.') {
                    return Err(self.err(
                        call.line,
                        call.column,
                        format!("undefined procedure `{target}`"),
                    ));
                }
                let index = *self.sig_index.get(target).ok_or_else(|| {
                    self.err(
                        call.line,
                        call.column,
                        format!("undefined procedure `{target}`"),
                    )
                })?;
                let params = self.sigs[index].params.clone();
                if call.args.len() != params.len() {
                    return Err(self.err(
                        call.line,
                        call.column,
                        format!(
                            "procedure `{target}` expects {} argument(s), got {}",
                            params.len(),
                            call.args.len()
                        ),
                    ));
                }
                for (position, (arg, param)) in call.args.iter().zip(params.iter()).enumerate() {
                    self.argument(arg, param, position + 1)?;
                }
                let call_pos = self.word(opcodes::CALL, 0, 0, 1);
                let _ = call_pos;
                self.imm32(index as u32);
                self.pop(params.len() as i32)?;
                Ok(None)
            }
        }
    }

    fn argument(&mut self, arg: &Expr, param: &Param, position: usize) -> Result<(), Diagnostic> {
        if param.ty.is_buffer() {
            let found = if param.mode == ParamMode::Var {
                self.emit_var_buffer_ref(arg)?
            } else {
                self.emit_ref(arg, Some(param.ty.kind))?
            };
            if found.kind != param.ty.kind {
                return Err(self.err_at(
                    arg,
                    format!(
                        "argument {position} expects `{}`, found `{}`",
                        param.ty.display(),
                        found.display()
                    ),
                ));
            }
            if found.cap < param.ty.cap {
                return Err(self.err_at(
                    arg,
                    format!(
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
                            format!(
                                "argument {position} expects `{}`, found `{}`",
                                param.ty.display(),
                                found.display()
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
                            format!(
                                "argument {position} expects `{}`, found `{}`",
                                param.ty.display(),
                                found.display()
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
    fn emit_ref(&mut self, expr: &Expr, want: Option<Kind>) -> Result<Type, Diagnostic> {
        match &expr.kind {
            ExprKind::Str(bytes) => {
                if let Some(kind) = want {
                    if kind == Kind::String && std::str::from_utf8(bytes).is_err() {
                        return Err(self.err_at(
                            expr,
                            "string literal is not valid UTF-8; use a `bytes` destination for raw data",
                        ));
                    }
                    if !kind.is_buffer() {
                        return Err(self.err_at(expr, "expected a scalar, found a string literal"));
                    }
                }
                let kind = match want {
                    Some(kind) if kind.is_buffer() => kind,
                    _ => Kind::String,
                };
                let offset = self.pool.intern(bytes);
                self.one(opcodes::REF_CONST, offset);
                self.push();
                Ok(Type::buffer(kind, bytes.len() as u32))
            }
            ExprKind::Ident(name) => {
                let ty = self.buffer_type(name, expr)?;
                if let Some(kind) = want {
                    if kind.is_buffer() && kind != ty.kind {
                        return Err(self.err_at(
                            expr,
                            format!("expected `{}`, found `{}`", kind.name(), ty.display()),
                        ));
                    }
                }
                self.push_named_ref(name, expr)?;
                Ok(ty)
            }
            _ => Err(self.err_at(expr, "expected a buffer expression")),
        }
    }

    fn emit_var_buffer_ref(&mut self, expr: &Expr) -> Result<Type, Diagnostic> {
        let name = match &expr.kind {
            ExprKind::Ident(name) => name,
            _ => {
                return Err(self.err_at(
                    expr,
                    "a `var` buffer argument must be a variable or parameter",
                ))
            }
        };
        let target = self.resolve_target(name, expr.line, expr.column)?;
        match &target {
            Target::LocalBuffer { .. } | Target::StateBuffer { .. } => {
                let ty = target.ty();
                self.push_target_ref(&target);
                Ok(ty)
            }
            _ => Err(self.err_at(expr, format!("`{name}` is not a buffer"))),
        }
    }

    fn emit_scalar_ref(&mut self, expr: &Expr) -> Result<Type, Diagnostic> {
        let name = match &expr.kind {
            ExprKind::Ident(name) => name,
            _ => return Err(self.err_at(expr, "a `var` argument must be a variable or parameter")),
        };
        if let Some(slot) = self.slots.get(name) {
            if !slot.writable {
                return Err(self.err_at(
                    expr,
                    format!("cannot pass `const` parameter `{name}` as `var`"),
                ));
            }
            if slot.ty.is_buffer() {
                return Err(self.err_at(expr, format!("`{name}` is not a scalar")));
            }
            let (base, ty, is_ref) = (slot.base, slot.ty, slot.is_ref);
            if is_ref {
                self.one(opcodes::LOAD_LOCAL, base);
            } else {
                self.one(opcodes::REF_LOCAL, base);
            }
            self.push();
            return Ok(ty);
        }
        if let Some(var) = self.state_vars.get(name) {
            if var.ty.is_buffer() {
                return Err(self.err_at(expr, format!("`{name}` is not a scalar")));
            }
            self.one(opcodes::REF_STATE, var.off);
            self.push();
            return Ok(var.ty);
        }
        if self.consts.contains_key(name) {
            return Err(self.err_at(expr, format!("cannot pass constant `{name}` as `var`")));
        }
        Err(self.err_at(expr, format!("unknown identifier `{name}`")))
    }

    fn buffer_type(&self, name: &str, expr: &Expr) -> Result<Type, Diagnostic> {
        if let Some(slot) = self.slots.get(name) {
            if slot.ty.is_buffer() {
                return Ok(slot.ty);
            }
            return Err(self.err_at(expr, format!("`{name}` is not a buffer")));
        }
        if let Some(var) = self.state_vars.get(name) {
            if var.ty.is_buffer() {
                return Ok(var.ty);
            }
            return Err(self.err_at(expr, format!("`{name}` is not a buffer")));
        }
        Err(self.err_at(expr, format!("unknown identifier `{name}`")))
    }

    fn push_named_ref(&mut self, name: &str, expr: &Expr) -> Result<(), Diagnostic> {
        if let Some(slot) = self.slots.get(name) {
            if !slot.ty.is_buffer() {
                return Err(self.err_at(expr, format!("`{name}` is not a buffer")));
            }
            if slot.is_ref {
                self.one(opcodes::LOAD_LOCAL, slot.base);
            } else {
                self.one(opcodes::REF_LOCAL, slot.base + 8);
            }
            self.push();
            return Ok(());
        }
        if let Some(var) = self.state_vars.get(name) {
            if !var.ty.is_buffer() {
                return Err(self.err_at(expr, format!("`{name}` is not a buffer")));
            }
            self.one(opcodes::REF_STATE, var.off + 8);
            self.push();
            return Ok(());
        }
        Err(self.err_at(expr, format!("unknown identifier `{name}`")))
    }
}

fn collect_concat<'e>(expr: &'e Expr, parts: &mut Vec<&'e Expr>) {
    if let ExprKind::Binary(BinOp::Add, left, right) = &expr.kind {
        collect_concat(left, parts);
        collect_concat(right, parts);
    } else {
        parts.push(expr);
    }
}

//! Compiler for the igis-v2 application language.
//!
//! The pipeline is `lex` → `parse` → `codegen` → `package`, and it is
//! allocation-free: every token, AST node, name table and instruction is stored
//! in the caller's scratch arena, and the package is written into the caller's
//! code and manifest buffers (spec 0008, B11). `abi` is the only dependency, so
//! the same code builds for the kernel and for the host.
//!
//! Compilation is deterministic (spec 0008, B9): names are emitted in
//! declaration order, constants are interned in first-use order, and nothing
//! reads an address, a timestamp or a random seed. Compiling the same source
//! twice therefore yields identical bytes.

#![no_std]
#![deny(unsafe_code)]
// `Diagnostic` is a fixed 448-byte record on purpose: spec 0003 models it as
// `unit_name: string[32]` + `message: string[96]` plus a path, and the kernel
// cannot allocate a box. Reporting it by value is the point, not a smell.
#![allow(clippy::result_large_err)]

mod arena;
mod ast;
mod codegen;
mod lexer;
mod package;
mod parser;
mod types;

use core::fmt::{self, Write};

use abi::Status;

pub use arena::FixedBuf;

use arena::Bump;
use ast::Unit;

/// Maximum accepted source size for one unit (the application or one `/lib`
/// module). Spec 0008/0018 caps a unit at 128 KiB; the kernel's source buffer
/// holds one unit, so this is the effective per-unit cap.
///
/// M2.4.2 raised this from 64 KiB: the M2.4.2 controls pushed the embedded
/// `stdlib` past the old cap (it is 72 KiB of source). The combined application
/// cap is separate (see [`MAX_APPLICATION_SOURCE_BYTES`]).
pub const MAX_SOURCE_BYTES: usize = 128 * 1024;

/// Maximum combined source size for an application: the application, the
/// embedded `stdlib` (when imported) and every `/lib` module (spec 0018, M11).
/// M3.2 raised it from 256 KiB to 384 KiB.
pub const MAX_APPLICATION_SOURCE_BYTES: usize = 384 * 1024;

/// Source of the embedded standard library unit (ADR 0019: source only, never a
/// precompiled artifact). It is compiled together with an application that has
/// `uses stdlib;`.
pub const STDLIB_SOURCE: &str = include_str!("../../stdlib/stdlib.ig");

/// Path used in diagnostics for the embedded standard library.
pub const STDLIB_PATH: &str = "stdlib/stdlib.ig";

/// Scratch arena the compiler needs for one compilation: the application, the
/// embedded `stdlib` (when imported) and every `/lib` module. Tokens, AST
/// nodes, name tables, the instruction and line-table images and the constant
/// pool all live here; a caller with a larger buffer still only gets this much,
/// so the bound is real.
///
/// M2.4.0 raised this from 1 MiB, M2.4.2 from 2 MiB and M2.4.5 from 4 MiB: the
/// UI layout and controls layers pushed the embedded `stdlib` unit past the old
/// caps (M2.4.6 needs 6,546,928 B for the `widgets` pair). M3.2 raises it from
/// 8 MiB to 16 MiB so `/lib` modules have room (spec 0018, M11).
pub const MAX_ARENA_BYTES: usize = 16 * 1024 * 1024;

/// Code-blob buffer a caller must provide (provisional tuning value, spec 0008:
/// 256 KiB per application).
pub const MAX_CODE_BYTES: usize = 256 * 1024;

/// Manifest buffer a caller must provide. The manifest is a 32-byte header plus
/// the unit name and endpoints; a few KiB is generous.
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024;

/// Maximum nesting depth accepted by the recursive parser and by the recursive
/// codegen walks (spec 0002, Q42).
///
/// The compiler runs in the kernel on a 256 KiB guarded stack. The measured
/// worst case is about 5.6 KiB per level (a nested call expression, release
/// build), and the existing `shell` already reaches 15 levels and ~70 KiB, so
/// 32 is the largest round value that leaves roughly 60 KiB of headroom for
/// deepest-case source. Exceeding it is a diagnostic, never a stack fault.
pub const MAX_NESTING: u32 = 32;

/// Capacity of [`Diagnostic::message`], matching `message: string[96]` in spec
/// 0003.
pub const DIAGNOSTIC_MESSAGE_BYTES: usize = 96;

/// Capacity of [`Diagnostic::unit`], matching `unit_name: string[32]` in spec
/// 0003.
pub const DIAGNOSTIC_UNIT_BYTES: usize = 32;

/// Capacity of [`Diagnostic::path`]. Spec 0003 limits `run`/`spawn` paths to 256
/// bytes, so this holds any path the REPL can pass.
pub const DIAGNOSTIC_PATH_BYTES: usize = 256;

/// The text of a compile diagnostic.
pub type Message = FixedBuf<DIAGNOSTIC_MESSAGE_BYTES>;

/// A compile diagnostic.
///
/// This is the compile-time subset of the `Diagnostic` record in spec 0003: the
/// unit and file that failed, a 1-based line and byte column, a status and a
/// short message. Runtime-only fields (`handle`, `offset`, `length`, `task_id`)
/// are not produced by the compiler.
///
/// The text fields are fixed-capacity copies, like the language's
/// `Diagnostic` record (`unit_name: string[32]`, `message: string[96]`): a
/// diagnostic can outlive the arena and the source, which is what lets a host
/// wrapper allocate scratch, compile, and still return the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Source path as given to [`compile_into`].
    pub path: FixedBuf<DIAGNOSTIC_PATH_BYTES>,
    /// Unit name, empty if the error precedes the `unit` declaration.
    pub unit: FixedBuf<DIAGNOSTIC_UNIT_BYTES>,
    /// Why compilation stopped.
    pub status: Status,
    /// 1-based line.
    pub line: u32,
    /// 1-based byte column.
    pub column: u32,
    /// Short human-readable message, truncated at
    /// [`DIAGNOSTIC_MESSAGE_BYTES`].
    pub message: Message,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.path, self.line, self.column, self.message
        )
    }
}

/// A compiled application: one manifest and one code blob, borrowed from the
/// caller's output buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package<'a> {
    /// Encoded manifest (ABI version, sizes, entry, unit name).
    pub manifest: &'a [u8],
    /// Encoded code blob (header, functions, instructions, constants, lines).
    pub code: &'a [u8],
}

/// Builds a diagnostic with [`Status::Invalid`].
pub(crate) fn diag(
    path: &str,
    unit: &str,
    line: u32,
    column: u32,
    message: fmt::Arguments<'_>,
) -> Diagnostic {
    diag_status(Status::Invalid, path, unit, line, column, message)
}

/// Builds a diagnostic with an explicit status.
pub(crate) fn diag_status(
    status: Status,
    path: &str,
    unit: &str,
    line: u32,
    column: u32,
    message: fmt::Arguments<'_>,
) -> Diagnostic {
    let mut text = Message::new();
    // `FixedBuf` never fails; it truncates and records the flag.
    let _ = text.write_fmt(message);
    finish_diag(status, path, unit, line, column, text)
}

/// Builds a diagnostic from already-formatted text (used where the message is
/// assembled piecewise, such as an import chain).
fn diag_text(
    status: Status,
    path: &str,
    unit: &str,
    line: u32,
    column: u32,
    message: &str,
) -> Diagnostic {
    let mut text = Message::new();
    text.push_str(message);
    finish_diag(status, path, unit, line, column, text)
}

fn finish_diag(
    status: Status,
    path: &str,
    unit: &str,
    line: u32,
    column: u32,
    text: Message,
) -> Diagnostic {
    let mut path_text = FixedBuf::new();
    path_text.push_str(path);
    let mut unit_text = FixedBuf::new();
    unit_text.push_str(unit);
    Diagnostic {
        status,
        path: path_text,
        unit: unit_text,
        line,
        column,
        message: text,
    }
}

/// Describes which bounded resource ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resource {
    /// The scratch arena (tokens, AST, tables, instructions).
    Arena,
    /// The caller's code-blob buffer.
    Code,
    /// The caller's manifest buffer.
    Manifest,
}

impl Resource {
    const fn text(self) -> &'static str {
        match self {
            Resource::Arena => "scratch arena",
            Resource::Code => "code buffer",
            Resource::Manifest => "manifest buffer",
        }
    }

    const fn cap(self) -> usize {
        match self {
            Resource::Arena => MAX_ARENA_BYTES,
            Resource::Code => MAX_CODE_BYTES,
            Resource::Manifest => MAX_MANIFEST_BYTES,
        }
    }
}

/// Builds the diagnostic for an exhausted bounded resource.
pub(crate) fn out_of_space(
    path: &str,
    unit: &str,
    line: u32,
    column: u32,
    resource: Resource,
) -> Diagnostic {
    diag_status(
        Status::CapacityExceeded,
        path,
        unit,
        line,
        column,
        format_args!(
            "compile {} is exhausted; the unit needs more than its {}-byte cap",
            resource.text(),
            resource.cap()
        ),
    )
}

/// Supplies `/lib` module sources to [`compile_with_imports`] (spec 0018,
/// "Compiler interface").
///
/// The compiler performs no I/O: the kernel implements this over the mounted
/// ext2 image, `langhost` over its source directory, and tests over an
/// in-memory table. [`size`](SourceReader::size) is called before any arena
/// space is reserved, so an oversized module is refused without reading it.
pub trait SourceReader {
    /// Size of `path` in bytes, before the compiler reserves arena space.
    fn size(&self, path: &str) -> Result<usize, Status>;
    /// Reads all of `path` into `out`; the returned length must equal
    /// [`size`](SourceReader::size).
    fn read(&self, path: &str, out: &mut [u8]) -> Result<usize, Status>;
}

/// What a unit is, which decides its `uses` rule and its allowed declarations
/// (spec 0018, M5/M6).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitRole {
    /// The application (unit id 0).
    Application,
    /// The embedded `stdlib` (unit id 1): no `lib.*` imports and no state.
    Library,
    /// An imported `/lib` module (unit ids 2..).
    Module,
}

/// One parsed `/lib` module and where it came from.
#[derive(Clone, Copy)]
pub(crate) struct ModuleUnit<'a> {
    /// The `uses` name as written (`lib.ui.menu`).
    pub(crate) use_name: &'a str,
    /// The resolved image path (`lib/ui/menu.ig`).
    pub(crate) path: &'a str,
    /// The parsed unit.
    pub(crate) unit: &'a Unit<'a>,
}

/// Most `/lib` modules one package may import. Unit ids `0..=16` are reserved
/// for the application (0), `stdlib` (1) and modules (2..), so the last id
/// leaves room for [`abi::MAX_UNIT_ID`] minus the two fixed units (spec 0018,
/// M8).
pub const MAX_MODULE_UNITS: usize = (abi::MAX_UNIT_ID as usize) - 1;

/// Compiles one unit of source text into a package.
///
/// `path` is only used in diagnostics. `source` must be UTF-8 and no larger than
/// [`MAX_SOURCE_BYTES`]. When the unit declares `uses stdlib;`, the embedded
/// [`STDLIB_SOURCE`] unit is compiled with it into the same package
/// (`stdlib.<name>` is then callable). `arena` is the scratch memory for all
/// units ([`MAX_ARENA_BYTES`]); `code` and `manifest` receive the artifact
/// ([`MAX_CODE_BYTES`], [`MAX_MANIFEST_BYTES`]). The returned package borrows
/// `code` and `manifest`.
///
/// This entry point does no I/O, so it rejects a `lib.*` import with a
/// diagnostic; [`compile_with_imports`] handles those through a
/// [`SourceReader`] (spec 0018, "Compiler interface").
///
/// Running out of scratch, code or manifest space is a diagnostic with
/// [`Status::CapacityExceeded`], never an allocation or a partial package.
pub fn compile_into<'a>(
    source: &'a [u8],
    path: &'a str,
    arena: &'a mut [u8],
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!(
                "source is {} bytes, over the {}-byte cap",
                source.len(),
                MAX_SOURCE_BYTES
            ),
        ));
    }
    if STDLIB_SOURCE.len() > MAX_SOURCE_BYTES {
        return Err(diag(
            STDLIB_PATH,
            "stdlib",
            1,
            1,
            format_args!(
                "embedded stdlib source is {} bytes, over the {}-byte cap",
                STDLIB_SOURCE.len(),
                MAX_SOURCE_BYTES
            ),
        ));
    }
    if core::str::from_utf8(source).is_err() {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!("source is not valid UTF-8"),
        ));
    }

    let arena_len = arena.len().min(MAX_ARENA_BYTES);
    let code_len = code.len().min(MAX_CODE_BYTES);
    let manifest_len = manifest.len().min(MAX_MANIFEST_BYTES);
    let arena = &mut arena[..arena_len];
    let code = &mut code[..code_len];
    let manifest = &mut manifest[..manifest_len];

    let bump = arena::Bump::new(arena);
    let tokens = lexer::lex(source, path, &bump)?;
    let unit = parser::parse(tokens, path, &bump)?;
    if let Some(used) = unit.uses.iter().find(|name| name.starts_with("lib.")) {
        return Err(diag(
            path,
            unit.name,
            unit.uses_line,
            unit.uses_column,
            format_args!("`{used}` needs a module reader; use `compile_with_imports`"),
        ));
    }
    let uses_stdlib = unit.uses.contains(&"stdlib");
    let combined = source.len() + if uses_stdlib { STDLIB_SOURCE.len() } else { 0 };
    if combined > MAX_APPLICATION_SOURCE_BYTES {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!(
                "application sources total {combined} bytes, over the {}-byte cap",
                MAX_APPLICATION_SOURCE_BYTES
            ),
        ));
    }
    let library = if uses_stdlib {
        let stdlib = STDLIB_SOURCE.as_bytes();
        let tokens = lexer::lex(stdlib, STDLIB_PATH, &bump)?;
        let unit = bump
            .alloc(parser::parse(tokens, STDLIB_PATH, &bump)?)
            .map_err(|_| out_of_space(STDLIB_PATH, "stdlib", 1, 1, Resource::Arena))?;
        Some((STDLIB_PATH, &*unit))
    } else {
        None
    };
    codegen::compile_units(&unit, path, library, &[], &bump, code, manifest)
}

/// Compiles an application together with the `/lib` modules it imports (spec
/// 0018, M1–M13).
///
/// The application source and `path` are as for [`compile_into`]. `reader`
/// supplies each module's source; every unit's bytes are copied into `arena`
/// once, so the ASTs' source borrows stay valid for the whole compilation.
/// Unit ids are fixed by discovery order: application 0, `stdlib` 1 (when the
/// application or any module imports it) and modules 2.. depth-first over the
/// `uses` clauses.
///
/// The same caps as [`compile_into`] apply, plus [`MAX_MODULE_UNITS`] modules
/// and the 384 KiB combined application cap ([`MAX_APPLICATION_SOURCE_BYTES`]).
pub fn compile_with_imports<'a>(
    source: &'a [u8],
    path: &'a str,
    reader: &mut dyn SourceReader,
    arena: &'a mut [u8],
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!(
                "source is {} bytes, over the {}-byte cap",
                source.len(),
                MAX_SOURCE_BYTES
            ),
        ));
    }
    if core::str::from_utf8(source).is_err() {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!("source is not valid UTF-8"),
        ));
    }
    if STDLIB_SOURCE.len() > MAX_SOURCE_BYTES {
        return Err(diag_status(
            Status::CapacityExceeded,
            STDLIB_PATH,
            "stdlib",
            1,
            1,
            format_args!(
                "embedded stdlib source is {} bytes, over the {}-byte cap",
                STDLIB_SOURCE.len(),
                MAX_SOURCE_BYTES
            ),
        ));
    }

    let arena_len = arena.len().min(MAX_ARENA_BYTES);
    let code_len = code.len().min(MAX_CODE_BYTES);
    let manifest_len = manifest.len().min(MAX_MANIFEST_BYTES);
    let arena = &mut arena[..arena_len];
    let code = &mut code[..code_len];
    let manifest = &mut manifest[..manifest_len];

    let bump = arena::Bump::new(arena);
    let tokens = lexer::lex(source, path, &bump)?;
    let application = parser::parse(tokens, path, &bump)?;
    codegen::check_uses(&application, path, UnitRole::Application)?;

    // Depth-first discovery: the chain holds the paths of the units currently
    // being expanded, which is what makes a cycle (including a self-import)
    // detectable and nameable.
    let mut chain = arena::SliceVec::<&'a str>::new(&bump)
        .map_err(|_| out_of_space(path, application.name, 1, 1, Resource::Arena))?;
    chain
        .push(path)
        .map_err(|_| out_of_space(path, application.name, 1, 1, Resource::Arena))?;
    let mut modules = arena::SliceVec::<ModuleUnit<'a>>::new(&bump)
        .map_err(|_| out_of_space(path, application.name, 1, 1, Resource::Arena))?;
    let mut total = source.len();
    let uses_stdlib = discover(
        &application,
        path,
        application.name,
        &mut chain,
        &mut modules,
        reader,
        &bump,
        &mut total,
    )?;
    total += if uses_stdlib { STDLIB_SOURCE.len() } else { 0 };
    if total > MAX_APPLICATION_SOURCE_BYTES {
        return Err(diag_status(
            Status::CapacityExceeded,
            path,
            application.name,
            1,
            1,
            format_args!(
                "package sources total {total} bytes, over the {}-byte application cap",
                MAX_APPLICATION_SOURCE_BYTES
            ),
        ));
    }

    let modules = modules.into_slice();
    let library = if uses_stdlib {
        let stdlib = STDLIB_SOURCE.as_bytes();
        let tokens = lexer::lex(stdlib, STDLIB_PATH, &bump)?;
        let unit = bump
            .alloc(parser::parse(tokens, STDLIB_PATH, &bump)?)
            .map_err(|_| out_of_space(STDLIB_PATH, "stdlib", 1, 1, Resource::Arena))?;
        Some((STDLIB_PATH, &*unit))
    } else {
        None
    };
    codegen::compile_units(&application, path, library, modules, &bump, code, manifest)
}

/// Resolves `lib.a.b` to the image path `lib/a/b.ig` and validates it (spec
/// 0018, M2).
fn resolve_module_path<'a>(
    use_name: &str,
    owner_path: &'a str,
    owner_unit: &'a str,
    line: u32,
    column: u32,
    bump: &Bump<'a>,
) -> Result<&'a str, Diagnostic> {
    for segment in use_name.split('.') {
        if segment.is_empty() || segment.len() > abi::fs::NAME_CAP || segment.contains('/') {
            return Err(diag(
                owner_path,
                owner_unit,
                line,
                column,
                format_args!("`{use_name}` is not a valid module name"),
            ));
        }
    }
    let len = use_name.len() + 3;
    let bytes = bump
        .alloc_slice::<u8>(len, 0)
        .map_err(|_| out_of_space(owner_path, owner_unit, line, column, Resource::Arena))?;
    for (index, byte) in use_name.bytes().enumerate() {
        bytes[index] = if byte == b'.' { b'/' } else { byte };
    }
    bytes[use_name.len()..].copy_from_slice(b".ig");
    // The buffer was filled with ASCII above, so this cannot fail.
    let path = core::str::from_utf8(&bytes[..]).map_err(|_| {
        diag(
            owner_path,
            owner_unit,
            line,
            column,
            format_args!("module path is not UTF-8"),
        )
    })?;
    match abi::fs::validate_path(path) {
        Status::Ok => Ok(path),
        status => Err(diag_status(
            status,
            path,
            owner_unit,
            line,
            column,
            format_args!("`{use_name}` does not resolve to a valid image path"),
        )),
    }
}

/// Reads and parses one module, then applies the module rules (spec 0018,
/// M3/M6).
fn load_module<'a>(
    use_name: &'a str,
    module_path: &'a str,
    owner_unit: &'a str,
    line: u32,
    column: u32,
    reader: &mut dyn SourceReader,
    bump: &Bump<'a>,
) -> Result<(&'a Unit<'a>, usize), Diagnostic> {
    let expected = use_name.rsplit('.').next().unwrap_or(use_name);
    let read_error = |status: Status| {
        let reason = match status {
            Status::NotFound => "no such module",
            Status::Invalid => "module is not a regular file",
            Status::CapacityExceeded => "module is larger than the per-unit cap",
            _ => "module cannot be read",
        };
        diag_status(
            status,
            module_path,
            owner_unit,
            line,
            column,
            format_args!("cannot read `{use_name}`: {reason}"),
        )
    };
    let size = reader.size(module_path).map_err(read_error)?;
    if size > MAX_SOURCE_BYTES {
        return Err(diag_status(
            Status::CapacityExceeded,
            module_path,
            expected,
            line,
            column,
            format_args!(
                "module source is {size} bytes, over the {MAX_SOURCE_BYTES}-byte per-unit cap"
            ),
        ));
    }
    let bytes = bump
        .alloc_slice::<u8>(size, 0)
        .map_err(|_| out_of_space(module_path, expected, line, column, Resource::Arena))?;
    let read = reader.read(module_path, bytes).map_err(read_error)?;
    if read != size {
        return Err(diag(
            module_path,
            expected,
            line,
            column,
            format_args!("module size changed while reading"),
        ));
    }
    let text = core::str::from_utf8(&bytes[..]).map_err(|_| {
        diag(
            module_path,
            expected,
            line,
            column,
            format_args!("module source is not valid UTF-8"),
        )
    })?;
    let tokens = lexer::lex(text.as_bytes(), module_path, bump)?;
    let unit = bump
        .alloc(parser::parse(tokens, module_path, bump)?)
        .map_err(|_| out_of_space(module_path, expected, line, column, Resource::Arena))?;

    if unit.name != expected {
        return Err(diag(
            module_path,
            unit.name,
            1,
            1,
            format_args!(
                "module declares `unit {}`; `{use_name}` requires `unit {expected}`",
                unit.name
            ),
        ));
    }
    if matches!(unit.name, "core" | "syslib" | "stdlib") {
        return Err(diag(
            module_path,
            unit.name,
            1,
            1,
            format_args!("`{}` is a reserved unit name", unit.name),
        ));
    }
    if let Some(proc) = unit.procs.iter().find(|proc| proc.name == "main") {
        return Err(diag(
            module_path,
            unit.name,
            proc.line,
            proc.column,
            format_args!("module `{}` may not declare `main`", unit.name),
        ));
    }
    if let Some(var) = unit
        .vars
        .iter()
        .find(|var| matches!(var.ty.name, "in_stream" | "out_stream"))
    {
        return Err(diag(
            module_path,
            unit.name,
            var.line,
            var.column,
            format_args!("module `{}` may not declare a stream endpoint", unit.name),
        ));
    }
    if let Some(var) = unit.vars.first() {
        return Err(diag(
            module_path,
            unit.name,
            var.line,
            var.column,
            format_args!(
                "module `{}` may not declare top-level `var` state",
                unit.name
            ),
        ));
    }
    codegen::check_uses(unit, module_path, UnitRole::Module)?;
    Ok((&*unit, size))
}

/// Expands one unit's `uses` clause depth-first, appending newly discovered
/// modules in order. Returns whether `stdlib` is part of the package.
#[allow(clippy::too_many_arguments)]
fn discover<'a>(
    owner: &Unit<'a>,
    owner_path: &'a str,
    application_name: &'a str,
    chain: &mut arena::SliceVec<'a, '_, &'a str>,
    modules: &mut arena::SliceVec<'a, '_, ModuleUnit<'a>>,
    reader: &mut dyn SourceReader,
    bump: &Bump<'a>,
    total: &mut usize,
) -> Result<bool, Diagnostic> {
    let mut uses_stdlib = false;
    for used in owner.uses {
        if *used == "stdlib" {
            uses_stdlib = true;
            continue;
        }
        let Some(_) = used.strip_prefix("lib.") else {
            // `core`/`syslib` add nothing here; anything else has already been
            // rejected by `check_uses`.
            continue;
        };
        let module_path = resolve_module_path(
            used,
            owner_path,
            owner.name,
            owner.uses_line,
            owner.uses_column,
            bump,
        )?;
        if chain.as_slice().contains(&module_path) {
            let mut message = Message::new();
            let _ = write!(&mut message, "import cycle:");
            for entry in chain.as_slice() {
                let _ = write!(&mut message, " {entry} ->");
            }
            let _ = write!(&mut message, " {module_path}");
            return Err(diag_text(
                Status::Invalid,
                module_path,
                owner.name,
                owner.uses_line,
                owner.uses_column,
                message.as_str(),
            ));
        }
        if modules
            .as_slice()
            .iter()
            .any(|module| module.path == module_path)
        {
            continue;
        }
        if modules.len() >= MAX_MODULE_UNITS {
            return Err(diag(
                owner_path,
                owner.name,
                owner.uses_line,
                owner.uses_column,
                format_args!(
                    "too many modules; a package holds at most {MAX_MODULE_UNITS} (unit ids 2..={})",
                    abi::MAX_UNIT_ID
                ),
            ));
        }
        let (unit, size) = load_module(
            used,
            module_path,
            owner.name,
            owner.uses_line,
            owner.uses_column,
            reader,
            bump,
        )?;
        if unit.name == application_name {
            return Err(diag(
                module_path,
                unit.name,
                1,
                1,
                format_args!(
                    "unit name `{}` is already declared by the application `{owner_path}`",
                    unit.name
                ),
            ));
        }
        for other in modules.as_slice() {
            if other.unit.name == unit.name {
                return Err(diag(
                    module_path,
                    unit.name,
                    1,
                    1,
                    format_args!(
                        "unit name `{}` is already declared by `{}`",
                        unit.name, other.path
                    ),
                ));
            }
        }
        *total += size;
        modules
            .push(ModuleUnit {
                use_name: used,
                path: module_path,
                unit,
            })
            .map_err(|_| {
                out_of_space(
                    owner_path,
                    owner.name,
                    owner.uses_line,
                    owner.uses_column,
                    Resource::Arena,
                )
            })?;
        let depth = chain.len();
        chain
            .push(module_path)
            .map_err(|_| out_of_space(module_path, unit.name, 1, 1, Resource::Arena))?;
        if discover(
            unit,
            module_path,
            application_name,
            chain,
            modules,
            reader,
            bump,
            total,
        )? {
            uses_stdlib = true;
        }
        chain.truncate(depth);
    }
    Ok(uses_stdlib)
}

/// Compiles one unit by itself, without requiring `main`. This is how the
/// compiler's own test suite proves the embedded `stdlib` source is valid on
/// its own; it is not an application entry point.
pub fn compile_library_into<'a>(
    source: &'a [u8],
    path: &'a str,
    arena: &'a mut [u8],
    code: &'a mut [u8],
    manifest: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!(
                "source is {} bytes, over the {}-byte cap",
                source.len(),
                MAX_SOURCE_BYTES
            ),
        ));
    }
    if core::str::from_utf8(source).is_err() {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!("source is not valid UTF-8"),
        ));
    }
    let arena_len = arena.len().min(MAX_ARENA_BYTES);
    let code_len = code.len().min(MAX_CODE_BYTES);
    let manifest_len = manifest.len().min(MAX_MANIFEST_BYTES);
    let arena = &mut arena[..arena_len];
    let code = &mut code[..code_len];
    let manifest = &mut manifest[..manifest_len];

    let bump = arena::Bump::new(arena);
    let tokens = lexer::lex(source, path, &bump)?;
    let unit = parser::parse(tokens, path, &bump)?;
    codegen::compile_library(&unit, path, &bump, code, manifest)
}

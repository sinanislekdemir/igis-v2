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

/// Maximum accepted source size for one unit. Spec 0008 caps a unit at 64 KiB
/// and an application at 256 KiB; the kernel's source buffer holds one unit, so
/// this is the effective per-unit cap.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// Maximum combined source size for an application and the embedded `stdlib`
/// unit. It matches the 256 KiB per-application cap in spec 0008.
pub const MAX_APPLICATION_SOURCE_BYTES: usize = 256 * 1024;

/// Source of the embedded standard library unit (ADR 0019: source only, never a
/// precompiled artifact). It is compiled together with an application that has
/// `uses stdlib;`.
pub const STDLIB_SOURCE: &str = include_str!("../../stdlib/stdlib.ig");

/// Path used in diagnostics for the embedded standard library.
pub const STDLIB_PATH: &str = "stdlib/stdlib.ig";

/// Scratch arena the compiler needs for one compilation: the application and,
/// when it imports `stdlib`, the library unit (provisional tuning value, spec
/// 0008 "Compile memory"). Tokens, AST nodes, name tables, the instruction
/// and line-table images and the constant pool all live here; a caller with a
/// larger buffer still only gets this much, so the bound is real.
///
/// M2.4.0 raised this from 1 MiB: the UI layout layer pushed the embedded
/// `stdlib` unit past the old cap. 2 MiB leaves room for M2.4.1–M2.4.6.
pub const MAX_ARENA_BYTES: usize = 2 * 1024 * 1024;

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

/// Compiles one unit of source text into a package.
///
/// `path` is only used in diagnostics. `source` must be UTF-8 and no larger than
/// [`MAX_SOURCE_BYTES`]. When the unit declares `uses stdlib;`, the embedded
/// [`STDLIB_SOURCE`] unit is compiled with it into the same package
/// (`stdlib.<name>` is then callable). `arena` is the scratch memory for both
/// units ([`MAX_ARENA_BYTES`]); `code` and `manifest` receive the artifact
/// ([`MAX_CODE_BYTES`], [`MAX_MANIFEST_BYTES`]). The returned package borrows
/// `code` and `manifest`.
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
    if source.len() + STDLIB_SOURCE.len() > MAX_APPLICATION_SOURCE_BYTES {
        return Err(diag(
            path,
            "",
            1,
            1,
            format_args!(
                "application and stdlib sources total {} bytes, over the {}-byte cap",
                source.len() + STDLIB_SOURCE.len(),
                MAX_APPLICATION_SOURCE_BYTES
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
    let library = if unit.uses.contains(&"stdlib") {
        let stdlib = STDLIB_SOURCE.as_bytes();
        let tokens = lexer::lex(stdlib, STDLIB_PATH, &bump)?;
        Some(parser::parse(tokens, STDLIB_PATH, &bump)?)
    } else {
        None
    };
    codegen::compile_units(
        &unit,
        path,
        library.as_ref(),
        STDLIB_PATH,
        &bump,
        code,
        manifest,
    )
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

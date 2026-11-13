//! Compiler for the igis-v2 application language.
//!
//! The pipeline is `lex` → `parse` → `codegen` → `package`. It has no
//! dependencies beyond `abi` and the standard library, so the same code can
//! later be linked into the kernel behind a bounded-memory arena.
//!
//! Compilation is deterministic (spec 0008, B9): names are emitted in
//! declaration order, constants are interned in first-use order, and nothing
//! reads an address, a timestamp or a random seed. Compiling the same source
//! twice therefore yields identical bytes.

#![forbid(unsafe_code)]

mod ast;
mod codegen;
mod lexer;
mod package;
mod parser;
mod types;

use std::fmt;

/// Maximum accepted source size. Spec 0008 caps a unit at 64 KiB and an
/// application at 256 KiB; ABI v1 has one unit per application, so the unit cap
/// is the effective one.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// A compile diagnostic.
///
/// This is the compile-time subset of the `Diagnostic` record in spec 0003:
/// the unit and file that failed, a 1-based line and byte column, and a short
/// message. Runtime-only fields (`handle`, `offset`, `length`, `task_id`) are
/// not produced by the compiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Source path as given to [`compile`].
    pub path: String,
    /// Unit name, empty if the error precedes the `unit` declaration.
    pub unit: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based byte column.
    pub column: u32,
    /// Short human-readable message.
    pub message: String,
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

impl std::error::Error for Diagnostic {}

/// A compiled application: one manifest and one code blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Encoded manifest (ABI version, sizes, entry, unit name).
    pub manifest: Vec<u8>,
    /// Encoded code blob (header, functions, instructions, constants, lines).
    pub code: Vec<u8>,
}

pub(crate) fn diag(
    path: &str,
    unit: &str,
    line: u32,
    column: u32,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        path: path.to_string(),
        unit: unit.to_string(),
        line,
        column,
        message: message.into(),
    }
}

/// Compiles one unit of source text into a package.
///
/// `path` is only used in diagnostics. `source` must be UTF-8 and no larger
/// than [`MAX_SOURCE_BYTES`].
pub fn compile(source: &[u8], path: &str) -> Result<Package, Diagnostic> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(diag(
            path,
            "",
            1,
            1,
            format!(
                "source is {} bytes, over the {MAX_SOURCE_BYTES}-byte cap",
                source.len()
            ),
        ));
    }
    if std::str::from_utf8(source).is_err() {
        return Err(diag(path, "", 1, 1, "source is not valid UTF-8"));
    }

    let tokens = lexer::lex(source, path)?;
    let unit = parser::parse(&tokens, path)?;
    codegen::compile_unit(&unit, path)
}

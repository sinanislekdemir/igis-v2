//! Package assembly: the code blob and the manifest.
//!
//! Section offsets are derived from the header (see the `abi` crate docs), so
//! the only absolute sizes here are the section lengths. Every section starts
//! on an 8-byte boundary and the padding is zero, which keeps the artifact
//! byte-identical between builds (spec 0008, B9).

use abi::{
    CodeHeader, Function, ManifestHeader, ABI_VERSION, CODE_MAGIC, LINE_ENTRY_SIZE, MANIFEST_MAGIC,
    NO_DIAGNOSTIC, PROFILE_X86_64_LE,
};

use crate::{diag, Diagnostic, Package};

/// Sections must stay under the verifier's per-section cap, so the compiler
/// never produces a package the interpreter would refuse.
const MAX_SECTION: usize = 1 << 20;

/// One line-table entry.
pub(crate) struct LineEntry {
    pub off: u32,
    pub line: u32,
    pub column: u32,
}

fn pad_to_8(buffer: &mut Vec<u8>) {
    while !buffer.len().is_multiple_of(8) {
        buffer.push(0);
    }
}

fn to_u32(value: usize, path: &str, unit: &str, what: &str) -> Result<u32, Diagnostic> {
    u32::try_from(value).map_err(|_| {
        diag(
            path,
            unit,
            1,
            1,
            format!("{what} is larger than the ABI's 4 GiB section limit"),
        )
    })
}

/// Assembles the code blob and manifest.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble(
    path: &str,
    unit_name: &str,
    functions: Vec<Function>,
    instructions: Vec<u8>,
    constants: Vec<u8>,
    lines: Vec<LineEntry>,
    state_size: u32,
    stack_size: u32,
    entry: u32,
) -> Result<Package, Diagnostic> {
    if unit_name.len() > u16::MAX as usize {
        return Err(diag(path, unit_name, 1, 1, "unit name is too long"));
    }
    for (length, what) in [
        (instructions.len(), "instruction section"),
        (constants.len(), "constant section"),
        (lines.len() * LINE_ENTRY_SIZE, "line table"),
    ] {
        if length > MAX_SECTION {
            return Err(diag(
                path,
                unit_name,
                1,
                1,
                format!("{what} is {length} bytes, over the {MAX_SECTION}-byte cap"),
            ));
        }
    }

    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: to_u32(functions.len(), path, unit_name, "function table")?,
        entry,
        code_len: to_u32(instructions.len(), path, unit_name, "instruction section")?,
        const_len: to_u32(constants.len(), path, unit_name, "constant section")?,
        line_len: to_u32(lines.len() * LINE_ENTRY_SIZE, path, unit_name, "line table")?,
        state_size,
        stack_size,
        diag_offset: NO_DIAGNOSTIC,
    };

    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    for function in &functions {
        code.extend_from_slice(&function.to_bytes());
    }
    pad_to_8(&mut code);
    code.extend_from_slice(&instructions);
    pad_to_8(&mut code);
    code.extend_from_slice(&constants);
    pad_to_8(&mut code);
    for line in &lines {
        code.extend_from_slice(&line.off.to_le_bytes());
        code.extend_from_slice(&0u32.to_le_bytes()); // unit index; ABI v1 has one unit
        code.extend_from_slice(&line.line.to_le_bytes());
        code.extend_from_slice(&line.column.to_le_bytes());
    }

    let code_len = to_u32(code.len(), path, unit_name, "code blob")?;
    let manifest_header = ManifestHeader {
        magic: MANIFEST_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        unit_name_len: unit_name.len() as u16,
        endpoint_count: 0,
        code_len,
        state_size,
        stack_size,
        entry,
        capacity_profile: 0,
    };
    let mut manifest = Vec::new();
    manifest.extend_from_slice(&manifest_header.to_bytes());
    manifest.extend_from_slice(unit_name.as_bytes());

    Ok(Package { manifest, code })
}

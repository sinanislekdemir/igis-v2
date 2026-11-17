//! Package assembly: the code blob and the manifest.
//!
//! Section offsets are derived from the header (see the `abi` crate docs), so
//! the only absolute sizes here are the section lengths. Every section starts
//! on an 8-byte boundary and the padding is zero, which keeps the artifact
//! byte-identical between builds (spec 0008, B9).
//!
//! Both artifacts are written straight into the caller's output buffers; a
//! buffer that is too small is a `CapacityExceeded` diagnostic, and no partial
//! package is returned.

use abi::{
    CodeHeader, Endpoint, Function, ManifestHeader, ABI_VERSION, CODE_MAGIC, LINE_ENTRY_SIZE,
    MANIFEST_MAGIC, NO_DIAGNOSTIC, PROFILE_X86_64_LE,
};

use crate::arena::OutOfSpace;
use crate::codegen::EndpointDecl;
use crate::{diag, out_of_space, Diagnostic, Package, Resource};

/// Sections must stay under the verifier's per-section cap, so the compiler
/// never produces a package the interpreter would refuse.
const MAX_SECTION: usize = 1 << 20;

/// One line-table entry: a code offset, its unit id (0 application, 1 the
/// embedded stdlib, 2.. imported `/lib` modules) and a 1-based source
/// position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineEntry {
    pub off: u32,
    pub unit: u32,
    pub line: u32,
    pub column: u32,
}

/// A front-to-back writer over a caller-provided buffer.
struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Out { buf, len: 0 }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), OutOfSpace> {
        let end = self.len.checked_add(bytes.len()).ok_or(OutOfSpace)?;
        if end > self.buf.len() {
            return Err(OutOfSpace);
        }
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    fn pad_to_8(&mut self) -> Result<(), OutOfSpace> {
        while !self.len.is_multiple_of(8) {
            self.put(&[0])?;
        }
        Ok(())
    }

    fn into_written(self) -> &'a [u8] {
        let Out { buf, len } = self;
        let buf: &'a [u8] = buf;
        &buf[..len]
    }
}

fn to_u32<'a>(
    value: usize,
    path: &'a str,
    unit_name: &'a str,
    what: &str,
) -> Result<u32, Diagnostic> {
    u32::try_from(value).map_err(|_| {
        diag(
            path,
            unit_name,
            1,
            1,
            format_args!("{what} is larger than the ABI's 4 GiB section limit"),
        )
    })
}

/// Assembles the code blob and manifest into the caller's buffers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble<'a>(
    path: &'a str,
    unit_name: &'a str,
    functions: &[Function],
    instructions: &[u8],
    constants: &[u8],
    lines: &[LineEntry],
    endpoints: &[EndpointDecl<'_>],
    state_size: u32,
    stack_size: u32,
    entry: u32,
    code_buffer: &'a mut [u8],
    manifest_buffer: &'a mut [u8],
) -> Result<Package<'a>, Diagnostic> {
    if unit_name.len() > u16::MAX as usize {
        return Err(diag(
            path,
            unit_name,
            1,
            1,
            format_args!("unit name is too long"),
        ));
    }
    if endpoints.len() > u16::MAX as usize {
        return Err(diag(
            path,
            unit_name,
            1,
            1,
            format_args!("too many stream endpoints"),
        ));
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
                format_args!("{what} is {length} bytes, over the {MAX_SECTION}-byte cap"),
            ));
        }
    }

    let full_code = || out_of_space(path, unit_name, 1, 1, Resource::Code);
    let full_manifest = || out_of_space(path, unit_name, 1, 1, Resource::Manifest);

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

    let mut code = Out::new(code_buffer);
    code.put(&header.to_bytes()).map_err(|_| full_code())?;
    for function in functions {
        code.put(&function.to_bytes()).map_err(|_| full_code())?;
    }
    code.pad_to_8().map_err(|_| full_code())?;
    code.put(instructions).map_err(|_| full_code())?;
    code.pad_to_8().map_err(|_| full_code())?;
    code.put(constants).map_err(|_| full_code())?;
    code.pad_to_8().map_err(|_| full_code())?;
    for line in lines {
        code.put(&line.off.to_le_bytes()).map_err(|_| full_code())?;
        code.put(&line.unit.to_le_bytes())
            .map_err(|_| full_code())?;
        code.put(&line.line.to_le_bytes())
            .map_err(|_| full_code())?;
        code.put(&line.column.to_le_bytes())
            .map_err(|_| full_code())?;
    }
    let code_len = to_u32(code.len(), path, unit_name, "code blob")?;
    let code: &'a [u8] = code.into_written();

    let manifest_header = ManifestHeader {
        magic: MANIFEST_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        unit_name_len: unit_name.len() as u16,
        endpoint_count: endpoints.len() as u16,
        code_len,
        state_size,
        stack_size,
        entry,
        capacity_profile: 0,
    };
    let mut manifest = Out::new(manifest_buffer);
    manifest
        .put(&manifest_header.to_bytes())
        .map_err(|_| full_manifest())?;
    manifest
        .put(unit_name.as_bytes())
        .map_err(|_| full_manifest())?;
    // Descriptor order is the state-prefix order: endpoint `i`'s 8-byte handle
    // lives at `state[i * 8]` (spec 0006, spec 0008).
    for endpoint in endpoints {
        if endpoint.name.len() > u16::MAX as usize {
            return Err(diag(
                path,
                unit_name,
                endpoint.line,
                endpoint.column,
                format_args!("stream name is too long"),
            ));
        }
        let descriptor = Endpoint {
            name_len: endpoint.name.len() as u16,
            direction: endpoint.direction,
            _reserved: 0,
            capacity: endpoint.capacity,
        };
        manifest
            .put(&descriptor.to_bytes())
            .map_err(|_| full_manifest())?;
        manifest
            .put(endpoint.name.as_bytes())
            .map_err(|_| full_manifest())?;
    }
    let manifest: &'a [u8] = manifest.into_written();

    Ok(Package { manifest, code })
}

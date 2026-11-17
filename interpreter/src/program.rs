//! Structural verification and section decoding.
//!
//! The verifier is deliberately structural, not a proof (spec 0008, B7): it
//! checks the magic, ABI version, profile, section bounds, function
//! descriptors, opcodes, operand ranges, function indices, constant ranges,
//! frame sizes and jump targets. It never parses source and never trusts the
//! compiler with a byte it can check itself.
//!
//! Jump-target validation needs one bit per instruction word. The caller
//! provides that scratch buffer, so the verifier never allocates:
//! `scratch.len() * 8 >= code_len / 4`.

use core::fmt;

use abi::{
    opcodes, CodeHeader, Function, Status, ABI_VERSION, CODE_HEADER_SIZE, CODE_MAGIC,
    FUNCTION_SIZE, LINE_ENTRY_SIZE, NO_DIAGNOSTIC, NO_NAME, PROFILE_X86_64_LE,
};

/// Largest section accepted by the verifier (1 MiB, matching the compiler's
/// per-value cap). This bounds the scratch requirement and keeps every later
/// offset arithmetic comfortably inside 32 bits.
const MAX_SECTION: u32 = 1 << 20;

/// Maximum number of functions in one package.
const MAX_FUNCTIONS: u32 = 1 << 16;

/// Maximum line-table unit id. ABI v1 packages name at most two units: the
/// application (0) and the embedded `stdlib` (1). The compiler assigns them.
const MAX_UNITS: u32 = 1;

/// A package that failed verification, or a verifier whose scratch buffer was
/// too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    /// `Status::Invalid` for a bad package, `Status::CapacityExceeded` when
    /// `scratch` was too small.
    pub status: Status,
    /// Byte offset in the code blob where the problem was found.
    pub offset: u32,
    /// Short static message.
    pub message: &'static str,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at code offset {:#x}: {}",
            self.status.name(),
            self.offset,
            self.message
        )
    }
}

fn invalid(offset: usize, message: &'static str) -> Diagnostic {
    Diagnostic {
        status: Status::Invalid,
        offset: offset as u32,
        message,
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let slice = bytes.get(offset..end)?;
    let mut word = [0u8; 8];
    word.copy_from_slice(slice);
    Some(u64::from_le_bytes(word))
}

fn align8(value: usize) -> Option<usize> {
    Some((value.checked_add(7)?) & !7usize)
}

struct Sections {
    functions: usize,
    instructions: usize,
    constants: usize,
    lines: usize,
    end: usize,
}

fn decode_sections(code: &[u8]) -> Result<(CodeHeader, Sections), Diagnostic> {
    if code.len() < CODE_HEADER_SIZE {
        return Err(invalid(0, "code blob is shorter than the header"));
    }
    let header = CodeHeader::from_bytes(&code[..CODE_HEADER_SIZE])
        .ok_or_else(|| invalid(0, "malformed code header"))?;
    if header.magic != CODE_MAGIC {
        return Err(invalid(0, "bad code magic"));
    }
    if header.abi_version != ABI_VERSION {
        return Err(invalid(4, "ABI version mismatch"));
    }
    if header.profile != PROFILE_X86_64_LE {
        return Err(invalid(6, "target profile mismatch"));
    }
    if header.function_count == 0 || header.function_count > MAX_FUNCTIONS {
        return Err(invalid(8, "function count out of range"));
    }
    if header.entry >= header.function_count {
        return Err(invalid(12, "entry function index out of range"));
    }
    for (offset, value) in [
        (16, header.code_len),
        (20, header.const_len),
        (24, header.line_len),
        (28, header.state_size),
        (32, header.stack_size),
    ] {
        if value > MAX_SECTION {
            return Err(invalid(offset, "section exceeds the verifier's cap"));
        }
    }
    if !(header.line_len as usize).is_multiple_of(LINE_ENTRY_SIZE) {
        return Err(invalid(24, "line table length is not a multiple of 16"));
    }
    if header.diag_offset != NO_DIAGNOSTIC
        && (header.diag_offset % 8 != 0 || header.diag_offset > header.state_size)
    {
        return Err(invalid(36, "diagnostic slot offset out of range"));
    }

    let functions_end = CODE_HEADER_SIZE
        .checked_add(header.function_count as usize * FUNCTION_SIZE)
        .ok_or_else(|| invalid(40, "function table overflows"))?;
    let instructions = align8(functions_end).ok_or_else(|| invalid(40, "section overflow"))?;
    let constants = align8(
        instructions
            .checked_add(header.code_len as usize)
            .ok_or_else(|| invalid(40, "section overflow"))?,
    )
    .ok_or_else(|| invalid(40, "section overflow"))?;
    let lines = align8(
        constants
            .checked_add(header.const_len as usize)
            .ok_or_else(|| invalid(40, "section overflow"))?,
    )
    .ok_or_else(|| invalid(40, "section overflow"))?;
    let end = lines
        .checked_add(header.line_len as usize)
        .ok_or_else(|| invalid(40, "section overflow"))?;
    if end != code.len() {
        return Err(invalid(
            end.min(code.len()),
            "code blob length does not match the header",
        ));
    }
    for (start, stop) in [
        (functions_end, instructions),
        (instructions + header.code_len as usize, constants),
        (constants + header.const_len as usize, lines),
    ] {
        if code[start..stop].iter().any(|byte| *byte != 0) {
            return Err(invalid(start, "section padding is not zero"));
        }
    }

    Ok((
        header,
        Sections {
            functions: CODE_HEADER_SIZE,
            instructions,
            constants,
            lines,
            end,
        },
    ))
}

/// A verified package.
#[derive(Debug, Clone, Copy)]
pub struct Program<'a> {
    header: CodeHeader,
    functions: &'a [u8],
    instructions: &'a [u8],
    constants: &'a [u8],
    lines: &'a [u8],
}

impl<'a> Program<'a> {
    /// Verifies `code` and decodes its sections.
    pub fn parse(code: &'a [u8], scratch: &mut [u8]) -> Result<Self, Diagnostic> {
        let (header, sections) = decode_sections(code)?;
        verify_functions(code, &header, &sections)?;
        verify_instructions(code, &header, &sections, scratch)?;
        verify_lines(code, &header, &sections)?;
        Ok(Program {
            header,
            functions: &code[sections.functions..sections.instructions],
            instructions: &code[sections.instructions..sections.constants],
            constants: &code[sections.constants..sections.lines],
            lines: &code[sections.lines..sections.end],
        })
    }

    /// The verified code header.
    pub fn header(&self) -> &CodeHeader {
        &self.header
    }

    /// The instruction section.
    pub fn instructions(&self) -> &'a [u8] {
        self.instructions
    }

    /// The constant blob.
    pub fn constants(&self) -> &'a [u8] {
        self.constants
    }

    /// Returns the function descriptor at `index`.
    pub fn function(&self, index: u32) -> Option<Function> {
        if index >= self.header.function_count {
            return None;
        }
        let start = (index as usize).checked_mul(FUNCTION_SIZE)?;
        let end = start.checked_add(FUNCTION_SIZE)?;
        Function::from_bytes(self.functions.get(start..end)?)
    }

    /// Looks up the source line and column for an instruction offset.
    pub fn line_for(&self, pc: u32) -> Option<(u32, u32)> {
        let count = self.lines.len() / LINE_ENTRY_SIZE;
        let mut low = 0usize;
        let mut high = count;
        while low < high {
            let mid = (low + high) / 2;
            let offset = read_u32(self.lines, mid * LINE_ENTRY_SIZE)?;
            if offset <= pc {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == 0 {
            return None;
        }
        let entry = (low - 1) * LINE_ENTRY_SIZE;
        let line = read_u32(self.lines, entry + 8)?;
        let column = read_u32(self.lines, entry + 12)?;
        Some((line, column))
    }
}
/// Verifies one code blob. Equivalent to [`Program::parse`] without keeping
/// the decoded sections.
pub fn verify(code: &[u8], scratch: &mut [u8]) -> Result<(), Diagnostic> {
    Program::parse(code, scratch).map(|_| ())
}

fn verify_functions(
    code: &[u8],
    header: &CodeHeader,
    sections: &Sections,
) -> Result<(), Diagnostic> {
    let mut previous_end = 0u32;
    for index in 0..header.function_count {
        let start = sections.functions + index as usize * FUNCTION_SIZE;
        let function = Function::from_bytes(&code[start..start + FUNCTION_SIZE])
            .ok_or_else(|| invalid(start, "malformed function descriptor"))?;
        if function.code_off != previous_end {
            return Err(invalid(start, "function code offsets are not contiguous"));
        }
        if function.code_off % 4 != 0 {
            return Err(invalid(start, "function code offset is not word aligned"));
        }
        if function.frame_size % 8 != 0 || function.frame_size < 32 {
            return Err(invalid(start + 4, "frame size is misaligned or too small"));
        }
        let locals = 32u32
            .checked_add(function.param_count as u32 * 8)
            .ok_or_else(|| invalid(start + 8, "parameter count overflows"))?;
        if function.operand_off % 8 != 0
            || function.operand_off < locals
            || function.operand_off > function.frame_size
        {
            return Err(invalid(start + 8, "operand area is out of range"));
        }
        if function.line_base as usize > header.line_len as usize / LINE_ENTRY_SIZE {
            return Err(invalid(start + 12, "line base is out of range"));
        }
        if function.name_off != NO_NAME
            && (function.name_off % 8 != 0
                || function.name_off < 8
                || function.name_off > header.const_len)
        {
            return Err(invalid(start + 16, "function name offset is out of range"));
        }
        if function._reserved != [0, 0] {
            return Err(invalid(start + 22, "function reserved bytes must be zero"));
        }

        let end = if index + 1 < header.function_count {
            let next_start = sections.functions + (index + 1) as usize * FUNCTION_SIZE;
            let next = Function::from_bytes(&code[next_start..next_start + FUNCTION_SIZE])
                .ok_or_else(|| invalid(next_start, "malformed function descriptor"))?;
            next.code_off
        } else {
            header.code_len
        };
        if end <= function.code_off || (end - function.code_off) % 4 != 0 {
            return Err(invalid(start, "function range is empty or misaligned"));
        }
        previous_end = end;
    }
    if previous_end != header.code_len {
        return Err(invalid(
            sections.instructions,
            "functions do not cover the instruction section",
        ));
    }
    Ok(())
}

fn verify_instructions(
    code: &[u8],
    header: &CodeHeader,
    sections: &Sections,
    scratch: &mut [u8],
) -> Result<(), Diagnostic> {
    let words = header.code_len as usize / 4;
    let needed = words.div_ceil(8);
    if scratch.len() < needed {
        return Err(Diagnostic {
            status: Status::CapacityExceeded,
            offset: 0,
            message: "verification scratch buffer is too small",
        });
    }
    scratch[..needed].fill(0);

    let base = sections.instructions;
    let instructions = &code[base..base + header.code_len as usize];

    // Pass 1: decode every function and record instruction boundaries.
    for index in 0..header.function_count {
        let function = program_function(code, sections, index)
            .ok_or_else(|| invalid(sections.functions, "malformed function descriptor"))?;
        let start = function.code_off as usize;
        let end = function_end(code, sections, header, index)?
            .ok_or_else(|| invalid(sections.functions, "function range is not contiguous"))?;
        if start >= end {
            return Err(invalid(base + start, "empty function"));
        }
        let mut pc = start;
        while pc < end {
            set_bit(scratch, pc / 4);
            if end - pc < 4 {
                return Err(invalid(base + pc, "truncated instruction"));
            }
            let op = instructions[pc];
            let a = instructions[pc + 1];
            let b = instructions[pc + 2];
            let c = instructions[pc + 3];
            let word_count = opcodes::instruction_words(op)
                .ok_or_else(|| invalid(base + pc, "unknown opcode"))?
                as usize;
            let size = word_count * 4;
            if pc + size > end {
                return Err(invalid(base + pc, "truncated instruction"));
            }

            if op == opcodes::CONVERT {
                if a > opcodes::value_kind::F64 || b > opcodes::value_kind::F64 || c != 0 {
                    return Err(invalid(base + pc, "conversion kinds are out of range"));
                }
            } else if op == opcodes::SUBSTR {
                // `a` is the destination kind and `b` the source kind; all
                // four combinations are legal (spec 0008, `SUBSTR`).
                if a > opcodes::buffer_kind::BYTES || b > opcodes::buffer_kind::BYTES || c != 0 {
                    return Err(invalid(base + pc, "substr kind is out of range"));
                }
            } else if op == opcodes::FORMAT_INT {
                if a > opcodes::buffer_kind::BYTES || b != 0 || c != 0 {
                    return Err(invalid(base + pc, "format_int kind is out of range"));
                }
            } else if op == opcodes::FORMAT_F64 {
                if a > opcodes::buffer_kind::BYTES || b != 0 || c != 0 {
                    return Err(invalid(base + pc, "format_f64 kind is out of range"));
                }
            } else if op == opcodes::FORMAT_F64_SHORT {
                if a > opcodes::buffer_kind::BYTES || b != 0 || c != 0 {
                    return Err(invalid(base + pc, "format_f64_short kind is out of range"));
                }
            } else if matches!(
                op,
                opcodes::MAP_PUT | opcodes::MAP_GET | opcodes::MAP_HAS | opcodes::MAP_REMOVE
            ) {
                if a > opcodes::map_key_kind::STRING
                    || b > opcodes::map_value_kind::BUFFER
                    || c != 0
                {
                    return Err(invalid(base + pc, "map kinds are out of range"));
                }
            } else if matches!(
                op,
                opcodes::JMP | opcodes::JZ | opcodes::JNZ | opcodes::CALL
            ) {
                if a != 0 || b != 0 || c > 1 {
                    return Err(invalid(
                        base + pc,
                        "reserved instruction fields must be zero",
                    ));
                }
            } else if a != 0 || b != 0 || c != 0 {
                return Err(invalid(
                    base + pc,
                    "reserved instruction fields must be zero",
                ));
            }

            match op {
                opcodes::LOAD_LOCAL | opcodes::STORE_LOCAL | opcodes::REF_LOCAL => {
                    let off = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?
                        as usize;
                    let locals_end = (function.operand_off as usize).saturating_sub(32);
                    if off.checked_add(8).is_none_or(|end| end > locals_end) {
                        return Err(invalid(base + pc + 4, "local offset is out of range"));
                    }
                }
                opcodes::LOAD_STATE | opcodes::STORE_STATE | opcodes::REF_STATE => {
                    let off = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?
                        as usize;
                    if off
                        .checked_add(8)
                        .is_none_or(|end| end > header.state_size as usize)
                    {
                        return Err(invalid(base + pc + 4, "state offset is out of range"));
                    }
                }
                opcodes::REF_CONST => {
                    let off = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?
                        as usize;
                    if !off.is_multiple_of(8) || off < 8 || off > header.const_len as usize {
                        return Err(invalid(base + pc + 4, "constant offset is out of range"));
                    }
                    let length = read_u64(&code[sections.constants..sections.lines], off - 8)
                        .ok_or_else(|| invalid(base + pc + 4, "truncated buffer header"))?;
                    if length > (header.const_len as usize - off) as u64 {
                        return Err(invalid(base + pc + 4, "constant range is out of bounds"));
                    }
                }
                opcodes::APPEND => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if capacity > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "buffer capacity is out of range"));
                    }
                }
                opcodes::READ_INPUT => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if capacity > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "buffer capacity is out of range"));
                    }
                }
                opcodes::STORE_U8 => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if capacity > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "buffer capacity is out of range"));
                    }
                }
                opcodes::STREAM_RECV | opcodes::STREAM_TRY_RECV => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // A zero-capacity destination can never receive a byte, so
                    // it is not a valid instruction.
                    if capacity == 0 || capacity > MAX_SECTION {
                        return Err(invalid(
                            base + pc + 4,
                            "stream destination capacity is out of range",
                        ));
                    }
                }
                opcodes::SUBSTR => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if capacity > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "buffer capacity is out of range"));
                    }
                }
                opcodes::FORMAT_INT => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if capacity > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "buffer capacity is out of range"));
                    }
                }
                opcodes::FORMAT_F64 => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // A zero-capacity destination can never receive a digit,
                    // so it is not a valid instruction.
                    if capacity == 0 || capacity > MAX_SECTION {
                        return Err(invalid(
                            base + pc + 4,
                            "format_f64 destination capacity is out of range",
                        ));
                    }
                }
                opcodes::FORMAT_F64_SHORT => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // A zero-capacity destination can never receive a digit,
                    // so it is not a valid instruction.
                    if capacity == 0 || capacity > MAX_SECTION {
                        return Err(invalid(
                            base + pc + 4,
                            "format_f64_short destination capacity is out of range",
                        ));
                    }
                }
                opcodes::ENUM_CAST => {
                    let count = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // An enum has at least one variant; the count bounds the
                    // ordinal check, so a zero or absurd count is invalid.
                    if count == 0 || count > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "enum variant count is out of range"));
                    }
                }
                opcodes::REF_OFFSET => {
                    let offset = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // Field offsets are 8-byte aligned (spec 0003, records).
                    if !offset.is_multiple_of(8) || offset > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "field offset is out of range"));
                    }
                }
                opcodes::REF_ELEMENT => {
                    let stride = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    let count = read_u32(instructions, pc + 8)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // Element strides are 8-byte aligned (spec 0003, arrays),
                    // and the whole array must fit one section.
                    if stride == 0 || !stride.is_multiple_of(8) || stride > MAX_SECTION {
                        return Err(invalid(base + pc + 4, "array stride is out of range"));
                    }
                    if count == 0
                        || count > MAX_SECTION
                        || stride
                            .checked_mul(count)
                            .is_none_or(|size| size > MAX_SECTION)
                    {
                        return Err(invalid(
                            base + pc + 8,
                            "array element count is out of range",
                        ));
                    }
                }
                opcodes::MAP_PUT | opcodes::MAP_GET | opcodes::MAP_HAS | opcodes::MAP_REMOVE => {
                    let capacity = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    let count = read_u32(instructions, pc + 8)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    let key_size = read_u32(instructions, pc + 12)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    let value_size = read_u32(instructions, pc + 16)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    // The map layout (spec 0003, "Maps"): a power-of-two table
                    // of at least twice the entry limit, 8-byte-multiple key
                    // and value images, and a whole image inside one section.
                    if !capacity.is_power_of_two() || capacity < 2 {
                        return Err(invalid(
                            base + pc + 4,
                            "map table capacity must be a power of two of at least 2",
                        ));
                    }
                    if count == 0 || u64::from(count) * 2 > u64::from(capacity) {
                        return Err(invalid(
                            base + pc + 8,
                            "map entry limit does not fit the table capacity",
                        ));
                    }
                    if key_size == 0 || !key_size.is_multiple_of(8) || key_size > MAX_SECTION {
                        return Err(invalid(base + pc + 12, "map key image is out of range"));
                    }
                    if value_size == 0 || !value_size.is_multiple_of(8) || value_size > MAX_SECTION
                    {
                        return Err(invalid(base + pc + 16, "map value image is out of range"));
                    }
                    let stride = 16u64 + u64::from(key_size) + u64::from(value_size);
                    let total = 8u64 + stride * u64::from(capacity);
                    if total > u64::from(MAX_SECTION) {
                        return Err(invalid(base + pc + 4, "map image exceeds the section cap"));
                    }
                }
                opcodes::BUILTIN => {
                    let id = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if opcodes::builtin::effect(id).is_none() {
                        return Err(invalid(base + pc + 4, "unknown builtin"));
                    }
                }
                opcodes::CALL => {
                    let target = read_u32(instructions, pc + 4)
                        .ok_or_else(|| invalid(base + pc, "truncated immediate"))?;
                    if target >= header.function_count {
                        return Err(invalid(base + pc + 4, "function index is out of range"));
                    }
                }
                _ => {}
            }

            pc += size;
        }
        if pc != end {
            return Err(invalid(
                base + start,
                "instruction stream overruns its function",
            ));
        }
    }

    // Pass 2: every jump must land on a recorded instruction boundary of the
    // same function.
    for index in 0..header.function_count {
        let function = program_function(code, sections, index)
            .ok_or_else(|| invalid(sections.functions, "malformed function descriptor"))?;
        let start = function.code_off as usize;
        let end = function_end(code, sections, header, index)?
            .ok_or_else(|| invalid(sections.functions, "function range is not contiguous"))?;
        let mut pc = start;
        while pc < end {
            if end - pc < 4 {
                return Err(invalid(base + pc, "truncated instruction"));
            }
            let op = instructions[pc];
            let size = opcodes::instruction_words(op)
                .ok_or_else(|| invalid(base + pc, "unknown opcode"))?
                as usize
                * 4;
            if matches!(op, opcodes::JMP | opcodes::JZ | opcodes::JNZ) {
                let rel = read_u32(instructions, pc + 4)
                    .ok_or_else(|| invalid(base + pc, "truncated immediate"))?
                    as i32;
                let next = (pc + size) as i64;
                let target = next + rel as i64 * 4;
                if target < start as i64 || target >= end as i64 || target % 4 != 0 {
                    return Err(invalid(
                        base + pc + 4,
                        "jump target is outside the function",
                    ));
                }
                if !get_bit(scratch, target as usize / 4) {
                    return Err(invalid(
                        base + pc + 4,
                        "jump target is not an instruction boundary",
                    ));
                }
            }
            pc += size;
        }
    }

    Ok(())
}

fn program_function(code: &[u8], sections: &Sections, index: u32) -> Option<Function> {
    let start = sections.functions + index as usize * FUNCTION_SIZE;
    Function::from_bytes(code.get(start..start + FUNCTION_SIZE)?)
}

fn function_end(
    code: &[u8],
    sections: &Sections,
    header: &CodeHeader,
    index: u32,
) -> Result<Option<usize>, Diagnostic> {
    if index + 1 < header.function_count {
        let next = program_function(code, sections, index + 1)
            .ok_or_else(|| invalid(sections.functions, "malformed function descriptor"))?;
        Ok(Some(next.code_off as usize))
    } else {
        Ok(Some(header.code_len as usize))
    }
}

fn verify_lines(code: &[u8], header: &CodeHeader, sections: &Sections) -> Result<(), Diagnostic> {
    let count = header.line_len as usize / LINE_ENTRY_SIZE;
    let lines = &code[sections.lines..sections.end];
    let mut previous = None;
    for index in 0..count {
        let offset = read_u32(lines, index * LINE_ENTRY_SIZE)
            .ok_or_else(|| invalid(sections.lines, "truncated line table"))?;
        let unit = read_u32(lines, index * LINE_ENTRY_SIZE + 4)
            .ok_or_else(|| invalid(sections.lines, "truncated line table"))?;
        if offset >= header.code_len {
            return Err(invalid(
                sections.lines + index * LINE_ENTRY_SIZE,
                "line-table code offset is out of range",
            ));
        }
        if unit > MAX_UNITS {
            return Err(invalid(
                sections.lines + index * LINE_ENTRY_SIZE + 4,
                "line-table unit id is out of range",
            ));
        }
        if let Some(previous) = previous {
            if offset < previous {
                return Err(invalid(
                    sections.lines + index * LINE_ENTRY_SIZE,
                    "line table is not sorted by code offset",
                ));
            }
        }
        previous = Some(offset);
    }
    Ok(())
}

fn set_bit(bits: &mut [u8], index: usize) {
    if let Some(byte) = bits.get_mut(index / 8) {
        *byte |= 1 << (index % 8);
    }
}

fn get_bit(bits: &[u8], index: usize) -> bool {
    bits.get(index / 8)
        .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
}

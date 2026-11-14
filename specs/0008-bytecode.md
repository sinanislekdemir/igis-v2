# 0008: Bytecode format and the compiler/runtime split

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `repl` (in-kernel compile), host language
  tooling
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002 (interpreted applications), 0005 (internal bytecode, no JIT), 0007 (fully
  static placement), 0011 (work stealing), 0018 (floating point), 0019 (source-only
  delivery)
- **Updated:** 2026-11-14

## Context

Applications are source text, not native binaries (ADR 0002). Something has to turn that
source into something the kernel can execute, and that something must fit the constraints
of the rest of the system:

- The compiler runs in the kernel at load time (ADR 0017), on memory that was allocated
  before it started; it cannot call `malloc` or grow a buffer.
- The interpreter runs inside the scheduler's quantum, at a safe point granularity of one
  bytecode (spec 0007), and must never leave a half-applied instruction visible.
- Every value lives inlined in a region or in a frame; there are no pointers the
  application can name (spec 0004).
- There is no JIT (ADR 0005). The compiled form is what executes, forever.
- Floating point is `f64` on SSE2 (ADR 0018), so arithmetic must be expressible without
  host libraries.

This spec fixes the artifact the compiler produces and the interpreter consumes. It does
not specify the source language (spec 0003) or the loader's filesystem (spec 0002); it
specifies the boundary between them.

Delivery is source-only and permanent (ADR 0019): the image never carries bytecode, and a
package exists only in memory for the lifetime of one instance. Compile latency is
explicitly not a design constraint; the compiler may be as slow as correctness requires.

Non-goals: source compatibility guarantees, an assembler, a linker between separately
compiled units, debugger integration, optimising compilation, and any form of precompiled
or cached delivery. The first milestone compiles whole applications, once per instance,
into one package.

## Requirements

- **B1 (MUST):** Bytecode is the only executable form. There is no JIT, no native code
  generation, and no way to execute memory.
- **B2 (MUST):** A loaded code region is immutable and read-only.
- **B3 (MUST):** Every instruction boundary is a safe point. The interpreter decrements a
  step counter per instruction and takes the slow path (clock, budget, heartbeat, yield,
  kill) when it reaches zero; backward branches and calls also carry an explicit poll bit
  as a second line of defence.
- **B4 (MUST):** Every load and store is bounds-checked against the owning region and, for
  buffers, against the compile-time capacity and the runtime length. No instruction holds
  a raw address.
- **B5 (MUST):** The interpreter never parses source and never links the compiler.
- **B6 (MUST):** The artifact carries an ABI version. The loader refuses an artifact whose
  version differs from the interpreter's.
- **B7 (MUST):** The loader verifies the artifact structurally before executing it:
  opcodes, operand ranges, jump targets, constant ranges, function indices, frame sizes.
- **B8 (MUST):** The artifact states `state` size, stack requirement, code and constant
  sizes, declared endpoints and the build-parameter profile.
- **B9 (MUST):** Compilation is deterministic: the same source with the same build
  parameters produces an identical artifact.
- **B10 (MUST):** A code offset maps to a unit, line and column for diagnostics.
- **B11 (MUST):** Compilation uses preallocated, bounded memory. Exceeding the source or
  scratch limit is a `Status` error, not an allocation.
- **B12 (MUST):** The compiler and the interpreter are separate crates over a shared `abi`
  crate; neither depends on the other's internals.
- **B13 (MUST):** Compilation always happens at load, in the kernel, from source. A package
  is an in-memory value with the lifetime of one instance; it is never persisted, cached
  across boots, or delivered through the image (ADR 0019).

## Design

### One artifact per application

The compiler consumes every unit reachable from an application's `uses` closure and emits
one package:

```
package
├── manifest        (ABI version, sizes, entry, endpoints, build profile)
└── code blob
    ├── header      (magic, version, counts, state/stack sizes)
    ├── functions   (one descriptor per function)
    ├── instructions
    ├── constants   (8-byte-aligned pool, including buffer images)
    └── line table  (code offset → unit, line, column)
```

In the first milestone the package is produced in memory from source read out of ext2, and
that is the only way a package ever comes into existence. It has the lifetime of one task:
no part of it is written back, cached for a later start, or stored in the image (ADR 0019).
A second start of the same application compiles its source again.

### Multi-unit compilation (`stdlib`)

An application that says `uses stdlib;` is compiled together with the embedded library unit
(`stdlib/stdlib.ig`, included with `include_str!`) into **one package**:

- **Name resolution.** The application's top-level names stay unqualified; the library's
  are reachable only as `stdlib.<name>`. Inside the library its own names are unqualified,
  and it may import only `core` — it cannot reference the application (no cycle, M4).
  A `stdlib.*` reference without the import, or an unknown `stdlib.*` name, is a compile
  error.
- **Function table.** Library procedures occupy the first indices (`0..L`), application
  procedures follow (`L..L+A`); cross-unit calls resolve to those indices. The entry is the
  application's `main`.
- **One instruction section, one constant blob.** Both units share the constant pool in
  first-use order, so deterministic output and one artifact hold.
- **Line table.** Each entry carries a unit id: `0` application, `1` embedded `stdlib`.
  The verifier accepts unit ids `0..=1`. Compile diagnostics name the failing unit and path
  (`stdlib/stdlib.ig:line:col: ...`), so a broken library is an ordinary `Diagnostic`.
- **Manifest.** It keeps the application's unit name and entry point.
- **Types.** Type ids are compiler-local per unit. A library signature may use scalars,
  buffers, `Status` and `FaultCode`; a record, sum or user enum crossing the unit boundary
  is a compile error until a shared type table exists.
- **Cost.** The library is recompiled from source for every application instance that
  imports it (ADR 0019); nothing precompiled is ever shipped.

### Target profile

x86_64, little-endian, 8-byte words, SSE2 doubles. The artifact records the profile; a
mismatch is refused like an ABI-version mismatch. Nothing in the format assumes a
particular core count or quantum size.

### Values in the VM

| Value | Representation |
|-------|----------------|
| `bool`, `byte` | 1 byte in memory, zero-extended to 64 bits in an operand |
| `int`, `uint`, `f64` | 8 bytes; `f64` is an IEEE-754 bit pattern |
| `bytes[N]`, `string[N]` | `[len: u64][data: N]`, 8-byte aligned; `len <= N` |
| `array[T,N]` | `N` elements at `stride = align8(size_of(T))`; the array size is `align8(N * stride)` (spec 0003) |
| enum | 8-byte ordinal, nominally typed (spec 0003) |
| record | fields in declaration order, each aligned to 8; size rounded to 8 (spec 0003) |
| sum type | 8-byte tag at offset 0, then the largest variant payload at offset 8; size rounded to 8 (spec 0003) |
| `ring[T,N]` | `N` elements at `stride = align8(size_of(T))`, then `head: u64` and `count: u64`; size `align8(N*stride + 16)` (spec 0003) |
| `map[K,V,N]` | table of `2^ceil(log2(2N))` entries, FNV-1a hashing (spec 0003) |
| endpoint | 8-byte handle, direction checked by the type checker |
| `TaskId` | 8-byte integer; zero is invalid |
| reference | 64-bit `(region:2, offset:62)` |

A call-scoped `const`/`var` parameter is a reference; its capacity is the callee's declared
N and its runtime length is read from the buffer header at `offset - 8`. The compiler's
at-least-N rule guarantees the actual buffer is wide enough; the interpreter still checks
the offset against the region size.

Records are inline values with the same rule. A field reference is formed from the
record's reference by `ref_offset`, one step per field, so every intermediate offset is
bounds-checked against the owning region. A scalar or nested-record field is referenced at
its first byte; a buffer field is referenced at its data (`field_offset + 8`), which leaves
the length word at `field_offset` for the ordinary buffer opcodes. A whole-record
assignment (`dst := src`) and a `value` record parameter copy are `copy_refs` with the
compile-time record size, which is an overlap-safe move inside one region.

Sum types add **no opcodes**. Construction writes the tag with `const` + `store_ref` at the
sum's first byte and the payload through the same instructions a value of the payload type
uses: a scalar/enum payload is `store_ref` at `+8`, a buffer payload is `clear`/`append`
against `sum + 16` (the data area, leaving the length word at `+8`), and a record or nested
sum payload is a whole-image `copy_refs` from `+8`. A whole-sum assignment and a `value`
sum parameter are `copy_refs` with the compile-time sum size. `case` reads the tag with
`load_ref` at the sum's first byte, compares it with `eq` against each variant ordinal, and
branches with `jz`; an arm's payload binding is a `copy_refs` of the payload size from
`sum + 8` into a fresh local, so the binding is a writable by-value copy. The compiler
proves exhaustiveness, so the final arm needs no run-time fallback; a tag that no arm
matches cannot be produced by a compiled package.

Arrays add **one** opcode, `ref_element` (immediates `stride` and `count`): it pops a
base reference and an index, checks `index < count`, and pushes `base + index * stride`.
An element is then read or written with the same instructions as a field: `load_ref` and
`store_ref` for scalars and enums, `copy_refs` for records, sums and nested arrays, and
the buffer opcodes for a buffer element (indexed after the element reference). A whole
array assignment and a `value` array parameter are `copy_refs` with the compile-time array
size. Nested access (`a[i][j]`, `a[i].field`) is a chain of `ref_element` and
`ref_offset` steps. The compiler interns `(element type, count)` structurally in one
table shared by the application and `stdlib`, so the same written array type has one id
on both sides of a call.

Rings add **no opcode**. `core.ring_push`/`pop`/`peek`/`full`/`empty` are compiler-lowered
over the ring layout (`N` elements, then `head` and `count`): the header is read with
`ref_offset` + `load_ref`, the physical index is `(head + count) % N` or `head` with
`add_u`/`mod_u`, the element reference is `ref_element` with the ring's stride and `N`,
and the value moves through `load_ref`/`store_ref` (scalars and enums), `copy_refs`
(records, sums, arrays, nested rings) or `clear`/`append` (buffers). The statuses are the
`Status` ordinals pushed with `const` (0 `Ok`, 3 `NotFound`, 5 `Full`), and `eq` + `jz`
select the failure branch before anything is written, so a failed `push`/`pop`/`peek`
leaves the ring and the out-parameter untouched. A whole-ring assignment and a `value`
ring parameter are `copy_refs` with the compile-time image size, header included. The
compiler interns `(element type, count)` in one structural ring table shared by the
application and `stdlib`, so a ring crosses the unit boundary with one id.

### Frames

Each call pushes one frame on the task's stack region:

```
frame = [ prev_fp: u64 ][ return_pc: u32 ][ function: u32 ][ return_sp: u64 ]
        [ dest_ref: u64 ][ locals... ][ operands... ]
```

The header is 32 bytes, so locals start at offset 32. `return_sp` restores the
caller's operand-stack top and `dest_ref` names the destination that receives a
function result. Every scalar, including `bool` and `byte`, occupies one 8-byte
local slot in the first implementation; records, sums and arrays use their
packed, 8-byte-aligned layout inline in the locals area.

- Locals are addressed by slot index, 8-byte aligned; buffers are inline in the locals
  area, so a large local is a large frame (spec 0004).
- The operand area is a fixed-depth stack of 8-byte slots. The compiler computes the
  maximum depth per function and records it in the frame size.
- On function entry the interpreter checks the frame against the stack region limit and
  faults `StackOverflow` with a `Diagnostic`; the guard page remains the backstop.
- A function that produces a value writes it into a caller-provided destination slot, which
  is how destination construction stays allocation-free. Procedures have no destination.

### Instruction encoding

Instructions are made of 4-byte words:

```
word 0:  op:u8   a:u8   b:u8   c:u8
word 1+: immediates, when the opcode has them
```

Jump targets are signed 32-bit instruction offsets relative to the next instruction.
Function references are indices into the function table; constant references are
`(offset, len)` into the constant blob. An instruction stream is decoded sequentially; the
decoder validates lengths as it goes.

The opcode set is grouped; the complete table lives in `abi` and is versioned with the
artifact:

| Group | Representative operations |
|-------|---------------------------|
| Constants and moves | `const`, `move`, `copy`, `fill` |
| Integer arithmetic | `add`, `sub`, `mul`, `div`, `mod`, `neg` (checked; overflow faults) |
| Float arithmetic | `fadd`, `fsub`, `fmul`, `fdiv`, `fneg`, `fabs` |
| Comparisons | `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `cmp_buffers` for buffers |
| Conversions | `i2f`, `f2i` (truncate and range-check), `trunc8`, `sext`, `zext` |
| Control flow | `jmp`, `jz`, `jnz`, `call`, `ret` |
| Buffers and aggregates | `append` (capacity-checked), `load_u8`/`store_u8` (length- and capacity-checked), `find` (byte scan), `substr` (checked slice), `format_int` (decimal/hex formatting), `enum_cast` (range-checked ordinal → enum), `ref_offset` (field reference), `ref_element` (array element reference, index-checked), `copy_refs` (record, sum, array, ring and `case` payload copies) |
| Builtins | `call_builtin n` — host-interface and `core` primitives |
| System | `safe` (explicit poll), `trap` (compiler-inserted fault) |

Aggregates are opcodes rather than monomorphised library code because the compiler, not
the runtime, knows each layout; the opcode carries a layout descriptor index.

### Compiler / runtime split

| Crate | Depends on | Runs on | Responsibility |
|-------|------------|---------|----------------|
| `abi` | none | both | opcode numbers, header and manifest layouts, `Status`, `FaultCode`, region ids |
| `compiler` | `abi` | host and kernel | lex, parse, type and capacity check, emit instructions, constants, line table, manifest |
| `interpreter` | `abi` | kernel (and host mock) | decode, execute, safe points, builtin dispatch, diagnostics |

The kernel links both; the host toolchain links both to run the language track without a
machine (vision R7). The interpreter has no parser and the compiler has no execution path,
so a bug in one cannot silently become a feature of the other.

Compile memory is caller-provided: source buffer, token/parse arenas, code buffer and
constant buffer, each with a compile-time cap. One package is compiled at a time because
the service holds the compiler lock (spec 0007), so one set of buffers is enough. The
first-milestone caps (provisional, tuning): 64 KiB of source per unit, 256 KiB per
application, 1 MiB of scratch. They bound memory because memory is statically placed; they
are not a time budget, and compile time is not a design constraint (ADR 0019).

The `compiler` crate is `no_std` and allocation-free: it borrows the source, stores tokens,
AST nodes and name tables in the caller's scratch arena, and writes the package into the
caller's code and manifest buffers. It exports the caps as `MAX_SOURCE_BYTES` (64 KiB),
`MAX_ARENA_BYTES` (1 MiB), `MAX_CODE_BYTES` (256 KiB) and `MAX_MANIFEST_BYTES` (4 KiB), and
`compile_into` reports exhaustion of any of them as `Status.CapacityExceeded` with a
`Diagnostic`. The only `unsafe` in the crate is the alignment/initialisation arithmetic of
the bump arena, isolated in `compiler/src/arena.rs`.

### Verification and diagnostics

- The loader runs a bounded linear verifier (B7). It does not re-typecheck the program; the
  compiler is trusted, the verifier protects the kernel from a corrupt image. A structural
  error refuses the package with `Status.Invalid` and a `Diagnostic`.
- The line table is sorted by code offset and names a unit id (0 application, 1 embedded
  `stdlib`); a fault binary-searches it for `line` and `column`, and the unit id names the
  source unit. The compiler reserves one hidden
  `Diagnostic` slot in `state` for the interpreter to fill before invoking `fault`/`exit`.
- Compile errors are reported through the same `Diagnostic` into the caller's buffer
  (spec 0005); compilation never writes to an application-visible region.

### Determinism

Opcode numbering, constant layout, hashing and float rounding are fixed by the ABI version.
No address, timestamp or random seed enters the artifact, so two builds of the same source
with the same parameters are byte-identical (B9). Nothing is shipped, so this is not a
distribution guarantee; it is what makes the language track testable on the host and a
compiler change reviewable.

## Interfaces

```rust
// Illustrative; the normative layouts are in `abi`.

pub const ABI_VERSION: u16 = 1;

pub struct CodeHeader {
    pub magic: [u8; 4],      // "IGC1"
    pub abi_version: u16,
    pub profile: u16,        // x86_64, little-endian
    pub function_count: u32,
    pub entry: u32,          // function index
    pub code_len: u32,
    pub const_len: u32,
    pub line_len: u32,
    pub state_size: u32,
    pub stack_size: u32,
    pub diag_offset: u32,    // hidden Diagnostic slot in `state`
}

pub struct Function {
    pub code_off: u32,
    pub frame_size: u32,
    pub param_count: u8,
    pub has_destination: bool, // false for procedures
    pub line_base: u32,
    pub name_off: u32,
}

pub fn compile(source: &[u8], profile: &BuildProfile) -> Result<Package, Diagnostic>;
pub fn verify(package: &Package) -> Result<(), Diagnostic>;
pub fn run(task: &mut Task);
```

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Unknown opcode or truncated instruction | Load-time verifier rejects: `Status.Invalid`; never executed |
| Jump target outside the function or mid-instruction | Load-time verifier rejects |
| Constant range outside the blob | Load-time verifier rejects |
| ABI-version or target-profile mismatch | Load-time refusal | 
| Frame exceeds the stack region | `fault(StackOverflow)` with a `Diagnostic`; guard page is the backstop |
| Buffer index beyond the runtime length (read) or capacity (write) | `fault(OutOfBounds)` or `fault(CapacityExceeded)`; never a partial write |
| Integer overflow, divide by zero, shift out of range | `fault(Overflow)`, `fault(DivideByZero)`, `fault(IllegalOperation)` |
| `int(f)` with `NaN` or out of range | `fault(Overflow)` |
| `EnumName(int)` with an ordinal outside `0..count` | `fault(Overflow)`; no invalid enum value exists |
| Source or compile scratch over its cap | `Status.CapacityExceeded` with a `Diagnostic`; no package |
| Compile error | `Status.Invalid` with a `Diagnostic`; no package |
| Corrupt package that still verifies structurally | Undefined; the verifier is structural, not a proof. Treated as a TCB defect and, in the first milestone, a kernel panic with serial output |
| Interpreter bug | Undefined; may corrupt the kernel. No recovery boundary exists (spec 0002) |
| Quantum counter misconfigured | The poll bit on branches and calls restores a release within one branch |

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Tree-walking AST interpreter | Simplest to write, but every node visit costs parsing-like work, frames carry no fixed layout, and capacity proofs have no place to live. Bytecode with fixed frames pays off at the first loop. |
| Register-based VM (Lua-style) | Fewer instructions for arithmetic, but much more compiler complexity (allocation, spilling) and harder structural verification. Revisit only if measurement demands it. |
| JIT to native code | Explicitly out of scope (ADR 0005): it reintroduces executable memory, per-core code caches and a whole load/verify problem. |
| Native object files plus a linker | Contradicts ADR 0002 and the no-address model; the loader would need relocation and a symbol table. |
| Textual bytecode / re-parse at load | Puts a parser in the kernel hot path and gives up the verifier's cheap structural checks. |
| Monomorphised prelude library for aggregates | Code size and startup cost for every application, and the compiler already knows each layout. |
| Storing only bytecode in the image, compiling on the host only | Loses the "source is the program" property of ADR 0002 and the in-kernel REPL. The package format keeps both paths open. |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | ~~Exact opcode table and operand encodings~~ | **Resolved** — ABI v1 is `abi::opcodes`: 63 opcodes, one 4-byte opcode word plus up to two immediate words, `i32` jump offsets in instruction words, 8-byte operand slots. The interpreter implements it. |
| 2 | Stack machine vs a small register machine | Performance measurement after the first loops run. |
| 3 | ~~Verifier depth: structural only, or limited control-flow and stack-depth checks~~ | **Resolved for v1** — structural checks plus one instruction-boundary bit per word for jump targets; operand-stack depth is enforced at run time against the frame's operand area, not by dataflow analysis. |
| 4 | Line-table encoding and compaction | When packages grow past the first few units. |
| 5 | Compile caps (source, scratch) and whether they are per-build parameters | First real applications and the host toolchain. |
| 6 | ~~Whether precompiled packages may ship in the ext2 image~~ | **Resolved** — never. The image carries source only; packages exist in memory for one instance (ADR 0019). |
| 7 | Constant-pool sharing between functions and units | Optimisation, not correctness. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-13 | Initial draft: one package per application, fixed 4-byte-word encoding, stack frames with inline locals, bounds-checked instructions, structural verification, compiler/interpreter crates over `abi`, line-table diagnostics, deterministic artifacts. |
| 2026-11-13 | Delivery fixed as source-only, permanently: the image never carries bytecode and a package lives only as long as its instance (ADR 0019). |
| 2026-11-13 | ABI v1 fixed and implemented on the host: opcode table in `abi::opcodes`, 32-byte frame header with `return_sp`/`dest_ref`, structural verifier with jump-boundary bits, `compiler`/`interpreter`/`langhost` crates. Open questions 1 and 3 resolved. |
| 2026-11-13 | Added the `read_line` buffer opcode (capacity immediate, copied length pushed) for the staged `core.read_line` line discipline (spec 0005, Q44). |
| 2026-11-13 | Buffer indexing, comparison and length: `load_u8` (index vs runtime length, `OutOfBounds`) and `store_u8` (index vs the capacity immediate, `CapacityExceeded`, extends the length and zero-fills the gap), `cmp_buffers` (length then bytes, `bool`), and the `core.len` builtin (id 6, one reference in, `uint` out). ABI v1 grows from 54 to 57 opcodes. |
| 2026-11-14 | Slicing and search: `find` (one reference and one `byte` in, first index or `-1`, `0..len`) and `substr` (`a` is the buffer kind, a capacity immediate, destination/source/start/length on the stack; copies a clamped slice and pushes the length, or `-4` on a negative offset or a `string` code-point split). ABI v1 grows from 57 to 59 opcodes. |
| 2026-11-14 | Integer formatting and ports: `format_int` (`a` is the buffer kind, a capacity immediate; destination/value/base/width on the stack; decimal/hex, sign and zero padding, pushes the byte count or `-4`/`-6`) grows ABI v1 from 59 to 60 opcodes; builtins `port_in8` (7) and `port_out8` (8) route `syslib` through the host with defaults. |
| 2026-11-14 | Enums: `enum_cast` (immediate `count`, pop an `int` ordinal, fault `Overflow` outside `0..count`, push the same slot typed as the enum) grows ABI v1 from 60 to 61 opcodes; enums are an 8-byte ordinal at run time and need no other instruction. Builtins `spawn`/`wait`/`kill` switch to the `Status`-returning ABI: `spawn` pops `(path, args, id_ref)`, `wait` pops `(id, reason_ref)` and writes the staged reason int, `kill` pops `id`; each pushes the `Status` ordinal (spec 0005). |
| 2026-11-14 | Records: `ref_offset` (pop a reference, add an 8-byte-aligned immediate field offset, bounds-check, push) and `copy_refs` (pop a count and a source/destination reference, bounds-check both, overlap-safe move into a writable region) grow ABI v1 from 61 to 63 opcodes. Record layout and field offsets stay a compiler concern; the interpreter sees only byte offsets. `Diagnostic`/`ExitReason` are predeclared records at the fixed `abi::layout` offsets, and the `spawn`/`wait` builtins pop a record reference (`SPAWN` grows from 3 to 4 operands with `diag`; `WAIT` writes the 16-byte `ExitReason`). |
| 2026-11-14 | Sum types need no new opcodes: construction is `const` + `store_ref` for the tag plus the payload type's ordinary instructions, `case` is `load_ref`/`eq`/`jz` over the tag, and a whole-sum copy or `case` binding is `copy_refs` with the compile-time size. ABI v1 stays at 63 opcodes. `ExitReason` becomes the real sum type at the same 16-byte layout (`TAG_OFFSET` 0, `PAYLOAD_OFFSET` 8); `WAIT` writes the tag and the `FaultCode` payload unchanged. |
| 2026-11-14 | Multi-unit packages: an application with `uses stdlib;` compiles the embedded `stdlib/stdlib.ig` into the same package. Library functions come first in one function table (entry is the application's `main`), the constant pool and instruction section are shared, and line-table entries carry a unit id (0 application, 1 stdlib; verifier accepts `0..=1`). The manifest keeps the application's unit name; compile diagnostics name the failing unit. Type ids stay per-unit, so records, sums and user enums may not cross the boundary yet. |
| 2026-11-14 | Rings need no new opcodes: `core.ring_push`/`pop`/`peek`/`full`/`empty` are compiler-lowered with `ref_offset`/`load_ref` for the header, `add_u`/`mod_u` for the wrap, `ref_element` for the element, `load_ref`/`store_ref` or `copy_refs`/`clear`+`append` for the value, and `eq`+`jz` for the status branches. The ABI v1 opcode table is unchanged. The ring value is `N` elements followed by `head` and `count`, and the structural ring table is shared with `stdlib`. |

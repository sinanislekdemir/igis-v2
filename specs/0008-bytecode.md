# 0008: Bytecode format and the compiler/runtime split

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `repl` (in-kernel compile), host language
  tooling
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002 (interpreted applications), 0005 (internal bytecode, no JIT), 0007 (fully
  static placement), 0011 (work stealing), 0018 (floating point), 0019 (source-only
  delivery)
- **Updated:** 2026-11-13

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
| `array[T,N]` | `N` elements, compiler-aligned |
| record | fields in declaration order, compiler-aligned |
| sum type | 8-byte tag, then the largest variant |
| `ring[T,N]` | array layout plus head and count words |
| `map[K,V,N]` | table of `2^ceil(log2(2N))` entries, FNV-1a hashing (spec 0003) |
| endpoint | 8-byte handle, direction checked by the type checker |
| `TaskId` | 8-byte integer; zero is invalid |
| reference | 64-bit `(region:2, offset:62)` |

A call-scoped `const`/`var` parameter is a reference; its capacity is the callee's declared
N and its runtime length is read from the buffer header at `offset - 8`. The compiler's
at-least-N rule guarantees the actual buffer is wide enough; the interpreter still checks
the offset against the region size.

### Frames

Each call pushes one frame on the task's stack region:

```
frame = [ prev_fp: u64 ][ return_pc: u32 ][ function: u32 ][ return_sp: u64 ]
        [ dest_ref: u64 ][ locals... ][ operands... ]
```

The header is 32 bytes, so locals start at offset 32. `return_sp` restores the
caller's operand-stack top and `dest_ref` names the destination that receives a
function result. Every scalar, including `bool` and `byte`, occupies one 8-byte
local slot in the first implementation; the packed layouts of records and
arrays are not part of this slice.

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
| Buffers and aggregates | `append` (capacity-checked), `load_u8`/`store_u8` (length- and capacity-checked), `array_load/store`, `ring_push/pop`, `map_get/put/has/remove` |
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
- The line table is sorted by code offset; a fault binary-searches it and fills
  `Diagnostic.unit_name`, `line` and `column`. The compiler reserves one hidden
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
| 1 | ~~Exact opcode table and operand encodings~~ | **Resolved** — ABI v1 is `abi::opcodes`: 57 opcodes, one 4-byte opcode word plus up to two immediate words, `i32` jump offsets in instruction words, 8-byte operand slots. The interpreter implements it. |
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

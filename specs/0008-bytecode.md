# 0008: Bytecode format and the compiler/runtime split

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `repl` (in-kernel compile), host language
  tooling
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002 (interpreted applications), 0005 (internal bytecode, no JIT), 0007 (fully
  static placement), 0011 (work stealing), 0018 (floating point), 0019 (source-only
  delivery)
- **Updated:** 2026-11-16

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

### Multi-unit compilation (`stdlib` and `/lib` modules)

An application that says `uses stdlib;` is compiled together with the embedded library unit
(`stdlib/stdlib.ig`, included with `include_str!`) into **one package**. An application or
module that says `uses lib.math;` (spec 0018) compiles the source at the image path
`lib/math.ig` into the same package as one more unit; the rules below apply to it
unchanged — its names are reachable as `math.<name>`, it may import `core`, `syslib`,
`stdlib` and other modules, and it may not declare state, a stream endpoint or `main`:

- **Name resolution.** The application's top-level names stay unqualified; the library's
  are reachable only as `stdlib.<name>`. Inside the library its own names are unqualified,
  and it may import `core` and `syslib` (ADR 0026) — it cannot reference the application
  (no cycle, M4). `syslib` adds no authority the application did not already have (spec
  0003, H4); it lets the UI layout layer draw with the primitives in spec 0009. A
  `stdlib.*` reference without the import, or an unknown `stdlib.*` name, is a compile
  error.
- **Function table.** The embedded library's procedures occupy the first indices, then
  each `/lib` module in discovery order, then the application's; cross-unit calls resolve
  to those indices. The entry is the application's `main`.
- **One instruction section, one constant blob.** Every unit shares the constant pool in
  first-use order, so deterministic output and one artifact hold.
- **Line table.** Each entry carries a unit id: `0` application, `1` embedded `stdlib`
  (reserved even in a package that only imports modules), and `2..=16` for `/lib` modules
  in depth-first `uses` order (`abi::MAX_UNIT_ID`). The verifier accepts unit ids
  `0..=16`. Compile diagnostics name the failing unit and path
  (`lib/math.ig:line:col: ...`), so a broken module is an ordinary `Diagnostic`.
- **Manifest.** It keeps the application's unit name and entry point, plus the stream
  endpoint descriptors in declaration order (name, direction, capacity). Handle `i` is
  written at `state[i * 8]`; no offset is stored in the descriptor.
- **Types.** Type ids are compiler-local per unit. A library or module signature may use
  scalars, buffers, `Status` and `FaultCode`; a record, sum or user enum crossing the unit
  boundary is a compile error until a shared type table exists.
- **Cost.** Every unit is recompiled from source for each application instance that
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
| `map[K,V,N]` | `count: u64` at 0, then `capacity` (next power of two ≥ 2N) slots of `state: u64`, `hash: u64`, key image and value image; FNV-1a hashing (spec 0003) |
| endpoint | 8-byte opaque handle (generation + table index), direction checked by the type checker; endpoint `i` lives at `state[i * 8]` |
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

### Maps

Maps add five opcodes, `map_put`, `map_get`, `map_has`, `map_remove` and
`map_count`, because hashing, probing and backward-shift deletion are one cohesive
algorithm and inlining it at every call site would duplicate the deletion logic. The
map image is `count: u64` at offset 0 and then `capacity` slots at offset 8, each
`state: u64` (0 empty, 1 occupied), `hash: u64`, the key image and the value image.

The opcode word of `map_put`/`map_get`/`map_has`/`map_remove` carries the key kind in
`a` (`int` 0, `bytes` 1, `string` 2) and the value kind in `b` (`scalar` 0, `fixed`
aggregate 1, buffer 2), with `c` reserved. Four immediate words follow: `capacity`,
`count` (the entry limit `N`), `key_size` and `value_size`; `slot_stride` is
`16 + key_size + value_size`. `map_count` has no immediates: it reads the header
count word.

Operands are pushed in reference order and popped in reverse:
`put` is `[map][key][value] -> Status`, `get` is `[map][key][destination] -> Status`,
`has` is `[map][key] -> bool`, `remove` is `[map][key] -> Status` and `count` is
`[map] -> int`. An `int` key and a scalar value are the 8-byte operand itself; a
buffer key, a buffer value and a fixed aggregate value are references (a buffer
reference names its data, with the runtime length at `ref - 8`).

The machine checks that the whole map image lies inside its region, resolves and
hashes the key (FNV-1a), and probes linearly from `hash & (capacity - 1)`. `put`
writes a matching value in place, or inserts into the empty slot the probe stopped
at when `count < N`; at `count = N` it pushes `Full` and writes nothing. `get` copies
the slot's value image through the destination reference, or pushes `NotFound` and
leaves it untouched. `remove` clears the found slot and then walks the cluster,
moving an entry back when its distance from home is greater than the distance from
home to the hole, until it reaches an empty slot; no tombstones exist. Every byte to
be written is checked before the first write, so a failure leaves the map unchanged.

The verifier checks the key and value kinds, that the capacity is a power of two of
at least 2 with `2 * count <= capacity`, non-zero 8-byte-multiple key and value
sizes (buffer images at least 16 bytes), and that the whole image fits the section
cap. A buffer key or value is copied as `[len][data]`, so only its runtime length is
read and written, never its unused capacity.

A whole-map assignment and a `value` map parameter are `copy_refs` with the
compile-time image size, header included. The compiler interns `(key, value, count)`
in one structural map table shared by the application and `stdlib`, so a map crosses
the unit boundary with one id.

### Streams and endpoints

Stream endpoints add four opcodes, `stream_send`, `stream_try_send`, `stream_recv` and
`stream_try_recv`. The handle is an ordinary 8-byte operand loaded from `state`; the
kernel owns the queue and the application never sees its bytes except through the copy
the opcode performs (spec 0006, S2/S6).

Operands are `[handle][data reference] -> Status` for a send and
`[handle][destination reference] -> int` for a receive. `stream_send`/`stream_try_send`
have no immediates. `stream_recv`/`stream_try_recv` carry one `capacity` immediate: the
destination's declared `bytes[N]` capacity, which bounds the write and is checked against
the region. A receive sets the destination's length word to the copied count; a failed
receive leaves the destination untouched.

A blocking opcode that cannot proceed (`WouldBlock` from the host) restores the operand
stack and the program counter and returns `Outcome::Blocked(WaitKind::Stream(handle))`,
so the same instruction is retried after a wake. The `try_*` variants push the staged
status instead: `Status::WouldBlock` (ordinal `1`) for a send and `-1` for a receive. A
closed stream pushes `Status::Closed` (send) or `-2` (receive). Sending a buffer larger
than the capacity pushes `Status::CapacityExceeded` without blocking or copying.

The manifest lists one `Endpoint` descriptor (direction, capacity, name length) plus the
name bytes per endpoint, in declaration order. Handles occupy the first
`8 * endpoint_count` bytes of `state`, in the same order; the loader creates or pairs the
endpoints before the task is placed and writes handle `i` at `state[i * 8]`. There is no
offset field in the descriptor: the state offset is derived from the index, so
`ENDPOINT_SIZE` and `MANIFEST_HEADER_SIZE` stay unchanged. Dynamic loading, waiting and
the load errors are spec 0006, "Dynamic loading".

### `substr` (mixed kinds)

`substr` is one opcode and no new one was needed for mixed-kind slicing. Its opcode
word carries the destination kind in `a` and the source kind in `b` (both
`buffer_kind`: `string` 0, `bytes` 1; `c` reserved), plus one `capacity` immediate
for the destination. Operands are `[destination][source][start][length] -> length`.

The machine resolves both references, checks that the destination length is within
the capacity and that both buffers lie inside their regions, then applies the
destination-kind rule:

- A `string` destination from a `string` source checks both slice boundaries with the
  source's UTF-8 invariant, so a slice that splits a code point — including an empty
  range that starts inside one — pushes `-4` and writes nothing.
- A `string` destination from a `bytes` source validates the copied range as **strict
  UTF-8**: only 1- to 4-byte sequences, no overlong encodings (`C0`/`C1` leads,
  `E0 80..`, `F0 80..`), no surrogates (`ED A0..`), nothing above `U+10FFFF`
  (`F4 90..`, `F5..`), no truncated sequence and no stray continuation byte. This is
  the only instruction that produces a `string` from raw bytes, so the invariant is
  enforced at the single point where it can be created (T3).
- A `bytes` destination copies either source kind raw, with no UTF-8 check (T4).

A negative `start` or `len` and a rejected code-point boundary or invalid byte
sequence all push `-4` (`Status.Invalid`) before any write; a `start` past the source
length pushes `0` and clears the destination; otherwise the copied length is
`min(len, src_len - start, capacity)` and is pushed. The verifier accepts `a` and `b`
in `0..=1` and requires `c = 0`.

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
| Buffers and aggregates | `append` (capacity-checked), `read_input` (raw non-blocking console read, capacity immediate), `load_u8`/`store_u8` (length- and capacity-checked), `find` (byte scan), `substr` (checked slice; `a` = destination kind, `b` = source kind), `format_int` (decimal/hex formatting), `format_f64` (fixed-point float text: `a` = destination kind, a capacity immediate), `format_f64_short` (shortest round-trip float text: same encoding, no width operand), `enum_cast` (range-checked ordinal → enum), `ref_offset` (field reference), `ref_element` (array element reference, index-checked), `copy_refs` (record, sum, array, ring and `case` payload copies), `map_put`/`map_get`/`map_has`/`map_remove`/`map_count` (open-addressing map, layout in the immediates), `stream_send`/`stream_try_send`/`stream_recv`/`stream_try_recv` (kernel byte FIFO; receive carries the destination capacity) |
| Builtins | `call_builtin n` — host-interface and `core` primitives |
| System | `safe` (explicit poll), `trap` (compiler-inserted fault) |

Aggregates are opcodes rather than monomorphised library code because the compiler, not
the runtime, knows each layout; the opcode carries a layout descriptor index.

### Host builtins

`BUILTIN` (opcode `0x50`) carries a `u32` id. The verifier accepts an id only when
`abi::opcodes::builtin::effect` names its operand count, and the interpreter dispatches
on the same table; the machine, not the compiler, owns the runtime checks.

| Id | Name | Operands → results | Notes |
|----|------|--------------------|-------|
| 1 | `write` | 1 → 0 | buffer reference |
| 2 | `clock` | 0 → 1 | nanoseconds |
| 3 | `spawn` | 4 → 1 | path, args, diag ref, id ref |
| 4 | `wait` | 2 → 1 | id, reason ref; may block |
| 5 | `kill` | 1 → 1 | task id |
| 6 | `len` | 1 → 1 | buffer reference |
| 7 | `port_in8` | 1 → 1 | `syslib` |
| 8 | `port_out8` | 2 → 1 | `syslib` |
| 9 | `run` | 4 → 1 | path, args, diag ref, reason ref; may block |
| 10 | `cpu_id` | 0 → 1 | APIC id |
| 11 | `realtime_on` | 0 → 1 | `Status` |
| 12 | `heartbeat` | 0 → 0 | progress claim |
| 13 | `screen_width` | 0 → 1 | `syslib`, pixels or 0 |
| 14 | `screen_height` | 0 → 1 | `syslib`, pixels or 0 |
| 15 | `fill_rect` | 5 → 1 | x, y, w, h, colour; `Status` |
| 16 | `fill_dither` | 5 → 1 | `(X + Y)` checkerboard |
| 17 | `draw_rect` | 5 → 1 | one-pixel outline |
| 18 | `draw_text` | 4 → 1 | x, y, text reference, colour |
| 19 | `get_pixel` | 2 → 1 | `0x00RRGGBB`, 0 outside |
| 20 | `present` | 4 → 1 | x, y, w, h |
| 21 | `clear` | 1 → 1 | colour; fills and presents |
| 22 | `pointer_x` | 0 → 1 | pointer `x` in pixels |
| 23 | `pointer_y` | 0 → 1 | pointer `y` in pixels |
| 24 | `pointer_buttons` | 0 → 1 | bit 0 left, bit 1 right, bit 2 middle |
| 25 | `inject_pointer` | 3 → 1 | dx, dy, buttons; debug-only; `buttons > 0b111` is `Invalid` |
| 26 | `pointer_source` | 0 → 1 | 0 none, 1 PS/2, 2 tablet |
| 27 | `next_event` | 1 → 1 | event record ref; blocks on an empty queue |
| 28 | `poll_event` | 1 → 1 | event record ref; `WouldBlock` when empty |
| 29 | `inject_event` | 1 → 1 | event record ref; debug-only; unknown kind is `Invalid` |
| 30 | `event_drops` | 0 → 1 | newest events dropped since boot |
| 31 | `event_flush` | 0 → 1 | discard the queue, return the count |
| 32 | `window_open` | 6 → 1 | x, y, w, h, title ref, id ref; `Status` |
| 33 | `window_close` | 1 → 1 | window id; owner-only |
| 34 | `window_raise` | 1 → 1 | window id; any task may raise |
| 35 | `window_move` | 3 → 1 | id, x, y; owner-only |
| 36 | `window_frame_rect` | 2 → 1 | id, `Rect` ref; writes on `Ok` |
| 37 | `window_content_rect` | 2 → 1 | id, `Rect` ref; writes on `Ok` |
| 38 | `window_focus` | 0 → 1 | focused window id or 0 |
| 39 | `window_present` | 1 → 1 | window id; presents the visible content; owner-only |
| 40 | `window_info` | 4 → 1 | index, id ref, `Rect` ref, title ref; topmost first; writes on `Ok` |
| 41 | `fs_list` | 5 → 1 | path ref, index, name ref, kind ref, size ref; root-only; writes on `Ok` |
| 42 | `fs_read` | 4 → 1 | path ref, offset, dst ref, read ref; ≤256 bytes, 0 at EOF; writes on `Ok` |
| 43 | `fs_size` | 2 → 1 | path ref, size ref; writes on `Ok` |
| 44 | `terminal_attach` | 1 → 1 | window id; binds the one console terminal to the caller's window |
| 45 | `text_width_prop` | 1 → 1 | text reference; measured proportional advance in pixels, 0 when disabled |
| 46 | `text_height_prop` | 0 → 1 | the proportional line height (12), 0 when disabled |
| 47 | `draw_text_prop` | 4 → 1 | x, y, text reference, colour; proportional glyphs with the 8×8 fallback |
| 48 | `halt` | 0 → 1 | request a machine-wide halt; never returns on a live kernel |
| 49 | `reboot` | 0 → 1 | request a system reset; never returns on a live kernel |
| 50 | `parse_f64` | 2 → 1 | src ref, `var f64` ref; correctly rounded decimal reader; writes on `Ok` |

Graphics builtins add no opcode, so ABI v1 stays at **75** opcodes (spec 0009). The input
builtins (spec 0010), the window builtins (spec 0011), the application file builtins
(spec 0013), `terminal_attach` (spec 0014), the proportional-font builtins (spec
0015), the system-control builtins (spec 0017) and the float parser (spec 0003, builtin 50)
add none either; the interpreter owns the button-mask rule, the path validation, the float
grammar and the operand decoding, the host owns the pointer state, the event queue, the
window table, poll-time routing, the per-window exposes, the read-only ext2 view, the
terminal attachment, the font's glyph cache and the stop/reset path.

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
first-milestone caps (provisional, tuning): 128 KiB of source per unit, 384 KiB per
application (application + `stdlib` + every `/lib` module), 16 MiB of scratch. They bound
memory because memory is statically placed; they
are not a time budget, and compile time is not a design constraint (ADR 0019). The scratch
cap was 1 MiB until M2.4.0, 2 MiB until M2.4.2, 4 MiB until M2.4.5 and 8 MiB until M3.2:
the UI layout, controls, menus and dialogs layers pushed the embedded library over the
earlier caps, and M2.4.5's icons layer plus its bitmap-bearing sample needed 5,386,584 B
for the pair (the library alone is 114,823 B of source and the pair with a minimal
application needs 4,134,600 B). M3.2 added `/lib` modules, raised the arena to 16 MiB and
the combined application source cap to 384 KiB. The per-unit source cap rose from 64 KiB
to 128 KiB in M2.4.2 for the same reason. The kernel's static buffers grow with the arena
(spec 0007); the application-source buffer stays 128 KiB because module sources are copied
into the arena.

The `compiler` crate is `no_std` and allocation-free: it borrows the source, stores tokens,
AST nodes and name tables in the caller's scratch arena, and writes the package into the
caller's code and manifest buffers. It exports the caps as `MAX_SOURCE_BYTES` (128 KiB),
`MAX_ARENA_BYTES` (16 MiB), `MAX_CODE_BYTES` (256 KiB) and `MAX_MANIFEST_BYTES` (4 KiB), and
`compile_into` reports exhaustion of any of them as `Status.CapacityExceeded` with a
`Diagnostic`. The only `unsafe` in the crate is the alignment/initialisation arithmetic of
the bump arena, isolated in `compiler/src/arena.rs`.

### Verification and diagnostics

- The loader runs a bounded linear verifier (B7). It does not re-typecheck the program; the
  compiler is trusted, the verifier protects the kernel from a corrupt image. A structural
  error refuses the package with `Status.Invalid` and a `Diagnostic`.
- The line table is sorted by code offset and names a unit id (0 application, 1 embedded
  `stdlib`, `2..=16` `/lib` modules); a fault binary-searches it for `line` and `column`,
  and the unit id names the
  source unit. The compiler reserves one hidden
  `Diagnostic` slot in `state` for the interpreter to fill before invoking `fault`/`exit`.
- Field validation is per opcode: a reserved field must be zero unless the opcode defines
  it. `substr` is the one buffer opcode with two kind fields — `a` is the destination kind
  and `b` the source kind, each `0..=1` (`string`, `bytes`) — so the verifier accepts both
  and rejects anything else; the capacity immediate must fit a section. `read_input`
  carries a capacity immediate and no kind field: the destination is always `bytes`
  (the compiler enforces it), so `a = b = c = 0`. `format_int`,
  `format_f64` and `format_f64_short` carry one kind field (`a`, `0..=1`), require
  `b = c = 0`, and bound their immediate to a section; `format_f64` and `format_f64_short`
  additionally require a non-zero capacity, because no digit can ever be written into an
  empty destination.
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
| 1 | ~~Exact opcode table and operand encodings~~ | **Resolved** — ABI v1 is `abi::opcodes`: 75 opcodes, one 4-byte opcode word plus up to four immediate words (the `map_*` ops carry the layout), `i32` jump offsets in instruction words, 8-byte operand slots. The interpreter implements it. |
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
| 2026-11-15 | Maps add five opcodes: `map_put`, `map_get`, `map_has`, `map_remove` (opcode word `a` = key kind, `b` = value kind; immediates `capacity`, `count`, `key_size`, `value_size`) and `map_count` (no immediates). The interpreter hashes with FNV-1a, probes linearly and deletes with backward shift; a `put` past the entry limit pushes `Full` and writes nothing. The map image is `count` then `capacity` slots of state/hash/key/value, the structural map table is shared with `stdlib`, and a library signature may carry a map. |
| 2026-11-15 | `substr` gained a source kind: the reserved `b` field is now the source's `buffer_kind` while `a` stays the destination kind, so all four `string`/`bytes` combinations are legal with no new opcode. The verifier accepts `a`/`b` in `0..=1` and `c = 0`; the machine applies the destination-kind rule (boundary check for a `string` source, strict UTF-8 validation for a `bytes` source, raw copy for a `bytes` destination). |
| 2026-11-15 | Builtin `cpu_id` (10): pushes the APIC id of the core executing the call; `Host::cpu_id` defaults to 0, so no opcode and no ABI version change. |
| 2026-11-15 | Builtins `realtime_on` (11; pushes the `Status` ordinal, `Ok` or `Full`) and `heartbeat` (12; no operands, no result). `Host::realtime_on` defaults to `Ok` and `Host::heartbeat` to a no-op, so no opcode and no ABI version change; the scheduler side is spec 0007, ADR 0016. |
| 2026-11-15 | Streams add four opcodes: `stream_send`/`stream_try_send` (`[handle][data] -> Status`, no immediates) and `stream_recv`/`stream_try_recv` (`[handle][destination] -> int`, one destination-capacity immediate). A blocking opcode that would block restores its operands and pc and returns `Blocked(WaitKind::Stream(handle))`; the try variants push `WouldBlock`/`Closed` staged codes. The manifest lists endpoints in declaration order and handle `i` is at `state[i * 8]`, so `Endpoint`/`MANIFEST_HEADER_SIZE` are unchanged. ABI v1 grows from 69 to 73 opcodes. |
| 2026-11-16 | Fixed-point float text adds one opcode: `format_f64` (`a` = destination buffer kind, a non-zero capacity immediate; destination/value/`frac_digits` on the stack, pops an `f64` bit pattern) pushes the byte count, or `-4` for a width above 9 or a magnitude at or above `2^63`, or `-6` when the result does not fit. The specials `nan`/`inf`/`-inf` are written literally. The formatter is pure interpreter code building into a local array, with no host call and no allocation. `stdlib.parse_f64_into` adds no opcode: it is ordinary language code over `core.len`, indexing and `f64` arithmetic. ABI v1 grows from 73 to 74 opcodes. |
| 2026-11-16 | Shortest round-trip float text adds one opcode: `format_f64_short` (`a` = destination buffer kind, a non-zero capacity immediate, `b = c = 0`; destination reference and `f64` bit pattern on the stack, no width operand) pushes the byte count, or `-6` when the result does not fit. The digits are the shortest significant digits that round-trip, printed positionally while `-6 < e10 + 1 <= 21` and in scientific form otherwise; `nan`, `inf`/`-inf` and `0`/`-0` are literal. The search is pure interpreter code over exact base-`10^9` integers (value plus both round-trip interval endpoints), with no host call, no allocation and no data tables: the power-of-five factor is built by repeated multiplication by `5^27`, so the `.rodata` cost is a 72-byte `10^0..=10^8` table and the code is about 9.4 KiB of `.text` at `-O`. The table counted from `abi/src/opcodes.rs` now holds **75** opcodes (74 before this change: the earlier `73 to 74` entry above is the correct count, and the `71 to 72` figure once quoted in `AGENTS.md` was off by two). |
| 2026-11-16 | Graphics builtins add no opcode (spec 0009): ids 13–21 are `screen_width`, `screen_height`, `fill_rect`, `fill_dither`, `draw_rect`, `draw_text`, `get_pixel`, `present` and `clear`, with effects `(0,1)`, `(0,1)`, `(5,1)`, `(5,1)`, `(5,1)`, `(4,1)`, `(2,1)`, `(4,1)` and `(1,1)`. The interpreter validates extents and colours and routes the rest through `Host` with disabled-screen defaults, so ABI v1 stays at **75** opcodes. |
| 2026-11-16 | Input builtins add no opcode (spec 0010): ids 22–25 are `pointer_x`, `pointer_y`, `pointer_buttons` and the debug-only `inject_pointer`, with effects `(0,1)`, `(0,1)`, `(0,1)` and `(3,1)`. The interpreter rejects `buttons > 0b111` before calling the host; ABI v1 stays at **75** opcodes. |
| 2026-11-16 | Builtin id **26** `pointer_source` (0 → 1) reports the active pointer source, 0 none / 1 PS/2 / 2 absolute USB tablet (spec 0010, I27). No opcode is added; ABI v1 stays at **75**. |
| 2026-11-16 | Event builtins (spec 0010, I18/I19): `next_event` (27, 1 → 1; blocks on an empty queue with `Blocked(WaitKind::Input)`), `poll_event` (28, 1 → 1; pushes `WouldBlock`), `inject_event` (29, 1 → 1; debug-only, unknown kind is `Invalid`), `event_drops` (30, 0 → 1) and `event_flush` (31, 0 → 1). The host writes a 40-byte `abi::layout::event` image; no opcode is added and ABI v1 stays at **75**. |
| 2026-11-16 | Window builtins add no opcode (spec 0011, M2.3.1): ids 32–38 are `window_open` (6 → 1), `window_close` (1 → 1), `window_raise` (1 → 1), `window_move` (3 → 1), `window_frame_rect` (2 → 1), `window_content_rect` (2 → 1) and `window_focus` (0 → 1). The interpreter decodes the operands, resolves the title and the out-parameters and writes the 32-byte `abi::layout::rect` image only on `Ok`; the kernel host owns the table and the policy. `Rect` is a predeclared record, so no new type or opcode is needed; ABI v1 stays at **75**. |
| 2026-11-17 | The embedded `stdlib` may import `syslib` (ADR 0026): its first-slice "only `core`" rule was a scope statement, not a security boundary. The library is trusted embedded source compiled into the application's package and `syslib` grants the caller no authority it did not already have (spec 0003, H4). The M2.4.0 UI layout layer uses this for the nine graphics primitives; the type-id rule is unchanged, so records, sums and user enums still may not cross the unit boundary. No opcode or builtin is added; ABI v1 stays at **75**. |
| 2026-11-16 | `window_present` is builtin 39 (1 → 1): pop a window id, present the window's content clipped to its visible parts, push the `Status`. Owner-only and disabled-graphics `Invalid`; no opcode, ABI v1 stays at **75**. `EventKind.Expose` is ordinal 5; routing (`Event.window`) happens in the host at poll time and adds no bytecode. |
| 2026-11-16 | `window_info` is builtin 40 (4 → 1, spec 0012 D15): index, id reference, `Rect` reference and a `var string` title reference. Index `0` is the topmost live window; `NotFound` past the end or with disabled graphics leaves every destination untouched; `Ok` writes the id, the frame image and the title (at most 64 bytes) and pushes `Status::Ok`. The compiler requires the title destination to have capacity 64 and the interpreter bounds-checks the region before the host call. No new opcode; ABI v1 stays at **75**. |
| 2026-11-17 | The M2.4.5 icons and desktop layer adds no opcode or builtin (spec 0003, "UI icons and desktop"): all 16 helpers are ordinary language code over `syslib`, and the 32×32 `array[uint, 32]` bitmap is an ordinary array parameter. The library grew to 114,823 B of source and 84,920 B of code, and the pair with a minimal application needs 4,134,600 B of scratch (the `ui_icons` pair needs 5,386,584 B), so `MAX_ARENA_BYTES` rose from 4 to 8 MiB; the source cap stays 128 KiB and the combined application cap 256 KiB. ABI v1 stays at **75** opcodes. |
| 2026-11-17 | M2.5.1 adds builtins 41–43 (`fs_list` 5 → 1, `fs_read` 4 → 1, `fs_size` 2 → 1) and no opcode; ABI v1 stays at **75**. The machine validates the path through `abi::fs` (the same syntax as `core.spawn`) and the root-only subset, checks the 64-byte name and 256-byte destination regions, stages the host result locally and publishes it with one copy only on `Ok`. `Ext2::list_entry` and `Ext2::read_at` add no allocation and stream the existing data blocks. |
| 2026-11-17 | M2.5.5 adds builtin 44 `terminal_attach` (1 → 1, spec 0014): the machine pops the window id, calls `Host::terminal_attach` and pushes the `Status`. The kernel host supplies the caller's task id, checks ownership and geometry and binds the one machine-wide console terminal. No opcode; ABI v1 stays at **75**. |
| 2026-11-17 | M2.6.1 adds builtins 45–47 (spec 0015): `text_width_prop` (1 → 1), `text_height_prop` (0 → 1) and `draw_text_prop` (4 → 1). The machine resolves the text reference with the same bounds checks as `draw_text` and pushes the measured width, the 12-pixel line height or the drawing status; the host defaults are 0/0/`Invalid` for a disabled screen. The proportional font itself is kernel data with a lazy glyph cache, so no opcode is added and ABI v1 stays at **75**. |
| 2026-11-17 | M2.6.4 adds builtins 48/49 (spec 0017): `halt` (0 → 1) and `reboot` (0 → 1). The machine stages no operands, calls `Host::halt`/`Host::reboot` and pushes the `Status`; the kernel host owns the stop/reset path and a bare host defaults to `Invalid`. No opcode; ABI v1 stays at **75**. |
| 2026-11-17 | Correctly rounded float parsing adds builtin 50 `parse_f64` (2 → 1, spec 0003 "Floating-point text"; Q41): a source buffer reference and a `var f64` reference. The machine copies nothing: it takes the source bytes, runs the spec grammar and `core`'s `f64: FromStr` (both in `interpreter/src/f64parse.rs`) and writes the bit pattern through the out-parameter only on `Ok`, otherwise pushing `Invalid`. There is no `Host` method and no opcode; ABI v1 stays at **75**. |
| 2026-11-17 | Q44 resolved (ADR 0037): the `read_line` opcode (`0x42`) becomes `read_input` in the same slot — capacity immediate, a `var bytes` destination reference on the stack, a raw non-blocking console read that pushes the copied count (`0` when empty) and always publishes the destination length. The kind field stays zero (the compiler requires `bytes`). This is a semantic change inside ABI v1, not a new opcode: the table stays at **75**. |

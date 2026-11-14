# 0003: Language and standard library

- **Status:** Accepted (design specified; implementation pending)
- **Scope:** application language, type system, memory model, standard library
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002, 0004, 0005, 0007, 0010, 0011, 0016, 0018
- **Updated:** 2026-11-14

## Context

The language is the system's real ABI. Every application, `stdlib`, and the host-side
tooling are written against it, so a semantic mistake here is expensive and propagates
everywhere. It is a separate workstream: the compiler and runtime run on the host before
the kernel boots (vision R7), behind the host interface.

The language is **Pascal-like**: `begin`/`end`, `var` sections, `procedure`/`function`,
`unit`/`uses`, `:=` for assignment, `=` for equality. It is not a dialect of Pascal; it is
a small, statically typed language with Pascal's shape and this system's memory model.

Two constraints dominate the design:

1. **Nothing is placed at runtime** (ADR 0007). Every value is inline or a call-scoped
   reference; every capacity is a compile-time constant; every construction writes into
   caller-provided storage.
2. **The type system carries the invariants** that hardware cannot enforce: capacity,
   UTF-8 validity, ownership, and the difference between a native `core` primitive and
   interpreted `stdlib` code.

## Inherited constraints

- **L1 (MUST):** The language is statically typed. Types make memory layouts and
  invariants explicit and checkable.
- **L2 (MUST):** No implicit allocation. Every piece of memory a program uses is declared,
  part of a fixed-size value, or a call-scoped reference to one.
- **L3 (MUST):** No dynamic growth. Collections have declared maximum sizes; operations
  that would exceed them fail explicitly.
- **L4 (MUST):** Safe code has no raw pointers and no pointer arithmetic. References are
  handles or region-relative offsets, validated by the runtime. Raw addresses exist only
  behind `syslib` handles.
- **L5 (MUST):** Fixed-capacity byte FIFO streams are the inter-application
  communication primitive; typed views are a `stdlib` layer over them.
- **L6 (MUST):** Destruction is deterministic. No tracing garbage collector.
- **L7 (SHOULD):** The language must remain usable for small programs with one screenful
  of code; ceremony is a cost.
- **L8 (MUST):** No runtime placement whatsoever (ADR 0007). Constructors write into
  caller-provided storage; capacities are compile-time constants; exceeding a capacity is
  an explicit failure.
- **L9 (MUST):** Buffers are declared with a fixed capacity (`string name[128]`), carry a
  runtime length and a compile-time capacity, and are never resized. Assignment is the
  construction form: `dst := a + b` writes the concatenation into `dst`.
- **L10 (MUST):** Text is a byte buffer with an explicit length; no implicit NUL
  terminator, so a 128+128 concatenation fits exactly in 256. Text and raw bytes are
  distinct types (`string` and `bytes`, below).
- **L11 (MUST):** Mutable aliasing is forbidden. There is no view construction and no
  reference value may be stored. Parameter passing is limited to value copies and
  call-scoped `const`/`var` borrows.
- **L12 (MUST):** An application may define two optional top-level handlers:
  `fault(code, diag)` and `exit(reason, diag)`. `exit` runs at every termination for
  teardown; `fault` runs on a fault and decides between termination and one recovery
  (ADR 0010).

## Lexical and syntactic shape

- Source is UTF-8 text. Identifiers are case-sensitive; keywords are lowercase.
- Comments: `{ ... }` and `// ...`.
- Statements end with `;`. Blocks are `begin ... end`.
- Assignment `:=`; equality `=`; inequality `<>`; comparisons `<`, `<=`, `>`, `>=`.
- Boolean operators are `and`, `or`, `not`; they take `bool` operands and short-circuit.
  Integer bitwise operations are named functions, not operators: `core.bit_and`,
  `core.bit_or`, `core.bit_xor`, `core.shl`, `core.shr`.
- Integer operators are `+`, `-`, `*`, `div`, `mod`. Integer overflow is a fault, never
  wraparound. Float operands use `+`, `-`, `*` and `/`, following IEEE-754.
- Precedence, highest first: unary `not` and `-`; `*`, `/`, `div`, `mod`; `+`, `-`;
  relational `<`, `<=`, `>`, `>=`; equality `=` and `<>`; `and`; `or`. Parentheses
  override. `and` and `or` bind looser than comparisons, so `a > 0 and b > 0` means what
  it reads like.
- A shift count outside `0..63` is `fault(IllegalOperation)`.
- String literals use double quotes with escapes (`\n`, `\t`, `\\`, `\"`, `\xNN`). A
  literal is typed by its destination: a `string` destination validates UTF-8 at compile
  time; a `bytes` destination accepts raw bytes.
- A unit is one source file:

```text
unit hello;

uses core, stdlib;

const
  MaxName = 64;

var
  name: string[MaxName];
  count: int;

procedure main(args: const string[1024]);
begin
  name := "igis";             // construction, not allocation
  count := 0;
  while count < 3 do
  begin
    core.write(name);
    core.write("\n");
    count := count + 1;
  end;
end;
```

## Types

### Scalars

| Type | Width | Notes |
|------|-------|-------|
| `bool` | 1 byte | `true` / `false` |
| `byte` | 1 byte | unsigned |
| `int` | 8 bytes | signed, two's complement |
| `uint` | 8 bytes | unsigned |
| `f64` | 8 bytes | IEEE-754 binary64 (ADR 0018) |

No implicit numeric conversions, including between the integer types and `f64`.
Conversions are written as a type applied to a value:

```text
f64(x)   int(x)   uint(x)   byte(x)
```

- `f64(x)` from an integer is ordinary IEEE conversion. Values that need more than 53 bits
  of mantissa round; that is not a fault.
- `int(x)`, `uint(x)` and `byte(x)` truncate toward zero and fault `Overflow` when the
  value is `NaN` or does not fit the destination.
- `string`/`bytes` conversion is not a cast. It can fail as data, so it is the checked
  `stdlib.utf8_decode_into` function returning `Status` (T3).

### Floating point

- `f64` is the only float type in the first milestone. `f32` is not included (ADR 0018).
- Arithmetic follows IEEE-754: `+`, `-`, `*`, `/`, with `Inf` and `NaN` as ordinary
  results. Float division by zero produces `Inf`/`NaN`; it is not a fault. Integer
  division by zero remains `fault(DivideByZero)`.
- Comparisons are IEEE: `NaN` is unequal to everything, including itself; `<`, `<=`, `>`
  and `>=` are false when an operand is `NaN`; `<>` is true. There is no literal for `NaN`
  or infinity; arithmetic produces them.
- Literals contain a decimal point or an exponent: `1.0`, `2.5e10`, `1e-3`. Hexadecimal
  float literals are not part of the first milestone.
- Rounding is round-to-nearest-even with exceptions masked; the compiler emits no fused
  multiply-add and never uses x87 extended precision. The bits a computation produces are
  therefore the same on any x86_64 core.
- Text conversion is fixed-point in the first milestone:
  `core.format_f64_into(dst, value, frac_digits)` and `stdlib.parse_f64_into(text, var
  value)`, which parses decimal text and returns `Status`. Shortest round-trip formatting
  and full IEEE text parsing are deferred.
- Transcendental and other math helpers (`sqrt`, `sin`, ...) live in `stdlib` and are
  written in the language; a helper moves to `core` only when measurement justifies it.
- `f64` may appear in records, sum types, arrays, rings and map values. It is not a map
  key.

### Fixed-capacity buffers

`bytes[N]` and `string[N]` share one representation: N bytes of storage, a runtime length
`0 <= len <= N`, and capacity N as a compile-time constant.

- `bytes[N]` is raw data: file contents, protocol frames, identifiers.
- `string[N]` is text with the invariant **valid UTF-8**. The invariant is established at
  construction, not re-checked on every operation.

Rules:

- **T1 (MUST):** A `string` literal is validated at compile time.
- **T2 (MUST):** Concatenation of two valid `string` values is valid UTF-8; the capacity
  proof of `0004-memory-model.md` still applies byte-for-byte.
- **T3 (MUST):** Converting raw bytes to `string` is explicit and checked
  (`stdlib.utf8_decode_into`), and reports an error instead of producing an invalid
  string.
- **T4 (MUST):** Slicing a `string` at an arbitrary byte offset yields `bytes`, not
  `string`. A checked `stdlib.substr_into` returns `string` only if both boundaries fall
  on code-point boundaries.
- **T5 (MUST):** Capacity is measured in bytes, never characters. `string[128]` holds at
  most 128 bytes of UTF-8.

### Indexing, comparison and length

- **Indexing.** `buf[index]` yields one `byte`. The index is `int`, `uint` or `byte`; a
  negative or out-of-range index is a fault, never a clamp. For `string` the result is the
  raw byte at that offset, not a code point (T4), so indexing implies no UTF-8 boundary and
  is not a text operation.
- **Writes are `bytes`-only.** `bytes[i] := expr` stores one byte and `expr` must be `byte`
  (`byte(x)` narrows explicitly). `string[i] := expr` is a compile error: a single-byte
  write could split a multi-byte sequence and break the invariant. Text changes by
  construction (`dst := ...`) or, later, by `stdlib` helpers that preserve UTF-8.
- **Bounds.** A read is checked against the runtime length and faults `OutOfBounds` at or
  past it. A write is checked against the declared capacity and faults `CapacityExceeded`
  at or past it.
- **Writing extends the length.** An index at or past the current length is allowed: the gap
  between the old length and the index is filled with zero bytes and the length becomes
  `index + 1`. The buffer never grows past its capacity. Writing inside the length replaces
  the byte and leaves the length alone. (This is a checked append without a second
  primitive; it is the one semantic worth remembering about buffer writes.)
- **Comparison.** `=` and `<>` accept two buffers of the same kind (`string` with `string`,
  `bytes` with `bytes`) and any capacities: they compare the runtime lengths, then the
  bytes. A literal takes its kind from the other operand, so `line = "help"` works. Mixed
  `string`/`bytes` comparison is a compile error, and ordering (`<`, `<=`, `>`, `>=`) is not
  defined for buffers.
- **Length.** `core.len(buf) -> uint` returns the runtime length for either kind. One
  builtin serves both because the length word is part of the shared representation.

### Slicing and search (staged)

- **T4 staging.** `core.substr_into(dst, src, start: int, len: int) -> int` is the checked
  slice from T4, temporarily in `core` because `stdlib` does not exist yet. It moves to
  `stdlib.substr_into` once the language can express it; the semantics do not change.
- `dst` must be a writable (`var`) buffer and `src` any buffer, including a literal. Both
  must have the same kind: `string` from `string`, `bytes` from `bytes`; mixing is a
  compile error.
- Negative `start` or `len` returns `-4` (`Status.Invalid`) and writes nothing.
  `start > len(src)` returns 0 and clears the destination. Otherwise the copied length is
  `min(len, len(src) - start, capacity(dst))`, the destination's runtime length becomes
  that value, and the result is the copied length.
- For `string`, both `start` and `start + copied` must fall on UTF-8 code-point boundaries
  in `src`; otherwise the result is `-4` and nothing is written. `bytes` copies raw data
  and needs no check.
- **`core.find(buf, value: byte) -> int`** returns the first index of `value`, or `-1`.
  It scans `0..len` only and serves both kinds; a `string` is searched byte-wise, not by
  code point.

### Integer formatting (staged)

- **`core.format_int_into(dst: var string[N], value: int, base: uint, width: uint) -> int`**
  writes a signed integer as text and returns the byte count. `dst` is a writable
  `string` or `bytes` buffer (the output is ASCII). `base` must be 10 (decimal) or 16
  (lowercase hexadecimal); any other base returns `-4` (`Status.Invalid`) and writes
  nothing.
- A negative value gets a leading `-`, then the magnitude. A non-zero `width` zero-pads
  the digits to at least that many characters; the sign is additional. A value with more
  digits than `width` keeps all of them.
- The result must fit `N`. If it does not, nothing is written, the destination length is
  left untouched, and the result is `-6` (`Status.CapacityExceeded`). Otherwise the
  runtime length becomes the number of bytes written.
- This is a pure buffer operation in the interpreter, not a host call. It is a staged
  `core` primitive because `stdlib` formatting does not exist yet; the semantics do not
  change when it moves (spec 0008, `format_int`).

### Compound and parametric types

- **Records:** `type Point = record x, y: int; end;`
- **Sum types:** `type Token = Ident(name: string[32]) | Number(value: int) | Eof;`
  Sized by the largest variant; no boxing, no indirect variant.
- **Enums:** `type Status = (Ok, WouldBlock, Closed, NotFound, Invalid, Full, CapacityExceeded, IoError);`
- **Arrays:** `array[T, N]` — fixed, index `0..N-1`, value semantics.
- **Rings:** `ring[T, N]` — fixed-capacity FIFO with `push`/`pop`/`full`/`empty`.
- **Maps:** `map[K, V, N]` — at most N entries, open addressing, no growth. `K` is
  restricted to `int`, `bytes[N]` or `string[N]`.
- **Stream endpoints:** `in_stream[N]` (receive only) and `out_stream[N]` (send only),
  N bytes of queue capacity. Distinct nominal types, so a receive endpoint cannot be sent
  to. The declaration's variable name is the stream name: at most 31 bytes matching
  `[a-z][a-z0-9_]*`, case-sensitive, and unique across the whole application. `N` is a
  compile-time constant in `1..=65536`; the loader refuses an application whose declared
  streams the kernel buffer pool cannot cover.
- **Task ids:** `TaskId` (an `int`), global, monotonically increasing, never reused;
  zero is invalid. The application stores the id in its own `state` only if it wants to
  `wait` or `kill` the child later; there is no per-task handle table.

There are **no user-defined generics**. The parametric types above are built in; the
compiler knows their layout and operations. Generic algorithms are either built-ins
(`ring`, `map`) or written per type in `stdlib`.

### Capacity compatibility

Capacities are part of the type: `string[128]` and `string[256]` are distinct (D11), and
so are `array[int, 4]` and `array[int, 8]`. Two rules make this bearable:

- **Variables and fields:** exact type. Assignment between two variables of different
  capacities is a compile error.
- **Construction:** assigning a literal or a construction expression into a buffer is a
  write into the destination, not a type assignment; the destination's capacity is what
  matters. `name := "igis"` into a `string[64]` is valid; `s64 := s32` is not.
- **Parameters:** the **at-least-N** rule. A parameter declared `const string[128]` or
  `var string[128]` accepts any `string[M]` with `M >= 128`; the callee may rely on at
  least 128 bytes and reads the runtime length. The reverse never holds. `const string[0]`
  is the idiomatic "any read-only string"; `var string[0]` is useless and rejected.

The callee's capacity proofs are conservative: inside the callee, `cap(dst)` is the
declared N, not the caller's actual capacity. This is sound and needs no hidden metadata.

## Values, parameters and construction

### No first-class slices

There is no slice value. A buffer is an inline value; a sub-range exists only as a
call-scoped reference:

- `value` (default) — the callee gets a copy.
- `const` — read-only borrow of the caller's buffer, valid for the call.
- `var` — writable borrow of the caller's buffer, valid for the call.

A `const` or `var` reference cannot be stored, returned, or captured. There is no view
construction and no lifetime system, because there is no reference that outlives the
expression that created it.

### Destination construction

`dst := a + b` is not an expression producing a temporary; it is a copy whose destination
the compiler knows statically:

```
fullname := name + surname
      ↓
concat_into(dst = fullname, a = name, b = surname)
```

- No temporaries: `a + b + c` lowers to sequential appends into `dst`.
- Static proof first: if `cap(dst) >= cap(a) + cap(b)`, no runtime check is emitted.
  `string[256] = string[128] + string[128]` is exactly this case.
- Otherwise a runtime length check precedes the write; failure is
  `fault(CapacityExceeded)`.
- The destination must not overlap an operand. The statically visible case (`s := s + t`)
  is a compile error; a single-expression non-aliasing rule covers reference cases.

### Locals and the stack

Locals live in the current frame on the task's declared stack, including large buffers.
The compiler computes the worst-case frame size; the manifest declares the stack that must
hold it. Recursion is bounded by the stack (S7) and a guard page catches overflow.

## Text and bytes

`bytes` and `string` are not two representations; they are one representation with two
invariants. This is deliberate: the type system answers "is this valid UTF-8?" at the
point where the value is created, instead of at every call site.

- Console, streams and files are byte-oriented; text applications convert explicitly.
- Shared byte operations (`len`, compare, find) are defined on both.
- UTF-8-specific operations (`codepoints`, `chars`, `substr_into`) exist only on `string`.
  The staged `core.substr_into` also accepts a `bytes` destination so raw slices have a
  checked copy before `stdlib` exists; only the `string` form checks boundaries.
- Comparison is byte-wise (`bytes`) or code-point-wise (`string`), both lexicographic.
  In this slice equality compares bytes, which is the same relation for valid UTF-8.

## Collections

| Type | Operations | Notes |
|------|------------|-------|
| `array[T, N]` | index, `len`, copy | fixed size, no append |
| `ring[T, N]` | `push`, `pop`, `peek`, `full`, `empty` | FIFO, fixed capacity; `push` on full is a failure |
| `map[K, V, N]` | `get`, `put`, `has`, `remove`, `count` | open addressing, fixed N entries; `put` on full fails |

All three are value types with deterministic layout. Iteration order of `map` is not
specified and must not be relied on.

`map` hashes with FNV-1a (64-bit) over the key's bytes: the eight little-endian bytes of
an `int`, or the bytes of a `bytes[N]`/`string[N]` key up to its runtime length. The table
size is the next power of two that is at least twice N, probing is linear, and deletion is
backward-shift. There is no random seed, so the hash is deterministic per build. A
deliberately bad key set can degrade lookup to O(N); the system is single-user, N is
small, and no adversary is assumed.

## Errors

Fatal conditions are faults and follow the `fault`/`exit` model (ADR 0010). Non-fatal
conditions use `Status`:

```text
type Status = (Ok, WouldBlock, Closed, NotFound, Invalid, Full, CapacityExceeded, IoError);
```

- **E1 (MUST):** A fallible operation returns `Status` and writes its result through a
  `var` out-parameter.
- **E2 (MUST):** The compiler warns when the `Status` returned by a call is discarded
  without being compared to `Ok`.
- **E3 (MUST):** The language has no exceptions, no unwinding and no hidden error values.

Example:

```text
var
  st: Status;
  n: int;
st := stdlib.parse_int(input, var n);
if st <> Ok then
  core.write("not a number\n");
```

## Modules

- **M1 (MUST):** One source file is one `unit`.
- **M2 (MUST):** A unit imports with `uses core, stdlib;`. `stdlib` and `syslib` are
  always named explicitly; there is no implicit prelude beyond `core`'s basic types.
- **M3 (MUST):** The three roots are distinct namespaces: `core.*` (native builtins),
  `stdlib.*` (written in the language), `syslib.*` (hardware). The namespace tells the
  reader where the code and the cost come from.
- **M4 (MUST):** Import cycles are a compile error.
- **M5 (MUST):** Every top-level declaration is visible to importers. The language has no
  `private`, `protected` or `public` modifier; visibility is not part of the type system.

### Entry point

- **EP1 (MUST):** The entry point is `procedure main(args: const string[1024]);`. The
  loader passes the request's argument; an empty string if none.
- **EP2 (MUST):** `args` is read-only. To modify or split it, the application copies it
  into its own buffer.
- **EP3 (MUST):** Exactly one `main` exists per application, in the unit named by the
  manifest.

## Concurrency

- **C1 (MUST):** An application is single-threaded. There are no threads, no `async`, and
  no shared-memory concurrency. `spawn` starts a *separate application instance* with its
  own footprint; it does not share the caller's regions.
- **C2 (MUST):** Communication is through `in_stream`/`out_stream` endpoints declared at
  unit level. The variable name is the stream name recorded in the manifest. Stream names
  are application-wide identifiers and must be unique across every unit of the application
  (`0006-streams.md`, S11).
- **C3 (MUST):** `send` and `receive` block by default; `try_send` and `try_receive`
  return `WouldBlock` through `Status`.
- **C4 (MUST):** `yield()` is the only explicit scheduling primitive. It is a hint, not a
  correctness requirement: the quantum already releases the task.
- **C5 (MUST):** Sends are atomic and bounded by the endpoint capacity; the language does
  not expose partial sends.
- **C6 (MUST):** A task may call `core.realtime_on()` once to enter the realtime class and
  must then call `core.heartbeat()` at least once per second of non-blocked time. There is
  no way back to the normal class. The model is soft realtime (ADR 0016).
- **C7 (MUST):** `core.run(path, args, var diag, var reason)` blocks until the child
  terminates. `core.spawn(path, args, var diag, var id)` reserves a global `TaskId` and
  returns immediately; `core.wait(id, var reason)` blocks if the task is live and returns
  its exit reason; `core.kill(id)` requests termination and does not wait. The request is
  handled by the kernel REPL service (ADR 0017). `diag` is filled on a compile error and
  cleared otherwise. `path` is at most 256 bytes and is interpreted relative to the image
  root (`0005-host-interface.md`). `kill(id)` on a queued request cancels it and records
  `Killed`; on a live task it requests termination at the next safe point; on an unknown or
  finished id it returns `Status.NotFound`.
- **C8 (MUST):** A spawned child is a separate application instance with its own regions.
  It shares no memory with the parent; communication is through streams.
- **C9 (MUST):** The argument passed to `run`/`spawn` is a fixed `string[1024]`, delivered
  as `main(args: const string[1024])`. Splitting, quoting and interpreting it is the
  child's decision; the kernel never parses it. An empty string means no arguments.

## Hardware access (`syslib`)

Raw pointers do not exist in the language. Hardware is reached through typed handles:

```text
type Port = ...;            // opaque
type MmioRegion = ...;      // opaque handle to a mapped physical range

function syslib.port_in8(port: Port): byte;
procedure syslib.port_out8(port: Port; value: byte);

function syslib.mmio_map(phys: uint; len: uint; var region: MmioRegion): Status;
function syslib.mmio_read32(region: MmioRegion; offset: uint): uint;
procedure syslib.mmio_write32(region: MmioRegion; offset: uint; value: uint);
```

- **H1 (MUST):** `syslib` handles are opaque; an application cannot construct one from an
  integer.
- **H2 (MUST):** Offset and bounds checks apply to `MmioRegion` like any other region.
- **H3 (MUST):** DMA buffers are `syslib`-owned; an application sees a handle and copies
  data through typed accessors (vision: kernel/syslib ownership).
- **H4 (MUST):** There is no capability check. Every application may call every `syslib`
  function (vision R14).

**Staged port handle (Q45).** The first `syslib` slice implements only 8-bit port I/O and
stages `Port` as a plain `uint`, not an opaque handle:

```text
syslib.port_in8(port: uint) -> uint;
syslib.port_out8(port: uint, value: uint) -> int;
```

`port_in8` returns the byte as a `uint`; the low 16 bits of `port` are the x86 port number.
`port_out8` returns `0`, or `-4` (`Status.Invalid`) without touching the port when
`value > 255`. `Port`, `MmioRegion` and the `mmio_*`/DMA primitives above remain the target
design; `syslib.*` names other than these two are compile errors for now. There is still no
capability check (H4).

## Constants and manifests

`const` declarations are compile-time expressions over literals, other constants, and the
arithmetic and comparison operators. They may size types, arrays and manifests. There are
no macros,
no comptime code generation, and no reflection.

```text
const
  LineMax = 256;
  BufMax  = LineMax * 2;
```

The manifest is **generated by the compiler**, never written by hand. It records:

- unit name, entry point and ABI version;
- code, `state` and stack sizes;
- declared stream endpoints: name, direction, capacity;
- the capacity profile (build parameters) used.

The loader validates the manifest and refuses an application whose footprint exceeds the
quota. Because the compiler computes the footprint from the same capacities it compiles
against, source and manifest cannot drift.

## Handlers and diagnostics

```text
procedure fault(code: FaultCode; const diag: Diagnostic);
procedure exit(reason: ExitReason; const diag: Diagnostic);
```

Both are optional top-level procedures.

**Fault path.** A fault abandons all frames back to main level and invokes `fault`. It may
`return` (the application terminates, `exit` runs) or `goto` a **validated main-level
recovery label** (the task resumes there). At most one recovery per task; a second fault is
terminal and `fault` is not invoked again. `BudgetExceeded` is delivered but recovery is
rejected. Recovery does not reset the budget.

**Exit path.** `exit` runs at every termination — normal return, unrecovered fault, or
kernel-forced end — and never resumes application code. It runs under a small emergency
budget; if it faults or overruns it is abandoned and the task is reclaimed. If no `exit`
handler exists, the task ends silently.

Both run on the current core's kernel stack, because the task stack may have overflowed.
Labels and loops inside the handlers are allowed. A recovery label is declared at unit
level and is validated by the compiler and the interpreter: no nested function interiors,
no return to the faulting instruction, no other application.

**Diagnostics.** `Diagnostic` is a caller-owned record filled by the kernel or the
compiler; the application only reads it:

```text
type Diagnostic = record
  unit_name: string[32];
  message: string[96];   // short text, truncated if it does not fit
  line: int;
  column: int;
  handle: int;      // region handle involved, or -1
  offset: int;      // offset involved, or -1
  length: int;      // length involved, or -1
  task_id: int;
end;
```

The same record carries compile diagnostics. `core.run` and `core.spawn` take a
`var diag: Diagnostic` out-parameter: on `Status.Invalid` it holds the failing unit, line,
column and message; on any other status it is cleared. The compiler writes into the
caller's buffer, so there is no global error slot and no diagnostic stream.

There is no stack trace in the first milestone. Frames carry no debug metadata, and a trace
would need an unwind table in the manifest; `Diagnostic` plus the serial log is the
observability budget.

**Fault codes.** `fault` receives one of:

| Code | Meaning |
|------|---------|
| `GenericFault` | Unclassified interpreter fault |
| `OutOfBounds` | Access outside an owned region |
| `CapacityExceeded` | Buffer or collection operation would exceed capacity |
| `Overflow` | Integer arithmetic overflow, or a float value that does not fit an integer conversion |
| `DivideByZero` | Integer division by zero |
| `StackOverflow` | Guard-page fault |
| `BudgetExceeded` | Watchdog termination (recovery rejected) |
| `WatchdogTimeout` | Realtime heartbeat missed (recovery rejected) |
| `IllegalOperation` | Invalid opcode or type operation |
| `StreamError` | Invalid or closed endpoint |
| `IoError` | Loader or device failure |
| `ExitFault` | Terminal reason recorded when `exit` itself failed; never delivered to `fault` |

`exit` receives `ExitReason`, which is `Ok`, `Killed`, or `Faulted(code: FaultCode)`.
`Killed` is recorded when `core.kill(id)` ends a task, including a request cancelled before
its task was created.

## Standard library

`core` (native, Rust) provides the primitives that must be fast or that touch the host
interface: console read/write, `yield`, stream send/receive/try, monotonic clock,
`sleep_until`, realtime promotion and `heartbeat`, `run`/`spawn`/`wait`/`kill`, byte and
string copy/compare/find, integer and `f64` math, formatting primitives (integer and
fixed-point float), and the checked conversions. `stdlib` (written in the language)
provides arrays, rings, maps, UTF-8 helpers, parsing (including decimal `f64`), formatting,
and typed stream views. `syslib` (native, Rust) provides ports, MMIO handles and future
block/DMA access.

A function moves from `stdlib` to `core` only when measurement justifies it; the namespace
is the signal.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Index outside `array`/`ring` | `fault(OutOfBounds)`; recover once or terminate |
| Buffer read at or past the runtime length | `fault(OutOfBounds)` |
| Buffer write at or past the declared capacity | `fault(CapacityExceeded)`; never a partial write |
| Buffer index expression that is not `int`/`uint`/`byte` | Compile error |
| `string[i] := expr` | Compile error (`string` is read-only by index) |
| Buffers of different kinds compared (`string` = `bytes`) | Compile error |
| `string`/`bytes` write past capacity | `fault(CapacityExceeded)`; no truncation, no growth |
| `ring` push on full, `map` put on full | `Status.Full` (non-fatal) or explicit failure, never growth |
| Invalid UTF-8 conversion | `Status.Invalid`; no invalid `string` can be constructed |
| Integer arithmetic overflow | `fault(Overflow)` |
| Integer division by zero | `fault(DivideByZero)` |
| Float division by zero | `Inf`/`NaN` per IEEE-754; not a fault |
| `int(f)`, `uint(f)` or `byte(f)` with `NaN` or out of range | `fault(Overflow)` |
| `f64` used as a map key | Compile error |
| Shift count outside `0..63` | `fault(IllegalOperation)` |
| Duplicate or malformed stream name | Compile error |
| Stream capacity outside `1..=65536` | Compile error |
| Path longer than 256 bytes | `Status.CapacityExceeded` |
| Recursion past the declared stack | Guard-page fault; `fault(StackOverflow)` |
| Send on a receive endpoint | Compile error (distinct types) |
| Send on a closed endpoint | `Status.Closed` |
| Realtime heartbeat missed | `fault(WatchdogTimeout)`; recovery rejected |
| `run`/`spawn` path not found | `Status.NotFound`; `diag` cleared |
| Argument longer than 1024 bytes | `Status.CapacityExceeded` |
| `run`/`spawn` compile error | `Status.Invalid`; `diag` filled; no child |
| `wait` on an unknown or evicted id | `Status.NotFound` |
| `kill` on an unknown or terminated id | `Status.NotFound` |
| `kill` on a live child | Terminates at the next safe point; `exit(Killed)` runs |
| `kill` on a queued request | Cancelled; `ExitReason.Killed` recorded; no child created |
| Discarded `Status` | Compile warning |
| Import cycle | Compile error |
| Capacity mismatch in assignment | Compile error |
| `s := s + t` | Compile error (aliasing) |

## Open questions

| # | Question | Why it matters |
|---|----------|----------------|
| 1 | ~~Floating point: `f32`/`f64` in the first milestone, or integer-only?~~ | **Resolved** — `f64` only, IEEE-754, with a per-task FXSAVE region (ADR 0018); `f32` deferred. |
| 2 | ~~Unit visibility: are all top-level declarations public, or is a `private` section needed?~~ | **Resolved** — every top-level declaration is public; the language has no visibility modifiers (M5). |
| 3 | ~~Explicit type conversion syntax and narrowing rules~~ | **Resolved** — `T(x)` casts for numeric conversions, checked; `string`/`bytes` conversion is a `Status`-returning `stdlib` function. |
| 4 | ~~Map hash and probe details~~ | **Resolved** — FNV-1a 64, power-of-two table of at least 2N, linear probing, backward-shift deletion. |
| 5 | ~~Whether `fault` may inspect more than the `Diagnostic` record~~ | **Resolved** — no; `Diagnostic` only, no stack trace in the first milestone. |
| 6 | ~~Exact precedence table and whether `and`/`or` accept integer operands~~ | **Resolved** — comparisons bind tighter than `and`/`or`; both are boolean-only; bitwise operations are `core.bit_*` functions. |
| 7 | ~~Bytecode format and the compiler/runtime split~~ | **Moved** — specified in `0008-bytecode.md` (to be written). |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-03 | Created as a constraints-only seed. |
| 2026-11-12 | Full design pass: Pascal-like syntax; nominal types; built-in parametric collections; `bytes`/`string` split; `Status` errors; `core`/`stdlib`/`syslib` namespaces; streams plus `yield`; typed `syslib` handles; `const` expressions; compiler-generated manifests; at-least-N parameters; diagnostics in `fault`/`exit`. |
| 2026-11-12 | Added the realtime class and `WatchdogTimeout`: one-way `core.realtime_on()`, mandatory `core.heartbeat()` (ADR 0016). |
| 2026-11-12 | Added task spawning and `run`/`spawn`/`wait`/`kill`; `spawn` starts a separate application instance, not a thread (ADR 0017). |
| 2026-11-12 | Added the fixed `string[1024]` argument, delivered as `main(args)` (ADR 0017 addendum). |
| 2026-11-12 | Replaced task handles with global `TaskId`s; `spawn` returns an id, `wait`/`kill` take one; no per-task handle table (ADR 0017 addendum 2). |
| 2026-11-13 | `Diagnostic` gained a `message: string[96]` field and is caller-owned; `core.run`/`core.spawn` take a `var diag` out-parameter filled on compile errors (spec 0005, whose OQ 7 is now closed). |
| 2026-11-13 | Stream endpoint names are application-wide unique (spec 0006, S11); `stdlib` framing defaults to a length prefix with newline-delimited helpers for text. |
| 2026-11-13 | Floating point added to the first milestone: `f64` only, IEEE-754, per-task 512-byte FXSAVE region (ADR 0018); explicit `T(x)` casts, IEEE float semantics; every top-level declaration is public with no visibility modifiers; FNV-1a maps with linear probing; comparisons bind tighter than `and`/`or` and bitwise operations are `core` functions; `Diagnostic` only, no stack trace; stream names are `string[32]` and capacities are `1..=65536`; `run`/`spawn` paths are limited to 256 bytes. |
| 2026-11-13 | `ExitReason` gained `Killed`; `kill` cancels a queued request and runs `exit(Killed)` on a live task; request paths are validated as relative with no `..` and must name regular files. |
| 2026-11-13 | Buffer indexing, comparison and length: `buf[i]` yields a `byte` (raw for `string`, T4); writes are `bytes`-only and extend the length to `index + 1`, zero-filling the gap; reads check the runtime length and writes the declared capacity; `=`/`<>` compare buffers of the same kind across capacities; `core.len(buf) -> uint`. |
| 2026-11-14 | Staged slicing and search: `core.substr_into(dst, src, start, len) -> int` (same kind, negative → `-4`, past-end → 0, clamped copy, UTF-8 boundary check for `string`) and `core.find(buf, byte) -> int` (first index or `-1`, `0..len`, both kinds). Both are `core` opcodes until `stdlib.substr_into` exists (T4 staging). |
| 2026-11-14 | Staged integer formatting: `core.format_int_into(dst, value, base, width) -> int` (decimal/hex, negative sign, zero padding, exact capacity or `-6`, bad base `-4`). First `syslib` slice: `port_in8`/`port_out8` with `Port` staged as a scalar `uint` (Q45); MMIO/DMA handles remain the target design. |

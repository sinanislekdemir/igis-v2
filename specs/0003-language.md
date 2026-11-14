# 0003: Language and standard library

- **Status:** Accepted (design specified; implementation pending)
- **Scope:** application language, type system, memory model, standard library
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002, 0004, 0005, 0007, 0010, 0011, 0016, 0018
- **Updated:** 2026-11-15

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

- **Records (implemented):** `type Point = record x, y: int; end;` (also accepted as
  `field: T;` lines). See "Records" below for the layout and access rules.
- **Sum types (implemented):** `type Token = Ident(name: string[32]) | Number(value: int) | Eof;`
  Sized by the largest variant; no boxing, no indirect variant. See "Sum types" below
  for the layout, construction and `case` rules.
- **Enums (implemented):** `type Status = (Ok, WouldBlock, Closed, NotFound, Invalid, Full, CapacityExceeded, IoError);`
- **Arrays (implemented):** `array[T, N]` — fixed, index `0..N-1`, value semantics. See
  "Arrays" below for the layout, index and copy rules.
- **Rings (implemented):** `ring[T, N]` — fixed-capacity FIFO with `push`/`pop`/`peek`/
  `full`/`empty`. See "Rings" below for the layout, operations and statuses.
- **Maps (implemented):** `map[K, V, N]` — at most `N` entries, open addressing,
  no growth. `K` is restricted to `int`, `bytes[N]` or `string[N]`. See "Maps" below
  for the layout, hashing, probing and deletion rules.
- **Stream endpoints:** `in_stream[N]` (receive only) and `out_stream[N]` (send only),
  N bytes of queue capacity. Distinct nominal types, so a receive endpoint cannot be sent
  to. The declaration's variable name is the stream name: at most 31 bytes matching
  `[a-z][a-z0-9_]*`, case-sensitive, and unique across the whole application. `N` is a
  compile-time constant in `1..=65536`; the loader refuses an application whose declared
  streams the kernel buffer pool cannot cover.
- **Task ids:** `TaskId` (an `int`), global, monotonically increasing, never reused;
  zero is invalid. The application stores the id in its own `state` only if it wants to
  `wait` or `kill` the child later; there is no per-task handle table.

#### Enums (implemented)

```text
type Color = (Red, Green, Blue);
```

- A `type` section sits with `const` and `var` at unit level and may appear in any
  order; one declaration is either `Name = (Variant, Variant, ...);`,
  `Name = record field: T; ... end;` or `Name = Variant | Variant(payload: T);`.
  A unit may have several `type` sections. Records, enums and sum types are all
  implemented; their rules follow.
- Enum types are **nominal**: `Color` and another enum with identical variants are
  distinct types. An enum name and its variant names share the one top-level namespace
  with constants, variables, procedures and other enum names. A duplicate type name, a
  duplicate variant (in one enum or across enums), or a variant/type name that collides
  with another top-level declaration is a compile error. An enum may not shadow a
  built-in type name.
- A variant is a **global constant of its enum type**, with ordinal `0..n-1` assigned in
  declaration order. The representation is one 8-byte slot holding the ordinal.
- Assignment and comparison accept the same enum type only. `=` and `<>` compare
  ordinals; `<`, `<=`, `>`, `>=` order by ordinal. Mixing two enums, or an enum with
  `int`/`uint`/`byte`, is a compile error without an explicit cast. Arithmetic on an
  enum is a compile error.
- Casts: `int(value)` yields the ordinal of an enum value. `EnumName(integer)` is the
  checked construction from an `int` ordinal and faults `Overflow` at run time when the
  value is outside `0..variant_count`, so an invalid enum value cannot exist. Other
  numeric casts from an enum must go through `int(x)` first; the only accepted source
  for an enum cast is `int` (or the same enum, which is the identity).
- The declaration emits no runtime type tag; only `EnumName(int)` emits an `enum_cast`
  instruction carrying the variant count (spec 0008).

#### Records (implemented)

```text
type Point = record
  x, y: int;
end;

type Named = record
  label: string[8];
  at: Point;
end;
```

- Fields may be scalars (`int`, `uint`, `byte`, `bool`, `f64`), enums, buffers
  (`string[N]`, `bytes[N]`), sum types and other records. Grouped names (`x, y: int;`) and
  one `name: type;` per line are both accepted. Every field type must be declared before
  the record; there are no forward references.
- Records are **nominal**: `Point` and another record with identical fields are distinct
  types. Assignment, parameters and comparisons require the same record type.
  Record equality is field-wise and not implemented in this slice; comparing two records
  is a compile error.
- There are **no record literals or constructors**. Declare a variable and assign its
  fields individually (`p.x := 1`); `Point(1)` is a compile error.
- Field access is `p.field` and nests (`n.at.x`). A read needs no writability; an
  assignment requires the base to be writable (a local, a state variable, a field of one,
  or a `var` parameter). A `const` parameter or any of its fields is read-only.
- Records appear as top-level `var`s, locals, procedure parameters (`value`, `const`,
  `var`) and fields of other records. `dst := src` copies the whole record; a `value`
  parameter is a copy, `const`/`var` parameters are call-scoped references.
- **Layout:** fields in declaration order, each aligned to 8 bytes: a scalar or enum is
  8; a buffer field is its `[len: u64][data: N]` image (`8 + N`, rounded to 8); a nested
  record or sum is its own aligned size. The record size is rounded to 8.

#### Sum types (implemented)

```text
type Token = Ident(name: string[32]) | Number(value: int) | Eof;
```

- A `type` declaration whose body begins with a variant name is a sum type. Variants are
  separated by `|`. Each variant carries **at most one payload**, written
  `Variant(name: T)`; a payload-less variant is just `Variant`.
- Sum types are **nominal**: `Token` and another sum with identical variants are distinct
  types. Assignment, parameters and copies require the same sum type. Comparison is not
  defined; `case` is the only way to inspect a value.
- **Layout:** an 8-byte tag at offset 0 holding the variant ordinal `0..n-1` in
  declaration order, then the **payload area** at offset 8. The payload area is the size
  of the largest variant payload, rounded to 8; a sum with no payloads is 8 bytes. A
  buffer payload's `[len][data]` image therefore starts at offset 8 (its data at 16).
- A payload may be a scalar, enum, buffer, record or another sum. Every payload type must
  be declared before the sum; there are no forward references and a sum cannot contain
  itself.
- **Construction is destination-driven** and writes into the destination, never a
  temporary: `token := Number(42)`, `token := Eof`. The destination names the sum type,
  so `Ok` is `Status.Ok` when the destination is `Status` and `ExitReason.Ok` when it is
  `ExitReason`; sum variant names are not global value constants. The payload must match
  the declared type exactly (no implicit conversion). A buffer payload is a construction
  like any buffer assignment (literals, same-kind buffers, `+`); a record or sum payload
  is copied from a value of that exact type (inline construction of a nested sum is not
  supported); a scalar or enum payload is an ordinary expression.
- `dst := src` copies the whole value (tag and payload bytes). A sum may be a state
  variable, a local, a `value`/`const`/`var` parameter, or a record field. A `value`
  parameter is a copy; `const`/`var` parameters are call-scoped references. A sum
  argument must be a variable or field, because there is no temporary to hold a fresh
  construction at the call site.
- **Pattern matching** is a Pascal-like statement:

  ```text
  case value of
    Ident(name): statements
    Number(value): statements
    Eof: statements
  end;
  ```

  The subject is a variable or field of a sum type. Every variant must have exactly one
  arm; a missing, duplicate or unknown variant is a compile error, and there is no `else`
  in this slice. An arm's body is a statement sequence (use `begin ... end` for several
  statements; `end` closes the `case`). There is no fallthrough. A payload variant binds
  its payload to a **new local of the payload type for that arm only**; the binding is by
  value for every payload kind (a buffer binding is a whole `[len][data]` copy), so
  assigning to it does not change the subject. A payload-less variant takes no binding.
  The binding name shares the local namespace, so it may not repeat an existing local or
  parameter name; it is not visible outside its arm. `case` statements nest.

#### Arrays (implemented)

```text
type Row = record cells: array[int, 3]; end;

var
  cells: array[int, 4];
  rows: array[array[string[8], 2], 3];
  grid: Row;
```

- `array[T, N]` is a fixed-size value: exactly `N` elements, indexes `0..N-1`, no
  append and no dynamic growth. `T` is any fixed-size type already supported (scalar,
  enum, record, sum, buffer or another array) and `N` is a positive constant expression
  (an integer literal or a `const`); `N = 0` and a negative `N` are compile errors.
- **Nominal identity:** arrays are identified by element type and count, so
  `array[int, 4]` and `array[int, 8]`, and `array[int, 4]` and `array[byte, 4]`, are
  distinct. Assignment, whole-array copies and parameters require the exact same type.
- **Layout:** `N` elements at `stride = align8(size_of(T))` bytes each; the array size is
  `align8(N * stride)`. Every current element type already has an 8-byte-aligned size, so
  the stride is its size and the array is `N * stride` bytes. Elements are inline; an
  array of buffers stores each element's `[len][data]` image.
- **Indexing:** `a[i]` reads the element and `a[i] := value` writes it. The index is
  `int`, `uint` or `byte`; the run-time check is `index < N` and failure is
  `fault(OutOfBounds)`, never a clamp (a negative `int` index is a huge `uint` and fails
  the same check). Nested access composes: `a[i][j]`, `a[i].field` and `rec.cells[j]`.
- **Element values:** scalars and enums load/store in one 8-byte slot; records, sums and
  nested arrays are copied whole; a buffer element is used through the ordinary buffer
  operations (indexing it yields a `byte`). The base must be writable for a write; a
  `const` parameter and everything reached through it is read-only.
- **Copies and parameters:** `dst := src` copies the whole `N * stride` image, exactly
  like a record copy. A `value` parameter is a by-value copy; `const`/`var` parameters
  are call-scoped references, so element writes through a `var` parameter reach the
  caller's array. The at-least-N rule does not apply to arrays: the element type and
  count must match exactly.
- Arrays may be `state` variables, locals, procedure parameters (`value`, `const`,
  `var`), record fields and sum payloads. Comparison is not defined for arrays.

#### Rings (implemented)

```text
var
  queue: ring[int, 4];
  words: ring[string[8], 2];
  grid: ring[ring[int, 2], 3];
```

- `ring[T, N]` is a fixed-capacity FIFO of exactly `N` slots with no growth. `T` is any
  fixed-size type already supported (scalar, enum, record, sum, buffer, array or another
  ring) and `N` is a positive constant expression; `N = 0` and a negative `N` are compile
  errors. `N` need not be a power of two.
- **Nominal identity:** rings are identified by element type and count, so `ring[int, 4]`,
  `ring[int, 8]` and `ring[byte, 4]` are all distinct. Assignment, whole-ring copies and
  parameters require the exact same type; the at-least-N rule does not apply. Comparison
  is not defined for rings.
- **Layout:** `stride = align8(size_of(T))`, storage `N * stride` bytes at offset 0, then
  `head: u64` at `N * stride` and `count: u64` at `N * stride + 8`; the total is
  `align8(N * stride + 16)`. Storage comes first, so an element reference uses the ring
  base directly. Invariants: `head < N`, `count <= N`; the logical index `i` (`0` is the
  oldest) maps to physical `(head + i) % N`, and the next push goes to physical
  `(head + count) % N`.
- **Operations** are built-in procedures in the `core` namespace, resolved and lowered by
  the compiler. `r` is the ring; `value` is the element payload.
  - `core.ring_push(r, value) -> Status`, `r` a writable ring. `Ok`, or `Full` when the
    ring is full; a failed push writes nothing and leaves `head`/`count` unchanged.
  - `core.ring_pop(r, var value) -> Status`, `r` a writable ring. Writes the oldest
    element through `value` and returns `Ok`, or returns `NotFound` on an empty ring and
    leaves `value` untouched.
  - `core.ring_peek(r, var value) -> Status` is `pop` without removal, with the same
    statuses and the same untouched-on-failure rule.
  - `core.ring_full(r) -> bool` and `core.ring_empty(r) -> bool` read `count`.
  The receiver must be a `ring[T, N]` designator. `push`/`pop` need a writable place: a
  `const` parameter is read-only and is a compile error. A `value` parameter is an
  ordinary writable copy (like a `value` record or array), so a push/pop through it
  affects the callee's copy only and never the caller's ring. `peek`/`full`/`empty` accept
  any of the three modes. A `core.ring_*` call on a non-ring first argument or with the
  wrong argument count is a compile error.
- **Element values:** scalars and enums use one 8-byte slot; records, sums, arrays and
  nested rings are copied whole (`stride` bytes); a buffer element is written as its
  `[len][data]` image (literal or a same-kind buffer, capacities exactly matching unless
  the source is a call-scoped parameter).
- **Copies and parameters:** `dst := src` copies the whole `align8(N * stride + 16)`
  image, header included, so `head` and `count` travel with the storage. A `value`
  parameter is a by-value copy; `const`/`var` parameters are call-scoped references.
- Rings may be `state` variables, locals, procedure parameters, record fields, sum
  payloads and array elements. A library signature may carry a ring: the structural ring
  table is shared by the application and `stdlib`.
- No opcode is added: the operations lower to `ref_offset`/`ref_element`/`load_ref`/
  `store_ref`/`copy_refs`, the buffer opcodes for a buffer element, `eq` for the checks,
  `mod` for the wrap and ordinary branches (spec 0008, "Rings").

#### Maps (implemented)

```text
var
  counts: map[int, int, 4];
  labels: map[string[8], string[16], 8];
  slots: map[int, Point, 2];
```

- `map[K, V, N]` holds at most `N` entries and never grows. `N` is a positive
  constant expression (an integer literal or a `const`); `N = 0` and a negative `N` are
  compile errors. `K` is `int`, `string[M]` or `bytes[M]`; any other key type
  (`byte`, `bool`, `f64`, an enum, record, sum, array or ring) is a compile error.
  `V` is any fixed-size type already supported: scalar, enum, buffer, record, sum,
  array, ring or another map.
- **Nominal identity:** a map is identified by `(K, V, N)`, so `map[int, int, 4]` and
  `map[int, int, 8]` are distinct, as are `map[string[8], int, 4]` and
  `map[bytes[8], int, 4]`. Assignment, whole-map copies and parameters require the
  exact same type; the at-least-N rule does not apply to the map type itself.
  Comparison is not defined for maps.
- **Layout:** `count: u64` at offset 0, then `capacity` slots at offset 8. The
  capacity is the smallest power of two at least `2 * N` (`N=1 → 2`, `N=3 → 8`,
  `N=4 → 8`, `N=5 → 16`), so an empty slot always exists while `count <= N` and
  probing terminates. A slot is `state: u64` at 0 (`0` empty, `1` occupied),
  `hash: u64` at 8, the key image at 16 and the value image at `16 + key_size`;
  `slot_stride = 16 + key_size + value_size` and the total is
  `8 + capacity * slot_stride`, all 8-byte multiples. `key_size` is 8 for an `int`
  key and `align8(8 + M)` for a buffer key; `value_size` is `size_of(V)`. A map whose
  total exceeds the section cap is a compile error.
- **Hashing and probing:** FNV-1a, 64-bit (offset basis `14695981039346656037`,
  prime `1099511628211`), over the eight little-endian bytes of an `int` key or the
  `[0..len)` bytes of a buffer key. The home slot is `hash & (capacity - 1)`; probing
  is linear (`+1`, same mask). The stored hash is compared first and only then the
  key (length, then bytes for a buffer), so deletion never re-hashes. No key is
  special: key `0`, an empty buffer key and a key whose hash is `0` are ordinary.
- **Deletion is backward-shift, with no tombstones.** The found slot is cleared, then
  the walk continues forward through the cluster and moves an entry back when its
  distance from its home slot (recomputed from the stored hash with the mask) is
  greater than the distance from home to the hole, repeating until an empty slot.
  Every remaining key stays findable at any load factor.
- **Operations** are built-in procedures in the `core` namespace, resolved by the
  compiler and lowered to the `map_*` opcodes (spec 0008, "Maps"). `m` is the map;
  `key` and `value` are the key and value operands.
  - `core.map_put(m, key, value) -> Status`, `m` a writable map. Replaces an existing
    key in place and returns `Ok`; a new key while `count < N` inserts and returns
    `Ok`; a new key when `count = N` returns `Full` and changes nothing.
  - `core.map_get(m, key, var value) -> Status` writes the value through `value` and
    returns `Ok`, or returns `NotFound` and leaves `value` untouched.
  - `core.map_has(m, key) -> bool` is the membership test, `false` for a missing key.
  - `core.map_remove(m, key) -> Status` removes a key (`Ok`) or returns `NotFound`.
  - `core.map_count(m) -> int` returns the live entry count.

  `put`/`remove` need a writable place: a `const` parameter is a compile error, while
  a `value` parameter is an ordinary writable copy (like a `value` record or ring), so
  the caller's map is unchanged. `get`/`has`/`count` accept any of the three modes. A
  non-map receiver, a wrong argument count, a wrong value or out-parameter type and a
  mutation of a `const` map are compile errors.
- **Keys and values:** a buffer key follows the at-least-N rule because the map only
  reads it, so a `const string[16]` parameter may be used with a `string[8]` key type;
  a literal is typed by `K` and must fit its capacity. Mixing `string` and `bytes` in
  either direction is a compile error. `put`'s value and `get`'s out-parameter must be
  exactly `V` (a literal is typed by `V`).
- Maps may be `state` variables, locals, record fields, sum payloads, array elements,
  ring elements and `value`/`const`/`var` parameters. `dst := src` copies the whole
  `8 + capacity * slot_stride` image, header and slots included, exactly like a ring
  copy; a `value` parameter is a by-value copy and `const`/`var` parameters are
  call-scoped references. A library signature may carry a map: the structural map
  table is shared by the application and `stdlib`.
- Iteration order is not specified, and this slice has no iteration operation.

#### Predeclared records and sum type

`Diagnostic` and `ExitReason` exist in every unit, with the fixed layout below
(`abi::layout` is the shared source of truth); declaring either name is a compile error
(and the same holds for `FaultCode` below). `Diagnostic` is a record. `ExitReason` is a
real sum type: tag `0` Ok, `1` Killed, `2` Faulted, with the `FaultCode` ordinal in the
payload area.

```text
type Diagnostic = record          // offsets, size 192
  unit_name: string[32];          // 0   (8 + 32 = 40 bytes)
  message: string[96];            // 40  (8 + 96 = 104 bytes)
  line: int;                      // 144
  column: int;                    // 152
  handle: int;                    // 160  (-1 when not involved)
  offset: int;                    // 168  (-1 when not involved)
  length: int;                    // 176  (-1 when not involved)
  task_id: int;                   // 184
end;

type ExitReason = Ok | Killed | Faulted(code: FaultCode);   // size 16:
                                                            // tag at 0, payload at 8
```

`FaultCode` is also predeclared, as an enum with the `abi::FaultCode` ordinals in order:
`GenericFault`, `OutOfBounds`, `CapacityExceeded`, `Overflow`, `DivideByZero`,
`StackOverflow`, `BudgetExceeded`, `WatchdogTimeout`, `IllegalOperation`, `StreamError`,
`IoError`, `ExitFault`. Because `CapacityExceeded` and `IoError` collide with `Status`
variants, `FaultCode` names its variants **qualified**, `FaultCode.DivideByZero`; it
contributes no global constants. Every other enum's variants are global constants (spec
0003, "Enums"), and a sum type's variants are reachable only through construction and
`case`.

There are **no user-defined generics**. The parametric types above are built in; the
compiler knows their layout and operations. Generic algorithms are either built-ins
(`ring`, `map`) or written per type in `stdlib`.

### Capacity compatibility

Capacities are part of the type: `string[128]` and `string[256]` are distinct (D11), and
so are `array[int, 4]` and `array[int, 8]`, and `ring[int, 4]` and `ring[int, 8]`. Two
rules make this bearable:

- **Variables and fields:** exact type. Assignment between two variables of different
  capacities, element types or array counts is a compile error.
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

### Functions

A subprogram is either a `procedure` or a `function`:

```text
function trim(dst: var string[64]; src: const string[0]): int;
var
  n: int;
begin
  n := ...;
  return n;
end;
```

- A `function` declares one result type after the parameter list. The implemented slice
  accepts **scalar** results only (including enums such as `Status`); a buffer, record or
  sum result is a compile error for now.
- `return value;` leaves the subprogram and delivers the value to the caller. A `procedure`
  may use `return;` to leave early.
- A function must return on every path: the compiler requires the body's last statement to
  be a `return`, a block whose last statement is one, or an `if`/`else` whose branches both
  are. A fall-through is a compile error.
- A function call is an ordinary expression with the declared result type; using it as a
  statement drops the value. The call syntax is the same as a procedure call.
- Implementation (spec 0008): the caller pushes a destination reference before the
  arguments, and the callee writes the result there on `return`. The interpreter's
  `dest_ref` frame slot and `Function.has_destination` carry it; no opcode is added.

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

`Status` is **predeclared**: it exists in every unit without a `type` declaration, and
its variant names (`Ok`, `WouldBlock`, ...) resolve as global constants with ordinals
`0..7` in exactly that order. A user `type Status = ...` is a compile error. The ordinal
of `Ok` is zero, so `st = Ok` is an ordinary enum comparison. The compiler's `Status`
and `abi::Status` share the same order; the interpreter pushes a host-returned `Status`
directly (spec 0005).

- **E1 (MUST):** A fallible operation returns `Status` and writes its result through a
  `var` out-parameter.
- **E2 (MUST):** The compiler warns when the `Status` returned by a call is discarded
  without being compared to `Ok`. (No warning channel exists yet; the rule stands for
  the first milestone.)
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

The implemented REPL primitives follow E1 and return `Status`:

```text
core.run(path: const string[0], args: const string[0], var diag: Diagnostic, var reason: ExitReason) -> Status
core.spawn(path: const string[0], args: const string[0], var diag: Diagnostic, var id: int) -> Status
core.wait(id: int, var reason: ExitReason) -> Status
core.kill(id: int) -> Status
```

`Diagnostic` and `ExitReason` are predeclared (above), so both out-parameters are real
language values: `spawn` fills `diag` on a synchronous failure and clears it on success;
`run` is spawn + wait and also fills `diag` from a load failure the service discovers
after the spawn returned; `wait` writes the child's `ExitReason` tag and payload, which the
caller matches with `case`. `TaskId` is still an `int` (spec 0005, "Implemented ABI").

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
  **Implemented slice:** `spawn`/`wait`/`kill` return `Status` and take the predeclared
  `Diagnostic` record and `ExitReason` sum type; a synchronous spawn failure fills `diag`,
  success clears it, and `wait` writes the reason's tag/payload. `run` is implemented as
  spawn + wait and additionally reports a post-spawn load failure through `diag`; `TaskId`
  is still an `int` (spec 0005).
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
`var diag: Diagnostic` out-parameter: a synchronous failure (a malformed, missing or
non-regular path, an over-long path or argument, a full request queue, or a compile
error) fills the failing unit, line, column and message; success clears it (zeroed text,
`0` line/column/task id and `-1` for `handle`/`offset`/`length`). The compiler writes
into the caller's buffer, so there is no global error slot and no diagnostic stream.

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

### Implemented first slice

The compiler embeds `stdlib/stdlib.ig` (`include_str!`) and compiles it into the
application's package when the application says `uses stdlib;` (source-only, ADR 0019; spec
0008, "Multi-unit compilation"). The library imports only `core`, declares no `state`, and
uses no new opcodes. Its top-level names are **qualified** from the application
(`stdlib.trim_into`) and unqualified inside the unit.

Capacities are concrete because the language has no generics; the numbers below are the
declared capacities and the at-least-N rule applies to callers.

| Helper | Semantics |
|--------|-----------|
| `function stdlib.trim_into(dst: var string[256]; src: const string[0]): int` | Copies `src` without leading or trailing space, tab, CR or LF. Returns the copied length, or `-6` (`Status.CapacityExceeded`) without writing when the trimmed result exceeds the destination capacity. |
| `function stdlib.split_once(dst_head: var string[128]; dst_tail: var string[256]; src: const string[0]; sep: byte): int` | Splits at the first `sep`: returns `1` with both parts, or `0` with the whole source in `dst_head` and an empty `dst_tail`. A part that does not fit returns `-6` without a partial tail write. Uses `core.find` and `core.substr_into`. |
| `function stdlib.parse_int_into(src: const string[0]; value: var int): Status` | Optional leading `-`, then decimal digits only. `Invalid` for empty text, a bare `-`, a `+` sign or any non-digit; `CapacityExceeded` when the magnitude does not fit an `int`; `-9223372036854775808` is accepted. Writes nothing on failure and never faults. |
| `function stdlib.starts_with(src: const string[0]; prefix: const string[0]): bool` | Byte-wise prefix test (not a code-point test). |
| `function stdlib.contains_byte(src: const string[0]; value: byte): bool` | True when `core.find` finds the byte. |
| `function stdlib.drain_into(dst: var array[int, 8]; count: var int; src: var ring[int, 8]): Status` | Moves up to eight integers out of `src`, oldest first, and writes the moved count. `NotFound` (nothing written) when `src` is empty. The first helper whose signature carries a `ring`. |

Still missing for a complete standard library: UTF-8 helpers (code-point iteration,
checked `utf8_decode_into`), the move of `substr_into` from `core`, `f64`
formatting/parsing, and typed stream views. `map` support and `map_get_or` landed with
the structural map table (spec 0003, "Maps").

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Index outside `array` | `fault(OutOfBounds)`; recover once or terminate |
| Buffer read at or past the runtime length | `fault(OutOfBounds)` |
| Buffer write at or past the declared capacity | `fault(CapacityExceeded)`; never a partial write |
| Buffer index expression that is not `int`/`uint`/`byte` | Compile error |
| `string[i] := expr` | Compile error (`string` is read-only by index) |
| `core.ring_push` on a full ring | `Status.Full`; value and ring unchanged |
| `core.ring_pop`/`peek` on an empty ring | `Status.NotFound`; out-parameter untouched |
| `core.ring_push`/`pop` on a `const` ring parameter | Compile error |
| `core.map_put`/`map_remove` on a `const` map parameter | Compile error |
| `core.ring_push`/`pop` on a `value` ring parameter | Mutates the callee's private copy; the caller's ring is unchanged |
| Buffers of different kinds compared (`string` = `bytes`) | Compile error |
| `string`/`bytes` write past capacity | `fault(CapacityExceeded)`; no truncation, no growth |
| `ring` push on full, `map` put on full | `Status.Full` (non-fatal) or explicit failure, never growth |
| Invalid UTF-8 conversion | `Status.Invalid`; no invalid `string` can be constructed |
| Integer arithmetic overflow | `fault(Overflow)` |
| Integer division by zero | `fault(DivideByZero)` |
| Float division by zero | `Inf`/`NaN` per IEEE-754; not a fault |
| `int(f)`, `uint(f)` or `byte(f)` with `NaN` or out of range | `fault(Overflow)` |
| A map key that is not `int`, `string[M]` or `bytes[M]` (including `f64`) | Compile error |
| Shift count outside `0..63` | `fault(IllegalOperation)` |
| Duplicate or malformed stream name | Compile error |
| Duplicate enum or sum type, duplicate variant, or a variant colliding with another top-level name | Compile error |
| `type Status = ...`, `type FaultCode = ...`, `type Diagnostic = ...`, `type ExitReason = ...` (predeclared) | Compile error |
| Duplicate record field, unknown field, or a field of a non-record | Compile error |
| Record or sum assigned, passed or compared with a different type of the same kind | Compile error |
| Assignment to a field of a `const` parameter or record | Compile error |
| Comparing two records for equality | Compile error (field-wise comparison is not implemented) |
| Comparing two sums, or using a sum as a scalar | Compile error (`case` is the only way to inspect a sum) |
| `case` subject is not a sum type | Compile error |
| `case` missing a variant, repeating one, or naming an unknown variant | Compile error (exhaustive, no `else`) |
| A variant constructed with a payload it does not declare, or without its payload | Compile error |
| A payload expression whose type differs from the declared payload type | Compile error |
| A variant name from another sum type | Compile error |
| Enum compared or assigned to a different enum, or to an integer, without a cast | Compile error |
| Arithmetic on an enum | Compile error |
| `EnumName(x)` with `x` outside `0..variant_count` | `fault(Overflow)`; no invalid enum value can exist |
| Stream capacity outside `1..=65536` | Compile error |
| Path longer than 256 bytes | `Status.CapacityExceeded` |
| Recursion past the declared stack | Guard-page fault; `fault(StackOverflow)` |
| Send on a receive endpoint | Compile error (distinct types) |
| Send on a closed endpoint | `Status.Closed` |
| Realtime heartbeat missed | `fault(WatchdogTimeout)`; recovery rejected |
| `run`/`spawn` path not found | `Status.NotFound`; `diag` filled; no child |
| Argument longer than 1024 bytes | `Status.CapacityExceeded`; `diag` filled |
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
| 2026-11-14 | Enums implemented: `type Name = (Variant, ...);` with nominal types, ordinals `0..n-1`, variants as global constants in the top-level namespace, one 8-byte slot, same-enum assignment/comparison plus ordinal ordering, `int(enum)` and range-checked `EnumName(int)` (`fault(Overflow)`). `Status` is predeclared with the spec's eight variants (`Ok` = 0); a user declaration is an error. `spawn`/`wait`/`kill` return `Status` with `var id`/`var reason` out-parameters (Q43 narrowed; spec 0005, spec 0008 `enum_cast`). |
| 2026-11-14 | Records implemented: `type Name = record field: T; ... end;` (grouped or one-per-line fields), nominal type identity, declaration-before-use, nested records, buffer fields, whole-record copy, `value`/`const`/`var` record parameters and aligned declaration-order layout. `Diagnostic` and `ExitReason` are predeclared at the fixed `abi::layout` offsets and cannot be redefined; `spawn`/`wait` use them (`var diag`, `var reason`). Record equality, sum types and `core.run` remain future work (Q43). |
| 2026-11-14 | Sum types and pattern matching implemented: `type Name = Variant | Variant(payload: T);` is an 8-byte tag at offset 0 plus the largest payload at offset 8, nominal, with destination-driven construction (`dst := Variant(payload)`) and no global variant constants. `case value of Variant: ... Variant(binding): ... end;` is exhaustive (one arm per variant, no `else`), has no fallthrough, and binds a payload by value to an arm-local copy; unknown, duplicate and missing arms, payload mismatches and cross-sum variants are compile errors. `ExitReason` becomes the real sum type `Ok | Killed | Faulted(code: FaultCode)` at the same 16-byte layout, and `FaultCode` is predeclared as an enum whose variants are qualified (`FaultCode.DivideByZero`) because two names collide with `Status`. `Diagnostic` stays a record; `spawn` is unchanged. Q43 narrows to `core.run` and the async compile-diagnostic path. |
| 2026-11-14 | `core.run(path, args, var diag, var reason) -> Status` implemented (spawn + wait, with the pending id held in the machine). The request-completion diagnostic path also landed: a compile/load failure after `spawn` returned fills `diag` for `run` callers while `wait` still observes `Faulted(IoError)`. Q43 narrows to a distinct `TaskId` type. |
| 2026-11-14 | `function Name(...): T;` with `return value;` implemented for scalar results; a function must end in a `return` on every path, and a procedure may `return;`. Implemented the first `stdlib` slice in real source: `trim_into`, `split_once`, `parse_int_into`, `starts_with` and `contains_byte`, compiled into the application package, qualified as `stdlib.*`, with no new opcodes. |
| 2026-11-14 | Fixed-capacity rings implemented: `ring[T, N]` is nominal in element and count, layout is `N` elements at `align8(element size)` followed by `head: u64` and `count: u64`, and the built-in `core.ring_push`/`pop`/`peek`/`full`/`empty` lower to the existing reference/copy/branch instructions (no opcode). `push` returns `Full` on a full ring; `pop`/`peek` return `NotFound` and leave the out-parameter untouched on an empty one; `push`/`pop` require a writable place, so a `const` receiver is rejected while a `value` copy is writable and private to the callee. Rings may be state, locals, parameters, record fields, sum payloads and array elements; the structural ring table is shared with `stdlib`, whose `drain_into` takes a ring. |

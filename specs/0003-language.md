# 0003: Language and standard library

- **Status:** Accepted (design specified; implementation pending)
- **Scope:** application language, type system, memory model, standard library
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002, 0004, 0005, 0007, 0010, 0011, 0016, 0018, 0021, 0024
- **Updated:** 2026-11-17

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
- Text conversion is fixed-point plus a shortest round-trip form in the first milestone:
  `core.format_f64_into(dst, value, frac_digits)` writes fixed-point decimal text with a
  chosen fraction width, `core.format_f64_short_into(dst, value)` writes the shortest
  significant-digit string that parses back to exactly the same bits (positional or
  scientific), and `stdlib.parse_f64_into(text, var value)` parses decimal text and returns
  `Status`. See "Floating-point text (implemented)" below for the shape rules, error codes
  and parse grammar. Correctly rounded decimal-to-binary parsing and its subnormal edges
  are deferred (Q41).
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
  `string`. A checked `core.substr_into` returns `string` only if both boundaries fall
  on code-point boundaries (`string` source) or the copied range is strict UTF-8
  (`bytes` source); its `bytes` destination copies raw data.
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

### Slicing and search (implemented)

- **The checked slice stays in `core`.** `core.substr_into(dst, src, start: int, len: int)
  -> int` is the checked slice from T4. The earlier staging note said it would move to
  `stdlib.substr_into` "once the language can express it"; it cannot, ever, without a
  language change: the destination capacity must be a **call-site immediate** (that is
  what the `SUBSTR` `capacity` immediate is), while a `stdlib` helper can only declare
  concrete capacities (there are no generics). A library helper would therefore only serve
  one destination size and would lose `split_once`'s `string[128]` head and every other
  smaller destination. `stdlib.substr_into` is withdrawn as a planned move; the future
  option is a buffer parameter that carries its runtime capacity (recorded in
  `OPEN-QUESTIONS.md`). There is one slicing primitive, not two.
- **Four kind combinations, destination-kind rule.** `dst` must be a writable (`var`)
  buffer and `src` any buffer, including a literal. `a` in the instruction is the
  destination kind and `b` the source kind, so all four combinations are legal and the
  **destination** decides the check:
  - `string` destination, `string` source: both `start` and `start + copied` must fall on
    UTF-8 code-point boundaries in `src`; a slice that splits a code point (including an
    empty range that starts inside one) is rejected.
  - `string` destination, `bytes` source: the copied range must be strict UTF-8 — 1- to
    4-byte sequences only, no overlong encodings, no surrogates `U+D800..U+DFFF`, nothing
    above `U+10FFFF`, no truncated sequence, no stray continuation byte. Invalid input is
    rejected. This is the only primitive that can produce a `string` from raw bytes, so the
    invariant is enforced where it is created (T3).
  - `bytes` destination (either source kind): a raw copy, no UTF-8 check (T4).
- **Results.** Negative `start` or `len` returns `-4` (`Status.Invalid`) and writes
  nothing. `start > len(src)` returns 0 and clears the destination. Otherwise the copied
  length is `min(len, len(src) - start, capacity(dst))`, the destination's runtime length
  becomes that value, and the result is the copied length. A rejected boundary or invalid
  byte sequence also returns `-4` and writes nothing.
- **`core.find(buf, value: byte) -> int`** returns the first index of `value`, or `-1`.
  It scans `0..len` only and serves both kinds; a `string` is searched byte-wise, not by
  code point.

### String editing and selection (implemented)

`stdlib` provides code-point-safe editing for a 256-byte `string` buffer (M2.4.1). The
helpers are ordinary language code, not opcodes or builtins. `string` stays read-only by
index (T4), so a mutator stages the result in a local `bytes[256]`, converts it back with
`core.substr_into` (which validates strict UTF-8) and publishes the whole value with one
assignment. Nothing is written into the destination before that final copy.

- **SE1 (MUST):** Positions are byte offsets, never code-point ordinals (T5). A
  **boundary** is 0, the runtime length, or the first byte of a code point. On a valid
  `string` that is exactly "the byte at the offset is not a continuation byte
  (`0x80..0xBF`)".
- **SE2 (MUST):** Insertion and deletion require both ends of their range to be boundaries
  inside the text. A negative or past-the-end position, a non-boundary, a negative count or
  a range that runs past the end returns `Invalid` and changes nothing.
- **SE3 (MUST):** A mutator is all-or-nothing. It publishes with one whole-buffer copy
  after every check and staging step, so a rejected call leaves the destination
  byte-identical, and the result is always valid UTF-8 (the conversion refuses anything
  else rather than constructing an invalid `string`).
- **SE4 (MUST):** Insertion and selection replacement return `CapacityExceeded` and change
  nothing when the result would exceed the helper's 256-byte working capacity. Deletion
  never grows the text, so its own result always fits; `CapacityExceeded` there only
  reports an incoming runtime length above 256 (the at-least-N rule can pass a larger
  buffer), and the text is still untouched.
- **SE5 (MUST):** Cursor helpers clamp a bad cursor rather than failing: negative → 0,
  past the end → the length, inside a code point → that code point's first byte. They never
  return `Invalid`, and movement crosses whole code points.
- **SE6 (MUST):** The selection is the scalar pair `start`/`stop`, both byte offsets.
  Normalizing clamps both to boundaries and orders them (`start <= stop`). It is never a
  record, so it crosses the unit boundary as two `int`s.
- **SE7 (SHOULD):** `str_backspace` at offset 0 is `Ok` with nothing changed: a text field
  always receives the key, and an empty prefix is a state, not an error.

The mutators are `str_insert_into`, `str_delete_into`, `str_backspace` and
`str_selection_replace`; the read-only helpers are `str_boundary`, `str_cursor_clamp`,
`str_cursor_left`, `str_cursor_right`, `str_cursor_home`, `str_cursor_end`,
`str_selection_normalize`, `str_selection_all` and `str_selection_collapse`. Their exact
signatures and results are in "Standard library" below.

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

### Floating-point text (implemented)

- **`core.format_f64_into(dst: var string[N]; value: f64; frac_digits: uint) -> int`**
  writes fixed-point decimal text into a writable `string` or `bytes` destination (the
  output is ASCII) and returns the byte count. `bytes` is accepted because the output is
  text, not because the formatter writes binary.
- **Width bound.** At most 9 fraction digits are supported. `frac_digits > 9` returns `-4`
  (`Status.Invalid`) and writes nothing, whatever the value. Nine digits is already below
  the ~15–17 significant decimal digits an `f64` carries, so a wider field would only
  print binary noise.
- **Range.** The fixed-point form has no exponent, so a magnitude at or above `2^63`
  (where the integer part stops fitting a 64-bit value) returns `-4` and writes nothing.
  `NaN`, `+Inf` and `-Inf` are not in that class: they are written as `nan`, `inf` and
  `-inf`, literally and ignoring `frac_digits`, and the result is their length.
- **Shape.** Otherwise the output is an optional `-`, the integer part without leading
  zeros (a single `0` when the integer part is zero), and — only when `frac_digits` is
  non-zero — a `.` followed by exactly `frac_digits` digits. `-0.0` keeps its sign
  (`-0.000` with three digits); the sign of a zero is the one piece of information a
  magnitude-only formatter would lose.
- **Rounding.** The fractional part is scaled by `10^frac_digits` and rounded half away
  from zero; a rounded value equal to the scale carries into the integer part (`9.999`
  with 2 digits → `10.00`, `-2.5` with 0 digits → `-3`, `0.5` with 0 digits → `1`). The
  digits come from `f64` arithmetic plus integer digit extraction: no allocation, no
  formatting library, and no decimal expansion of the binary value beyond what `f64`
  itself carries. That is the fixed-point form's precision (the shortest form below is
  exact), so a value whose decimal form is not exact in binary64 rounds from the binary
  value (`2.675` with 2 digits gives `2.67`, not the decimal reader's `2.68`).
- **Capacity.** The whole result must fit `N`. If it does not, nothing is written, the
  destination length is left untouched, and the result is `-6`
  (`Status.CapacityExceeded`). On success the runtime length is the byte count and the
  result is that count.
- The formatter is a pure buffer operation in the interpreter (`format_f64`, spec 0008),
  like `format_int`, not a host call: it builds the digits in a local staging array and
  publishes them with one copy, so a rejected value never leaves a partial destination.
- **`core.format_f64_short_into(dst: var string[N]; value: f64) -> int`** writes the
  shortest significant-digit string that parses back to exactly the same IEEE-754 bits.
  Like the fixed-point form it accepts a `bytes` destination (the output is ASCII) and
  returns the byte count. There is no width argument and no magnitude bound.
- **Digits.** The digits are the fewest that round-trip, with no leading zero and no
  trailing zero except the single digit `0` of a zero (which is written `0`, and `-0` for
  negative zero). A value with fewer than 17 significant digits prints them exactly;
  otherwise 17 are used. `NaN` is `nan`, the infinities are `inf`/`-inf`, and neither takes
  a sign from the payload.
- **Shape.** Let `digits` be the significant digits (`n >= 1`) and `e10` the decimal
  exponent with `value = d[0].d[1..] * 10^e10`; let `k = e10 + 1`. The positional form is
  used exactly when `-6 < k <= 21`:
  `k <= 0` → `0.` followed by `-k` zeros and `digits`; `0 < k < n` → `digits[0..k]`, `.`,
  `digits[k..]`; `k >= n` → `digits` followed by `k - n` zeros. Otherwise the scientific
  form is `digits[0]`, then `.` plus the rest only when `n > 1`, then `e` and the exponent
  in decimal, with `-` when it is negative and no leading zero (`1e300`, `1.5e-7`,
  `5e-324`). This is the ECMAScript boundary: `1e20` prints positionally as
  `100000000000000000000`, `1e21` prints `1e21`, `1e-6` prints `0.000001` and `1e-7` prints
  `1e-7`. There is never a trailing `.0`, so an integral value prints with no fraction
  (`1`, not `1.0`).
- **Nearest, ties up.** When two `n`-digit decimals both round-trip, the one nearer the
  binary value is chosen; an exact tie (the value is precisely halfway between them, e.g.
  `2^-25`) rounds away from zero. Both candidates round-trip, so the choice only affects
  which equally short string is printed.
- **Capacity.** The whole result must fit `N`. If it does not, nothing is written, the
  destination length is left untouched, and the result is `-6`
  (`Status.CapacityExceeded`). The longest accepted text is 25 bytes (a sign, `0.`, five
  zeros and 17 digits), so a positional form never needs more.
- The digit search is a pure interpreter operation (`format_f64_short`, spec 0008) and the
  same staging discipline applies: the exact decimal expansion and the round-trip interval
  are integers, the shortest digits are found by integer comparison, and the text is
  published with one copy. No allocation and no host call.
- **`stdlib.parse_f64_into(src: const string[0]; value: var f64) -> Status`** parses
  decimal text. The grammar is
  `[+-]? ( digits [ '.' digits? ] | '.' digits ) ( [eE] [+-]? digits )?` with at least one
  mantissa digit, plus the exact strings `inf`, `+inf`, `-inf` and `nan` (the formatter's
  own output), so a format→parse round trip works. Everything else — empty text, a bare
  sign, `.`, `1e`, `1e+`, `1.2.3`, an embedded or trailing space, `+nan`, `INF` — is
  `Invalid`, writes nothing and never faults.
- **Precision.** The mantissa is accumulated as an `f64`: the fraction as an integer
  numerator over a power of ten (so `0.375` is exact, and only 18 significant digits are
  kept), then the decimal exponent is applied by repeated multiplication or division by
  10. Shortest round-trip formatting is `core.format_f64_short_into` above; correctly
  rounded decimal-to-binary parsing and its subnormal edges stay deferred (Q41): parsing
  lands on the `f64` value of the text, not on a correctly rounded one.
- **Saturation, not errors.** A magnitude that overflows `f64` (e.g. `1e400`) becomes
  `±Inf` with `Ok`; an underflow (e.g. `1e-400`) becomes `±0` with `Ok`. The sign of
  `-0.0` is preserved, including from `-0.0`, `-0e5` and the formatter's `-0.000`. The
  exponent loop stops once the value has saturated, so the cost does not grow with the
  exponent the text names, and a mantissa with hundreds of integer digits saturates to
  `±Inf` rather than being renormalised.
- Parsing is ordinary language code (`stdlib`), not a new opcode or builtin.

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
- **Stream endpoints (implemented):** `in_stream[N]` (receive only) and `out_stream[N]` (send only),
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

#### Streams (implemented)

```text
var
  events_out: out_stream[4096];   // stream "events_out", send side
  config_in:  in_stream[256];      // stream "config_in", receive side
```

- `in_stream[N]`/`out_stream[N]` are distinct nominal types; the value is one opaque
  8-byte kernel handle. Sending on an `in_stream` (or receiving on an `out_stream`) is a
  compile error. `N` is a compile-time constant in `1..=65536`.
- A stream endpoint may be declared **only** as a top-level `var` of the application
  unit. The variable name is the stream name: at most 31 bytes matching
  `[a-z][a-z0-9_]*`, unique application-wide across both units (S11/S12). Locals,
  record fields, sum payloads, arrays, rings, maps and `value`/`var` parameters are
  compile errors. A `const in_stream`/`const out_stream` parameter is allowed and
  accepts any capacity of the same direction (it borrows the caller's handle); this is
  how the `stdlib` framing helpers take an endpoint (they declare capacity `1`, since the
  declared capacity of a stream parameter is not checked).
- Stream handles occupy the first `8 * endpoint_count` bytes of `state`, in declaration
  order; the manifest descriptors are emitted in the same order (spec 0008). The loader
  writes handle `i` at `state[i * 8]` before the task starts.
- Assignment, comparison, copying and any other use as a value are compile errors: an
  endpoint is not data.
- `core.send(ep, data) -> Status` copies a whole `string`/`bytes` buffer (literal or
  variable) atomically into the stream. It returns `Ok`, `CapacityExceeded` when the
  buffer is larger than the stream capacity (never blocks), `Closed` when the peer is
  gone, or blocks while the queue has less free space than the buffer needs.
- `core.try_send(ep, data) -> Status` is the same but returns `WouldBlock` instead of
  blocking.
- `core.recv(ep, dst) -> int` copies up to `dst`'s capacity bytes from the stream into a
  writable `bytes[N]`, returns the copied count (`>= 1`), or blocks while the queue is
  empty. `dst` must be `bytes`; a `string` destination is a compile error because a
  partial read could split a code point. A closed, empty stream returns `-2`; a negative
  result is a staged status code (`-1` `WouldBlock`, `-2` `Closed`, `-4` `Invalid`).
- `core.try_recv(ep, dst) -> int` is the same but returns `-1` instead of blocking.
- Blocking is the scheduler's ordinary block: the task stops consuming CPU and is woken
  by a send, by a receive that frees space, or by a close. The task owns the direction
  its type names; the kernel also checks this on every call.
- The compiler's failure table adds: a stream type outside a top-level `var` or `const`
  parameter, a name outside `[a-z][a-z0-9_]*`, a capacity outside `1..=65536`, a
  same-direction or same-name duplicate, an assignment/comparison of an endpoint,
  `send` on an `in_stream`, `recv` on an `out_stream`, and a non-`bytes` receive
  destination.

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

`EventKind` and `Event` are also predeclared (spec 0010, I19), with the fixed layout in
`abi::layout::event`:

```text
type EventKind = (PointerMove, PointerDown, PointerUp, KeyDown, KeyUp, Expose);

type Event = record              // offsets, size 40
  kind: EventKind;               // 0   (ordinal in one 8-byte slot)
  x: int;                        // 8   pointer x (0 for key events)
  y: int;                        // 16  pointer y (0 for key events)
  code: uint;                    // 24  packed key value or button mask
  window: int;                   // 32  always 0 until M2.3 routes events
end;
```

The `EventKind` variants are global constants (`KeyDown`, `PointerMove`, ..., `Expose`), so
an application writes `ev.kind := KeyDown` and compares `ev.kind = PointerDown`. `Expose`
is the window service's whole-content repaint request (spec 0011, W12). For a key
event, `code` packs `text | modifiers << 8 | key code << 16` with `abi::key`'s shifts;
the text byte is the ASCII/control byte (0 for a non-text key or a release), the modifier
bits are Shift 1 / Ctrl 2 / Alt 4 / Caps 8 at the time of the transition, and the key code
is the unshifted ASCII (letters uppercase) or a `0x100`-range code for Escape,
Backspace, Tab, Enter, arrows, Home/End/Page keys, Insert/Delete, F1–F12 and the modifier
keys. Declaring `Event`, `EventKind`, `Diagnostic`, `ExitReason`, `Status`, `FaultCode` or
`Rect` is a compile error.

`Rect` is also predeclared (spec 0011, W3), as a record of four `int` fields (size 32 at
`abi::layout::rect`). It is the geometry value the window service returns; it is an
ordinary value that can be declared, copied and passed to `stdlib` helpers, and it holds
no reference to a window.

```text
type Rect = record        // offsets, size 32
  x: int;                 // 0
  y: int;                 // 8
  w: int;                 // 16
  h: int;                 // 24
end;
```

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
- UTF-8-specific operations (`utf8_len`, `utf8_decode`, `utf8_valid`, `utf8_decode_into`)
  live in `stdlib` and take a `string` (or `bytes` for the validator). The one `core`
  primitive that straddles the invariant is `core.substr_into`: its destination kind
  decides the check, so `string` → `string` and `bytes` → `string` are validated while a
  `bytes` destination copies raw data (T4). `bytes` → `string` through `substr_into` is
  the checked conversion T3 names.
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

The realtime class is reached through two more primitives (spec 0007, ADR 0016):

```text
core.realtime_on() -> Status
core.heartbeat()
```

`realtime_on` is one-way: `Ok` when the task is promoted (or already realtime, in which
case no second admission slot is taken), `Full` when every online core already holds a
realtime task. `heartbeat` has no result and is a no-op on a normal task; a realtime task
must call it at least once per second of non-blocked time or the kernel terminates it with
`FaultCode.WatchdogTimeout`.

### Application files (M2.5.1)

Applications read the read-only application image through three `core` builtins (spec
0013, ADR 0027):

```text
core.fs_list(path: const string[0]; index: uint;
             var name: string[64]; var kind: uint; var size: uint) -> Status
core.fs_read(path: const string[0]; offset: uint;
             var dst: bytes[256]; var read: int) -> Status
core.fs_size(path: const string[0]; var size: uint) -> Status
```

They are host builtins (41–43), not opcodes. The M2.5.1 view is the **root directory
only**: `fs_list` accepts `""` or `"."`, and `fs_read`/`fs_size` accept one root
component with no `/`. Paths use the same syntax as `core.spawn` (relative, at most 256
bytes, no leading/trailing `/`, no empty or `..` component); a malformed path is
`Invalid` and an over-long one is `CapacityExceeded`. A path with a separator addresses
a subdirectory, which is unsupported and also `Invalid` — not a silent `NotFound`.

`fs_list` returns entry `index` in directory order (not sorted), excluding `.` and `..`;
`NotFound` past the end. `kind` is `0` regular file, `1` directory and `2` other; an
entry name longer than 64 bytes is `CapacityExceeded`. `fs_read` copies up to 256 bytes
and is an `Ok` with zero bytes at or past the end of the file: it sets both the
destination's runtime length and `read` to the copied count, so the caller loops on
`read = 0`. The destination must be a `var bytes[N]` with `N >= 256` (a compile check),
and a missing name is `NotFound`, a directory `Invalid`. `fs_size` returns the same
inode byte size the kernel's loader uses. Every failure leaves all out-parameters
untouched; there is no allocation and no handle. The exact error table is spec 0013,
F14.

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
syslib.cpu_id() -> uint;
```

`port_in8` returns the byte as a `uint`; the low 16 bits of `port` are the x86 port number.
`port_out8` returns `0`, or `-4` (`Status.Invalid`) without touching the port when
`value > 255`. `cpu_id` returns the APIC id of the core executing the call: a task is
migratable, so it reports "where am I now", never an affinity (ADR 0011). `Port`,
`MmioRegion` and the `mmio_*`/DMA primitives above remain the target design; `syslib.*`
names other than these three are compile errors for now. There is still no capability check
(H4).

**Staged graphics surface (spec 0009).** The desktop milestone adds the shadow-screen
drawing primitives. They are the `syslib` namespace because they reach the machine's
screen; all rasterisation happens in the kernel, so an application pays one builtin call
per shape, not one per pixel.

```text
syslib.screen_width() -> uint            // 0 when graphics are disabled
syslib.screen_height() -> uint           // 0 when graphics are disabled
syslib.fill_rect(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.fill_dither(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.draw_rect(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.draw_text(x: int, y: int, text: const string[0], color: uint) -> Status
syslib.get_pixel(x: int, y: int) -> uint
syslib.present(x: int, y: int, w: int, h: int) -> Status
syslib.clear(color: uint) -> Status
```

Colours are `0x00RRGGBB`; coordinates are pixels with the origin at the top-left.
`w <= 0`, `h <= 0` or a colour with a bit above bit 23 is `Status.Invalid` and draws
nothing; coordinates outside the screen are clipped and are not an error. `fill_dither`
writes `color` to every pixel `(X, Y)` with `(X + Y)` even and leaves the rest untouched;
`draw_rect` is a one-pixel outline inside the extent; `draw_text` draws 8×8 glyphs from
the console font with a transparent background, one byte per cell, no newline handling and
a blank cell for a byte outside `0x00..=0x7F`; `get_pixel` returns `0` outside the screen;
`present` copies the rectangle from the shadow to the framebuffer and `clear` fills the
whole screen and presents it. Drawing primitives other than `clear` do not present. When
graphics are disabled the size is `0x0`, every primitive is `Invalid` and `get_pixel` is
`0` (spec 0009, G15).

**Staged input surface (spec 0010).** The pointer is a machine-wide value, not a device
handle. These are the only primitives an application has for it:

```text
syslib.pointer_x() -> int
syslib.pointer_y() -> int
syslib.pointer_buttons() -> uint      // bit 0 left, bit 1 right, bit 2 middle
syslib.pointer_source() -> uint       // 0 none, 1 PS/2, 2 USB tablet
syslib.inject_pointer(dx: int, dy: int, buttons: uint) -> Status
```

`pointer_x`/`pointer_y` are screen pixels, origin top-left; `pointer_buttons` is the
current mask; `pointer_source` names the device currently driving the machine-wide
pointer (spec 0010, I27). `inject_pointer` is **debug-only**: it is the deterministic test
hook, it moves the pointer by screen deltas with the same clamping as a real report (`dy`
positive moves down), sets the mask, and returns `Status.Invalid` without changing
anything when `buttons > 0b111`. It does not expose the device, and there is no way for
an application to read raw mouse or keyboard bytes.

**Event queue (spec 0010, I18/I19).** `EventKind` and `Event` are predeclared alongside
`Diagnostic`; the event surface is:

```text
syslib.next_event(var ev: Event) -> Status    // blocks until one is queued
syslib.poll_event(var ev: Event) -> Status    // WouldBlock when empty
syslib.inject_event(ev: Event) -> Status      // debug-only test hook
syslib.event_drops() -> uint                  // newest events dropped, monotonic
syslib.event_flush() -> uint                  // discard the queue, return the count
```

The queue is machine-wide; a task that calls `next_event` blocks in `WaitFor::Input` and
is woken by an insertion on any core. The keyboard still feeds the console byte stream
(`core.read_line`) unchanged: one key byte produces the console byte and a key event.

**Window surface (spec 0011, M2.3.1).** `Rect` is predeclared, so geometry is returned
through a value instead of a row of out-parameters:

```text
syslib.window_open(x: int, y: int, w: int, h: int, title: const string[0],
                   var id: int) -> Status
syslib.window_close(id: int) -> Status
syslib.window_raise(id: int) -> Status
syslib.window_move(id: int, x: int, y: int) -> Status
syslib.window_frame_rect(id: int, var rect: Rect) -> Status
syslib.window_content_rect(id: int, var rect: Rect) -> Status
syslib.window_focus() -> int
syslib.window_present(id: int) -> Status
syslib.window_info(index: uint, var id: int, var rect: Rect,
                   var title: string[64]) -> Status
```

`(x, y)` is the frame origin and `(w, h)` the content size; the service derives the frame
(content plus a 1-pixel border and a 16-pixel title bar) and both rectangles are readable
through the predeclared `Rect`. Window ids are `int`
with `0` invalid, assigned by the service and never reused. `open` reports `Invalid` for a
refused geometry or disabled graphics and `Full` at 16 windows; `close`/`move` are
owner-only (`Invalid` for another task's window, `NotFound` for an unknown id) while
`raise` is allowed to any task; the rect reads report `NotFound`/`Invalid`. An
out-parameter is written only on `Ok`. `window_present` presents the window's content
clipped to its visible parts (the sanctioned presentation path for a window client;
`syslib.present` remains a generic full-screen primitive) and is owner-only. `window_info`
is the read-only enumeration added in M2.3.4 (spec 0012): index `0` is the topmost live
window, and an index past the end is `NotFound` with every out-parameter untouched. The
title destination must be a `var string` of at least 64 bytes (compile-checked); titles
are truncated to that cap by the window service. When graphics are disabled, `open` and
the rect reads are `Invalid`, `close`/`raise`/`move` are `NotFound`, `window_info` is
`NotFound` and `window_focus` is `0`. The window model is fixed by spec 0011 and ADR 0024;
no opcode is added.

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
`sleep_until`, realtime promotion and `heartbeat`, `run`/`spawn`/`wait`/`kill`, read-only
file access, byte and string copy/compare/find, integer and `f64` math, formatting
primitives (integer and fixed-point float), and the checked conversions. `stdlib` (written in the language)
provides arrays, rings, maps, UTF-8 helpers, parsing (including decimal `f64`), formatting,
typed stream views, and the UI layout/chrome and controls layers. `syslib` (native, Rust) provides
ports, MMIO handles, the drawing primitives and future block/DMA access.

A function moves from `stdlib` to `core` only when measurement justifies it; the namespace
is the signal.

### Implemented first slice

The compiler embeds `stdlib/stdlib.ig` (`include_str!`) and compiles it into the
application's package when the application says `uses stdlib;` (source-only, ADR 0019; spec
0008, "Multi-unit compilation"). The library imports `core` and `syslib` (ADR 0026),
declares no `state`, and uses no new opcodes or builtins. Its top-level names are
**qualified** from the application (`stdlib.trim_into`) and unqualified inside the unit.

Capacities are concrete because the language has no generics; the numbers below are the
declared capacities and the at-least-N rule applies to callers.

| Helper | Semantics |
|--------|-----------|
| `function stdlib.trim_into(dst: var string[256]; src: const string[0]): int` | Copies `src` without leading or trailing space, tab, CR or LF. Returns the copied length, or `-6` (`Status.CapacityExceeded`) without writing when the trimmed result exceeds the destination capacity. |
| `function stdlib.split_once(dst_head: var string[128]; dst_tail: var string[256]; src: const string[0]; sep: byte): int` | Splits at the first `sep`: returns `1` with both parts, or `0` with the whole source in `dst_head` and an empty `dst_tail`. A part that does not fit returns `-6` without a partial tail write. Uses `core.find` and `core.substr_into`. |
| `function stdlib.parse_int_into(src: const string[0]; value: var int): Status` | Optional leading `-`, then decimal digits only. `Invalid` for empty text, a bare `-`, a `+` sign or any non-digit; `CapacityExceeded` when the magnitude does not fit an `int`; `-9223372036854775808` is accepted. Writes nothing on failure and never faults. |
| `function stdlib.parse_f64_into(src: const string[0]; value: var f64): Status` | Decimal text in `[+-]? ( digits [ '.' digits? ] \| '.' digits ) ( [eE] [+-]? digits )?` form, or the exact strings `inf`, `+inf`, `-inf`, `nan`. `Invalid` (nothing written) for anything else; overflowing magnitudes become `±Inf` and underflows `±0`, both with `Ok`; `-0.0` keeps its sign. Fixed-point precision, repeated ×/÷10 scaling; correctly rounded parsing stays open (Q41). |
| `function stdlib.starts_with(src: const string[0]; prefix: const string[0]): bool` | Byte-wise prefix test (not a code-point test). |
| `function stdlib.contains_byte(src: const string[0]; value: byte): bool` | True when `core.find` finds the byte. |
| `function stdlib.drain_into(dst: var array[int, 8]; count: var int; src: var ring[int, 8]): Status` | Moves up to eight integers out of `src`, oldest first, and writes the moved count. `NotFound` (nothing written) when `src` is empty. The first helper whose signature carries a `ring`. |
| `function stdlib.utf8_decode_into(dst: var string[256]; src: const bytes[0]): Status` | T3's checked conversion of the whole source. `CapacityExceeded` (nothing written) when `core.len(src) > 256`; otherwise the conversion runs through `core.substr_into`, where `-4` (`Status.Invalid`) means the bytes are not strict UTF-8 and nothing is written. Valid input yields `Ok` and exactly the source bytes as a `string`. |
| `function stdlib.utf8_len(src: const string[0]): int` | Number of code points: the bytes that are not continuation bytes (`0x80..0xBF`). The source is a valid `string`, so the count is a byte scan (T5: capacities stay in bytes). |
| `function stdlib.utf8_decode(src: const string[0]; index: int; codepoint: var int; next: var int): Status` | Decodes the code point that starts at `index`. `Invalid` and nothing written when `index < 0`, `index >= len`, the byte at `index` is a continuation byte, or the sequence would run past the end; otherwise writes the scalar (`0..=0x10FFFF`) and the index just past it and returns `Ok`. A `string` source needs no re-validation, only the bounds checks. |
| `function stdlib.utf8_valid(src: const bytes[0]): bool` | Strict UTF-8 validator for raw bytes, matching the interpreter's rule exactly: 1- to 4-byte sequences only, no overlong encodings, no surrogates, nothing above `U+10FFFF`, no truncated sequence, no stray continuation byte. It checks bytes without converting them and cross-checks `utf8_decode_into`'s primitive. |
| `function stdlib.recv_exact(ep: const in_stream[1]; dst: var bytes[256]; n: int): Status` | Reads exactly `n` bytes into `dst[0..n]` and sets the destination length to `n`. `n < 0` → `Invalid`; `n > 256` → `CapacityExceeded`; both read nothing. `Closed` (or a zero receive) returns `Closed` with `dst`'s length unchanged. Bytes are staged in a local `bytes[256]` and published with one copy, so no partial value is ever visible. One byte per receive (see spec 0006, "Receive"). |
| `function stdlib.send_frame(ep: const out_stream[1]; text: const string[0]): Status` | Sends `[u32 little-endian length][payload]` as one atomic send. A payload over 252 bytes (256 − 4) returns `CapacityExceeded` and sends nothing; the endpoint's own capacity is the other bound and `core.send` enforces it. |
| `function stdlib.recv_frame_into(ep: const in_stream[1]; dst: var bytes[256]): Status` | Reads the 4-byte little-endian length with `recv_exact`, then the payload. A length above 256 returns `CapacityExceeded` and leaves the payload bytes queued: the stream is desynchronised and the caller must stop using it. `Closed`/a short read propagate with `dst` untouched. |
| `function stdlib.send_line(ep: const out_stream[1]; text: const string[0]): Status` | Sends `text` plus one `\n` as one atomic send. Text over 255 bytes returns `CapacityExceeded` and sends nothing. |
| `function stdlib.recv_line_into(ep: const in_stream[1]; dst: var bytes[256]): Status` | Accumulates one `\n`-terminated line and strips the terminator and a preceding CR (CRLF). A line over 256 bytes keeps consuming to its LF (discarding the excess, so the stream stays synchronised) and returns `CapacityExceeded` with `dst` untouched; `Closed` before the LF returns `Closed` with `dst` untouched. |
| `function stdlib.send_u64(ep: const out_stream[1]; value: int): Status` | Sends exactly 8 little-endian bytes (two's complement, so negatives round-trip) as one atomic send. |
| `function stdlib.recv_u64(ep: const in_stream[1]; value: var int): Status` | Reads exactly 8 little-endian bytes with `recv_exact` and stores the signed value. `Closed`/`CapacityExceeded` propagate and leave `value` untouched. |
| `function stdlib.send_bytes(ep: const out_stream[1]; data: const bytes[0]): Status` | Sends any `bytes` buffer as one atomic send. The `const bytes[0]` capacity accepts a buffer of any size (at-least-N rule); the endpoint capacity still bounds the send. |

#### UI layout and chrome (M2.4.0)

The UI layer is ordinary language code over the nine `syslib` drawing primitives (ADR
0026). It is **client** chrome: the window service still draws the real window frames
(spec 0011), while an application uses these helpers for a dialog, panel or control inside
its own content, or for a self-drawn title bar. No record, sum, user enum, generic or
`state` is involved; every argument is a scalar or a buffer with the capacity shown, and
a destination follows the at-least-N rule. A drawing helper returns the first `syslib`
`Status` it receives (`Ok`, or `Invalid` on disabled graphics or invalid geometry).

**Metrics.** The font is 8×8 and drawn byte-per-cell by `syslib.draw_text` (G13).

| Helper | Value |
|--------|-------|
| `stdlib.ui_text_width(text: const string[0]) -> int` | `utf8_len(text) * 8`: one 8-pixel cell per code point. |
| `stdlib.ui_text_height() -> int` | `8`: one glyph row. |
| `stdlib.ui_line_height() -> int` | `16`: one text row plus leading, matching the title bar. |
| `stdlib.ui_content_inset() -> int` | `8`: the standard content/control margin. |
| `stdlib.ui_title_bar_height() -> int` | `16`: the title bar height. |
| `stdlib.ui_close_box_size() -> int` | `11`: the close box's side. |
| `stdlib.ui_text_center_x(x: int; w: int; text: const string[0]) -> int` | `x + (w - width) / 2`, or `x` when the text is at least as wide as `w` (left aligned). Integer division, both operands positive in the centred case. |
| `stdlib.ui_text_center_y(y: int; h: int) -> int` | `y + (h - 8) / 2`, or `y` when `h <= 8` (top aligned). |

`ui_text_width` counts code points, while `draw_text` advances one cell per **byte** and
draws a blank cell for a byte above `0x7F`; for ASCII the two agree. The 8×8 font covers
ASCII only, so a non-ASCII byte still renders blank (M2.6.1 owns a proportional font and
glyph metrics).

**Palette.** `stdlib` constants are not exported across a unit boundary, so the palette is
a set of functions returning `uint` in `0x00RRGGBB` form. The values are the System 7.5
palette used by the window service and the desktop:

| Helper | Value | Use |
|--------|-------|-----|
| `stdlib.ui_black() -> uint` | `0x000000` | text, borders, chrome ink, dither |
| `stdlib.ui_white() -> uint` | `0xFFFFFF` | content and desktop base, focused title background |
| `stdlib.ui_gray() -> uint` | `0xCCCCCC` | unfocused title background, disabled control face |
| `stdlib.ui_focus() -> uint` | `0xFFFFFF` | the focused title background (the named semantic value; equal to white) |
| `stdlib.ui_accent() -> uint` | `0x000080` | selection/highlight accent |

**Drawing.** `(x, y)` is the top-left pixel of the named rectangle, `w` and `h` are its
width and height, and the `syslib` geometry rules apply (an extent at or below zero or a
disabled screen gives `Invalid`, and drawing is clipped, never faulted).

| Helper | Semantics |
|--------|-----------|
| `stdlib.ui_frame(x: int; y: int; w: int; h: int) -> Status` | A 1-px black outline inside the extent (the `syslib.draw_rect` rule); the interior is untouched. |
| `stdlib.ui_title_bar(x: int; y: int; w: int; title: const string[0]; focused: bool) -> Status` | Paints a title bar inside a frame whose origin is `(x, y)` and whose width is `w` (the caller has drawn the 1-px frame with `ui_frame`; the helper never paints the outer border). Interior rows `y+1..y+14` are filled white when focused and `0xCCCCCC` when not; a focused bar gets one black stripe on every other row from `y+2` while the row is below `y+15`. An 11×11 close box is filled with the bar background and outlined black at `(x+4, y+3)` when the interior is at least 14 px wide. The title is truncated to the band right of the close box (`w - 22` pixels, at most 64 bytes, cut at a code-point boundary), its band background is repainted so stripes never cross a glyph, and the text is centred at `y+4` with `ui_text_width`. A separator at `(x+1, y+15)` width `w-2` closes the bar. `w < 3` is `Invalid` and draws nothing; a narrow bar simply omits the box or the title. |
| `stdlib.ui_close_box(x: int; y: int) -> Status` | An 11×11 black outline at `(x, y)`; the background is untouched, so the caller chooses the fill. `ui_title_bar` calls it after painting the close box's background. |
| `stdlib.ui_separator(x: int; y: int; w: int) -> Status` | One black row `(x, y, w, 1)`. |
| `stdlib.ui_center_text(x: int; y: int; w: int; h: int; text: const string[0]; color: uint) -> Status` | Draws `text` at `ui_text_center_x` / `ui_text_center_y` inside the extent, with `syslib.draw_text` (transparent background). It is left/top aligned when the text does not fit horizontally or the row does not fit vertically, so it never produces a negative offset. |

The title staging buffer is 64 bytes, the window service's title capacity; a longer title
is truncated. Text is drawn one cell per byte with the existing font, so a title containing
non-ASCII bytes shows blank cells for those bytes. When graphics are disabled, every
drawing helper returns `Invalid` and writes nothing; metrics and palette still return their
values. `langhost` and the test hosts use that disabled-screen model, so a sample can
branch on `syslib.screen_width() = 0` deterministically.

#### String editing and selection (M2.4.1)

Code-point-safe editing of a 256-byte text buffer, as ordinary language code over
`core.len`, `core.substr_into`, buffer indexing and `utf8_len`. There is no new opcode,
builtin, record, sum, user enum or `state`. Every `var string[256]` destination follows the
at-least-N rule and the helpers' working capacity is 256 bytes; a caller buffer whose
runtime length is already above 256 is refused with `CapacityExceeded` before anything is
read or written. Positions are byte offsets, and the boundary, atomicity, capacity, cursor
and selection rules are SE1–SE7 above.

| Helper | Semantics |
|--------|-----------|
| `stdlib.str_boundary(text: const string[0]; index: int) -> bool` | True when `index` is 0, the length, or the first byte of a code point; false for a negative or past-the-end offset. |
| `stdlib.str_cursor_clamp(text: const string[0]; cursor: int) -> int` | Clamps to `0..len` and to a boundary: negative → 0, past the end → the length, inside a code point → that code point's first byte. A boundary is returned unchanged. |
| `stdlib.str_cursor_left(text: const string[0]; cursor: var int) -> int` | Clamps, moves back one whole code point (0 stays 0), writes and returns the new cursor. |
| `stdlib.str_cursor_right(text: const string[0]; cursor: var int) -> int` | Clamps, moves forward one whole code point (the length stays the length), writes and returns the new cursor. |
| `stdlib.str_cursor_home(cursor: var int) -> int` | Sets the cursor to 0 and returns it. |
| `stdlib.str_cursor_end(text: const string[0]; cursor: var int) -> int` | Sets the cursor to the runtime length and returns it. |
| `stdlib.str_insert_into(text: var string[256]; at: int; insert: const string[0]) -> Status` | Inserts `insert` at boundary `at` (`0` prepends, the length appends). `Invalid` (nothing changed) for a negative or past-the-end offset or a non-boundary; `CapacityExceeded` (nothing changed) when the result would exceed 256 bytes. Empty insertion at a valid boundary is `Ok` and changes nothing. |
| `stdlib.str_delete_into(text: var string[256]; at: int; count: int) -> Status` | Deletes whole code points in `[at, at + count)`; both ends must be boundaries. `Invalid` (nothing changed) for a negative offset or count, an offset past the end, a range past the end or either end inside a code point. `count = 0` is `Ok` and changes nothing. Deletion never grows the text, so `CapacityExceeded` only reports an incoming length above 256 (SE4). |
| `stdlib.str_backspace(text: var string[256]; cursor: var int) -> Status` | Deletes the code point before the clamped cursor and moves the cursor to that code point's first byte. At 0 it is `Ok` and changes nothing (SE7). The text is untouched on failure; the cursor is written only on success (apart from the documented clamp). |
| `stdlib.str_selection_normalize(text: const string[0]; start: var int; stop: var int) -> int` | Clamps both scalars to boundaries, orders them (`start <= stop`) and returns the selection length in bytes, 0 when collapsed. |
| `stdlib.str_selection_all(text: const string[0]; start: var int; stop: var int) -> int` | Sets `start = 0` and `stop = len`, returning the length. |
| `stdlib.str_selection_collapse(text: const string[0]; start: var int; stop: var int; to_stop: bool) -> int` | Collapses to the `stop` end when `to_stop`, otherwise to the `start` end; clamps the chosen value, sets both scalars to it and returns the caret. Safe on an unnormalized pair. |
| `stdlib.str_selection_replace(text: var string[256]; start: var int; stop: var int; insert: const string[0]) -> Status` | Normalizes the pair, then replaces the selection with `insert` in one publish. On success the pair collapses to the caret just past the inserted text. `CapacityExceeded` (text and both scalars untouched) when the result would exceed 256 bytes; the staging path is `str_insert_into`'s, so no partial buffer is visible. |

The M2.4.1 proof is `samples/apps/text_edit`: it exercises ASCII and 1-, 2-, 3- and
4-byte code points, every rejection path, the capacity boundary, the clamp rule and the
selection helpers, prints one line per check and ends `text_edit: ok`. It runs in the
kernel (`run text_edit`) and on `langhost` with the same transcript.

#### Application buffers above 256 bytes (M2.5.3)

The M2.4.1 mutators keep their fixed 256-byte working capacity: there are no generics, and
the checked slice's destination capacity is a call-site immediate, so a library mutator
serves exactly one size. The text editor (`samples/apps/editor`) is the first caller that
needs a larger document: it holds one `string[8192]` and implements one **app-local**
mutator with the same rules rather than widening the library:

- both ends of the replaced range must be code-point boundaries (SE2), and the inserted
  text is itself a valid `string`;
- the result is staged in a local `bytes[8192]` and published with one
  `core.substr_into` (strict UTF-8), so a rejected edit leaves the document
  byte-identical (SE3);
- an over-capacity result returns `CapacityExceeded` and writes nothing (SE4 applied to
  the app's 8192-byte capacity);
- the read-only helpers (`str_boundary`, `str_cursor_clamp`/`left`/`right`/`home`/`end`,
  `str_selection_normalize`/`all`/`collapse`) take `const string[0]` and apply unchanged
  to the larger buffer.

8192 bytes is an application constant, not a language limit: `string` capacities stay
compile-time and any capacity the app declares works. The editor checks a file's size
before reading and refuses anything larger with a status line instead of truncating. There
is no writable filesystem in M2, so the editor's Save is disabled and a save attempt only
reports "read-only" (spec 0013, open question 2 remains open).

The editor's proof is `run editor check` (line/addressing logic, every load error class,
insert/delete/backspace/Enter, a 2-byte code point, the capacity and invalid-UTF-8
refusals, scrolling, an injected routed key and drawing pixels) and `editor: host ok` on
`langhost`; `run editor shot` holds a frame for the QMP screenshot harness.

#### UI controls (M2.4.2)

Drawing, hit-testing and small state-transition helpers for buttons, checkboxes, text
fields, vertical scrollbars and lists, as ordinary language code over the M2.4.0 metrics,
palette, `ui_frame` and the `syslib` drawing primitives. There is no new opcode, builtin,
record, sum, user enum, generic or `state`. The **application** owns the event loop and
every state scalar (`pressed`, `checked`, the text buffer, the cursor, the selection, the
scroll position, the list selection); the library stores nothing, takes no callback and
runs no event pump.

**Conventions.** The outer `(x, y, w, h)` is the control's full extent and the border is
inside it, so a hit test is inclusive of `(x, y)` and exclusive of `(x + w, y + h)`. The
focused state is a dotted one-pixel black ring inset **inside** the control's own extent
(`ui_focus_ring`), distinct from the solid border and never drawn outside the control
(there is no per-task clip in M2). A drawing helper returns the first `syslib` status it
receives: `Ok`, or `Invalid` with nothing drawn when graphics are disabled; an extent too
small for the control's border, padding and one glyph row returns `Invalid` before any
paint (`ui_button`/`ui_text_field` need `w >= 9` and `h >= 10`, `ui_scrollbar` needs
`w >= 5` and `h >= 5`, `ui_list` needs `w >= 4` and `h >= 4`). Drawing is clipped by
`syslib` to the screen and never faults. Pure helpers (hit tests, clamps, transitions and
the key decoders) keep working with a disabled screen. Labels and list items are cut at a
code-point boundary and at most 64 bytes.

**Key decoding.** `Event.code` packs `text | modifiers << 8 | key code << 16` (spec 0010)
and the language has no bitwise operators, so the helpers use `div`/`mod` over `uint`.
The application decides which control receives an event:

| Packed field / key | Helper | Mapping used by the control model |
|---|---|---|
| text, bits 0–7 | `ui_key_text(code) -> byte` | printable `0x20..=0x7E` inserts at the selection (through `str_selection_replace`) |
| modifiers, bits 8–11 | `ui_key_modifiers(code) -> int` | Shift bit 1, Ctrl 2, Alt 4, Caps 8; Shift+Tab is `ui_focus_next(current, count, -1)` |
| key code, bits 16+ | `ui_key_code(code) -> int` | stable identity; compare or use a predicate |
| Tab `0x102` | `ui_key_is_tab` | next focus (`ui_focus_next` with `+1`) |
| Enter `0x103` | `ui_key_is_enter` | activate the focused button or toggle the focused checkbox |
| Backspace `0x101` / Delete `0x119` | `ui_key_is_backspace` / `ui_key_is_delete` | edit the text field (`str_backspace` / `str_delete_into`) |
| Left `0x110` / Right `0x111` | `ui_key_is_left` / `ui_key_is_right` | move the text-field cursor (`str_cursor_left`/`right`) |
| Up `0x112` / Down `0x113` | `ui_key_is_up` / `ui_key_is_down` | list selection (`ui_list_move`) or scrollbar page |
| Home `0x114` / End `0x115` | `ui_key_is_home` / `ui_key_is_end` | text-field cursor to 0 / the length |

| Helper | Semantics |
|--------|-----------|
| `stdlib.ui_rect_hit(x, y, w, h, px, py) -> bool` | The shared hit-test rule: positive extent, `x <= px < x + w`, `y <= py < y + h`. |
| `stdlib.ui_focus_ring(x, y, w, h) -> Status` | A dotted one-pixel black ring on the perimeter of the extent (one pixel every two positions per edge). `w < 1` or `h < 1` is `Invalid`; callers inset it so it never leaves the control. |
| `stdlib.ui_focus_next(current, count, delta) -> int` | The next focus index for `count` controls, wrapping in both directions. `count <= 0` returns 0; a stale `current` is wrapped first. |
| `stdlib.ui_button(x, y, w, h, label, pressed, focused) -> Status` | White face (gray while pressed), one-pixel black border, centred label cut to the interior, a dotted inset ring while focused. A pressed label is inset by one pixel when the face is large enough. |
| `stdlib.ui_button_hit(x, y, w, h, px, py) -> bool` | `ui_rect_hit` over the button's full extent. |
| `stdlib.ui_button_pressed(pressed, inside) -> bool` | The press/release transition: true when a release lands inside a held button, i.e. the click activates it. |
| `stdlib.ui_checkbox(x, y, label, checked, focused) -> Status` | An 11×11 white box with a black outline, a seven-pixel check mark when checked and the label six pixels to the right, centred on the box. Focused draws the dotted ring around the control's full extent before the box. |
| `stdlib.ui_checkbox_hit(x, y, px, py) -> bool` | The 11×11 box only (the signature has no label width). |
| `stdlib.ui_checkbox_toggle(checked) -> bool` | The new checked value after a click (`not checked`). |
| `stdlib.ui_text_field(x, y, w, h, text, cursor, scroll, sel_start, sel_stop, focused) -> Status` | White interior, black border, the visible slice from the `scroll` byte offset (at most 64 bytes and `(w - 6) / 8` code points), an accent band with white text for the normalized selection clipped to the slice, a one-pixel caret at the clamped cursor when focused and visible, and a dotted inset ring. Never mutates `text`. |
| `stdlib.ui_text_field_reveal(text, cursor, scroll, width_px) -> int` | The scroll offset that keeps the clamped caret visible in `width_px` pixels: back to the caret when it is left of the scroll, forward so it sits at the right edge when it is beyond. |
| `stdlib.ui_scrollbar(x, y, w, h, position, visible, total, focused) -> Status` | A vertical white track with a black border and a gray thumb with a black outline. `position` is clamped to `0..max(total - visible, 0)`; the thumb is at least 8 pixels tall (clamped to the track) and moves over the remaining travel. Focused adds a dotted inset ring. |
| `stdlib.ui_scrollbar_clamp(position, visible, total) -> int` | Clamps `position` to `0..max(total - visible, 0)`; negative `total`/`visible` are treated as 0. |
| `stdlib.ui_scrollbar_hit(x, y, w, h, px, py, position, visible, total) -> int` | `0` outside, `1` above the thumb (page up), `2` below it (page down), `3` on the thumb, using the draw helper's geometry. The caller applies the page; thumb dragging is the M2.4 pointer-drag slice. |
| `stdlib.ui_list(x, y, w, h, items, count, selected, top, focused) -> Status` | A white interior, black border and one 16-pixel row per item from `top`; `count` is clamped to `0..8` and `top` to `0..max(count - rows, 0)`. The selected row is accent-filled with white text, other rows are black on white. Focused adds an inset ring. A list with no whole row still draws its frame. |
| `stdlib.ui_list_rows(h) -> int` | Whole item rows in a list of height `h`: `(h - 2) / 16`, or 0. |
| `stdlib.ui_list_hit(x, y, w, h, px, py, count, top) -> int` | The row index under the point, or `-1` outside, in the border or past the last item. `top` is clamped as `ui_list` clamps it. |
| `stdlib.ui_list_ensure_visible(selected, top, rows) -> int` | The `top` that keeps the selected row inside the `rows` visible rows (scroll up to it, or down so it is the last row), otherwise unchanged. |
| `stdlib.ui_list_move(selected, count, delta) -> int` | The selected row after a `delta` move, clamped to `0..count-1` without wrapping; `count <= 0` returns 0. |

The M2.4.2 proof is `samples/apps/ui_controls`: it opens a window, draws every control,
injects pointer and key events and checks the pressed face, the check mark, the caret, the
accent selection, the thumb travel, the list selection, the focus rings and the wrap with
`syslib.get_pixel`. It prints `ui_controls: ok` in the kernel on `-smp 4` and `-smp 1` and
`ui_controls: host ok` on `langhost`, where the pure helpers and the disabled-screen
`Invalid` contract still run. `langhost/tests/e2e.rs` also records the drawing sequence of
every control with a fake host. `ui_controls shot` holds a composed frame (focused text
field with a selection, checked box, paged scrollbar, selected list row) and
`scripts/ui_controls_screenshot_qmp.py` captures it through QMP `screendump` to
`/home/sinan/Desktop/igis-controls.png` (1024×768).

#### UI menus (M2.4.3)

A menu-bar model, pull-down drawing, hit-testing, mask helpers and a pure keyboard mapping,
as ordinary language code over the M2.4.0 metrics/palette and the `syslib` drawing
primitives. There is no new opcode, builtin, record, sum, user enum, generic or `state`.
The **application** owns the event loop and every state scalar: the open menu index
(`-1` closed), the highlighted item, and the checked/disabled masks. The library stores
nothing, takes no callback and runs no event pump; there is no command model.

**Menu bar.** `ui_menu_bar` draws a white bar of height 22 with a black bottom rule, one
label per title from `x + 8`, and a 16-pixel gap after each label. The active menu's label
cell (the title width plus 4 pixels on each side) is inverted black with white text; every
other label is black on white. `ui_menu_bar_hit` uses the same layout and returns the menu
index under a point, or `-1` in a gap, outside the bar or past the last title. `count` is
clamped to `0..8`; a title is cut at a code-point boundary to the remaining bar width (at
most 32 bytes). `w <= 0` is `Invalid`.

**Pull-down.** `ui_menu_dropdown` draws `count` 16-pixel rows at `(x, y)`: a white
interior, a 1-px black border, a check column at `x + 8`, the label at `x + 22` and the
shortcut right-aligned at `x + w - 8`. Height is `count * 16 + 2`, count is clamped to
`0..16`, and `w < 32` or an empty menu is `Invalid`. A row layout:

| State | Drawing |
|---|---|
| selected, enabled, `active` | accent-filled row; label and shortcut white |
| enabled | black label and shortcut |
| disabled | gray label, gray shortcut and gray check mark |
| checked | the seven-pixel check mark in the row's ink (white on the accent, black, or gray when disabled) |
| separator (label exactly `-`) | a 1-px black line inset 8 pixels; never highlighted, checked or selectable |

Labels are cut at a code-point boundary to the space left after the shortcut (at most 64
bytes); a shortcut that does not fit is omitted. `ui_menu_hit` returns the row under a
point, including a disabled row, or `-1` outside, in the border, past the last item or on a
separator. `ui_menu_selectable(label, disabled_mask, index)` is the shared rule: not a
separator and the disabled bit clear. Drawing is clipped by `syslib`, never faulted, so a
partly off-screen menu is safe.

**Masks.** Checked and disabled are independent `uint` bit sets, one bit per item index
`0..=63`: `ui_menu_bit(index)` returns the single-bit value (0 outside the range),
`ui_menu_has(mask, index)` tests it and `ui_menu_set(mask, index, value)` returns the mask
with the bit set or cleared. The language has no bitwise operators, so all three use
`div`/`mod` and powers of two. The highlight never changes either mask. A mask is
menu-scoped by the caller: an application with several menus keeps one checked mask per
menu (or passes `0` where no item is checkable), because item indices are local to the open
pull-down.

**State transitions.**

| Helper | Semantics |
|--------|-----------|
| `stdlib.ui_menu_bar_height() -> int` | 22, the bar height. |
| `stdlib.ui_menu_is_separator(label) -> bool` | True when the label is exactly `-`. |
| `stdlib.ui_menu_bit(index) -> uint` | The single-bit value for `0..=63`; 0 outside. |
| `stdlib.ui_menu_has(mask, index) -> bool` | True when the mask has the item's bit; false outside. |
| `stdlib.ui_menu_set(mask, index, value) -> uint` | The mask with the bit set or cleared; out-of-range is a no-op. |
| `stdlib.ui_menu_selectable(label, disabled_mask, index) -> bool` | Not a separator and not disabled. |
| `stdlib.ui_menu_first(count, items, disabled_mask) -> int` | The first selectable item, or -1 when there is none. |
| `stdlib.ui_menu_open(menu, count, items, disabled_mask, selected) -> int` | Returns `menu` (or -1 for a negative index) and sets `selected` to the first selectable item. |
| `stdlib.ui_menu_close() -> int` | -1, the closed menu index. |
| `stdlib.ui_menu_move(selected, count, delta, items, disabled_mask) -> int` | The highlighted item after `delta` moves, skipping separators and disabled items and clamping without wrapping. A stale or unselectable start snaps to the nearest selectable item in the direction of travel (or the other way); -1 when nothing is selectable. |
| `stdlib.ui_menu_key(code, open) -> int` | The keyboard action for a packed `Event.code` (below). |
| `stdlib.ui_menu_bar(x, y, w, titles, count, active) -> Status` | The bar. |
| `stdlib.ui_menu_bar_hit(x, y, w, titles, count, px, py) -> int` | The menu index under a point, or -1. |
| `stdlib.ui_menu_dropdown(x, y, w, items, shortcuts, count, selected, checked_mask, disabled_mask, active) -> Status` | The pull-down. |
| `stdlib.ui_menu_hit(x, y, w, items, count, px, py) -> int` | The item row under a point, or -1. |

**Key mapping.** `ui_menu_key` decodes `Event.code` through the M2.4.2 key decoders and
returns one of six app-facing action codes:

| Key | Action | When |
|-----|--------|------|
| Escape `0x100` | `6` close | always (closed is a harmless no-op) |
| Enter `0x103` | `5` activate | open only |
| Up `0x112` / Down `0x113` | `1` / `2` | open only |
| Left `0x110` / Right `0x111` | `3` / `4` | open only (switch the top-level menu; the app wraps with `ui_focus_next`) |
| anything else | `0` none | always |

The application applies the action: up/down call `ui_menu_move`, left/right call
`ui_menu_open` on the wrapped neighbour, activate calls its own item action and then
`ui_menu_close`, and close assigns `-1`. Disabled and separator items never become the
highlight, so activate can never reach them. Shortcut text in the right column is display
only in M2; the same M2.4.2 key decoders let an application add its own accelerators.

The M2.4.3 proof is `samples/apps/ui_menus`: it opens a window, draws File/Edit/View and
one pull-down, then uses injected pointer and key events to open a menu, move the
highlight across a separator and a disabled item, activate a checked item, toggle its
check mark, clamp on a disabled item, refuse a disabled click, close with Escape and
switch menus with Left/Right. It checks the bar, inverted cell, border, accent row, check
mark, separator and erased pull-down with `syslib.get_pixel`, prints `ui_menus: ok` on
`-smp 4` and `-smp 1`, and prints `ui_menus: host ok` on `langhost` where the pure helpers
and the disabled-screen `Invalid` contract run. `langhost/tests/e2e.rs` records the exact
bar/pull-down drawing sequence and the mask/key results. `ui_menus shot` holds View open
with two check marks, the disabled Zoom row and a separator;
`scripts/ui_menus_screenshot_qmp.py` captures `/home/sinan/Desktop/igis-menus.png`
(1024×768).

#### UI dialogs (M2.4.4)

A framed alert, a text-input dialog, word-wrapped message text and a pure modal key
mapping, as ordinary language code over the M2.4.0 metrics/palette/chrome, the M2.4.1
editing helpers and the M2.4.2 controls. There is no new opcode, builtin, record, sum,
user enum, generic or `state`. The **application** owns the modal loop
(`syslib.next_event`/`poll_event`), the focus slot, the result and the text buffer; the
library stores nothing, runs no event pump, takes no callback and never mutates the text.

**Layout.** Both dialogs take their full client extent `(x, y, w, h)`: a white interior, a
1-px `ui_frame`, a focused `ui_title_bar` (a modal dialog is focused by definition) and a
bottom-right button row of height 24.

| Helper | Semantics |
|--------|-----------|
| `stdlib.ui_dialog_button_width(label) -> int` | `max(72, ui_text_width(label) + 24)`. |
| `stdlib.ui_dialog_button_left(x, w, default_label, cancel_label, which) -> int` | The left edge of a dialog button; the cancel button is right-aligned `ui_content_inset` from `x + w` and the default button is `12` px to its left. `which` 0 is the default button and 1 the cancel button (any other value is treated as 0). |
| `stdlib.ui_dialog_button_top(y, h) -> int` | `y + h - 8 - 24`: the top of the button row. |
| `stdlib.ui_dialog_button(x, y, w, h, label, focused, is_default) -> Status` | `ui_button` at rest plus, when `is_default`, a solid inset black ring at `(x + 2, y + 2, w - 4, h - 4)` — the classic double border. The dotted focus ring from `ui_button` is one pixel inside it, so "default" and "focused" stay independent. |
| `stdlib.ui_message(x, y, w, h, text) -> Status` | Word-wrapped black text: whole 8-px rows on a 16-px line advance, at most `h / 16` lines, breaking at the last space that fits (`w / 8` cells per line) and hard-breaking an unbroken run; `\n` starts a new line. One byte is one cell (the font's rule); a hard break never splits a code point; text past the last whole line is dropped. `w < 8` or `h < 8` is `Invalid`. |
| `stdlib.ui_alert(x, y, w, h, title, message, default_label, cancel_label, focus) -> Status` | The alert: white interior, frame, focused title bar, the wrapped message between the title bar and the button row, and the right-aligned default/cancel buttons. `focus` 0 draws the default button's focus ring, 1 the cancel button's, anything else neither (the default keeps its double border). Minimum `h = 80` and `w = default_w + cancel_w + 12 + 16 + 2`. |
| `stdlib.ui_alert_hit(x, y, w, h, default_label, cancel_label, px, py) -> int` | The alert slot under a point: 0 default, 1 cancel, -1 outside. The title bar's close box also returns 1. A dialog too small to be drawn returns -1. |
| `stdlib.ui_input_dialog(x, y, w, h, title, prompt, default_label, cancel_label, text, cursor, scroll, sel_start, sel_stop, focus) -> Status` | The text-input dialog: a one-line prompt at `y + 24`, a `ui_text_field` at `y + 44` spanning `w - 16` and 22 tall, and the button row. `focus` 0 is the field, 1 the default button, 2 cancel; the field shows its caret and focus ring only while focused. Minimum `h = 106` and the alert's width rule. |
| `stdlib.ui_input_dialog_hit(x, y, w, h, default_label, cancel_label, px, py) -> int` | The input-dialog slot under a point: 0 field, 1 default, 2 cancel, -1 outside. The close box returns 2. Too-small geometry returns -1. |
| `stdlib.ui_dialog_key(code, has_cancel) -> int` | The modal action for a packed `Event.code` (below). |

**Focus and result codes.** Focus slots are dialog-local and equal the hit-test value:
0 is the default button in an alert and the text field in an input dialog, 1 is the default
button in an input dialog and the cancel button in an alert, and 2 is the input dialog's
cancel button. `ui_dialog_key` returns an action, and two of its values are also the result
codes the app closes with:

| Key | Action | Result |
|-----|--------|--------|
| Enter `0x103` | default/OK | 1 |
| Escape `0x100` | cancel when `has_cancel` | 2; 0 when the dialog has no cancel |
| Tab `0x102` | focus next | 3 |
| Shift+Tab | focus previous | 4 |
| anything else | none | 0 |

Result 0 means "none/open"; 1 and 2 are the closing results (default/OK, cancel); 3 and 4
only move the focus. Enter always takes the default action, whatever slot has the focus;
focused cancel is still cancelled by Escape, never by Enter. The app applies an action:
1/2 close the dialog with that result, 3/4 call `ui_focus_next(focus, count, ±1)` with
`count = 2` for an alert and 3 for an input dialog. A pointer press sets the focus slot
from the hit test and a release on the same slot activates it; slot 0 in an input dialog
places the caret at the end and never closes.

**Text ownership.** `ui_input_dialog` takes `text` as `const string[0]` plus the cursor,
scroll and selection scalars and only draws through `ui_text_field`; it never mutates the
buffer. The text therefore belongs to the caller on OK and is still unchanged on cancel
(the helper cannot have changed it); the app decides whether to discard it. Editing is
exclusively the M2.4.1 helpers; there is no second editing API. The `langhost` and
disabled-screen model still runs every pure helper (geometry, hit tests, key mapping,
`ui_message` guards) and returns `Invalid` from every drawing helper.

The M2.4.4 proof is `samples/apps/ui_dialogs`: it draws an alert with a two-line message
and a default/cancel pair, checks the frame, focused title bar, message lines, double
border and focus ring with `syslib.get_pixel`, moves the focus with Tab/Shift+Tab, takes
the default action with Enter and cancel with Escape and with a close-box click, then
opens a text-input dialog, injects printable text, Backspace, Home/End and Left/Right,
edits the buffer with `str_*`, checks the caret/glyph/selection pixels, cycles the focus
through field/OK/Cancel, commits with Enter and a pointer click and cancels with Escape
without touching the buffer. It prints `ui_dialogs: ok` on `-smp 4` and `-smp 1` and
`ui_dialogs: host ok` on `langhost`, where the pure helpers and the disabled-screen
`Invalid` contract run. `ui_dialogs shot` composes the alert and the input dialog and
`scripts/ui_dialogs_screenshot_qmp.py` captures `/home/sinan/Desktop/igis-dialogs.png`
(1024×768).

#### UI icons and desktop (M2.4.5)

A 32×32 monochrome icon, grid geometry, double-click timing, drag offsets, a selection
rectangle and a trash drop rule, as ordinary language code over the `syslib` drawing
primitives. There is no new opcode, builtin, record, sum, user enum, generic or `state`.
The **application** owns the bitmap assets, the cell assignment, the live/selected flags,
the drag offset and position, the last click's time and target, and the rubber-band
origin; the library draws and computes only.

**Icon bit layout.** An icon is `array[uint, 32]`: one row mask per row, row 0 the top
row, bit 0 the left pixel and bit 31 the right one. `ui_icon` draws one 1-pixel-high
`syslib.fill_rect` per maximal run of set bits, so a bitmap costs at most 64 calls, and an
all-zero row draws nothing. The caller paints any white plate or background it wants; the
helper draws only set bits, so the desktop dither shows through an unfilled stencil. A
colour above `0x00FFFFFF` is `Invalid` before anything is drawn, and `syslib` clips the
bitmap instead of faulting.

| Helper | Semantics |
|--------|-----------|
| `stdlib.ui_icon_size() -> int` | `32`, the bitmap and grid-icon side. |
| `stdlib.ui_icon_row_bit(row, col) -> bool` | Bit `col` of `row` (0 = left); a `col` outside `0..31` is false. `div`/`mod` with a doubling mask, because the language has no bitwise operators. |
| `stdlib.ui_icon(x, y, rows, color) -> Status` | Draws the 32×32 bitmap at `(x, y)`; clipped, `Invalid` for a bad colour or disabled graphics. |
| `stdlib.ui_icon_label(x, y, label) -> Status` | A black 8×8 label at `(x, y)`, transparent background (`syslib.draw_text`); the app centres it. |
| `stdlib.ui_icon_cell_x(col, origin_x, cell_w) -> int` | `origin_x + col * cell_w`. |
| `stdlib.ui_icon_cell_y(row, origin_y, cell_h) -> int` | `origin_y + row * cell_h`. |
| `stdlib.ui_icon_label_y(icon_y) -> int` | `icon_y + 32 + 2`, the top of the label under an icon. |
| `stdlib.ui_icon_grid_hit(x, y, cols, count, cell_w, cell_h, px, py) -> int` | Row-major cell index under a point: `(px - x) / cell_w` and `(py - y) / cell_h`, `row * cols + col`; -1 outside, past `count`, or for a non-positive `cols`, `count`, `cell_w` or `cell_h`. |
| `stdlib.ui_double_click(last_ns, now_ns, same_target) -> bool` | True when `same_target` and `now_ns - last_ns` is at most 400 ms (`UiDoubleClickNs`, inclusive); false for a different target, a clock that went backwards or a longer gap. No internal state. |
| `stdlib.ui_icon_grab(icon_x, icon_y, px, py, dx: var int, dy: var int) -> bool` | True only inside the 32×32 bitmap, writing `px - icon_x` and `py - icon_y`; outside, both offsets are 0, so a label press selects but does not drag. |
| `stdlib.ui_icon_follow(px, py, dx, dy, x: var int, y: var int) -> Status` | `x = px - dx`, `y = py - dy`; the app clamps the result to its own content. |
| `stdlib.ui_selection_normalize(x1, y1, x2, y2, x: var int, y: var int, w: var int, h: var int) -> Status` | An inclusive normalized band: `x`/`y` are the smaller ordinates and `w`/`h` the absolute differences plus one, so both endpoints are inside. |
| `stdlib.ui_selection_rect(x, y, w, h) -> Status` | The dotted 1-pixel black rubber-band outline (the `ui_focus_ring` pattern), transparent interior; `Invalid` below 1×1. |
| `stdlib.ui_rect_intersects(ax, ay, aw, ah, bx, by, bw, bh) -> bool` | Axis-aligned overlap of two positive rectangles; touching edges do not count, and a non-positive extent is false. |
| `stdlib.ui_icon_drop(x, y, cols, count, cell_w, cell_h, tx, ty, tw, th, px, py) -> int` | The release target: -2 inside the trash rectangle, otherwise the grid cell index or -1. |
| `stdlib.ui_trash_hit(x, y, w, h, px, py) -> bool` | The `ui_rect_hit` rule named for the trash drop target. |

**Grid.** The application chooses the origin, the column count, the cell size and the
number of valid cells; `ui_icon_cell_x`/`_y` place a cell and `ui_icon_grid_hit` maps a
point back, so drawing and hit-testing cannot disagree. Cell edges belong to the cell to
the right/below (the `ui_rect_hit` rule). `count` is the number of valid cells, which may
be larger than the number of live icons so an empty cell is still a drag/drop target.

**Double-click.** The app records the last press time (`core.clock()` nanoseconds) and
the last target itself and calls `ui_double_click` on each icon press; 400 ms is the
threshold and a same-target repeat at exactly 400 ms counts. When it returns true the app
opens something (or prints a marker) and resets the remembered time and target. A drag
cancels the pending click: a press that moved before release is not a double-click
candidate.

**Drag, selection and trash.** A press inside a live icon's bitmap starts a drag with the
offset from `ui_icon_grab`; every `PointerMove` with the button held updates the icon
origin through `ui_icon_follow`, the application draws the icon at the dragged position
and leaves its cell showing the desktop, and `PointerUp` asks `ui_icon_drop`: -2 removes
the icon (its live flag clears and the trash stays drawn), a cell index moves it when
that cell is empty or its own, and -1 leaves it where it was. A press on empty desktop
starts a rubber band: the app normalizes the origin and current point with
`ui_selection_normalize` and selects every live icon whose cell rectangle
`ui_rect_intersects`, drawing the band with `ui_selection_rect`. The library never
touches the window service; the app keeps every icon inside its content by clamping the
drag origin, and `syslib` clips any overhang.

The M2.4.5 proof is `samples/apps/ui_icons`: it opens a `Desktop Icons` window, draws a
dithered desktop with a 2×2 icon grid (folder, document, terminal) and a trash target,
checks the dither, three bitmaps, labels and the trash with `get_pixel`, double-clicks the
document (prints `ui_icons: open Document` and counts exactly one open), drags the folder
to the empty cell (source cleared, icon redrawn, cell recorded), drags it into the trash
(the drop removes it and the cell clears), rubber-band selects the two remaining icons
(accent label bands, removed folder excluded), checks the band outline and its removal,
presents and closes the window, and prints `ui_icons: ok` on `-smp 4` and `-smp 1` or
`ui_icons: host ok` on `langhost`, where the pure helpers and the disabled-screen
`Invalid` contract run. `ui_icons shot` composes the desktop with the document and
terminal selected and `scripts/ui_icons_screenshot_qmp.py` captures
`/home/sinan/Desktop/igis-icons.png` (1024×768).

Still missing for a complete standard library: `f32` (Q40) and the remaining float-text
work (Q41: correctly rounded decimal-to-binary parsing and its subnormal edges); the
fixed-point `core.format_f64_into`, the shortest round-trip
`core.format_f64_short_into` and `stdlib.parse_f64_into` have landed. The typed stream
views landed with the framing layer (`recv_exact`, `send_frame`, `recv_frame_into`,
`send_line`, `recv_line_into`, `send_u64`, `recv_u64`, `send_bytes`). The `substr_into`
move is withdrawn: the checked slice stays in `core` because the destination capacity must
be a call-site immediate, and the language has no generics (see "Slicing and search").
`map` support and `map_get_or` landed with the structural map table (spec 0003, "Maps");
the UTF-8 helpers landed as ordinary language code over `core.substr_into`, `core.len` and
buffer indexing, adding no opcode. The M2.4.0 UI layout/chrome layer landed as ordinary
language code over the `syslib` drawing primitives (`ui_text_width`, the metrics, the
palette, `ui_frame`, `ui_title_bar`, `ui_close_box`, `ui_separator`, `ui_center_text`),
adding no opcode or builtin. The M2.4.1 string editing layer landed as ordinary language code
over `core.substr_into`, `core.len`, buffer indexing and the UTF-8 helpers (`str_boundary`,
the cursor helpers, `str_insert_into`, `str_delete_into`, `str_backspace` and the selection
helpers), again with no opcode or builtin. The M2.4.2 controls layer landed as ordinary
language code over the same drawing primitives plus the M2.4.1 editing helpers
(`ui_rect_hit`, `ui_focus_ring`, `ui_focus_next`, the key decoders, the button/checkbox/
text-field/scrollbar/list helpers and their hit tests and transitions), again with no
opcode or builtin. The M2.4.3 menus layer landed as ordinary language code over the
metrics/palette and the mask/key helpers (`ui_menu_bar`, `ui_menu_dropdown`, the bar/item
hit tests, `ui_menu_bit`/`has`/`set`, `ui_menu_first`/`open`/`close`/`move`, `ui_menu_key`
and `ui_menu_selectable`), again with no opcode or builtin. The M2.4.4 dialogs layer landed
as ordinary language code over the same chrome, controls and editing helpers (`ui_message`,
the dialog button geometry, `ui_alert`, `ui_input_dialog`, their hit tests and
`ui_dialog_key`), again with no opcode or builtin. The M2.4.5 icons and desktop layer
landed as ordinary language code over the drawing primitives (`ui_icon`, the bit and grid
geometry helpers, `ui_double_click`, the grab/follow pair, the selection
normalize/outline/intersection helpers and the drop/trash rules), again with no opcode or
builtin. The M2.4.6 widgets demo (`samples/apps/widgets`) is the integration proof:
one 760×560 client panel composes a client menu bar with File/Edit/View pull-downs,
a button, a checkbox, a text field, a scrollbar over a 16-line document, an 8-row
list, a compact folder/document icon strip with a trash target, an alert and a
text-input dialog. It adds no helper, opcode or builtin; the app owns every state
scalar and the one event loop, and the dialogs are the same state machine (while a
dialog is open every key and pointer event goes to it). Check mode drives injected
pointer and key events and checks representative pixels, including a 2-byte `é`
edit through the M2.4.1 helpers, and prints `widgets: ok` in the kernel on `-smp 4`
and `-smp 1` or `widgets: host ok` on `langhost`; `widgets shot` composes the menu,
controls and input dialog for the QMP screenshot harness.

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
| `core.realtime_on` with no free core | `Status.Full`; the task stays normal |
| Realtime task misses its heartbeat | `fault(WatchdogTimeout)`; recovery rejected, task terminated and reclaimed |
| Buffers of different kinds compared (`string` = `bytes`) | Compile error |
| `core.substr_into` `string` destination that splits a code point or copies invalid UTF-8 | `-4` (`Status.Invalid`); destination untouched |
| `core.substr_into` `bytes` destination | Raw copy from either source kind; no UTF-8 check |
| `string`/`bytes` write past capacity | `fault(CapacityExceeded)`; no truncation, no growth |
| `ring` push on full, `map` put on full | `Status.Full` (non-fatal) or explicit failure, never growth |
| Invalid UTF-8 conversion | `Status.Invalid`; no invalid `string` can be constructed |
| Integer arithmetic overflow | `fault(Overflow)` |
| Integer division by zero | `fault(DivideByZero)` |
| Float division by zero | `Inf`/`NaN` per IEEE-754; not a fault |
| `int(f)`, `uint(f)` or `byte(f)` with `NaN` or out of range | `fault(Overflow)` |
| `core.format_f64_into` with `frac_digits > 9` | `-4` (`Status.Invalid`); destination untouched |
| `core.format_f64_into` with `\|value\| >= 2^63` | `-4`; destination untouched (the fixed-point form has no exponent) |
| `core.format_f64_into` of `NaN`/`±Inf` | `nan`/`inf`/`-inf`, ignoring the fraction width |
| `core.format_f64_into` result wider than the destination | `-6` (`Status.CapacityExceeded`); destination length untouched |
| `core.format_f64_short_into` of `NaN`/`±Inf` | `nan`/`inf`/`-inf`; no fraction width applies |
| `core.format_f64_short_into` result wider than the destination | `-6` (`Status.CapacityExceeded`); destination length untouched |
| `stdlib.parse_f64_into` of malformed text | `Invalid`; `value` untouched; never faults |
| `stdlib.parse_f64_into` of an overflowing or underflowing magnitude | `±Inf` or `±0` with `Ok`; not an error |
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
| Stream name outside `[a-z][a-z0-9_]*` or longer than 31 bytes | Compile error |
| Stream type outside a top-level `var` or a `const` parameter | Compile error |
| `send` larger than the stream capacity | `Status.CapacityExceeded`; writes nothing, never blocks |
| `try_send` on a full stream | `Status.WouldBlock` |
| `recv` into a `string`, or `recv` on an `out_stream` | Compile error |
| `recv` on an empty stream | Blocks; `try_recv` returns `-1` |
| `recv` on a closed, empty stream | `-2` (`Status.Closed`) |
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
| `stdlib.ui_*` drawing on a disabled screen | `Status.Invalid`; nothing drawn |
| `stdlib.ui_frame` with `w <= 0` or `h <= 0` | `Status.Invalid`; nothing drawn |
| `stdlib.ui_title_bar` with `w < 3` | `Status.Invalid`; nothing drawn; a narrower interior omits the box/title |
| `stdlib.ui_menu_bar` with `w <= 0` | `Status.Invalid`; nothing drawn |
| `stdlib.ui_menu_dropdown` with `w < 32` or an empty menu | `Status.Invalid`; nothing drawn |
| A partly off-screen menu bar or pull-down | Clipped by `syslib`; never faulted |
| A separator item | Never highlighted, checked or activated; a 1-px line |
| A disabled menu item | Gray, skipped by `ui_menu_move`/`ui_menu_first`, never activated |
| An out-of-range menu mask index or `ui_menu_bit(>= 64)` | False/0; `ui_menu_set` is a no-op |
| A `stdlib.ui_*` call with a record, sum or user enum argument | Compile error (aggregates still may not cross the unit boundary) |
| `stdlib.str_insert_into` at a negative/past-the-end offset or a non-boundary | `Status.Invalid`; text untouched |
| `stdlib.str_insert_into` or `str_selection_replace` whose result exceeds 256 bytes | `Status.CapacityExceeded`; text and selection untouched |
| `stdlib.str_delete_into` with a negative offset/count, a range past the end or an end inside a code point | `Status.Invalid`; text untouched |
| `stdlib.str_delete_into`/`str_insert_into`/`str_backspace` with an incoming text longer than 256 bytes | `Status.CapacityExceeded`; nothing read or written |
| A `stdlib.str_cursor_*` call with a negative, past-the-end or in-code-point position | Clamped to 0, the length or the code point's first byte; never `Invalid` |
| `stdlib.str_selection_replace` on an unnormalized pair | The pair is clamped and ordered first; no fault |
| `stdlib.ui_alert` or `ui_input_dialog` too small or on a disabled screen | `Status.Invalid`; nothing drawn |
| `stdlib.ui_alert_hit`/`ui_input_dialog_hit` on a dialog too small to be drawn | `-1` (the dialog draws nothing) |
| `stdlib.ui_message` with `w < 8` or `h < 8` | `Status.Invalid`; nothing drawn |
| A click in a dialog's title-bar close box | The cancel slot (1 in an alert, 2 in an input dialog) |
| Enter while a dialog's cancel button is focused | The default action (result 1), never cancel |

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
| 2026-11-15 | Stream endpoints implemented: `in_stream[N]`/`out_stream[N]` top-level state variables with the name as the stream name, `const` endpoints as parameters, `core.send`/`try_send` (atomic, `CapacityExceeded`/`WouldBlock`/`Closed`) and `core.recv`/`try_recv` (bytes destination only, staged `-1`/`-2` codes). Handles occupy the `state` prefix and the manifest lists the descriptors in the same order. Dynamic loading and its load errors are spec 0006. |
| 2026-11-15 | Typed stream views in `stdlib`: `recv_exact`, `send_frame`, `recv_frame_into`, `send_line`, `recv_line_into`, `send_u64`, `recv_u64` and `send_bytes`, all ordinary language code over `const ..._stream[1]` parameters (capacity `1` because the declared capacity of a stream parameter is not checked). Each publishes a whole value with one copy, so an error leaves the destination untouched. A receive is one byte wide: `core.recv` returns up to the destination capacity and there is no pushback, so a wider view would steal the peer's next message (spec 0006, "Receive"). |
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
| 2026-11-15 | UTF-8 slice implemented. `core.substr_into` accepts all four destination/source kind combinations: the destination kind decides the check (a `string` source keeps T4's code-point-boundary rule, a `bytes` source is validated as strict UTF-8, a `bytes` destination copies raw), encoded in the `SUBSTR` `b` field as the source kind with no new opcode. `stdlib` gains `utf8_decode_into` (T3, checked bytes→string), `utf8_len`, `utf8_decode` and `utf8_valid`. The `stdlib.substr_into` move is withdrawn: the destination capacity must be a call-site immediate, and without generics a library helper could only serve one capacity (recorded in `OPEN-QUESTIONS.md`). |
| 2026-11-15 | `syslib.cpu_id() -> uint` added (builtin 10): the APIC id of the executing core, the proof hook for per-core scheduling. |
| 2026-11-15 | Realtime primitives implemented: `core.realtime_on() -> Status` (one-way; `Ok` or `Full`, no second slot on a repeat) and `core.heartbeat()` (no result; a no-op on a normal task). A realtime task must heartbeat once per second of non-blocked time or it is terminated with `FaultCode.WatchdogTimeout` (spec 0007, ADR 0016). |
| 2026-11-16 | Fixed-point float text implemented. `core.format_f64_into(dst, value, frac_digits) -> int` writes an optional `-`, the integer part and exactly `frac_digits` (0–9) fraction digits rounded half away from zero with the carry applied; `NaN`/`±Inf` are `nan`/`inf`/`-inf`, `frac_digits > 9` and `\|value\| >= 2^63` are `-4`, and a result wider than the destination is `-6` with the length untouched. `stdlib.parse_f64_into(src, value) -> Status` accepts decimal text with an optional exponent (plus the formatter's `inf`/`-inf`/`nan`), rejects everything else with `Invalid`, and saturates overflow/underflow to `±Inf`/`±0` with `Ok`, preserving `-0.0`. Fixed-point precision; shortest round-trip formatting and exact IEEE parsing stay open (Q41), `f32` stays open (Q40). |
| 2026-11-16 | Shortest round-trip float text implemented. `core.format_f64_short_into(dst, value) -> int` writes the fewest significant digits that parse back to exactly the same bits, positionally while `-6 < e10 + 1 <= 21` and in scientific form otherwise (`1e20` positional, `1e21` scientific, `1e-6` positional, `1e-7` scientific), with no width, no `2^63` bound and no trailing `.0`; `0`/`-0`, `nan`, `inf`/`-inf` are literal, an exact decimal tie rounds away from zero, and a result wider than the destination is `-6` with the length untouched. The digit search is pure interpreter code (`format_f64_short`): the value and both round-trip interval endpoints are expanded to exact `10^9`-limb integers, the shortest bracketing `n`-digit decimals are tested with integer comparisons, and the text is published with one copy. No allocation, no host call. Q41 narrows to correctly rounded decimal-to-binary parsing and its subnormal edges; `f32` stays open (Q40). |
| 2026-11-16 | Staged graphics surface (spec 0009): `syslib.screen_width`/`screen_height`, `fill_rect`, `fill_dither`, `draw_rect`, `draw_text`, `get_pixel`, `present` and `clear`, with `int` coordinates, `uint` colours (`0x00RRGGBB`), a `const string[0]` text argument and `Status` results. `Invalid` covers `w <= 0`, `h <= 0` and a colour above `0x00FFFFFF`; off-screen coordinates are clipped, never faulted. The primitives are host builtins, not new opcodes, and the disabled-screen shape is `0x0` / `Invalid` / `0`. |
| 2026-11-16 | Staged input surface (spec 0010): `syslib.pointer_x() -> int`, `pointer_y() -> int`, `pointer_buttons() -> uint` and the debug-only `inject_pointer(dx: int, dy: int, buttons: uint) -> Status`. The pointer is a machine-wide value read through the host, not a device handle; `inject_pointer` moves by screen deltas with the same clamping as a real report and returns `Invalid` above `0b111` without changing anything. Raw device bytes stay out of the language: the keyboard remains the console byte stream for `core.read_line`, and key events (M2.2.3) and the event queue (M2.2.4) are future slices. |
| 2026-11-16 | Event surface (spec 0010, M2.2.3–M2.2.5): `EventKind` and `Event` are predeclared (`PointerMove`/`PointerDown`/`PointerUp`/`KeyDown`/`KeyUp`; `kind,x,y,code,window`, 40 bytes at `abi::layout::event`), and `syslib.next_event`/`poll_event`/`inject_event`/`event_drops`/`event_flush` are host builtins. A key event packs text, modifiers and the stable key code into `code` (`abi::key`), and one key byte still feeds the console stream for `core.read_line`. `next_event` blocks in the new `WaitFor::Input`. |
| 2026-11-16 | `syslib.pointer_source() -> uint` added (spec 0010, I27): 0 none, 1 PS/2, 2 absolute USB tablet. It is a read-only machine value next to the pointer, a host builtin (id 26) with no new opcode, and `inject_pointer` keeps working regardless of source. |
| 2026-11-16 | Window surface (spec 0011, M2.3.1): `Rect` is a predeclared record (`x, y, w, h: int`, 32 bytes at `abi::layout::rect`) and `syslib.window_open`/`close`/`raise`/`move`/`frame_rect`/`content_rect`/`focus` are host builtins. `open` takes the frame origin and the content size; ids are `int` with `0` invalid, monotonic and never reused; `close`/`move` are owner-only while `raise` is any task's; the rect reads write a `Rect` only on `Ok`; 16 windows, a 64-byte title and an 8×8 minimum content. No new opcode. |
| 2026-11-16 | M2.3.3: `EventKind` gains `Expose` (ordinal 5), the whole-content repaint request; `Event.window` is filled by the kernel at poll time (focused window, or the hit-tested window for a pointer-down). `syslib.window_present(id) -> Status` is added as the owner-only window presentation path, clipped to the window's visible content; `syslib.present` remains a legal generic primitive. Builtin 39, no new opcode. |
| 2026-11-16 | M2.3.4 adds `syslib.window_info(index: uint, var id: int, var rect: Rect, var title: string[64]) -> Status` (spec 0012, builtin 40): the read-only topmost-first window enumeration used by the desktop shell's window list. The title destination is compile-checked at the service's 64-byte cap; no new opcode. |
| 2026-11-17 | M2.4.0 adds the `stdlib` UI layout/chrome layer (ADR 0026): the library now says `uses core, syslib;` and the helpers draw through the nine graphics primitives as ordinary language code. Metrics (`ui_text_width` = code points × 8, `ui_text_height`, `ui_line_height`, `ui_content_inset`, `ui_title_bar_height`, `ui_close_box_size`, `ui_text_center_x`, `ui_text_center_y`), the System 7.5 palette (`ui_black`, `ui_white`, `ui_gray`, `ui_focus`, `ui_accent`), and client chrome (`ui_frame`, `ui_title_bar`, `ui_close_box`, `ui_separator`, `ui_center_text`). No record/sum/generic/`state`, no new opcode or builtin. |
| 2026-11-17 | M2.4.1 adds the `stdlib` string editing and selection layer: `str_boundary`, `str_cursor_clamp`/`left`/`right`/`home`/`end`, `str_insert_into`, `str_delete_into`, `str_backspace`, `str_selection_normalize`/`all`/`collapse`/`replace` (13 helpers, 52 total). Positions are byte offsets; insertion and deletion require code-point boundaries, a mutator stages into `bytes[256]`, converts with `core.substr_into` and publishes with one copy, so a failure leaves the destination untouched and the text valid UTF-8. Cursors clamp instead of failing (SE5); backspace at 0 is an `Ok` no-op (SE7); the selection is the scalar pair `start`/`stop` (SE6). The 256-byte working capacity returns `CapacityExceeded` without a partial write. No record/sum/generic/`state`, no new opcode or builtin. Closes the language side of Q44. |
| 2026-11-17 | M2.4.2 adds the `stdlib` controls layer (32 drawing, hit-test, transition, focus and key-decode helpers, 84 total): button (normal/pressed/focused), checkbox (box, check mark, label, toggle), text field (visible slice, accent selection, caret, reveal), vertical scrollbar (track, thumb, page hit, clamp), list (rows, selection, scroll into view, row hit, move), `ui_rect_hit`, `ui_focus_ring`, `ui_focus_next` and the packed-key decoders. The application owns the event loop and all state; the library has no widget object, callback, event pump or `state`. Drawing uses the M2.4.0 metrics/palette; too-small geometry is `Invalid` and disabled graphics draw nothing while the pure helpers keep working. No new opcode or builtin. The embedded source cap rises to 128 KiB and the compile arena to 4 MiB (the library is 72,426 B and needs 2,428,888 B of scratch). Proof `samples/apps/ui_controls` (injected pointer/key events, `get_pixel` checks) and the `langhost` sequence test. |
| 2026-11-17 | M2.4.3 adds the `stdlib` menus layer (15 helpers, 99 total): a menu-bar model and drawing (`ui_menu_bar`, `ui_menu_bar_hit`, `ui_menu_bar_height`), pull-downs (`ui_menu_dropdown`, `ui_menu_hit`) with a check column, right-aligned shortcut text, accent selection, gray disabled rows and `-` separators, `uint` mask helpers (`ui_menu_bit`/`has`/`set`), the selectability and state transitions (`ui_menu_selectable`, `ui_menu_first`, `ui_menu_open`, `ui_menu_close`, `ui_menu_move`), and a pure key mapping (`ui_menu_key`: Escape closes, Enter activates, arrows move/switch menus). The app owns the open menu, the highlight and the masks; checked and disabled are independent and the highlight never changes them. No record/sum/generic/`state`, no new opcode or builtin. Too-small geometry is `Invalid`, disabled graphics draw nothing while the pure helpers work, and clipping is `syslib`'s. Proof `samples/apps/ui_menus` (injected pointer/key events, `get_pixel`), the `langhost` sequence test and `/home/sinan/Desktop/igis-menus.png`. |
| 2026-11-17 | M2.4.4 adds the `stdlib` dialogs layer (10 helpers, 109 total): `ui_message` (word-wrapped text in a fixed box), the dialog button geometry (`ui_dialog_button_width`, `ui_dialog_button_left`, `ui_dialog_button_top`), `ui_dialog_button` (a button with the default action's double border), `ui_alert`/`ui_alert_hit` (frame, focused title bar, wrapped message, default/cancel pair; alert slots 0 default, 1 cancel, the close box cancels), `ui_input_dialog`/`ui_input_dialog_hit` (prompt, `ui_text_field`, OK/Cancel; slots 0 field, 1 default, 2 cancel, the close box cancels) and `ui_dialog_key` (Enter = result 1 default, Escape = result 2 cancel when `has_cancel`, Tab/Shift+Tab = actions 3/4 focus next/previous, everything else 0). The app owns the modal loop, the focus slot, the result and the text buffer; the input dialog only draws and never mutates the buffer, and editing stays exclusively the M2.4.1 helpers. No record/sum/generic/`state`, no new opcode or builtin. Too-small geometry is `Invalid` (80 px alert, 106 px input dialog, buttons fitting the width); disabled graphics draw nothing while the pure helpers work. Proof `samples/apps/ui_dialogs` (injected key/pointer events, `get_pixel`), the `langhost` sequence test and `/home/sinan/Desktop/igis-dialogs.png`. |
| 2026-11-17 | M2.4.5 adds the `stdlib` icons and desktop interaction layer (16 helpers, 125 total): `ui_icon` draws a 32×32 monochrome bitmap supplied as `array[uint, 32]` row masks (row 0 the top, bit 0 the left pixel) with one `fill_rect` per maximal set-bit run, and `ui_icon_size`/`ui_icon_row_bit`/`ui_icon_label` complete the asset surface; `ui_icon_cell_x`/`_y`, `ui_icon_label_y` and `ui_icon_grid_hit` are the grid geometry (row-major, `count` valid cells, -1 outside or for non-positive geometry); `ui_double_click` is a pure 400 ms inclusive same-target test with no state; `ui_icon_grab`/`ui_icon_follow` carry the drag offset (inside the bitmap only) and position; `ui_selection_normalize` returns an inclusive band, `ui_selection_rect` the dotted outline and `ui_rect_intersects` the cell overlap (touching edges excluded); `ui_icon_drop` returns -2 inside the trash, else the cell or -1, and `ui_trash_hit` names the rectangle rule. The four bitmaps live in the sample and the app owns the cells, live/selected flags, click time/target and drag clamp; a drag cancels the pending double-click. No record/sum/generic/`state`, no new opcode or builtin. Proof `samples/apps/ui_icons` (injected pointer events, double-click marker, drag to a cell and to the trash, rubber-band selection, `get_pixel`), the `langhost` sequence/pure test and `/home/sinan/Desktop/igis-icons.png`. The compile arena cap rises to 8 MiB (spec 0008). |
| 2026-11-17 | M2.5.1 adds the application file surface (spec 0013, ADR 0027): `core.fs_list(path, index, var name, var kind, var size) -> Status`, `core.fs_read(path, offset, var dst: bytes[256], var read) -> Status` and `core.fs_size(path, var size) -> Status` as host builtins 41–43. The view is the image root only; paths use the shared `core.spawn` syntax (relative, no `..`, ≤256 bytes), a separator is `Invalid` (unsupported subdirectory), listing reports kind `0` file / `1` directory / `2` other, reads are 256-byte chunks with `Ok`/0 at EOF, and every failure leaves the out-parameters untouched. No allocation, no handle and no new opcode. |
| 2026-11-17 | M2.5.3 records application buffers above the 256-byte `stdlib` editing capacity. The M2.4.1 mutators stay fixed at 256 bytes; `samples/apps/editor` holds one `string[8192]` and implements one app-local atomic mutator with the SE2/SE3/SE4 rules (boundary-checked, staged in a local `bytes[8192]`, one strict-UTF-8 publish, `CapacityExceeded` without a partial write), while `str_boundary`, `str_cursor_*` and `str_selection_normalize`/`all`/`collapse` run unchanged on the larger buffer. 8192 is an application constant, not a language limit; a larger file is refused by size before it is read, and saving stays disabled until a writable filesystem exists. |

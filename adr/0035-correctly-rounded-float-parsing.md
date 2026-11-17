# 0035: Correctly rounded float parsing is a `core` interpreter primitive

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 language/runtime
- **Related specs:** 0003 ("Floating-point text"), 0008 (builtins)

## Context

Q41 asked for decimal text → binary64 parsing that matches a correctly rounded reader
bit for bit, including long mantissas and the subnormal range. The first parser was
`stdlib` language code: it accumulated the mantissa as an `f64` (18 significant digits),
built the fraction as an integer numerator over a power of ten, and then applied the
decimal exponent by repeated multiplication or division by 10. That is a *convenient*
reader, not a correct one. The final scaling rounds several times, so `1e-7` came out two
ulps high and `2.2250738585072012e-308` (the smallest normal) landed three ulps above it;
long mantissas lose digits below the 18th before the rounding decision.

The language has no unbounded integers, no bitwise access to an `f64`, and no way to
express the exact decimal expansion of a binary64 value. Writing a correct reader as
`stdlib` code is therefore impossible without new arithmetic primitives.

## Decision

We will parse float text in the **interpreter**, behind a new `core` builtin.

- `core.parse_f64_into(src: const string[0]; value: var f64) -> Status` is builtin 50
  (no opcode; ABI v1 stays at 75). It checks spec 0003's grammar over the source bytes
  and converts with `core`'s `f64: FromStr`, which is allocation-free and correctly
  rounded (19 significant digits plus a sticky bit, exact decimal exponent, one
  round-half-to-even step), then writes the bit pattern through the `var f64`
  out-parameter only on `Ok`. There is no `Host` method: the whole operation is
  interpreter code.
- The grammar check stays in front of the reader. `FromStr` alone also accepts
  `infinity`, case-insensitive `nan`/`inf` and a bare `+`/`-`, which the language does
  not, so the language's accepted set is exactly spec 0003's.
- `nan` writes one canonical positive quiet NaN. Its sign and payload are not observable
  through the language, so the choice only makes the result deterministic across cores.
- `stdlib.parse_f64_into` keeps its signature, its `Status` semantics and its place in
  the function table, and now only delegates. Applications keep the same call and the
  same format→parse round trip.
- Correct rounding is verified against hard-coded bit patterns for the boundary cases
  and against the exact shortest-text formatter (`f64short`), which is an independent
  implementation over base-`10^9` integers: formatting any value and parsing it back
  must reproduce identical bits.

## Alternatives rejected

- **Keep the repeated ×/÷10 reader and document the ulp error.** Rejected: the whole
  point of Q41 is a reader that can be used as an oracle (the shortest-text round trip
  and the compiler's own literal reader both assume correct rounding).
- **Write the exact reader in the interpreter as integer arithmetic.** Rejected as the
  first choice, not as a principle: it duplicates what `core`'s `f64: FromStr` already
  provides (an allocation-free dec2flt with the sticky-bit and interval logic), and it
  would be a second large numeric implementation to prove. `f64short` exists because the
  *formatter* has no `core` equivalent; the reader does.
- **Expose a host method so the host supplies the reader.** Rejected: the kernel host
  has no formatting library either (it is `no_std`), so a host method would only move
  the same call behind a trait boundary, and the primitive must work identically in
  `langhost` and in the kernel.
- **Add a `parse_f64` opcode.** Rejected: the operation is one call with a buffer and a
  scalar out-parameter and needs no immediates, so the existing `BUILTIN` opcode is
  enough and the opcode table does not grow.

## Consequences

### Positive

- Parsed text and compiled literals now agree bit for bit, including subnormals, ties and
  the overflow/underflow boundaries.
- The `stdlib` helper shrank by about 5 KiB of source and needs less compile scratch; the
  function count is unchanged.
- No allocation, no host call, no new opcode, and no new `Host` surface for mocks.

### Negative

- The language's reader is now tied to Rust `core`'s `dec2flt` implementation. That is a
  stable, correctness-tested part of the standard library, and the bit-table and
  round-trip tests would catch a behavioural change on a toolchain upgrade.
- The exact rounding rule lives in Rust rather than in the specification's own prose; the
  spec records the required *behaviour* (nearest, ties to even, saturation), which is what
  the tests pin.

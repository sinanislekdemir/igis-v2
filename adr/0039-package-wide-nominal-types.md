# 0039: Nominal type identity is package-wide; `/lib` types are unit-qualified

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** project owner
- **Related specs:** 0003 (types), 0008 (multi-unit compilation), 0018 (modules)

## Context

Multi-unit compilation (spec 0008) assigns enum, record and sum ids in a table that
is local to each unit, so a function parameter that names a record, sum or user enum
cannot cross a unit boundary. The compiler enforces this in one place
(`cross_unit_type_ok`) and reports "aggregate types cannot cross a unit boundary".

That rule blocks the actual reason `/lib` exists: shared application helpers. An
event helper needs `Event`, a window helper needs `Rect`, a lifecycle helper needs
`ExitReason` and `Diagnostic` — all predeclared nominal types whose ids happen to be
identical in every unit already (`Diagnostic` 0, `Event` 1, `Rect` 2; `ExitReason`
0; `Status` 0, `FaultCode` 1), because `abi::layout` fixes them. User-declared
aggregates are the remaining case: a module that defines `type Point = record … end;`
cannot expose it at all.

The question is how a package gives nominal types one identity without making type
names a package-wide flat namespace, and how another unit names an imported type.

## Decision

We will:

1. **Keep the predeclared nominal types package-global and unqualified.** `Status`,
   `FaultCode`, `Diagnostic`, `Event`, `Rect` and `ExitReason` have fixed ids
   (spec 0008) and may appear in any unit's declarations, signatures, state and
   aggregate element types. This lands first (C1) because it is one gate in the
   compiler and no artifact change: the ids never reach the bytecode.
2. **Give user-declared types package-global ids** in a deterministic order
   (`stdlib`, then modules in discovery order, then the application; declaration
   order inside a unit), after the predeclared ids. `Kind::Record`/`Sum`/`Enum`
   ids therefore mean the same type in every unit, and cross-unit signature checks
   become id equality (C2).
3. **Qualify imported type names with the unit name.** A type declared in module
   `math` is written `math.Point` from another unit; an enum constant is
   `math.Color.Red`. Inside the declaring unit the unqualified forms keep working.
   The application's own types stay application-local: no module may name
   `app.Something`, so the dependency direction remains module → application-free.
4. **Keep value-level variant names unqualified.** Sum construction and `case`
   arms already resolve a variant through the destination or scrutinee type, so
   `Ok`, `Faulted(code)` and their arms work with an imported sum without new
   syntax. Only enum constants need the three-part form.
5. **Keep type names unique per unit, not per package.** Two modules may both
   declare `Point`; the unit qualifier distinguishes them.
6. **Reject value cycles** (a record or sum whose by-value layout contains
   itself, directly or through other units) with a diagnostic. Forward references
   inside one unit become legal as a consequence of pre-assignment.
7. **Change no ABI, verifier or runtime data.** Type ids are compile-time only;
   layout (offsets, sizes, tags) is computed in the compiler and carried as
   immediates. `abi`, the manifest and the interpreter are untouched.

## Consequences

### Positive

- `/lib` can expose real APIs: functions taking and returning `Event`, `Rect`,
  `ExitReason`, `Diagnostic`, and modules can define and export their own records,
  sums and enums.
- Cross-unit checking becomes a simple id comparison; the special-case gate for
  predeclared types disappears once C2 lands.
- Source-level deduplication of application helpers becomes possible without
  changing the one-package-per-application model or the runtime.

### Negative

- The compiler's type tables become package-wide, so a unit's tables can no longer
  be built in isolation; the compiler test for "one unit alone" (`compile_library`)
  must construct the predeclared tables as before, and the shared-table paths need
  their own tests.
- A cyclic record/sum across units is now representable syntactically and must be
  detected during layout rather than by "unknown name".
- Three-part names (`unit.Type.Variant`) are new syntax in type positions and
  enum-constant expressions.

### Neutral / follow-up work

- C2 is the second ticket; C1 (predeclared types) is independent and unblocks the
  motivating helpers immediately.
- After C2, the sample applications can move their duplicated helpers (event and
  pointer loops, reason classification, path/record utilities) into `/lib`; that
  refactor is its own slice with a before/after size report.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Keep types unit-local and pass only scalars/buffers (the "say/say_int" refactor) | It is a workaround: every helper that touches `Event`, `Rect` or `ExitReason` stays duplicated, which is the bulk of the repetition. |
| Package-wide unique type names, unqualified | Simple syntax, but it forbids two modules from using the same natural name (`Point`, `Entry`) and collides with application types. |
| Structural typing for cross-unit aggregates | Contradicts the language's nominal design (spec 0003) and would make two different records with the same fields interchangeable. |
| Put all shared helpers in the embedded `stdlib` instead | `stdlib` is a single embedded blob; application-level helpers would grow every compilation and lose the file-backed, import-what-you-use model. |
| Pass aggregates by reference through an opaque handle | Adds a runtime type/descriptor model and allocation, contradicting the static memory model (ADR 0004/0007). |

## Addendum (2026-11-18, M4.1)

`EventKind` is also a predeclared enum (`abi::EventKind`, enum id 2, identical in every
unit) and is part of the same package-global set: a helper may take `EventKind` directly
(e.g. `is_pointer(k: EventKind)`) instead of only reading `Event.kind`. The decision text
lists six types; this addendum adds the seventh without changing the rule or the ids.

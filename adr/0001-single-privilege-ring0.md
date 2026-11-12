# 0001: Single privilege level (ring 0 only)

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0001 (vision)

## Context

igis-v2 is an experimental OS whose central thesis is that the kernel is a programmable
system rather than a process manager. The conventional argument for ring separation is
that it protects the kernel from faulty or malicious application code. That argument
assumes a trust boundary and a multi-tenant machine. Neither is a goal here: the system
is single-user, experimental, and explicitly not security-oriented.

Meanwhile, the hardware boundary is expensive in design terms: syscall ABI, argument
validation, user-copy routines, per-process address spaces, and the entire class of
"kernel forgot to validate a pointer" bugs.

## Decision

All code — kernel and applications — will execute at CPL 0 in a single address space.
No ring 3 code, no user-mode transitions, and no privilege-change instructions will
exist in the execution path. Kernel code may access any address without translation
checks.

Paging will still be used for mapping and layout (MMIO, heap growth, large pages), but
not for protection between components.

## Consequences

### Positive

- Removes an entire subsystem: no syscall entry/exit path, no user/kernel copies, no
  address-space switching, no per-process page tables.
- Application code can pass pointers and structures to system services directly.
- Debugging is simpler in one respect: there is one address space and one stack model.

### Negative

- No fault isolation. An application bug can corrupt kernel state, and a kernel panic
  kills everything. This is accepted, not mitigated.
- Rust's `unsafe` discipline becomes the only defense against memory corruption. It must
  be applied strictly, with documented safety invariants.
- A single bad pointer in the runtime is a total loss; there is no process to kill.
- Some hardware features (SMAP/SMEP, user/kernel page-table split) become irrelevant,
  and tooling that assumes a userland (e.g. conventional debuggers) fits poorly.

### Neutral

- Interrupts, exceptions, the GDT, the IDT, and the TSS remain necessary.
- Stack layout must still account for interrupt entry, but not for privilege changes.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Monolithic kernel with ring 3 user processes | Adds the exact machinery (ABI, isolation, copies) this project exists to avoid. |
| Microkernel with isolated servers | Maximizes the boundary being rejected; far more IPC machinery than the thesis needs. |
| Single address space with ring 3 (unikernel-style isolation) | Keeps a privilege boundary without the multi-process model; would still require syscall entry, though simpler. |
| Ring 0 kernel + ring 3 only for the interpreter | Splits the thesis: the interpreter is the thing that needs hardware access, and crossing the boundary per builtin call would dominate. |

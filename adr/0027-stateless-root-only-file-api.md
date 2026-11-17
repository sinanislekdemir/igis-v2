# 0027: File access for applications is stateless, root-only host builtins

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** project owner
- **Spec:** 0013 (application files), 0005 (host interface), 0008 (bytecode)
- **Supersedes:** nothing

## Context

M2.5.1 must give applications read-only access to the ext2 image the kernel already
compiles from, because the editor (M2.5.3) and the browser (M2.5.4) are applications.
The driver is small and root-only by design (spec 0002: 1/4 KiB blocks, direct/single/
double indirect, regular files in the image root, no subdirectories). It allocates
nothing and validates every block before use.

The choice is the shape of the application interface, not whether the bytes are
reachable: `core` and `syslib` already expose console, ports and the window service, and
there is no capability check (spec 0003, H4). The options are a descriptor model
(`open`/`read`/`close` with kernel slots), a preload-the-whole-file model, or a stateless
named read.

## Decision

The application file API is three stateless `core` host builtins (spec 0013):

- `core.fs_list(path, index, var name, var kind, var size) -> Status`
- `core.fs_read(path, offset, var dst, var read) -> Status`
- `core.fs_size(path, var size) -> Status`

and no bytecode opcode. A call names a path every time; there is no handle, no
descriptor table, no per-task file state and no working directory. M2.5.1 is root-only:
listing accepts `""`/`"."`, reading and sizing accept one root component with no `/`.
The interpreter validates the path syntax with `abi::fs::validate_path` (the same
function `core.spawn` uses), stages a 64-byte name and a 256-byte read chunk on its own
frame, and publishes out-parameters only on `Ok`. The kernel host resolves the name
through the existing `Ext2`; `langhost` implements the same view over `samples/apps`.

## Consequences

### Positive

- No new kernel state, no lifetime problem and no resource-exhaustion path: a task
  cannot leak or exhaust a handle table, and a terminated task has nothing to close.
- No allocation anywhere: the chunk and the name live on the interpreter frame, and
  the driver streams blocks into the caller's buffer.
- The 256-byte chunk and the root-only rule fit the M2 image (source text only) and the
  existing driver exactly; no new filesystem code beyond two indexed/offset reads.
- One path validator for spawn and files means `..`, empty components and the 256-byte
  limit cannot drift between the two consumers.
- The language already provides the state an editor needs (offset, buffer, selection
  scalars and the M2.4.1 helpers), so the kernel does not have to.

### Negative

- Every read re-resolves the name by walking the root directory and re-reads the inode;
  there is no cached descriptor. Source files are small and this is not a measured
  bottleneck in M2.
- A read is not a snapshot: two calls can observe a different file only if the
  underlying image changes, which it cannot (read-only mount), so the lack of a handle
  does not cost consistency here.
- The API cannot express a long-lived open cursor or a partial-buffer pushback; the
  caller carries `offset` and `read`. That is the intended division for the editor.

### Neutral / follow-up work

- Subdirectory support, writable files and a handle model are deferred
  (`OPEN-QUESTIONS`/spec 0013 open questions); adding them means amending spec 0013 or
  superseding this ADR.
- `langhost` sorts its listing for determinism while ext2 returns directory order. The
  spec makes order unspecified, so no program may depend on either.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Kernel `open`/`read`/`close` with a handle table | New kernel state, a lifetime/close-on-exit problem and a resource cap for a read-only source-only image whose files are small; no benefit in M2 |
| Preload a whole file into an application buffer | The destination capacity would have to be the file size, every reader would pay for the whole file, and a larger file would need a new cap; the chunked read already works for any size |
| Expose the image as a `syslib` block or MMIO handle | Violates the "no raw address crosses the interface" rule (spec 0005, H3) and puts ext2 parsing in the application |
| A `stdlib` wrapper over a single read primitive | The destination capacity is a call-site immediate (no generics), so a library helper could serve only one capacity; `core` is the right home for the checked chunk read (the same reasoning as `substr_into`) |
| A separate file-service task with message streams | A scheduler round trip and a protocol for a mount the kernel already owns; the window service is a kernel service for the same reason (ADR 0020) |

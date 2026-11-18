# 0041: Add a truncate primitive to the home file API

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** project owner
- **Related specs:** 0019 (normative amendment), amends ADR 0040 ("no truncate")

## Context

ADR 0040 deliberately shipped the first writable home filesystem without a
truncate primitive: a shorter replacement was `remove` + create, or
write-a-sibling-then-rename. That was enough for the editor's safe save, but it
leaves the file API unable to shorten a file in place, and every "save a shorter
document" needs a second inode and a directory rename.

The owner asked for truncate as the next slice. The system now has a real ext2
writer with block/inode allocation, direct/single/double indirect blocks and a
write-through cache, so freeing blocks and zero-filling an extension is a small
addition, not a new filesystem.

## Decision

We will add one host builtin, with no opcode (builtin **57**, ABI v1 stays at 75):

```text
core.fs_truncate(path: const string[0]; size: uint) -> Status
```

- **Only under `home/`**, like the other write primitives; elsewhere `Invalid`;
  without a home mount `Invalid`.
- **Shrink:** free every data block at or past `size` (direct, single and double
  indirect, including the now-empty pointer blocks), clear the inode's pointers,
  zero the tail of the last partial block up to the block boundary (so a later
  extension reads zeros, not stale bytes), set the inode size, and only then
  release the bitmaps/counters (a crash between the two leaks blocks, which
  `e2fsck` reclaims; freeing first could let a block be reused while the inode
  still points at it).
- **Extend:** allocate and zero-fill blocks up to `size` (the same gap rule as
  `fs_write`), set the size; capacity is planned before any mutation and a full
  disk is `CapacityExceeded` **without** a partial change.
- **Errors:** a directory is `Invalid`, a missing path `NotFound`, and the
  operation is idempotent at the current size (`Ok`).
- **The editor keeps its temporary-file + rename save.** That path is the
  closest thing to an atomic replacement without a journal; truncate is for
  applications that shorten a file in place, and for tests. This is a deliberate
  trade-off: a write+truncate save would leave a mixed document if a power loss
  hit between the two calls.
- No new opcode, no manifest/verifier change, no change to the application image
  (still read-only).

## Consequences

### Positive

- A file can be shortened or zero-extended without remove+create; the API is
  complete for ordinary text work.
- Shrinking a file returns its data blocks to the bitmap and the counters, so a
  long session does not leak disk space.
- `e2fsck -fn` remains the consistency proof; the ordering above keeps a crash
  recoverable (orphaned blocks, never a double-allocated block).

### Negative

- One more write primitive and one more failure path to test.
- Shrinking is not atomic with a concurrent reader on another core (the HOME
  lock serialises the filesystem, but an application can still observe a size
  before the bytes it expects); the API is not a transaction.
- The filesystem still has no journal; a crash during truncate may need
  `e2fsck`.

### Neutral / follow-up work

- A future journal or an atomic replace primitive would let the editor save
  without the temporary file.
- An append-oriented API and a `fs_chmod`-style surface remain out of scope.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Keep no truncate (`remove` + create) | The owner asked for truncate; it also loses the inode and any future metadata. |
| `fs_truncate` must only shrink | POSIX `truncate` extends with zeros; the same code path serves `fs_write`'s gap rule, and an editor may want to pad. |
| A `replace(path, data)` primitive | It hides the file's existence/creation rules and duplicates `fs_write`; truncate is the smaller primitive. |
| A journal first | Much larger; the ordering keeps `e2fsck` able to recover. |
| Use truncate in the editor and drop the temporary file | Loses the nearest thing to an atomic save without a journal; the current save path is safer. |

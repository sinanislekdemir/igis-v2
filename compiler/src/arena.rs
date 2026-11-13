//! Bounded storage for one compilation: a bump arena, an arena-backed vector
//! and a truncating text buffer.
//!
//! Everything the compiler allocates while compiling lives in the caller's
//! `&mut [u8]` scratch arena (spec 0008, B11). The arena is a classic bump
//! allocator: it hands out disjoint, alignment-correct regions and never frees
//! anything. That is enough because one package is compiled at a time and the
//! whole arena is released when compilation returns.
//!
//! # Why this module contains `unsafe`
//!
//! The rest of the crate is `#![deny(unsafe_code)]`; this module opts out for
//! the three operations that cannot be written safely in stable Rust: turning a
//! borrowed byte range into a typed slice, and reading an initialised value
//! back out of a `MaybeUninit` slot. Every such operation is confined to the
//! handful of functions below, and each one carries a `// SAFETY:` comment.
//! Nodes and table entries are `Copy` plain data, so a region is either fully
//! uninitialised (as `MaybeUninit`) or fully written; there are no partial
//! values and no destructors to run.

#![allow(unsafe_code)]

use core::cell::Cell;
use core::fmt;
use core::marker::PhantomData;
use core::mem::{align_of, size_of, MaybeUninit};

/// Returned when an arena, an arena-backed vector or a fixed buffer is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfSpace;

/// A bump allocator over a caller-provided byte arena.
///
/// Regions are handed out front to back and are never reused, so two live
/// allocations never overlap. `used` is a `Cell`, which lets the lexer, the
/// parser and the code generator hold a shared `&Bump` at the same time and
/// still allocate; the borrow checker is kept out of the arena arithmetic.
pub struct Bump<'a> {
    base: *mut u8,
    len: usize,
    used: Cell<usize>,
    _borrow: PhantomData<&'a mut [u8]>,
}

impl<'a> Bump<'a> {
    /// Wraps the caller's scratch memory.
    pub fn new(memory: &'a mut [u8]) -> Self {
        Bump {
            base: memory.as_mut_ptr(),
            len: memory.len(),
            used: Cell::new(0),
            _borrow: PhantomData,
        }
    }

    /// Bytes handed out so far.
    ///
    /// Part of the arena's observability surface: callers use it to size their
    /// scratch buffers, and tests use it to prove nothing was over-allocated.
    #[allow(dead_code)]
    pub fn used(&self) -> usize {
        self.used.get()
    }

    /// Bytes still available for new allocations.
    #[allow(dead_code)]
    pub fn remaining(&self) -> usize {
        self.len - self.used.get()
    }

    /// Reserves `size` bytes at the next `align`-aligned offset and returns the
    /// offset of the region.
    fn carve(&self, align: usize, size: usize) -> Result<usize, OutOfSpace> {
        let start = self.used.get().checked_add(align - 1).ok_or(OutOfSpace)? & !(align - 1);
        let end = start.checked_add(size).ok_or(OutOfSpace)?;
        if end > self.len {
            return Err(OutOfSpace);
        }
        self.used.set(end);
        Ok(start)
    }

    /// Reserves room for `len` values of `T`, uninitialised.
    ///
    /// The returned slice is the only owner of its bytes: `carve` always
    /// advances `used`, so later allocations start strictly after it.
    pub fn alloc_uninit<T>(&self, len: usize) -> Result<&'a mut [MaybeUninit<T>], OutOfSpace> {
        let size = size_of::<T>().checked_mul(len).ok_or(OutOfSpace)?;
        let offset = self.carve(align_of::<T>(), size)?;
        // SAFETY: `offset` is aligned for `T`, `size` bytes starting there lie
        // inside the arena, and `carve` never returns the same bytes twice.
        // Any bit pattern is a valid `MaybeUninit<T>`, so the uninitialised
        // region may be viewed through this type.
        Ok(unsafe {
            core::slice::from_raw_parts_mut(self.base.add(offset) as *mut MaybeUninit<T>, len)
        })
    }

    /// Allocates one value and returns a reference to it.
    pub fn alloc<T>(&self, value: T) -> Result<&'a mut T, OutOfSpace> {
        let slots = self.alloc_uninit::<T>(1)?;
        Ok(slots[0].write(value))
    }

    /// Allocates `len` copies of `fill`.
    pub fn alloc_slice<T: Copy>(&self, len: usize, fill: T) -> Result<&'a mut [T], OutOfSpace> {
        let slots = self.alloc_uninit::<T>(len)?;
        for slot in slots.iter_mut() {
            slot.write(fill);
        }
        Ok(initialized(slots))
    }

    /// Copies `text` into the arena and returns it as a string slice.
    #[allow(dead_code)]
    pub fn alloc_str(&self, text: &str) -> Result<&'a str, OutOfSpace> {
        let bytes = self.alloc_slice::<u8>(text.len(), 0)?;
        bytes.copy_from_slice(text.as_bytes());
        core::str::from_utf8(&bytes[..]).map_err(|_| OutOfSpace)
    }
}

/// Views the first `len` slots as initialised values.
fn initialized<T>(slots: &mut [MaybeUninit<T>]) -> &mut [T] {
    let len = slots.len();
    // SAFETY: the caller only passes slots it has just filled, and `len`
    // matches the slot count.
    unsafe { core::slice::from_raw_parts_mut(slots.as_mut_ptr() as *mut T, len) }
}

/// A growable vector backed by the bump arena.
///
/// `'a` is the arena lifetime: the returned slices live that long. `'b` is how
/// long the vector borrows the bump handle itself, which is just the vector's
/// own scope; keeping the two apart lets short-lived scratch (the lexer's token
/// list, a parser list, a function's instruction buffer) hand out `'a` data
/// while the `Bump` value itself stays on `compile_into`'s stack frame.
///
/// Growth allocates a fresh, larger region and copies the live prefix; the old
/// region is abandoned (bump allocation never reuses memory). Lists in one unit
/// are short, so the waste is bounded and deterministic.
pub struct SliceVec<'a, 'b, T> {
    bump: &'b Bump<'a>,
    slots: &'a mut [MaybeUninit<T>],
    len: usize,
}

impl<'a, 'b, T: Copy> SliceVec<'a, 'b, T> {
    /// Creates an empty list with a small initial capacity.
    pub fn new(bump: &'b Bump<'a>) -> Result<Self, OutOfSpace> {
        Ok(SliceVec {
            bump,
            slots: bump.alloc_uninit::<T>(8)?,
            len: 0,
        })
    }

    /// Number of live elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Appends one element, growing the backing region when needed.
    pub fn push(&mut self, value: T) -> Result<(), OutOfSpace> {
        if self.len == self.slots.len() {
            self.grow()?;
        }
        self.slots[self.len].write(value);
        self.len += 1;
        Ok(())
    }

    /// Appends every element of `items`.
    pub fn extend_from_slice(&mut self, items: &[T]) -> Result<(), OutOfSpace> {
        for item in items {
            self.push(*item)?;
        }
        Ok(())
    }

    /// The live elements.
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: the first `len` slots were written by `push`.
        unsafe { core::slice::from_raw_parts(self.slots.as_ptr() as *const T, self.len) }
    }

    /// The live elements, mutably (used to patch jump immediates).
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: as `as_slice`; `&mut self` makes the borrow exclusive.
        unsafe { core::slice::from_raw_parts_mut(self.slots.as_mut_ptr() as *mut T, self.len) }
    }

    /// An element, if `index` is live.
    pub fn get(&self, index: usize) -> Option<T> {
        (index < self.len).then(|| self.as_slice()[index])
    }

    /// The last element, if any.
    pub fn last(&self) -> Option<T> {
        (self.len > 0).then(|| self.as_slice()[self.len - 1])
    }

    /// Overwrites a live element.
    pub fn set(&mut self, index: usize, value: T) {
        self.slots[index].write(value);
    }

    /// Consumes the vector, returning the live prefix.
    pub fn into_slice(self) -> &'a [T] {
        let len = self.len;
        // SAFETY: the first `len` slots were written; the unused capacity stays
        // uninitialised and is never observed through this slice.
        unsafe { core::slice::from_raw_parts(self.slots.as_ptr() as *const T, len) }
    }

    fn grow(&mut self) -> Result<(), OutOfSpace> {
        let capacity = self.slots.len();
        let next = if capacity == 0 { 8 } else { capacity * 2 };
        let fresh = self.bump.alloc_uninit::<T>(next)?;
        for (index, slot) in fresh.iter_mut().enumerate().take(self.len) {
            // SAFETY: `index < self.len`, so the slot holds a value.
            slot.write(unsafe { self.slots[index].assume_init() });
        }
        self.slots = fresh;
        Ok(())
    }
}

/// A fixed-capacity text buffer for diagnostics.
///
/// Writes are truncated so that at most `N - 1` bytes are stored (leaving room
/// for a terminator should a future caller need one), always on a UTF-8
/// boundary. Truncation is sticky and reported by [`FixedBuf::is_truncated`].
pub struct FixedBuf<const N: usize> {
    bytes: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> FixedBuf<N> {
    /// An empty buffer.
    pub const fn new() -> Self {
        FixedBuf {
            bytes: [0; N],
            len: 0,
            truncated: false,
        }
    }

    /// The stored text.
    pub fn as_str(&self) -> &str {
        // SAFETY-free: only whole characters are ever appended.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    /// True when some input did not fit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Empties the buffer.
    pub fn clear(&mut self) {
        self.len = 0;
        self.truncated = false;
    }

    /// Appends `text`, truncating at the capacity instead of failing.
    pub fn push_str(&mut self, text: &str) {
        let limit = N.saturating_sub(1);
        for ch in text.chars() {
            let mut encoded = [0u8; 4];
            let bytes = ch.encode_utf8(&mut encoded).as_bytes();
            if self.len + bytes.len() > limit {
                self.truncated = true;
                return;
            }
            self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }
}

impl<const N: usize> Default for FixedBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.push_str(text);
        Ok(())
    }
}

impl<const N: usize> core::ops::Deref for FixedBuf<N> {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize> fmt::Display for FixedBuf<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> fmt::Debug for FixedBuf<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> PartialEq for FixedBuf<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<const N: usize> PartialEq<str> for FixedBuf<N> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<const N: usize> PartialEq<&str> for FixedBuf<N> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl<const N: usize> Eq for FixedBuf<N> {}

impl<const N: usize> Clone for FixedBuf<N> {
    fn clone(&self) -> Self {
        let mut copy = FixedBuf::new();
        copy.push_str(self.as_str());
        copy.truncated = self.truncated;
        copy
    }
}

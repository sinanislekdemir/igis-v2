//! The multi-source wait set shared by the language, the interpreter and the
//! kernel (spec 0003, spec 0007; Q39).
//!
//! `core.next_any` names up to [`MAX_SOURCES`] input sources and blocks until
//! one is ready. The interpreter records that condition on the task as a
//! [`WaitSet`](WaitSet) and the kernel compares it against its wake-up
//! decisions, so the set must be a plain `Copy` value with no dependency on
//! either side. The kernel's stream handle encoding uses `(generation << 8) |
//! index` with generation at least one, so a stream source is never `0` or `1`.

/// Most sources one wait set may name.
pub const MAX_SOURCES: usize = 8;

/// Source value for one completed console line (`core.read_line`).
pub const SOURCE_CONSOLE: u64 = 0;

/// Source value for one machine-wide input event (`syslib.next_event`,
/// including a pending window `Expose`).
pub const SOURCE_INPUT: u64 = 1;

/// A set of input sources named by `core.poll_any`/`core.next_any`.
///
/// The first [`MAX_SOURCES`] entries of the caller's `array[uint, 8]` are
/// copied into the set; `count` is the number of meaningful entries. The
/// constructor never fails after the caller's count has been validated, and
/// the representation is a plain value so it can live in a `Task`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitSet {
    count: u8,
    sources: [u64; MAX_SOURCES],
}

impl WaitSet {
    /// An empty set.
    pub const fn empty() -> Self {
        WaitSet {
            count: 0,
            sources: [0; MAX_SOURCES],
        }
    }

    /// Builds a set from the first `sources.len()` entries.
    ///
    /// Returns `None` when more than [`MAX_SOURCES`] entries are given. An
    /// empty slice is accepted; the builtins reject a zero count themselves.
    pub fn from_slice(sources: &[u64]) -> Option<Self> {
        if sources.len() > MAX_SOURCES {
            return None;
        }
        let mut set = WaitSet::empty();
        set.count = sources.len() as u8;
        set.sources[..sources.len()].copy_from_slice(sources);
        Some(set)
    }

    /// Number of sources in the set.
    pub const fn len(&self) -> usize {
        self.count as usize
    }

    /// True when the set names no sources.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The source at `index`, or `None` past [`WaitSet::len`].
    pub fn get(&self, index: usize) -> Option<u64> {
        if index < self.len() {
            Some(self.sources[index])
        } else {
            None
        }
    }

    /// True when `source` is a member.
    pub fn contains(&self, source: u64) -> bool {
        self.sources[..self.len()].contains(&source)
    }

    /// True when the two sets share at least one source.
    pub fn intersects(&self, other: &WaitSet) -> bool {
        self.sources[..self.len()]
            .iter()
            .any(|source| other.contains(*source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_keeps_its_first_eight_sources() {
        let set = WaitSet::from_slice(&[SOURCE_CONSOLE, SOURCE_INPUT, 256, 257]).unwrap();
        assert_eq!(set.len(), 4);
        assert_eq!(set.get(0), Some(SOURCE_CONSOLE));
        assert_eq!(set.get(1), Some(SOURCE_INPUT));
        assert_eq!(set.get(2), Some(256));
        assert_eq!(set.get(3), Some(257));
        assert_eq!(set.get(4), None);
        assert!(set.contains(256));
        assert!(!set.contains(258));
    }

    #[test]
    fn a_set_is_capped_at_eight_sources() {
        assert!(WaitSet::from_slice(&[0; 9]).is_none());
        assert_eq!(WaitSet::from_slice(&[0; 8]).unwrap().len(), 8);
        assert!(WaitSet::from_slice(&[]).unwrap().is_empty());
    }

    #[test]
    fn intersection_follows_membership() {
        let a = WaitSet::from_slice(&[SOURCE_CONSOLE, 256]).unwrap();
        let b = WaitSet::from_slice(&[SOURCE_INPUT, 256, 257]).unwrap();
        let c = WaitSet::from_slice(&[SOURCE_INPUT]).unwrap();
        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
        assert!(!a.intersects(&WaitSet::empty()));
    }

    #[test]
    fn a_set_is_a_copy_value() {
        let a = WaitSet::from_slice(&[SOURCE_INPUT, 256]).unwrap();
        let b = a;
        assert_eq!(a, b);
        let mut sources = a;
        sources.count = 1;
        assert_ne!(a, sources);
    }
}

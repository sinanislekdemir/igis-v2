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

/// Source value for console input bytes being available (`core.read_input`
/// would return at least one byte). The console is a raw byte stream: the
/// source reports queued bytes, not a completed line.
pub const SOURCE_CONSOLE_BYTE: u64 = 0;

/// Source value for one machine-wide input event (`syslib.next_event`,
/// including a pending window `Expose`).
pub const SOURCE_INPUT: u64 = 1;

/// Tag bit that marks a [`WaitSet`] entry as a socket read source
/// (`core.socket_source`, spec 0020, N13; ADR 0050).
///
/// A stream handle is `(generation << 8) | index` with a non-zero generation,
/// so it is far below `2^63`; the high bit therefore separates a socket source
/// from a stream handle without a second array element type.
pub const SOURCE_SOCKET_BASE: u64 = 1 << 63;

/// Builds the `next_any`/`poll_any` source value for a socket handle.
///
/// The handle itself is the low 63 bits; the caller must not pass the result to
/// a stream primitive. `core.socket_source` is compiler-lowered to this OR
/// (an unsigned add, because a live handle's high bit is always clear).
pub const fn socket_source(handle: u64) -> u64 {
    SOURCE_SOCKET_BASE | handle
}

/// True when `source` names a socket rather than the console, the input queue
/// or a stream.
pub const fn is_socket_source(source: u64) -> bool {
    source & SOURCE_SOCKET_BASE != 0
}

/// The socket handle carried by a source; the inverse of [`socket_source`].
pub const fn socket_handle(source: u64) -> u64 {
    source & !SOURCE_SOCKET_BASE
}

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

    /// Builds a set from one source.
    pub const fn single(source: u64) -> Self {
        let mut sources = [0u64; MAX_SOURCES];
        sources[0] = source;
        WaitSet { count: 1, sources }
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
        let set = WaitSet::from_slice(&[SOURCE_CONSOLE_BYTE, SOURCE_INPUT, 256, 257]).unwrap();
        assert_eq!(set.len(), 4);
        assert_eq!(set.get(0), Some(SOURCE_CONSOLE_BYTE));
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
        let a = WaitSet::from_slice(&[SOURCE_CONSOLE_BYTE, 256]).unwrap();
        let b = WaitSet::from_slice(&[SOURCE_INPUT, 256, 257]).unwrap();
        let c = WaitSet::from_slice(&[SOURCE_INPUT]).unwrap();
        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
        assert!(!a.intersects(&WaitSet::empty()));
    }

    #[test]
    fn a_single_source_set_has_one_member() {
        let set = WaitSet::single(SOURCE_CONSOLE_BYTE);
        assert_eq!(set.len(), 1);
        assert_eq!(set.get(0), Some(SOURCE_CONSOLE_BYTE));
        assert_eq!(set.get(1), None);
        assert!(!set.is_empty());
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

    #[test]
    fn a_socket_source_sets_the_high_bit_and_keeps_the_handle() {
        for handle in [1, 0x0101, 0x00CD_0000_0000_0001, (1 << 62) | 7] {
            let source = socket_source(handle);
            assert!(is_socket_source(source), "{handle:#x}");
            assert_eq!(socket_handle(source), handle, "{handle:#x}");
            assert_eq!(source & !SOURCE_SOCKET_BASE, handle);
        }
        assert_eq!(SOURCE_SOCKET_BASE, 1 << 63);
        assert_eq!(socket_source(0), SOURCE_SOCKET_BASE);
    }

    #[test]
    fn stream_and_console_sources_are_never_socket_sources() {
        for source in [SOURCE_CONSOLE_BYTE, SOURCE_INPUT, 256, 0x0101, (1 << 62) | 1] {
            assert!(!is_socket_source(source), "{source:#x}");
        }
        // The two encodings cannot alias: a live socket handle and a stream
        // handle are both below the tag bit.
        let stream = (7u64 << 8) | 3;
        assert_ne!(socket_source(stream), stream);
        assert!(!is_socket_source(stream));
        let mut set = WaitSet::empty();
        assert!(set.is_empty());
        set = WaitSet::single(socket_source(stream));
        assert!(set.contains(socket_source(stream)));
        assert!(!set.contains(stream));
    }
}

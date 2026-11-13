//! The system clock: one global nanosecond counter, advanced by the boot
//! core's periodic timer tick (spec 0007).
//!
//! This is the only clock the runtime sees. It is coarse (the tick period) and
//! deliberately not derived from the TSC, which can drift between vCPUs.

use core::sync::atomic::{AtomicU64, Ordering};

/// Tick period in nanoseconds; the timer runs at 1 kHz in the first milestone.
pub const TICK_NS: u64 = 1_000_000;

static NOW_NS: AtomicU64 = AtomicU64::new(0);

pub fn now_ns() -> u64 {
    NOW_NS.load(Ordering::Relaxed)
}

pub fn advance_tick() {
    NOW_NS.fetch_add(TICK_NS, Ordering::Relaxed);
}

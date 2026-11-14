//! Injectable monotonic time source.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A monotonic clock. Only differences between readings are meaningful.
pub trait Clock: Send + Sync {
    /// Time elapsed since an arbitrary fixed origin.
    fn now(&self) -> Duration;
}

/// The real clock.
#[derive(Debug, Clone)]
pub struct SystemClock(Instant);

impl SystemClock {
    /// A clock whose origin is now.
    #[must_use]
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A clock that only moves when told to; clones share the same time.
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// A clock at time zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Move time forward.
    pub fn advance(&self, by: Duration) {
        let ms = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }
}

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use crate::time::{TimeError, Timestamp};

/// The only way a service or background job reads the time. fold-core has no implementation that reads the system clock.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// `2026-01-01T00:00:00.000Z`.
const DEFAULT_START_MILLIS: i64 = 1_767_225_600_000;

/// A test clock. Clones share one time.
#[derive(Debug, Clone)]
pub struct ManualClock {
    millis: Arc<AtomicI64>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(DEFAULT_START_MILLIS)),
        }
    }

    pub fn at(start: Timestamp) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(start.as_millis())),
        }
    }

    pub fn advance(&self, by: Duration) -> Result<Timestamp, TimeError> {
        let mut next = None;
        let update = self
            .millis
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                let moved = Timestamp::from_millis(current).ok()?.checked_add(by)?;
                next = Some(moved);
                Some(moved.as_millis())
            });
        match (update, next) {
            (Ok(_), Some(moved)) => Ok(moved),
            _ => Err(TimeError::OutOfRange),
        }
    }

    pub fn set(&self, time: Timestamp) {
        self.millis.store(time.as_millis(), Ordering::SeqCst);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        // Only values taken from a `Timestamp` are ever stored.
        Timestamp::from_valid_millis(self.millis.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_starts_fixed_and_advances() {
        let clock = ManualClock::new();
        assert_eq!(clock.now().to_string(), "2026-01-01T00:00:00.000Z");
        let moved = clock.advance(Duration::from_secs(90)).unwrap();
        assert_eq!(moved.to_string(), "2026-01-01T00:01:30.000Z");
        assert_eq!(clock.now(), moved);
        let start = Timestamp::from_millis(42).unwrap();
        let other = ManualClock::at(start);
        assert_eq!(other.now(), start);
        other.set(Timestamp::from_millis(7).unwrap());
        assert_eq!(other.now().as_millis(), 7);
        assert_eq!(ManualClock::default().now(), ManualClock::new().now());
    }

    #[test]
    fn manual_clock_clones_share_one_time() {
        let clock = ManualClock::new();
        let copy = clock.clone();
        copy.advance(Duration::from_millis(5)).unwrap();
        assert_eq!(clock.now(), copy.now());
        clock.set(Timestamp::from_millis(0).unwrap());
        assert_eq!(copy.now().as_millis(), 0);
        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        clock.set(Timestamp::from_millis(9).unwrap());
        assert_eq!(shared.now().as_millis(), 9);
    }

    #[test]
    fn manual_clock_advance_checks_overflow() {
        let top = Timestamp::from_millis(253_402_300_799_999).unwrap();
        let clock = ManualClock::at(top);
        assert_eq!(
            clock.advance(Duration::from_millis(1)),
            Err(TimeError::OutOfRange)
        );
        assert_eq!(clock.now(), top);
        assert_eq!(clock.advance(Duration::MAX), Err(TimeError::OutOfRange));
    }
}

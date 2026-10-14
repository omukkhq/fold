use std::time::SystemTime;

use fold_core::{Clock, Timestamp};

/// The production clock and the only place in the daemon that reads the system time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        // A host clock outside the supported years reads as the epoch.
        Timestamp::from_system_time(SystemTime::now()).unwrap_or(Timestamp::EPOCH)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn reads_the_system_time_in_utc_milliseconds() {
        let before = Timestamp::from_system_time(SystemTime::now()).unwrap();
        let now = SystemClock.now();
        let after = Timestamp::from_system_time(SystemTime::now()).unwrap();
        assert!(before <= now && now <= after);
        assert!(now.duration_since(Timestamp::EPOCH).unwrap() > Duration::from_secs(1_700_000_000));
    }
}

use std::time::Duration;

use crate::time::{MIN_MILLIS, Timestamp};

/// A claim is stale once `lease_minutes` have passed since its last renewal. Staleness is computed when a claim is read and never stored by this rule. A `renewed_at` later than `now` is not stale.
pub fn is_stale(renewed_at: Timestamp, now: Timestamp, lease_minutes: u64) -> bool {
    let lease = Duration::from_secs(lease_minutes.saturating_mul(60));
    now.duration_since(renewed_at)
        .is_some_and(|elapsed| elapsed >= lease)
}

/// The proposal-stage check runs that a claim's holder started, as its lease sees them: the holder waits for such a run and makes no calls while it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HolderRuns {
    /// No run of the holder goes or ever ended.
    #[default]
    None,
    /// A run of the holder goes.
    Running,
    /// No run of the holder goes, and its newest run ended `at`.
    Ended { at: Timestamp },
}

/// The time a claim's lease counts from at `now`: its last renewal, or the end of its holder's newest check run when that is later, and `now` while a run of its holder goes. A claim never runs out during its holder's own run, and a full lease follows the run's end.
pub fn lease_start(renewed_at: Timestamp, runs: HolderRuns, now: Timestamp) -> Timestamp {
    match runs {
        HolderRuns::None => renewed_at,
        HolderRuns::Running => renewed_at.max(now),
        HolderRuns::Ended { at } => renewed_at.max(at),
    }
}

/// A claim lapsed: it is stale by [`is_stale`] counted from [`lease_start`], so a takeover is allowed.
pub fn is_lapsed(
    renewed_at: Timestamp,
    runs: HolderRuns,
    now: Timestamp,
    lease_minutes: u64,
) -> bool {
    is_stale(lease_start(renewed_at, runs, now), now, lease_minutes)
}

/// A claim is near stale once three quarters of its lease have passed since its last renewal, so its holder hears of it while a quarter of the lease is left. A claim already stale and not yet taken over is near stale too: its holder still keeps it. A claim renewed at or before the returned time is near stale; `None` when the cutoff falls before the first representable time, and then no claim is.
pub fn near_stale_cutoff(now: Timestamp, lease_minutes: u64) -> Option<Timestamp> {
    let lease_millis = lease_minutes.saturating_mul(60_000);
    let warned_after = Duration::from_millis(lease_millis - lease_millis / 4);
    now.checked_sub(warned_after)
}

/// The renewal time at or before which a claim is stale at `now`, by the rule of `is_stale`, so a query can tell stale claims apart without reading each one. A lease longer than the timestamp range reaches back to its earliest value.
pub fn stale_cutoff(now: Timestamp, lease_minutes: u64) -> Timestamp {
    let lease = Duration::from_secs(lease_minutes.saturating_mul(60));
    now.checked_sub(lease)
        .unwrap_or(Timestamp::from_valid_millis(MIN_MILLIS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn a_claim_is_stale_from_the_end_of_its_lease() {
        let renewed = at(1_000_000);
        assert!(!is_stale(renewed, at(1_000_000 + 29 * MINUTE), 30));
        assert!(!is_stale(renewed, at(1_000_000 + 30 * MINUTE - 1), 30));
        assert!(is_stale(renewed, at(1_000_000 + 30 * MINUTE), 30));
        assert!(is_stale(renewed, at(1_000_000 + 31 * MINUTE), 30));
    }

    #[test]
    fn the_lease_follows_lease_minutes() {
        let renewed = at(0);
        assert!(!is_stale(renewed, at(4 * MINUTE), 5));
        assert!(is_stale(renewed, at(5 * MINUTE), 5));
        assert!(!is_stale(renewed, at(5 * MINUTE), 6));
        assert!(!is_stale(renewed, at(200_000_000_000_000), u64::MAX));
    }

    #[test]
    fn a_claim_is_near_stale_in_the_last_quarter_of_its_lease() {
        let now = at(100 * MINUTE);
        assert_eq!(near_stale_cutoff(now, 40), Some(at(70 * MINUTE)));
        assert_eq!(near_stale_cutoff(now, 1), Some(at(100 * MINUTE - 45_000)));
        assert_eq!(near_stale_cutoff(now, 0), Some(now));
        assert_eq!(
            near_stale_cutoff(at(10 * MINUTE), 40),
            Some(at(-20 * MINUTE))
        );
        assert_eq!(near_stale_cutoff(now, u64::MAX), None);
    }

    #[test]
    fn the_cutoff_splits_stale_renewals_from_live_ones() {
        let now = at(1_000_000 + 30 * MINUTE);
        let cutoff = stale_cutoff(now, 30);
        assert_eq!(cutoff, at(1_000_000));
        for renewed in [
            cutoff,
            at(1_000_000 - 1),
            at(1_000_000 + 1),
            now,
            at(now.as_millis() + 1),
        ] {
            assert_eq!(renewed <= cutoff, is_stale(renewed, now, 30), "{renewed}");
        }
        assert_eq!(stale_cutoff(now, 0), now);
        assert_eq!(
            stale_cutoff(now, u64::MAX),
            Timestamp::from_millis(MIN_MILLIS).unwrap()
        );
    }

    #[test]
    fn a_renewal_in_the_future_is_not_stale() {
        assert!(!is_stale(at(10 * MINUTE), at(0), 1));
        assert!(!is_stale(at(0), at(0), 30));
    }

    #[test]
    fn a_claim_never_lapses_while_its_holders_run_goes() {
        let renewed = at(0);
        for minutes in [30, 60, 24 * 60] {
            let now = at(minutes * MINUTE);
            assert!(is_stale(renewed, now, 30));
            assert!(!is_lapsed(renewed, HolderRuns::Running, now, 30));
            assert_eq!(lease_start(renewed, HolderRuns::Running, now), now);
        }
    }

    #[test]
    fn a_full_lease_follows_the_end_of_the_holders_newest_run() {
        let renewed = at(0);
        let ended = HolderRuns::Ended {
            at: at(50 * MINUTE),
        };
        assert_eq!(
            lease_start(renewed, ended, at(60 * MINUTE)),
            at(50 * MINUTE)
        );
        assert!(!is_lapsed(renewed, ended, at(80 * MINUTE - 1), 30));
        assert!(is_lapsed(renewed, ended, at(80 * MINUTE), 30));
        assert_eq!(
            lease_start(at(70 * MINUTE), ended, at(90 * MINUTE)),
            at(70 * MINUTE),
            "a call after the run counts"
        );
    }

    #[test]
    fn without_a_run_the_lease_counts_from_the_renewal() {
        let renewed = at(0);
        assert_eq!(HolderRuns::default(), HolderRuns::None);
        assert_eq!(lease_start(renewed, HolderRuns::None, at(MINUTE)), renewed);
        assert!(!is_lapsed(
            renewed,
            HolderRuns::None,
            at(30 * MINUTE - 1),
            30
        ));
        assert!(is_lapsed(renewed, HolderRuns::None, at(30 * MINUTE), 30));
        assert_eq!(
            lease_start(at(10 * MINUTE), HolderRuns::Running, at(5 * MINUTE)),
            at(10 * MINUTE),
            "a renewal later than now stays"
        );
    }
}

use std::time::Duration;

use crate::time::Timestamp;

/// A claim is stale once `lease_minutes` have passed since its last renewal. Staleness is computed when a claim is read and never stored by this rule. A `renewed_at` later than `now` is not stale.
pub fn is_stale(renewed_at: Timestamp, now: Timestamp, lease_minutes: u64) -> bool {
    let lease = Duration::from_secs(lease_minutes.saturating_mul(60));
    now.duration_since(renewed_at)
        .is_some_and(|elapsed| elapsed >= lease)
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
    fn a_renewal_in_the_future_is_not_stale() {
        assert!(!is_stale(at(10 * MINUTE), at(0), 1));
        assert!(!is_stale(at(0), at(0), 30));
    }
}

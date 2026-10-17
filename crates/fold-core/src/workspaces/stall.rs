use crate::time::Timestamp;

/// The kind and state of a workspace as the stall rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFact {
    /// A task workspace whose directory is in place.
    Active,
    /// A task workspace whose directory is gone.
    Missing,
    /// An archived or abandoned task workspace.
    Closed,
    /// A review checkout: nothing in it is recorded, so its files say nothing about the worker.
    ReviewCheckout,
}

/// A worker is stalled when it holds the unreleased implementation claim of the task whose active workspace has had no tracked content change for `stall_minutes`. The time counts from `last_activity_at`, which is the creation time of a workspace with no edits. Calls do not count: a stalled worker can hold a fresh lease. A `last_activity_at` later than `now` is not stalled.
pub fn is_stalled(
    now: Timestamp,
    has_impl_claim: bool,
    workspace: WorkspaceFact,
    last_activity_at: Timestamp,
    stall_minutes: u64,
) -> bool {
    if !has_impl_claim || workspace != WorkspaceFact::Active {
        return false;
    }
    crate::tasks::claims::is_stale(last_activity_at, now, stall_minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn an_active_claimed_workspace_stalls_from_the_end_of_the_window() {
        let edited = at(1_000_000);
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(
            at(1_000_000 + 4 * MINUTE),
            true,
            active,
            edited,
            5
        ));
        assert!(!is_stalled(
            at(1_000_000 + 5 * MINUTE - 1),
            true,
            active,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 5 * MINUTE),
            true,
            active,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 60 * MINUTE),
            true,
            active,
            edited,
            5
        ));
    }

    #[test]
    fn the_window_follows_stall_minutes() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(at(9 * MINUTE), true, active, at(0), 10));
        assert!(is_stalled(at(10 * MINUTE), true, active, at(0), 10));
        assert!(!is_stalled(
            at(200_000_000_000_000),
            true,
            active,
            at(0),
            u64::MAX
        ));
    }

    #[test]
    fn without_an_implementation_claim_nothing_stalls() {
        let late = at(60 * MINUTE);
        assert!(!is_stalled(late, false, WorkspaceFact::Active, at(0), 5));
    }

    #[test]
    fn only_an_active_task_workspace_stalls() {
        let late = at(60 * MINUTE);
        for workspace in [
            WorkspaceFact::Missing,
            WorkspaceFact::Closed,
            WorkspaceFact::ReviewCheckout,
        ] {
            assert!(
                !is_stalled(late, true, workspace, at(0), 5),
                "{workspace:?}"
            );
        }
    }

    #[test]
    fn activity_in_the_future_is_not_stalled() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(at(0), true, active, at(10 * MINUTE), 1));
        assert!(!is_stalled(at(0), true, active, at(0), 5));
    }
}

use std::time::Duration;

use crate::ids::CheckpointId;
use crate::time::Timestamp;

/// Why fold took an automatic checkpoint. Each command that can lose the current files takes one first; `Quiet` closes an editing burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoReason {
    BeforeSync,
    BeforeRecord,
    BeforeProposal,
    BeforeProposalUpdate,
    Quiet,
}

impl AutoReason {
    pub const ALL: [AutoReason; 5] = [
        AutoReason::BeforeSync,
        AutoReason::BeforeRecord,
        AutoReason::BeforeProposal,
        AutoReason::BeforeProposalUpdate,
        AutoReason::Quiet,
    ];

    /// The value of `checkpoints.reason`.
    pub fn as_str(self) -> &'static str {
        match self {
            AutoReason::BeforeSync => "before_sync",
            AutoReason::BeforeRecord => "before_record",
            AutoReason::BeforeProposal => "before_proposal",
            AutoReason::BeforeProposalUpdate => "before_proposal_update",
            AutoReason::Quiet => "quiet",
        }
    }
}

/// Whether a workspace is due its `Quiet` checkpoint at `now`: it saw activity after the last automatic checkpoint, and none for at least `quiet_seconds` since. One editing burst gives one checkpoint, however many files it wrote, because the checkpoint's time then covers the burst's last activity. `None` for `last_activity_at` is a workspace nobody changed; `None` for `last_checkpointed_activity_at` is one with no automatic checkpoint yet.
pub fn quiet_checkpoint_due(
    now: Timestamp,
    last_activity_at: Option<Timestamp>,
    last_checkpointed_activity_at: Option<Timestamp>,
    quiet_seconds: u64,
) -> bool {
    let Some(activity) = last_activity_at else {
        return false;
    };
    if last_checkpointed_activity_at.is_some_and(|covered| covered >= activity) {
        return false;
    }
    activity
        .checked_add(Duration::from_secs(quiet_seconds))
        .is_some_and(|due| now >= due)
}

/// The automatic checkpoints to prune: every one after the newest `keep`. The input holds automatic checkpoints only, newest first, so an explicit checkpoint is never in the result.
pub fn prune_set(autos_newest_first: &[CheckpointId], keep: u64) -> Vec<CheckpointId> {
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    autos_newest_first.iter().skip(keep).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIET: u64 = 120;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(seconds * 1000).unwrap()
    }

    fn ids(numbers: impl IntoIterator<Item = u32>) -> Vec<CheckpointId> {
        numbers
            .into_iter()
            .map(|number| CheckpointId::from_number(number).unwrap())
            .collect()
    }

    #[test]
    fn a_workspace_without_activity_is_never_due() {
        assert!(!quiet_checkpoint_due(at(10_000), None, None, QUIET));
        assert!(!quiet_checkpoint_due(at(10_000), None, Some(at(5)), QUIET));
    }

    #[test]
    fn activity_is_due_only_after_the_quiet_period() {
        assert!(!quiet_checkpoint_due(at(100), Some(at(100)), None, QUIET));
        assert!(!quiet_checkpoint_due(at(219), Some(at(100)), None, QUIET));
        assert!(quiet_checkpoint_due(at(220), Some(at(100)), None, QUIET));
        assert!(quiet_checkpoint_due(at(5_000), Some(at(100)), None, QUIET));
    }

    #[test]
    fn quiet_exactly_at_the_limit_is_due() {
        let activity = at(1_000);
        let limit = activity.checked_add(Duration::from_secs(QUIET)).unwrap();
        let before = Timestamp::from_millis(limit.as_millis() - 1).unwrap();
        assert!(!quiet_checkpoint_due(before, Some(activity), None, QUIET));
        assert!(quiet_checkpoint_due(limit, Some(activity), None, QUIET));
    }

    #[test]
    fn activity_that_a_checkpoint_covers_is_not_due_again() {
        assert!(!quiet_checkpoint_due(
            at(5_000),
            Some(at(100)),
            Some(at(220)),
            QUIET
        ));
        assert!(!quiet_checkpoint_due(
            at(5_000),
            Some(at(100)),
            Some(at(100)),
            QUIET
        ));
    }

    #[test]
    fn activity_after_a_checkpoint_is_due_after_its_own_quiet_period() {
        let checkpointed = Some(at(220));
        assert!(!quiet_checkpoint_due(
            at(300),
            Some(at(250)),
            checkpointed,
            QUIET
        ));
        assert!(quiet_checkpoint_due(
            at(370),
            Some(at(250)),
            checkpointed,
            QUIET
        ));
    }

    #[test]
    fn a_quiet_period_past_the_end_of_time_is_never_due() {
        let last = Timestamp::from_millis(253_402_300_799_999).unwrap();
        assert!(!quiet_checkpoint_due(last, Some(last), None, QUIET));
    }

    #[test]
    fn nineteen_and_twenty_checkpoints_prune_nothing() {
        assert!(prune_set(&ids((1..=19).rev()), 20).is_empty());
        assert!(prune_set(&ids((1..=20).rev()), 20).is_empty());
    }

    #[test]
    fn twenty_five_checkpoints_prune_the_five_oldest() {
        assert_eq!(prune_set(&ids((1..=25).rev()), 20), ids((1..=5).rev()));
    }

    #[test]
    fn explicit_checkpoints_between_them_change_nothing() {
        // Every third number is an explicit checkpoint, so it is not in the input.
        let autos: Vec<u32> = (1..=36).rev().filter(|number| number % 3 != 0).collect();
        assert_eq!(autos.len(), 24);
        let pruned = prune_set(&ids(autos.iter().copied()), 20);
        assert_eq!(pruned, ids([5, 4, 2, 1]));
        assert!(pruned.iter().all(|id| id.number() % 3 != 0));
    }

    #[test]
    fn keeping_none_prunes_every_one() {
        assert_eq!(prune_set(&ids([3, 2, 1]), 0), ids([3, 2, 1]));
        assert!(prune_set(&[], 0).is_empty());
    }

    #[test]
    fn the_reasons_have_distinct_stored_names() {
        let mut names: Vec<&str> = AutoReason::ALL
            .iter()
            .map(|reason| reason.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), AutoReason::ALL.len());
    }
}

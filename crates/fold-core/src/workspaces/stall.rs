use crate::time::Timestamp;

/// The kind and state of a workspace as the stall rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFact {
    /// A task workspace whose directory is in place.
    Active,
    /// A task workspace whose files are the tree of its open proposal's newest version while the proposal waits on others: the worker sent its work and waits for the review or the integration.
    Sent,
    /// A task workspace whose directory is gone.
    Missing,
    /// An archived or abandoned task workspace.
    Closed,
    /// A review checkout: nothing in it is recorded, so its files say nothing about the worker.
    ReviewCheckout,
}

/// Whose move the open proposal of a task waits for, as the stall rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalTurn {
    /// Reviewers or the integration. A new version always starts here: it has no verdict, and its conflict and check flags start clear.
    Others,
    /// Its author, since the proposal came back: changes were requested, or the version was flagged as conflicting with main or failing its integration checks.
    Author { since: Timestamp },
}

impl ProposalTurn {
    /// From the time the latest verdict on the current version requested changes and the time the proposal was flagged as conflicting or failing its checks, each `None` when it does not apply. These are the facts that put the next move with the author in the task status too; the later time is the one the author answers.
    pub fn of(changes_requested_at: Option<Timestamp>, flagged_at: Option<Timestamp>) -> Self {
        match changes_requested_at.max(flagged_at) {
            Some(since) => Self::Author { since },
            None => Self::Others,
        }
    }
}

/// The fact of a task workspace whose directory is in place: `Sent` while its files are the open proposal's newest version (`edits_after_version` is false) and the proposal waits on others, `Active` otherwise. A proposal that came back to its author makes the workspace active again, edited or not.
pub fn present_workspace(
    edits_after_version: Option<bool>,
    turn: Option<ProposalTurn>,
) -> WorkspaceFact {
    match (edits_after_version, turn) {
        (Some(false), Some(ProposalTurn::Others)) => WorkspaceFact::Sent,
        _ => WorkspaceFact::Active,
    }
}

/// The proposal-stage check runs of a task workspace as the stall rule sees them: a run in the workspace (a `run_checks` call or the gate of `create_proposal` or `update_proposal`) is work the worker waits for, while it goes and until it tells the worker its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceChecks {
    /// No run of the workspace ever ended and none goes.
    None,
    /// A run of the workspace goes.
    Running,
    /// No run goes, and the newest run of the workspace ended `at`.
    Ended { at: Timestamp },
}

/// The time the stall window counts from: the last activity, the time the proposal came back to its author, or the end of the workspace's newest check run, whichever is latest, so the author gets the whole window to answer a verdict and the worker the whole window to act on its check results.
pub fn stall_start(
    last_activity_at: Timestamp,
    turn: Option<ProposalTurn>,
    checks: WorkspaceChecks,
) -> Timestamp {
    let start = match turn {
        Some(ProposalTurn::Author { since }) => last_activity_at.max(since),
        Some(ProposalTurn::Others) | None => last_activity_at,
    };
    match checks {
        WorkspaceChecks::Ended { at } => start.max(at),
        WorkspaceChecks::None | WorkspaceChecks::Running => start,
    }
}

/// A worker is stalled when it holds the unreleased implementation claim of the task whose active workspace has had no tracked content change for `stall_minutes` and runs no checks. The time counts from `last_activity_at`, which is the creation time of a workspace with no edits. Calls do not count: a stalled worker can hold a fresh lease. A `last_activity_at` later than `now` is not stalled.
pub fn is_stalled(
    now: Timestamp,
    has_impl_claim: bool,
    workspace: WorkspaceFact,
    checks: WorkspaceChecks,
    last_activity_at: Timestamp,
    stall_minutes: u64,
) -> bool {
    if !has_impl_claim || workspace != WorkspaceFact::Active || checks == WorkspaceChecks::Running {
        return false;
    }
    crate::tasks::claims::is_stale(last_activity_at, now, stall_minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;
    const NONE: WorkspaceChecks = WorkspaceChecks::None;

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
            NONE,
            edited,
            5
        ));
        assert!(!is_stalled(
            at(1_000_000 + 5 * MINUTE - 1),
            true,
            active,
            NONE,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 5 * MINUTE),
            true,
            active,
            NONE,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 60 * MINUTE),
            true,
            active,
            NONE,
            edited,
            5
        ));
    }

    #[test]
    fn the_window_follows_stall_minutes() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(at(9 * MINUTE), true, active, NONE, at(0), 10));
        assert!(is_stalled(at(10 * MINUTE), true, active, NONE, at(0), 10));
        assert!(!is_stalled(
            at(200_000_000_000_000),
            true,
            active,
            NONE,
            at(0),
            u64::MAX
        ));
    }

    #[test]
    fn without_an_implementation_claim_nothing_stalls() {
        let late = at(60 * MINUTE);
        assert!(!is_stalled(
            late,
            false,
            WorkspaceFact::Active,
            NONE,
            at(0),
            5
        ));
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
                !is_stalled(late, true, workspace, NONE, at(0), 5),
                "{workspace:?}"
            );
        }
    }

    #[test]
    fn a_workspace_that_holds_its_sent_version_does_not_stall() {
        let late = at(60 * MINUTE);
        assert!(!is_stalled(late, true, WorkspaceFact::Sent, NONE, at(0), 5));
        assert!(is_stalled(
            late,
            true,
            WorkspaceFact::Active,
            NONE,
            at(0),
            5
        ));
    }

    #[test]
    fn a_proposal_is_with_its_author_from_the_later_of_its_verdict_and_its_flag() {
        assert_eq!(ProposalTurn::of(None, None), ProposalTurn::Others);
        assert_eq!(
            ProposalTurn::of(Some(at(5)), None),
            ProposalTurn::Author { since: at(5) }
        );
        assert_eq!(
            ProposalTurn::of(None, Some(at(7))),
            ProposalTurn::Author { since: at(7) }
        );
        assert_eq!(
            ProposalTurn::of(Some(at(9)), Some(at(7))),
            ProposalTurn::Author { since: at(9) }
        );
    }

    #[test]
    fn only_a_sent_version_that_waits_on_others_is_sent() {
        let back = Some(ProposalTurn::Author { since: at(0) });
        let waiting = Some(ProposalTurn::Others);
        assert_eq!(present_workspace(Some(false), waiting), WorkspaceFact::Sent);
        assert_eq!(present_workspace(Some(false), back), WorkspaceFact::Active);
        assert_eq!(
            present_workspace(Some(true), waiting),
            WorkspaceFact::Active
        );
        assert_eq!(present_workspace(None, waiting), WorkspaceFact::Active);
        assert_eq!(present_workspace(Some(false), None), WorkspaceFact::Active);
    }

    #[test]
    fn requested_changes_stall_from_the_verdict_not_from_the_last_edit() {
        let verdict = at(30 * MINUTE);
        let turn = Some(ProposalTurn::Author { since: verdict });
        let start = stall_start(at(0), turn, NONE);
        assert_eq!(start, verdict);
        let fact = present_workspace(Some(false), turn);
        assert!(!is_stalled(at(34 * MINUTE), true, fact, NONE, start, 5));
        assert!(is_stalled(at(35 * MINUTE), true, fact, NONE, start, 5));
        assert_eq!(
            stall_start(at(40 * MINUTE), turn, NONE),
            at(40 * MINUTE),
            "a later edit counts"
        );
        assert_eq!(stall_start(at(3), Some(ProposalTurn::Others), NONE), at(3));
        assert_eq!(stall_start(at(3), None, NONE), at(3));
    }

    #[test]
    fn activity_in_the_future_is_not_stalled() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(at(0), true, active, NONE, at(10 * MINUTE), 1));
        assert!(!is_stalled(at(0), true, active, NONE, at(0), 5));
    }

    #[test]
    fn a_running_check_run_never_stalls_its_worker() {
        let active = WorkspaceFact::Active;
        let running = WorkspaceChecks::Running;
        for minutes in [5, 60, 24 * 60] {
            assert!(!is_stalled(
                at(minutes * MINUTE),
                true,
                active,
                running,
                at(0),
                5
            ));
        }
        assert_eq!(stall_start(at(7), None, running), at(7));
    }

    #[test]
    fn the_window_starts_again_at_the_end_of_the_newest_run() {
        let active = WorkspaceFact::Active;
        let ended = WorkspaceChecks::Ended {
            at: at(20 * MINUTE),
        };
        let start = stall_start(at(0), None, ended);
        assert_eq!(start, at(20 * MINUTE));
        assert!(!is_stalled(at(24 * MINUTE), true, active, ended, start, 5));
        assert_eq!(
            stall_start(at(30 * MINUTE), None, ended),
            at(30 * MINUTE),
            "an edit after the run counts"
        );
        let back = Some(ProposalTurn::Author {
            since: at(40 * MINUTE),
        });
        assert_eq!(stall_start(at(0), back, ended), at(40 * MINUTE));
    }

    #[test]
    fn a_failed_run_and_no_edits_for_the_window_after_it_stall() {
        let active = WorkspaceFact::Active;
        let ended = WorkspaceChecks::Ended {
            at: at(12 * MINUTE),
        };
        let start = stall_start(at(0), None, ended);
        assert!(!is_stalled(
            at(17 * MINUTE - 1),
            true,
            active,
            ended,
            start,
            5
        ));
        assert!(is_stalled(at(17 * MINUTE), true, active, ended, start, 5));
    }
}

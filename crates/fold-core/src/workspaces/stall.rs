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

/// What the worker of a task waits for from others and when they last gave it the turn, as the stall rule sees it beside its workspace and its checks. The default is a worker that waits for nobody and was never given the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkerTurn {
    /// An ask about the task or a permission question of the worker waits for a person.
    pub waits_for_person: bool,
    /// The newest answer of a person: a person's message about the task or to the worker, an answer for the user or the closure of an ask about the task, or the answer to a permission question of the worker.
    pub answered_at: Option<Timestamp>,
    /// The start of the worker's current process: a start, a resume or a hand-off.
    pub started_at: Option<Timestamp>,
}

impl WorkerTurn {
    /// The next move is someone else's, so the worker's silence says nothing.
    pub fn waits(&self) -> bool {
        self.waits_for_person
    }

    /// The latest time the worker was given the turn, or `None` when it never was.
    pub fn given_at(&self) -> Option<Timestamp> {
        self.answered_at.max(self.started_at)
    }
}

/// The time the stall window counts from: the last activity, the time the proposal came back to its author, the end of the workspace's newest check run, or the time the worker was last given the turn, whichever is latest. The author gets the whole window to answer a verdict, the worker the whole window to act on its check results, an answer or a restart.
pub fn stall_start(
    last_activity_at: Timestamp,
    turn: Option<ProposalTurn>,
    checks: WorkspaceChecks,
    worker: WorkerTurn,
) -> Timestamp {
    let start = match turn {
        Some(ProposalTurn::Author { since }) => last_activity_at.max(since),
        Some(ProposalTurn::Others) | None => last_activity_at,
    };
    let start = match checks {
        WorkspaceChecks::Ended { at } => start.max(at),
        WorkspaceChecks::None | WorkspaceChecks::Running => start,
    };
    worker.given_at().map_or(start, |given| start.max(given))
}

/// A worker is stalled when it holds the unreleased implementation claim of the task whose active workspace has had no tracked content change for `stall_minutes`, runs no checks and waits for nobody. The time counts from `last_activity_at`, which is the creation time of a workspace with no edits. Calls do not count: a stalled worker can hold a fresh lease. A `last_activity_at` later than `now` is not stalled.
pub fn is_stalled(
    now: Timestamp,
    has_impl_claim: bool,
    workspace: WorkspaceFact,
    checks: WorkspaceChecks,
    worker: WorkerTurn,
    last_activity_at: Timestamp,
    stall_minutes: u64,
) -> bool {
    if !has_impl_claim
        || workspace != WorkspaceFact::Active
        || checks == WorkspaceChecks::Running
        || worker.waits()
    {
        return false;
    }
    crate::tasks::claims::is_stale(last_activity_at, now, stall_minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;
    const NONE: WorkspaceChecks = WorkspaceChecks::None;
    const IDLE: WorkerTurn = WorkerTurn {
        waits_for_person: false,
        answered_at: None,
        started_at: None,
    };

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
            IDLE,
            edited,
            5
        ));
        assert!(!is_stalled(
            at(1_000_000 + 5 * MINUTE - 1),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 5 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 60 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
    }

    #[test]
    fn the_window_follows_stall_minutes() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(
            at(9 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            10
        ));
        assert!(is_stalled(
            at(10 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            10
        ));
        assert!(!is_stalled(
            at(200_000_000_000_000),
            true,
            active,
            NONE,
            IDLE,
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
            IDLE,
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
                !is_stalled(late, true, workspace, NONE, IDLE, at(0), 5),
                "{workspace:?}"
            );
        }
    }

    #[test]
    fn a_workspace_that_holds_its_sent_version_does_not_stall() {
        let late = at(60 * MINUTE);
        assert!(!is_stalled(
            late,
            true,
            WorkspaceFact::Sent,
            NONE,
            IDLE,
            at(0),
            5
        ));
        assert!(is_stalled(
            late,
            true,
            WorkspaceFact::Active,
            NONE,
            IDLE,
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
        let start = stall_start(at(0), turn, NONE, IDLE);
        assert_eq!(start, verdict);
        let fact = present_workspace(Some(false), turn);
        assert!(!is_stalled(
            at(34 * MINUTE),
            true,
            fact,
            NONE,
            IDLE,
            start,
            5
        ));
        assert!(is_stalled(
            at(35 * MINUTE),
            true,
            fact,
            NONE,
            IDLE,
            start,
            5
        ));
        assert_eq!(
            stall_start(at(40 * MINUTE), turn, NONE, IDLE),
            at(40 * MINUTE),
            "a later edit counts"
        );
        assert_eq!(
            stall_start(at(3), Some(ProposalTurn::Others), NONE, IDLE),
            at(3)
        );
        assert_eq!(stall_start(at(3), None, NONE, IDLE), at(3));
    }

    #[test]
    fn activity_in_the_future_is_not_stalled() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(
            at(0),
            true,
            active,
            NONE,
            IDLE,
            at(10 * MINUTE),
            1
        ));
        assert!(!is_stalled(at(0), true, active, NONE, IDLE, at(0), 5));
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
                IDLE,
                at(0),
                5
            ));
        }
        assert_eq!(stall_start(at(7), None, running, IDLE), at(7));
    }

    #[test]
    fn the_window_starts_again_at_the_end_of_the_newest_run() {
        let active = WorkspaceFact::Active;
        let ended = WorkspaceChecks::Ended {
            at: at(20 * MINUTE),
        };
        let start = stall_start(at(0), None, ended, IDLE);
        assert_eq!(start, at(20 * MINUTE));
        assert!(!is_stalled(
            at(24 * MINUTE),
            true,
            active,
            ended,
            IDLE,
            start,
            5
        ));
        assert_eq!(
            stall_start(at(30 * MINUTE), None, ended, IDLE),
            at(30 * MINUTE),
            "an edit after the run counts"
        );
        let back = Some(ProposalTurn::Author {
            since: at(40 * MINUTE),
        });
        assert_eq!(stall_start(at(0), back, ended, IDLE), at(40 * MINUTE));
    }

    #[test]
    fn a_failed_run_and_no_edits_for_the_window_after_it_stall() {
        let active = WorkspaceFact::Active;
        let ended = WorkspaceChecks::Ended {
            at: at(12 * MINUTE),
        };
        let start = stall_start(at(0), None, ended, IDLE);
        assert!(!is_stalled(
            at(17 * MINUTE - 1),
            true,
            active,
            ended,
            IDLE,
            start,
            5
        ));
        assert!(is_stalled(
            at(17 * MINUTE),
            true,
            active,
            ended,
            IDLE,
            start,
            5
        ));
    }

    #[test]
    fn a_worker_that_waits_for_a_person_never_stalls() {
        let active = WorkspaceFact::Active;
        let asked = WorkerTurn {
            waits_for_person: true,
            ..IDLE
        };
        assert!(asked.waits());
        for minutes in [5, 60, 24 * 60] {
            assert!(!is_stalled(
                at(minutes * MINUTE),
                true,
                active,
                NONE,
                asked,
                at(0),
                5
            ));
        }
        assert!(!IDLE.waits());
        assert!(is_stalled(
            at(5 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            5
        ));
    }

    #[test]
    fn an_answer_clears_the_stall_and_only_a_new_full_window_brings_it_back() {
        let active = WorkspaceFact::Active;
        let edited = at(0);
        let waiting = WorkerTurn {
            waits_for_person: true,
            ..IDLE
        };
        let start = stall_start(edited, None, NONE, waiting);
        assert!(!is_stalled(
            at(40 * MINUTE),
            true,
            active,
            NONE,
            waiting,
            start,
            5
        ));
        let answered = WorkerTurn {
            answered_at: Some(at(40 * MINUTE)),
            ..IDLE
        };
        let start = stall_start(edited, None, NONE, answered);
        assert_eq!(start, at(40 * MINUTE));
        assert!(
            !is_stalled(at(40 * MINUTE), true, active, NONE, answered, start, 5),
            "the answer clears the mark at once"
        );
        assert!(!is_stalled(
            at(45 * MINUTE - 1),
            true,
            active,
            NONE,
            answered,
            start,
            5
        ));
        assert!(
            is_stalled(at(45 * MINUTE), true, active, NONE, answered, start, 5),
            "a new full window of silence"
        );
    }

    #[test]
    fn a_restart_of_the_worker_starts_the_window_again() {
        let active = WorkspaceFact::Active;
        let worker = WorkerTurn {
            started_at: Some(at(20 * MINUTE)),
            ..IDLE
        };
        assert_eq!(worker.given_at(), Some(at(20 * MINUTE)));
        let start = stall_start(at(0), None, NONE, worker);
        assert_eq!(start, at(20 * MINUTE));
        assert!(!is_stalled(
            at(25 * MINUTE - 1),
            true,
            active,
            NONE,
            worker,
            start,
            5
        ));
        assert!(is_stalled(
            at(25 * MINUTE),
            true,
            active,
            NONE,
            worker,
            start,
            5
        ));
    }

    #[test]
    fn the_latest_of_every_restart_counts() {
        let worker = WorkerTurn {
            answered_at: Some(at(30)),
            started_at: Some(at(20)),
            ..IDLE
        };
        assert_eq!(worker.given_at(), Some(at(30)));
        assert_eq!(IDLE.given_at(), None);
        assert_eq!(WorkerTurn::default(), IDLE);
        assert_eq!(
            stall_start(at(50), None, NONE, worker),
            at(50),
            "an edit after the answer counts"
        );
        let back = Some(ProposalTurn::Author { since: at(40) });
        assert_eq!(stall_start(at(0), back, NONE, worker), at(40));
        let ended = WorkspaceChecks::Ended { at: at(45) };
        assert_eq!(stall_start(at(0), back, ended, worker), at(45));
    }
}

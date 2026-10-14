use serde::{Deserialize, Serialize};

use crate::ids::TaskId;

/// A task's column on the board. The value is always calculated from `TaskFacts`; nothing sets it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Backlog,
    Todo,
    InProgress,
    Review,
    Done,
}

/// Where the task's open proposal stands. `WithAuthor` means the next move belongs to the author, `WithReviewer` that it belongs to a reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalFact {
    None,
    WithReviewer,
    WithAuthor,
}

impl ProposalFact {
    /// Builds the fact for an open proposal. `changes_requested` is true when the latest verdict on the current version is `request_changes`; comments are not verdicts and never set it.
    pub fn open(changes_requested: bool, conflicted: bool) -> Self {
        if changes_requested || conflicted {
            Self::WithAuthor
        } else {
            Self::WithReviewer
        }
    }
}

/// The stored facts that decide a task's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFacts {
    /// A completion record exists.
    pub completed: bool,
    pub proposal: ProposalFact,
    /// A task workspace in state `active` or `missing` exists.
    pub has_active_workspace: bool,
    /// The status of each direct prerequisite.
    pub prerequisites: Vec<(TaskId, TaskStatus)>,
}

/// The calculated status and the unfinished prerequisites that block a started task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calculated {
    pub status: TaskStatus,
    /// Sorted by id. Empty unless the status is `InProgress` or `Review`.
    pub blocked_by: Vec<TaskId>,
}

impl Calculated {
    pub fn blocked(&self) -> bool {
        !self.blocked_by.is_empty()
    }
}

/// Only `completed` yields `Done`, and the first matching rule wins.
pub fn calculate(facts: &TaskFacts) -> Calculated {
    let unfinished = || {
        let mut ids: Vec<TaskId> = facts
            .prerequisites
            .iter()
            .filter(|(_, status)| *status != TaskStatus::Done)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let status = if facts.completed {
        TaskStatus::Done
    } else if facts.proposal == ProposalFact::WithReviewer {
        TaskStatus::Review
    } else if facts.has_active_workspace {
        TaskStatus::InProgress
    } else if unfinished().is_empty() {
        TaskStatus::Todo
    } else {
        TaskStatus::Backlog
    };
    let blocked_by = match status {
        TaskStatus::InProgress | TaskStatus::Review => unfinished(),
        _ => Vec::new(),
    };
    Calculated { status, blocked_by }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskStatus::*;

    fn t(n: u32) -> TaskId {
        TaskId::from_number(n).unwrap()
    }

    fn facts(
        completed: bool,
        proposal: ProposalFact,
        workspace: bool,
        prerequisites: &[(u32, TaskStatus)],
    ) -> TaskFacts {
        TaskFacts {
            completed,
            proposal,
            has_active_workspace: workspace,
            prerequisites: prerequisites.iter().map(|&(n, s)| (t(n), s)).collect(),
        }
    }

    fn status(f: &TaskFacts) -> TaskStatus {
        calculate(f).status
    }

    use ProposalFact::{None as NoProposal, WithAuthor, WithReviewer};

    #[test]
    fn row_1_completed_is_done_whatever_else_holds() {
        let f = facts(true, WithReviewer, true, &[(1, Backlog)]);
        let c = calculate(&f);
        assert_eq!(c.status, Done);
        assert!(!c.blocked());
    }

    #[test]
    fn row_2_reviewer_proposal_is_review() {
        assert_eq!(status(&facts(false, WithReviewer, true, &[])), Review);
        assert_eq!(status(&facts(false, WithReviewer, false, &[])), Review);
    }

    #[test]
    fn row_3_workspace_is_in_progress_also_with_author_proposal() {
        assert_eq!(status(&facts(false, NoProposal, true, &[])), InProgress);
        assert_eq!(status(&facts(false, WithAuthor, true, &[])), InProgress);
    }

    #[test]
    fn row_4_no_prerequisites_or_all_done_is_todo() {
        assert_eq!(status(&facts(false, NoProposal, false, &[])), Todo);
        assert_eq!(
            status(&facts(false, NoProposal, false, &[(1, Done), (2, Done)])),
            Todo
        );
    }

    #[test]
    fn row_5_an_unfinished_prerequisite_is_backlog() {
        for open in [Backlog, Todo, InProgress, Review] {
            assert_eq!(
                status(&facts(false, NoProposal, false, &[(1, Done), (2, open)])),
                Backlog
            );
        }
    }

    #[test]
    fn author_proposal_without_workspace_falls_through() {
        assert_eq!(status(&facts(false, WithAuthor, false, &[])), Todo);
        assert_eq!(
            status(&facts(false, WithAuthor, false, &[(1, Todo)])),
            Backlog
        );
    }

    #[test]
    fn proposal_fact_open_follows_verdict_and_conflict() {
        assert_eq!(ProposalFact::open(false, false), WithReviewer);
        assert_eq!(ProposalFact::open(true, false), WithAuthor);
        assert_eq!(ProposalFact::open(false, true), WithAuthor);
        assert_eq!(ProposalFact::open(true, true), WithAuthor);
    }

    // Each board event as the status before and after it.

    fn moves(before: &TaskFacts, after: &TaskFacts) -> (TaskStatus, TaskStatus) {
        (status(before), status(after))
    }

    #[test]
    fn event_created_without_unfinished_prerequisites_is_todo() {
        assert_eq!(status(&facts(false, NoProposal, false, &[])), Todo);
        assert_eq!(status(&facts(false, NoProposal, false, &[(1, Done)])), Todo);
    }

    #[test]
    fn event_created_with_unfinished_prerequisites_is_backlog() {
        assert_eq!(
            status(&facts(false, NoProposal, false, &[(1, InProgress)])),
            Backlog
        );
    }

    #[test]
    fn event_last_prerequisite_done_moves_backlog_to_todo() {
        let before = facts(false, NoProposal, false, &[(1, Done), (2, Review)]);
        let after = facts(false, NoProposal, false, &[(1, Done), (2, Done)]);
        assert_eq!(moves(&before, &after), (Backlog, Todo));
    }

    #[test]
    fn event_workspace_created_moves_todo_to_in_progress() {
        let before = facts(false, NoProposal, false, &[(1, Done)]);
        let after = facts(false, NoProposal, true, &[(1, Done)]);
        assert_eq!(moves(&before, &after), (Todo, InProgress));
    }

    #[test]
    fn event_proposal_opened_moves_in_progress_to_review() {
        let before = facts(false, NoProposal, true, &[]);
        let after = facts(false, ProposalFact::open(false, false), true, &[]);
        assert_eq!(moves(&before, &after), (InProgress, Review));
    }

    #[test]
    fn event_changes_requested_moves_review_to_in_progress() {
        let before = facts(false, ProposalFact::open(false, false), true, &[]);
        let after = facts(false, ProposalFact::open(true, false), true, &[]);
        assert_eq!(moves(&before, &after), (Review, InProgress));
    }

    #[test]
    fn event_approval_after_request_changes_returns_to_review() {
        // The loader reads the latest verdict on the version, so the approval clears `changes_requested`.
        let before = facts(false, ProposalFact::open(true, false), true, &[]);
        let after = facts(false, ProposalFact::open(false, false), true, &[]);
        assert_eq!(moves(&before, &after), (InProgress, Review));
    }

    #[test]
    fn event_comment_does_not_change_status() {
        // A comment is not a verdict: the fact stays as the last verdict left it.
        assert_eq!(
            status(&facts(false, ProposalFact::open(false, false), true, &[])),
            Review
        );
        assert_eq!(
            status(&facts(false, ProposalFact::open(true, false), true, &[])),
            InProgress
        );
    }

    #[test]
    fn event_conflict_moves_review_to_in_progress() {
        let before = facts(false, ProposalFact::open(false, false), true, &[]);
        let after = facts(false, ProposalFact::open(false, true), true, &[]);
        assert_eq!(moves(&before, &after), (Review, InProgress));
    }

    #[test]
    fn event_new_version_moves_in_progress_to_review() {
        // A new version has no verdict and (after sync) no conflict.
        for (changes_requested, conflicted) in [(true, false), (false, true)] {
            let before = facts(
                false,
                ProposalFact::open(changes_requested, conflicted),
                true,
                &[],
            );
            let after = facts(false, ProposalFact::open(false, false), true, &[]);
            assert_eq!(moves(&before, &after), (InProgress, Review));
        }
    }

    #[test]
    fn event_proposal_closed_or_rejected_returns_to_in_progress() {
        for open in [WithReviewer, WithAuthor] {
            let before = facts(false, open, true, &[]);
            let after = facts(false, NoProposal, true, &[]);
            assert_eq!(status(&before) == Review, open == WithReviewer);
            assert_eq!(status(&after), InProgress);
        }
    }

    #[test]
    fn event_workspace_abandoned_returns_to_todo_or_backlog() {
        let before = facts(false, NoProposal, true, &[(1, Done)]);
        let after = facts(false, NoProposal, false, &[(1, Done)]);
        assert_eq!(moves(&before, &after), (InProgress, Todo));

        let before = facts(false, NoProposal, true, &[(1, Todo)]);
        let after = facts(false, NoProposal, false, &[(1, Todo)]);
        assert_eq!(moves(&before, &after), (InProgress, Backlog));
    }

    #[test]
    fn event_integrated_moves_review_to_done() {
        let before = facts(false, ProposalFact::open(false, false), true, &[]);
        let after = facts(true, NoProposal, false, &[]);
        assert_eq!(moves(&before, &after), (Review, Done));
    }

    #[test]
    fn started_task_with_new_unfinished_prerequisite_is_blocked() {
        let c = calculate(&facts(
            false,
            NoProposal,
            true,
            &[(9, Todo), (3, Backlog), (5, Done)],
        ));
        assert_eq!(c.status, InProgress);
        assert_eq!(c.blocked_by, vec![t(3), t(9)]);
        assert!(c.blocked());
    }

    #[test]
    fn blocked_list_empties_when_the_prerequisite_is_done() {
        let c = calculate(&facts(false, NoProposal, true, &[(3, Done)]));
        assert_eq!(c.status, InProgress);
        assert!(c.blocked_by.is_empty());
        assert!(!c.blocked());
    }

    #[test]
    fn a_prerequisite_listed_twice_blocks_once() {
        let c = calculate(&facts(false, NoProposal, true, &[(4, Todo), (4, Todo)]));
        assert_eq!(c.blocked_by, vec![t(4)]);
    }

    #[test]
    fn review_can_be_blocked() {
        let c = calculate(&facts(false, WithReviewer, false, &[(2, Todo)]));
        assert_eq!((c.status, c.blocked_by), (Review, vec![t(2)]));
    }

    #[test]
    fn backlog_and_todo_are_never_blocked() {
        assert!(
            calculate(&facts(false, NoProposal, false, &[(2, Todo)]))
                .blocked_by
                .is_empty()
        );
        assert!(
            calculate(&facts(false, NoProposal, false, &[]))
                .blocked_by
                .is_empty()
        );
    }

    #[test]
    fn status_serialises_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&InProgress).unwrap(),
            "\"in_progress\""
        );
    }
}

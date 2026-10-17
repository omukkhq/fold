use serde::{Deserialize, Serialize};

use crate::ids::{ProposalId, TaskId, WorkspaceId};
use crate::tasks::{TaskKind, TaskValueError, define_names};

define_names!(
    /// A task's column on the board. The value is always calculated from `TaskFacts`; nothing sets it directly. Its text is what `tasks.status` stores and the serde form.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum TaskStatus {
        Backlog = "backlog",
        Todo = "todo",
        InProgress = "in_progress",
        Review = "review",
        Done = "done",
    },
    error = TaskValueError::InvalidStatus
);

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

/// The stored facts that decide a task's status. A `proposal` task reads `proposal` and `has_active_workspace`; a `no_proposal` task reads `claimed` and `reported` instead, because its work leaves no workspace or proposal behind; a `human` task reads neither, because a person does its work outside fold and only the completion record moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFacts {
    pub kind: TaskKind,
    /// A completion record exists.
    pub completed: bool,
    pub proposal: ProposalFact,
    /// A task workspace in state `active` or `missing` exists.
    pub has_active_workspace: bool,
    /// An unreleased implementation claim exists, stale or not.
    pub claimed: bool,
    /// The coder's report of finished work waits for a lead check: it is neither completed nor sent back.
    pub reported: bool,
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

/// The prerequisites that are not Done, sorted by id.
fn unfinished(facts: &TaskFacts) -> Vec<TaskId> {
    let mut ids: Vec<TaskId> = facts
        .prerequisites
        .iter()
        .filter(|(_, status)| *status != TaskStatus::Done)
        .map(|(id, _)| *id)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The status the task's own work gives it, before the prerequisites decide between Todo and Backlog: Review while a reviewer or a lead check has the next move, In Progress while the work goes on. A human task never starts in fold: it stays in Todo, or Backlog while a prerequisite is unfinished, until a person marks it Done.
fn started(facts: &TaskFacts) -> Option<TaskStatus> {
    let (review, in_progress) = match facts.kind {
        TaskKind::Proposal => (
            facts.proposal == ProposalFact::WithReviewer,
            facts.has_active_workspace,
        ),
        TaskKind::NoProposal => (facts.reported, facts.claimed),
        TaskKind::Human => (false, false),
    };
    if review {
        Some(TaskStatus::Review)
    } else if in_progress {
        Some(TaskStatus::InProgress)
    } else {
        None
    }
}

/// Only `completed` yields `Done`, and the first matching rule wins.
pub fn calculate(facts: &TaskFacts) -> Calculated {
    let unfinished = || unfinished(facts);
    let status = if facts.completed {
        TaskStatus::Done
    } else if let Some(status) = started(facts) {
        status
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

/// The sentence that names the rule of [`calculate`] that decided the status of `facts`. `workspace` is the live task workspace and `proposal` the open one; the rule never needs them to decide, only to name them.
pub fn status_reason(
    facts: &TaskFacts,
    workspace: Option<WorkspaceId>,
    proposal: Option<ProposalId>,
) -> String {
    let calculated = calculate(facts);
    let ids = |ids: &[TaskId]| {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let proposal = proposal.map_or_else(|| "the proposal".to_owned(), |id| id.to_string());
    let workspace = workspace.map_or_else(|| "a workspace".to_owned(), |id| id.to_string());
    let mut text = match (calculated.status, facts.kind) {
        (TaskStatus::Done, _) => "Completed: a completion record exists".to_owned(),
        (TaskStatus::Review, TaskKind::NoProposal) => {
            "Calculated: the coder reported the work finished and it waits for a lead check"
                .to_owned()
        }
        (TaskStatus::InProgress, TaskKind::NoProposal) => {
            "Calculated: a coder holds the claim, no report yet".to_owned()
        }
        (TaskStatus::Todo, TaskKind::NoProposal) => {
            "Calculated: no claim yet and every prerequisite is Done".to_owned()
        }
        (TaskStatus::Backlog, _) => format!("Calculated: waits for {}", ids(&unfinished(facts))),
        (_, TaskKind::Human) => {
            "Calculated: every prerequisite is Done and a person marks the task Done".to_owned()
        }
        (TaskStatus::Review, TaskKind::Proposal) => {
            format!("Calculated: proposal {proposal} waits for a reviewer")
        }
        (TaskStatus::InProgress, TaskKind::Proposal) => match facts.proposal {
            ProposalFact::WithAuthor => format!(
                "Calculated: workspace {workspace} is open and proposal {proposal} waits for its author"
            ),
            _ => format!("Calculated: workspace {workspace} is open, no proposal yet"),
        },
        (TaskStatus::Todo, TaskKind::Proposal) => {
            "Calculated: no workspace yet and every prerequisite is Done".to_owned()
        }
    };
    if calculated.blocked() {
        text.push_str(&format!(", needs {}", ids(&calculated.blocked_by)));
    }
    text
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
            kind: TaskKind::Proposal,
            completed,
            proposal,
            has_active_workspace: workspace,
            claimed: false,
            reported: false,
            prerequisites: prerequisites.iter().map(|&(n, s)| (t(n), s)).collect(),
        }
    }

    /// The facts of a task without a proposal.
    fn lead(
        completed: bool,
        claimed: bool,
        reported: bool,
        prerequisites: &[(u32, TaskStatus)],
    ) -> TaskFacts {
        TaskFacts {
            kind: TaskKind::NoProposal,
            claimed,
            reported,
            ..facts(completed, NoProposal, false, prerequisites)
        }
    }

    /// The facts of a human task.
    fn human(completed: bool, prerequisites: &[(u32, TaskStatus)]) -> TaskFacts {
        TaskFacts {
            kind: TaskKind::Human,
            ..facts(completed, NoProposal, false, prerequisites)
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
    fn a_task_without_a_proposal_moves_by_its_claim_report_and_completion() {
        let todo = lead(false, false, false, &[(1, Done)]);
        let claimed = lead(false, true, false, &[(1, Done)]);
        let reported = lead(false, true, true, &[(1, Done)]);
        let completed = lead(true, false, true, &[(1, Done)]);
        assert_eq!(moves(&todo, &claimed), (Todo, InProgress));
        assert_eq!(moves(&claimed, &reported), (InProgress, Review));
        assert_eq!(moves(&reported, &completed), (Review, Done));
        assert_eq!(moves(&reported, &claimed), (Review, InProgress));
        assert_eq!(status(&lead(false, false, true, &[])), Review);
        assert_eq!(status(&lead(false, false, false, &[(1, Todo)])), Backlog);
        assert_eq!(
            calculate(&lead(false, true, false, &[(4, Review)])).blocked_by,
            vec![t(4)]
        );
    }

    #[test]
    fn each_kind_reads_only_its_own_facts() {
        let mut proposal = facts(false, NoProposal, false, &[]);
        proposal.claimed = true;
        proposal.reported = true;
        assert_eq!(status(&proposal), Todo);
        let mut no_proposal = lead(false, false, false, &[]);
        no_proposal.proposal = WithReviewer;
        no_proposal.has_active_workspace = true;
        assert_eq!(status(&no_proposal), Todo);
    }

    #[test]
    fn a_human_task_waits_in_todo_until_its_completion() {
        assert_eq!(status(&human(false, &[])), Todo);
        assert_eq!(status(&human(false, &[(1, Done)])), Todo);
        assert_eq!(status(&human(false, &[(1, InProgress)])), Backlog);
        assert_eq!(status(&human(true, &[(1, Todo)])), Done);
        assert_eq!(
            status_reason(&human(false, &[]), None, None),
            "Calculated: every prerequisite is Done and a person marks the task Done"
        );
        assert_eq!(
            status_reason(&human(false, &[(2, Todo)]), None, None),
            "Calculated: waits for T-2"
        );
        assert_eq!(
            status_reason(&human(true, &[]), None, None),
            "Completed: a completion record exists"
        );
    }

    #[test]
    fn the_reason_names_the_lead_check_rules() {
        let reason = |f: &TaskFacts| status_reason(f, None, None);
        assert_eq!(
            reason(&lead(false, true, true, &[])),
            "Calculated: the coder reported the work finished and it waits for a lead check"
        );
        assert_eq!(
            reason(&lead(false, true, false, &[(2, Todo)])),
            "Calculated: a coder holds the claim, no report yet, needs T-2"
        );
        assert_eq!(
            reason(&lead(false, false, false, &[])),
            "Calculated: no claim yet and every prerequisite is Done"
        );
        assert_eq!(
            reason(&lead(true, false, true, &[])),
            "Completed: a completion record exists"
        );
        assert_eq!(
            reason(&lead(false, false, false, &[(3, Todo)])),
            "Calculated: waits for T-3"
        );
    }

    #[test]
    fn status_serialises_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&InProgress).unwrap(),
            "\"in_progress\""
        );
    }
    #[test]
    fn the_reason_names_the_rule_that_decided_the_status() {
        let ws = WorkspaceId::from_number(2).unwrap();
        let p = ProposalId::from_number(3).unwrap();
        let reason = |f: &TaskFacts| status_reason(f, Some(ws), Some(p));
        assert_eq!(
            reason(&facts(true, WithReviewer, true, &[])),
            "Completed: a completion record exists"
        );
        assert_eq!(
            reason(&facts(false, WithReviewer, true, &[])),
            "Calculated: proposal P-3 waits for a reviewer"
        );
        assert_eq!(
            reason(&facts(false, WithReviewer, true, &[(5, InProgress)])),
            "Calculated: proposal P-3 waits for a reviewer, needs T-5"
        );
        assert_eq!(
            reason(&facts(false, NoProposal, true, &[])),
            "Calculated: workspace ws-2 is open, no proposal yet"
        );
        assert_eq!(
            reason(&facts(false, WithAuthor, true, &[(1, Todo)])),
            "Calculated: workspace ws-2 is open and proposal P-3 waits for its author, needs T-1"
        );
        assert_eq!(
            reason(&facts(false, NoProposal, false, &[(1, Done)])),
            "Calculated: no workspace yet and every prerequisite is Done"
        );
        assert_eq!(
            reason(&facts(
                false,
                NoProposal,
                false,
                &[(4, Todo), (1, Review), (2, Done)]
            )),
            "Calculated: waits for T-1, T-4"
        );
    }
}

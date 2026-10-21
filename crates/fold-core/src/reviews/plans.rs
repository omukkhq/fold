use std::time::Duration;

use crate::ids::TaskId;
use crate::tasks::TaskKind;
use crate::tasks::dag::Dag;
use crate::tasks::define_names;
use crate::tasks::status::TaskStatus;
use crate::time::Timestamp;

/// The most tasks one review plan holds, the tasks of its open proposals included, so the batch it becomes stays a size one reviewer reads at once.
pub const MAX_PLAN_TASKS: usize = 5;

/// How long a plan waits for its awaited tasks to open their proposals, counted from the plan's creation, which is when its first proposal starts to wait.
pub const PLAN_WAIT: Duration = Duration::from_secs(60 * 60);

/// When the wait of a plan created at `created_at` ends; `None` only at the end of time.
pub fn plan_deadline(created_at: Timestamp) -> Option<Timestamp> {
    created_at.checked_add(PLAN_WAIT)
}

define_names!(
    /// Where a review plan stands. An `open` plan waits for its awaited tasks or for the review lead to start it; `started` gave its open proposals to one reviewer; `ended` has no member left to review.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum PlanState {
        Open = "open",
        Started = "started",
        Ended = "ended",
    }
);

define_names!(
    /// Why a task cannot be awaited by a plan. The same reason takes an awaited task out of its plan when it stops being eligible, so a refusal and a leave never disagree.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum AwaitIssue {
        Archived = "archived",
        NotProposalKind = "not_proposal_kind",
        MainArea = "main_area",
        HumanReview = "human_review",
        Blocker = "blocker",
        DependsOnMember = "depends_on_member",
        Backlog = "backlog",
        InReview = "in_review",
        Done = "done",
    }
);

impl AwaitIssue {
    /// Why the task cannot be awaited, without its id.
    pub fn message(self) -> &'static str {
        match self {
            AwaitIssue::Archived => "is archived",
            AwaitIssue::NotProposalKind => "ends without a proposal",
            AwaitIssue::MainArea => "belongs to the area main, which no reviewer reviews",
            AwaitIssue::HumanReview => {
                "has the human review mark, so its proposal is reviewed alone"
            }
            AwaitIssue::Blocker => "has a blocker, so no agent works on it until a human clears it",
            AwaitIssue::DependsOnMember => {
                "needs the task of a plan member, so its proposal could only open after the plan's review"
            }
            AwaitIssue::Backlog => "is in Backlog, so it waits for its prerequisites",
            AwaitIssue::InReview => "is in Review without a proposal to join",
            AwaitIssue::Done => "is Done",
        }
    }
}

/// The facts of a task that decide whether a plan can wait for it. `depends_on_member` comes from [`depends_on_member`] over the plan's other tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwaitFacts {
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub archived: bool,
    pub human_review: bool,
    pub main_area: bool,
    pub blocker: bool,
    pub depends_on_member: bool,
}

/// A plan waits only for an agent task of kind `proposal` in To do or In progress that will open a proposal a reviewer agent reviews in a batch: not archived, not of the area main, without the human review mark and without a blocker, and not after a member's task, which would make the plan wait for itself.
pub fn await_issue(facts: &AwaitFacts) -> Option<AwaitIssue> {
    if facts.archived {
        return Some(AwaitIssue::Archived);
    }
    if facts.kind != TaskKind::Proposal {
        return Some(AwaitIssue::NotProposalKind);
    }
    // fold marks every task of main for a human, so main is named before the mark.
    if facts.main_area {
        return Some(AwaitIssue::MainArea);
    }
    if facts.human_review {
        return Some(AwaitIssue::HumanReview);
    }
    if facts.blocker {
        return Some(AwaitIssue::Blocker);
    }
    if facts.depends_on_member {
        return Some(AwaitIssue::DependsOnMember);
    }
    match facts.status {
        TaskStatus::Todo | TaskStatus::InProgress => None,
        TaskStatus::Backlog => Some(AwaitIssue::Backlog),
        TaskStatus::Review => Some(AwaitIssue::InReview),
        TaskStatus::Done => Some(AwaitIssue::Done),
    }
}

/// Whether `task` needs one of `members` other than itself, directly or through other tasks. `dag` holds the edges among the members and every task that needs one of them.
pub fn depends_on_member(dag: &Dag, task: TaskId, members: &[TaskId]) -> bool {
    members
        .iter()
        .filter(|member| **member != task)
        .any(|member| dag.dependent_closure([*member]).contains(&task))
}

define_names!(
    /// Why a member left its plan. `started` members are the proposals the plan gave to its reviewer, as one batch when there were two or more; the other reasons end a member's wait without a review.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum PlanRelease {
        Started = "started",
        NotOpened = "not_opened",
        Deadline = "deadline",
        Removed = "removed",
        Closed = "closed",
        Archived = "archived",
        NotProposalKind = "not_proposal_kind",
        MainArea = "main_area",
        HumanReview = "human_review",
        Blocker = "blocker",
        DependsOnMember = "depends_on_member",
        Backlog = "backlog",
        InReview = "in_review",
        Done = "done",
    }
);

impl From<AwaitIssue> for PlanRelease {
    fn from(issue: AwaitIssue) -> Self {
        match issue {
            AwaitIssue::Archived => PlanRelease::Archived,
            AwaitIssue::NotProposalKind => PlanRelease::NotProposalKind,
            AwaitIssue::MainArea => PlanRelease::MainArea,
            AwaitIssue::HumanReview => PlanRelease::HumanReview,
            AwaitIssue::Blocker => PlanRelease::Blocker,
            AwaitIssue::DependsOnMember => PlanRelease::DependsOnMember,
            AwaitIssue::Backlog => PlanRelease::Backlog,
            AwaitIssue::InReview => PlanRelease::InReview,
            AwaitIssue::Done => PlanRelease::Done,
        }
    }
}

define_names!(
    /// Why an open proposal cannot be put in a plan. Only a first version that nobody reviewed or claimed waits for a group; a later version is reviewed alone with its review scope.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum PlanProposalIssue {
        LaterVersion = "later_version",
        Reviewed = "reviewed",
        Claimed = "claimed",
        InBatch = "in_batch",
        HumanReview = "human_review",
    }
);

impl PlanProposalIssue {
    /// Why the proposal cannot wait, without its id.
    pub fn message(self) -> &'static str {
        match self {
            PlanProposalIssue::LaterVersion => {
                "has a version after its first, which is reviewed alone with its review scope"
            }
            PlanProposalIssue::Reviewed => "has a review already, so it never waits for a group",
            PlanProposalIssue::Claimed => "has a review claim, so its reviewer has it",
            PlanProposalIssue::InBatch => "is in a review batch",
            PlanProposalIssue::HumanReview => {
                "has the human review mark on its task, so it is reviewed alone"
            }
        }
    }
}

/// The facts of an open proposal that decide whether it can be put in a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanProposalFacts {
    pub current_version: u32,
    pub reviewed: bool,
    pub claimed: bool,
    pub in_batch: bool,
    pub human_review: bool,
}

/// An open proposal waits in a plan only at its first version, before any review, claim or batch, and without the human review mark, which has it reviewed alone.
pub fn proposal_issue(facts: &PlanProposalFacts) -> Option<PlanProposalIssue> {
    if facts.human_review {
        Some(PlanProposalIssue::HumanReview)
    } else if facts.current_version > 1 {
        Some(PlanProposalIssue::LaterVersion)
    } else if facts.reviewed {
        Some(PlanProposalIssue::Reviewed)
    } else if facts.claimed {
        Some(PlanProposalIssue::Claimed)
    } else if facts.in_batch {
        Some(PlanProposalIssue::InBatch)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    const ELIGIBLE: AwaitFacts = AwaitFacts {
        status: TaskStatus::Todo,
        kind: TaskKind::Proposal,
        archived: false,
        human_review: false,
        main_area: false,
        blocker: false,
        depends_on_member: false,
    };

    #[test]
    fn a_plan_waits_only_for_an_eligible_todo_or_in_progress_task() {
        assert_eq!(await_issue(&ELIGIBLE), None);
        let working = AwaitFacts {
            status: TaskStatus::InProgress,
            ..ELIGIBLE
        };
        assert_eq!(await_issue(&working), None);
        let cases = [
            (
                AwaitFacts {
                    archived: true,
                    ..ELIGIBLE
                },
                AwaitIssue::Archived,
            ),
            (
                AwaitFacts {
                    kind: TaskKind::NoProposal,
                    ..ELIGIBLE
                },
                AwaitIssue::NotProposalKind,
            ),
            (
                AwaitFacts {
                    kind: TaskKind::Human,
                    ..ELIGIBLE
                },
                AwaitIssue::NotProposalKind,
            ),
            (
                AwaitFacts {
                    main_area: true,
                    human_review: true,
                    ..ELIGIBLE
                },
                AwaitIssue::MainArea,
            ),
            (
                AwaitFacts {
                    human_review: true,
                    ..ELIGIBLE
                },
                AwaitIssue::HumanReview,
            ),
            (
                AwaitFacts {
                    blocker: true,
                    ..ELIGIBLE
                },
                AwaitIssue::Blocker,
            ),
            (
                AwaitFacts {
                    depends_on_member: true,
                    ..ELIGIBLE
                },
                AwaitIssue::DependsOnMember,
            ),
            (
                AwaitFacts {
                    status: TaskStatus::Backlog,
                    ..ELIGIBLE
                },
                AwaitIssue::Backlog,
            ),
            (
                AwaitFacts {
                    status: TaskStatus::Review,
                    ..ELIGIBLE
                },
                AwaitIssue::InReview,
            ),
            (
                AwaitFacts {
                    status: TaskStatus::Done,
                    ..ELIGIBLE
                },
                AwaitIssue::Done,
            ),
        ];
        for (facts, issue) in cases {
            assert_eq!(await_issue(&facts), Some(issue), "{facts:?}");
            assert_eq!(PlanRelease::from(issue).as_str(), issue.as_str());
            assert!(!issue.message().is_empty());
        }
    }

    #[test]
    fn a_task_after_a_member_directly_or_not_is_not_awaited() {
        // T-3 needs T-2, which needs T-1; T-4 stands alone.
        let dag = Dag::from_edges([(t(3), t(2)), (t(2), t(1))]);
        assert!(depends_on_member(&dag, t(3), &[t(1), t(3)]));
        assert!(depends_on_member(&dag, t(2), &[t(1)]));
        assert!(!depends_on_member(&dag, t(1), &[t(3), t(1)]));
        assert!(!depends_on_member(&dag, t(4), &[t(1), t(2), t(3)]));
        assert!(!depends_on_member(&dag, t(3), &[t(3)]));
    }

    #[test]
    fn only_an_unreviewed_first_version_waits_in_a_plan() {
        let fresh = PlanProposalFacts {
            current_version: 1,
            reviewed: false,
            claimed: false,
            in_batch: false,
            human_review: false,
        };
        assert_eq!(proposal_issue(&fresh), None);
        let cases = [
            (
                PlanProposalFacts {
                    current_version: 2,
                    ..fresh
                },
                PlanProposalIssue::LaterVersion,
            ),
            (
                PlanProposalFacts {
                    reviewed: true,
                    ..fresh
                },
                PlanProposalIssue::Reviewed,
            ),
            (
                PlanProposalFacts {
                    claimed: true,
                    ..fresh
                },
                PlanProposalIssue::Claimed,
            ),
            (
                PlanProposalFacts {
                    in_batch: true,
                    ..fresh
                },
                PlanProposalIssue::InBatch,
            ),
            (
                PlanProposalFacts {
                    human_review: true,
                    current_version: 3,
                    ..fresh
                },
                PlanProposalIssue::HumanReview,
            ),
        ];
        for (facts, issue) in cases {
            assert_eq!(proposal_issue(&facts), Some(issue), "{facts:?}");
            assert!(!issue.message().is_empty());
        }
    }

    #[test]
    fn the_wait_ends_an_hour_after_the_plan_is_made() {
        let created = Timestamp::from_millis(1_000).unwrap();
        assert_eq!(
            plan_deadline(created),
            Timestamp::from_millis(1_000 + 3_600_000).ok()
        );
        assert_eq!(PlanState::parse("open"), Some(PlanState::Open));
        assert_eq!(PlanRelease::parse("started"), Some(PlanRelease::Started));
    }
}

use std::collections::BTreeMap;
use std::time::Duration;

use crate::ids::{ProposalId, TaskId};
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
    /// How a task belongs with a proposal's task, closest first: a direct prerequisite of it, a checklist parent that links both, the request whose approval created both, or, weakest, the same area. Shared files are no part of it: the code and the tests or docs of one feature often touch different files. A task that needs the proposal's task is never awaited, since a plan cannot wait for it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Relation {
        Dependency = "dependency",
        SameParent = "same_parent",
        SameRequest = "same_request",
        SameArea = "same_area",
    }
);

impl Relation {
    /// A plan waits for a task only by a link of the same feature. One review of more proposals costs less than several, so an open proposal of the same area joins the group, but a plan never waits for a task only because it shares the area.
    pub fn can_wait(self) -> bool {
        self != Relation::SameArea
    }
}

/// The open proposal of a related task, with the facts that decide whether it can join a plan or a reviewer's group with the new proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelatedProposal {
    pub id: ProposalId,
    pub facts: PlanProposalFacts,
}

/// A task related to a proposal's task, with the facts a plan reads. `facts.depends_on_member` is ignored: [`plan_candidates`] decides it over the proposal's task and the other candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelatedTask {
    pub task: TaskId,
    pub relation: Relation,
    pub facts: AwaitFacts,
    /// The task's open proposal, which goes into a plan's proposals rather than its awaited tasks.
    pub open_proposal: Option<RelatedProposal>,
    /// The task is a member of a live plan, awaited or with its proposal.
    pub in_plan: bool,
}

/// A related task that goes into a plan or a reviewer's group with the new proposal: its open proposal joins, or the plan waits for the task when `proposal` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanCandidate {
    pub task: TaskId,
    pub relation: Relation,
    pub status: TaskStatus,
    pub proposal: Option<ProposalId>,
}

/// The related tasks of `task` that go with its new proposal, strongest relation first, then by id, at most [`MAX_PLAN_TASKS`] less the proposal's own task. A task related in several ways counts by its closest relation. An open proposal joins when `create_review_plan` and a reviewer's group accept it: no verdict, claim, batch or human review mark, not of main and in no plan. A task without one is awaited when `create_review_plan` accepts it by [`await_issue`] and its relation [`Relation::can_wait`]; one after the proposal's task or after another candidate is left out, since the plan would wait for itself, and its prerequisite stays. `dag` holds the edges among `task`, the related tasks and every task that needs one of them.
pub fn plan_candidates(task: TaskId, related: &[RelatedTask], dag: &Dag) -> Vec<PlanCandidate> {
    let mut closest: BTreeMap<TaskId, &RelatedTask> = BTreeMap::new();
    for item in related.iter().filter(|item| item.task != task) {
        closest
            .entry(item.task)
            .and_modify(|kept| {
                if item.relation < kept.relation {
                    *kept = item;
                }
            })
            .or_insert(item);
    }
    let eligible: Vec<&RelatedTask> = closest
        .into_values()
        .filter(|item| !item.in_plan)
        .filter(|item| match item.open_proposal {
            Some(proposal) => {
                !item.facts.main_area
                    && !item.facts.archived
                    && proposal_issue(&proposal.facts).is_none()
            }
            None => {
                item.relation.can_wait()
                    && await_issue(&AwaitFacts {
                        depends_on_member: false,
                        ..item.facts
                    })
                    .is_none()
            }
        })
        .collect();
    let members: Vec<TaskId> = std::iter::once(task)
        .chain(eligible.iter().map(|item| item.task))
        .collect();
    let mut candidates: Vec<PlanCandidate> = eligible
        .into_iter()
        .filter(|item| item.open_proposal.is_some() || !depends_on_member(dag, item.task, &members))
        .map(|item| PlanCandidate {
            task: item.task,
            relation: item.relation,
            status: item.facts.status,
            proposal: item.open_proposal.map(|proposal| proposal.id),
        })
        .collect();
    candidates.sort_by_key(|candidate| (candidate.relation, candidate.task));
    candidates.truncate(MAX_PLAN_TASKS - 1);
    candidates
}

/// The call that groups the new `proposal` with `candidates`, as fold names it to the review lead, and its tool: a plan that waits for the awaited tasks with every open proposal, or, when no task is awaited, one reviewer for the open proposals, since a plan needs a task to wait for.
pub fn plan_hint(proposal: ProposalId, candidates: &[PlanCandidate]) -> (String, &'static str) {
    let proposals: Vec<String> = std::iter::once(proposal)
        .chain(candidates.iter().filter_map(|candidate| candidate.proposal))
        .map(|id| id.to_string())
        .collect();
    let tasks: Vec<String> = candidates
        .iter()
        .filter(|candidate| candidate.proposal.is_none())
        .map(|candidate| candidate.task.to_string())
        .collect();
    if tasks.is_empty() {
        return (
            format!(
                "start_worker(role reviewer, proposals[{}]) gives them to one reviewer",
                proposals.join(", ")
            ),
            "start_worker",
        );
    }
    let them = if tasks.len() == 1 { "it" } else { "them" };
    (
        format!(
            "create_review_plan(proposals[{}], tasks[{}]) waits for {them}",
            proposals.join(", "),
            tasks.join(", ")
        ),
        "create_review_plan",
    )
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
        Reviewed = "reviewed",
        Claimed = "claimed",
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
    /// Why an open proposal cannot be put in a plan. Only a proposal that nobody reviewed or claimed waits for a group; a version after a review is reviewed alone with its review scope.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum PlanProposalIssue {
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
            PlanProposalIssue::Reviewed => {
                "has a review with a verdict, so its next version is reviewed alone with its review scope"
            }
            PlanProposalIssue::Claimed => "has a review claim, so its reviewer has it",
            PlanProposalIssue::InBatch => "is in a review batch",
            PlanProposalIssue::HumanReview => {
                "has the human review mark on its task, so it is reviewed alone"
            }
        }
    }
}

/// The facts of an open proposal that decide whether it can be put in a plan, or keep it in one. `reviewed` is a verdict on any of its versions; a review with only comments is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanProposalFacts {
    pub reviewed: bool,
    pub claimed: bool,
    pub in_batch: bool,
    pub human_review: bool,
}

/// An open proposal waits in a plan only before any verdict, claim or batch, and without the human review mark, which has it reviewed alone. A new version that its author makes before any review still waits.
pub fn proposal_issue(facts: &PlanProposalFacts) -> Option<PlanProposalIssue> {
    if facts.human_review {
        Some(PlanProposalIssue::HumanReview)
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

/// The facts of a joined member's proposal that decide whether it stays in its plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinedFacts {
    pub open: bool,
    pub main_area: bool,
    pub human_review: bool,
    /// A verdict on any version of the proposal; a review with only comments is none.
    pub reviewed: bool,
    pub claimed: bool,
}

/// Why a joined member leaves its plan, or `None` while it stays. A proposal leaves once it closes, comes into main, gets the human review mark, gets a verdict, such as a human's request for changes, or gets a review claim, so a version after a review is reviewed alone with its review scope. A new version without a review stays.
pub fn joined_release(facts: &JoinedFacts) -> Option<PlanRelease> {
    if !facts.open {
        Some(PlanRelease::Closed)
    } else if facts.main_area {
        Some(PlanRelease::MainArea)
    } else if facts.human_review {
        Some(PlanRelease::HumanReview)
    } else if facts.reviewed {
        Some(PlanRelease::Reviewed)
    } else if facts.claimed {
        Some(PlanRelease::Claimed)
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
    fn only_an_unreviewed_unclaimed_proposal_waits_in_a_plan() {
        let fresh = PlanProposalFacts {
            reviewed: false,
            claimed: false,
            in_batch: false,
            human_review: false,
        };
        assert_eq!(proposal_issue(&fresh), None);
        let cases = [
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
                    reviewed: true,
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
    fn a_joined_proposal_leaves_at_a_verdict_or_a_claim_but_not_at_a_new_version() {
        let waiting = JoinedFacts {
            open: true,
            main_area: false,
            human_review: false,
            reviewed: false,
            claimed: false,
        };
        assert_eq!(joined_release(&waiting), None);
        let cases = [
            (
                JoinedFacts {
                    open: false,
                    reviewed: true,
                    ..waiting
                },
                PlanRelease::Closed,
            ),
            (
                JoinedFacts {
                    main_area: true,
                    human_review: true,
                    ..waiting
                },
                PlanRelease::MainArea,
            ),
            (
                JoinedFacts {
                    human_review: true,
                    claimed: true,
                    ..waiting
                },
                PlanRelease::HumanReview,
            ),
            (
                JoinedFacts {
                    reviewed: true,
                    claimed: true,
                    ..waiting
                },
                PlanRelease::Reviewed,
            ),
            (
                JoinedFacts {
                    claimed: true,
                    ..waiting
                },
                PlanRelease::Claimed,
            ),
        ];
        for (facts, release) in cases {
            assert_eq!(joined_release(&facts), Some(release), "{facts:?}");
        }
    }

    fn related(number: u32, relation: Relation, status: TaskStatus) -> RelatedTask {
        RelatedTask {
            task: t(number),
            relation,
            facts: AwaitFacts { status, ..ELIGIBLE },
            open_proposal: None,
            in_plan: false,
        }
    }

    fn p(number: u32) -> ProposalId {
        ProposalId::from_number(number).unwrap()
    }

    const UNREVIEWED: PlanProposalFacts = PlanProposalFacts {
        reviewed: false,
        claimed: false,
        in_batch: false,
        human_review: false,
    };

    /// A related task in Review with the open proposal `proposal` that nobody reviewed.
    fn open(number: u32, relation: Relation, proposal: u32) -> RelatedTask {
        RelatedTask {
            open_proposal: Some(RelatedProposal {
                id: p(proposal),
                facts: UNREVIEWED,
            }),
            ..related(number, relation, TaskStatus::Review)
        }
    }

    fn candidate(number: u32, relation: Relation, status: TaskStatus) -> PlanCandidate {
        PlanCandidate {
            task: t(number),
            relation,
            status,
            proposal: None,
        }
    }

    fn joining(number: u32, relation: Relation, proposal: u32) -> PlanCandidate {
        PlanCandidate {
            proposal: Some(p(proposal)),
            ..candidate(number, relation, TaskStatus::Review)
        }
    }

    #[test]
    fn a_plan_can_wait_for_related_tasks_that_can_still_open_a_proposal() {
        // T-1 has the new proposal and needs T-2; T-5 needs T-1; T-6 needs T-4.
        let dag = Dag::from_edges([(t(1), t(2)), (t(5), t(1)), (t(6), t(4))]);
        let related = [
            related(4, Relation::SameRequest, TaskStatus::Todo),
            related(3, Relation::SameParent, TaskStatus::InProgress),
            related(2, Relation::Dependency, TaskStatus::InProgress),
            related(5, Relation::SameRequest, TaskStatus::Todo),
            related(6, Relation::SameRequest, TaskStatus::Todo),
            // The proposal's own task never waits for itself.
            related(1, Relation::SameRequest, TaskStatus::Review),
        ];
        assert_eq!(
            plan_candidates(t(1), &related, &dag),
            vec![
                candidate(2, Relation::Dependency, TaskStatus::InProgress),
                candidate(3, Relation::SameParent, TaskStatus::InProgress),
                candidate(4, Relation::SameRequest, TaskStatus::Todo),
            ]
        );
    }

    #[test]
    fn a_related_task_the_plan_would_refuse_is_left_out() {
        let dag = Dag::default();
        let mut refused = vec![
            RelatedTask {
                in_plan: true,
                ..related(3, Relation::SameRequest, TaskStatus::Todo)
            },
            related(4, Relation::SameRequest, TaskStatus::Backlog),
            related(5, Relation::SameRequest, TaskStatus::Done),
            related(6, Relation::SameRequest, TaskStatus::Review),
            // A plan never waits for a task only because it shares the area.
            related(12, Relation::SameArea, TaskStatus::Todo),
            RelatedTask {
                in_plan: true,
                ..open(13, Relation::SameRequest, 13)
            },
            RelatedTask {
                facts: AwaitFacts {
                    status: TaskStatus::Review,
                    main_area: true,
                    ..ELIGIBLE
                },
                ..open(14, Relation::SameArea, 14)
            },
        ];
        for (number, change) in [
            (
                7,
                AwaitFacts {
                    blocker: true,
                    ..ELIGIBLE
                },
            ),
            (
                8,
                AwaitFacts {
                    human_review: true,
                    ..ELIGIBLE
                },
            ),
            (
                9,
                AwaitFacts {
                    main_area: true,
                    ..ELIGIBLE
                },
            ),
            (
                10,
                AwaitFacts {
                    archived: true,
                    ..ELIGIBLE
                },
            ),
            (
                11,
                AwaitFacts {
                    kind: TaskKind::NoProposal,
                    ..ELIGIBLE
                },
            ),
        ] {
            refused.push(RelatedTask {
                facts: change,
                ..related(number, Relation::SameParent, TaskStatus::Todo)
            });
        }
        // An open proposal with a verdict, a claim, a batch or the human review mark is reviewed alone.
        for (number, facts) in [
            (
                15,
                PlanProposalFacts {
                    reviewed: true,
                    ..UNREVIEWED
                },
            ),
            (
                16,
                PlanProposalFacts {
                    claimed: true,
                    ..UNREVIEWED
                },
            ),
            (
                17,
                PlanProposalFacts {
                    in_batch: true,
                    ..UNREVIEWED
                },
            ),
            (
                18,
                PlanProposalFacts {
                    human_review: true,
                    ..UNREVIEWED
                },
            ),
        ] {
            refused.push(RelatedTask {
                open_proposal: Some(RelatedProposal {
                    id: p(number),
                    facts,
                }),
                ..related(number, Relation::SameRequest, TaskStatus::Review)
            });
        }
        assert_eq!(plan_candidates(t(1), &refused, &dag), Vec::new());
    }

    #[test]
    fn open_proposals_join_with_the_stronger_links_first_and_the_same_area_last() {
        // T-6 needs T-1, but its open proposal can still be reviewed with T-1's: only a wait for it would wait for the plan.
        let dag = Dag::from_edges([(t(6), t(1))]);
        let related = [
            open(8, Relation::SameArea, 8),
            open(6, Relation::SameRequest, 6),
            related(3, Relation::SameRequest, TaskStatus::Todo),
            open(7, Relation::SameArea, 7),
            open(2, Relation::Dependency, 2),
        ];
        let found = plan_candidates(t(1), &related, &dag);
        assert_eq!(
            found,
            vec![
                joining(2, Relation::Dependency, 2),
                candidate(3, Relation::SameRequest, TaskStatus::Todo),
                joining(6, Relation::SameRequest, 6),
                joining(7, Relation::SameArea, 7),
            ]
        );
        assert_eq!(
            plan_hint(p(1), &found),
            (
                "create_review_plan(proposals[P-1, P-2, P-6, P-7], tasks[T-3]) waits for it"
                    .to_owned(),
                "create_review_plan"
            )
        );
        // With no task left to wait for, one reviewer gets the open proposals together, since a plan needs a task to wait for.
        let only_open = plan_candidates(t(1), &[open(7, Relation::SameArea, 7)], &dag);
        assert_eq!(
            plan_hint(p(1), &only_open),
            (
                "start_worker(role reviewer, proposals[P-1, P-7]) gives them to one reviewer"
                    .to_owned(),
                "start_worker"
            )
        );
        assert_eq!(
            plan_hint(
                p(1),
                &[
                    candidate(3, Relation::SameRequest, TaskStatus::Todo),
                    candidate(4, Relation::SameRequest, TaskStatus::Todo)
                ]
            )
            .0,
            "create_review_plan(proposals[P-1], tasks[T-3, T-4]) waits for them"
        );
        assert!(Relation::SameRequest.can_wait() && !Relation::SameArea.can_wait());
    }

    proptest::proptest! {
        /// Whatever the graph and the facts, every task fold names is one `create_review_plan` accepts with the proposal's task and the other named tasks, by its closest relation: an open proposal that may join a plan, or a task the plan may wait for by a link stronger than the area. The list fits a plan, strongest link first.
        #[test]
        fn every_candidate_is_a_task_the_plan_accepts(
            edges in proptest::collection::vec((1u32..=10, 1u32..=10), 0..16),
            facts in proptest::collection::vec(
                (0usize..5, proptest::bool::ANY, proptest::bool::ANY, 0usize..6, proptest::bool::ANY),
                10,
            ),
            links in proptest::collection::vec((1u32..=10, 0usize..4), 0..20),
        ) {
            // An edge always points to a lower number, so the graph has no cycle; T-5 has the proposal.
            let dag = Dag::from_edges(
                edges
                    .into_iter()
                    .filter(|(task, needs)| task > needs)
                    .map(|(task, needs)| (t(task), t(needs))),
            );
            let statuses = [
                TaskStatus::Backlog,
                TaskStatus::Todo,
                TaskStatus::InProgress,
                TaskStatus::Review,
                TaskStatus::Done,
            ];
            let relations = [
                Relation::Dependency,
                Relation::SameParent,
                Relation::SameRequest,
                Relation::SameArea,
            ];
            // 0 is no open proposal, 1 one nobody reviewed, and 2 to 5 one with a verdict, a claim, a batch or the human review mark.
            let proposal_of = |number: u32, kind: usize| {
                (kind > 0).then(|| RelatedProposal {
                    id: p(number),
                    facts: PlanProposalFacts {
                        reviewed: kind == 2,
                        claimed: kind == 3,
                        in_batch: kind == 4,
                        human_review: kind == 5,
                    },
                })
            };
            let related: Vec<RelatedTask> = links
                .iter()
                .map(|&(number, relation)| {
                    let (status, blocker, archived, proposal, in_plan) =
                        facts[number as usize - 1];
                    RelatedTask {
                        task: t(number),
                        relation: relations[relation],
                        facts: AwaitFacts {
                            status: statuses[status],
                            blocker,
                            archived,
                            ..ELIGIBLE
                        },
                        open_proposal: proposal_of(number, proposal),
                        in_plan,
                    }
                })
                .collect();
            let found = plan_candidates(t(5), &related, &dag);
            proptest::prop_assert!(found.len() < MAX_PLAN_TASKS);
            let ordered = found
                .windows(2)
                .all(|pair| (pair[0].relation, pair[0].task) < (pair[1].relation, pair[1].task));
            proptest::prop_assert!(ordered);
            let members: Vec<TaskId> = std::iter::once(t(5))
                .chain(found.iter().map(|candidate| candidate.task))
                .collect();
            for candidate in &found {
                proptest::prop_assert_ne!(candidate.task, t(5));
                let closest = related
                    .iter()
                    .filter(|item| item.task == candidate.task)
                    .min_by_key(|item| item.relation);
                proptest::prop_assert!(closest.is_some());
                let Some(closest) = closest else { continue };
                proptest::prop_assert_eq!(candidate.relation, closest.relation);
                proptest::prop_assert_eq!(candidate.status, closest.facts.status);
                proptest::prop_assert!(!closest.in_plan);
                proptest::prop_assert_eq!(
                    candidate.proposal,
                    closest.open_proposal.map(|proposal| proposal.id)
                );
                match closest.open_proposal {
                    Some(proposal) => {
                        proptest::prop_assert_eq!(proposal_issue(&proposal.facts), None);
                        proptest::prop_assert!(!closest.facts.archived && !closest.facts.main_area);
                    }
                    None => {
                        proptest::prop_assert!(candidate.relation.can_wait());
                        let facts = AwaitFacts {
                            depends_on_member: depends_on_member(&dag, candidate.task, &members),
                            ..closest.facts
                        };
                        proptest::prop_assert_eq!(await_issue(&facts), None);
                    }
                }
            }
        }
    }

    #[test]
    fn a_task_counts_by_its_closest_relation_and_the_list_fits_a_plan() {
        // T-3 needs T-2, so a plan that waits for both would wait for itself; T-2 stays.
        let dag = Dag::from_edges([(t(3), t(2))]);
        let mut items = vec![
            related(2, Relation::SameRequest, TaskStatus::Todo),
            related(2, Relation::Dependency, TaskStatus::Todo),
            related(3, Relation::SameRequest, TaskStatus::Todo),
        ];
        items.extend(
            (10..16).map(|number| related(number, Relation::SameRequest, TaskStatus::Todo)),
        );
        // Open proposals of the same area come after the stronger links and fall off the end.
        items.extend((20..23).map(|number| open(number, Relation::SameArea, number)));
        let found = plan_candidates(t(1), &items, &dag);
        assert_eq!(found.len(), MAX_PLAN_TASKS - 1);
        assert_eq!(
            found[0],
            candidate(2, Relation::Dependency, TaskStatus::Todo)
        );
        assert_eq!(
            found.iter().map(|found| found.task).collect::<Vec<_>>(),
            [t(2), t(10), t(11), t(12)]
        );
        assert_eq!(Relation::parse("same_parent"), Some(Relation::SameParent));
        assert_eq!(Relation::parse("same_area"), Some(Relation::SameArea));
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

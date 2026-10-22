use serde_json::json;
use thiserror::Error;

use super::{AgentStanding, can_human_approve};
use crate::agents::{AgentName, HumanName};
use crate::error::{FoldError, codes};
use crate::reviews::{
    HumanReviewMark, ReviewFact, Verdict, counted_approval, human_approval_standing,
    latest_verdict, main_approval,
};
use crate::{TaskId, TaskStatus};

/// Who asks to integrate. A named human integrates whatever the agents decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerFacts {
    /// An agent with its standing as it is at the time of the call.
    Agent {
        agent: AgentName,
        standing: AgentStanding,
    },
    Human(HumanName),
    /// fold itself, integrating a ready proposal without a call. Only the automatic integration job acts as this caller. It needs what an agent that decides reviews needs: a standing approval by such an agent or by a human, and, on a marked task, a human approval the mark accepts.
    Automatic,
}

/// The facts of the proposal the gates read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalFacts {
    pub open: bool,
    /// The agent of the worker that opened the proposal.
    pub author: AgentName,
    pub current_version: u32,
}

/// Everything the gates decide on, loaded by the caller so every decision can be replayed from stored inputs.
#[derive(Debug, Clone, Copy)]
pub struct IntegrateInput<'a> {
    pub caller: &'a CallerFacts,
    pub proposal: &'a ProposalFacts,
    /// The version the caller saw, when it sends one.
    pub expected_version: Option<u32>,
    /// The reviews of the current version only, because approvals belong to the version they reviewed.
    pub reviews: &'a [ReviewFact],
    pub human_review: Option<&'a HumanReviewMark>,
    /// The task is in the area Main: a human approves it without a reviewer agent first, and without the human review mark the main agent's approval of its own proposal counts.
    pub main_task: bool,
    /// The status of each direct prerequisite.
    pub prerequisites: &'a [(TaskId, TaskStatus)],
}

/// Why integration is refused. It carries ids, names and statuses only, never a review body, comment or diff.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GateRefusal {
    #[error("the proposal is not open")]
    ProposalNotOpen,
    #[error("the caller saw v{expected}, but the current version is v{current}")]
    StaleVersion { expected: u32, current: u32 },
    #[error("only the main agent, the review lead, a reviewer or a human can integrate a proposal")]
    IntegratorRequired,
    #[error("the author's agent cannot integrate its own proposal")]
    AuthorCannotIntegrate,
    #[error("the latest verdict on the current version asks for changes")]
    ChangesRequested,
    #[error("the current version has no approval by an agent that decides reviews or a human")]
    NotApproved,
    #[error("the task's human review mark waits for a human approval")]
    HumanApprovalMissing { reviewer: Option<HumanName> },
    #[error(
        "a human integrates a task with the human review mark only after a reviewer agent approved the version"
    )]
    AgentApprovalRequired,
    #[error("a prerequisite of the task is not Done")]
    PrerequisitesNotDone {
        unfinished: Vec<(TaskId, TaskStatus)>,
    },
}

impl GateRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            GateRefusal::ProposalNotOpen => codes::PROPOSAL_NOT_OPEN,
            GateRefusal::StaleVersion { .. } => codes::STALE_VERSION,
            GateRefusal::IntegratorRequired => codes::INTEGRATOR_REQUIRED,
            GateRefusal::AuthorCannotIntegrate => codes::AUTHOR_CANNOT_INTEGRATE,
            GateRefusal::ChangesRequested => codes::CHANGES_REQUESTED,
            GateRefusal::NotApproved => codes::NOT_APPROVED,
            GateRefusal::HumanApprovalMissing { .. } => codes::HUMAN_APPROVAL_MISSING,
            GateRefusal::AgentApprovalRequired => codes::AGENT_APPROVAL_REQUIRED,
            GateRefusal::PrerequisitesNotDone { .. } => codes::PREREQUISITES_NOT_DONE,
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            GateRefusal::ProposalNotOpen => "call get_proposal to see its state",
            GateRefusal::StaleVersion { .. } => {
                "call get_proposal and review the current version before integrating"
            }
            GateRefusal::IntegratorRequired => {
                "call list_workers to find the review lead, or ask a human to integrate"
            }
            GateRefusal::AuthorCannotIntegrate => {
                "call list_workers to find the review lead, or ask a human to integrate"
            }
            GateRefusal::ChangesRequested => {
                "call get_review_comments to read the requested changes, then wait for the author's next version"
            }
            GateRefusal::NotApproved => {
                "ask the review lead to call submit_review with approve, or ask a human to approve"
            }
            GateRefusal::HumanApprovalMissing { .. } => {
                "call send_message to ask the named human, or any human when none is named, to approve the current version"
            }
            GateRefusal::AgentApprovalRequired => {
                "ask the review lead to review and approve the current version first"
            }
            GateRefusal::PrerequisitesNotDone { .. } => {
                "call get_task on each prerequisite in the details and integrate it first"
            }
        }
    }
}

impl From<GateRefusal> for FoldError {
    fn from(refusal: GateRefusal) -> Self {
        let error = FoldError::new(refusal.code(), refusal.to_string()).with_hint(refusal.hint());
        match &refusal {
            GateRefusal::StaleVersion { expected, current } => error.with_details([
                ("expected_version", json!(expected)),
                ("current_version", json!(current)),
            ]),
            GateRefusal::HumanApprovalMissing { reviewer } => {
                error.with_details([("reviewer", json!(reviewer.as_ref().map(HumanName::as_str)))])
            }
            GateRefusal::PrerequisitesNotDone { unfinished } => {
                let list: Vec<_> = unfinished
                    .iter()
                    .map(|(task, status)| json!({"task": task.to_string(), "status": status.as_str()}))
                    .collect();
                error.with_details([("prerequisites", json!(list))])
            }
            GateRefusal::ProposalNotOpen
            | GateRefusal::IntegratorRequired
            | GateRefusal::AuthorCannotIntegrate
            | GateRefusal::ChangesRequested
            | GateRefusal::NotApproved
            | GateRefusal::AgentApprovalRequired => error,
        }
    }
}

/// The gates in the order `can_integrate` checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Open,
    Version,
    Caller,
    Approval,
    HumanReview,
    Prerequisites,
    /// The version merges cleanly with the current main; only the integration procedure can tell.
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateStatus {
    Met,
    Unmet(GateRefusal),
    CheckedAtIntegration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateState {
    pub gate: Gate,
    pub status: GateStatus,
}

/// The first refusal in gate order, or `Ok` when every gate this module decides is met. The merge gate is left to the integration procedure.
pub fn can_integrate(input: &IntegrateInput<'_>) -> Result<(), GateRefusal> {
    match checks(input).into_iter().find_map(|(_, refusal)| refusal) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// The state of every gate, never stopping at the first unmet one, for the proposal screen and `get_proposal`.
pub fn gate_report(input: &IntegrateInput<'_>) -> Vec<GateState> {
    checks(input)
        .into_iter()
        .map(|(gate, refusal)| GateState {
            gate,
            status: refusal.map_or(GateStatus::Met, GateStatus::Unmet),
        })
        .chain([GateState {
            gate: Gate::Merge,
            status: GateStatus::CheckedAtIntegration,
        }])
        .collect()
}

fn checks(input: &IntegrateInput<'_>) -> [(Gate, Option<GateRefusal>); 6] {
    [
        (Gate::Open, open(input)),
        (Gate::Version, version(input)),
        (Gate::Caller, caller(input)),
        (Gate::Approval, approval(input)),
        (Gate::HumanReview, human_review(input)),
        (Gate::Prerequisites, prerequisites(input)),
    ]
}

fn open(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    (!input.proposal.open).then_some(GateRefusal::ProposalNotOpen)
}

fn version(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    let current = input.proposal.current_version;
    input
        .expected_version
        .filter(|expected| *expected != current)
        .map(|expected| GateRefusal::StaleVersion { expected, current })
}

/// The author's agent is refused before its standing is read, as it is for a verdict.
fn caller(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    match input.caller {
        CallerFacts::Human(_) | CallerFacts::Automatic => None,
        CallerFacts::Agent { agent, .. } if *agent == input.proposal.author => {
            Some(GateRefusal::AuthorCannotIntegrate)
        }
        CallerFacts::Agent { standing, .. } if standing.acts_on_reviews() => None,
        CallerFacts::Agent { .. } => Some(GateRefusal::IntegratorRequired),
    }
}

/// A human caller approves in the same action. Every other caller needs a standing approval that counts, whoever gave the latest one, unless the latest verdict asks for changes or rejects.
fn approval(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    if matches!(input.caller, CallerFacts::Human(_)) {
        return None;
    }
    match latest_verdict(input.reviews).map(|latest| latest.verdict) {
        Some(Verdict::RequestChanges) => Some(GateRefusal::ChangesRequested),
        Some(Verdict::Reject) => Some(GateRefusal::NotApproved),
        Some(Verdict::Approve | Verdict::Comment) | None
            if counted_approval(input.reviews, &input.proposal.author)
                || (input.main_task
                    && input.human_review.is_none()
                    && main_approval(input.reviews)) =>
        {
            None
        }
        Some(Verdict::Approve | Verdict::Comment) | None => Some(GateRefusal::NotApproved),
    }
}

/// A human who integrates approves in the same action, so on a marked task the human integrates only after a reviewer agent approved the version, as [`can_human_approve`] decides for an approval.
fn human_review(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    if matches!(input.caller, CallerFacts::Human(_))
        && can_human_approve(
            input.human_review,
            input.reviews,
            &input.proposal.author,
            input.main_task,
        )
        .is_err()
    {
        return Some(GateRefusal::AgentApprovalRequired);
    }
    let integrating = match input.caller {
        CallerFacts::Human(name) => Some(name),
        CallerFacts::Agent { .. } | CallerFacts::Automatic => None,
    };
    if human_approval_standing(input.human_review, input.reviews, integrating) {
        return None;
    }
    Some(GateRefusal::HumanApprovalMissing {
        reviewer: input.human_review.and_then(|mark| mark.reviewer.clone()),
    })
}

fn prerequisites(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    prerequisites_done(input.prerequisites).err()
}

/// A task becomes Done only after each of its direct prerequisites, so both the integration of its proposal and the lead check of a task without a proposal refuse while one is not Done. The refusal lists the unfinished prerequisites in number order.
pub fn prerequisites_done(prerequisites: &[(TaskId, TaskStatus)]) -> Result<(), GateRefusal> {
    let mut unfinished: Vec<(TaskId, TaskStatus)> = prerequisites
        .iter()
        .filter(|(_, status)| *status != TaskStatus::Done)
        .copied()
        .collect();
    if unfinished.is_empty() {
        return Ok(());
    }
    unfinished.sort_by_key(|(task, _)| *task);
    Err(GateRefusal::PrerequisitesNotDone { unfinished })
}

/// The lead check of a task without a proposal stands in for the review of a proposal, so a task with the human review mark is completed only by a human: the human's check is the human review. An agent's complete of a marked task is refused with `human_approval_missing`, which names the mark's reviewer.
pub fn human_check_done(
    mark: Option<&HumanReviewMark>,
    checker_is_human: bool,
) -> Result<(), GateRefusal> {
    match mark {
        Some(mark) if !checker_is_human => Err(GateRefusal::HumanApprovalMissing {
            reviewer: mark.reviewer.clone(),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::agents::roles::Role;
    use crate::events::EventActor;
    use crate::reviews::Approver;

    const AUTHOR: &str = "backend";

    fn name(text: &str) -> HumanName {
        HumanName::parse(text).unwrap()
    }

    fn task(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    fn agent_caller(agent: &str, role: Option<Role>) -> CallerFacts {
        CallerFacts::Agent {
            agent: AgentName::parse(agent).unwrap(),
            standing: AgentStanding {
                holds_main: false,
                role,
            },
        }
    }

    /// The review lead `review`.
    fn decider() -> CallerFacts {
        agent_caller("review", Some(Role::ReviewLead))
    }

    fn human(text: &str) -> CallerFacts {
        CallerFacts::Human(name(text))
    }

    /// A review by `who`: `decider` (an agent that decides reviews), `author-decider`, `agent` or `human:<name>`.
    fn review(verdict: Verdict, who: &str, seq: u64) -> ReviewFact {
        let (reviewer, approver) = match who {
            "decider" => (
                EventActor::parse("review").unwrap(),
                Approver::DecidingAgent,
            ),
            "author-decider" => (
                EventActor::parse("backend/ci").unwrap(),
                Approver::DecidingAgent,
            ),
            "agent" => (EventActor::parse("frontend").unwrap(), Approver::OtherAgent),
            human => {
                let human = name(human.trim_start_matches(EventActor::HUMAN_PREFIX));
                (EventActor::human(&human), Approver::Human(human))
            }
        };
        ReviewFact {
            verdict,
            reviewer,
            approver,
            seq,
        }
    }

    fn mark(reviewer: Option<&str>) -> HumanReviewMark {
        HumanReviewMark {
            reviewer: reviewer.map(name),
            reason: "touches auth".to_owned(),
            set_by: EventActor::parse("planner").unwrap(),
        }
    }

    #[derive(Debug, Clone)]
    struct Case {
        caller: CallerFacts,
        open: bool,
        expected_version: Option<u32>,
        reviews: Vec<ReviewFact>,
        mark: Option<HumanReviewMark>,
        main_task: bool,
        prerequisites: Vec<(TaskId, TaskStatus)>,
    }

    impl Case {
        /// The review lead as caller on an open v2 that it approved, with one Done prerequisite.
        fn new() -> Self {
            Self {
                caller: decider(),
                open: true,
                expected_version: None,
                reviews: vec![review(Verdict::Approve, "decider", 1)],
                mark: None,
                main_task: false,
                prerequisites: vec![(task(2), TaskStatus::Done)],
            }
        }

        fn run<T>(&self, decide: impl FnOnce(&IntegrateInput<'_>) -> T) -> T {
            let proposal = ProposalFacts {
                open: self.open,
                author: AgentName::parse(AUTHOR).unwrap(),
                current_version: 2,
            };
            decide(&IntegrateInput {
                caller: &self.caller,
                proposal: &proposal,
                expected_version: self.expected_version,
                reviews: &self.reviews,
                human_review: self.mark.as_ref(),
                main_task: self.main_task,
                prerequisites: &self.prerequisites,
            })
        }
    }

    #[test]
    fn each_gate_refuses_with_its_own_reason() {
        use GateRefusal::*;
        use Verdict::{Approve, Comment, Reject, RequestChanges};
        let cases: Vec<(&str, Case, Result<(), GateRefusal>)> = vec![
            ("approved by the review lead", Case::new(), Ok(())),
            (
                "a reviewer as caller",
                Case {
                    caller: agent_caller("reviewer-1", Some(Role::Reviewer)),
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "the main agent as caller",
                Case {
                    caller: CallerFacts::Agent {
                        agent: AgentName::parse("main").unwrap(),
                        standing: AgentStanding {
                            holds_main: true,
                            role: None,
                        },
                    },
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "an area lead as caller",
                Case {
                    caller: agent_caller("web-lead", Some(Role::AreaLead)),
                    ..Case::new()
                },
                Err(IntegratorRequired),
            ),
            (
                "a task worker as caller",
                Case {
                    caller: agent_caller("web-worker", Some(Role::TaskWorker)),
                    ..Case::new()
                },
                Err(IntegratorRequired),
            ),
            (
                "an unsupervised agent without the main tag",
                Case {
                    caller: agent_caller("frontend", None),
                    ..Case::new()
                },
                Err(IntegratorRequired),
            ),
            (
                "author's agent as caller",
                Case {
                    caller: agent_caller(AUTHOR, Some(Role::ReviewLead)),
                    ..Case::new()
                },
                Err(AuthorCannotIntegrate),
            ),
            (
                "human caller without a prior approval",
                Case {
                    caller: human("tanim"),
                    reviews: Vec::new(),
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "human caller with a prior approval",
                Case {
                    caller: human("tanim"),
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "human caller after an earlier request_changes",
                Case {
                    caller: human("tanim"),
                    reviews: vec![review(RequestChanges, "agent", 1)],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "stale expected version",
                Case {
                    expected_version: Some(1),
                    ..Case::new()
                },
                Err(StaleVersion {
                    expected: 1,
                    current: 2,
                }),
            ),
            (
                "current expected version",
                Case {
                    expected_version: Some(2),
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "no approval",
                Case {
                    reviews: vec![review(Comment, "agent", 1)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "approval by an agent that does not decide reviews",
                Case {
                    reviews: vec![review(Approve, "agent", 1)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "approval by a deciding agent of the author's agent",
                Case {
                    reviews: vec![review(Approve, "author-decider", 1)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "approval by a human",
                Case {
                    reviews: vec![review(Approve, "human:ana", 1)],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "approval then request_changes",
                Case {
                    reviews: vec![
                        review(Approve, "decider", 1),
                        review(RequestChanges, "agent", 2),
                    ],
                    ..Case::new()
                },
                Err(ChangesRequested),
            ),
            (
                "request_changes then approval",
                Case {
                    reviews: vec![
                        review(RequestChanges, "agent", 1),
                        review(Approve, "decider", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "approval then reject",
                Case {
                    reviews: vec![review(Approve, "decider", 1), review(Reject, "agent", 2)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "a deciding approval then an approval that does not count",
                Case {
                    reviews: vec![review(Approve, "decider", 1), review(Approve, "agent", 2)],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "approval then a comment review",
                Case {
                    reviews: vec![review(Approve, "decider", 1), review(Comment, "agent", 2)],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "marked task without a standing human approval",
                Case {
                    mark: Some(mark(Some("tanim"))),
                    ..Case::new()
                },
                Err(HumanApprovalMissing {
                    reviewer: Some(name("tanim")),
                }),
            ),
            (
                "marked task with a standing human approval",
                Case {
                    mark: Some(mark(None)),
                    reviews: vec![
                        review(Approve, "human:ana", 1),
                        review(Approve, "decider", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "an integrating human as the named reviewer after a reviewer agent's approval",
                Case {
                    caller: human("tanim"),
                    mark: Some(mark(Some("tanim"))),
                    reviews: vec![review(Approve, "decider", 1)],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "an integrating human as the named reviewer before any reviewer agent's approval",
                Case {
                    caller: human("tanim"),
                    mark: Some(mark(Some("tanim"))),
                    reviews: Vec::new(),
                    ..Case::new()
                },
                Err(AgentApprovalRequired),
            ),
            (
                "an integrating human who is not the named reviewer",
                Case {
                    caller: human("ana"),
                    mark: Some(mark(Some("tanim"))),
                    ..Case::new()
                },
                Err(HumanApprovalMissing {
                    reviewer: Some(name("tanim")),
                }),
            ),
            (
                "one unfinished prerequisite",
                Case {
                    prerequisites: vec![
                        (task(2), TaskStatus::Done),
                        (task(3), TaskStatus::InProgress),
                    ],
                    ..Case::new()
                },
                Err(PrerequisitesNotDone {
                    unfinished: vec![(task(3), TaskStatus::InProgress)],
                }),
            ),
            (
                "several unfinished prerequisites",
                Case {
                    prerequisites: vec![
                        (task(5), TaskStatus::Review),
                        (task(2), TaskStatus::Done),
                        (task(4), TaskStatus::Backlog),
                    ],
                    ..Case::new()
                },
                Err(PrerequisitesNotDone {
                    unfinished: vec![
                        (task(4), TaskStatus::Backlog),
                        (task(5), TaskStatus::Review),
                    ],
                }),
            ),
            (
                "closed proposal",
                Case {
                    open: false,
                    ..Case::new()
                },
                Err(ProposalNotOpen),
            ),
            (
                "automatic after a deciding approval",
                Case {
                    caller: CallerFacts::Automatic,
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "automatic after a human approval alone",
                Case {
                    caller: CallerFacts::Automatic,
                    reviews: vec![review(Approve, "human:ana", 1)],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "automatic after an approval that does not count",
                Case {
                    caller: CallerFacts::Automatic,
                    reviews: vec![review(Approve, "agent", 1)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "automatic after a deciding then a human's approval",
                Case {
                    caller: CallerFacts::Automatic,
                    reviews: vec![
                        review(Approve, "decider", 1),
                        review(Approve, "human:ana", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "automatic after a deciding approval and a request_changes",
                Case {
                    caller: CallerFacts::Automatic,
                    reviews: vec![
                        review(Approve, "decider", 1),
                        review(RequestChanges, "agent", 2),
                    ],
                    ..Case::new()
                },
                Err(ChangesRequested),
            ),
            (
                "automatic on a marked task without a human approval",
                Case {
                    caller: CallerFacts::Automatic,
                    mark: Some(mark(None)),
                    ..Case::new()
                },
                Err(HumanApprovalMissing { reviewer: None }),
            ),
            (
                "automatic on a marked task with the human approval before the agent's",
                Case {
                    caller: CallerFacts::Automatic,
                    mark: Some(mark(Some("tanim"))),
                    reviews: vec![
                        review(Approve, "human:tanim", 1),
                        review(Approve, "decider", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "automatic on a marked task approved by another human than the named one",
                Case {
                    caller: CallerFacts::Automatic,
                    mark: Some(mark(Some("tanim"))),
                    reviews: vec![review(Approve, "human:ana", 1)],
                    ..Case::new()
                },
                Err(HumanApprovalMissing {
                    reviewer: Some(name("tanim")),
                }),
            ),
            (
                "automatic on a marked task with the human approval after the agent's",
                Case {
                    caller: CallerFacts::Automatic,
                    mark: Some(mark(Some("tanim"))),
                    reviews: vec![
                        review(Approve, "decider", 1),
                        review(Approve, "human:tanim", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "automatic on a marked task with a second agent approval after the human's",
                Case {
                    caller: CallerFacts::Automatic,
                    mark: Some(mark(Some("tanim"))),
                    reviews: vec![
                        review(Approve, "decider", 1),
                        review(Approve, "human:tanim", 2),
                        review(Approve, "decider", 3),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "automatic with an unfinished prerequisite",
                Case {
                    caller: CallerFacts::Automatic,
                    prerequisites: vec![(task(3), TaskStatus::Todo)],
                    ..Case::new()
                },
                Err(PrerequisitesNotDone {
                    unfinished: vec![(task(3), TaskStatus::Todo)],
                }),
            ),
        ];
        for (name, case, expected) in cases {
            assert_eq!(case.run(can_integrate), expected, "{name}");
            let report = case.run(gate_report);
            let unmet = report.iter().find_map(|state| match &state.status {
                GateStatus::Unmet(refusal) => Some(refusal.clone()),
                GateStatus::Met | GateStatus::CheckedAtIntegration => None,
            });
            assert_eq!(unmet.map_or(Ok(()), Err), expected, "{name} report");
        }
    }

    #[test]
    fn the_first_refusal_wins_and_the_report_shows_every_gate() {
        let case = Case {
            caller: agent_caller("frontend", None),
            open: false,
            expected_version: Some(1),
            reviews: Vec::new(),
            mark: Some(mark(None)),
            main_task: false,
            prerequisites: vec![(task(3), TaskStatus::Todo)],
        };
        assert_eq!(case.run(can_integrate), Err(GateRefusal::ProposalNotOpen));
        let gates: Vec<(Gate, bool)> = case
            .run(gate_report)
            .into_iter()
            .map(|state| (state.gate, matches!(state.status, GateStatus::Unmet(_))))
            .collect();
        assert_eq!(
            gates,
            [
                (Gate::Open, true),
                (Gate::Version, true),
                (Gate::Caller, true),
                (Gate::Approval, true),
                (Gate::HumanReview, true),
                (Gate::Prerequisites, true),
                (Gate::Merge, false),
            ]
        );
        let report = Case::new().run(gate_report);
        assert!(
            report[..6]
                .iter()
                .all(|state| state.status == GateStatus::Met)
        );
        assert_eq!(report[6].status, GateStatus::CheckedAtIntegration);
    }

    #[test]
    fn refusals_become_errors_with_ids_only() {
        let all = [
            (GateRefusal::ProposalNotOpen, codes::PROPOSAL_NOT_OPEN, None),
            (
                GateRefusal::StaleVersion {
                    expected: 1,
                    current: 3,
                },
                codes::STALE_VERSION,
                Some(json!({"expected_version": 1, "current_version": 3})),
            ),
            (
                GateRefusal::IntegratorRequired,
                codes::INTEGRATOR_REQUIRED,
                None,
            ),
            (
                GateRefusal::AuthorCannotIntegrate,
                codes::AUTHOR_CANNOT_INTEGRATE,
                None,
            ),
            (
                GateRefusal::ChangesRequested,
                codes::CHANGES_REQUESTED,
                None,
            ),
            (GateRefusal::NotApproved, codes::NOT_APPROVED, None),
            (
                GateRefusal::HumanApprovalMissing {
                    reviewer: Some(name("tanim")),
                },
                codes::HUMAN_APPROVAL_MISSING,
                Some(json!({"reviewer": "tanim"})),
            ),
            (
                GateRefusal::HumanApprovalMissing { reviewer: None },
                codes::HUMAN_APPROVAL_MISSING,
                Some(json!({"reviewer": null})),
            ),
            (
                GateRefusal::PrerequisitesNotDone {
                    unfinished: vec![(task(3), TaskStatus::InProgress)],
                },
                codes::PREREQUISITES_NOT_DONE,
                Some(json!({"prerequisites": [{"task": "T-3", "status": "in_progress"}]})),
            ),
        ];
        for (refusal, code, details) in all {
            assert_eq!(refusal.code(), code);
            let message = refusal.to_string();
            let error = FoldError::from(refusal);
            assert_eq!(error.code(), code);
            assert_eq!(error.message(), message);
            assert!(error.hint().is_some_and(|hint| !hint.is_empty()));
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                details,
                "{code}"
            );
        }
    }

    #[derive(Debug, Clone)]
    enum Who {
        Decider,
        AuthorDecider,
        Agent,
        Human(&'static str),
    }

    fn who() -> impl Strategy<Value = Who> {
        prop_oneof![
            Just(Who::Decider),
            Just(Who::AuthorDecider),
            Just(Who::Agent),
            Just(Who::Human("tanim")),
            Just(Who::Human("ana")),
        ]
    }

    fn verdict() -> impl Strategy<Value = Verdict> {
        prop_oneof![
            Just(Verdict::Comment),
            Just(Verdict::RequestChanges),
            Just(Verdict::Approve),
            Just(Verdict::Reject),
        ]
    }

    fn caller() -> impl Strategy<Value = CallerFacts> {
        prop_oneof![
            Just(decider()),
            Just(agent_caller("reviewer-1", Some(Role::Reviewer))),
            Just(agent_caller("frontend", None)),
            Just(agent_caller("web-worker", Some(Role::TaskWorker))),
            Just(agent_caller(AUTHOR, Some(Role::ReviewLead))),
            Just(human("tanim")),
            Just(human("ana")),
            Just(CallerFacts::Automatic),
        ]
    }

    fn status() -> impl Strategy<Value = TaskStatus> {
        prop_oneof![
            4 => Just(TaskStatus::Done),
            1 => Just(TaskStatus::Backlog),
            1 => Just(TaskStatus::InProgress),
        ]
    }

    /// Reviews whose `seq` values are distinct but arrive in any order, so the oracle must find "latest" itself.
    fn reviews() -> impl Strategy<Value = Vec<ReviewFact>> {
        prop::collection::vec((verdict(), who()), 0..6)
            .prop_flat_map(|entries| {
                let count = entries.len();
                (
                    Just(entries),
                    Just((1..=count as u64).collect::<Vec<_>>()).prop_shuffle(),
                )
            })
            .prop_map(|(entries, seqs)| {
                entries
                    .into_iter()
                    .zip(seqs)
                    .map(|((verdict, who), seq)| {
                        let label = match who {
                            Who::Decider => "decider".to_owned(),
                            Who::AuthorDecider => "author-decider".to_owned(),
                            Who::Agent => "agent".to_owned(),
                            Who::Human(human) => format!("human:{human}"),
                        };
                        review(verdict, &label, seq)
                    })
                    .collect()
            })
    }

    fn mark_strategy() -> impl Strategy<Value = Option<HumanReviewMark>> {
        prop_oneof![
            Just(None),
            Just(Some(mark(None))),
            Just(Some(mark(Some("tanim")))),
        ]
    }

    fn case() -> impl Strategy<Value = Case> {
        (
            caller(),
            prop::bool::weighted(0.9),
            prop::option::of(1_u32..=3),
            reviews(),
            mark_strategy(),
            prop::bool::weighted(0.3),
            prop::collection::vec(status(), 0..3),
        )
            .prop_map(
                |(caller, open, expected_version, reviews, mark, main_task, statuses)| Case {
                    caller,
                    open,
                    expected_version,
                    reviews,
                    mark,
                    main_task,
                    prerequisites: statuses
                        .into_iter()
                        .zip(2..)
                        .map(|(status, number)| (task(number), status))
                        .collect(),
                },
            )
    }

    /// The rule written out without the module's helpers: scan for the highest `seq` by hand.
    fn oracle(case: &Case) -> bool {
        let human_caller = match &case.caller {
            CallerFacts::Human(name) => Some(name.as_str()),
            CallerFacts::Agent { .. } | CallerFacts::Automatic => None,
        };
        let caller_ok = match &case.caller {
            CallerFacts::Human(_) | CallerFacts::Automatic => true,
            CallerFacts::Agent { agent, standing } => {
                agent.as_str() != AUTHOR
                    && matches!(standing.role, Some(Role::ReviewLead | Role::Reviewer))
            }
        };
        let last_request = case
            .reviews
            .iter()
            .filter(|review| review.verdict == Verdict::RequestChanges)
            .map(|review| review.seq)
            .max();
        let standing = |review: &&ReviewFact| {
            review.verdict == Verdict::Approve && last_request.is_none_or(|seq| review.seq > seq)
        };
        let counted = case.reviews.iter().filter(standing).any(|review| {
            matches!(review.approver, Approver::Human(_))
                || review.reviewer.as_str() == "review"
                || (case.main_task
                    && case.mark.is_none()
                    && review.approver == Approver::DecidingAgent)
        });
        let mut latest: Option<&ReviewFact> = None;
        for review in &case.reviews {
            if review.verdict != Verdict::Comment && latest.is_none_or(|l| review.seq > l.seq) {
                latest = Some(review);
            }
        }
        let approved = human_caller.is_some()
            || (latest.is_none_or(|review| review.verdict == Verdict::Approve) && counted);
        let agent_approved = case
            .reviews
            .iter()
            .filter(standing)
            .any(|review| review.reviewer.as_str() == "review");
        let human_ok = match &case.mark {
            None => true,
            Some(_) if human_caller.is_some() && !agent_approved && !case.main_task => false,
            Some(mark) => {
                let accepts = |who: &str| mark.reviewer.as_ref().is_none_or(|r| r.as_str() == who);
                human_caller.is_some_and(accepts)
                    || case.reviews.iter().filter(standing).any(|approval| {
                        matches!(&approval.approver, Approver::Human(h) if accepts(h.as_str()))
                    })
            }
        };
        let version_ok = case.expected_version.is_none_or(|expected| expected == 2);
        let done = case
            .prerequisites
            .iter()
            .all(|(_, status)| *status == TaskStatus::Done);
        case.open && version_ok && caller_ok && approved && human_ok && done
    }

    #[test]
    fn a_human_integrates_a_marked_task_only_after_a_reviewer_agent_approved() {
        let marked = mark(None);
        let proposal = ProposalFacts {
            open: true,
            author: AgentName::parse(AUTHOR).unwrap(),
            current_version: 2,
        };
        let run = |reviews: &[ReviewFact]| {
            can_integrate(&IntegrateInput {
                caller: &human("tanim"),
                proposal: &proposal,
                expected_version: Some(2),
                reviews,
                human_review: Some(&marked),
                main_task: false,
                prerequisites: &[],
            })
        };
        assert_eq!(run(&[]), Err(GateRefusal::AgentApprovalRequired));
        assert_eq!(
            run(&[review(Verdict::Approve, "agent", 1)]),
            Err(GateRefusal::AgentApprovalRequired)
        );
        assert_eq!(
            run(&[review(Verdict::Approve, "author-decider", 1)]),
            Err(GateRefusal::AgentApprovalRequired)
        );
        assert_eq!(run(&[review(Verdict::Approve, "decider", 1)]), Ok(()));
        let error = FoldError::from(GateRefusal::AgentApprovalRequired);
        assert_eq!(error.code(), codes::AGENT_APPROVAL_REQUIRED);
        assert!(error.hint().unwrap_or_default().contains("review lead"));
    }

    #[test]
    fn a_task_of_main_takes_a_human_approval_alone_or_the_authors_on_the_users_word() {
        use Verdict::Approve;
        let main_case = |caller: CallerFacts, reviews: Vec<ReviewFact>, marked: bool| Case {
            caller,
            reviews,
            mark: marked.then(|| mark(None)),
            main_task: true,
            ..Case::new()
        };
        assert_eq!(
            main_case(human("tanim"), vec![], true).run(can_integrate),
            Ok(())
        );
        assert_eq!(
            main_case(
                CallerFacts::Automatic,
                vec![review(Approve, "human:tanim", 1)],
                true
            )
            .run(can_integrate),
            Ok(())
        );
        assert_eq!(
            main_case(
                CallerFacts::Automatic,
                vec![review(Approve, "author-decider", 1)],
                true
            )
            .run(can_integrate),
            Err(GateRefusal::NotApproved)
        );
        assert_eq!(
            main_case(
                CallerFacts::Automatic,
                vec![review(Approve, "author-decider", 1)],
                false
            )
            .run(can_integrate),
            Ok(())
        );
        assert_eq!(
            main_case(
                agent_caller(AUTHOR, None),
                vec![review(Approve, "author-decider", 1)],
                false
            )
            .run(can_integrate),
            Err(GateRefusal::AuthorCannotIntegrate)
        );
        assert_eq!(
            Case {
                caller: CallerFacts::Automatic,
                reviews: vec![review(Approve, "author-decider", 1)],
                ..Case::new()
            }
            .run(can_integrate),
            Err(GateRefusal::NotApproved)
        );
    }

    proptest! {
        #[test]
        fn can_integrate_matches_the_oracle_and_the_report_agrees(case in case()) {
            let decided = case.run(can_integrate);
            prop_assert_eq!(decided.is_ok(), oracle(&case));
            let report = case.run(gate_report);
            prop_assert_eq!(report.len(), 7);
            let first_unmet = report.iter().find_map(|state| match &state.status {
                GateStatus::Unmet(refusal) => Some(refusal.clone()),
                GateStatus::Met | GateStatus::CheckedAtIntegration => None,
            });
            prop_assert_eq!(first_unmet.map_or(Ok(()), Err), decided);
        }

        /// The caller passes only the current version's reviews; reviews of other versions, filtered out the same way, never change the decision.
        #[test]
        fn reviews_of_other_versions_cannot_change_the_result(
            case in case(),
            others in reviews(),
            other_version in prop::sample::select(vec![1_u32, 3]),
        ) {
            let alone = case.run(can_integrate);
            let mut tagged: Vec<(u32, ReviewFact)> =
                case.reviews.iter().cloned().map(|review| (2, review)).collect();
            tagged.extend(others.into_iter().map(|review| (other_version, review)));
            let current: Vec<ReviewFact> = tagged
                .into_iter()
                .filter(|(version, _)| *version == 2)
                .map(|(_, review)| review)
                .collect();
            let filtered = Case { reviews: current, ..case };
            prop_assert_eq!(filtered.run(can_integrate), alone);
        }
    }

    #[test]
    fn a_marked_task_without_a_proposal_is_completed_only_by_a_human() {
        assert_eq!(human_check_done(None, false), Ok(()));
        assert_eq!(human_check_done(None, true), Ok(()));
        let named = mark(Some("ana"));
        assert_eq!(human_check_done(Some(&named), true), Ok(()));
        let refusal = human_check_done(Some(&named), false).unwrap_err();
        assert_eq!(refusal.code(), codes::HUMAN_APPROVAL_MISSING);
        assert_eq!(
            refusal,
            GateRefusal::HumanApprovalMissing {
                reviewer: Some(name("ana"))
            }
        );
        assert_eq!(
            human_check_done(Some(&mark(None)), false),
            Err(GateRefusal::HumanApprovalMissing { reviewer: None })
        );
    }
}

use serde_json::json;
use thiserror::Error;

use crate::agents::{AgentName, HumanName, Tag};
use crate::error::{FoldError, codes};
use crate::reviews::{
    Approver, HumanReviewMark, ReviewFact, Verdict, human_approval_standing, latest_verdict,
};
use crate::{TaskId, TaskStatus};

/// Who asks to integrate. The gates never read a human's tags: a named human integrates without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerFacts {
    /// An agent with its tags as they are at the time of the call.
    Agent {
        agent: AgentName,
        tags: Vec<Tag>,
    },
    Human(HumanName),
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
    #[error("only an integrator or a human can integrate a proposal")]
    IntegratorRequired,
    #[error("the author's agent cannot integrate its own proposal")]
    AuthorCannotIntegrate,
    #[error("the latest verdict on the current version asks for changes")]
    ChangesRequested,
    #[error("the current version has no approval by an integrator or a human")]
    NotApproved,
    #[error("the task's human review mark waits for a human approval")]
    HumanApprovalMissing { reviewer: Option<HumanName> },
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
                "call list_agents to find an integrator, or ask a human to integrate"
            }
            GateRefusal::AuthorCannotIntegrate => {
                "call list_agents to find another integrator, or ask a human to integrate"
            }
            GateRefusal::ChangesRequested => {
                "call get_review_comments to read the requested changes, then wait for the author's next version"
            }
            GateRefusal::NotApproved => {
                "call submit_review with approve as an integrator, or ask a human to approve"
            }
            GateRefusal::HumanApprovalMissing { .. } => {
                "call send_message to ask the named human, or any human when none is named, to approve the current version"
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
            | GateRefusal::NotApproved => error,
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

/// The author's agent is refused before the tag is read, as it is for a verdict.
fn caller(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    match input.caller {
        CallerFacts::Human(_) => None,
        CallerFacts::Agent { agent, .. } if *agent == input.proposal.author => {
            Some(GateRefusal::AuthorCannotIntegrate)
        }
        CallerFacts::Agent { tags, .. } if tags.iter().any(Tag::is_integrator) => None,
        CallerFacts::Agent { .. } => Some(GateRefusal::IntegratorRequired),
    }
}

/// A human caller approves in the same action, so the latest verdict is that approval.
fn approval(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    if matches!(input.caller, CallerFacts::Human(_)) {
        return None;
    }
    match latest_verdict(input.reviews) {
        Some(latest) if latest.verdict == Verdict::RequestChanges => {
            Some(GateRefusal::ChangesRequested)
        }
        Some(latest)
            if latest.verdict == Verdict::Approve && counts(latest, &input.proposal.author) =>
        {
            None
        }
        Some(_) | None => Some(GateRefusal::NotApproved),
    }
}

/// An approval counts when a human gave it, or an agent that carries `integrator` and is not the author's agent.
fn counts(review: &ReviewFact, author: &AgentName) -> bool {
    match &review.approver {
        Approver::Human(_) => true,
        Approver::IntegratorAgent => review.reviewer.agent() != Some(author.as_str()),
        Approver::OtherAgent => false,
    }
}

fn human_review(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    let integrating = match input.caller {
        CallerFacts::Human(name) => Some(name),
        CallerFacts::Agent { .. } => None,
    };
    if human_approval_standing(input.human_review, input.reviews, integrating) {
        return None;
    }
    Some(GateRefusal::HumanApprovalMissing {
        reviewer: input.human_review.and_then(|mark| mark.reviewer.clone()),
    })
}

fn prerequisites(input: &IntegrateInput<'_>) -> Option<GateRefusal> {
    let mut unfinished: Vec<(TaskId, TaskStatus)> = input
        .prerequisites
        .iter()
        .filter(|(_, status)| *status != TaskStatus::Done)
        .copied()
        .collect();
    if unfinished.is_empty() {
        return None;
    }
    unfinished.sort_by_key(|(task, _)| *task);
    Some(GateRefusal::PrerequisitesNotDone { unfinished })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::events::EventActor;

    const AUTHOR: &str = "backend";

    fn name(text: &str) -> HumanName {
        HumanName::parse(text).unwrap()
    }

    fn task(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    fn agent_caller(agent: &str, tags: &[&str]) -> CallerFacts {
        CallerFacts::Agent {
            agent: AgentName::parse(agent).unwrap(),
            tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
        }
    }

    fn integrator() -> CallerFacts {
        agent_caller("review", &["integrator"])
    }

    fn human(text: &str) -> CallerFacts {
        CallerFacts::Human(name(text))
    }

    /// A review by `who`: `integrator`, `author-integrator`, `agent` or `human:<name>`.
    fn review(verdict: Verdict, who: &str, seq: u64) -> ReviewFact {
        let (reviewer, approver) = match who {
            "integrator" => (
                EventActor::parse("review").unwrap(),
                Approver::IntegratorAgent,
            ),
            "author-integrator" => (
                EventActor::parse("backend/ci").unwrap(),
                Approver::IntegratorAgent,
            ),
            "agent" => (EventActor::parse("frontend").unwrap(), Approver::OtherAgent),
            human => {
                let human = name(human.trim_start_matches("human:"));
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
        prerequisites: Vec<(TaskId, TaskStatus)>,
    }

    impl Case {
        /// An integrator caller on an open v2 approved by another integrator, with one Done prerequisite.
        fn new() -> Self {
            Self {
                caller: integrator(),
                open: true,
                expected_version: None,
                reviews: vec![review(Verdict::Approve, "integrator", 1)],
                mark: None,
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
                prerequisites: &self.prerequisites,
            })
        }
    }

    #[test]
    fn each_gate_refuses_with_its_own_reason() {
        use GateRefusal::*;
        use Verdict::{Approve, Comment, Reject, RequestChanges};
        let cases: Vec<(&str, Case, Result<(), GateRefusal>)> = vec![
            ("approved by an integrator", Case::new(), Ok(())),
            (
                "caller without the tag",
                Case {
                    caller: agent_caller("frontend", &["ui"]),
                    ..Case::new()
                },
                Err(IntegratorRequired),
            ),
            (
                "author's agent as caller",
                Case {
                    caller: agent_caller(AUTHOR, &["integrator"]),
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
                "approval by a non-integrator",
                Case {
                    reviews: vec![review(Approve, "agent", 1)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "approval by an integrator of the author's agent",
                Case {
                    reviews: vec![review(Approve, "author-integrator", 1)],
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
                        review(Approve, "integrator", 1),
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
                        review(Approve, "integrator", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "approval then reject",
                Case {
                    reviews: vec![review(Approve, "integrator", 1), review(Reject, "agent", 2)],
                    ..Case::new()
                },
                Err(NotApproved),
            ),
            (
                "approval then a comment review",
                Case {
                    reviews: vec![
                        review(Approve, "integrator", 1),
                        review(Comment, "agent", 2),
                    ],
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
                        review(Approve, "integrator", 2),
                    ],
                    ..Case::new()
                },
                Ok(()),
            ),
            (
                "an integrating human as the named reviewer",
                Case {
                    caller: human("tanim"),
                    mark: Some(mark(Some("tanim"))),
                    reviews: Vec::new(),
                    ..Case::new()
                },
                Ok(()),
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
            caller: agent_caller("frontend", &[]),
            open: false,
            expected_version: Some(1),
            reviews: Vec::new(),
            mark: Some(mark(None)),
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
        Integrator,
        AuthorIntegrator,
        Agent,
        Human(&'static str),
    }

    fn who() -> impl Strategy<Value = Who> {
        prop_oneof![
            Just(Who::Integrator),
            Just(Who::AuthorIntegrator),
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
            Just(integrator()),
            Just(agent_caller("frontend", &[])),
            Just(agent_caller(AUTHOR, &["integrator"])),
            Just(human("tanim")),
            Just(human("ana")),
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
                            Who::Integrator => "integrator".to_owned(),
                            Who::AuthorIntegrator => "author-integrator".to_owned(),
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
            prop::collection::vec(status(), 0..3),
        )
            .prop_map(
                |(caller, open, expected_version, reviews, mark, statuses)| Case {
                    caller,
                    open,
                    expected_version,
                    reviews,
                    mark,
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
            CallerFacts::Agent { .. } => None,
        };
        let caller_ok = match &case.caller {
            CallerFacts::Human(_) => true,
            CallerFacts::Agent { agent, tags } => {
                agent.as_str() != AUTHOR && tags.iter().any(|tag| tag.as_str() == "integrator")
            }
        };
        let mut latest: Option<&ReviewFact> = None;
        for review in &case.reviews {
            if review.verdict != Verdict::Comment && latest.is_none_or(|l| review.seq > l.seq) {
                latest = Some(review);
            }
        }
        let approved = human_caller.is_some()
            || latest.is_some_and(|review| {
                review.verdict == Verdict::Approve
                    && match &review.approver {
                        Approver::Human(_) => true,
                        Approver::IntegratorAgent => review.reviewer.as_str() == "review",
                        Approver::OtherAgent => false,
                    }
            });
        let human_ok = match &case.mark {
            None => true,
            Some(mark) => {
                let accepts = |who: &str| mark.reviewer.as_ref().is_none_or(|r| r.as_str() == who);
                human_caller.is_some_and(accepts) || case.reviews.iter().any(|approval| {
                    approval.verdict == Verdict::Approve
                        && matches!(&approval.approver, Approver::Human(h) if accepts(h.as_str()))
                        && !case.reviews.iter().any(|later| {
                            later.verdict == Verdict::RequestChanges && later.seq > approval.seq
                        })
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
}

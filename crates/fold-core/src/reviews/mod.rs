use crate::actor::AgentActor;
use crate::agents::{AgentName, HumanName};
use crate::events::EventActor;
use crate::tasks::define_names;

pub mod batches;
pub mod checks;
pub mod plans;

/// The longest reason a review command takes, in characters: why a proposal closes, why a reviewer gives a review claim up, or why a human must look.
pub const MAX_REASON_CHARS: usize = 500;

/// The trimmed reason is stored with the human review mark and never repeats rejected text in an error.
pub fn human_review_reason(reason: &str) -> Result<String, crate::FoldError> {
    let reason = reason.trim();
    let problem = if reason.is_empty() {
        "the reason must not be empty".to_owned()
    } else if reason.chars().count() > MAX_REASON_CHARS {
        format!("the reason must be at most {MAX_REASON_CHARS} characters")
    } else {
        return Ok(reason.to_owned());
    };
    Err(
        crate::FoldError::new(crate::codes::INVALID_REQUEST, problem)
            .with_details([("field", serde_json::json!("reason"))])
            .with_hint("say in one line why a human must look at the work"),
    )
}

/// The longest intent a proposal version takes, in characters.
pub const MAX_INTENT_CHARS: usize = 2000;

/// The intent of a proposal version: what the change does, why, what it leaves as it is and its risks, which a human reads beside the reviewer's summary instead of every line. It is stored trimmed, with each line break as `\n`, and becomes the body of the version's Git commit. An empty or too long intent is `invalid_intent`, whose details name the reason and never repeat the text; `again` names the call that sends it again.
pub fn proposal_intent(intent: &str, again: &str) -> Result<String, crate::FoldError> {
    let intent = intent.trim().replace("\r\n", "\n").replace('\r', "\n");
    let reason = if intent.is_empty() {
        "empty"
    } else if intent.chars().count() > MAX_INTENT_CHARS {
        "too_long"
    } else {
        return Ok(intent);
    };
    Err(crate::FoldError::new(
        crate::codes::INVALID_INTENT,
        format!("the intent must be 1 to {MAX_INTENT_CHARS} characters of text"),
    )
    .with_details([
        ("field", serde_json::json!("intent")),
        ("reason", serde_json::json!(reason)),
        ("max_chars", serde_json::json!(MAX_INTENT_CHARS)),
    ])
    .with_hint(format!(
        "call {again} again with an intent of at most {MAX_INTENT_CHARS} characters: what the change does, why, what it does not change and its risks"
    )))
}

define_names!(
    /// What a review says about one proposal version. `Comment` is not a verdict on the work: it leaves the turn where it was.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Verdict {
        Comment = "comment",
        RequestChanges = "request_changes",
        Approve = "approve",
        Reject = "reject",
    }
);

/// Who a policy gate admitted to review: an agent through one of its workers, or a named human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reviewer<'a> {
    Agent(&'a AgentActor),
    Human(&'a HumanName),
}

/// Which tree a line comment reads: the version's base revision (`Old`) or the version itself (`New`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Old,
    New,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Old => "old",
            Side::New => "new",
        }
    }

    pub fn parse(text: &str) -> Option<Side> {
        match text {
            "old" => Some(Side::Old),
            "new" => Some(Side::New),
            _ => None,
        }
    }
}

/// The weight of a review's author, read from the agent's tag and role when the review is read rather than when it was written, so an agent that hands the `main` tag on withdraws the weight of its approvals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approver {
    /// An agent that [acts on reviews](crate::policy::AgentStanding::acts_on_reviews), whose approval counts.
    DecidingAgent,
    OtherAgent,
    Human(HumanName),
}

/// One review of a proposal version as the turn and the integration gates read it. `seq` orders the reviews of a version: a higher `seq` is a later review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFact {
    pub verdict: Verdict,
    pub reviewer: EventActor,
    pub approver: Approver,
    pub seq: u64,
}

/// The review with the highest `seq` whose verdict is not `comment`, because a comment is not a verdict. Pass the reviews of one version only: approvals belong to the version they reviewed.
pub fn latest_verdict(reviews: &[ReviewFact]) -> Option<&ReviewFact> {
    reviews
        .iter()
        .filter(|review| review.verdict != Verdict::Comment)
        .max_by_key(|review| review.seq)
}

/// The latest verdict on the current version asks for changes, which puts the proposal with its author. A later approval on the same version gives it back to the reviewer; a comment changes nothing.
pub fn changes_requested(reviews: &[ReviewFact]) -> bool {
    latest_verdict(reviews).is_some_and(|review| review.verdict == Verdict::RequestChanges)
}

/// Where a proposal stands for the pill on a card and the proposal block of a task. A conflict comes first, because it puts the proposal with its author whatever the verdicts say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStanding {
    Open,
    Approved,
    ChangesRequested,
    Conflict,
    Integrated,
}

impl ProposalStanding {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStanding::Open => "open",
            ProposalStanding::Approved => "approved",
            ProposalStanding::ChangesRequested => "changes_requested",
            ProposalStanding::Conflict => "conflict",
            ProposalStanding::Integrated => "integrated",
        }
    }

    /// `latest` is the verdict of [`latest_verdict`] on the current version. Only `request_changes` moves the card to its author, so a `reject` leaves the proposal open, as the status calculation reads it.
    pub fn of(integrated: bool, conflicted: bool, latest: Option<Verdict>) -> Self {
        if integrated {
            return ProposalStanding::Integrated;
        }
        if conflicted {
            return ProposalStanding::Conflict;
        }
        match latest {
            Some(Verdict::RequestChanges) => ProposalStanding::ChangesRequested,
            Some(Verdict::Approve) => ProposalStanding::Approved,
            Some(Verdict::Comment | Verdict::Reject) | None => ProposalStanding::Open,
        }
    }
}

/// A task's request that a human looks at its proposal before it integrates. `reviewer` names the one human whose approval counts; `None` lets any human approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanReviewMark {
    pub reviewer: Option<HumanName>,
    pub reason: String,
    /// The worker or human that set the mark.
    pub set_by: EventActor,
}

impl HumanReviewMark {
    /// Whether an approval by `human` satisfies the mark.
    pub fn accepts(&self, human: &HumanName) -> bool {
        self.reviewer
            .as_ref()
            .is_none_or(|reviewer| reviewer == human)
    }
}

/// Whether the human review a mark asks for is met. Pass the reviews of the current version only, because approvals belong to the version they reviewed. A human who integrates approves in the same action, so `integrating_human` counts as an approval when the mark accepts that human. A recorded human approval stands until a later `request_changes` on the same version; an agent's approval never counts, whatever its role.
pub fn human_approval_standing(
    mark: Option<&HumanReviewMark>,
    reviews: &[ReviewFact],
    integrating_human: Option<&HumanName>,
) -> bool {
    let Some(mark) = mark else {
        return true;
    };
    if integrating_human.is_some_and(|human| mark.accepts(human)) {
        return true;
    }
    let withdrawn = last_request_changes(reviews);
    reviews.iter().any(|review| {
        review.verdict == Verdict::Approve
            && matches!(&review.approver, Approver::Human(human) if mark.accepts(human))
            && withdrawn.is_none_or(|seq| review.seq > seq)
    })
}

/// The agent whose approval of the version stands: the latest approval by an agent after the last `request_changes`, or `None` when no agent's approval stands. Pass the reviews of one version only.
pub fn standing_agent_approval(reviews: &[ReviewFact]) -> Option<AgentName> {
    let withdrawn = last_request_changes(reviews);
    reviews
        .iter()
        .filter(|review| {
            review.verdict == Verdict::Approve && withdrawn.is_none_or(|seq| review.seq > seq)
        })
        .filter_map(|review| Some((review.seq, AgentName::parse(review.reviewer.agent()?).ok()?)))
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, agent)| agent)
}

/// Every reviewer whose approval of the version stands: an approval after the last `request_changes`, workers and humans alike, each once, in the order of their first standing approval. Pass the reviews of one version only.
pub fn standing_approvers(reviews: &[ReviewFact]) -> Vec<EventActor> {
    let withdrawn = last_request_changes(reviews);
    let mut standing: Vec<&ReviewFact> = reviews
        .iter()
        .filter(|review| {
            review.verdict == Verdict::Approve && withdrawn.is_none_or(|seq| review.seq > seq)
        })
        .collect();
    standing.sort_by_key(|review| review.seq);
    let mut approvers: Vec<EventActor> = Vec::new();
    for review in standing {
        if !approvers.contains(&review.reviewer) {
            approvers.push(review.reviewer.clone());
        }
    }
    approvers
}

/// Whether an approval by an agent that decides reviews, other than `author`'s, stands on the version: one after the last `request_changes`. Pass the reviews of one version only.
pub fn deciding_approval(reviews: &[ReviewFact], author: &AgentName) -> bool {
    let withdrawn = last_request_changes(reviews);
    reviews.iter().any(|review| {
        review.verdict == Verdict::Approve
            && review.approver == Approver::DecidingAgent
            && review.reviewer.agent() != Some(author.as_str())
            && withdrawn.is_none_or(|seq| review.seq > seq)
    })
}

/// Whether an approval by an agent that decides reviews stands on the version, one after the last `request_changes`, the author's own included: on a task of Main without the human review mark the main agent approves its own proposal. Pass the reviews of one version only.
pub fn main_approval(reviews: &[ReviewFact]) -> bool {
    let withdrawn = last_request_changes(reviews);
    reviews.iter().any(|review| {
        review.verdict == Verdict::Approve
            && review.approver == Approver::DecidingAgent
            && withdrawn.is_none_or(|seq| review.seq > seq)
    })
}

/// Whether an approval that integration counts stands on the version: [`deciding_approval`], or a human's approval after the last `request_changes`. Any standing approval counts, not only the latest verdict, so a later approval by an agent without the right to decide takes nothing away. Pass the reviews of one version only.
pub fn counted_approval(reviews: &[ReviewFact], author: &AgentName) -> bool {
    let withdrawn = last_request_changes(reviews);
    deciding_approval(reviews, author)
        || reviews.iter().any(|review| {
            review.verdict == Verdict::Approve
                && matches!(review.approver, Approver::Human(_))
                && withdrawn.is_none_or(|seq| review.seq > seq)
        })
}

/// The newest version before `version` that has a verdict, a comment not counted, so a reviewer of `version` can read only what changed since it. Anyone's verdict counts, an agent's or a human's: the next version is often reviewed by another agent than the earlier one, because each reviewer fold starts is an agent of its own.
pub fn last_verdict_version(
    reviews: impl IntoIterator<Item = (u32, Verdict)>,
    version: u32,
) -> Option<u32> {
    reviews
        .into_iter()
        .filter(|(reviewed, verdict)| *verdict != Verdict::Comment && *reviewed < version)
        .map(|(reviewed, _)| reviewed)
        .max()
}

define_names!(
    /// How much of a revised version a review reads: only what changed since the earlier version and the code that change touches, or the whole version again.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ReviewScope {
        Change = "change",
        Full = "full",
    }
);

define_names!(
    /// A fact fold can check that makes it suggest a full review of a revised version. A reviewer sees more, such as a change to a public API or to permission code, so the review lead may choose the other scope with a reason.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub enum ScopeRule {
        /// The change since the earlier version touches a file that the earlier version did not change.
        NewFiles = "new_files",
        /// The change since the earlier version has more added and removed lines than half of the earlier version's own change.
        LargeDelta = "large_delta",
        /// The author merged a newer main into files of the earlier version's change by hand.
        BaseConflicts = "base_conflicts",
        /// The version is the third or later, so the work has gone back and forth.
        ThirdVersion = "v3_or_later",
    }
);

/// The facts of a revised version's change since the earlier version with a verdict, which [`suggest_scope`] decides from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeFacts {
    pub version: u32,
    /// How many files the change touches that the earlier version did not change.
    pub new_files: usize,
    /// The added and removed lines of the change since the earlier version.
    pub delta_lines: u64,
    /// The added and removed lines of the earlier version's own change against its base.
    pub earlier_lines: u64,
    /// How many files the author merged with a newer main by hand.
    pub base_conflicts: usize,
}

/// The scope fold suggests for a review and the rules that made it suggest a full one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSuggestion {
    pub scope: ReviewScope,
    /// In declaration order; empty for a change review.
    pub rules: Vec<ScopeRule>,
}

/// Any rule that holds suggests a full review; with none, a review of the change is enough.
pub fn suggest_scope(facts: &ScopeFacts) -> ScopeSuggestion {
    let checks = [
        (ScopeRule::NewFiles, facts.new_files > 0),
        (
            ScopeRule::LargeDelta,
            facts.delta_lines.saturating_mul(2) > facts.earlier_lines,
        ),
        (ScopeRule::BaseConflicts, facts.base_conflicts > 0),
        (ScopeRule::ThirdVersion, facts.version >= 3),
    ];
    let rules: Vec<ScopeRule> = checks
        .into_iter()
        .filter(|(_, holds)| *holds)
        .map(|(rule, _)| rule)
        .collect();
    let scope = if rules.is_empty() {
        ReviewScope::Change
    } else {
        ReviewScope::Full
    };
    ScopeSuggestion { scope, rules }
}

/// The review lead's choice of scope for the review of one proposal's current version, made when it assigns the review: it accepts fold's suggestion by passing none, or chooses a scope with its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChoice {
    pub proposal: crate::ids::ProposalId,
    pub scope: ReviewScope,
    pub reason: String,
}

impl ScopeChoice {
    /// A choice needs a reason of visible text, at most [`MAX_REASON_CHARS`] characters, which is stored trimmed.
    pub fn new(
        proposal: crate::ids::ProposalId,
        scope: ReviewScope,
        reason: &str,
    ) -> Result<Self, crate::FoldError> {
        let reason = reason.trim();
        let problem = if reason.is_empty() {
            "a chosen review scope needs a reason".to_owned()
        } else if reason.chars().count() > MAX_REASON_CHARS {
            format!("the scope reason must be at most {MAX_REASON_CHARS} characters")
        } else {
            return Ok(ScopeChoice {
                proposal,
                scope,
                reason: reason.to_owned(),
            });
        };
        Err(
            crate::FoldError::new(crate::codes::INVALID_REQUEST, problem).with_details([
                ("field", serde_json::json!("reason")),
                ("proposal", serde_json::json!(proposal.to_string())),
            ])
            .with_hint("say in one line why this scope fits, such as a public API change or a change of strings only"),
        )
    }
}

/// The scope a review of a revised version was made with: fold's suggestion from the change since `since`, and the scope the review followed, which is the review lead's choice when it made one, with its reason, else the suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewScopeRecord {
    pub since: u32,
    pub suggested: ReviewScope,
    pub rules: Vec<ScopeRule>,
    pub scope: ReviewScope,
    pub reason: Option<String>,
}

impl ReviewScopeRecord {
    pub fn decide(
        since: u32,
        suggestion: ScopeSuggestion,
        choice: Option<(ReviewScope, String)>,
    ) -> Self {
        let (scope, reason) = match choice {
            Some((scope, reason)) => (scope, Some(reason)),
            None => (suggestion.scope, None),
        };
        ReviewScopeRecord {
            since,
            suggested: suggestion.scope,
            rules: suggestion.rules,
            scope,
            reason,
        }
    }
}

fn last_request_changes(reviews: &[ReviewFact]) -> Option<u64> {
    reviews
        .iter()
        .filter(|review| review.verdict == Verdict::RequestChanges)
        .map(|review| review.seq)
        .max()
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_intent_is_trimmed_text_of_at_most_the_limit() {
        assert_eq!(
            proposal_intent("  Adds the report.\r\n\r\nRisk: none.\r", "create_proposal").unwrap(),
            "Adds the report.\n\nRisk: none."
        );
        let longest = "é".repeat(MAX_INTENT_CHARS);
        assert_eq!(
            proposal_intent(&longest, "create_proposal").unwrap(),
            longest
        );
        for (text, reason) in [
            (" \n\t ".to_owned(), "empty"),
            ("x".repeat(MAX_INTENT_CHARS + 1), "too_long"),
        ] {
            let error = proposal_intent(&text, "update_proposal").unwrap_err();
            assert_eq!(error.code(), crate::codes::INVALID_INTENT);
            assert_eq!(
                serde_json::Value::Object(error.details().unwrap().clone()),
                serde_json::json!({"field": "intent", "reason": reason, "max_chars": MAX_INTENT_CHARS})
            );
            assert!(
                error
                    .hint()
                    .unwrap()
                    .starts_with("call update_proposal again")
            );
        }
    }

    #[test]
    fn standing_puts_integration_then_conflict_before_the_verdict() {
        use ProposalStanding::*;
        use Verdict::*;
        assert_eq!(
            ProposalStanding::of(true, true, Some(RequestChanges)),
            Integrated
        );
        assert_eq!(ProposalStanding::of(false, true, Some(Approve)), Conflict);
        assert_eq!(
            ProposalStanding::of(false, false, Some(RequestChanges)),
            ChangesRequested
        );
        assert_eq!(ProposalStanding::of(false, false, Some(Approve)), Approved);
        assert_eq!(ProposalStanding::of(false, false, Some(Reject)), Open);
        assert_eq!(ProposalStanding::of(false, false, None), Open);
        assert_eq!(ChangesRequested.as_str(), "changes_requested");
    }

    use super::*;

    fn fact(verdict: Verdict, seq: u64) -> ReviewFact {
        ReviewFact {
            verdict,
            reviewer: EventActor::parse("review").unwrap(),
            approver: Approver::OtherAgent,
            seq,
        }
    }

    fn name(text: &str) -> HumanName {
        HumanName::parse(text).unwrap()
    }

    fn by_human(verdict: Verdict, human: &str, seq: u64) -> ReviewFact {
        ReviewFact {
            verdict,
            reviewer: EventActor::human(&name(human)),
            approver: Approver::Human(name(human)),
            seq,
        }
    }

    fn by_decider(verdict: Verdict, seq: u64) -> ReviewFact {
        ReviewFact {
            approver: Approver::DecidingAgent,
            ..fact(verdict, seq)
        }
    }

    #[test]
    fn last_verdict_version_is_the_newest_earlier_version_with_a_verdict() {
        let rows = [
            (1, Verdict::RequestChanges),
            (2, Verdict::Comment),
            (2, Verdict::RequestChanges),
            (3, Verdict::Comment),
            (4, Verdict::Approve),
        ];
        assert_eq!(last_verdict_version(rows, 1), None);
        assert_eq!(last_verdict_version(rows, 2), Some(1));
        assert_eq!(last_verdict_version(rows, 3), Some(2));
        assert_eq!(last_verdict_version(rows, 4), Some(2));
        assert_eq!(last_verdict_version(rows, 5), Some(4));
        assert_eq!(last_verdict_version([(1, Verdict::Comment)], 2), None);
        assert_eq!(last_verdict_version([], 2), None);
    }

    fn mark(reviewer: Option<&str>) -> HumanReviewMark {
        HumanReviewMark {
            reviewer: reviewer.map(name),
            reason: "touches auth".to_owned(),
            set_by: EventActor::parse("planner").unwrap(),
        }
    }

    #[test]
    fn verdicts_print_and_parse_as_snake_case() {
        let names: Vec<&str> = Verdict::ALL
            .iter()
            .map(|verdict| verdict.as_str())
            .collect();
        assert_eq!(names, ["comment", "request_changes", "approve", "reject"]);
        for verdict in Verdict::ALL {
            assert_eq!(Verdict::parse(verdict.as_str()), Some(verdict));
            assert_eq!(verdict.to_string(), verdict.as_str());
        }
        assert_eq!(Verdict::parse("Approve"), None);
        assert_eq!(Verdict::parse("request-changes"), None);
        assert_eq!(Verdict::parse(""), None);
    }

    #[test]
    fn sides_print_and_parse() {
        for side in [Side::Old, Side::New] {
            assert_eq!(Side::parse(side.as_str()), Some(side));
        }
        assert_eq!(Side::parse("middle"), None);
    }

    #[test]
    fn latest_verdict_skips_comments_and_takes_the_highest_seq() {
        assert_eq!(latest_verdict(&[]), None);
        assert_eq!(latest_verdict(&[fact(Verdict::Comment, 1)]), None);
        let reviews = [
            fact(Verdict::Approve, 4),
            fact(Verdict::RequestChanges, 2),
            fact(Verdict::Comment, 9),
        ];
        assert_eq!(latest_verdict(&reviews), Some(&reviews[0]));
        let reviews = [
            fact(Verdict::RequestChanges, 7),
            fact(Verdict::Approve, 3),
            fact(Verdict::Comment, 8),
        ];
        assert_eq!(latest_verdict(&reviews), Some(&reviews[0]));
    }

    #[test]
    fn changes_are_requested_until_a_later_approval() {
        assert!(!changes_requested(&[]));
        let requested = fact(Verdict::RequestChanges, 1);
        assert!(changes_requested(std::slice::from_ref(&requested)));
        assert!(changes_requested(&[
            requested.clone(),
            fact(Verdict::Comment, 2)
        ]));
        assert!(!changes_requested(&[
            requested.clone(),
            fact(Verdict::Approve, 3)
        ]));
        assert!(changes_requested(&[
            fact(Verdict::Approve, 1),
            fact(Verdict::RequestChanges, 2)
        ]));
        assert!(!changes_requested(&[requested, fact(Verdict::Reject, 2)]));
    }

    #[test]
    fn no_mark_always_stands() {
        assert!(human_approval_standing(None, &[], None));
        let requested = [fact(Verdict::RequestChanges, 1)];
        assert!(human_approval_standing(None, &requested, None));
    }

    #[test]
    fn a_mark_without_a_reviewer_takes_any_human_approval() {
        let any = mark(None);
        assert!(!human_approval_standing(Some(&any), &[], None));
        let approved = [by_human(Verdict::Approve, "ana", 1)];
        assert!(human_approval_standing(Some(&any), &approved, None));
    }

    #[test]
    fn a_named_reviewer_needs_that_human() {
        let named = mark(Some("tanim"));
        let by_tanim = [by_human(Verdict::Approve, "tanim", 1)];
        assert!(human_approval_standing(Some(&named), &by_tanim, None));
        let by_ana = [by_human(Verdict::Approve, "ana", 1)];
        assert!(!human_approval_standing(Some(&named), &by_ana, None));
    }

    #[test]
    fn a_later_request_changes_withdraws_the_approval() {
        let any = mark(None);
        let withdrawn = [
            by_human(Verdict::Approve, "ana", 1),
            fact(Verdict::RequestChanges, 2),
        ];
        assert!(!human_approval_standing(Some(&any), &withdrawn, None));
        let again = [
            by_human(Verdict::Approve, "ana", 1),
            fact(Verdict::RequestChanges, 2),
            by_human(Verdict::Approve, "ana", 3),
        ];
        assert!(human_approval_standing(Some(&any), &again, None));
        let commented = [
            by_human(Verdict::Approve, "ana", 1),
            fact(Verdict::Comment, 2),
        ];
        assert!(human_approval_standing(Some(&any), &commented, None));
        let human_request = [
            fact(Verdict::RequestChanges, 1),
            by_human(Verdict::Approve, "ana", 2),
            by_human(Verdict::RequestChanges, "tanim", 3),
        ];
        assert!(!human_approval_standing(Some(&any), &human_request, None));
    }

    /// The caller passes only the current version's reviews, so an approval of an older version is not among them and the mark waits.
    #[test]
    fn an_approval_of_an_older_version_is_not_passed_in() {
        let any = mark(None);
        let current_version: [ReviewFact; 0] = [];
        assert!(!human_approval_standing(Some(&any), &current_version, None));
    }

    #[test]
    fn an_agent_approval_never_counts() {
        let any = mark(None);
        let agents = [by_decider(Verdict::Approve, 1), fact(Verdict::Approve, 2)];
        assert!(!human_approval_standing(Some(&any), &agents, None));
        let named = mark(Some("tanim"));
        assert!(!human_approval_standing(Some(&named), &agents, None));
    }

    #[test]
    fn an_integrating_human_counts_when_the_mark_accepts_them() {
        let named = mark(Some("tanim"));
        let tanim = name("tanim");
        let ana = name("ana");
        assert!(human_approval_standing(Some(&named), &[], Some(&tanim)));
        let requested = [fact(Verdict::RequestChanges, 1)];
        assert!(human_approval_standing(
            Some(&named),
            &requested,
            Some(&tanim)
        ));
        assert!(!human_approval_standing(Some(&named), &[], Some(&ana)));
        let approved_by_tanim = [by_human(Verdict::Approve, "tanim", 1)];
        assert!(human_approval_standing(
            Some(&named),
            &approved_by_tanim,
            Some(&ana)
        ));
        assert!(human_approval_standing(Some(&mark(None)), &[], Some(&ana)));
    }

    #[test]
    fn the_mark_accepts_its_reviewer_or_anyone() {
        assert!(mark(None).accepts(&name("ana")));
        assert!(mark(Some("ana")).accepts(&name("ana")));
        assert!(!mark(Some("ana")).accepts(&name("tanim")));
    }

    #[test]
    fn the_latest_agent_approval_after_the_last_request_changes_stands() {
        let by = |reviewer: &str, verdict: Verdict, seq: u64| ReviewFact {
            reviewer: EventActor::parse(reviewer).unwrap(),
            ..fact(verdict, seq)
        };
        let agent = |text: &str| Some(AgentName::parse(text).unwrap());
        assert_eq!(standing_agent_approval(&[]), None);
        let reviews = [
            by("lead", Verdict::Approve, 1),
            by("review/deep", Verdict::Approve, 2),
            by("human:ana", Verdict::Approve, 3),
            by("other", Verdict::Comment, 4),
        ];
        assert_eq!(standing_agent_approval(&reviews), agent("review"));
        let withdrawn = [
            by("lead", Verdict::Approve, 1),
            by("human:ana", Verdict::RequestChanges, 2),
        ];
        assert_eq!(standing_agent_approval(&withdrawn), None);
        let again = [
            by("lead", Verdict::RequestChanges, 1),
            by("lead", Verdict::Approve, 2),
        ];
        assert_eq!(standing_agent_approval(&again), agent("lead"));
    }

    #[test]
    fn every_reviewer_whose_approval_stands_is_listed_once_in_order() {
        let by = |reviewer: &str, verdict: Verdict, seq: u64| ReviewFact {
            reviewer: EventActor::parse(reviewer).unwrap(),
            ..fact(verdict, seq)
        };
        let names = |reviews: &[ReviewFact]| -> Vec<String> {
            standing_approvers(reviews)
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        assert!(standing_approvers(&[]).is_empty());
        let reviews = [
            by("review/deep", Verdict::Approve, 3),
            by("lead", Verdict::Approve, 1),
            by("human:ana", Verdict::Approve, 2),
            by("lead", Verdict::Approve, 4),
            by("other", Verdict::Comment, 5),
        ];
        assert_eq!(names(&reviews), ["lead", "human:ana", "review/deep"]);
        let withdrawn = [
            by("lead", Verdict::Approve, 1),
            by("human:ana", Verdict::RequestChanges, 2),
            by("review", Verdict::Approve, 3),
        ];
        assert_eq!(names(&withdrawn), ["review"]);
    }

    #[test]
    fn a_deciding_approval_stands_until_the_next_request_changes() {
        let author = AgentName::parse("backend").unwrap();
        let by_author = ReviewFact {
            reviewer: EventActor::parse("backend/ci").unwrap(),
            ..by_decider(Verdict::Approve, 5)
        };
        assert!(!deciding_approval(&[], &author));
        assert!(!counted_approval(&[], &author));
        let reviews = [
            by_decider(Verdict::Approve, 1),
            fact(Verdict::Approve, 3),
            by_author.clone(),
        ];
        assert!(deciding_approval(&reviews, &author));
        assert!(counted_approval(&reviews, &author));
        let only_others = [fact(Verdict::Approve, 1), by_author];
        assert!(!deciding_approval(&only_others, &author));
        assert!(!counted_approval(&only_others, &author));
        let withdrawn = [
            by_decider(Verdict::Approve, 1),
            by_human(Verdict::RequestChanges, "ana", 2),
        ];
        assert!(!deciding_approval(&withdrawn, &author));
        assert!(!counted_approval(&withdrawn, &author));
        let again = [
            by_decider(Verdict::Approve, 1),
            fact(Verdict::RequestChanges, 2),
            by_decider(Verdict::Approve, 3),
            fact(Verdict::Approve, 4),
        ];
        assert!(deciding_approval(&again, &author));
        assert!(counted_approval(&again, &author));
    }

    #[test]
    fn a_standing_human_approval_counts_without_an_agent() {
        let author = AgentName::parse("backend").unwrap();
        let human = [
            by_human(Verdict::Approve, "ana", 1),
            fact(Verdict::Comment, 2),
        ];
        assert!(!deciding_approval(&human, &author));
        assert!(counted_approval(&human, &author));
        let withdrawn = [
            by_human(Verdict::Approve, "ana", 1),
            fact(Verdict::RequestChanges, 2),
        ];
        assert!(!counted_approval(&withdrawn, &author));
    }
}

#[cfg(test)]
mod scope_tests {
    use proptest::prelude::*;

    use super::*;

    /// A v2 whose small delta stays in the earlier version's files.
    const SMALL: ScopeFacts = ScopeFacts {
        version: 2,
        new_files: 0,
        delta_lines: 5,
        earlier_lines: 100,
        base_conflicts: 0,
    };

    #[test]
    fn no_rule_suggests_a_change_review() {
        assert_eq!(
            suggest_scope(&SMALL),
            ScopeSuggestion {
                scope: ReviewScope::Change,
                rules: Vec::new()
            }
        );
    }

    #[test]
    fn each_rule_alone_suggests_a_full_review() {
        for (facts, rule) in [
            (
                ScopeFacts {
                    new_files: 1,
                    ..SMALL
                },
                ScopeRule::NewFiles,
            ),
            (
                ScopeFacts {
                    delta_lines: 51,
                    ..SMALL
                },
                ScopeRule::LargeDelta,
            ),
            (
                ScopeFacts {
                    base_conflicts: 2,
                    ..SMALL
                },
                ScopeRule::BaseConflicts,
            ),
            (
                ScopeFacts {
                    version: 3,
                    ..SMALL
                },
                ScopeRule::ThirdVersion,
            ),
        ] {
            let suggestion = suggest_scope(&facts);
            assert_eq!(suggestion.scope, ReviewScope::Full, "{rule}");
            assert_eq!(suggestion.rules, [rule]);
        }
    }

    #[test]
    fn half_the_earlier_change_is_still_a_change_review() {
        let half = ScopeFacts {
            delta_lines: 50,
            ..SMALL
        };
        assert_eq!(suggest_scope(&half).scope, ReviewScope::Change);
        let empty_earlier = ScopeFacts {
            delta_lines: 1,
            earlier_lines: 0,
            ..SMALL
        };
        assert_eq!(suggest_scope(&empty_earlier).rules, [ScopeRule::LargeDelta]);
    }

    #[test]
    fn every_rule_that_holds_is_named_in_order() {
        let all = ScopeFacts {
            version: 4,
            new_files: 3,
            delta_lines: 400,
            earlier_lines: 10,
            base_conflicts: 1,
        };
        assert_eq!(suggest_scope(&all).rules, ScopeRule::ALL);
        assert_eq!(
            ScopeRule::ALL.map(ScopeRule::as_str),
            ["new_files", "large_delta", "base_conflicts", "v3_or_later"]
        );
    }

    proptest! {
        #[test]
        fn a_full_review_names_its_rules_and_a_larger_delta_never_lowers_the_scope(
            version in 2u32..6,
            new_files in 0usize..3,
            delta_lines in 0u64..400,
            more in 0u64..400,
            earlier_lines in 0u64..400,
            base_conflicts in 0usize..3,
        ) {
            let facts = ScopeFacts { version, new_files, delta_lines, earlier_lines, base_conflicts };
            let suggestion = suggest_scope(&facts);
            prop_assert_eq!(suggestion.scope == ReviewScope::Full, !suggestion.rules.is_empty());
            prop_assert_eq!(
                suggestion.rules.contains(&ScopeRule::LargeDelta),
                2 * delta_lines > earlier_lines
            );
            let larger = suggest_scope(&ScopeFacts { delta_lines: delta_lines + more, ..facts });
            prop_assert!(suggestion.scope == ReviewScope::Change || larger.scope == ReviewScope::Full);
        }
    }

    #[test]
    fn a_choice_needs_a_reason_and_overrides_the_suggestion() {
        let proposal = crate::ids::ProposalId::from_number(3).unwrap();
        for blank in ["", "   "] {
            let error = ScopeChoice::new(proposal, ReviewScope::Change, blank).unwrap_err();
            assert_eq!(error.code(), crate::codes::INVALID_REQUEST);
        }
        let long = "x".repeat(MAX_REASON_CHARS + 1);
        assert!(ScopeChoice::new(proposal, ReviewScope::Full, &long).is_err());
        let choice =
            ScopeChoice::new(proposal, ReviewScope::Change, "  only strings changed ").unwrap();
        assert_eq!(choice.reason, "only strings changed");
        let full = suggest_scope(&ScopeFacts {
            version: 3,
            ..SMALL
        });
        let chosen =
            ReviewScopeRecord::decide(2, full.clone(), Some((choice.scope, choice.reason)));
        assert_eq!(
            chosen,
            ReviewScopeRecord {
                since: 2,
                suggested: ReviewScope::Full,
                rules: vec![ScopeRule::ThirdVersion],
                scope: ReviewScope::Change,
                reason: Some("only strings changed".to_owned()),
            }
        );
        let followed = ReviewScopeRecord::decide(2, full, None);
        assert_eq!((followed.scope, followed.reason), (ReviewScope::Full, None));
    }
}

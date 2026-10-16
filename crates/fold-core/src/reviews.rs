use std::fmt;

use crate::actor::AgentActor;
use crate::agents::{AgentName, HumanName};
use crate::events::EventActor;

/// What a review says about one proposal version. `Comment` is not a verdict on the work: it leaves the turn where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    Comment,
    RequestChanges,
    Approve,
    Reject,
}

impl Verdict {
    pub const ALL: [Verdict; 4] = [
        Verdict::Comment,
        Verdict::RequestChanges,
        Verdict::Approve,
        Verdict::Reject,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Comment => "comment",
            Verdict::RequestChanges => "request_changes",
            Verdict::Approve => "approve",
            Verdict::Reject => "reject",
        }
    }

    pub fn parse(text: &str) -> Option<Verdict> {
        Verdict::ALL
            .into_iter()
            .find(|verdict| verdict.as_str() == text)
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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

/// The weight of a review's author, read from the agent's tags when the review is read rather than when it was written, so removing `integrator` from an agent withdraws the weight of its approvals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approver {
    IntegratorAgent,
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

/// Whether the human review a mark asks for is met. Pass the reviews of the current version only, because approvals belong to the version they reviewed. A human who integrates approves in the same action, so `integrating_human` counts as an approval when the mark accepts that human. A recorded human approval stands until a later `request_changes` on the same version; an agent's approval never counts, whatever its tags.
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

/// The `seq` of the agent approval that lets fold integrate the version on its own: the first approval by an integrator agent other than `author`'s after the last `request_changes`, or `None` when no such approval stands. It is the first one because only a `request_changes` withdraws a human approval: a second agent approval after the human's leaves it standing. Pass the reviews of one version only.
pub fn integrator_approval(reviews: &[ReviewFact], author: &AgentName) -> Option<u64> {
    let withdrawn = last_request_changes(reviews);
    reviews
        .iter()
        .filter(|review| {
            review.verdict == Verdict::Approve
                && review.approver == Approver::IntegratorAgent
                && review.reviewer.agent() != Some(author.as_str())
                && withdrawn.is_none_or(|seq| review.seq > seq)
        })
        .map(|review| review.seq)
        .min()
}

/// Whether a human the mark accepts approved the version after the review with `seq`, the agent approval the human's approval must follow. Pass the reviews of one version only.
pub fn human_approved_after(mark: &HumanReviewMark, reviews: &[ReviewFact], seq: u64) -> bool {
    reviews.iter().any(|review| {
        review.verdict == Verdict::Approve
            && review.seq > seq
            && matches!(&review.approver, Approver::Human(human) if mark.accepts(human))
    })
}

/// The newest version before `version` that has a verdict, a comment not counted, so a reviewer of `version` can read only what changed since it. Anyone's verdict counts, an agent's or a human's: the next version is often reviewed by another agent than the earlier one, because each batch reviewer fold starts is an agent of its own.
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

    fn by_integrator(verdict: Verdict, seq: u64) -> ReviewFact {
        ReviewFact {
            approver: Approver::IntegratorAgent,
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
        let agents = [
            by_integrator(Verdict::Approve, 1),
            fact(Verdict::Approve, 2),
        ];
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
    fn the_integrator_approval_is_the_first_one_after_the_last_request_changes() {
        let author = AgentName::parse("backend").unwrap();
        let by_author = ReviewFact {
            reviewer: EventActor::parse("backend/ci").unwrap(),
            ..by_integrator(Verdict::Approve, 5)
        };
        assert_eq!(integrator_approval(&[], &author), None);
        let reviews = [
            by_integrator(Verdict::Approve, 1),
            fact(Verdict::Approve, 3),
            by_human(Verdict::Approve, "ana", 4),
            by_author,
        ];
        assert_eq!(integrator_approval(&reviews, &author), Some(1));
        let withdrawn = [
            by_integrator(Verdict::Approve, 1),
            by_human(Verdict::RequestChanges, "ana", 2),
        ];
        assert_eq!(integrator_approval(&withdrawn, &author), None);
        let again = [
            by_integrator(Verdict::Approve, 1),
            fact(Verdict::RequestChanges, 2),
            by_integrator(Verdict::Approve, 3),
            by_integrator(Verdict::Approve, 4),
        ];
        assert_eq!(integrator_approval(&again, &author), Some(3));
    }

    #[test]
    fn only_a_human_approval_after_the_agent_approval_follows_it() {
        let named = mark(Some("tanim"));
        let reviews = [
            by_human(Verdict::Approve, "tanim", 1),
            by_integrator(Verdict::Approve, 2),
            by_human(Verdict::Approve, "ana", 3),
            by_human(Verdict::Comment, "tanim", 4),
        ];
        assert!(!human_approved_after(&named, &reviews, 2));
        assert!(human_approved_after(&mark(None), &reviews, 2));
        assert!(human_approved_after(&named, &reviews, 0));
    }
}

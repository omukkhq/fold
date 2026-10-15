use std::fmt;

use crate::actor::AgentActor;
use crate::agents::HumanName;
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

/// One review of a proposal version as the turn and the integration gates read it. `seq` orders the reviews of a version: a higher `seq` is a later review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFact {
    pub verdict: Verdict,
    pub reviewer: EventActor,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(verdict: Verdict, seq: u64) -> ReviewFact {
        ReviewFact {
            verdict,
            reviewer: EventActor::parse("review").unwrap(),
            seq,
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
}

use crate::actor::Actor;
use crate::agents::{AgentName, HumanName};
use crate::error::codes;
use crate::events::EventActor;
use crate::reviews::{HumanReviewMark, ReviewFact, deciding_approval};

use super::PolicyDenied;

/// What setting the mark does to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkChange {
    /// The columns change and `task.human_review_set` is appended.
    Write,
    /// The mark already says the same thing, so nothing is written.
    Unchanged,
}

/// Any agent or human may ask for a human's eyes on a task, but once the mark is on only a human changes what it says: an agent repeating the same reviewer and reason changes nothing, and an agent asking for something else is refused. The system sets the mark only through [`checks_file_mark`].
pub fn can_set_human_review(
    actor: &Actor,
    current: Option<&HumanReviewMark>,
    reviewer: Option<&HumanName>,
    reason: &str,
) -> Result<MarkChange, PolicyDenied> {
    if matches!(actor, Actor::System) {
        return Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only an agent or a human can set the human review mark",
            hint: "ask an agent or a human to call require_human_review",
        });
    }
    let Some(current) = current else {
        return Ok(MarkChange::Write);
    };
    if current.reviewer.as_ref() == reviewer && current.reason == reason {
        return Ok(MarkChange::Unchanged);
    }
    if actor.is_human() {
        return Ok(MarkChange::Write);
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REVIEW_ALREADY_SET,
        message: "the task already has a human review mark that says something else",
        hint: "call get_task to see the mark; only a human changes or clears it",
    })
}

/// The reason of the mark fold sets on a task whose proposal changes the checks file.
pub const CHECKS_FILE_REASON: &str =
    "the proposal changes fold-checks.toml, so a human approves the change of the checks";

/// A proposal version that adds, changes or removes the checks file needs a human's approval, so no agent removes or weakens a check without the user. Returns the mark fold sets on the task in the transaction that makes the version: any human may approve, and `set_by` is fold. A task that already has a mark keeps it, because every mark already holds integration for a human, and a version that leaves the file as it is needs none.
pub fn checks_file_mark(
    current: Option<&HumanReviewMark>,
    checks_changed: bool,
) -> Option<HumanReviewMark> {
    if !checks_changed || current.is_some() {
        return None;
    }
    Some(HumanReviewMark {
        reviewer: None,
        reason: CHECKS_FILE_REASON.to_owned(),
        set_by: EventActor::fold(),
    })
}

/// The mark holds integration until a human has looked, so only a human takes it off.
pub fn can_clear_human_review(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: "only a human can clear the human review mark",
        hint: "call send_message to ask a human to clear the mark",
    })
}

/// A human changes a proposal only in a human review: on a task with the human review mark, the human that the mark names, or any human when it names none, may edit a version or send a human edit for review again. Everywhere else the change goes back to the author with `request_changes`, and an agent or the system never edits. It returns the admitted human, who becomes the new version's author.
///
/// The version a human edit makes keeps the agent approvals of the version it was made from. This is the one exception to the rule that a new version starts without approvals: the human looked at the change while making it. No human approval is kept, so a human approves the edited version again and an edit never integrates on a human's earlier approval. Sending the edit for review again withdraws the kept approvals and gives the turn to the reviewers.
pub fn can_edit_as_human<'a>(
    actor: &'a Actor,
    mark: Option<&HumanReviewMark>,
) -> Result<&'a HumanName, PolicyDenied> {
    match (actor, mark) {
        (Actor::Human { name }, Some(mark)) if mark.accepts(name) => Ok(name),
        (Actor::Human { .. }, Some(_)) => Err(PolicyDenied {
            code: codes::HUMAN_EDIT_DENIED,
            message: "the human review mark names another human",
            hint: "ask the human that the mark names, or comment and request changes",
        }),
        (Actor::Human { .. }, None) => Err(PolicyDenied {
            code: codes::HUMAN_EDIT_DENIED,
            message: "a human edits a proposal only on a task with the human review mark",
            hint: "comment and request changes, so the author makes the change",
        }),
        (Actor::Agent(_) | Actor::System, _) => Err(PolicyDenied {
            code: codes::HUMAN_EDIT_DENIED,
            message: "only a human edits a proposal in a human review",
            hint: "submit request_changes, so the author's worker adds a version with update_proposal",
        }),
    }
}

/// A human reviews a marked task after an agent: the human approves only once an agent that decides reviews, other than `author`'s, has approved the version. `reviews` are the reviews of the version the human approves. A task without the mark, and a human's comment or request for changes, need no agent approval.
pub fn can_human_approve(
    mark: Option<&HumanReviewMark>,
    reviews: &[ReviewFact],
    author: &AgentName,
) -> Result<(), PolicyDenied> {
    if mark.is_none() || deciding_approval(reviews, author) {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::AGENT_APPROVAL_REQUIRED,
        message: "a human approves a task with the human review mark only after a reviewer agent approved the version",
        hint: "ask the review lead to review and approve the current version first; comment or request changes meanwhile",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::{Tag, WorkerName};

    fn agent(worker: &str, tags: &[&str]) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
        })
    }

    fn name(text: &str) -> HumanName {
        HumanName::parse(text).unwrap()
    }

    fn mark(reviewer: Option<&str>, reason: &str) -> HumanReviewMark {
        HumanReviewMark {
            reviewer: reviewer.map(name),
            reason: reason.to_owned(),
            set_by: EventActor::parse("planner").unwrap(),
        }
    }

    fn fact(
        verdict: crate::reviews::Verdict,
        approver: crate::reviews::Approver,
        seq: u64,
    ) -> ReviewFact {
        ReviewFact {
            verdict,
            reviewer: EventActor::parse("review").unwrap(),
            approver,
            seq,
        }
    }

    #[test]
    fn a_human_approves_a_marked_task_only_after_a_deciding_agent() {
        use crate::reviews::{Approver, Verdict};
        let author = AgentName::parse("backend").unwrap();
        let marked = mark(None, "billing");
        assert!(can_human_approve(None, &[], &author).is_ok());
        let denied = can_human_approve(Some(&marked), &[], &author).unwrap_err();
        assert_eq!(denied.code(), codes::AGENT_APPROVAL_REQUIRED);
        assert!(denied.hint().contains("review lead"));
        let other = [fact(Verdict::Approve, Approver::OtherAgent, 1)];
        assert!(can_human_approve(Some(&marked), &other, &author).is_err());
        let decided = [fact(Verdict::Approve, Approver::DecidingAgent, 1)];
        assert!(can_human_approve(Some(&marked), &decided, &author).is_ok());
        let withdrawn = [
            fact(Verdict::Approve, Approver::DecidingAgent, 1),
            fact(Verdict::RequestChanges, Approver::DecidingAgent, 2),
        ];
        assert!(can_human_approve(Some(&marked), &withdrawn, &author).is_err());
    }

    fn outcome(result: Result<MarkChange, PolicyDenied>) -> Result<MarkChange, &'static str> {
        result.map_err(|denied| denied.code())
    }

    #[test]
    fn agents_and_humans_set_and_the_system_does_not() {
        let tanim = name("tanim");
        let setters = [
            agent("frontend", &[]),
            agent("review", &["review"]),
            Actor::human(name("ana")),
        ];
        for actor in &setters {
            assert_eq!(
                outcome(can_set_human_review(actor, None, Some(&tanim), "auth")),
                Ok(MarkChange::Write),
                "{actor:?}"
            );
            assert_eq!(
                outcome(can_set_human_review(actor, None, None, "auth")),
                Ok(MarkChange::Write)
            );
        }
        let denied = can_set_human_review(&Actor::System, None, None, "auth").unwrap_err();
        assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
    }

    #[test]
    fn only_a_human_changes_an_existing_mark() {
        let current = mark(Some("tanim"), "auth");
        let tanim = name("tanim");
        let ana = name("ana");
        let frontend = agent("frontend", &[]);
        let review_agent = agent("review", &["review"]);
        let human = Actor::human(name("ana"));
        for actor in [&frontend, &review_agent, &human] {
            assert_eq!(
                outcome(can_set_human_review(
                    actor,
                    Some(&current),
                    Some(&tanim),
                    "auth"
                )),
                Ok(MarkChange::Unchanged),
                "{actor:?} repeats the mark"
            );
        }
        for (reviewer, reason) in [
            (Some(&ana), "auth"),
            (Some(&tanim), "billing"),
            (None, "auth"),
        ] {
            for actor in [&frontend, &review_agent] {
                assert_eq!(
                    outcome(can_set_human_review(
                        actor,
                        Some(&current),
                        reviewer,
                        reason
                    )),
                    Err(codes::HUMAN_REVIEW_ALREADY_SET),
                    "{actor:?} changes the mark"
                );
            }
            assert_eq!(
                outcome(can_set_human_review(
                    &human,
                    Some(&current),
                    reviewer,
                    reason
                )),
                Ok(MarkChange::Write)
            );
        }
        let denied = can_set_human_review(&frontend, Some(&current), None, "other").unwrap_err();
        assert!(denied.hint().contains("human"));
    }

    #[test]
    fn only_the_human_of_a_human_review_edits() {
        let tanim = Actor::human(name("tanim"));
        let ana = Actor::human(name("ana"));
        let named = mark(Some("tanim"), "auth");
        let anyone = mark(None, "auth");
        assert_eq!(
            can_edit_as_human(&tanim, Some(&named)).unwrap().as_str(),
            "tanim"
        );
        assert_eq!(
            can_edit_as_human(&ana, Some(&anyone)).unwrap().as_str(),
            "ana"
        );
        let other = can_edit_as_human(&ana, Some(&named)).unwrap_err();
        assert_eq!(other.code(), codes::HUMAN_EDIT_DENIED);
        assert!(other.to_string().contains("another human"));
        let unmarked = can_edit_as_human(&tanim, None).unwrap_err();
        assert_eq!(unmarked.code(), codes::HUMAN_EDIT_DENIED);
        assert!(unmarked.hint().contains("request changes"));
        for actor in [
            agent("frontend", &[]),
            agent("main", &["main"]),
            Actor::System,
        ] {
            for mark in [None, Some(&anyone), Some(&named)] {
                let denied = can_edit_as_human(&actor, mark).unwrap_err();
                assert_eq!(denied.code(), codes::HUMAN_EDIT_DENIED, "{actor:?}");
                assert!(denied.hint().contains("request_changes"));
            }
        }
    }

    #[test]
    fn only_a_human_clears() {
        assert!(can_clear_human_review(&Actor::human(name("ana"))).is_ok());
        for actor in [
            agent("frontend", &[]),
            agent("review", &["review"]),
            Actor::System,
        ] {
            let denied = can_clear_human_review(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED, "{actor:?}");
        }
    }

    #[test]
    fn a_changed_checks_file_marks_an_unmarked_task_for_any_human() {
        let mark = checks_file_mark(None, true).unwrap();
        assert_eq!(mark.reviewer, None);
        assert_eq!(mark.reason, CHECKS_FILE_REASON);
        assert_eq!(mark.set_by, EventActor::fold());
        assert_eq!(checks_file_mark(None, false), None);
        let held = HumanReviewMark {
            reviewer: Some(HumanName::parse("tanim").unwrap()),
            reason: "look".to_owned(),
            set_by: EventActor::fold(),
        };
        assert_eq!(checks_file_mark(Some(&held), true), None);
    }
}

use crate::actor::Actor;
use crate::agents::HumanName;
use crate::error::codes;
use crate::reviews::HumanReviewMark;

use super::PolicyDenied;

/// What setting the mark does to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkChange {
    /// The columns change and `task.human_review_set` is appended.
    Write,
    /// The mark already says the same thing, so nothing is written.
    Unchanged,
}

/// Any agent or human may ask for a human's eyes on a task, but once the mark is on only a human changes what it says: an agent repeating the same reviewer and reason changes nothing, and an agent asking for something else is refused. The system never sets the mark.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::{Tag, WorkerName};
    use crate::events::EventActor;

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

    fn outcome(result: Result<MarkChange, PolicyDenied>) -> Result<MarkChange, &'static str> {
        result.map_err(|denied| denied.code())
    }

    #[test]
    fn agents_and_humans_set_and_the_system_does_not() {
        let tanim = name("tanim");
        let setters = [
            agent("frontend", &[]),
            agent("review", &["integrator"]),
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
        let integrator = agent("review", &["integrator"]);
        let human = Actor::human(name("ana"));
        for actor in [&frontend, &integrator, &human] {
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
            for actor in [&frontend, &integrator] {
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
    fn only_a_human_clears() {
        assert!(can_clear_human_review(&Actor::human(name("ana"))).is_ok());
        for actor in [
            agent("frontend", &[]),
            agent("review", &["integrator"]),
            Actor::System,
        ] {
            let denied = can_clear_human_review(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED, "{actor:?}");
        }
    }
}

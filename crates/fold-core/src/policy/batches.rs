use crate::actor::{Actor, AgentActor};
use crate::agents::WorkerName;
use crate::error::codes;

use super::{AgentStanding, PolicyDenied, Starter, can_review};

/// A review batch holds the review claims of its members for the worker that created it, so only that worker's agent changes it, through any of its workers, as with a single review claim. `holder` is the batch's worker.
pub fn can_change_batch<'a>(
    actor: &'a Actor,
    holder: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if holder.belongs_to(&agent.agent) => Ok(agent),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_CLAIM_HOLDER,
            message: "only the agent that holds the review batch can change, check out or submit it",
            hint: "call get_review_batch to see the holder, or review each proposal with submit_review",
        }),
    }
}

/// Checking out and submitting a batch review it, so besides [`can_change_batch`] the review lead is refused, which never reviews ([`can_review`]). `standing` is the acting agent's.
pub fn can_review_batch<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    holder: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    can_review(standing)?;
    can_change_batch(actor, holder)
}

/// The review lead groups related proposals, so only it makes, changes and starts a review plan; it starts the plan's reviewer as it starts every reviewer. `starter` is the acting agent's standing as a starter of workers.
pub fn can_plan_reviews(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::ReviewLead => Ok(()),
        _ => Err(PolicyDenied {
            code: codes::REVIEW_PLAN_DENIED,
            message: "only the review lead plans which related proposals one reviewer reviews together",
            hint: "tell the review lead with send_message which proposals belong together",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::HumanName;
    use crate::agents::roles::Role;

    fn agent(worker: &str) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: Vec::new(),
        })
    }

    const REVIEWER: AgentStanding = AgentStanding {
        holds_main: false,
        role: Some(Role::Reviewer),
    };

    #[test]
    fn only_the_holder_agent_acts_on_a_batch() {
        let holder = WorkerName::parse("reviewer-1/batch").unwrap();
        assert!(can_change_batch(&agent("reviewer-1/batch"), &holder).is_ok());
        assert!(can_review_batch(&agent("reviewer-1"), REVIEWER, &holder).is_ok());
        for actor in [
            agent("frontend"),
            Actor::human(HumanName::parse("tanim").unwrap()),
            Actor::System,
        ] {
            let denied = can_change_batch(&actor, &holder).unwrap_err();
            assert_eq!(denied.code(), codes::NOT_CLAIM_HOLDER);
            let denied = can_review_batch(&actor, AgentStanding::NO_RIGHTS, &holder).unwrap_err();
            assert_eq!(denied.code(), codes::NOT_CLAIM_HOLDER);
        }
    }

    #[test]
    fn only_the_review_lead_plans_reviews() {
        assert!(can_plan_reviews(&Starter::ReviewLead).is_ok());
        for starter in [
            Starter::MainAgent,
            Starter::Human,
            Starter::System,
            Starter::Unsupervised,
            Starter::ShortLived,
            Starter::Subagent,
        ] {
            let denied = can_plan_reviews(&starter).unwrap_err();
            assert_eq!(denied.code(), codes::REVIEW_PLAN_DENIED);
            assert!(denied.hint().contains("review lead"));
        }
    }

    #[test]
    fn the_review_lead_changes_its_own_batch_but_never_reviews_one() {
        let holder = WorkerName::parse("review").unwrap();
        let lead = AgentStanding {
            holds_main: false,
            role: Some(Role::ReviewLead),
        };
        assert!(can_change_batch(&agent("review"), &holder).is_ok());
        let denied = can_review_batch(&agent("review"), lead, &holder).unwrap_err();
        assert_eq!(denied.code(), codes::REVIEW_LEAD_DELEGATES);
        assert!(denied.hint().contains("start_worker"));
    }
}

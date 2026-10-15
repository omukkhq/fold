use crate::actor::{Actor, AgentActor};
use crate::agents::WorkerName;
use crate::error::codes;

use super::PolicyDenied;

/// A review batch holds the review claims of its members for the worker that created it, so only that worker's agent changes, checks out or submits it, through any of its workers, as with a single review claim. `holder` is the batch's worker.
pub fn can_review_batch<'a>(
    actor: &'a Actor,
    holder: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if agent.agent == holder.agent() => Ok(agent),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_CLAIM_HOLDER,
            message: "only the agent that holds the review batch can change, check out or submit it",
            hint: "call get_review_batch to see the holder, or review each proposal with submit_review",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::HumanName;

    fn agent(worker: &str) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: Vec::new(),
        })
    }

    #[test]
    fn only_the_holder_agent_acts_on_a_batch() {
        let holder = WorkerName::parse("review/batch").unwrap();
        assert!(can_review_batch(&agent("review/batch"), &holder).is_ok());
        assert!(can_review_batch(&agent("review"), &holder).is_ok());
        for actor in [
            agent("frontend"),
            Actor::human(HumanName::parse("tanim").unwrap()),
            Actor::System,
        ] {
            let denied = can_review_batch(&actor, &holder).unwrap_err();
            assert_eq!(denied.code(), codes::NOT_CLAIM_HOLDER);
        }
    }
}

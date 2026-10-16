use crate::actor::Actor;
use crate::error::codes;

use super::PolicyDenied;

/// Bringing the remote branch into main changes main outside any proposal, so only a named human runs `fld pull`.
pub fn can_pull(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human brings the remote branch into main",
            hint: "ask a human to run fld pull",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::{AgentName, HumanName, WorkerName};

    #[test]
    fn only_a_human_pulls() {
        assert!(can_pull(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("frontend").unwrap(),
            worker: WorkerName::parse("frontend").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_pull(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert_eq!(denied.hint(), "ask a human to run fld pull");
        }
    }
}

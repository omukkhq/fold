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

/// A human change in the project folder reaches main with no task and no review: the human who made it records it, so only a named human runs `fld record`. A worker that writes the project folder is refused, and so is fold itself, which never records a change no human asked for.
pub fn can_record(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human records the changes of the project folder",
            hint: "ask a human to run fld record in the project folder",
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

    #[test]
    fn only_a_human_records_the_project_folder() {
        assert!(can_record(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("main").unwrap(),
            worker: WorkerName::parse("main").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_record(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert!(denied.hint().contains("fld record"));
        }
    }
}

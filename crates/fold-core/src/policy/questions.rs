use crate::actor::Actor;
use crate::error::codes;

use super::PolicyDenied;

/// Only a named human answers a worker's permission question: the question exists because Claude Code's auto mode would not decide the call, so no agent may decide it either.
pub fn can_answer_worker_permission(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: "only a human answers a worker's permission question",
        hint: "a human answers it in fold's web interface",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::{HumanName, WorkerName};

    #[test]
    fn only_a_human_answers() {
        let human = Actor::human(HumanName::parse("ana").unwrap());
        assert!(can_answer_worker_permission(&human).is_ok());
        let worker = WorkerName::parse("web-lead").unwrap();
        let agent = Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: Vec::new(),
        });
        assert_eq!(
            can_answer_worker_permission(&agent).unwrap_err().code,
            codes::HUMAN_REQUIRED
        );
        assert!(can_answer_worker_permission(&Actor::System).is_err());
    }
}

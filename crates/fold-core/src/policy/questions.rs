use crate::actor::Actor;
use crate::agents::WorkerName;
use crate::error::codes;

use super::{AgentStanding, PolicyDenied};

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

/// An agent's ask to a human stops waiting without a reply in its thread when the user answered it elsewhere, such as in the main agent's session. The asking agent knows that from the answer it got, and the main agent from the user's own words, so only they record it; any named human may mark an ask answered. `standing` is the acting agent's and `asker` the worker that sent the ask.
pub fn can_close_ask(
    actor: &Actor,
    standing: AgentStanding,
    asker: &WorkerName,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } => Ok(()),
        Actor::Agent(agent) if agent.agent == asker.agent() || standing.is_main_agent() => Ok(()),
        Actor::Agent(_) | Actor::System => Err(PolicyDenied {
            code: codes::ASK_CLOSE_DENIED,
            message: "only the asking agent, the main agent or a human closes an ask",
            hint: "send_message to the asking agent or the main agent when the user answered elsewhere",
        }),
    }
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

    #[test]
    fn the_asking_agent_the_main_agent_or_a_human_closes_an_ask() {
        let asker = WorkerName::parse("web-lead").unwrap();
        let agent = |name: &str| {
            let worker = WorkerName::parse(name).unwrap();
            Actor::Agent(AgentActor {
                agent: worker.agent(),
                worker,
                tags: Vec::new(),
            })
        };
        let main = AgentStanding {
            holds_main: true,
            role: None,
        };
        let none = AgentStanding::NO_RIGHTS;
        assert!(can_close_ask(&agent("web-lead/ui"), none, &asker).is_ok());
        assert!(can_close_ask(&agent("main"), main, &asker).is_ok());
        let human = Actor::human(HumanName::parse("ana").unwrap());
        assert!(can_close_ask(&human, none, &asker).is_ok());
        assert_eq!(
            can_close_ask(&agent("backend"), none, &asker)
                .unwrap_err()
                .code,
            codes::ASK_CLOSE_DENIED
        );
        assert!(can_close_ask(&Actor::System, none, &asker).is_err());
    }
}

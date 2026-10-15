use crate::actor::{Actor, AgentActor};
use crate::agents::WorkerName;
use crate::error::codes;
use crate::requests::Requester;

use super::PolicyDenied;

/// Anyone who acts for themselves creates a request: any worker of an agent, or a named human. The system never asks for work. It returns the requester to record.
pub fn can_create_request(actor: &Actor) -> Result<Requester, PolicyDenied> {
    Requester::of(actor).ok_or(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: "only an agent or a human can create a request",
        hint: "ask an agent or a human to call create_request",
    })
}

/// Turning a request into cards is an orchestrator's job, so only the main worker of an agent claims one: a subagent worker works inside its parent's plan, and a human answers the drafter instead. It returns the admitted agent, whose main worker becomes the drafter.
pub fn can_claim_request(actor: &Actor) -> Result<&AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if agent.worker.is_main() => Ok(agent),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::ORCHESTRATOR_REQUIRED,
            message: "only an agent's main worker can claim a request",
            hint: "ask an orchestrator to call claim_request",
        }),
    }
}

/// The requester decides what the drafts become, so only they approve, ask for changes, edit or close: the same named human, or any worker of the requesting agent.
pub fn is_requester(actor: &Actor, requester: &Requester) -> Result<(), PolicyDenied> {
    if requester.matches(actor) {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::REQUESTER_REQUIRED,
        message: "only the requester can decide on a request",
        hint: "call get_request to see the requester, or send_message to ask them",
    })
}

/// The drafter's agent writes and gives up the draft through any of its workers, so an orchestrator may hand the writing to a subagent. `drafter` is the request's drafter worker; a request nobody drafts is refused too.
pub fn is_drafter<'a>(
    actor: &'a Actor,
    drafter: Option<&WorkerName>,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if drafter.is_some_and(|drafter| agent.agent == drafter.agent()) => {
            Ok(agent)
        }
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::DRAFTER_REQUIRED,
            message: "only the drafter's agent can draft or release a request",
            hint: "call claim_request first, or get_request to see the drafter",
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

    fn human(name: &str) -> Actor {
        Actor::human(HumanName::parse(name).unwrap())
    }

    fn code<T>(result: Result<T, PolicyDenied>) -> Option<&'static str> {
        result.err().map(|denied| denied.code())
    }

    #[test]
    fn agents_and_humans_create_and_the_system_does_not() {
        assert_eq!(
            can_create_request(&agent("backend/api")).unwrap().label(),
            "backend"
        );
        assert_eq!(
            can_create_request(&human("tanim")).unwrap().label(),
            "human:tanim"
        );
        assert_eq!(
            code(can_create_request(&Actor::System)),
            Some(codes::HUMAN_REQUIRED)
        );
    }

    #[test]
    fn only_a_main_worker_claims_a_request() {
        let main = agent("frontend");
        assert_eq!(
            can_claim_request(&main).unwrap().worker.as_str(),
            "frontend"
        );
        for actor in [agent("frontend/ui"), human("tanim"), Actor::System] {
            let denied = can_claim_request(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
            assert!(denied.hint().contains("claim_request"));
        }
    }

    #[test]
    fn only_the_requester_decides() {
        let backend = Requester::parse("backend").unwrap();
        assert_eq!(code(is_requester(&agent("backend"), &backend)), None);
        assert_eq!(code(is_requester(&agent("backend/sub"), &backend)), None);
        for actor in [agent("frontend"), human("backend"), Actor::System] {
            assert_eq!(
                code(is_requester(&actor, &backend)),
                Some(codes::REQUESTER_REQUIRED)
            );
        }
        let tanim = Requester::parse("human:tanim").unwrap();
        assert_eq!(code(is_requester(&human("tanim"), &tanim)), None);
        assert_eq!(
            code(is_requester(&human("ana"), &tanim)),
            Some(codes::REQUESTER_REQUIRED)
        );
    }

    #[test]
    fn the_drafters_agent_drafts() {
        let drafter = WorkerName::parse("review").unwrap();
        assert_eq!(code(is_drafter(&agent("review"), Some(&drafter))), None);
        assert_eq!(
            code(is_drafter(&agent("review/writer"), Some(&drafter))),
            None
        );
        for actor in [agent("frontend"), human("review"), Actor::System] {
            assert_eq!(
                code(is_drafter(&actor, Some(&drafter))),
                Some(codes::DRAFTER_REQUIRED)
            );
        }
        assert_eq!(
            code(is_drafter(&agent("review"), None)),
            Some(codes::DRAFTER_REQUIRED)
        );
    }
}

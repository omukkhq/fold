use crate::actor::{Actor, AgentActor};
use crate::agents::roles::Role;
use crate::agents::{Tag, WorkerName};
use crate::error::codes;
use crate::tasks::requests::Requester;

use super::PolicyDenied;

/// Anyone who acts for themselves creates a request: any worker of an agent, or a named human. The system never asks for work. It returns the requester to record.
pub fn can_create_request(actor: &Actor) -> Result<Requester, PolicyDenied> {
    Requester::of(actor).ok_or(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: "only an agent or a human can create a request",
        hint: "ask an agent or a human to call create_request",
    })
}

/// Turning a request into cards is an orchestrator's job, so the main agent claims one, or an area lead that judges the request relevant to its area; fold does not check the request against the area. Only an agent's main worker claims: a subagent worker works inside its parent's plan, and a human answers the drafter instead. `role` is the role of the agent's worker process, `None` when fold does not supervise it. It returns the admitted agent, whose main worker becomes the drafter.
pub fn can_claim_request(actor: &Actor, role: Option<Role>) -> Result<&AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent)
            if agent.worker.is_main()
                && (agent.tags.iter().any(Tag::is_main) || role == Some(Role::AreaLead)) =>
        {
            Ok(agent)
        }
        Actor::Agent(agent) if agent.worker.is_main() => Err(PolicyDenied {
            code: codes::ORCHESTRATOR_REQUIRED,
            message: "only the main agent or an area lead claims a request",
            hint: "ask the main agent or an area lead to call claim_request",
        }),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::ORCHESTRATOR_REQUIRED,
            message: "only an agent's main worker can claim a request",
            hint: "ask an orchestrator to call claim_request",
        }),
    }
}

/// The requester decides what the drafts become, so only they approve, ask for changes, edit or close: the same named human, or any worker of the requesting agent. `deputy` (see [`crate::tasks::requests::deputy`]) decides for a short-lived requester, which fold stops once its work is done.
pub fn is_requester(
    actor: &Actor,
    requester: &Requester,
    deputy: Option<&Requester>,
) -> Result<(), PolicyDenied> {
    if requester.matches(actor) || deputy.is_some_and(|deputy| deputy.matches(actor)) {
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
        Actor::Agent(agent) if drafter.is_some_and(|drafter| drafter.belongs_to(&agent.agent)) => {
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
    use crate::agents::{HumanName, MAIN_TAG};

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

    fn tagged(worker: &str, tag: &str) -> Actor {
        let Actor::Agent(mut agent) = agent(worker) else {
            unreachable!()
        };
        agent.tags.push(Tag::parse(tag).unwrap());
        Actor::Agent(agent)
    }

    #[test]
    fn the_main_agent_or_an_area_lead_claims_a_request() {
        let main = tagged("lead", MAIN_TAG);
        assert_eq!(
            can_claim_request(&main, None).unwrap().worker.as_str(),
            "lead"
        );
        let area_lead = agent("web-lead");
        assert_eq!(
            can_claim_request(&area_lead, Some(Role::AreaLead))
                .unwrap()
                .worker
                .as_str(),
            "web-lead"
        );
        for (other, role) in [
            (agent("backend"), None),
            (tagged("backend", "review"), None),
            (agent("review-lead"), Some(Role::ReviewLead)),
            (agent("web-coder"), Some(Role::TaskCoder)),
            (agent("reviewer-1"), Some(Role::BatchReviewer)),
        ] {
            let denied = can_claim_request(&other, role).unwrap_err();
            assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED, "{other:?}");
            assert!(denied.hint().contains("main agent or an area lead"));
        }
        for actor in [
            tagged("lead/sub", MAIN_TAG),
            agent("web-lead/plan"),
            human("tanim"),
            Actor::System,
        ] {
            let denied = can_claim_request(&actor, Some(Role::AreaLead)).unwrap_err();
            assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
            assert!(denied.hint().contains("claim_request"));
        }
    }

    #[test]
    fn only_the_requester_decides() {
        let backend = Requester::parse("backend").unwrap();
        assert_eq!(code(is_requester(&agent("backend"), &backend, None)), None);
        assert_eq!(
            code(is_requester(&agent("backend/sub"), &backend, None)),
            None
        );
        for actor in [agent("frontend"), human("backend"), Actor::System] {
            assert_eq!(
                code(is_requester(&actor, &backend, None)),
                Some(codes::REQUESTER_REQUIRED)
            );
        }
        let tanim = Requester::parse("human:tanim").unwrap();
        assert_eq!(code(is_requester(&human("tanim"), &tanim, None)), None);
        assert_eq!(
            code(is_requester(&human("ana"), &tanim, None)),
            Some(codes::REQUESTER_REQUIRED)
        );
    }

    #[test]
    fn the_deputy_decides_for_its_short_lived_requester() {
        let coder = Requester::parse("web-coder").unwrap();
        let lead = Requester::parse("web-lead").unwrap();
        for actor in [agent("web-coder"), agent("web-lead"), agent("web-lead/sub")] {
            assert_eq!(code(is_requester(&actor, &coder, Some(&lead))), None);
        }
        for actor in [agent("frontend"), human("ana"), Actor::System] {
            assert_eq!(
                code(is_requester(&actor, &coder, Some(&lead))),
                Some(codes::REQUESTER_REQUIRED)
            );
        }
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

use crate::agents::{AgentName, HumanName, Tag, WorkerName};
use crate::events::EventActor;

/// An agent acting through one of its workers. The services build it from the session and `resolve_worker`; `tags` are the agent's tags read at the start of the call, so a policy decision never sees a tag that changes during it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActor {
    pub agent: AgentName,
    pub worker: WorkerName,
    pub tags: Vec<Tag>,
}

/// Who runs a command. Every mutating command takes one and checks it with `policy` before its transaction. Humans are always named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Agent(AgentActor),
    Human {
        name: HumanName,
    },
    /// The daemon itself: recompute, the container rule, jobs.
    System,
}

impl Actor {
    pub fn human(name: HumanName) -> Actor {
        Actor::Human { name }
    }

    /// The label stored in events, reviews and comments: the worker name, `human:<name>` or `fold`.
    pub fn event_actor(&self) -> EventActor {
        match self {
            Actor::Agent(agent) => EventActor::worker(&agent.worker),
            Actor::Human { name } => EventActor::human(name),
            Actor::System => EventActor::fold(),
        }
    }

    pub fn is_human(&self) -> bool {
        matches!(self, Actor::Human { .. })
    }
}

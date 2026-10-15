use thiserror::Error;

use crate::actor::{Actor, AgentActor};
use crate::agents::{Tag, WorkerName};
use crate::error::{FoldError, codes};

/// A refusal by a policy gate. The code is a stable error code, so every transport reports it unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct PolicyDenied {
    code: &'static str,
    message: &'static str,
    hint: &'static str,
}

impl PolicyDenied {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn hint(&self) -> &'static str {
        self.hint
    }
}

impl From<PolicyDenied> for FoldError {
    fn from(denied: PolicyDenied) -> Self {
        FoldError::new(denied.code, denied.message).with_hint(denied.hint)
    }
}

/// Only an agent's worker holds a claim, because a claim names the worker whose workspace does the work. A human or the system works on a task through an agent. It returns the admitted agent, which is the claimant.
pub fn can_claim(actor: &Actor) -> Result<&AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) => Ok(agent),
        Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::ACTOR_CANNOT_CLAIM,
            message: "only an agent's worker can claim a task",
            hint: "ask an agent to claim the task with claim_task",
        }),
    }
}

/// A push publishes main to a shared Git remote with the user's own Git credentials, so a human runs it, or the daemon itself when it pushes after an integration. An agent never pushes.
pub fn can_push(actor: &Actor) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } | Actor::System => Ok(()),
        Actor::Agent(_) => Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human can push main to the Git remote",
            hint: "ask a human to run fold push",
        }),
    }
}

/// Only a human grants or removes `integrator`, because the integrator moves main. Any other tag is a free label that an agent sets on itself at registration.
pub fn can_set_tag(actor: &Actor, tag: &Tag, add: bool) -> Result<(), PolicyDenied> {
    if !tag.is_integrator() || actor.is_human() {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: if add {
            "only a human can grant the integrator tag"
        } else {
            "only a human can remove the integrator tag"
        },
        hint: "ask a human to run fold agent tag <agent> integrator",
    })
}

/// Only the worker that owns a workspace abandons it, because abandoning gives up that worker's own work. `owner` is the workspace's worker; a human, the system and every other worker are refused.
pub fn can_abandon_workspace(
    actor: &Actor,
    owner: Option<&WorkerName>,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(agent) if owner == Some(&agent.worker) => Ok(()),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_WORKSPACE_OWNER,
            message: "only the owning worker can abandon a workspace",
            hint: "call get_workspace to see the owner",
        }),
    }
}

/// Feedback is the human path onto a task: it is stored as a message from the named human, so an agent or the system sends an ordinary message instead.
pub fn can_send_feedback(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: "only a human writes feedback on a task",
        hint: "send a message linked to the task with send_message",
    })
}

/// Only the worker that owns a workspace records, restores or syncs in it, because a workspace holds that one worker's unfinished work. `owner` is the workspace's worker; a human, the system and every other worker are refused.
pub fn can_change_workspace(actor: &Actor, owner: Option<&WorkerName>) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(agent) if owner == Some(&agent.worker) => Ok(()),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_WORKSPACE_OWNER,
            message: "only the owning worker can change a workspace",
            hint: "call get_workspace to see the owner",
        }),
    }
}

use thiserror::Error;

use crate::actor::{Actor, AgentActor};
use crate::agents::Tag;
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

use std::path::PathBuf;

use fold_core::{AgentName, FoldError, codes};
use fold_store::agents;

use super::{AgentError, AgentService};
use crate::push::{PushToken, check_socket};

impl AgentService {
    /// Stores the Claude Code inbox socket of a session of `agent`, as the session-start hook reports it, so the daemon can post notices into that session. The socket path changes at every session start. Nothing is written to the database or the event log: the token stays in memory and is never formatted.
    pub async fn register_push_target(
        &self,
        agent: &AgentName,
        socket: PathBuf,
        token: &str,
    ) -> Result<(), AgentError> {
        check_socket(&socket)?;
        let token = PushToken::parse(token)?;
        if agents::find_agent(self.store.read(), agent)
            .await?
            .is_none()
        {
            return Err(FoldError::new(
                codes::AGENT_NOT_REGISTERED,
                format!("agent {agent} is not registered"),
            )
            .with_hint("call register_agent")
            .into());
        }
        self.push.register(agent, socket, token, self.clock.now());
        Ok(())
    }

    /// Drops every push target of `agent`.
    pub fn clear_push_targets(&self, agent: &AgentName) {
        self.push.clear(agent);
    }
}

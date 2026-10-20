use std::path::PathBuf;

use fold_core::{AgentName, FoldError, Timestamp, codes, policy};
use fold_store::agents;

use super::{AgentError, AgentService, SessionContext};
use crate::push::{PushReach, PushToken, check_socket};

impl AgentService {
    /// Stores the Claude Code inbox socket of a session of `agent`, as the session-start hook reports it, so the daemon can post notices into that session. The socket path changes at every session start. The socket and token are also kept in the daemon's own file of the agent, outside the project folder, for a restarted daemon; nothing is written to the database or the event log, and the token is never formatted. An agent the project does not know yet is refused with `agent_not_registered`, and its inbox waits in memory until a session's adapter registers the agent.
    pub async fn register_push_target(
        &self,
        agent: &AgentName,
        socket: PathBuf,
        token: &str,
    ) -> Result<(), AgentError> {
        check_socket(&socket)?;
        let token = PushToken::parse(token)?;
        let now = self.clock.now();
        if agents::find_agent(self.store.read(), agent)
            .await?
            .is_none()
        {
            self.push.hold(agent, socket, token, now);
            return Err(FoldError::new(
                codes::AGENT_NOT_REGISTERED,
                format!("agent {agent} is not registered"),
            )
            .with_hint("call register_agent")
            .into());
        }
        self.push.keep(agent, &socket, &token).await;
        self.push.register(agent, socket, token, now);
        Ok(())
    }

    /// Holds the inbox socket that the adapter's handshake reports for the session `ctx` as `agent`'s, in memory, before the handshake registers the agent, which registers it as a target counted from the session's start. A session that may not bind to `agent` holds nothing.
    pub fn keep_session_inbox(
        &self,
        ctx: &SessionContext,
        agent: &AgentName,
        socket: PathBuf,
        token: &str,
    ) -> Result<(), AgentError> {
        if policy::can_bind_agent(&ctx.peer, agent).is_err() {
            return Ok(());
        }
        check_socket(&socket)?;
        let token = PushToken::parse(token)?;
        self.push.hold(agent, socket, token, self.clock.now());
        Ok(())
    }

    /// Registers the inbox held for `agent`, else the socket kept for it, as a target registered at `at`; a socket the agent has already counts from `at` when that is later. Returns whether a target was added.
    pub async fn restore_push_target(&self, agent: &AgentName, at: Timestamp) -> bool {
        self.push.restore(agent, at).await
    }

    /// Registers the kept socket of every agent of the project, as a project opens after the daemon started, once the temporary files of cut-short writes are gone. Returns how many it added.
    pub async fn restore_push_targets(&self) -> Result<usize, AgentError> {
        self.push.sweep().await;
        let now = self.clock.now();
        let mut restored = 0;
        for agent in agents::list_agents(self.store.read()).await? {
            if self.push.restore(&agent.name, now).await {
                restored += 1;
            }
        }
        Ok(restored)
    }

    /// How a notice to `agent` would travel now.
    pub fn push_reach(&self, agent: &AgentName) -> PushReach {
        self.push.reach(agent)
    }

    /// Drops every push target of `agent`.
    pub fn clear_push_targets(&self, agent: &AgentName) {
        self.push.clear(agent);
    }
}

use std::collections::HashMap;

use fold_core::policy;
use fold_core::{Actor, AgentName, FoldError};
use fold_store::StoreError;
use fold_store::agents::{self, PreviousSessionRow, WorkerProcessRow};

use super::{AgentError, AgentService};

impl AgentService {
    /// Starts the worker again in a new Claude Code session; `Supervisor::restart_fresh` names the steps. A named human or the main agent, on the user's word, asks for it, and so does the lead that started the worker. `instructions` are added to the first turn of the new session and never stored.
    pub async fn restart_worker(
        &self,
        actor: &Actor,
        agent: &AgentName,
        instructions: Option<&str>,
    ) -> Result<WorkerProcessRow, AgentError> {
        let supervisor = self.supervisor()?.clone();
        let starter = self.starter(actor).await?;
        let started_it = match actor {
            Actor::Agent(acting) => {
                let mut conn = self
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                agents::find_worker_process(&mut conn, agent)
                    .await?
                    .and_then(|row| crate::push::starter_agent(&row.started_by))
                    .is_some_and(|starter| starter == acting.agent)
            }
            Actor::Human { .. } | Actor::System => false,
        };
        policy::can_restart_worker(&starter, started_it).map_err(FoldError::from)?;
        supervisor
            .restart_fresh(agent, &actor.event_actor(), instructions)
            .await?;
        self.get_worker(agent).await
    }

    /// The sessions each worker had before its current one, oldest first, by agent name.
    pub async fn previous_sessions(
        &self,
    ) -> Result<HashMap<AgentName, Vec<PreviousSessionRow>>, AgentError> {
        Ok(agents::previous_sessions(self.store.read()).await?)
    }
}

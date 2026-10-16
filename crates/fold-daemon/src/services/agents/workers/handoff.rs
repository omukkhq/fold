use fold_core::roles::check_handoff;
use fold_core::{Actor, AgentName, FoldError, WorkerState, codes, policy};
use fold_store::StoreError;
use fold_store::agents::{self, HandoffRow};

use super::not_supervised;
use crate::services::agents::{AgentError, AgentService};

impl AgentService {
    /// Stores the acting lead's hand-off note for its current session. fold then starts the lead again with a fresh session that reads the note: at once when the lead is idle, else when its turn ends. The note is content and goes into no log, event or error.
    pub async fn write_handoff(&self, actor: &Actor, body: &str) -> Result<(), AgentError> {
        let supervisor = self.supervisor()?.clone();
        let starter = self.starter(actor).await?;
        policy::can_write_handoff(&starter).map_err(FoldError::from)?;
        let body = check_handoff(body)?;
        let Actor::Agent(agent) = actor else {
            return Err(FoldError::new(
                codes::HANDOFF_DENIED,
                "only a lead writes a hand-off note",
            )
            .with_hint("keep working; only an area lead or the review lead calls write_handoff")
            .into());
        };
        let agent = &agent.agent;
        let now = self.clock.now();
        let row = self
            .store
            .write(async |tx| {
                let row = agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or_else(|| not_supervised(agent))?;
                agents::insert_handoff(tx.conn(), row.agent_id, &row.session_id, body, now).await?;
                Ok::<_, AgentError>(row)
            })
            .await?;
        if row.state == WorkerState::Idle {
            supervisor.refresh_soon(agent);
        }
        Ok(())
    }

    /// The newest hand-off note of `agent`, for `list_workers`.
    pub async fn latest_handoff(
        &self,
        agent: &AgentName,
    ) -> Result<Option<HandoffRow>, AgentError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let Some(row) = agents::find_agent(&mut *conn, agent).await? else {
            return Ok(None);
        };
        Ok(agents::latest_handoff(&mut *conn, row.id).await?)
    }
}

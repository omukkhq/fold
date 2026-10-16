use fold_core::events::types;
use fold_core::policy;
use fold_core::{Actor, FoldError, NewEvent, Timestamp};
use fold_store::agents::{self, PauseRow};
use fold_store::tasks::claims;
use serde_json::json;

use super::super::{AgentError, AgentService};
use crate::services::events::EventBuffer;

/// Whether the project's agents are paused, since when and by whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseState {
    pub paused_since: Option<Timestamp>,
    /// The event actor label of whoever paused, such as `human:ana` or an agent name.
    pub paused_by: Option<String>,
}

impl From<PauseRow> for PauseState {
    fn from(row: PauseRow) -> Self {
        Self {
            paused_since: row.paused_at,
            paused_by: row.paused_by,
        }
    }
}

impl AgentService {
    pub async fn pause_state(&self) -> Result<PauseState, AgentError> {
        Ok(agents::pause_state(self.store.read()).await?.into())
    }

    /// Pauses every agent of the project: in one transaction with `project.paused` the pause is stored, so it outlives a daemon restart; then every worker process ends the way a daemon stop ends it, interrupting a working turn, and each row keeps its state and session for the resume. While paused fold starts no worker, no claim goes stale, and the idle sweep and stall detection wait; integration and push go on. The main agent's own session is not supervised, so it only hears of the pause. A project that is paused already stays as it is, without a second event.
    pub async fn pause_project(&self, actor: &Actor) -> Result<PauseState, AgentError> {
        let supervisor = self.supervisor()?.clone();
        let starter = self.starter(actor).await?;
        policy::can_pause_project(&starter).map_err(FoldError::from)?;
        let by = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let paused = self
            .store
            .write(async |tx| {
                if !agents::set_paused(tx, by.as_str(), now).await? {
                    return Ok::<_, AgentError>(false);
                }
                let event = NewEvent::new(types::PROJECT_PAUSED, by.clone())?.data(json!({}))?;
                buffer.append(tx, &event, now).await?;
                Ok(true)
            })
            .await?;
        self.bus.publish(buffer);
        if paused {
            supervisor.detach_all(true).await;
        }
        self.pause_state().await
    }

    /// Ends the pause: in one transaction with `project.resumed` the pause is cleared and every unreleased claim gets a full lease from now; then each worker the pause ended runs again in its own session, and the starts the pause held back are served. A project that is not paused stays as it is, without an event.
    pub async fn resume_project(&self, actor: &Actor) -> Result<PauseState, AgentError> {
        let supervisor = self.supervisor()?.clone();
        let starter = self.starter(actor).await?;
        policy::can_pause_project(&starter).map_err(FoldError::from)?;
        let by = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let paused_at = self
            .store
            .write(async |tx| {
                let Some(paused_at) = agents::set_resumed(tx, now).await? else {
                    return Ok::<_, AgentError>(None);
                };
                let renewed = claims::renew_all(tx, now).await?;
                let event = NewEvent::new(types::PROJECT_RESUMED, by.clone())?.data(json!({
                    "paused_at": paused_at.as_millis(),
                    "renewed_claims": renewed,
                }))?;
                buffer.append(tx, &event, now).await?;
                Ok(Some(paused_at))
            })
            .await?;
        self.bus.publish(buffer);
        if let Some(paused_at) = paused_at {
            supervisor.resume_after_pause(paused_at).await?;
        }
        self.pause_state().await
    }
}

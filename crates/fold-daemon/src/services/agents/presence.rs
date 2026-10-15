use std::collections::HashMap;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{AgentName, EventError, LiveEvent, Presence, SessionFact, Timestamp, presence_of};
use fold_store::agents;
use serde_json::json;
use tokio::sync::Mutex;

use super::{AgentError, AgentService};
use crate::services::events::EventBus;

/// The presence of each agent as the calls and the presence job of one project last saw it, which decides when an `agent.presence_changed` frame is due. The job holds the lock across its read of the sessions, so a call that commits during that read takes the lock after the job and corrects a stale `idle`.
#[derive(Debug, Clone, Default)]
pub struct SeenPresence(Arc<Mutex<HashMap<AgentName, Presence>>>);

impl SeenPresence {
    pub fn new() -> Self {
        Self::default()
    }

    /// After a committed call: `active` is sent when the agent was not active before the call or was last seen otherwise. A call on an ended session leaves the agent as it was.
    pub(super) async fn after_call(
        &self,
        bus: &EventBus,
        agent: &AgentName,
        before: Presence,
        after: Presence,
    ) -> Result<(), EventError> {
        if after != Presence::Active {
            return Ok(());
        }
        let mut seen = self.0.lock().await;
        let last = seen.insert(agent.clone(), Presence::Active);
        if before != Presence::Active || last != Some(Presence::Active) {
            bus.publish_live(frame(agent, Presence::Active)?);
        }
        Ok(())
    }
}

fn frame(agent: &AgentName, presence: Presence) -> Result<LiveEvent, EventError> {
    LiveEvent::new(
        types::AGENT_PRESENCE_CHANGED,
        None,
        json!({"agent": agent.as_str(), "presence": presence.as_str()}),
    )
}

pub(super) fn facts(open: &[Option<Timestamp>]) -> Vec<SessionFact> {
    open.iter()
        .map(|last_call_at| SessionFact {
            ended: false,
            last_call_at: *last_call_at,
        })
        .collect()
}

impl AgentService {
    /// Sends `agent.presence_changed` for every agent last seen active that is no longer active at the clock's time, and remembers every agent's presence. It writes nothing and returns how many frames it sent.
    pub async fn sweep_presence(&self) -> Result<usize, AgentError> {
        let mut seen = self.seen.0.lock().await;
        let rows = agents::open_session_calls_by_agent(self.store.read()).await?;
        let now = self.clock.now();
        let mut open: HashMap<AgentName, Vec<Option<Timestamp>>> = HashMap::new();
        for (agent, last_call_at) in rows {
            open.entry(agent).or_default().push(last_call_at);
        }
        let mut names: Vec<AgentName> = seen.keys().chain(open.keys()).cloned().collect();
        names.sort();
        names.dedup();
        let mut sent = 0;
        for agent in names {
            let calls = open.get(&agent).map(Vec::as_slice).unwrap_or_default();
            let current = presence_of(now, &facts(calls));
            let last = if current == Presence::Offline {
                seen.remove(&agent)
            } else {
                seen.insert(agent.clone(), current)
            };
            if last == Some(Presence::Active) && current != Presence::Active {
                self.bus.publish_live(frame(&agent, current)?);
                sent += 1;
            }
        }
        Ok(sent)
    }
}

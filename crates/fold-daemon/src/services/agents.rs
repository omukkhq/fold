use std::collections::HashMap;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, AgentName, Clock, EventActor, EventError, FoldError, INTEGRATOR_TAG,
    NewEvent, Tag, TaskId, Timestamp, WorkerName, codes, policy,
};
use fold_store::agents::{self, AgentRow, WorkerRow};
use fold_store::{Store, StoreError, WriteTx};
use serde_json::json;
use thiserror::Error;

use crate::services::events::{EventBuffer, EventBus};
use crate::services::tasks::renew_leases;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Event(#[from] EventError),
}

/// What one adapter connection knows about itself. `session_id` is a database row id and stays inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub session_id: i64,
    pub agent: Option<AgentName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub agent: AgentName,
    pub tags: Vec<Tag>,
    pub main_worker: WorkerName,
    /// True when this call created the agent.
    pub created: bool,
}

/// One line of the agent directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSummary {
    pub agent: AgentName,
    pub tags: Vec<Tag>,
    /// The main worker first, then the children by name.
    pub workers: Vec<WorkerName>,
}

/// An agent and its whole tag set after a tag command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTags {
    pub agent: AgentName,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredWorker {
    pub worker: WorkerRow,
    /// True when this call created the worker.
    pub created: bool,
}

/// Commands and lookups for agents, sessions and workers. A command reads the time from `clock`, appends its events in its write transaction and publishes them after the commit.
#[derive(Clone)]
pub struct AgentService {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AgentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentService").finish_non_exhaustive()
    }
}

impl AgentService {
    pub fn new(store: Store, bus: EventBus, clock: Arc<dyn Clock>) -> Self {
        Self { store, bus, clock }
    }

    /// Inserts the session row when an adapter connects. The session has no agent until `register_agent` binds it, and no event is stored until then.
    pub async fn start_session(&self, pid: u32) -> Result<SessionContext, AgentError> {
        let now = self.clock.now();
        let session_id = self
            .store
            .write(async |tx| agents::insert_session(tx.conn(), pid, now).await)
            .await?;
        Ok(SessionContext {
            session_id,
            agent: None,
        })
    }

    /// Creates the agent and its main worker on first use, merges `tags` and binds the session. A repeat call changes nothing except the tags it adds. It keeps the session context instead of an `Actor` because it binds that session; the registering agent is the actor that `policy::can_set_tag` checks for each tag.
    pub async fn register_agent(
        &self,
        ctx: &mut SessionContext,
        name: &AgentName,
        tags: &[Tag],
    ) -> Result<Registered, AgentError> {
        let current = match agents::find_agent(self.store.read(), name).await? {
            Some(agent) => agents::tags(self.store.read(), agent.id).await?,
            None => Vec::new(),
        };
        let registering = Actor::Agent(AgentActor {
            agent: name.clone(),
            worker: WorkerName::main(name),
            tags: current,
        });
        for tag in tags {
            policy::can_set_tag(&registering, tag, true).map_err(FoldError::from)?;
        }
        let session_id = ctx.session_id;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let registered = self
            .store
            .write(async |tx| register_agent_in(tx, &mut buffer, session_id, name, tags, now).await)
            .await?;
        self.bus.publish(buffer);
        ctx.agent = Some(name.clone());
        Ok(registered)
    }

    /// Creates the child worker `<parent>/<segment>` of the caller's agent. `parent` defaults to the main worker. A repeat call returns the existing worker.
    pub async fn register_worker(
        &self,
        ctx: &SessionContext,
        segment: &str,
        parent: Option<&WorkerName>,
    ) -> Result<RegisteredWorker, AgentError> {
        let agent = self.require_agent(ctx).await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let registered = self
            .store
            .write(async |tx| {
                let parent = match parent {
                    Some(name) => agents::find_worker(tx.conn(), agent.id, name).await?,
                    None => agents::main_worker(tx.conn(), agent.id).await?,
                }
                .ok_or_else(worker_not_found)?;
                let name = parent.name.child(segment).map_err(FoldError::from)?;
                if let Some(existing) = agents::find_worker(tx.conn(), agent.id, &name).await? {
                    return Ok::<_, AgentError>(RegisteredWorker {
                        worker: existing,
                        created: false,
                    });
                }
                let id =
                    agents::insert_worker(tx.conn(), agent.id, &name, Some(parent.id), now).await?;
                let event = NewEvent::new(types::WORKER_REGISTERED, main_actor(&agent.name))?
                    .data(json!({
                        "agent": agent.name.as_str(),
                        "worker": name.as_str(),
                        "parent": parent.name.as_str(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok(RegisteredWorker {
                    worker: WorkerRow {
                        id,
                        agent_id: agent.id,
                        name,
                        parent_id: Some(parent.id),
                        last_call_at: None,
                        activity: None,
                    },
                    created: true,
                })
            })
            .await?;
        self.bus.publish(buffer);
        Ok(registered)
    }

    /// Every agent with its tags and workers, ordered by name. The three reads share one read transaction, so they see one state.
    pub async fn list_agents(&self) -> Result<Vec<AgentSummary>, AgentError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let rows = agents::list_agents(&mut *tx).await?;
        let tags = agents::all_tags(&mut *tx).await?;
        let mut workers = agents::all_workers(&mut *tx).await?;
        tx.commit().await.map_err(StoreError::from)?;
        workers.sort_by(|a, b| (!a.name.is_main(), &a.name).cmp(&(!b.name.is_main(), &b.name)));
        let mut tags_of: HashMap<i64, Vec<Tag>> = HashMap::new();
        for (agent_id, tag) in tags {
            tags_of.entry(agent_id).or_default().push(tag);
        }
        let mut workers_of: HashMap<i64, Vec<WorkerName>> = HashMap::new();
        for worker in workers {
            workers_of
                .entry(worker.agent_id)
                .or_default()
                .push(worker.name);
        }
        Ok(rows
            .into_iter()
            .map(|row| AgentSummary {
                tags: tags_of.remove(&row.id).unwrap_or_default(),
                workers: workers_of.remove(&row.id).unwrap_or_default(),
                agent: row.name,
            })
            .collect())
    }

    /// The agent the session is bound to, or `agent_not_registered`.
    pub async fn require_agent(&self, ctx: &SessionContext) -> Result<AgentRow, AgentError> {
        let missing = || {
            FoldError::new(
                codes::AGENT_NOT_REGISTERED,
                "this session has no registered agent",
            )
            .with_hint("call register_agent")
        };
        let name = ctx.agent.as_ref().ok_or_else(missing)?;
        let agent = agents::find_agent(self.store.read(), name)
            .await?
            .ok_or_else(missing)?;
        Ok(agent)
    }

    /// The acting agent of a call: the worker from `resolve_worker` and the agent's tags as they are now.
    pub async fn actor(
        &self,
        ctx: &SessionContext,
        as_worker: Option<&WorkerName>,
    ) -> Result<Actor, AgentError> {
        let worker = self.resolve_worker(ctx, as_worker).await?;
        let tags = agents::tags(self.store.read(), worker.agent_id).await?;
        Ok(Actor::Agent(AgentActor {
            agent: worker.name.agent(),
            worker: worker.name,
            tags,
        }))
    }

    /// Grants (`on`) or removes `integrator` on `agent`. `policy::can_set_tag` runs first, so only a human gets past it. A grant of a present tag or a removal of an absent one changes nothing and appends no event.
    pub async fn set_integrator(
        &self,
        actor: &Actor,
        agent: &AgentName,
        on: bool,
    ) -> Result<AgentTags, AgentError> {
        let tag = Tag::parse(INTEGRATOR_TAG).map_err(FoldError::from)?;
        policy::can_set_tag(actor, &tag, on).map_err(FoldError::from)?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let tags = self
            .store
            .write(async |tx| {
                let row = agents::find_agent(tx.conn(), agent).await?.ok_or_else(|| {
                    FoldError::new(
                        codes::AGENT_NOT_FOUND,
                        format!("agent {agent} does not exist"),
                    )
                    .with_details([("agent", json!(agent.as_str()))])
                    .with_hint("the agent registers itself with register_agent first")
                })?;
                let changed = if on {
                    agents::add_tag(tx.conn(), row.id, &tag, now).await?
                } else {
                    agents::remove_tag(tx.conn(), row.id, &tag).await?
                };
                if changed {
                    let event_type = if on {
                        types::AGENT_TAG_ADDED
                    } else {
                        types::AGENT_TAG_REMOVED
                    };
                    let event = NewEvent::new(event_type, actor.event_actor())?
                        .data(json!({"agent": agent.as_str(), "tag": tag.as_str()}))?;
                    buffer.append(tx, &event, now).await?;
                }
                Ok::<_, AgentError>(agents::tags(tx.conn(), row.id).await?)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(AgentTags {
            agent: agent.clone(),
            tags,
        })
    }

    /// The worker named by `as_worker`, or the agent's main worker when it is absent. A worker of another agent is reported as `worker_not_found`, so its existence is never revealed.
    pub async fn resolve_worker(
        &self,
        ctx: &SessionContext,
        as_worker: Option<&WorkerName>,
    ) -> Result<WorkerRow, AgentError> {
        let agent = self.require_agent(ctx).await?;
        let worker = match as_worker {
            Some(name) => agents::find_worker(self.store.read(), agent.id, name).await?,
            None => agents::main_worker(self.store.read(), agent.id).await?,
        };
        Ok(worker.ok_or_else(worker_not_found)?)
    }

    /// Stamps the session and the worker with the time of a tool call and renews the claims of every worker of the agent in the same transaction. The worker's activity is the tool name, followed by the task id when there is one.
    pub async fn record_call(
        &self,
        ctx: &SessionContext,
        worker: &WorkerRow,
        tool: &str,
        task: Option<TaskId>,
    ) -> Result<(), AgentError> {
        let now = self.clock.now();
        let activity = match task {
            Some(task) => format!("{tool} {task}"),
            None => tool.to_owned(),
        };
        self.store
            .write(async |tx| {
                agents::touch_session(tx.conn(), ctx.session_id, now).await?;
                agents::touch_worker(tx.conn(), worker.id, now, &activity).await?;
                let workers = agents::worker_ids(tx.conn(), worker.agent_id).await?;
                renew_leases(tx, &workers, now).await?;
                Ok::<_, StoreError>(())
            })
            .await?;
        Ok(())
    }

    /// Ends the session. A second call does nothing. `session.ended` is stored only for a session that was bound.
    pub async fn end_session(&self, ctx: &SessionContext) -> Result<(), AgentError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let Some(session) = agents::find_session(tx.conn(), ctx.session_id).await? else {
                    return Ok::<_, AgentError>(());
                };
                if !agents::end_session(tx.conn(), ctx.session_id, now).await? {
                    return Ok(());
                }
                let Some(agent_id) = session.agent_id else {
                    return Ok(());
                };
                let name = agents::agent_name(tx.conn(), agent_id)
                    .await?
                    .ok_or(StoreError::Corrupt("sessions.agent_id"))?;
                let event = NewEvent::new(types::SESSION_ENDED, main_actor(&name))?
                    .data(json!({"agent": name.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                Ok(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Ends every session still open at daemon start, because no adapter survives a restart. Returns how many sessions it ended.
    pub async fn close_orphan_sessions(&self) -> Result<u64, AgentError> {
        self.close_open_sessions("daemon_restart").await
    }

    /// Ends every session still open when the daemon stops. The `session.ended` events carry `reason = "daemon_stop"`.
    pub async fn close_sessions_at_shutdown(&self) -> Result<u64, AgentError> {
        self.close_open_sessions("daemon_stop").await
    }

    async fn close_open_sessions(&self, reason: &str) -> Result<u64, AgentError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                let bound = agents::open_bound_sessions(tx.conn()).await?;
                let ended = agents::end_open_sessions(tx.conn(), now).await?;
                for session in bound {
                    let event = NewEvent::new(types::SESSION_ENDED, EventActor::fold())?
                        .data(json!({"agent": session.agent.as_str(), "reason": reason}))?;
                    buffer.append(tx, &event, now).await?;
                }
                Ok::<_, AgentError>(ended)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(ended)
    }
}

async fn register_agent_in(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    session_id: i64,
    name: &AgentName,
    tags: &[Tag],
    now: Timestamp,
) -> Result<Registered, AgentError> {
    let session = agents::find_session(tx.conn(), session_id)
        .await?
        .ok_or(StoreError::Invalid("the session does not exist"))?;
    if session.ended_at.is_some() {
        return Err(StoreError::Invalid("the session has ended").into());
    }
    let existing = agents::find_agent(tx.conn(), name).await?;
    if let Some(bound) = session.agent_id
        && existing.as_ref().map(|agent| agent.id) != Some(bound)
    {
        return Err(FoldError::new(
            codes::AGENT_ALREADY_BOUND,
            "this session is already bound to another agent",
        )
        .with_hint("start a new session to act as another agent")
        .into());
    }
    let actor = main_actor(name);
    let (agent_id, created) = match existing {
        Some(agent) => (agent.id, false),
        None => {
            let id = agents::insert_agent(tx.conn(), name, now).await?;
            let main = WorkerName::main(name);
            agents::insert_worker(tx.conn(), id, &main, None, now).await?;
            let registered = NewEvent::new(types::AGENT_REGISTERED, actor.clone())?
                .data(json!({"agent": name.as_str()}))?;
            buffer.append(tx, &registered, now).await?;
            let worker = NewEvent::new(types::WORKER_REGISTERED, actor.clone())?
                .data(json!({"agent": name.as_str(), "worker": main.as_str()}))?;
            buffer.append(tx, &worker, now).await?;
            (id, true)
        }
    };
    for tag in tags {
        agents::add_tag(tx.conn(), agent_id, tag, now).await?;
    }
    if session.agent_id.is_none() {
        agents::bind_session(tx.conn(), session_id, agent_id).await?;
        let started = NewEvent::new(types::SESSION_STARTED, actor)?
            .data(json!({"agent": name.as_str(), "pid": session.pid}))?;
        buffer.append(tx, &started, now).await?;
    }
    Ok(Registered {
        agent: name.clone(),
        tags: agents::tags(tx.conn(), agent_id).await?,
        main_worker: WorkerName::main(name),
        created,
    })
}

fn main_actor(agent: &AgentName) -> EventActor {
    EventActor::worker(&WorkerName::main(agent))
}

fn worker_not_found() -> FoldError {
    FoldError::new(codes::WORKER_NOT_FOUND, "the worker does not exist")
        .with_hint("call register_worker")
}

use std::path::Path;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, AgentName, Clock, EventActor, EventError, FoldError, INTEGRATOR_TAG,
    NewEvent, ProjectConfig, ProjectPaths, Tag, TaskId, Timestamp, WorkerName, codes, policy,
    presence_of,
};
use fold_store::agents::{self, AgentRow, WorkerRow};
use fold_store::{Store, StoreError, WriteTx};
use serde_json::json;
use thiserror::Error;

use crate::live::LiveState;
use crate::push::PushTargets;
use crate::services::events::{EventBuffer, EventBus};
use crate::services::projects::{Lease, ProjectError};
use crate::services::tasks::{TaskError, renew_leases};

/// Claude Code's headless stream-json protocol as fold reads and writes it, the oldest Claude Code fold drives, and the sign-in answer of `claude auth status`. It is public because `fld doctor` checks the installed Claude Code with the same version and sign-in types.
pub mod claude;
mod context;
mod needs_you;
mod overview;
mod presence;
mod push;
mod workers;

pub use context::{
    CONTEXT_READY_TASKS, ContextClaim, MAX_SESSION_CONTEXT, SessionFacts,
    render as render_session_context,
};
pub use needs_you::{NeedsYouItem, NeedsYouKind, Screen, needs_you};
pub use overview::{
    AgentAttention, AgentOverview, ClaimSummary, MAX_OVERVIEW_EVENTS, OverviewFilter,
    ProposalSummary, SessionSummary, WorkerOverview, WorkspaceSummary,
};
pub use presence::SeenPresence;
pub(crate) use workers::code_of;
pub use workers::{
    FOLD_SERVER, Launcher, RESTART_TURN, StartWorker, StopTimeouts, Supervisor, WorkerLimitsChanged,
};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Event(#[from] EventError),
}

impl From<TaskError> for AgentError {
    fn from(error: TaskError) -> Self {
        match error {
            TaskError::Fold(error) => Self::Fold(error),
            TaskError::Store(error) => Self::Store(error),
            TaskError::Event(error) => Self::Event(error),
        }
    }
}

impl From<ProjectError> for AgentError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Fold(error) => Self::Fold(error),
            ProjectError::Store(error) => Self::Store(error),
        }
    }
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

/// Commands and lookups for agents, sessions and workers. A command reads the time from `clock`, appends its events in its write transaction and publishes them after the commit. `seen` is shared by every service and the presence job of one project, so they agree on when presence changed.
#[derive(Clone)]
pub struct AgentService {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    lease: Lease,
    seen: SeenPresence,
    push: PushTargets,
    live: Option<LiveState>,
    workers: Option<Supervisor>,
}

impl std::fmt::Debug for AgentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentService").finish_non_exhaustive()
    }
}

impl AgentService {
    /// The lease is the default of a project without configuration until [`AgentService::with_lease_minutes`] or [`AgentService::with_project_config`] sets the project's own. The seen presence is the service's own until [`AgentService::with_seen_presence`] shares the project's.
    pub fn new(store: Store, bus: EventBus, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            bus,
            clock,
            lease: Lease::Minutes(ProjectConfig::default().lease_minutes),
            seen: SeenPresence::new(),
            push: PushTargets::new(),
            live: None,
            workers: None,
        }
    }

    #[must_use]
    pub fn with_lease_minutes(mut self, lease_minutes: u64) -> Self {
        self.lease = Lease::Minutes(lease_minutes);
        self
    }

    /// Reads the lease from the project's `.fold/config.toml` on every overview; an invalid file fails the overview with its configuration error.
    #[must_use]
    pub fn with_project_config(mut self, paths: ProjectPaths) -> Self {
        self.lease = Lease::Config(paths);
        self
    }

    #[must_use]
    pub fn with_seen_presence(mut self, seen: SeenPresence) -> Self {
        self.seen = seen;
        self
    }

    /// Shares the project's push targets, which [`AgentService::end_session`] clears when an agent's last session ends. Until then the service has targets of its own.
    #[must_use]
    pub fn with_push_targets(mut self, push: PushTargets) -> Self {
        self.push = push;
        self
    }

    /// Lets the overview read the stalled flags of the project's live workspaces. Without it no task counts as stalled.
    #[must_use]
    pub fn with_live(mut self, live: LiveState) -> Self {
        self.live = Some(live);
        self
    }

    /// Runs the worker commands through the project's supervisor. Without it they fail with `internal_error`.
    #[must_use]
    pub fn with_workers(mut self, workers: Supervisor) -> Self {
        self.workers = Some(workers);
        self
    }

    async fn lease_minutes(&self) -> Result<u64, AgentError> {
        Ok(self.lease.minutes().await?)
    }

    /// Inserts a session row that records no working directory. The session has no agent until `register_agent` binds it, and no event is stored until then.
    pub async fn start_session(&self, pid: u32) -> Result<SessionContext, AgentError> {
        self.insert_session(pid, None).await
    }

    /// Inserts the session row when an adapter connects, with the canonical working directory from its handshake, which the agents overview shows.
    pub async fn start_session_in(
        &self,
        pid: u32,
        cwd: &Path,
    ) -> Result<SessionContext, AgentError> {
        self.insert_session(pid, Some(&cwd.to_string_lossy())).await
    }

    async fn insert_session(
        &self,
        pid: u32,
        cwd: Option<&str>,
    ) -> Result<SessionContext, AgentError> {
        let now = self.clock.now();
        let session_id = self
            .store
            .write(async |tx| agents::insert_session(tx.conn(), pid, cwd, now).await)
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
        let granter = actor.event_actor();
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
                    agents::add_tag(tx.conn(), row.id, &tag, Some(granter.as_str()), now).await?
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

    /// Stamps the session and the worker with the time of a tool call and renews the claims of every worker of the agent in the same transaction. The worker's activity is the tool name, followed by the task id when there is one. After the commit it sends a live `agent.presence_changed` when the call made the agent active.
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
        let (before, after) = self
            .store
            .write(async |tx| {
                let before = agents::open_session_calls(tx.conn(), worker.agent_id).await?;
                agents::touch_session(tx.conn(), ctx.session_id, now).await?;
                agents::touch_worker(tx.conn(), worker.id, now, &activity).await?;
                let after = agents::open_session_calls(tx.conn(), worker.agent_id).await?;
                let workers = agents::worker_ids(tx.conn(), worker.agent_id).await?;
                renew_leases(tx, &workers, now).await?;
                Ok::<_, StoreError>((
                    presence_of(now, &presence::facts(&before)),
                    presence_of(now, &presence::facts(&after)),
                ))
            })
            .await?;
        self.seen
            .after_call(&self.bus, &worker.name.agent(), before, after)
            .await?;
        Ok(())
    }

    /// Ends the session. A second call does nothing. `session.ended` is stored only for a session that was bound. When it was the agent's last open session, the agent's push targets registered up to its start are dropped after the commit; a target a newer session registered since stays.
    pub async fn end_session(&self, ctx: &SessionContext) -> Result<(), AgentError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let last = self
            .store
            .write(async |tx| {
                let Some(session) = agents::find_session(tx.conn(), ctx.session_id).await? else {
                    return Ok::<_, AgentError>(None);
                };
                if !agents::end_session(tx.conn(), ctx.session_id, now).await? {
                    return Ok(None);
                }
                let Some(agent_id) = session.agent_id else {
                    return Ok(None);
                };
                let name = agents::agent_name(tx.conn(), agent_id)
                    .await?
                    .ok_or(StoreError::Corrupt("sessions.agent_id"))?;
                let event = NewEvent::new(types::SESSION_ENDED, main_actor(&name))?
                    .data(json!({"agent": name.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                let open = agents::open_session_calls(tx.conn(), agent_id).await?;
                Ok(open.is_empty().then_some((name, session.started_at)))
            })
            .await?;
        self.bus.publish(buffer);
        if let Some((agent, started_at)) = last {
            self.push.clear_registered_by(&agent, started_at);
        }
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
        self.push.clear_all();
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
    if tags.iter().any(Tag::is_main) {
        let supervised = agents::find_worker_process(tx.conn(), name)
            .await?
            .is_some();
        let held_by_other = agents::all_tags(tx.conn())
            .await?
            .iter()
            .any(|(holder, tag)| tag.is_main() && *holder != agent_id);
        policy::can_take_main_tag(supervised, held_by_other).map_err(FoldError::from)?;
    }
    for tag in tags {
        agents::add_tag(tx.conn(), agent_id, tag, None, now).await?;
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

use std::path::Path;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::policy::{SessionPeer, TagOrigin};
use fold_core::{
    Actor, AgentActor, AgentName, Clock, EventActor, EventError, FoldError, NewEvent,
    ProjectConfig, ProjectPaths, Tag, TaskId, Timestamp, WorkerName, codes, policy, presence_of,
};
use fold_store::agents::{self, AgentRow, WorkerRow};
use fold_store::{SqliteConnection, Store, StoreError, WriteTx};
use serde_json::json;
use thiserror::Error;

use crate::live::LiveState;
use crate::push::PushTargets;
use crate::services::events::{EventBuffer, EventBus};
use crate::services::projects::{Lease, ProjectError};
use crate::services::reviews::ReviewError;
use crate::services::tasks::{TaskError, renew_leases};
use crate::services::workspaces::WorkspaceLocks;

/// Claude Code's headless stream-json protocol as fold reads and writes it, the oldest Claude Code fold drives, and the sign-in answer of `claude auth status`. It is public because `fld doctor` checks the installed Claude Code with the same version and sign-in types.
pub mod claude;
mod context;
mod cost;
/// The environment of the workers: which variables of the login shell they take, and what the daemon found about it at its start.
pub mod environment;
mod needs_you;
mod overview;
/// The kernel's view of a socket peer: whether a worker that fold started is the peer or one of its ancestors.
pub mod peers;
mod presence;
mod push;
mod workers;

pub use context::{
    CONTEXT_READY_TASKS, ContextClaim, MAX_SESSION_CONTEXT, SessionFacts,
    render as render_session_context,
};
pub use cost::{ProjectCost, cost_totals};
pub use needs_you::{NeedsYouItem, NeedsYouKind, Screen, needs_you, with_folder_attention};
pub use overview::{
    AgentAttention, AgentOverview, ClaimSummary, MAX_OVERVIEW_EVENTS, OverviewFilter,
    ProcessSummary, ProposalSummary, SessionSummary, WorkerOverview, WorkspaceSummary,
};
pub use presence::SeenPresence;
#[cfg(feature = "dev-launcher")]
pub use workers::{DEV_STUB_COMMAND, DEV_STUB_DIR};
pub use workers::{
    Launcher, MCP_RETRY_TURN, PAUSE_TURN, PauseState, RESTART_TURN, StartWorker, StartedWorker,
    StopTimeouts, Supervisor, WorkerLimitsChanged, new_session_id,
};
pub(crate) use workers::{code_of, starter_of};

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

impl From<ReviewError> for AgentError {
    fn from(error: ReviewError) -> Self {
        match error {
            ReviewError::Fold(error) => Self::Fold(error),
            ReviewError::Store(error) => Self::Store(error),
            ReviewError::Task(error) => error.into(),
            ReviewError::Event(error) => Self::Event(error),
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

impl AgentError {
    /// The code a log line names: the stable code, or `internal_error` for an internal failure.
    pub fn log_code(&self) -> &'static str {
        match self {
            Self::Fold(error) => error.code(),
            Self::Store(error) => error.log_code(),
            Self::Event(_) => codes::INTERNAL_ERROR,
        }
    }
}

/// What one adapter connection knows about itself. `session_id` is a database row id and stays inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub session_id: i64,
    pub agent: Option<AgentName>,
    /// The worker process tree the session runs in, resolved once when the socket connection opens; [`AgentService::register_agent`] binds it to no other agent.
    pub peer: SessionPeer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub agent: AgentName,
    pub tags: Vec<Tag>,
    pub main_worker: WorkerName,
    /// True when this call created the agent.
    pub created: bool,
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
    /// The project's workspace locks (`ProjectHandle::workspace_locks`): a start that gives a task worker a released workspace waits for a sync, restore or record there to finish.
    ops: Arc<WorkspaceLocks>,
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
            ops: Arc::new(WorkspaceLocks::default()),
        }
    }

    #[must_use]
    pub fn with_workspace_locks(mut self, ops: Arc<WorkspaceLocks>) -> Self {
        self.ops = ops;
        self
    }

    #[must_use]
    pub fn with_lease_minutes(mut self, lease_minutes: u64) -> Self {
        self.lease = Lease::Minutes(lease_minutes);
        self
    }

    /// Reads the lease from the project's `.fold/config.toml` on every overview; an invalid file fails the overview with its configuration error.
    #[must_use]
    pub fn with_project_config(mut self, paths: ProjectPaths) -> Self {
        self.lease = Lease::config(paths);
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
        Ok(self.lease.staleness_minutes(&self.store).await?)
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
            .write(async |tx| agents::insert_session(tx, pid, cwd, now).await)
            .await?;
        Ok(SessionContext {
            session_id,
            agent: None,
            peer: SessionPeer::Unsupervised,
        })
    }

    /// Creates the agent and its main worker on first use, merges `tags` and binds the session. A repeat call changes nothing except the tags it adds. After the commit it registers the inbox held for the agent, else its kept inbox socket, as a push target counted from the session's start, which covers a session whose start hook ran before the agent existed and a session that reconnects to a restarted daemon; the end of that session drops the target and its kept file. It keeps the session context instead of an `Actor` because it binds that session. A session under a worker's process tree binds only to that worker's agent; any other name appends `session.bind_refused` and fails. The tags come from the adapter's handshake, which carries the `main` tag of `fld claude`.
    pub async fn register_agent(
        &self,
        ctx: &mut SessionContext,
        name: &AgentName,
        tags: &[Tag],
    ) -> Result<Registered, AgentError> {
        self.register(ctx, name, tags, TagOrigin::Handshake).await
    }

    async fn register(
        &self,
        ctx: &mut SessionContext,
        name: &AgentName,
        tags: &[Tag],
        origin: TagOrigin,
    ) -> Result<Registered, AgentError> {
        if let Err(denied) = policy::can_bind_agent(&ctx.peer, name) {
            self.refuse_bind(ctx, name).await?;
            return Err(FoldError::from(denied).into());
        }
        let session_id = ctx.session_id;
        // A session that `fld claude` starts asks for the tag in its handshake from outside every worker's process tree; the supervised main's own adapter runs inside its process tree.
        let interactive_main = origin == TagOrigin::Handshake
            && ctx.peer == SessionPeer::Unsupervised
            && tags.iter().any(Tag::is_main);
        // A worker of another role never holds the tag, and the tag rule refuses it in the registration.
        let role = if interactive_main {
            let mut conn = self
                .store
                .read()
                .acquire()
                .await
                .map_err(StoreError::from)?;
            agents::find_worker_process(&mut conn, name)
                .await?
                .map(|row| row.role)
        } else {
            None
        };
        let gate = self
            .workers
            .as_ref()
            .filter(|_| interactive_main && role.is_none_or(|role| role == fold_core::Role::Main));
        if let Some(supervisor) = gate {
            supervisor.enter_interactive_main(session_id, name)?;
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                register_agent_in(tx, &mut buffer, session_id, name, tags, origin, now).await
            })
            .await;
        let (registered, started_at) = match written {
            Ok(written) => written,
            Err(error) => {
                if let Some(supervisor) = gate {
                    supervisor.leave_interactive_main(session_id);
                }
                return Err(error);
            }
        };
        self.bus.publish(buffer);
        ctx.agent = Some(name.clone());
        self.push.restore(name, started_at).await;
        Ok(registered)
    }

    /// [`AgentService::register_agent`] for the `register_agent` tool, whose tags the session names itself: it never gives the `main` tag to an agent that lacks it (`policy::can_take_main_tag`). A call that acts as `as_worker` names one of `name`'s workers that exists, or the agent's main worker, which the registration creates; anything else is `worker_not_found` before anything is written.
    pub async fn register_agent_from_tool(
        &self,
        ctx: &mut SessionContext,
        name: &AgentName,
        tags: &[Tag],
        as_worker: Option<&WorkerName>,
    ) -> Result<Registered, AgentError> {
        if let Some(worker) = as_worker
            && *worker != WorkerName::main(name)
        {
            let found = match agents::find_agent(self.store.read(), name).await? {
                Some(agent) => agents::find_worker(self.store.read(), agent.id, worker)
                    .await?
                    .is_some(),
                None => false,
            };
            if !found {
                return Err(worker_not_found().into());
            }
        }
        self.register(ctx, name, tags, TagOrigin::RegisterTool)
            .await
    }

    async fn refuse_bind(&self, ctx: &SessionContext, name: &AgentName) -> Result<(), AgentError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let session_id = ctx.session_id;
        let worker = ctx.peer.worker().map(AgentName::as_str);
        self.store
            .write(async |tx| {
                let pid = agents::find_session(tx.conn(), session_id)
                    .await?
                    .map(|session| session.pid);
                let event = NewEvent::new(types::SESSION_BIND_REFUSED, EventActor::fold())?
                    .data(json!({"agent": name.as_str(), "worker": worker, "pid": pid}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.bus.publish(buffer);
        tracing::warn!(agent = %name, worker, "refused a bind from a worker's process tree");
        Ok(())
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
                let id = agents::insert_worker(tx, agent.id, &name, Some(parent.id), now).await?;
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
        self.actor_of(&worker).await
    }

    /// The acting agent of a call whose worker is already resolved, with the agent's tags as they are now.
    pub async fn actor_of(&self, worker: &WorkerRow) -> Result<Actor, AgentError> {
        let tags = agents::tags(self.store.read(), worker.agent_id).await?;
        Ok(Actor::Agent(AgentActor {
            agent: worker.name.agent(),
            worker: worker.name.clone(),
            tags,
        }))
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
                agents::touch_session(tx, ctx.session_id, now).await?;
                agents::touch_worker(tx, worker.id, now, &activity).await?;
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
                if !agents::end_session(tx, ctx.session_id, now).await? {
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
        if let Some(supervisor) = &self.workers {
            supervisor.leave_interactive_main(ctx.session_id);
        }
        if let Some((agent, started_at)) = last {
            self.push.clear_registered_by(&agent, started_at).await;
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
                let ended = agents::end_open_sessions(tx, now).await?;
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
        if let Some(supervisor) = &self.workers {
            supervisor.clear_interactive_main();
        }
        Ok(ended)
    }
}

async fn register_agent_in(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    session_id: i64,
    name: &AgentName,
    tags: &[Tag],
    origin: TagOrigin,
    now: Timestamp,
) -> Result<(Registered, Timestamp), AgentError> {
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
            let id = agents::insert_agent(tx, name, now).await?;
            let main = WorkerName::main(name);
            agents::insert_worker(tx, id, &main, None, now).await?;
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
            .is_some_and(|row| row.role != fold_core::Role::Main);
        let holders: Vec<i64> = agents::all_tags(tx.conn())
            .await?
            .into_iter()
            .filter(|(_, tag)| tag.is_main())
            .map(|(holder, _)| holder)
            .collect();
        let holds_main = holders.contains(&agent_id);
        let held_by_other = holders.iter().any(|holder| *holder != agent_id);
        policy::can_take_main_tag(origin, holds_main, supervised, held_by_other)
            .map_err(FoldError::from)?;
    }
    for tag in tags {
        agents::add_tag(tx, agent_id, tag, None, now).await?;
    }
    if session.agent_id.is_none() {
        agents::bind_session(tx, session_id, agent_id).await?;
        let started = NewEvent::new(types::SESSION_STARTED, actor)?
            .data(json!({"agent": name.as_str(), "pid": session.pid}))?;
        buffer.append(tx, &started, now).await?;
    }
    Ok((
        Registered {
            agent: name.clone(),
            tags: agents::tags(tx.conn(), agent_id).await?,
            main_worker: WorkerName::main(name),
            created,
        },
        session.started_at,
    ))
}

fn main_actor(agent: &AgentName) -> EventActor {
    EventActor::worker(&WorkerName::main(agent))
}

pub(crate) fn worker_not_found() -> FoldError {
    FoldError::new(codes::WORKER_NOT_FOUND, "the worker does not exist")
        .with_hint("call register_worker")
}

/// The worker row of `actor`, read on `conn`: a command's transaction, or the read pool for a check before one; `None` when its agent or the worker does not exist.
pub(crate) async fn actor_worker(
    conn: &mut SqliteConnection,
    actor: &AgentActor,
) -> Result<Option<WorkerRow>, StoreError> {
    match agents::find_agent(&mut *conn, &actor.agent).await? {
        Some(agent) => agents::find_worker(&mut *conn, agent.id, &actor.worker).await,
        None => Ok(None),
    }
}

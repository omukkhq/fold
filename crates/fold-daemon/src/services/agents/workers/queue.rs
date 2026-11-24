use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::start_queue::{QueuedStart, queue_order};
use fold_core::tasks::dag::Dag;
use fold_core::{
    Actor, AgentName, AreaName, EventActor, FoldError, NewEvent, ProposalId, ReviewPlanId, Role,
    TaskId, TaskStatus, Timestamp, WorkerState, codes, policy,
};
use fold_store::agents::{self, DropReason, NewQueuedStart, QueuedStartRow, WorkerProcessRow};
use fold_store::reviews::ProposalState;
use fold_store::{SqliteConnection, StoreError, WriteTx, reviews, tasks};
use serde_json::json;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::{AgentError, AgentService, StartWorker, StartedWorker, binding_area};
use crate::services::events::{BusMessage, EventBuffer, EventBus};
use crate::services::messages::tell_actor_in_tx;

/// Where a start waits in the queue: `position` 1 runs first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedWorkerStart {
    pub position: usize,
    pub role: Role,
    pub area: Option<AreaName>,
    pub name: Option<AgentName>,
}

/// What `start_worker` gave when its caller lets it wait for a place.
#[derive(Debug, Clone)]
pub enum StartOutcome {
    Started(Box<StartedWorker>),
    Queued(QueuedWorkerStart),
}

/// What `restart_worker` gave when its caller lets it wait for a place.
#[derive(Debug, Clone)]
pub enum RestartOutcome {
    Restarted(Box<WorkerProcessRow>),
    Queued(QueuedWorkerStart),
}

/// One waiting start as `list_workers` shows it, in the order fold runs the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub position: usize,
    pub role: Role,
    pub area: Option<AreaName>,
    pub name: Option<AgentName>,
    pub tasks: Vec<TaskId>,
    pub proposals: Vec<ProposalId>,
    pub plan: Option<ReviewPlanId>,
    pub queued_by: String,
    pub queued_at: Timestamp,
}

/// How fold runs a waiting start. A row with a `name` that is a supervised worker of the same role is that worker's queued `restart_worker`: it restarts in a new session while the worker has ended, and drops once the worker runs again. A row with a `name` of another agent drops, since a new agent cannot take it; a row with a free name or none is a queued `start_worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum QueuedRun {
    Start,
    Restart(AgentName),
    Drop(DropReason),
}

/// The [`QueuedRun`] of a waiting start of `role` with `name`: `existing` is the role and state of the supervised worker of that name, `agent_exists` whether any agent has it.
pub(super) fn queued_run(
    role: Role,
    name: Option<&AgentName>,
    existing: Option<(Role, WorkerState)>,
    agent_exists: bool,
) -> QueuedRun {
    let Some(name) = name else {
        return QueuedRun::Start;
    };
    match existing {
        Some((existing, state)) if existing == role => {
            if state.is_running() {
                QueuedRun::Drop(DropReason::Cancelled)
            } else {
                QueuedRun::Restart(name.clone())
            }
        }
        Some(_) => QueuedRun::Drop(DropReason::Cancelled),
        None if agent_exists => QueuedRun::Drop(DropReason::Cancelled),
        None => QueuedRun::Start,
    }
}

/// Whether the worker limits, or a resume fold owes first, refused a start.
pub(super) fn is_limit_refusal(error: &AgentError) -> bool {
    matches!(error, AgentError::Fold(error) if error.code() == codes::WORKER_LIMIT_REACHED)
}

/// Whether a refusal of the worker limits names a limit of 0, which no stop frees a place under.
fn zero_limit(error: &FoldError) -> bool {
    error.details().is_some_and(|details| {
        ["max", "max_per_area"]
            .iter()
            .any(|key| details.get(*key).is_some_and(|max| max == 0))
    })
}

/// Adds to a refusal of the worker limits that `tool` can wait for a place with `queue: true`; a limit of 0 gets no such hint, and a hint that names `queue: true` already stays as it is.
pub(super) fn with_queue_hint(error: AgentError, tool: &str) -> AgentError {
    match error {
        AgentError::Fold(fold)
            if fold.code() == codes::WORKER_LIMIT_REACHED && !zero_limit(&fold) =>
        {
            let hint = match fold.hint() {
                Some(hint) if hint.contains("queue: true") => return AgentError::Fold(fold),
                Some(hint) => format!(
                    "{hint}; or call {tool} again with queue: true, and fold runs it when a place frees"
                ),
                None => format!(
                    "call {tool} again with queue: true, and fold runs it when a place frees"
                ),
            };
            AgentError::Fold(fold.with_hint(hint))
        }
        other => other,
    }
}

/// Why the waiting start `row` cannot run any more, read on `conn`: a task archived, deleted or Done, a task claimed by a running worker of another agent than `own` with a claim that is not stale, or a proposal that is no longer open. A claim of a worker that has ended, or a stale one, moves to the new worker, as the start itself moves it.
async fn drop_reason(
    conn: &mut SqliteConnection,
    tasks_of: &[TaskId],
    proposals: &[ProposalId],
    own: Option<&AgentName>,
) -> Result<Option<DropReason>, StoreError> {
    for task in tasks_of {
        let Some(row) = tasks::fetch(&mut *conn, *task).await? else {
            return Ok(Some(DropReason::TaskArchived));
        };
        if row.archived_at.is_some() {
            return Ok(Some(DropReason::TaskArchived));
        }
        if row.status == TaskStatus::Done {
            return Ok(Some(DropReason::TaskDone));
        }
        if let Some(claim) = tasks::claims::unreleased_for_task(&mut *conn, *task).await?
            && claim.stale_at.is_none()
            && own.is_none_or(|own| claim.worker.agent() != *own)
            && !crate::services::tasks::holder_has_ended(&mut *conn, &claim.worker).await?
        {
            return Ok(Some(DropReason::TaskClaimed));
        }
    }
    for proposal in proposals {
        let open = reviews::get_proposal(&mut *conn, *proposal)
            .await?
            .is_some_and(|row| row.state == ProposalState::Open);
        if !open {
            return Ok(Some(DropReason::ProposalEnded));
        }
    }
    Ok(None)
}

/// The agent of a stored actor label, `None` for a human and for fold.
fn agent_of(label: &EventActor) -> Option<AgentName> {
    AgentName::parse(label.agent()?).ok()
}

fn ids<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

/// The work of a start in words for a notice, such as "for T-3, T-4".
fn work_text(tasks_of: &[TaskId], proposals: &[ProposalId]) -> String {
    let listed: Vec<String> = ids(tasks_of).into_iter().chain(ids(proposals)).collect();
    if listed.is_empty() {
        String::new()
    } else {
        format!(" for {}", listed.join(", "))
    }
}

/// The waiting start ids in the order fold runs them: [`queue_order`] over the waiting starts, each with its tasks (a reviewer's, the tasks of its proposals), on the dependency graph below those tasks and the time each area last ran a queued start.
async fn ordered(conn: &mut SqliteConnection) -> Result<Vec<QueuedStartRow>, StoreError> {
    let waiting = agents::waiting_starts(&mut *conn).await?;
    let mut starts = Vec::with_capacity(waiting.len());
    let mut seeds = BTreeSet::new();
    for row in &waiting {
        let mut tasks_of = row.tasks.clone();
        for proposal in &row.proposals {
            if let Some(found) = reviews::get_proposal(&mut *conn, *proposal).await? {
                tasks_of.push(found.task);
            }
        }
        seeds.extend(tasks_of.iter().copied());
        starts.push(QueuedStart {
            key: row.id,
            area: row.area.clone(),
            tasks: tasks_of,
            queued_at: row.queued_at,
        });
    }
    let seeds: Vec<TaskId> = seeds.into_iter().collect();
    let dag = Dag::from_edges(tasks::dependent_edges(&mut *conn, &seeds).await?);
    let last = agents::last_queued_starts_by_area(&mut *conn).await?;
    let mut rows = waiting;
    let mut order = Vec::with_capacity(rows.len());
    for key in queue_order(&starts, &dag, &last) {
        if let Some(at) = rows.iter().position(|row| row.id == key) {
            order.push(rows.swap_remove(at));
        }
    }
    Ok(order)
}

/// Ends the queued start `id` with the worker process `worker_process_id` it started, inside that start's transaction. A start that ended already, such as one a second run started, refuses, so the whole start rolls back and runs once.
pub(super) async fn mark_started(
    tx: &mut WriteTx,
    id: i64,
    worker_process_id: i64,
    now: Timestamp,
) -> Result<(), AgentError> {
    if agents::mark_queued_start_started(tx, id, worker_process_id, now).await? {
        return Ok(());
    }
    Err(
        FoldError::new(codes::INVALID_REQUEST, "the queued start ended already")
            .with_details([("reason", json!("queued_start_ended"))])
            .with_hint("call list_workers")
            .into(),
    )
}

/// Tells the starter `by` that its queued worker `agent` started.
pub(super) async fn tell_started_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    by: &EventActor,
    agent: &AgentName,
    tasks_of: &[TaskId],
    proposals: &[ProposalId],
    now: Timestamp,
) -> Result<(), AgentError> {
    let text = format!(
        "fold started @{agent}{}, the worker start you queued, now that a place is free.",
        work_text(tasks_of, proposals)
    );
    tell_actor_in_tx(
        tx,
        buffer,
        by,
        tasks_of.first().copied(),
        proposals.first().copied(),
        text,
        now,
    )
    .await?;
    Ok(())
}

/// What a waiting start ran into.
enum Ran {
    Started,
    Dropped,
    /// `role_full` when the role's own limit, or a resume fold owes, refused it, not the limit of its area: no later start of the role runs in the same pass, so a place that frees while a pass runs goes to the start that comes first in the order. Later starts of the role still drop when their work is gone.
    Waits {
        role_full: bool,
    },
}

/// Whether a start failed for a cause that passes, such as a config file that does not parse for a moment or a file system error, so its queued start waits instead of dropping.
fn passing(error: &FoldError) -> bool {
    [
        codes::INVALID_CONFIG,
        codes::WORKSPACE_IO,
        codes::INTERNAL_ERROR,
    ]
    .contains(&error.code())
}

/// Whether a refusal of the worker limits holds for the whole role rather than for one area.
fn role_full(error: &AgentError) -> bool {
    match error {
        AgentError::Fold(error) => !error
            .details()
            .is_some_and(|details| details.contains_key("max_per_area")),
        AgentError::Store(_) | AgentError::Event(_) => true,
    }
}

impl AgentService {
    /// [`AgentService::start_worker`] that, with `queue`, puts a task worker's or a reviewer's start that the worker limits refuse in the queue instead: see [`AgentService::queue_start`]. A refusal of the limits names `queue: true` in its hint. `queue` with review scopes is refused with `invalid_request`, since the queue does not keep scopes.
    pub async fn start_or_queue_worker(
        &self,
        actor: &Actor,
        request: StartWorker,
        queue: bool,
    ) -> Result<StartOutcome, AgentError> {
        if queue && !request.scopes.is_empty() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "a queued start keeps no review scopes",
            )
            .with_details([("field", json!("scopes"))])
            .with_hint("leave scopes out, then call set_review_scope after the start")
            .into());
        }
        match self.start_worker(actor, request.clone()).await {
            Ok(started) => Ok(StartOutcome::Started(Box::new(started))),
            Err(error) if queue && is_limit_refusal(&error) => self
                .queue_start(actor, &request, None)
                .await
                .map(StartOutcome::Queued),
            Err(error) => Err(with_queue_hint(error, "start_worker")),
        }
    }

    /// [`AgentService::restart_worker`] that, with `queue`, puts the restart of a stopped task worker or reviewer that the worker limits refuse in the queue instead, as a start named after the worker with its tasks or proposals. A refusal of the limits names `queue: true` in its hint.
    pub async fn restart_or_queue_worker(
        &self,
        actor: &Actor,
        agent: &AgentName,
        instructions: Option<&str>,
        queue: bool,
    ) -> Result<RestartOutcome, AgentError> {
        match self.restart_worker(actor, agent, instructions).await {
            Ok(row) => Ok(RestartOutcome::Restarted(Box::new(row))),
            Err(error) if queue && is_limit_refusal(&error) => {
                let row = self.get_worker(agent).await?;
                let request = StartWorker {
                    role: Some(row.role),
                    name: Some(agent.clone()),
                    tasks: row.tasks.clone(),
                    proposals: row.proposals.clone(),
                    instructions: instructions.map(str::to_owned),
                    ..StartWorker::default()
                };
                self.queue_start(actor, &request, Some(&row))
                    .await
                    .map(RestartOutcome::Queued)
            }
            Err(error) => Err(with_queue_hint(error, "restart_worker")),
        }
    }

    /// Stores the start in the queue with `worker.start_queued` in one transaction, and gives its place in the order. A new start's name must be free (`worker_exists`). A start whose tasks or proposals already make it drop is refused with `invalid_request` and the drop reason. `restart` is the stopped worker of a queued restart.
    async fn queue_start(
        &self,
        actor: &Actor,
        request: &StartWorker,
        restart: Option<&WorkerProcessRow>,
    ) -> Result<QueuedWorkerStart, AgentError> {
        let role = request.role.ok_or_else(|| {
            FoldError::new(codes::INVALID_REQUEST, "start_worker needs a role")
                .with_hint("pass task_worker or reviewer")
        })?;
        let starter = self.starter(actor).await?;
        let area = match restart {
            Some(row) => row.area.clone(),
            None => binding_area(role, &starter, request)?,
        };
        let by = actor.event_actor();
        let own = match restart {
            Some(row) => Some(row.agent.clone()),
            None => agent_of(&by),
        };
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let queued = self
            .store
            .write(async |tx| {
                if restart.is_none()
                    && let Some(name) = &request.name
                    && agents::find_agent(tx.conn(), name).await?.is_some()
                {
                    return Err(FoldError::new(
                        codes::WORKER_EXISTS,
                        format!("an agent named {name} exists"),
                    )
                    .with_details([("agent", json!(name.as_str()))])
                    .with_hint("pick another name or leave name out")
                    .into());
                }
                if let Some(reason) =
                    drop_reason(tx.conn(), &request.tasks, &request.proposals, own.as_ref()).await?
                {
                    return Err(FoldError::new(
                        codes::INVALID_REQUEST,
                        "the start cannot wait in the queue: its work is taken or has ended",
                    )
                    .with_details([("reason", json!(reason.as_str()))])
                    .with_hint("call get_task or get_proposal for its work")
                    .into());
                }
                let area_id = match (&area, role) {
                    (Some(area), Role::TaskWorker) => Some(
                        agents::find_area(tx.conn(), area)
                            .await?
                            .ok_or(StoreError::Corrupt("areas"))?
                            .id,
                    ),
                    _ => None,
                };
                let name = request.name.as_ref().map(AgentName::as_str);
                let id = agents::insert_queued_start(
                    tx,
                    &NewQueuedStart {
                        role,
                        area_id,
                        name,
                        tasks: &request.tasks,
                        proposals: &request.proposals,
                        review_plan: request.plan,
                        model: request.model.as_deref(),
                        instructions: request.instructions.as_deref(),
                        queued_by: by.as_str(),
                        now,
                    },
                )
                .await?;
                let position = ordered(tx.conn())
                    .await?
                    .iter()
                    .position(|row| row.id == id)
                    .map_or(0, |at| at + 1);
                let event = NewEvent::new(types::WORKER_START_QUEUED, by.clone())?.data(json!({
                    "role": role.as_str(),
                    "area": area.as_ref().map(AreaName::as_str),
                    "name": name,
                    "tasks": ids(&request.tasks),
                    "proposals": ids(&request.proposals),
                    "plan": request.plan.map(|plan| plan.to_string()),
                    "restart": restart.is_some(),
                    "position": position,
                }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, AgentError>(QueuedWorkerStart {
                    position,
                    role,
                    area: area.clone(),
                    name: request.name.clone(),
                })
            })
            .await?;
        self.bus.publish(buffer);
        Ok(queued)
    }

    /// The waiting starts in the order fold runs them.
    pub async fn queued_starts(&self) -> Result<Vec<QueueEntry>, AgentError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        Ok(ordered(&mut conn)
            .await?
            .into_iter()
            .enumerate()
            .map(|(at, row)| QueueEntry {
                position: at + 1,
                role: row.role,
                area: row.area,
                name: row.name.and_then(|name| AgentName::parse(&name).ok()),
                tasks: row.tasks,
                proposals: row.proposals,
                plan: row.review_plan,
                queued_by: row.queued_by,
                queued_at: row.queued_at,
            })
            .collect())
    }

    /// Runs the waiting starts while places are free, one write transaction for each start, in the order of [`queue_order`] read again after each start. A start that the limits refuse, or a resume fold owes first, or a pause, waits; one whose work was claimed, Done, archived or ended drops with `worker.queued_start_dropped` and a notice to its starter, and so does one whose start fails for another reason. Two runs at once start each row once, since the start's own transaction ends its row.
    pub async fn run_queued_starts(&self) -> Result<(), AgentError> {
        loop {
            let order = {
                let mut conn = self
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                ordered(&mut conn).await?
            };
            let mut started = false;
            let mut full: BTreeSet<Role> = BTreeSet::new();
            for row in order {
                let may_start = !full.contains(&row.role);
                match self.run_one(&row, may_start).await? {
                    Ran::Started => {
                        started = true;
                        break;
                    }
                    Ran::Waits { role_full: true } => {
                        full.insert(row.role);
                    }
                    Ran::Dropped | Ran::Waits { role_full: false } => {}
                }
            }
            if !started {
                return Ok(());
            }
        }
    }

    /// The starter of a queued start with its rights as they are now: a human, or an agent's worker with the agent's tags read again; `None` for a label of fold or of an agent that no longer exists.
    async fn queued_starter(&self, label: &str) -> Result<Option<Actor>, AgentError> {
        let Some(actor) = crate::services::reviews::queued_actor(label) else {
            return Ok(None);
        };
        let Actor::Agent(mut agent) = actor else {
            return Ok(Some(actor));
        };
        let Some(row) = agents::find_agent(self.store.read(), &agent.agent).await? else {
            return Ok(None);
        };
        agent.tags = agents::tags(self.store.read(), row.id).await?;
        Ok(Some(Actor::Agent(agent)))
    }

    /// Drops the waiting `row` when its work or its worker is gone, else tries its start when `may_start`.
    async fn run_one(&self, row: &QueuedStartRow, may_start: bool) -> Result<Ran, AgentError> {
        let Some(actor) = self.queued_starter(&row.queued_by).await? else {
            self.drop_start(row, DropReason::Cancelled, None).await?;
            return Ok(Ran::Dropped);
        };
        let name = row
            .name
            .as_deref()
            .map(AgentName::parse)
            .transpose()
            .map_err(|_| StoreError::Corrupt("worker_start_queue.name"))?;
        let (existing, agent_exists) = match &name {
            None => (None, false),
            Some(name) => {
                let mut conn = self
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                let existing = agents::find_worker_process(&mut conn, name).await?;
                let exists = agents::find_agent(&mut *conn, name).await?.is_some();
                (existing, exists)
            }
        };
        let run = queued_run(
            row.role,
            name.as_ref(),
            existing.as_ref().map(|worker| (worker.role, worker.state)),
            agent_exists,
        );
        let own = match &run {
            QueuedRun::Restart(agent) => Some(agent.clone()),
            QueuedRun::Start | QueuedRun::Drop(_) => agent_of(&actor.event_actor()),
        };
        let reason = match &run {
            QueuedRun::Drop(reason) => Some(*reason),
            QueuedRun::Start | QueuedRun::Restart(_) => {
                let mut conn = self
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                drop_reason(&mut conn, &row.tasks, &row.proposals, own.as_ref()).await?
            }
        };
        if let Some(reason) = reason {
            self.drop_start(row, reason, None).await?;
            return Ok(Ran::Dropped);
        }
        if !may_start {
            return Ok(Ran::Waits { role_full: true });
        }
        let result = match run {
            QueuedRun::Restart(agent) => {
                let starter = self.starter(&actor).await?;
                let started_it = existing
                    .as_ref()
                    .and_then(|worker| crate::push::starter_agent(&worker.started_by))
                    .zip(agent_of(&actor.event_actor()))
                    .is_some_and(|(started, acting)| started == acting);
                match policy::can_restart_worker(&starter, started_it) {
                    Err(error) => Err(AgentError::from(FoldError::from(error))),
                    Ok(()) => self
                        .supervisor()?
                        .restart_fresh(
                            &agent,
                            &actor.event_actor(),
                            row.instructions.as_deref(),
                            Some(row.id),
                        )
                        .await
                        .map(|_| ()),
                }
            }
            QueuedRun::Start | QueuedRun::Drop(_) => {
                let request = StartWorker {
                    role: Some(row.role),
                    name,
                    area: None,
                    tasks: row.tasks.clone(),
                    proposals: row.proposals.clone(),
                    plan: row.review_plan,
                    scopes: Vec::new(),
                    model: row.model.clone(),
                    instructions: row.instructions.clone(),
                };
                self.start_worker_with(&actor, request, Some(row.id))
                    .await
                    .map(|_| ())
            }
        };
        match result {
            Ok(()) => Ok(Ran::Started),
            Err(error) if is_limit_refusal(&error) => Ok(Ran::Waits {
                role_full: role_full(&error),
            }),
            Err(error) if super::is_paused_refusal(&error) => Ok(Ran::Waits { role_full: true }),
            Err(AgentError::Fold(error))
                if error.code() == codes::INVALID_REQUEST
                    && error
                        .details()
                        .and_then(|details| details.get("reason"))
                        .is_some_and(|reason| reason == "queued_start_ended") =>
            {
                Ok(Ran::Waits { role_full: false })
            }
            Err(AgentError::Fold(error)) if passing(&error) => {
                tracing::warn!(
                    code = error.code(),
                    "a queued worker start failed for now; it waits for the next free place"
                );
                Ok(Ran::Waits { role_full: false })
            }
            Err(AgentError::Fold(error)) => {
                let reason = if error.code() == codes::ALREADY_CLAIMED {
                    DropReason::TaskClaimed
                } else {
                    DropReason::Cancelled
                };
                self.drop_start(row, reason, Some(error.code())).await?;
                Ok(Ran::Dropped)
            }
            Err(other) => Err(other),
        }
    }

    /// Ends the waiting `row` without a worker for `reason`, with `worker.queued_start_dropped` and a notice to its starter in the same transaction. `refusal` is the code of the start that failed, when one did. A row that has ended already changes nothing.
    async fn drop_start(
        &self,
        row: &QueuedStartRow,
        reason: DropReason,
        refusal: Option<&'static str>,
    ) -> Result<(), AgentError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                if !agents::drop_queued_start(tx, row.id, reason, now).await? {
                    return Ok::<_, AgentError>(());
                }
                let event =
                    NewEvent::new(types::WORKER_QUEUED_START_DROPPED, EventActor::fold())?.data(
                        json!({
                            "role": row.role.as_str(),
                            "area": row.area.as_ref().map(AreaName::as_str),
                            "name": row.name,
                            "tasks": ids(&row.tasks),
                            "proposals": ids(&row.proposals),
                            "reason": reason.as_str(),
                            "refusal": refusal,
                        }),
                    )?;
                buffer.append(tx, &event, now).await?;
                if let Ok(by) = EventActor::parse(&row.queued_by) {
                    let who = row.name.as_deref().map_or_else(
                        || format!("a {}", row.role.as_str().replace('_', " ")),
                        |name| format!("@{name}"),
                    );
                    let why = match refusal {
                        Some(code) => format!("its start failed with {code}"),
                        None => drop_words(reason).to_owned(),
                    };
                    let text = format!(
                        "fold dropped the worker start you queued for {who}{}: {why}. Read the work again before you start a worker for it.",
                        work_text(&row.tasks, &row.proposals)
                    );
                    tell_actor_in_tx(
                        tx,
                        &mut buffer,
                        &by,
                        row.tasks.first().copied(),
                        row.proposals.first().copied(),
                        text,
                        now,
                    )
                    .await?;
                }
                Ok(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }
}

fn drop_words(reason: DropReason) -> &'static str {
    match reason {
        DropReason::TaskClaimed => "another worker claimed its task",
        DropReason::TaskDone => "its task is Done",
        DropReason::TaskArchived => "its task was archived or deleted",
        DropReason::ProposalEnded => "its proposal is no longer open",
        DropReason::Cancelled => "the worker it names runs again or is no longer the same worker",
    }
}

/// The stored events after which a queued start may run or drop: a worker that started, resumed or restarted outside the queue, which can make a queued restart drop or leave a place an owed resume did not take, a worker process that ended, new limits, the end of a pause, a new queued start, and a task or proposal that a start waits for that was claimed, Done, archived or ended.
fn moves_the_queue(event: &fold_core::Event) -> bool {
    [
        types::WORKER_STARTED,
        types::WORKER_RESUMED,
        types::WORKER_REFRESHED,
        types::WORKER_STOPPED,
        types::WORKER_FAILED,
        types::WORKERS_LIMITS_CHANGED,
        types::PROJECT_RESUMED,
        types::WORKER_START_QUEUED,
        types::CLAIM_ACQUIRED,
        types::TASK_COMPLETED,
        types::TASK_ARCHIVED,
        types::TASK_DELETED,
        types::PROPOSAL_CLOSED,
        types::PROPOSAL_INTEGRATED,
    ]
    .contains(&event.event_type)
}

/// Runs the queued starts after each commit that can free a place or end a queued start's work. A queued start runs after the resumes fold owes, since its start lets those take the free places first. It ends when the project closes.
pub async fn follow_queued_starts(
    service: AgentService,
    bus: EventBus,
    mut events: broadcast::Receiver<BusMessage>,
) {
    let mut closing = bus.closing();
    // The starts a stopped daemon left in the queue run once at the open.
    let mut due = true;
    loop {
        if due {
            // Everything sent so far was committed before it was sent, so the run below covers it.
            while !matches!(
                events.try_recv(),
                Err(TryRecvError::Empty | TryRecvError::Closed)
            ) {}
            if let Err(error) = service.run_queued_starts().await {
                tracing::warn!(
                    code = super::code_of(&error),
                    "the queued worker starts did not run; the next stop runs them"
                );
            }
        }
        due = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            received = events.recv() => match received {
                Ok(BusMessage::Durable(event)) => moves_the_queue(&event),
                Ok(BusMessage::Live(_)) => false,
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => return,
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> AgentName {
        AgentName::parse(text).unwrap()
    }

    #[test]
    fn a_row_without_a_name_or_with_a_free_one_is_a_start() {
        assert_eq!(
            queued_run(Role::TaskWorker, None, None, false),
            QueuedRun::Start
        );
        assert_eq!(
            queued_run(Role::TaskWorker, Some(&name("w-1")), None, false),
            QueuedRun::Start
        );
    }

    #[test]
    fn a_row_named_after_an_ended_worker_of_its_role_restarts_it() {
        for state in [WorkerState::Stopped, WorkerState::Failed] {
            assert_eq!(
                queued_run(
                    Role::Reviewer,
                    Some(&name("reviewer-2")),
                    Some((Role::Reviewer, state)),
                    true
                ),
                QueuedRun::Restart(name("reviewer-2"))
            );
        }
    }

    #[test]
    fn a_row_whose_worker_runs_again_or_is_another_agent_drops() {
        assert_eq!(
            queued_run(
                Role::TaskWorker,
                Some(&name("w-1")),
                Some((Role::TaskWorker, WorkerState::Idle)),
                true
            ),
            QueuedRun::Drop(DropReason::Cancelled)
        );
        assert_eq!(
            queued_run(
                Role::TaskWorker,
                Some(&name("w-1")),
                Some((Role::Reviewer, WorkerState::Stopped)),
                true
            ),
            QueuedRun::Drop(DropReason::Cancelled)
        );
        assert_eq!(
            queued_run(Role::TaskWorker, Some(&name("w-1")), None, true),
            QueuedRun::Drop(DropReason::Cancelled)
        );
    }

    #[test]
    fn a_limit_refusal_gets_the_queue_hint_and_others_stay() {
        let refused: AgentError = FoldError::new(codes::WORKER_LIMIT_REACHED, "full")
            .with_hint("wait")
            .into();
        let AgentError::Fold(hinted) = with_queue_hint(refused, "start_worker") else {
            panic!("a fold error stays one");
        };
        assert_eq!(
            hinted.hint(),
            Some(
                "wait; or call start_worker again with queue: true, and fold runs it when a place frees"
            )
        );
        let bare: AgentError = FoldError::new(codes::WORKER_LIMIT_REACHED, "full").into();
        let AgentError::Fold(bare) = with_queue_hint(bare, "restart_worker") else {
            panic!("a fold error stays one");
        };
        assert!(
            bare.hint()
                .unwrap()
                .starts_with("call restart_worker again with queue: true")
        );
        let owed: AgentError = FoldError::new(codes::WORKER_LIMIT_REACHED, "owed")
            .with_hint(
                "call start_worker again with queue: true, and fold runs it when a place frees",
            )
            .into();
        let AgentError::Fold(owed) = with_queue_hint(owed, "start_worker") else {
            panic!("a fold error stays one");
        };
        assert_eq!(
            owed.hint().unwrap().matches("queue: true").count(),
            1,
            "a hint that names the queue gets no second mention"
        );
        let zero: AgentError = FoldError::new(codes::WORKER_LIMIT_REACHED, "none")
            .with_details([("max", json!(0))])
            .with_hint("raise it")
            .into();
        let AgentError::Fold(zero) = with_queue_hint(zero, "start_worker") else {
            panic!("a fold error stays one");
        };
        assert_eq!(zero.hint(), Some("raise it"));
        let other: AgentError = FoldError::new(codes::INVALID_REQUEST, "no").into();
        let AgentError::Fold(other) = with_queue_hint(other, "start_worker") else {
            panic!("a fold error stays one");
        };
        assert_eq!(other.hint(), None);
    }

    #[test]
    fn a_notice_lists_the_work_of_the_start() {
        let task = TaskId::from_number(3).unwrap();
        let proposal = ProposalId::from_number(4).unwrap();
        assert_eq!(work_text(&[task], &[proposal]), " for T-3, P-4");
        assert_eq!(work_text(&[], &[]), "");
    }
}

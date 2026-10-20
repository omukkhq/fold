use fold_core::events::types;
use fold_core::policy::{self, Starter};
use fold_core::reviews::ScopeChoice;
use fold_core::roles::{
    AreaProfile, AreaProfileChange, StartFacts, check_model, default_worker_name, first_turn,
    outside_area, session_uuid,
};
use fold_core::tasks::areas::check_new_area;
use fold_core::{
    Actor, AgentName, AreaName, FoldError, LabelName, MessageBody, NewEvent, ProjectConfig,
    ProposalId, ReviewBatchId, Role, TaskId, WorkerName, WorkspaceId, codes,
};
use fold_store::agents::{self, AreaRow, NewWorkerProcess, WorkerProcessRow, WorkerRow};
use fold_store::{StoreError, WriteTx};
use fold_store::{tasks, workspaces};
use serde_json::json;

use super::{AgentError, AgentService, main_actor};
use crate::services::events::EventBuffer;
use crate::services::messages::tell_agents_in_tx;
use crate::services::projects::load_config;
use crate::services::reviews::{Assignment, assign_reviews_in_tx};
use crate::services::tasks::assign_in_tx;
use tokio::sync::OwnedMutexGuard;

mod handoff;
mod limits;
mod output;
mod pause;
mod process;

pub use limits::WorkerLimitsChanged;
pub use pause::PauseState;
#[cfg(feature = "dev-launcher")]
pub use process::{DEV_STUB_COMMAND, DEV_STUB_DIR};
pub use process::{Launcher, MCP_RETRY_TURN, PAUSE_TURN, RESTART_TURN, StopTimeouts, Supervisor};

/// The most default names tried for one start before the start gives up.
const MAX_NAME_ATTEMPTS: u32 = 100;

/// What `start_worker` asks for. `area` names the area of an area lead; a task worker takes its lead's area. `tasks` binds a task worker and `proposals` a reviewer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartWorker {
    pub role: Option<Role>,
    pub name: Option<AgentName>,
    pub area: Option<AreaName>,
    pub tasks: Vec<TaskId>,
    pub proposals: Vec<ProposalId>,
    /// The scopes the review lead chooses for the reviews of some of a reviewer's proposals; the others follow fold's suggestion.
    pub scopes: Vec<ScopeChoice>,
    pub model: Option<String>,
    /// Content from the starter for the worker's first turn; it is never stored or logged.
    pub instructions: Option<String>,
}

/// What fold tells the running workers of an area whose instructions changed: the whole new text, or that the area has none now.
fn instructions_message(
    area: &AreaName,
    instructions: Option<&str>,
) -> Result<MessageBody, FoldError> {
    MessageBody::parse(match instructions {
        Some(text) => format!(
            "The instructions of area {area} changed. From now on they take the place of the area's instructions in your system prompt, with the same limits:\n\n{text}"
        ),
        None => format!(
            "Area {area} has no instructions now. From now on ignore the area's instructions in your system prompt; your role and task instructions stay."
        ),
    })
}

/// The stable code of an agent error for a log line.
pub(crate) fn code_of(error: &AgentError) -> &'static str {
    error.log_code()
}

/// A fresh Claude Code session id for a new worker, a lead's fresh context or a worker whose session is gone.
fn new_session() -> Result<String, AgentError> {
    Ok(new_session_id()?)
}

/// A random Claude Code session id, for every session fold starts: a worker's, and a human's main session that `fld claude` starts.
pub fn new_session_id() -> Result<String, FoldError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            "the system could not give random bytes",
        )
    })?;
    Ok(session_uuid(bytes))
}

fn invalid(message: &str, hint: &str) -> AgentError {
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_hint(hint)
        .into()
}

/// A start names each task or proposal once: the claim it gives for the first would refuse the second.
fn named_once<T: Ord>(ids: &[T], field: &str) -> Result<(), AgentError> {
    let distinct: std::collections::BTreeSet<&T> = ids.iter().collect();
    if distinct.len() == ids.len() {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!("start_worker names each of its {field} once"),
    )
    .with_details([("field", json!(field))])
    .with_hint("call start_worker again without the repeated id")
    .into())
}

fn not_supervised(agent: &AgentName) -> AgentError {
    FoldError::new(
        codes::WORKER_PROCESS_NOT_FOUND,
        format!("fold runs no worker named {agent}"),
    )
    .with_details([("agent", json!(agent.as_str()))])
    .with_hint("call list_workers")
    .into()
}

/// Refuses a start inside the transaction that would move a row to `starting` while the project's agents are paused. An automatic resume that this refuses stays owed and is served at the resume.
pub(super) async fn refuse_while_paused(tx: &mut WriteTx) -> Result<(), AgentError> {
    if agents::pause_state(tx.conn()).await?.is_paused() {
        return Err(FoldError::new(
            codes::PROJECT_PAUSED,
            "the agents of this project are paused, so fold starts no worker",
        )
        .with_hint("start it again after resume_project; only the user decides when to resume")
        .into());
    }
    Ok(())
}

/// Whether `error` is the refusal of [`refuse_while_paused`].
pub(super) fn is_paused_refusal(error: &AgentError) -> bool {
    matches!(error, AgentError::Fold(error) if error.code() == codes::PROJECT_PAUSED)
}

fn area_not_found(area: &AreaName) -> AgentError {
    FoldError::new(codes::AREA_NOT_FOUND, format!("area {area} does not exist"))
        .with_details([("area", json!(area.as_str()))])
        .with_hint("the main agent defines it with define_area")
        .into()
}

/// What a start's transaction gave the new worker besides its claims: the workspaces it adopted, whose live owner changes after the commit, and the batch made of its proposals.
#[derive(Default)]
struct Assigned {
    adopted: Vec<WorkspaceId>,
    batch: Option<ReviewBatchId>,
}

/// What a start found inside its transaction: a new worker, or a lead that exists and is stopped and resumes instead.
enum Started {
    New(Box<WorkerProcessRow>),
    Resume(AgentName),
}

impl AgentService {
    fn supervisor(&self) -> Result<&Supervisor, AgentError> {
        self.workers.as_ref().ok_or_else(|| {
            FoldError::new(
                codes::INTERNAL_ERROR,
                "this service runs no worker processes",
            )
            .into()
        })
    }

    /// Who the actor is for the worker rules: a human, the main agent, an agent that is neither, a supervised worker of some role, a subagent or the daemon.
    pub async fn starter(&self, actor: &Actor) -> Result<Starter, AgentError> {
        let supervised = match actor {
            Actor::Agent(agent) => {
                let mut conn = self
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                agents::find_worker_process(&mut conn, &agent.agent)
                    .await?
                    .map(|row| (row.role, row.area))
            }
            Actor::Human { .. } | Actor::System => None,
        };
        Ok(Starter::of(actor, supervised))
    }

    /// Defines area `name` over the tasks labelled `label`, with its `profile`. The label must exist; an area name or label that another area has is refused with `area_exists`, and a label that shares a task that is not archived with another area's label is refused with `two_areas` (`check_new_area`), so no task gains a second area.
    pub async fn define_area(
        &self,
        actor: &Actor,
        name: &AreaName,
        label: &LabelName,
        profile: &AreaProfile,
    ) -> Result<AreaRow, AgentError> {
        let starter = self.starter(actor).await?;
        policy::can_define_area(&starter).map_err(FoldError::from)?;
        let now = self.clock.now();
        let by = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let area = self
            .store
            .write(async |tx| {
                let label_id = agents::label_id(tx.conn(), label).await?.ok_or_else(|| {
                    FoldError::new(
                        codes::LABEL_NOT_FOUND,
                        format!("label {label} does not exist"),
                    )
                    .with_details([("label", json!(label.as_str()))])
                    .with_hint("create it with create_label first")
                })?;
                let areas = agents::list_areas(tx.conn()).await?;
                let taken = areas
                    .iter()
                    .find(|area| area.name == *name || area.label_id == label_id);
                if let Some(taken) = taken {
                    return Err(FoldError::new(
                        codes::AREA_EXISTS,
                        format!("area {} already owns this name or label", taken.name),
                    )
                    .with_details([("area", json!(taken.name.as_str()))])
                    .with_hint("call list_workers to see the areas")
                    .into());
                }
                let labelled: Vec<(TaskId, Vec<LabelName>)> =
                    tasks::labels_by_task(tx.conn(), false)
                        .await?
                        .into_iter()
                        .map(|(task, labels)| {
                            (task, labels.into_iter().map(|label| label.name).collect())
                        })
                        .collect();
                let existing: Vec<(AreaName, LabelName)> = areas
                    .into_iter()
                    .map(|area| (area.name, area.label))
                    .collect();
                check_new_area(name, label, &labelled, &existing)?;
                let area_id = agents::insert_area(tx, name, label_id, by.as_str(), now).await?;
                if !profile.is_empty() {
                    agents::set_area_profile(tx, area_id, profile).await?;
                }
                let event = NewEvent::new(types::AREA_DEFINED, by.clone())?
                    .data(json!({"area": name.as_str(), "label": label.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                agents::find_area(tx.conn(), name)
                    .await?
                    .ok_or(StoreError::Corrupt("areas"))
                    .map_err(AgentError::from)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(area)
    }

    /// Changes the profile of area `name`. The next start of a lead or a task worker of the area gets the new profile in its system prompt. A running process keeps the prompt it started with, so a change of the instructions also goes, in the same transaction, as a message from fold to every running worker of the area, which fold pushes as its next turn.
    pub async fn update_area(
        &self,
        actor: &Actor,
        name: &AreaName,
        change: &AreaProfileChange,
    ) -> Result<AreaRow, AgentError> {
        let starter = self.starter(actor).await?;
        policy::can_update_area(&starter).map_err(FoldError::from)?;
        let now = self.clock.now();
        let by = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let area = self
            .store
            .write(async |tx| {
                let area = agents::find_area(tx.conn(), name)
                    .await?
                    .ok_or_else(|| area_not_found(name))?;
                let profile = change.apply(&area.profile)?;
                agents::set_area_profile(tx, area.id, &profile).await?;
                if change.touches_instructions()
                    && profile.instructions != area.profile.instructions
                {
                    let running: Vec<AgentName> = agents::list_worker_processes(tx.conn())
                        .await?
                        .into_iter()
                        .filter(|row| row.area_id == Some(area.id) && row.state.is_running())
                        .map(|row| row.agent)
                        .collect();
                    let body = instructions_message(name, profile.instructions.as_deref())?;
                    tell_agents_in_tx(tx, &mut buffer, &running, &body, now)
                        .await
                        .map_err(AgentError::from)?;
                }
                let event = NewEvent::new(types::AREA_UPDATED, by.clone())?
                    .data(json!({"area": name.as_str(), "fields": change.fields()}))?;
                buffer.append(tx, &event, now).await?;
                agents::find_area(tx.conn(), name)
                    .await?
                    .ok_or(StoreError::Corrupt("areas"))
                    .map_err(AgentError::from)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(area)
    }

    pub async fn list_areas(&self) -> Result<Vec<AreaRow>, AgentError> {
        Ok(agents::list_areas(self.store.read()).await?)
    }

    /// The project's worker settings from `.fold/config.toml`, or its defaults when there is no file; `None` when the service knows no project folder or the file does not parse, so a list still answers.
    pub async fn worker_config(&self) -> Option<ProjectConfig> {
        load_config(self.config_paths()?).await.ok()
    }

    pub async fn list_workers(&self) -> Result<Vec<WorkerProcessRow>, AgentError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        Ok(agents::list_worker_processes(&mut conn).await?)
    }

    pub async fn get_worker(&self, agent: &AgentName) -> Result<WorkerProcessRow, AgentError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        agents::find_worker_process(&mut conn, agent)
            .await?
            .ok_or_else(|| not_supervised(agent))
    }

    /// The live task workspaces of `tasks` that a new task worker may adopt, each with its operation lock held, taken before the start's transaction as `claim_task` takes it: the adoption then waits for a sync, restore or record of the old owner. Only a task worker is started for tasks.
    async fn lock_task_workspaces(
        &self,
        role: Role,
        tasks: &[TaskId],
    ) -> Result<Vec<(TaskId, WorkspaceId, OwnedMutexGuard<()>)>, AgentError> {
        let mut held = Vec::new();
        if role != Role::TaskWorker {
            return Ok(held);
        }
        let mut live = Vec::new();
        for task in tasks {
            if let Some(workspace) =
                workspaces::live_task_workspace(self.store.read(), *task).await?
            {
                live.push((*task, workspace.id));
            }
        }
        // In id order, so two starts with overlapping tasks never hold one lock each while waiting for the other's.
        live.sort_by_key(|(_, workspace)| *workspace);
        for (task, workspace) in live {
            held.push((task, workspace, self.ops.lock(workspace).await));
        }
        Ok(held)
    }

    /// Starts a worker agent: the agent and its main worker, its process row in state `starting`, its binding and its claims, in one transaction with `worker.started`; then its Claude Code process with its first turn. Only a lead claims, so the start gives the new worker its work: a task worker the implementation claim on each of its tasks (see [`assign_in_tx`]), a reviewer the review claim on its one proposal or a batch of its two or more (see [`assign_reviews_in_tx`]). A claim that cannot be given refuses the whole start, so no worker runs without its claims. A task worker is refused for a human task (`task_needs_human`) and for a task on hold (`task_on_hold`). A lead whose area or role already has a stopped or failed lead resumes that lead instead, and starts it in a new session when its old one is gone.
    pub async fn start_worker(
        &self,
        actor: &Actor,
        request: StartWorker,
    ) -> Result<WorkerProcessRow, AgentError> {
        let supervisor = self.supervisor()?.clone();
        let role = request.role.ok_or_else(|| {
            invalid(
                "start_worker needs a role",
                "pass area_lead, review_lead, task_worker or reviewer",
            )
        })?;
        let starter = self.starter(actor).await?;
        policy::can_start_worker(&starter, role).map_err(FoldError::from)?;
        let area = binding_area(role, &starter, &request)?;
        if let Some(model) = &request.model {
            check_model(model)?;
        }
        let config = load_config(supervisor.paths()).await?;
        let role_model = config.role(role).model;
        let session = new_session()?;
        let by = actor.event_actor();
        let now = self.clock.now();
        let limits = config.workers.limits();
        let lease_minutes = self.lease_minutes().await?;
        let held = self.lock_task_workspaces(role, &request.tasks).await?;
        let own = match actor {
            Actor::Agent(agent) => Some(&agent.agent),
            Actor::Human { .. } | Actor::System => None,
        };
        let mut buffer = EventBuffer::new();
        let mut assigned = Assigned::default();
        let started = self
            .store
            .write(async |tx| {
                refuse_while_paused(tx).await?;
                let area_row = match &area {
                    Some(area) => Some(
                        agents::find_area(tx.conn(), area)
                            .await?
                            .ok_or_else(|| area_not_found(area))?,
                    ),
                    None => None,
                };
                let model = request.model.clone().or_else(|| {
                    area_row
                        .as_ref()
                        .and_then(|area| area.profile.model.clone())
                        .or_else(|| role_model.clone())
                });
                match role {
                    Role::AreaLead => {
                        if let Some(lead) = area_row.as_ref().and_then(|area| area.lead.clone()) {
                            return existing_lead(tx, lead).await;
                        }
                    }
                    Role::ReviewLead => {
                        if let Some(lead) = agents::review_lead(tx.conn()).await? {
                            return existing_lead(tx, lead).await;
                        }
                    }
                    Role::TaskWorker => {
                        let label = area_row
                            .as_ref()
                            .map(|area| area.label.clone())
                            .ok_or(StoreError::Corrupt("areas"))?;
                        let mut facts = Vec::new();
                        for task in &request.tasks {
                            let Some(row) = tasks::fetch(tx.conn(), *task).await? else {
                                return Err(crate::services::tasks::task_missing(*task)
                                    .with_details([("task", json!(task.to_string()))])
                                    .into());
                            };
                            if let Some(refusal) = crate::services::tasks::agent_refusal(&row) {
                                return Err(refusal.into());
                            }
                            let labels = tasks::labels_for_conn(tx.conn(), *task)
                                .await?
                                .into_iter()
                                .map(|label| label.name)
                                .collect();
                            facts.push((*task, labels));
                        }
                        if let Some(task) = outside_area(&label, &facts) {
                            return Err(FoldError::new(
                                codes::TASK_OUTSIDE_AREA,
                                format!("task {task} does not carry the area's label {label}"),
                            )
                            .with_details([("task", json!(task.to_string()))])
                            .with_hint("start task workers only for tasks of your own area")
                            .into());
                        }
                    }
                    Role::Reviewer => {}
                }
                limits::check_running(
                    tx,
                    limits,
                    role,
                    area_row.as_ref().map(|area| (area.id, &area.name)),
                )
                .await?;
                let name = match &request.name {
                    Some(name) => {
                        if agents::find_agent(tx.conn(), name).await?.is_some() {
                            return Err(FoldError::new(
                                codes::WORKER_EXISTS,
                                format!("an agent named {name} exists"),
                            )
                            .with_details([("agent", json!(name.as_str()))])
                            .with_hint("pick another name or leave name out")
                            .into());
                        }
                        name.clone()
                    }
                    None => free_name(tx, role, area.as_ref()).await?,
                };
                let agent_id = agents::insert_agent(tx, &name, now).await?;
                let main = WorkerName::main(&name);
                let worker_id = agents::insert_worker(tx, agent_id, &main, None, now).await?;
                for (kind, data) in [
                    (types::AGENT_REGISTERED, json!({"agent": name.as_str()})),
                    (
                        types::WORKER_REGISTERED,
                        json!({"agent": name.as_str(), "worker": main.as_str()}),
                    ),
                ] {
                    let event = NewEvent::new(kind, main_actor(&name))?.data(data)?;
                    buffer.append(tx, &event, now).await?;
                }
                let id = agents::insert_worker_process(
                    tx,
                    &NewWorkerProcess {
                        agent_id,
                        role,
                        area_id: area_row.as_ref().map(|area| area.id),
                        model: model.as_deref(),
                        session_id: &session,
                        started_by: by.as_str(),
                        now,
                    },
                )
                .await?;
                for task in &request.tasks {
                    agents::bind_task(tx, id, *task).await?;
                }
                for proposal in &request.proposals {
                    if !agents::bind_proposal(tx, id, *proposal).await? {
                        return Err(crate::services::reviews::proposal_missing(*proposal).into());
                    }
                }
                if role == Role::AreaLead
                    && let Some(area) = &area_row
                {
                    agents::set_area_lead(tx, area.id, agent_id).await?;
                }
                let event = NewEvent::new(types::WORKER_STARTED, by.clone())?.data(json!({
                    "agent": name.as_str(),
                    "role": role.as_str(),
                    "area": area.as_ref().map(AreaName::as_str),
                    "tasks": request.tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "proposals": request.proposals.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "model": model,
                }))?;
                buffer.append(tx, &event, now).await?;
                let worker = WorkerRow {
                    id: worker_id,
                    agent_id,
                    name: main,
                    parent_id: None,
                    last_call_at: None,
                    activity: None,
                };
                for task in &request.tasks {
                    let workspace = held
                        .iter()
                        .find(|(held, _, _)| held == task)
                        .map(|(_, workspace, _)| *workspace);
                    let assign = crate::services::tasks::Assign {
                        task: *task,
                        workspace,
                        starter: own,
                    };
                    let claimed =
                        assign_in_tx(tx, &mut buffer, &worker, assign, now, lease_minutes).await?;
                    if let (true, Some(workspace)) = (claimed.adopted, claimed.workspace) {
                        assigned.adopted.push(workspace);
                    }
                }
                let proposals = &request.proposals;
                let lease = lease_minutes;
                crate::services::reviews::free_reviews_for_start(
                    tx,
                    &mut buffer,
                    &worker,
                    proposals,
                    own,
                    lease,
                    now,
                )
                .await?;
                if !request.proposals.is_empty() {
                    let assignment = Assignment {
                        proposals: &request.proposals,
                        scopes: &request.scopes,
                        by: &by,
                    };
                    assigned.batch = assign_reviews_in_tx(
                        tx,
                        &mut buffer,
                        &worker,
                        &assignment,
                        lease_minutes,
                        now,
                    )
                    .await?;
                }
                let row = agents::find_worker_process(tx.conn(), &name)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_processes"))?;
                Ok::<_, AgentError>(Started::New(Box::new(row)))
            })
            .await?;
        drop(held);
        self.bus.publish(buffer);
        if let (Some(live), Started::New(row)) = (&self.live, &started) {
            let holder = WorkerName::main(&row.agent);
            for workspace in &assigned.adopted {
                live.set_worker(*workspace, &holder);
            }
        }
        let label = match &area {
            Some(area) => agents::find_area(self.store.read(), area)
                .await?
                .map(|area| area.label),
            None => None,
        };
        let turn = |name: &AgentName| {
            first_turn(&StartFacts {
                role,
                name,
                area: area.as_ref().zip(label.as_ref()),
                tasks: &request.tasks,
                proposals: &request.proposals,
                batch: assigned.batch,
                instructions: request.instructions.as_deref(),
            })
        };
        match started {
            Started::New(row) => {
                supervisor
                    .launch(&row, false, Some(turn(&row.agent)))
                    .await?;
                self.get_worker(&row.agent).await
            }
            Started::Resume(agent) => {
                let text = turn(&agent);
                if let Some(watch) = supervisor.resume(&agent, text, &[]).await? {
                    tokio::spawn(supervisor.clone().restart_lost(
                        agent.clone(),
                        watch,
                        request.instructions.clone(),
                    ));
                }
                self.get_worker(&agent).await
            }
        }
    }

    /// Stops a running worker on request: see [`Supervisor`] for the steps. Whoever may start a worker of its role may stop it. A worker that is not running is returned as it is.
    pub async fn stop_worker(
        &self,
        actor: &Actor,
        agent: &AgentName,
    ) -> Result<WorkerProcessRow, AgentError> {
        let supervisor = self.supervisor()?.clone();
        let row = self.get_worker(agent).await?;
        let starter = self.starter(actor).await?;
        policy::can_stop_worker(&starter, row.role, row.area.as_ref()).map_err(FoldError::from)?;
        supervisor.stop(agent).await?;
        self.get_worker(agent).await
    }

    /// Starts a stopped or failed worker again in the same agent and session with `--resume`, and sends it `turn`. Whoever may stop the worker may resume it. The worker counts against the limits again. A running worker is refused with `worker_exists`.
    pub async fn resume_worker(
        &self,
        actor: &Actor,
        agent: &AgentName,
        turn: String,
    ) -> Result<WorkerProcessRow, AgentError> {
        let supervisor = self.supervisor()?.clone();
        let row = self.get_worker(agent).await?;
        let starter = self.starter(actor).await?;
        policy::can_stop_worker(&starter, row.role, row.area.as_ref()).map_err(FoldError::from)?;
        supervisor.resume(agent, turn, &[]).await?;
        self.get_worker(agent).await
    }
}

/// [`AgentService::starter`] read inside a write transaction, so a command of another domain decides it with the rows it changes.
pub(crate) async fn starter_of(tx: &mut WriteTx, actor: &Actor) -> Result<Starter, StoreError> {
    let supervised = match actor {
        Actor::Agent(agent) => agents::find_worker_process(tx.conn(), &agent.agent)
            .await?
            .map(|row| (row.role, row.area)),
        Actor::Human { .. } | Actor::System => None,
    };
    Ok(Starter::of(actor, supervised))
}

/// The area a start binds: an area lead names its own, a task worker takes its lead's, the other roles have none. It also checks that each role gets only its own kind of binding.
fn binding_area(
    role: Role,
    starter: &Starter,
    request: &StartWorker,
) -> Result<Option<AreaName>, AgentError> {
    let no_tasks = request.tasks.is_empty();
    let no_proposals = request.proposals.is_empty();
    if role != Role::Reviewer && !request.scopes.is_empty() {
        return Err(invalid(
            "only a reviewer's start takes review scopes",
            "leave scopes out",
        ));
    }
    match role {
        Role::AreaLead => {
            if !no_tasks || !no_proposals {
                return Err(invalid(
                    "an area lead is bound to its area, not to tasks or proposals",
                    "pass area alone",
                ));
            }
            request
                .area
                .clone()
                .map(Some)
                .ok_or_else(|| invalid("an area lead needs an area", "pass the area's name"))
        }
        Role::ReviewLead => {
            if !no_tasks || !no_proposals || request.area.is_some() {
                return Err(invalid(
                    "the review lead takes no area, tasks or proposals",
                    "leave area, tasks and proposals out",
                ));
            }
            Ok(None)
        }
        Role::TaskWorker => {
            let Starter::AreaLead(own) = starter else {
                return Err(invalid(
                    "a task worker needs an area lead",
                    "ask an area lead",
                ));
            };
            if no_tasks || !no_proposals {
                return Err(invalid(
                    "a task worker is bound to one or more tasks",
                    "pass the task ids in tasks",
                ));
            }
            named_once(&request.tasks, "tasks")?;
            if request.area.as_ref().is_some_and(|area| area != own) {
                return Err(FoldError::new(
                    codes::WORKER_START_DENIED,
                    "an area lead starts task workers only in its own area",
                )
                .with_hint("leave area out")
                .into());
            }
            Ok(Some(own.clone()))
        }
        Role::Reviewer => {
            if no_proposals || !no_tasks || request.area.is_some() {
                return Err(invalid(
                    "a reviewer is bound to one or more proposals",
                    "pass the proposal ids in proposals",
                ));
            }
            named_once(&request.proposals, "proposals")?;
            Ok(None)
        }
    }
}

/// A lead that exists: a running one refuses the start, a stopped or failed one resumes.
async fn existing_lead(tx: &mut WriteTx, lead: AgentName) -> Result<Started, AgentError> {
    let row = agents::find_worker_process(tx.conn(), &lead)
        .await?
        .ok_or(StoreError::Corrupt("areas.lead_agent_id"))?;
    if row.state.is_running() {
        let code = if row.role == Role::AreaLead {
            codes::AREA_HAS_LEAD
        } else {
            codes::WORKER_EXISTS
        };
        return Err(
            FoldError::new(code, format!("lead {lead} is already running"))
                .with_details([("agent", json!(lead.as_str()))])
                .with_hint("call list_workers; message the lead instead")
                .into(),
        );
    }
    Ok(Started::Resume(lead))
}

async fn free_name(
    tx: &mut WriteTx,
    role: Role,
    area: Option<&AreaName>,
) -> Result<AgentName, AgentError> {
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let Some(name) = default_worker_name(role, area, attempt) else {
            break;
        };
        if agents::find_agent(tx.conn(), &name).await?.is_none() {
            return Ok(name);
        }
    }
    Err(invalid(
        "fold found no free name for the worker",
        "pass a name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(name: &str) -> AreaName {
        AreaName::parse(name).unwrap()
    }

    #[test]
    fn each_role_takes_only_its_own_binding() {
        let task = TaskId::from_number(1).unwrap();
        let proposal = ProposalId::from_number(1).unwrap();
        let lead = Starter::AreaLead(area("web"));
        let request =
            |area: Option<&str>, tasks: Vec<TaskId>, proposals: Vec<ProposalId>| StartWorker {
                area: area.map(|name| AreaName::parse(name).unwrap()),
                tasks,
                proposals,
                ..StartWorker::default()
            };
        assert_eq!(
            binding_area(
                Role::AreaLead,
                &Starter::MainAgent,
                &request(Some("web"), vec![], vec![])
            )
            .unwrap(),
            Some(area("web"))
        );
        assert!(
            binding_area(
                Role::AreaLead,
                &Starter::MainAgent,
                &request(None, vec![], vec![])
            )
            .is_err()
        );
        assert!(
            binding_area(
                Role::AreaLead,
                &Starter::MainAgent,
                &request(Some("web"), vec![task], vec![])
            )
            .is_err()
        );
        assert_eq!(
            binding_area(
                Role::ReviewLead,
                &Starter::MainAgent,
                &request(None, vec![], vec![])
            )
            .unwrap(),
            None
        );
        assert!(
            binding_area(
                Role::ReviewLead,
                &Starter::MainAgent,
                &request(Some("web"), vec![], vec![])
            )
            .is_err()
        );
        assert_eq!(
            binding_area(Role::TaskWorker, &lead, &request(None, vec![task], vec![])).unwrap(),
            Some(area("web"))
        );
        assert!(binding_area(Role::TaskWorker, &lead, &request(None, vec![], vec![])).is_err());
        assert!(
            binding_area(
                Role::TaskWorker,
                &Starter::MainAgent,
                &request(None, vec![task], vec![])
            )
            .is_err()
        );
        let other = binding_area(
            Role::TaskWorker,
            &lead,
            &request(Some("api"), vec![task], vec![]),
        )
        .unwrap_err();
        assert!(
            matches!(other, AgentError::Fold(error) if error.code() == codes::WORKER_START_DENIED)
        );
        assert_eq!(
            binding_area(
                Role::Reviewer,
                &Starter::ReviewLead,
                &request(None, vec![], vec![proposal])
            )
            .unwrap(),
            None
        );
        assert!(
            binding_area(
                Role::Reviewer,
                &Starter::ReviewLead,
                &request(None, vec![], vec![])
            )
            .is_err()
        );
        for (role, starter, twice) in [
            (
                Role::TaskWorker,
                &lead,
                request(None, vec![task, task], vec![]),
            ),
            (
                Role::Reviewer,
                &Starter::ReviewLead,
                request(None, vec![], vec![proposal, proposal]),
            ),
        ] {
            let repeated = binding_area(role, starter, &twice).unwrap_err();
            assert!(
                matches!(repeated, AgentError::Fold(error) if error.code() == codes::INVALID_REQUEST),
                "{role:?}"
            );
        }
    }
}

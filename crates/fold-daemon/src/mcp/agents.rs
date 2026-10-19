use fold_core::roles::{
    AreaProfile, AreaProfileChange, AreaProfileRequest, WorkerLimits, WorkerLimitsChange,
};
use fold_core::text::shorten;
use fold_core::{
    AgentName, AreaName, FoldError, LabelName, ProposalId, Role, Tag, TaskId, WorkerName, codes,
};
use fold_store::agents::{AreaRow, HandoffRow, WorkerProcessRow};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::from_agent;
use super::parse;
use crate::services::agents::{
    AgentOverview, ClaimSummary, OverviewFilter, PauseState, ProposalSummary, StartWorker,
    WorkerOverview, WorkspaceSummary,
};

pub const GET_ACTIVITY: &str = "get_activity";
pub const LIST_AGENTS: &str = "list_agents";
pub const DEFINE_AREA: &str = "define_area";
pub const UPDATE_AREA: &str = "update_area";
pub const START_WORKER: &str = "start_worker";
pub const STOP_WORKER: &str = "stop_worker";
pub const LIST_WORKERS: &str = "list_workers";
pub const SET_WORKER_LIMITS: &str = "set_worker_limits";
pub const WRITE_HANDOFF: &str = "write_handoff";
pub const PAUSE_PROJECT: &str = "pause_project";
pub const RESUME_PROJECT: &str = "resume_project";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PauseProjectInput {
    /// The worker that makes this call. Leave it out: only the main agent's main worker pauses or resumes.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DefineAreaInput {
    /// The area's name, such as web: 1 to 32 characters of a-z, 0-9, - and _. It becomes part of its workers' names.
    pub name: String,
    /// The task label whose tasks the area owns, such as frontend. Create it with create_label first. A task has at most one area, so a label that shares a task with another area's label is refused (two_areas).
    pub label: String,
    /// What every worker of the area must know about its kind of work, such as "draw screens in Pencil", at most 8000 characters. fold adds it to the system prompt of the area lead and the task workers. Leave it out for none.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The Claude Code model of the area's lead and task workers, such as opus. A start_worker model comes first. Leave it out for the role's model from .fold/config.toml.
    #[serde(default)]
    pub model: Option<String>,
    /// The MCP servers the area's lead and task workers load, by name: fold and servers of the project's .mcp.json, such as ["fold", "pencil"]. Leave it out for the role's servers.
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateAreaInput {
    /// The area's name, such as design.
    pub name: String,
    /// The new instructions for every worker of the area, at most 8000 characters. Leave it out to keep them.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The new Claude Code model of the area's lead and task workers, such as opus. Leave it out to keep it.
    #[serde(default)]
    pub model: Option<String>,
    /// The new MCP server list of the area's lead and task workers, such as ["fold", "pencil"]. Leave it out to keep it.
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
    /// True removes the area's instructions. Do not combine it with instructions.
    #[serde(default)]
    pub clear_instructions: bool,
    /// True removes the area's model, so its workers get the role's model again. Do not combine it with model.
    #[serde(default)]
    pub clear_model: bool,
    /// True removes the area's MCP server list, so its workers get the role's servers again. Do not combine it with mcp.
    #[serde(default)]
    pub clear_mcp: bool,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StartWorkerInput {
    /// area_lead or review_lead (started by the main agent), task_worker (started by an area lead) or reviewer (started by the review lead).
    pub role: String,
    /// The worker's agent name. Leave it out for a name from the role and area, such as web-lead or web-worker-2.
    #[serde(default)]
    pub name: Option<String>,
    /// The area of an area lead. A task worker takes its lead's area; leave it out for the other roles.
    #[serde(default)]
    pub area: Option<String>,
    /// The tasks a task worker works on, such as T-12. Each must carry the area's label; fold gives the worker the claim on each.
    #[serde(default)]
    pub tasks: Vec<String>,
    /// The proposals a reviewer reviews, such as P-3: one proposal, or two or more related proposals, which fold makes into a review batch of the reviewer's.
    #[serde(default)]
    pub proposals: Vec<String>,
    /// For a reviewer's revised proposals: the scope of the review you choose for some of them, each with its reason, when fold's suggested_scope from get_proposal_diff does not fit. The other proposals follow the suggestion, and the reviewer follows the scope it gets.
    #[serde(default)]
    pub scopes: Vec<super::reviews::ScopeChoiceInput>,
    /// The Claude Code model, such as sonnet. Leave it out for the area's model, else the role's model from .fold/config.toml.
    #[serde(default)]
    pub model: Option<String>,
    /// What the worker should know beyond its role and its tasks; fold adds it to the worker's first turn.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StopWorkerInput {
    /// The worker's agent name, such as web-worker.
    pub name: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListWorkersInput {
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetWorkerLimitsInput {
    /// The most task workers that run at once, a whole number with no upper bound; 0 lets none start. Leave it out to keep the limit.
    #[serde(default)]
    pub task_worker: Option<u64>,
    /// The most reviewers that run at once, a whole number with no upper bound; 0 lets none start. Leave it out to keep the limit.
    #[serde(default)]
    pub reviewer: Option<u64>,
    /// The most task workers that run at once in one area, a whole number with no upper bound; 0 lets none start. Leave it out to keep the limit.
    #[serde(default)]
    pub max_per_area: Option<u64>,
    /// True removes the limit of one area, so an area has no limit of its own. Do not combine it with max_per_area.
    #[serde(default)]
    pub clear_max_per_area: bool,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListAgentsInput {
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetActivityInput {
    /// Only the workers of this agent, such as backend. Leave it out for the workers of every agent.
    #[serde(default)]
    pub agent: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WriteHandoffInput {
    /// The hand-off note for your next session, at most 8000 characters: what you are doing, your open decisions and what comes next. Tasks, proposals, reviews and messages stay in fold, so leave them out.
    pub text: String,
    /// The worker that makes this call. Leave it out: only a lead's main worker writes its note.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            GET_ACTIVITY,
            "Show what each worker is doing: its claims, its live workspaces with their changed files and newest checkpoint, and its open proposals. Give agent to see one agent's workers.",
            schema_for_type::<GetActivityInput>(),
        ),
        Tool::new(
            LIST_AGENTS,
            "List every agent of the project in registration order with its tags, presence (active, idle or offline) and workers, each with its last call, claims and live workspaces.",
            schema_for_type::<ListAgentsInput>(),
        ),
        Tool::new(
            DEFINE_AREA,
            "As the main agent, define an area: a part of the project that one area lead owns through one task label. Give it a profile for its kind of work: instructions, a model and MCP servers that its area lead and task workers start with.",
            schema_for_type::<DefineAreaInput>(),
        ),
        Tool::new(
            UPDATE_AREA,
            "As the main agent, change the profile of an area: its instructions, model and MCP servers. The next start of the area's lead or task workers gets the new profile; a running worker keeps what it started with.",
            schema_for_type::<UpdateAreaInput>(),
        ),
        Tool::new(
            START_WORKER,
            "Start a worker agent that fold runs and supervises: the main agent starts an area lead or the review lead, an area lead starts a task worker for tasks of its area, the review lead starts a reviewer for one proposal or a group of related proposals. fold gives the new worker its claims in the same step: the implementation claim on each task, or the review claim on the one proposal, or a review batch of two or more. A task or proposal that another running worker holds refuses the start with already_claimed; a stale claim, a claim of a worker that has ended and a task claim of your own agent move to the new worker. A group with a proposal whose task has the human review mark is refused with invalid_request: give that proposal to a reviewer alone. Refused over the worker limits.",
            schema_for_type::<StartWorkerInput>(),
        ),
        Tool::new(
            STOP_WORKER,
            "Stop a worker that fold runs: its turn is interrupted and its process ends. It keeps its session, so fold can resume it later.",
            schema_for_type::<StopWorkerInput>(),
        ),
        Tool::new(
            LIST_WORKERS,
            "List the areas with their leads and every worker fold runs, with its role, state, bound tasks or proposals, turns, cost, context size and newest hand-off note.",
            schema_for_type::<ListWorkersInput>(),
        ),
        Tool::new(
            SET_WORKER_LIMITS,
            "As the main agent, change how many task workers and reviewers run at once, for example when the user asks for more or fewer agents. Each limit is a whole number with no upper bound, and 0 lets no worker of that role start. The new limits apply to the next start; a running worker is never stopped by a lower limit. Say the new numbers back to the user.",
            schema_for_type::<SetWorkerLimitsInput>(),
        ),
        Tool::new(
            PAUSE_PROJECT,
            "As the main agent, and only when the user asks, pause every agent of the project, for example before the user sleeps or loses power or internet. fold interrupts and stops each worker, keeps its session, starts no worker until resume_project, and freezes the claim leases; approved proposals still integrate and push. Start no work yourself while paused.",
            schema_for_type::<PauseProjectInput>(),
        ),
        Tool::new(
            RESUME_PROJECT,
            "As the main agent, and only when the user asks, end a pause: every unreleased claim gets a full lease, each worker the pause stopped runs again in its own session, and the starts the pause held back run.",
            schema_for_type::<PauseProjectInput>(),
        ),
        Tool::new(
            WRITE_HANDOFF,
            "As an area lead or the review lead, store your hand-off note when fold asks for it; fold then starts you again with a fresh context whose first turn holds the note and the current fold state.",
            schema_for_type::<WriteHandoffInput>(),
        ),
    ]
}

fn area_json(area: &AreaRow) -> Value {
    json!({
        "name": area.name.as_str(),
        "label": area.label.as_str(),
        "lead": area.lead.as_ref().map(AgentName::as_str),
        "instructions": area.profile.instructions,
        "model": area.profile.model,
        "mcp": area.profile.mcp,
    })
}

/// A failed worker also names why it failed.
fn worker_process_json(row: &WorkerProcessRow) -> Value {
    let mut value = json!({
        "name": row.agent.as_str(),
        "role": row.role.as_str(),
        "area": row.area.as_ref().map(AreaName::as_str),
        "state": row.state.as_str(),
        "model": row.model,
        "tasks": row.tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "proposals": row.proposals.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "turns": row.turns,
        "cost_micro_usd": row.cost_micro_usd,
        "context_tokens": row.context_tokens,
    });
    if let Some(reason) = &row.failed_reason {
        value["failed_reason"] = json!(reason);
    }
    value
}

fn ids<T: std::str::FromStr>(texts: &[String], what: &str) -> Result<Vec<T>, FoldError> {
    texts
        .iter()
        .map(|text| {
            text.parse().map_err(|_| {
                FoldError::new(codes::INVALID_ID, format!("{text:?} is not a {what} id"))
            })
        })
        .collect()
}

pub async fn define_area(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: DefineAreaInput = parse(arguments)?;
    let name = AreaName::parse(&input.name)?;
    let label = LabelName::parse(&input.label)?;
    let profile = AreaProfile::from_request(input.instructions, input.model, input.mcp)?;
    let actor = server.actor(as_worker).await?;
    let area = server
        .agents
        .define_area(&actor, &name, &label, &profile)
        .await
        .map_err(from_agent)?;
    Ok(json!({ "area": area_json(&area) }))
}

pub async fn update_area(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: UpdateAreaInput = parse(arguments)?;
    let name = AreaName::parse(&input.name)?;
    let change = AreaProfileChange::from_request(AreaProfileRequest {
        instructions: input.instructions,
        model: input.model,
        mcp: input.mcp,
        clear_instructions: input.clear_instructions,
        clear_model: input.clear_model,
        clear_mcp: input.clear_mcp,
    })?;
    let actor = server.actor(as_worker).await?;
    let area = server
        .agents
        .update_area(&actor, &name, &change)
        .await
        .map_err(from_agent)?;
    Ok(json!({
        "area": area_json(&area),
        "next": "the next start of the area's lead or task workers gets this profile; running workers keep theirs",
    }))
}

pub async fn start_worker(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: StartWorkerInput = parse(arguments)?;
    let role = Role::parse(&input.role).ok_or_else(|| {
        FoldError::new(codes::INVALID_REQUEST, "role is not a worker role")
            .with_hint("pass area_lead, review_lead, task_worker or reviewer")
    })?;
    let request = StartWorker {
        role: Some(role),
        name: input.name.as_deref().map(AgentName::parse).transpose()?,
        area: input.area.as_deref().map(AreaName::parse).transpose()?,
        tasks: ids::<TaskId>(&input.tasks, "task")?,
        proposals: ids::<ProposalId>(&input.proposals, "proposal")?,
        scopes: super::reviews::scope_choices(&input.scopes)?,
        model: input.model,
        instructions: input.instructions,
    };
    let actor = server.actor(as_worker).await?;
    let worker = server
        .agents
        .start_worker(&actor, request)
        .await
        .map_err(from_agent)?;
    Ok(json!({ "worker": worker_process_json(&worker) }))
}

pub async fn stop_worker(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: StopWorkerInput = parse(arguments)?;
    let name = AgentName::parse(&input.name)?;
    let actor = server.actor(as_worker).await?;
    let worker = server
        .agents
        .stop_worker(&actor, &name)
        .await
        .map_err(from_agent)?;
    Ok(json!({ "worker": worker_process_json(&worker) }))
}

fn limits_json(limits: WorkerLimits) -> Value {
    json!({
        "task_worker": limits.task_worker,
        "reviewer": limits.reviewer,
        "max_per_area": limits.max_per_area,
    })
}

pub async fn set_worker_limits(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SetWorkerLimitsInput = parse(arguments)?;
    let change = WorkerLimitsChange::from_request(
        input.task_worker,
        input.reviewer,
        input.max_per_area,
        input.clear_max_per_area,
    )?;
    let actor = server.actor(as_worker).await?;
    let changed = server
        .agents
        .set_worker_limits(&actor, change)
        .await
        .map_err(from_agent)?;
    Ok(json!({ "limits": limits_json(changed.new), "previous": limits_json(changed.old) }))
}

fn pause_json(state: &PauseState) -> Value {
    json!({
        "paused": state.paused_since.is_some(),
        "paused_since": state.paused_since.map(|at| at.as_millis()),
        "paused_by": state.paused_by,
    })
}

pub async fn pause_project(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let _: PauseProjectInput = parse(arguments)?;
    let actor = server.actor(as_worker).await?;
    let state = server
        .agents
        .pause_project(&actor)
        .await
        .map_err(from_agent)?;
    Ok(pause_json(&state))
}

pub async fn resume_project(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let _: PauseProjectInput = parse(arguments)?;
    let actor = server.actor(as_worker).await?;
    let state = server
        .agents
        .resume_project(&actor)
        .await
        .map_err(from_agent)?;
    Ok(pause_json(&state))
}

pub async fn list_workers(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let _: ListWorkersInput = parse(arguments)?;
    let areas = server.agents.list_areas().await.map_err(from_agent)?;
    let workers = server.agents.list_workers().await.map_err(from_agent)?;
    let mut notes = server.agents.latest_handoffs().await.map_err(from_agent)?;
    let config = server.agents.worker_config().await;
    let pause = server.agents.pause_state().await.map_err(from_agent)?;
    let mut listed = Vec::with_capacity(workers.len());
    for worker in &workers {
        let mut value = worker_process_json(worker);
        value["cache_ttl"] = config.as_ref().map_or(Value::Null, |config| {
            json!(config.role(worker.role).cache_ttl.as_str())
        });
        value["handoff"] = notes
            .remove(&worker.agent)
            .map_or(Value::Null, |note| handoff_json(&note));
        listed.push(value);
    }
    Ok(json!({
        "areas": areas.iter().map(area_json).collect::<Vec<_>>(),
        "workers": listed,
        "limits": config.as_ref().map_or(Value::Null, |config| limits_json(config.workers.limits())),
        "pause": pause_json(&pause),
    }))
}

/// How much of a hand-off note `list_workers` shows. The whole note goes to the lead's next session; the list only lets the main agent see what each lead was doing.
const HANDOFF_PREVIEW_CHARS: usize = 300;

fn handoff_json(note: &HandoffRow) -> Value {
    json!({
        "session": note.session_id,
        "created_at": note.created_at,
        "text": shorten(&note.body, HANDOFF_PREVIEW_CHARS),
        "chars": note.body.chars().count(),
    })
}

fn claim_json(claim: &ClaimSummary) -> Value {
    json!({
        "task": claim.task.to_string(),
        "kind": claim.kind.as_str(),
        "acquired_at": claim.acquired_at,
        "renewed_at": claim.renewed_at,
        "stale": claim.stale,
    })
}

fn workspace_json(workspace: &WorkspaceSummary) -> Value {
    json!({
        "ws": workspace.ws.to_string(),
        "task": workspace.task.to_string(),
        "state": workspace.state.as_str(),
        "base": workspace.base.to_string(),
        "changed_files": workspace.changed_files,
        "last_activity_at": workspace.last_activity_at,
        "latest_checkpoint": workspace.latest_checkpoint.map(|id| id.to_string()),
    })
}

fn proposal_json(proposal: &ProposalSummary) -> Value {
    json!({
        "id": proposal.id.to_string(),
        "task": proposal.task.to_string(),
        "title": proposal.title,
        "conflicted": proposal.conflicted,
    })
}

fn worker_json(worker: &WorkerOverview) -> Value {
    json!({
        "name": worker.name.as_str(),
        "parent": worker.parent.as_ref().map(WorkerName::as_str),
        "last_call_at": worker.last_call_at,
        "activity": worker.activity,
        "claims": worker.claims.iter().map(claim_json).collect::<Vec<_>>(),
        "workspaces": worker.workspaces.iter().map(workspace_json).collect::<Vec<_>>(),
    })
}

fn agent_json(agent: &AgentOverview) -> Value {
    json!({
        "name": agent.name.as_str(),
        "tags": agent.tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
        "presence": agent.presence.as_str(),
        "last_call_at": agent.last_call_at,
        "workers": agent.workers.iter().map(worker_json).collect::<Vec<_>>(),
    })
}

fn activity_json(worker: &WorkerOverview) -> Value {
    json!({
        "worker": worker.name.as_str(),
        "claims": worker.claims.iter().map(claim_json).collect::<Vec<_>>(),
        "workspaces": worker.workspaces.iter().map(workspace_json).collect::<Vec<_>>(),
        "proposals": worker.proposals.iter().map(proposal_json).collect::<Vec<_>>(),
    })
}

pub async fn list_agents(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let _: ListAgentsInput = parse(arguments)?;
    let agents = server
        .agents
        .overview(&OverviewFilter::default())
        .await
        .map_err(from_agent)?;
    Ok(json!({ "agents": agents.iter().map(agent_json).collect::<Vec<_>>() }))
}

pub async fn get_activity(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let input: GetActivityInput = parse(arguments)?;
    let filter = OverviewFilter {
        agent: input.agent.as_deref().map(AgentName::parse).transpose()?,
        events: 0,
    };
    let agents = server.agents.overview(&filter).await.map_err(from_agent)?;
    let workers: Vec<Value> = agents
        .iter()
        .flat_map(|agent| agent.workers.iter().map(activity_json))
        .collect();
    Ok(json!({ "workers": workers }))
}

pub async fn write_handoff(
    server: &FoldServer,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: WriteHandoffInput = parse(arguments)?;
    let actor = server.actor(as_worker).await?;
    server
        .agents
        .write_handoff(&actor, &input.text)
        .await
        .map_err(from_agent)?;
    Ok(json!({
        "stored": true,
        "next": "end your turn; fold starts you again with a fresh context that reads this note",
    }))
}

#[cfg(test)]
mod tests {
    use fold_core::Timestamp;

    use super::*;

    #[test]
    fn a_listed_hand_off_note_shows_its_start_and_its_length() {
        let note = HandoffRow {
            session_id: "s1".to_owned(),
            body: "é".repeat(HANDOFF_PREVIEW_CHARS + 50),
            created_at: Timestamp::from_millis(5).unwrap(),
        };
        let shown = handoff_json(&note);
        let text = shown["text"].as_str().unwrap();
        assert_eq!(text.chars().count(), HANDOFF_PREVIEW_CHARS);
        assert!(text.ends_with('…'));
        assert_eq!(shown["chars"], HANDOFF_PREVIEW_CHARS + 50);
        assert_eq!(shown["session"], "s1");
    }
}

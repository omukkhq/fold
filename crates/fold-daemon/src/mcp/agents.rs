use fold_core::{AgentName, FoldError, Tag, WorkerName};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::from_agent;
use super::identity::parse;
use crate::services::agents::{
    AgentOverview, ClaimSummary, OverviewFilter, ProposalSummary, WorkerOverview, WorkspaceSummary,
};

pub const GET_ACTIVITY: &str = "get_activity";
pub const LIST_AGENTS: &str = "list_agents";

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
    ]
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

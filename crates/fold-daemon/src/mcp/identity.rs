use fold_core::{AgentName, FoldError, Tag, WorkerName};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::from_agent;

pub const REGISTER_AGENT: &str = "register_agent";
pub const REGISTER_WORKER: &str = "register_worker";
pub const LIST_AGENTS: &str = "list_agents";

/// The tools a session may call before it has an agent.
pub fn is_identity_tool(name: &str) -> bool {
    matches!(name, REGISTER_AGENT | REGISTER_WORKER | LIST_AGENTS)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RegisterAgentInput {
    /// Your agent name: 1 to 32 characters of a-z, 0-9, - and _, starting with a letter or digit. The first call creates the agent; later calls reuse it.
    pub name: String,
    /// Role or capability tags to add to the agent, such as frontend or review. Tags are only added, never removed.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RegisterWorkerInput {
    /// The last segment of the new worker's name: register_worker with name ui gives frontend/ui. A repeat call returns the existing worker.
    pub name: String,
    /// The full name of the parent worker, such as frontend/ui. Leave it out to register under your main worker.
    #[serde(default)]
    pub parent: Option<String>,
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

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            REGISTER_AGENT,
            "Register this session as an agent. Call it once at the start of every session; every tool except register_agent, register_worker and list_agents needs it.",
            schema_for_type::<RegisterAgentInput>(),
        ),
        Tool::new(
            REGISTER_WORKER,
            "Register a child worker of your agent, for a subagent that works under your name.",
            schema_for_type::<RegisterWorkerInput>(),
        ),
        Tool::new(
            LIST_AGENTS,
            "List every agent of the project with its tags and workers.",
            schema_for_type::<ListAgentsInput>(),
        ),
    ]
}

fn parse<T: for<'de> Deserialize<'de>>(arguments: &JsonObject) -> Result<T, FoldError> {
    serde_json::from_value(Value::Object(arguments.clone())).map_err(|_| {
        FoldError::new(
            fold_core::codes::INVALID_REQUEST,
            "the arguments do not match the tool's input schema",
        )
    })
}

pub async fn register_agent(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RegisterAgentInput = parse(arguments)?;
    let name = AgentName::parse(&input.name)?;
    let tags = input
        .tags
        .iter()
        .map(|tag| Tag::parse(tag))
        .collect::<Result<Vec<_>, _>>()?;
    let mut session = server.session.lock().await;
    // An unbound session resolves `as` only once this call has bound it, so a worker of another agent is refused before anything is written.
    if session.agent.is_none()
        && let Some(worker) = input.as_worker.as_deref()
        && WorkerName::parse(worker)?.agent() != name
    {
        return Err(FoldError::new(
            fold_core::codes::WORKER_NOT_FOUND,
            "the worker does not exist",
        ));
    }
    let registered = server
        .agents
        .register_agent(&mut session, &name, &tags)
        .await
        .map_err(from_agent)?;
    Ok(json!({
        "agent": registered.agent.as_str(),
        "tags": registered.tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
        "main_worker": registered.main_worker.as_str(),
        "created": registered.created,
    }))
}

pub async fn register_worker(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RegisterWorkerInput = parse(arguments)?;
    let parent = input.parent.as_deref().map(WorkerName::parse).transpose()?;
    let session = server.session().await;
    let registered = server
        .agents
        .register_worker(&session, &input.name, parent.as_ref())
        .await
        .map_err(from_agent)?;
    let worker = registered.worker.name;
    Ok(json!({
        "worker": worker.as_str(),
        "parent": worker.parent().as_ref().map(WorkerName::as_str),
    }))
}

pub async fn list_agents(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let _: ListAgentsInput = parse(arguments)?;
    let agents = server.agents.list_agents().await.map_err(from_agent)?;
    let agents: Vec<Value> = agents
        .iter()
        .map(|summary| {
            json!({
                "agent": summary.agent.as_str(),
                "tags": summary.tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
                "workers": summary.workers.iter().map(WorkerName::as_str).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "agents": agents }))
}

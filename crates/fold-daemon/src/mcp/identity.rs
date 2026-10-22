use fold_core::{AgentName, FoldError, Tag};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::error::from_agent;
use super::{FoldServer, agents, parse};

pub const REGISTER_AGENT: &str = "register_agent";

/// The tools a session may call before it has an agent.
pub fn is_identity_tool(name: &str) -> bool {
    matches!(name, REGISTER_AGENT | agents::LIST_AGENTS)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RegisterAgentInput {
    /// Your agent name: 1 to 32 characters of a-z, 0-9, - and _, starting with a letter or digit. The first call creates the agent; later calls reuse it.
    pub name: String,
    /// Role or capability tags to add to the agent, such as frontend or review. Tags are only added, never removed. The main tag is refused with main_tag_denied: only the session that fld claude starts gets it, from its handshake.
    #[serde(default)]
    pub tags: Vec<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        REGISTER_AGENT,
        "Register this session as an agent. Call it once at the start of every session; every tool except register_agent and list_agents needs it.",
        schema_for_type::<RegisterAgentInput>(),
    )]
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
    let registered = server
        .agents
        .register_agent_from_tool(&mut session, &name, &tags)
        .await
        .map_err(from_agent)?;
    Ok(json!({
        "agent": registered.agent.as_str(),
        "tags": registered.tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
        "main_worker": registered.main_worker.as_str(),
        "created": registered.created,
    }))
}

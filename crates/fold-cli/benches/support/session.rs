use std::path::Path;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::process::Child;

use super::host::Host;
use super::{BenchResult, fail};

/// One `fld mcp` adapter driven by an rmcp client, which is one agent session.
pub struct Session {
    client: RunningService<RoleClient, ()>,
    child: Child,
}

impl Session {
    /// Starts the adapter in `cwd` as agent `agent` and registers the agent.
    pub async fn open(host: &Host, cwd: &Path, agent: &str) -> BenchResult<Self> {
        let mut command = host.fold(&["mcp"], cwd);
        command.env("FOLD_AGENT", agent);
        let mut child = command.spawn()?;
        let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
            return fail("the adapter has no standard streams");
        };
        let client = ().serve((stdout, stdin)).await?;
        let session = Self { client, child };
        session
            .call("register_agent", json!({ "name": agent }))
            .await?;
        Ok(session)
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> BenchResult<Value> {
        let Value::Object(arguments) = arguments else {
            return fail("tool arguments must be an object");
        };
        let result = self
            .client
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
            .await?;
        let Some(text) = result.content.first().and_then(|content| content.as_text()) else {
            return fail(format!("{tool} returned no text"));
        };
        if result.is_error == Some(true) {
            return fail(format!("{tool} failed: {}", text.text));
        }
        Ok(serde_json::from_str(&text.text)?)
    }

    /// Creates a task and returns its public id, such as `T-4`.
    pub async fn create_task(&self, title: &str) -> BenchResult<String> {
        let task = self.call("create_task", json!({ "title": title })).await?;
        match task["id"].as_str() {
            Some(id) => Ok(id.to_owned()),
            None => fail("create_task returned no id"),
        }
    }

    /// Creates the task workspace and returns its public id and its path relative to the project root.
    pub async fn create_workspace(&self, task: &str) -> BenchResult<(String, String)> {
        let created = self
            .call("create_workspace", json!({ "task": task }))
            .await?;
        match (created["workspace"].as_str(), created["path"].as_str()) {
            (Some(id), Some(path)) => Ok((id.to_owned(), path.to_owned())),
            _ => fail("create_workspace returned no workspace"),
        }
    }

    /// Ends the session by closing the adapter's standard input.
    pub async fn close(self) {
        let Self { client, mut child } = self;
        let _ = client.cancel().await;
        let _ = child.wait().await;
    }
}

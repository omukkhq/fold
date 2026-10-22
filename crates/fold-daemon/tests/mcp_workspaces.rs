#![cfg(test)]

use crate::workspaces_support;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::workspaces_support::{service, three_revisions};
use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::messages::MessageService;
use fold_daemon::services::tasks::TaskService;
use fold_store::testing::TestProject;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ()>;

struct Fixed(FooterPart, u64);

impl FooterSource for Fixed {
    fn part(&self) -> FooterPart {
        self.0
    }

    fn count<'a>(
        &'a self,
        _session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async move { self.1 })
    }
}

fn tasks(project: &TestProject) -> TaskService {
    TaskService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
        30,
    )
}

fn messages(project: &TestProject) -> MessageService {
    MessageService::new(
        project.store().clone(),
        project.registry().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
    )
}

fn events(project: &TestProject) -> EventService {
    EventService::new(project.store().clone(), EventBus::new())
}

const FOOTER: &str = "fold: 2 unread";

/// A registered agent's session over an in-memory stream, with a footer count so every result carries the footer line.
async fn connect(project: &TestProject) -> Client {
    let agents = AgentService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
    );
    let session = agents.start_session(7).await.unwrap();
    let server = FoldServer::new(
        agents,
        events(project),
        service(project),
        tasks(project),
        messages(project),
        session,
    )
    .with_footer_source(Arc::new(Fixed(FooterPart::Unread, 2)));
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_end).await.unwrap();
    let registered = call_raw(&client, "register_agent", json!({"name": "review"})).await;
    assert_eq!(registered.is_error, Some(false));
    client
}

async fn call_raw(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

/// The JSON body of a result, after checking that its last line is the footer.
async fn call(client: &Client, tool: &str, arguments: Value, ok: bool) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(!ok), "{tool}: {result:?}");
    assert_eq!(result.content.len(), 2, "{tool} has no footer");
    assert_eq!(result.content[1].as_text().unwrap().text, FOOTER);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn the_repository_state_names_the_head_and_the_recent_revisions() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let client = connect(&project).await;
    let state = call(&client, "get_repository_state", json!({}), true).await;
    assert_eq!(state["head"], "r3");
    assert_eq!(
        state["main_path"],
        project.paths().root().to_string_lossy().as_ref()
    );
    assert_eq!(
        state["recent"],
        json!([
            {"r": "r3", "created_at": 1003, "task": null, "proposal": null, "title": null},
            {"r": "r2", "created_at": 1002, "task": "T-3", "proposal": "P-5", "title": "Add the guide"},
            {"r": "r1", "created_at": 1000, "task": null, "proposal": null, "title": null},
        ])
    );
}

#[tokio::test]
async fn a_revision_lists_its_files_and_an_unknown_one_is_refused() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let client = connect(&project).await;
    let r2 = call(&client, "get_revision", json!({"r": "r2"}), true).await;
    assert_eq!(r2["r"], "r2");
    assert_eq!(r2["parent"], "r1");
    assert_eq!(r2["task"], "T-3");
    assert_eq!(r2["proposal"], "P-5");
    assert_eq!(r2["tree"].as_str().unwrap().len(), 64);
    assert_eq!(
        r2["files"],
        json!([
            {"path": "docs/guide.md", "change": "added", "added": 1, "removed": 0},
            {"path": "src/lib.rs", "change": "modified", "added": 1, "removed": 0},
        ])
    );
    assert_eq!(r2["totals"], json!({"files": 2, "added": 2, "removed": 0}));
    let r1 = call(&client, "get_revision", json!({"r": "r1"}), true).await;
    assert_eq!(r1["parent"], Value::Null);
    assert_eq!(
        r1["files"][1],
        json!({"path": "logo.png", "change": "added", "added": 0, "removed": 0, "binary": true})
    );

    let error = call(&client, "get_revision", json!({"r": "r9"}), false).await;
    assert_eq!(error["error"]["code"], "revision_not_found");
    assert_eq!(
        error["error"]["hint"],
        "call get_repository_state for the head and the recent revisions"
    );
    let error = call(&client, "get_revision", json!({"r": "T-1"}), false).await;
    assert_eq!(error["error"]["code"], "invalid_id");
}

#[tokio::test]
async fn compare_revisions_lists_files_or_gives_one_file_s_hunks() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let client = connect(&project).await;
    let listed = call(
        &client,
        "compare_revisions",
        json!({"from": "r1", "to": "r3"}),
        true,
    )
    .await;
    assert_eq!(listed["from"], json!({"id": "r1", "kind": "revision"}));
    assert_eq!(
        listed["files"],
        json!([
            {"path": "README.md", "change": "deleted", "added": 0, "removed": 1},
            {"path": "docs/guide.md", "change": "added", "added": 1, "removed": 0},
            {"path": "src/lib.rs", "change": "modified", "added": 1, "removed": 0},
            {"path": "src/main.rs", "change": "added", "added": 1, "removed": 0},
        ])
    );
    assert_eq!(
        listed["totals"],
        json!({"files": 4, "added": 3, "removed": 1})
    );

    let hunks = call(
        &client,
        "compare_revisions",
        json!({"from": "r1", "to": "r3", "path": "src/lib.rs"}),
        true,
    )
    .await;
    assert_eq!(
        hunks,
        json!({
            "path": "src/lib.rs",
            "change": "modified",
            "binary": false,
            "hunks": [{
                "header": "@@ -1,1 +1,2 @@",
                "lines": [" 1 fn a() {}\n", "+2 fn b() {}\n"],
            }],
        })
    );

    let error = call(
        &client,
        "compare_revisions",
        json!({"from": "r1", "to": "r3", "path": "logo.png"}),
        false,
    )
    .await;
    assert_eq!(error["error"]["code"], "path_not_in_comparison");
    assert_eq!(
        error["error"]["hint"],
        "call compare_revisions without path to list the changed files"
    );
    let error = call(
        &client,
        "compare_revisions",
        json!({"from": "main", "to": "r3"}),
        false,
    )
    .await;
    assert_eq!(error["error"]["code"], "invalid_state_ref");
    let error = call(
        &client,
        "compare_revisions",
        json!({"from": "r1", "to": "r7"}),
        false,
    )
    .await;
    assert_eq!(error["error"]["code"], "revision_not_found");
    let error = call(&client, "compare_revisions", json!({"from": "r1"}), false).await;
    assert_eq!(error["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn a_working_side_carries_its_workspace_and_a_rename_its_old_path() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    workspaces_support::workspace_dir(
        &project,
        &[
            ("README.md", b"hello\n"),
            ("logo.png", b"\x89PNG\0\x01"),
            ("src/core.rs", b"fn a() {}\n"),
        ],
    );
    let client = connect(&project).await;
    let compared = call(
        &client,
        "compare_revisions",
        json!({"from": "r1", "to": "ws-1"}),
        true,
    )
    .await;
    assert_eq!(
        compared["to"],
        json!({"id": "ws-1", "kind": "workspace", "base": "r1", "task": "T-3", "worker": "backend/api"})
    );
    assert_eq!(
        compared["files"],
        json!([{"path": "src/core.rs", "change": "renamed", "added": 0, "removed": 0, "old_path": "src/lib.rs"}])
    );
    let renamed = call(
        &client,
        "compare_revisions",
        json!({"from": "r1", "to": "ws-1", "path": "src/core.rs"}),
        true,
    )
    .await;
    assert_eq!(renamed["old_path"], "src/lib.rs");
    assert_eq!(renamed["hunks"], json!([]));
}

#[tokio::test]
async fn repository_tools_need_a_registered_agent() {
    let project = TestProject::new().await;
    let agents = AgentService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
    );
    let session = agents.start_session(8).await.unwrap();
    let server = FoldServer::new(
        agents,
        events(&project),
        service(&project),
        tasks(&project),
        messages(&project),
        session,
    );
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_end).await.unwrap();
    let result = call_raw(&client, "get_repository_state", json!({})).await;
    assert_eq!(result.is_error, Some(true));
    let body: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(body["error"]["code"], "agent_not_registered");
}

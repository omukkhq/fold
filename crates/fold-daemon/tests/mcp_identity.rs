use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use fold_core::WorkerName;
use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource, INSTRUCTIONS};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::agents;
use fold_store::testing::TestProject;
use fold_vcs::git::Git;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

#[cfg(test)]
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

#[cfg(test)]
fn service(project: &TestProject) -> AgentService {
    AgentService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
    )
}

/// One adapter session served over an in-memory stream, as the handshake serves it after its answer line.
#[cfg(test)]
async fn connect_with(
    project: &TestProject,
    sources: Vec<Arc<dyn FooterSource>>,
) -> (Client, SessionContext) {
    let agents = service(project);
    let events = EventService::new(project.store().clone(), EventBus::new());
    let session = agents.start_session(7).await.unwrap();
    let workspaces = WorkspaceService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
        project.paths().clone(),
        Git::default(),
        30,
    );
    let mut server = FoldServer::new(agents, events, workspaces, session.clone());
    for source in sources {
        server = server.with_footer_source(source);
    }
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    (().serve(client_end).await.unwrap(), session)
}

#[cfg(test)]
async fn connect(project: &TestProject) -> Client {
    connect_with(project, Vec::new()).await.0
}

#[cfg(test)]
async fn call_raw(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

#[cfg(test)]
fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// The JSON of a successful result that has no footer line.
#[cfg(test)]
async fn call(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    assert_eq!(result.content.len(), 1, "no footer without counts");
    body(&result)
}

/// The `error` object of a failed result.
#[cfg(test)]
async fn call_err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    body(&result)["error"].clone()
}

#[tokio::test]
async fn register_agent_creates_the_agent_once_and_merges_tags() {
    let project = TestProject::new().await;
    let client = connect(&project).await;
    assert_eq!(
        call(
            &client,
            "register_agent",
            json!({"name": "frontend", "tags": ["ui"]})
        )
        .await,
        json!({"agent": "frontend", "tags": ["ui"], "main_worker": "frontend", "created": true})
    );
    assert_eq!(
        call(
            &client,
            "register_agent",
            json!({"name": "frontend", "tags": ["web"]})
        )
        .await,
        json!({"agent": "frontend", "tags": ["ui", "web"], "main_worker": "frontend", "created": false})
    );
    let error = call_err(&client, "register_agent", json!({"name": "backend"})).await;
    assert_eq!(error["code"], "agent_already_bound");
}

#[tokio::test]
async fn register_worker_names_the_worker_and_its_parent() {
    let project = TestProject::new().await;
    let client = connect(&project).await;
    call(&client, "register_agent", json!({"name": "frontend"})).await;
    assert_eq!(
        call(&client, "register_worker", json!({"name": "ui"})).await,
        json!({"worker": "frontend/ui", "parent": "frontend"})
    );
    assert_eq!(
        call(
            &client,
            "register_worker",
            json!({"name": "forms", "parent": "frontend/ui"})
        )
        .await,
        json!({"worker": "frontend/ui/forms", "parent": "frontend/ui"})
    );
    assert_eq!(
        call(&client, "register_worker", json!({"name": "ui"})).await,
        json!({"worker": "frontend/ui", "parent": "frontend"})
    );
    let error = call_err(
        &client,
        "register_worker",
        json!({"name": "x", "parent": "frontend/none"}),
    )
    .await;
    assert_eq!(error["code"], "worker_not_found");
    assert_eq!(error["hint"], "call register_worker");
}

#[tokio::test]
async fn list_agents_shows_each_agent_with_its_tags_and_workers_in_registration_order() {
    let project = TestProject::new().await;
    let reader = connect(&project).await;
    assert_eq!(
        call(&reader, "list_agents", json!({})).await,
        json!({"agents": []})
    );
    let frontend = connect(&project).await;
    call(
        &frontend,
        "register_agent",
        json!({"name": "frontend", "tags": ["web", "ui"]}),
    )
    .await;
    call(&frontend, "register_worker", json!({"name": "a"})).await;
    let backend = connect(&project).await;
    call(&backend, "register_agent", json!({"name": "backend"})).await;
    let listed = call(&reader, "list_agents", json!({})).await;
    let summary: Vec<Value> = listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| {
            let workers: Vec<&Value> = agent["workers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|worker| &worker["name"])
                .collect();
            json!({"name": agent["name"], "tags": agent["tags"], "workers": workers})
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            json!({"name": "frontend", "tags": ["ui", "web"], "workers": ["frontend", "frontend/a"]}),
            json!({"name": "backend", "tags": [], "workers": ["backend"]}),
        ],
        "registration order, the main worker first"
    );
}

#[tokio::test]
async fn any_other_tool_before_registration_names_register_agent() {
    let project = TestProject::new().await;
    let client = connect(&project).await;
    for tool in ["register_worker", "claim_task", "send_message"] {
        let error = call_err(&client, tool, json!({"name": "ui"})).await;
        assert_eq!(error["code"], "agent_not_registered", "{tool}");
        assert_eq!(error["hint"], "call register_agent", "{tool}");
    }
    let error = call_err(&client, "list_agents", json!({"as": "frontend"})).await;
    assert_eq!(error["code"], "agent_not_registered");

    call(&client, "register_agent", json!({"name": "frontend"})).await;
    let unknown = client
        .call_tool(CallToolRequestParams::new("claim_task"))
        .await;
    assert!(unknown.is_err(), "an unknown tool is a protocol error");
}

#[tokio::test]
async fn as_resolves_only_the_callers_own_workers_and_records_the_call() {
    let project = TestProject::new().await;
    let frontend = connect(&project).await;
    call(&frontend, "register_agent", json!({"name": "frontend"})).await;
    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let backend = connect(&project).await;
    call(&backend, "register_agent", json!({"name": "backend"})).await;

    for worker in ["frontend/ui", "frontend", "backend/none"] {
        let error = call_err(&backend, "list_agents", json!({"as": worker})).await;
        assert_eq!(error["code"], "worker_not_found", "{worker}");
        assert_eq!(error["hint"], "call register_worker", "{worker}");
    }

    call(&frontend, "list_agents", json!({"as": "frontend/ui"})).await;
    let store = project.store();
    let agent = agents::find_agent(
        store.read(),
        &fold_core::AgentName::parse("frontend").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let ui = agents::find_worker(
        store.read(),
        agent.id,
        &WorkerName::parse("frontend/ui").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ui.activity.as_deref(), Some("list_agents"));
    assert_eq!(ui.last_call_at, Some(project.now()));
    let main = agents::main_worker(store.read(), agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(main.activity.as_deref(), Some("register_worker"));

    let unbound = connect(&project).await;
    let error = call_err(
        &unbound,
        "register_agent",
        json!({"name": "review", "as": "frontend/ui"}),
    )
    .await;
    assert_eq!(error["code"], "worker_not_found");
    assert_eq!(error["hint"], "call register_worker");
    let error = call_err(&unbound, "claim_task", json!({})).await;
    assert_eq!(
        error["code"], "agent_not_registered",
        "the session stays unbound"
    );
    assert!(
        agents::find_agent(
            store.read(),
            &fold_core::AgentName::parse("review").unwrap()
        )
        .await
        .unwrap()
        .is_none(),
        "the refused call created no agent"
    );

    let registered = connect(&project).await;
    call(
        &registered,
        "register_agent",
        json!({"name": "review", "as": "review"}),
    )
    .await;
    let error = call_err(
        &registered,
        "register_agent",
        json!({"name": "review", "as": "review/none"}),
    )
    .await;
    assert_eq!(error["code"], "worker_not_found");
}

#[tokio::test]
async fn bad_input_is_refused_with_a_code() {
    let project = TestProject::new().await;
    let client = connect(&project).await;
    let cases = [
        ("register_agent", json!({"name": 5}), "invalid_request"),
        (
            "register_agent",
            json!({"name": "Bad Name"}),
            "invalid_name",
        ),
        (
            "register_agent",
            json!({"name": "a", "tags": ["No"]}),
            "invalid_name",
        ),
        (
            "register_agent",
            json!({"name": "a", "as": 5}),
            "invalid_request",
        ),
        (
            "register_agent",
            json!({"name": "a", "as": "a//b"}),
            "invalid_name",
        ),
        (
            "register_agent",
            json!({"name": "a", "tags": ["integrator"]}),
            "human_required",
        ),
    ];
    for (tool, arguments, code) in cases {
        let error = call_err(&client, tool, arguments.clone()).await;
        assert_eq!(error["code"], code, "{arguments}");
    }
    call(&client, "register_agent", json!({"name": "a"})).await;
    let error = call_err(&client, "register_worker", json!({"name": "a/b"})).await;
    assert_eq!(error["code"], "invalid_name");
    let error = call_err(
        &client,
        "register_worker",
        json!({"name": "b", "parent": "B"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_name");
    let error = call_err(&client, "list_agents", json!({"as": []})).await;
    assert_eq!(error["code"], "invalid_request");
}

#[tokio::test]
async fn every_result_ends_with_the_footer_line_when_a_count_is_not_zero() {
    let project = TestProject::new().await;
    let sources: Vec<Arc<dyn FooterSource>> = vec![
        Arc::new(Fixed(FooterPart::ClaimsNearStale, 1)),
        Arc::new(Fixed(FooterPart::Unread, 2)),
        Arc::new(Fixed(FooterPart::NewEvents, 0)),
    ];
    let (client, _) = connect_with(&project, sources).await;
    let calls = [
        ("list_agents", json!({})),
        ("register_worker", json!({"name": "ui"})),
        ("claim_task", json!({})),
        ("register_agent", json!({"name": "frontend"})),
        ("register_worker", json!({"name": "ui"})),
        ("list_agents", json!({"as": "frontend/none"})),
    ];
    for (tool, arguments) in calls {
        let result = call_raw(&client, tool, arguments).await;
        assert_eq!(result.content.len(), 2, "{tool}: {result:?}");
        assert_eq!(
            result.content[1].as_text().unwrap().text,
            "fold: 2 unread · 1 claims near stale",
            "{tool}"
        );
    }
}

#[tokio::test]
async fn the_server_lists_described_tools_and_sends_the_workflow() {
    let project = TestProject::new().await;
    let client = connect(&project).await;
    let info = client.peer_info().unwrap();
    assert_eq!(info.instructions.as_deref(), Some(INSTRUCTIONS));
    assert!(INSTRUCTIONS.contains("register_agent"));
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "register_agent",
            "register_worker",
            "get_activity",
            "list_agents",
            "get_events",
            "wait_for_events",
            "get_repository_state",
            "get_revision",
            "compare_revisions",
            "create_workspace",
            "get_workspace",
            "record_checkpoint",
            "list_checkpoints",
            "restore_checkpoint",
            "record_changes",
            "sync_workspace",
            "abandon_workspace",
        ]
    );
    for tool in &tools {
        assert!(tool.description.is_some(), "{}", tool.name);
        let properties = tool.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("as"), "{}", tool.name);
        for (field, schema) in properties {
            assert!(
                schema["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{}.{field} has no description",
                tool.name
            );
        }
    }
}

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource, INSTRUCTIONS};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::messages::MessageService;
use fold_daemon::services::tasks::TaskService;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::testing::TestProject;
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
        30,
    );
    let tasks = TaskService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
        30,
    );
    let messages = MessageService::new(
        project.store().clone(),
        project.registry().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
    );
    let mut server = FoldServer::new(agents, events, workspaces, tasks, messages, session.clone());
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
            json!({"name": "frontend", "tags": ["ui", "web"], "workers": ["frontend"]}),
            json!({"name": "backend", "tags": [], "workers": ["backend"]}),
        ],
        "registration order, one worker each"
    );
}

#[tokio::test]
async fn any_other_tool_before_registration_names_register_agent() {
    let project = TestProject::new().await;
    let client = connect(&project).await;
    for tool in ["claim_task", "send_message"] {
        let error = call_err(&client, tool, json!({"name": "ui"})).await;
        assert_eq!(error["code"], "agent_not_registered", "{tool}");
        assert_eq!(error["hint"], "call register_agent", "{tool}");
    }

    call(&client, "register_agent", json!({"name": "frontend"})).await;
    let unknown = client
        .call_tool(CallToolRequestParams::new("no_such_tool"))
        .await;
    let Err(rmcp::ServiceError::McpError(error)) = unknown else {
        panic!("an unknown tool is a protocol error: {unknown:?}");
    };
    assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(!error.message.contains("no_such_tool"));
    assert_eq!(
        error.data,
        Some(json!({"hint": "call tools/list to see the tools of this server"}))
    );
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
    ];
    for (tool, arguments, code) in cases {
        let error = call_err(&client, tool, arguments.clone()).await;
        assert_eq!(error["code"], code, "{arguments}");
    }
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
        ("claim_task", json!({})),
        ("register_agent", json!({"name": "frontend"})),
        ("list_agents", json!({})),
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
    assert!(INSTRUCTIONS.contains("fld claude"));
    assert!(INSTRUCTIONS.contains("start_worker"));
    let review_rule = "work that the task still misses back with request_changes, describe new work outside the task with create_request (the proposal can still be approved)";
    let ask_rule = "ask the humans with asks_human for a product decision or a conflict with a settled decision";
    let review_tools = fold_daemon::mcp::reviews::tools();
    let fix = review_tools
        .iter()
        .find(|tool| tool.name == "submit_review_fix")
        .and_then(|tool| tool.description.as_deref())
        .unwrap();
    for text in [INSTRUCTIONS, fix] {
        assert!(text.contains(review_rule), "{text}");
        assert!(text.contains(ask_rule), "{text}");
    }
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "register_agent",
            "get_activity",
            "list_agents",
            "define_area",
            "update_area",
            "start_worker",
            "stop_worker",
            "list_workers",
            "set_worker_limits",
            "pause_project",
            "resume_project",
            "write_handoff",
            "restart_worker",
            "get_events",
            "wait_for_events",
            "get_repository_state",
            "get_revision",
            "compare_revisions",
            "create_workspace",
            "get_workspace",
            "get_workspace_changes",
            "record_checkpoint",
            "list_checkpoints",
            "restore_checkpoint",
            "record_changes",
            "sync_workspace",
            "abandon_workspace",
            "open_main_workspace",
            "add_exclude_patterns",
            "get_task_graph",
            "get_ready_tasks",
            "get_task",
            "create_task",
            "create_task_plan",
            "update_task",
            "delete_task",
            "archive_task",
            "add_dependency",
            "remove_dependency",
            "add_checklist_item",
            "update_checklist_item",
            "move_checklist_item",
            "remove_checklist_item",
            "create_label",
            "claim_task",
            "release_task",
            "takeover_claim",
            "report_task",
            "complete_task",
            "send_back_task",
            "set_blocker",
            "clear_blocker",
            "create_request",
            "list_requests",
            "get_request",
            "claim_request",
            "release_request",
            "submit_request_draft",
            "request_draft_changes",
            "edit_request_draft",
            "approve_request_draft",
            "close_request",
            "send_message",
            "get_inbox",
            "get_thread",
            "acknowledge_message",
            "mark_answered",
            "list_open_asks"
        ]
    );
    for tool in &tools {
        assert!(tool.description.is_some(), "{}", tool.name);
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        assert!(!properties.contains_key("as"), "{}", tool.name);
        for (field, schema) in &properties {
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

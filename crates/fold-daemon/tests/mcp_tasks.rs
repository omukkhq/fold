use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::messages::MessageService;
use fold_daemon::services::tasks::TaskService;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::testing::{TestProject, execute};
use fold_vcs::git::Git;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const LEASE: u64 = 30;
/// The fixed count always leads the footer line; a real `new events` count can follow it.
const FOOTER: &str = "fold: 1 unread";

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

/// A footer source that always counts one, so every result must end with the footer line.
#[cfg(test)]
struct One;

impl FooterSource for One {
    fn part(&self) -> FooterPart {
        FooterPart::Unread
    }

    fn count<'a>(
        &'a self,
        _session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async { 1 })
    }
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    agents: AgentService,
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let bus = EventBus::new();
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock, LEASE);
        Self {
            project,
            bus,
            agents,
            tasks,
        }
    }

    /// An adapter session served over an in-memory stream and registered as `name`.
    async fn connect(&self, name: &str) -> Client {
        let events = EventService::new(self.project.store().clone(), self.bus.clone());
        let session = self.agents.start_session(7).await.unwrap();
        let workspaces = WorkspaceService::new(
            self.project.store().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
            self.project.paths().clone(),
            Git::default(),
            LEASE,
        );
        let server = FoldServer::new(
            self.agents.clone(),
            events,
            workspaces,
            self.tasks.clone(),
            MessageService::new(
                self.project.store().clone(),
                self.project.registry().clone(),
                self.bus.clone(),
                Arc::new(self.project.clock().clone()),
            ),
            session,
        )
        .with_footer_source(Arc::new(One));
        let (client_end, server_end) = tokio::io::duplex(256 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });
        let client = ().serve(client_end).await.unwrap();
        ok(&client, "register_agent", json!({"name": name})).await;
        client
    }
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

/// The JSON of a successful result, which must end with the footer line.
#[cfg(test)]
async fn ok(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    assert_eq!(result.content.len(), 2, "{tool}: {result:?}");
    assert_footer(&result, tool);
    body(&result)
}

#[cfg(test)]
fn assert_footer(result: &CallToolResult, tool: &str) {
    let line = &result.content[1].as_text().unwrap().text;
    assert!(line.starts_with(FOOTER), "{tool}: {line}");
}

/// The `error` object of a failed result, which also ends with the footer line.
#[cfg(test)]
async fn err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    assert_footer(&result, tool);
    body(&result)["error"].clone()
}

#[tokio::test]
async fn every_task_tool_succeeds_and_ends_with_the_footer() {
    let fx = Fixture::new().await;
    let client = fx.connect("frontend").await;

    assert_eq!(
        ok(
            &client,
            "create_label",
            json!({"name": "api", "color": "teal"})
        )
        .await,
        json!({"label": "api"})
    );
    assert_eq!(
        ok(
            &client,
            "create_task",
            json!({"title": "Schema", "labels": ["api"]})
        )
        .await,
        json!({"id": "T-1", "status": "todo"})
    );
    assert_eq!(
        ok(
            &client,
            "create_task",
            json!({"title": "Endpoint", "needs": ["T-1"], "description": "Serve it"})
        )
        .await,
        json!({"id": "T-2", "status": "backlog"})
    );
    assert_eq!(
        ok(
            &client,
            "create_task",
            json!({"title": "Handler", "parent": "T-2", "as": "frontend"})
        )
        .await,
        json!({"id": "T-3", "status": "todo"})
    );
    assert_eq!(
        ok(
            &client,
            "update_task",
            json!({"id": "T-1", "title": "Schema v2", "labels": []})
        )
        .await,
        json!({"id": "T-1"})
    );
    assert_eq!(
        ok(
            &client,
            "add_dependency",
            json!({"task": "T-3", "needs": "T-1"})
        )
        .await,
        json!({"task": "T-3", "needs": "T-1"})
    );
    assert_eq!(
        ok(
            &client,
            "remove_dependency",
            json!({"task": "T-3", "needs": "T-1"})
        )
        .await,
        json!({"task": "T-3", "needs": "T-1"})
    );
    assert_eq!(
        ok(
            &client,
            "add_checklist_item",
            json!({"task": "T-2", "text": "Wire it"})
        )
        .await,
        json!({"item": "ci-2", "checklist": "Subtasks"})
    );
    assert_eq!(
        ok(
            &client,
            "add_checklist_item",
            json!({"task": "T-2", "text": "Docs", "checklist": "Later"})
        )
        .await,
        json!({"item": "ci-3", "checklist": "Later"})
    );
    assert_eq!(
        ok(
            &client,
            "update_checklist_item",
            json!({"id": "ci-2", "done": true, "text": "Wire it up"})
        )
        .await,
        json!({"item": "ci-2"})
    );
    assert_eq!(
        ok(
            &client,
            "move_checklist_item",
            json!({"id": "ci-2", "position": 0})
        )
        .await,
        json!({"item": "ci-2", "checklist": "Subtasks"})
    );
    assert_eq!(
        ok(
            &client,
            "move_checklist_item",
            json!({"id": "ci-3", "position": 9, "checklist": "Subtasks"})
        )
        .await,
        json!({"item": "ci-3", "checklist": "Subtasks"})
    );

    let task = ok(&client, "get_task", json!({"id": "T-2"})).await;
    assert_eq!(task["title"], "Endpoint");
    assert_eq!(task["description"], "Serve it");
    assert_eq!(task["status"], "backlog");
    assert_eq!(task["archived"], false);
    assert_eq!(
        task["checklists"],
        json!([
            {"name": "Subtasks", "items": [
                {"id": "ci-2", "text": "Wire it up", "done": true},
                {"id": "ci-1", "linked_task": "T-3", "status": "todo", "done": false},
                {"id": "ci-3", "text": "Docs", "done": false},
            ]},
            {"name": "Later", "items": []},
        ])
    );
    assert_eq!(task["progress"], json!({"done": 1, "total": 3}));
    assert_eq!(
        task["needs"],
        json!([
            {"id": "T-1", "title": "Schema v2", "status": "todo", "origin": "direct"},
            {"id": "T-3", "title": "Handler", "status": "todo", "origin": "checklist"},
        ])
    );
    assert_eq!(task["needed_by"], json!([]));
    assert_eq!(task["claim"], Value::Null);
    assert_eq!(task["workspace"], Value::Null);

    assert_eq!(
        ok(&client, "remove_checklist_item", json!({"id": "ci-3"})).await,
        json!({"item": "ci-3"})
    );
    assert_eq!(
        ok(&client, "claim_task", json!({"id": "T-1"})).await,
        json!({"id": "T-1", "holder": "frontend"})
    );
    let graph = ok(&client, "get_task_graph", json!({})).await;
    assert_eq!(
        graph["tasks"][0],
        json!({"id": "T-1", "title": "Schema v2", "status": "todo", "blocked": false, "labels": [], "needs": [], "holder": "frontend"})
    );
    assert_eq!(graph["tasks"][1]["needs"], json!(["T-1", "T-3"]));
    let ready = ok(&client, "get_ready_tasks", json!({})).await;
    assert_eq!(
        ready,
        json!({"tasks": [{"id": "T-3", "title": "Handler", "labels": [], "unblocks": 0}]})
    );
    assert_eq!(
        ok(&client, "release_task", json!({"id": "T-1"})).await,
        json!({"id": "T-1", "holder": null})
    );
    let ready = ok(&client, "get_ready_tasks", json!({})).await;
    assert_eq!(ready["tasks"][0]["id"], "T-1");
    assert_eq!(ready["tasks"][0]["unblocks"], 0);

    assert_eq!(
        ok(&client, "archive_task", json!({"id": "T-3"})).await,
        json!({"id": "T-3"})
    );
    ok(&client, "create_task", json!({"title": "Spare"})).await;
    assert_eq!(
        ok(&client, "delete_task", json!({"id": "T-4"})).await,
        json!({"id": "T-4"})
    );
    let graph = ok(&client, "get_task_graph", json!({"include_done": true})).await;
    let ids: Vec<&str> = graph["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["T-1", "T-2"]);
}

#[tokio::test]
async fn a_cycle_names_the_cycle_and_a_create_with_a_cycle_leaves_nothing() {
    let fx = Fixture::new().await;
    let client = fx.connect("frontend").await;
    ok(&client, "create_task", json!({"title": "A"})).await;
    ok(
        &client,
        "create_task",
        json!({"title": "B", "needs": ["T-1"]}),
    )
    .await;

    let cycle = err(
        &client,
        "add_dependency",
        json!({"task": "T-1", "needs": "T-2"}),
    )
    .await;
    assert_eq!(cycle["code"], "dependency_cycle");
    assert!(cycle["hint"].is_string());

    let whole = err(
        &client,
        "create_task",
        json!({"title": "C", "needs": ["T-2"], "parent": "T-2"}),
    )
    .await;
    assert_eq!(whole["code"], "dependency_cycle");
    assert!(whole["hint"].is_string());
    let graph = ok(&client, "get_task_graph", json!({})).await;
    assert_eq!(graph["tasks"].as_array().unwrap().len(), 2);
    let parent = ok(&client, "get_task", json!({"id": "T-2"})).await;
    assert_eq!(parent["checklists"], json!([]));
    assert_eq!(
        ok(&client, "create_task", json!({"title": "D"})).await["id"],
        "T-3",
        "the refused create used no task number"
    );
}

#[tokio::test]
async fn two_clients_racing_for_a_claim_get_one_claim_and_one_already_claimed() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    let backend = fx.connect("backend").await;
    ok(&frontend, "create_task", json!({"title": "Contested"})).await;

    let args = json!({"id": "T-1"});
    let (a, b) = tokio::join!(
        call_raw(&frontend, "claim_task", args.clone()),
        call_raw(&backend, "claim_task", args)
    );
    let (won, lost) = match (a.is_error, b.is_error) {
        (Some(false), Some(true)) => (a, b),
        (Some(true), Some(false)) => (b, a),
        other => panic!("expected one winner, got {other:?}"),
    };
    let holder = body(&won)["holder"].as_str().unwrap().to_owned();
    let error = body(&lost)["error"].clone();
    assert_eq!(error["code"], "already_claimed");
    assert_eq!(error["details"]["holder"], json!(holder));
    assert!(error["hint"].as_str().unwrap().contains("takeover_claim"));
}

#[tokio::test]
async fn a_delete_after_history_is_refused_with_its_reason() {
    let fx = Fixture::new().await;
    let client = fx.connect("frontend").await;
    ok(&client, "create_task", json!({"title": "Talked about"})).await;
    execute(
        fx.project.store(),
        "INSERT INTO messages (number, thread_id, sender_kind, sender_human, task_id, body, created_at)
         VALUES (1, 1, 'human', 'ana', (SELECT id FROM tasks WHERE number = 1), 'hello', 1)",
    )
    .await;
    let error = err(&client, "delete_task", json!({"id": "T-1"})).await;
    assert_eq!(error["code"], "invalid_request");
    assert_eq!(error["details"]["reason"], "task_has_history");
    assert!(error["hint"].as_str().unwrap().contains("archive_task"));
}

#[tokio::test]
async fn takeover_claim_waits_for_a_stale_lease() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    let backend = fx.connect("backend").await;
    ok(&frontend, "create_task", json!({"title": "Abandoned"})).await;
    ok(&frontend, "claim_task", json!({"id": "T-1"})).await;

    let early = err(&backend, "takeover_claim", json!({"id": "T-1"})).await;
    assert_eq!(early["code"], "claim_not_stale");
    assert_eq!(early["details"]["holder"], "frontend");
    assert_eq!(early["details"]["lease_minutes"], LEASE);
    assert!(early["hint"].as_str().unwrap().contains("get_ready_tasks"));
    let mine = err(&frontend, "takeover_claim", json!({"id": "T-1"})).await;
    assert_eq!(mine["code"], "already_holder");
    assert!(mine["hint"].is_string());

    fx.project
        .clock()
        .advance(Duration::from_secs(LEASE * 60))
        .unwrap();
    assert_eq!(
        ok(&backend, "takeover_claim", json!({"id": "T-1"})).await,
        json!({"id": "T-1", "holder": "backend"})
    );
    let task = ok(&backend, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(task["claim"]["holder"], "backend");
    assert_eq!(task["claim"]["stale"], false);
    let lost = err(&frontend, "release_task", json!({"id": "T-1"})).await;
    assert_eq!(lost["code"], "not_claim_holder");
}

#[tokio::test]
async fn bad_input_is_refused_with_a_code() {
    let fx = Fixture::new().await;
    let client = fx.connect("frontend").await;
    for (tool, arguments, code) in [
        ("get_task", json!({"id": "12"}), "invalid_id"),
        ("get_task", json!({"id": "T-9"}), "task_not_found"),
        ("get_task", json!({}), "invalid_request"),
        ("create_task", json!({"title": " "}), "invalid_task_title"),
        (
            "create_task",
            json!({"title": "x", "labels": ["nope"]}),
            "label_not_found",
        ),
        (
            "create_label",
            json!({"name": "x", "color": "red"}),
            "invalid_label_color",
        ),
        (
            "add_checklist_item",
            json!({"task": "T-1"}),
            "invalid_request",
        ),
        (
            "update_checklist_item",
            json!({"id": "ci-9", "done": true}),
            "checklist_item_not_found",
        ),
        (
            "move_checklist_item",
            json!({"id": "ci-9", "position": 0}),
            "checklist_item_not_found",
        ),
        ("takeover_claim", json!({"id": "T-9"}), "task_not_found"),
    ] {
        let error = err(&client, tool, arguments).await;
        assert_eq!(error["code"], code, "{tool}");
    }
    let error = err(&client, "add_checklist_item", json!({"task": "T-1"})).await;
    assert!(error["hint"].as_str().unwrap().contains("linked_task"));
    let error = err(&client, "get_task", json!({"id": "T-1", "as": "backend"})).await;
    assert_eq!(error["code"], "worker_not_found");
}

#[test]
fn every_task_tool_input_field_has_a_description() {
    let tools = fold_daemon::mcp::tasks::tools();
    assert_eq!(tools.len(), 17);
    for tool in &tools {
        assert!(tool.description.is_some(), "{}", tool.name);
        let properties = tool.input_schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} has no properties", tool.name));
        assert!(properties.contains_key("as"), "{}", tool.name);
        for (field, schema) in properties {
            let described = schema["description"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty());
            assert!(described, "{}.{field} has no description", tool.name);
        }
    }
}

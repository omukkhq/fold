use std::sync::Arc;

use fold_core::{HumanName, TaskDescription, TaskTitle};
use fold_daemon::mcp::FoldServer;
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::messages::MessageService;
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::projects;
use fold_store::testing::TestProject;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    agents: AgentService,
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    /// Human `tanim` and task `T-1`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let now = project.now();
        let tanim = HumanName::parse("tanim").unwrap();
        project
            .registry()
            .write(async |tx| projects::insert_human(tx, &tanim, now).await)
            .await
            .unwrap();
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock, 5);
        tasks
            .create_task(
                &fold_core::Actor::human(tanim),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new(""),
                    labels: Vec::new(),
                },
            )
            .await
            .unwrap();
        Self {
            project,
            bus,
            agents,
            tasks,
        }
    }

    fn workspaces(&self) -> WorkspaceService {
        WorkspaceService::new(
            self.project.store().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
            self.project.paths().clone(),
            5,
        )
    }

    /// A new adapter session of agent `name` over an in-memory stream, already registered.
    async fn connect(&self, name: &str) -> Client {
        let events = EventService::new(self.project.store().clone(), self.bus.clone());
        let messages = MessageService::new(
            self.project.store().clone(),
            self.project.registry().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
        );
        let context = self.agents.start_session(7).await.unwrap();
        let server = FoldServer::new(
            self.agents.clone(),
            events,
            self.workspaces(),
            self.tasks.clone(),
            messages,
            context,
        );
        let (client_end, server_end) = tokio::io::duplex(256 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });
        let client = ().serve(client_end).await.unwrap();
        call(&client, "register_agent", json!({"name": name})).await;
        client
    }
}

#[cfg(test)]
fn params(tool: &str, arguments: Value) -> CallToolRequestParams {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments)
}

#[cfg(test)]
async fn call_raw(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    client.call_tool(params(tool, arguments)).await.unwrap()
}

#[cfg(test)]
fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// The footer line of a result, if it has one.
#[cfg(test)]
fn footer(result: &CallToolResult) -> Option<String> {
    result
        .content
        .get(1)
        .map(|block| block.as_text().unwrap().text.clone())
}

#[cfg(test)]
async fn call(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    body(&result)
}

#[cfg(test)]
fn inbox_ids(body: &Value) -> Vec<(String, String)> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            (
                message["id"].as_str().unwrap().to_owned(),
                message["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(id, to)| ((*id).to_owned(), (*to).to_owned()))
        .collect()
}

#[tokio::test]
async fn the_message_tools_are_listed_with_described_inputs() {
    let fixture = Fixture::new().await;
    let client = fixture.connect("frontend").await;
    let tools = client.list_all_tools().await.unwrap();
    for name in [
        "send_message",
        "get_inbox",
        "get_thread",
        "acknowledge_message",
        "mark_answered",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        let schema = serde_json::to_value(tool.input_schema.as_ref()).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("as"), "{name}");
        for (field, property) in properties {
            assert!(
                property["description"].is_string(),
                "{name}.{field} has no description"
            );
        }
    }
}

#[tokio::test]
async fn send_then_inbox_returns_the_message_until_it_is_acknowledged() {
    let fixture = Fixture::new().await;
    let frontend = fixture.connect("frontend").await;
    let backend = fixture.connect("backend").await;
    let sent = call(
        &frontend,
        "send_message",
        json!({"to": ["backend"], "body": "the schema is ready", "task": "T-1"}),
    )
    .await;
    assert_eq!(
        sent,
        json!({"id": "m-1", "thread": "m-1", "recipients": ["backend"]})
    );
    let inbox = call(&backend, "get_inbox", json!({})).await;
    let first = &inbox["messages"][0];
    assert_eq!(first["id"], "m-1");
    assert_eq!(first["thread"], "m-1");
    assert_eq!(first["from"], "frontend");
    assert_eq!(first["to"], "backend");
    assert_eq!(first["addressed_as"], "agent");
    assert_eq!(first["task"], "T-1");
    assert_eq!(first["body"], "the schema is ready");
    assert!(first["sent_at"].is_i64());
    assert!(first.get("reply_to").is_none() && first.get("proposal").is_none());
    let again = call(&backend, "get_inbox", json!({})).await;
    assert_eq!(inbox_ids(&again), pairs(&[("m-1", "backend")]));
    let reply = call(
        &backend,
        "send_message",
        json!({"to": ["frontend"], "body": "thanks", "reply_to": "m-1"}),
    )
    .await;
    assert_eq!(reply["thread"], "m-1");
    let acknowledged = call(&backend, "acknowledge_message", json!({"id": "m-1"})).await;
    assert_eq!(acknowledged, json!({"acknowledged": ["m-1"]}));
    let empty = call(&backend, "get_inbox", json!({})).await;
    assert_eq!(empty, json!({"messages": []}));
    let thread = call(&frontend, "get_thread", json!({"thread": "m-1"})).await;
    assert_eq!(
        thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| (message["id"].clone(), message["from"].clone()))
            .collect::<Vec<_>>(),
        vec![
            (json!("m-1"), json!("frontend")),
            (json!("m-2"), json!("backend"))
        ]
    );
    assert_eq!(thread["messages"][1]["reply_to"], "m-1");
    assert_eq!(thread["messages"][1]["task"], "T-1");
    assert_eq!(thread["messages"][1]["to"], json!(["frontend"]));
}

#[tokio::test]
async fn acknowledge_with_up_to_clears_every_earlier_message() {
    let fixture = Fixture::new().await;
    let frontend = fixture.connect("frontend").await;
    let backend = fixture.connect("backend").await;
    for _ in 0..3 {
        call(
            &frontend,
            "send_message",
            json!({"to": ["backend"], "body": "x"}),
        )
        .await;
    }
    let one = call(&backend, "acknowledge_message", json!({"id": "m-2"})).await;
    assert_eq!(one, json!({"acknowledged": ["m-2"]}));
    let rest = call(
        &backend,
        "acknowledge_message",
        json!({"id": "m-3", "up_to": true}),
    )
    .await;
    assert_eq!(rest, json!({"acknowledged": ["m-1", "m-3"]}));
    let repeat = call(
        &backend,
        "acknowledge_message",
        json!({"id": "m-3", "up_to": true}),
    )
    .await;
    assert_eq!(repeat, json!({"acknowledged": []}));
}

#[tokio::test]
async fn a_message_sent_while_the_receiver_is_away_waits_for_its_next_session() {
    let fixture = Fixture::new().await;
    let backend = fixture.connect("backend").await;
    backend.cancel().await.unwrap();
    let frontend = fixture.connect("frontend").await;
    call(
        &frontend,
        "send_message",
        json!({"to": ["backend"], "body": "while you were away"}),
    )
    .await;
    let back = fixture.connect("backend").await;
    let inbox = call(&back, "get_inbox", json!({})).await;
    assert_eq!(inbox_ids(&inbox), pairs(&[("m-1", "backend")]));
}

#[tokio::test]
async fn as_reads_one_worker_and_include_workers_reads_them_all() {
    let fixture = Fixture::new().await;
    let frontend = fixture.connect("frontend").await;
    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let backend = fixture.connect("backend").await;
    call(
        &backend,
        "send_message",
        json!({"to": ["frontend"], "body": "x"}),
    )
    .await;
    call(
        &backend,
        "send_message",
        json!({"to": ["frontend/ui"], "body": "y"}),
    )
    .await;
    let main = call(&frontend, "get_inbox", json!({})).await;
    assert_eq!(inbox_ids(&main), pairs(&[("m-1", "frontend")]));
    let ui = call(&frontend, "get_inbox", json!({"as": "frontend/ui"})).await;
    assert_eq!(inbox_ids(&ui), pairs(&[("m-2", "frontend/ui")]));
    assert_eq!(ui["messages"][0]["addressed_as"], "worker");
    let all = call(&frontend, "get_inbox", json!({"include_workers": true})).await;
    assert_eq!(
        inbox_ids(&all),
        pairs(&[("m-1", "frontend"), ("m-2", "frontend/ui")])
    );
    let wrong = call_raw(&frontend, "acknowledge_message", json!({"id": "m-2"})).await;
    assert_eq!(body(&wrong)["error"]["code"], "message_not_found");
    let acknowledged = call(
        &frontend,
        "acknowledge_message",
        json!({"id": "m-2", "as": "frontend/ui"}),
    )
    .await;
    assert_eq!(acknowledged, json!({"acknowledged": ["m-2"]}));
}

#[tokio::test]
async fn the_footer_counts_unread_messages_and_drops_the_part_once_acknowledged() {
    let fixture = Fixture::new().await;
    let frontend = fixture.connect("frontend").await;
    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let backend = fixture.connect("backend").await;
    for to in ["frontend", "frontend/ui"] {
        call(&backend, "send_message", json!({"to": [to], "body": "x"})).await;
    }
    let listed = call_raw(&frontend, "list_agents", json!({})).await;
    let line = footer(&listed).unwrap();
    assert!(line.starts_with("fold: 2 unread"), "{line}");
    call(&frontend, "acknowledge_message", json!({"id": "m-1"})).await;
    let line = footer(&call_raw(&frontend, "list_agents", json!({})).await).unwrap();
    assert!(line.starts_with("fold: 1 unread"), "{line}");
    let last = call_raw(
        &frontend,
        "acknowledge_message",
        json!({"id": "m-2", "as": "frontend/ui"}),
    )
    .await;
    assert!(
        footer(&last).is_none_or(|line| !line.contains("unread")),
        "{last:?}"
    );
    assert!(
        footer(&call_raw(&backend, "list_agents", json!({})).await)
            .is_none_or(|line| !line.contains("unread"))
    );
}

#[tokio::test]
async fn errors_carry_the_code_and_the_next_tool() {
    let fixture = Fixture::new().await;
    let frontend = fixture.connect("frontend").await;
    let unknown = call_raw(
        &frontend,
        "send_message",
        json!({"to": ["ghost"], "body": "x"}),
    )
    .await;
    assert_eq!(unknown.is_error, Some(true));
    assert_eq!(
        body(&unknown)["error"],
        json!({
            "code": "recipient_not_found",
            "message": "recipient ghost does not exist",
            "details": {"to": ["ghost"]},
            "hint": "list the agents and their workers with list_agents",
        })
    );
    for (tool, arguments) in [
        ("acknowledge_message", json!({"id": "m-9"})),
        ("get_thread", json!({"thread": "m-9"})),
        (
            "send_message",
            json!({"to": ["frontend"], "body": "x", "reply_to": "m-9"}),
        ),
    ] {
        let result = call_raw(&frontend, tool, arguments).await;
        let error = &body(&result)["error"];
        assert_eq!(error["code"], "message_not_found", "{tool}");
        assert_eq!(
            error["hint"], "call get_inbox to see your messages",
            "{tool}"
        );
    }
    for (tool, arguments, code) in [
        ("get_thread", json!({"thread": "T-1"}), "invalid_id"),
        (
            "send_message",
            json!({"to": ["frontend"], "body": "  "}),
            "invalid_request",
        ),
        (
            "send_message",
            json!({"to": ["Not A Name"], "body": "x"}),
            "invalid_name",
        ),
        (
            "send_message",
            json!({"to": ["frontend"], "body": "x", "task": "P-1"}),
            "invalid_id",
        ),
        (
            "get_inbox",
            json!({"include_workers": "yes"}),
            "invalid_request",
        ),
        (
            "get_inbox",
            json!({"as": "frontend/ghost"}),
            "worker_not_found",
        ),
    ] {
        let result = call_raw(&frontend, tool, arguments).await;
        assert_eq!(body(&result)["error"]["code"], code, "{tool}");
    }
}

#[tokio::test]
async fn the_message_tools_need_a_registered_agent() {
    let fixture = Fixture::new().await;
    let events = EventService::new(fixture.project.store().clone(), fixture.bus.clone());
    let messages = MessageService::new(
        fixture.project.store().clone(),
        fixture.project.registry().clone(),
        fixture.bus.clone(),
        Arc::new(fixture.project.clock().clone()),
    );
    let context = fixture.agents.start_session(9).await.unwrap();
    let server = FoldServer::new(
        fixture.agents.clone(),
        events,
        fixture.workspaces(),
        fixture.tasks.clone(),
        messages,
        context,
    );
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client: Client = ().serve(client_end).await.unwrap();
    let result = call_raw(&client, "get_inbox", json!({})).await;
    assert_eq!(body(&result)["error"]["code"], "agent_not_registered");
    assert_eq!(footer(&result), None);
}

#[tokio::test]
async fn an_ask_to_a_human_shows_its_mark_and_its_sender_records_an_answer_given_elsewhere() {
    let fixture = Fixture::new().await;
    let frontend = fixture.connect("frontend").await;
    let backend = fixture.connect("backend").await;
    let sent = call(
        &frontend,
        "send_message",
        json!({"to": ["human:tanim", "backend"], "body": "which one?", "asks_human": true}),
    )
    .await;
    assert_eq!(sent["id"], "m-1");
    let inbox = call(&backend, "get_inbox", json!({})).await;
    assert_eq!(inbox["messages"][0]["asks_human"], true);
    let thread = call(&backend, "get_thread", json!({"thread": "m-1"})).await;
    assert_eq!(thread["messages"][0]["asks_human"], true);
    let report = call(
        &frontend,
        "send_message",
        json!({"to": ["humans"], "body": "done"}),
    )
    .await;
    let thread = call(&backend, "get_thread", json!({"thread": report["id"]})).await;
    assert!(thread["messages"][0].get("asks_human").is_none());
    let no_human = call_raw(
        &frontend,
        "send_message",
        json!({"to": ["backend"], "body": "x", "asks_human": true}),
    )
    .await;
    assert_eq!(body(&no_human)["error"]["code"], "invalid_request");
    let denied = call_raw(&backend, "mark_answered", json!({"message": "m-1"})).await;
    assert_eq!(body(&denied)["error"]["code"], "ask_close_denied");
    assert!(body(&denied)["error"]["hint"].is_string());
    let answered = call(&frontend, "mark_answered", json!({"message": "m-1"})).await;
    assert_eq!(answered, json!({"answered": "m-1"}));
    let again = call_raw(&frontend, "mark_answered", json!({"message": "m-1"})).await;
    assert_eq!(body(&again)["error"]["code"], "ask_not_open");
}

use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use fold_core::object::ObjectKind;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, EventFilter, RevisionId, TaskDescription, TaskTitle, WorkerName,
};
use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::messages::MessageService;
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::events;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    self, advance_main, create_initial_revision, initial_revision, insert_revision,
};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const BASE: &[u8] = b"one\ntwo\nthree\n";
const MINE: &[u8] = b"one\nmine\nthree\n";
const THEIRS: &[u8] = b"one\ntheirs\nthree\n";
const README: &[u8] = b"# demo\n";
const FOOTER: &str = "fold: 1 unread";

const WRITE_TOOLS: [&str; 5] = [
    "record_checkpoint",
    "record_changes",
    "sync_workspace",
    "abandon_workspace",
    "restore_checkpoint",
];

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

/// One unread message for every session, so every result must end with the footer line.
#[cfg(test)]
struct OneUnread;

impl FooterSource for OneUnread {
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
    workspaces: WorkspaceService,
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    /// `r1` holds `src/lib.rs` and `README.md`; the tasks `T-1` and `T-2` exist.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let tree = write_initial_tree(&objects, flat(&objects, &[("src/lib.rs", BASE)])).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock.clone(), 5);
        let creator = Actor::Agent(AgentActor {
            agent: WorkerName::parse("frontend").unwrap().agent(),
            worker: WorkerName::parse("frontend").unwrap(),
            tags: Vec::new(),
        });
        for _ in 0..2 {
            tasks
                .create_task(
                    &creator,
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let workspaces = WorkspaceService::new(
            project.store().clone(),
            bus.clone(),
            clock,
            project.paths().clone(),
            5,
        );
        Self {
            project,
            bus,
            agents,
            workspaces,
            tasks,
        }
    }

    fn messages(&self) -> MessageService {
        MessageService::new(
            self.project.store().clone(),
            self.project.registry().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
        )
    }

    /// A session of agent `name` over an in-memory stream, registered.
    async fn connect(&self, name: &str) -> Client {
        let events = EventService::new(self.project.store().clone(), self.bus.clone());
        let session = self.agents.start_session(7).await.unwrap();
        let server = FoldServer::new(
            self.agents.clone(),
            events,
            self.workspaces.clone(),
            self.tasks.clone(),
            self.messages(),
            session,
        )
        .with_footer_source(Arc::new(OneUnread));
        let (client_end, server_end) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });
        let client = ().serve(client_end).await.unwrap();
        call(&client, "register_agent", json!({"name": name})).await;
        client
    }

    fn file(&self, workspace_path: &str, path: &str) -> PathBuf {
        self.project.paths().root().join(workspace_path).join(path)
    }

    /// Records a revision on top of `main` with `src/lib.rs` holding `lib`, and moves `main` to it, as the integration procedure would.
    async fn move_main(&self, lib: &[u8]) -> RevisionId {
        let store = self.project.store();
        let head = workspaces::head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.project.paths().fold_dir()).unwrap();
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, flat(&objects, &[("src/lib.rs", lib)])).unwrap(),
            parent: Some(head.hash),
            number,
            integration: None,
            created_at: self.project.now(),
        };
        let stored = write_revision(&objects, &revision).unwrap();
        store
            .write(async |tx| {
                insert_revision(tx, &stored).await?;
                advance_main(tx, head.number, number).await
            })
            .await
            .unwrap();
        number
    }

    async fn event_types(&self) -> Vec<String> {
        events::read_after(
            self.project.store().read(),
            0,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
        .iter()
        .map(|event| event.event_type.as_str().to_owned())
        .collect()
    }
}

/// `README.md` plus `extra`, as a flat tree whose blobs are stored.
#[cfg(test)]
fn flat(objects: &ObjectStore, extra: &[(&str, &[u8])]) -> FlatTree {
    let mut tree = FlatTree::new();
    for (path, content) in [("README.md", README)].iter().chain(extra) {
        let hash = objects.write(ObjectKind::Blob, content).unwrap();
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash,
            },
        );
    }
    tree
}

#[cfg(test)]
async fn call_raw(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap();
    assert_eq!(result.content.len(), 2, "{tool}: {result:?}");
    assert!(
        result.content[1]
            .as_text()
            .unwrap()
            .text
            .starts_with(FOOTER),
        "{tool} has no footer: {result:?}"
    );
    result
}

#[cfg(test)]
fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[cfg(test)]
async fn call(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    body(&result)
}

#[cfg(test)]
async fn call_err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    body(&result)["error"].clone()
}

#[tokio::test]
async fn an_agent_works_in_its_workspace_from_create_to_abandon() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;

    let created = call(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    assert_eq!(
        created,
        json!({"workspace": "ws-1", "path": ".fold/ws/t-1", "base": "r1", "task": "T-1", "adopted": false})
    );
    let lib = fx.file(".fold/ws/t-1", "src/lib.rs");
    assert_eq!(fs::read(&lib).unwrap(), BASE);
    fs::write(&lib, MINE).unwrap();

    let checkpoint = call(
        &frontend,
        "record_checkpoint",
        json!({"ws": "ws-1", "label": "first try"}),
    )
    .await;
    assert_eq!(checkpoint, json!({"checkpoint": "ck-1"}));
    let listed = call(&frontend, "list_checkpoints", json!({"ws": "ws-1"})).await;
    let checkpoints = listed["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0]["checkpoint"], "ck-1");
    assert_eq!(checkpoints[0]["kind"], "explicit");
    assert_eq!(checkpoints[0]["label"], "first try");
    assert!(checkpoints[0]["created_at"].is_number());

    let recorded = call(
        &frontend,
        "record_changes",
        json!({"ws": "ws-1", "message": "change the second line"}),
    )
    .await;
    assert_eq!(recorded, json!({"changeset": "C-1", "files": 1}));

    // `ck-1` already holds these files on this base, so neither the record nor the sync adds a checkpoint.
    let main = fx.move_main(THEIRS).await;
    assert_eq!(main.to_string(), "r2");
    let synced = call(&frontend, "sync_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(
        synced,
        json!({"base": "r2", "conflicted": ["src/lib.rs"], "checkpoint": "ck-1"})
    );
    let merged = fs::read_to_string(&lib).unwrap();
    assert!(merged.contains("<<<<<<< workspace"), "{merged}");
    assert!(merged.contains(">>>>>>> main r2"), "{merged}");

    let got = call(&frontend, "get_workspace", json!({"task": "T-1"})).await;
    assert_eq!(got["workspace"], "ws-1");
    assert_eq!(got["state"], "active");
    assert_eq!(got["path"], ".fold/ws/t-1");
    assert_eq!(got["base"], "r2");
    assert_eq!(got["task"], "T-1");
    assert_eq!(got["worker"], "frontend");
    assert_eq!(got["conflicted"], json!(["src/lib.rs"]));
    assert_eq!(got["latest_checkpoint"], "ck-1");
    assert_eq!(got["latest_changeset"], "C-1");
    assert!(got.get("changed_files").is_some());
    assert!(got.get("last_activity_at").is_some());

    let restored = call(&frontend, "restore_checkpoint", json!({"ck": "ck-1"})).await;
    assert_eq!(
        restored,
        json!({"restored": "ck-1", "safety": "ck-2", "base": "r1"})
    );
    assert_eq!(fs::read(&lib).unwrap(), MINE);
    let got = call(&frontend, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(got["base"], "r1");
    assert_eq!(got["latest_checkpoint"], "ck-2");

    // The safety checkpoint holds the merged files, so the restore can be undone too.
    call(&frontend, "restore_checkpoint", json!({"ck": "ck-2"})).await;
    assert_eq!(fs::read_to_string(&lib).unwrap(), merged);
    let got = call(&frontend, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(got["base"], "r2");
    assert_eq!(got["conflicted"], json!(["src/lib.rs"]));
    let error = call_err(
        &frontend,
        "record_changes",
        json!({"ws": "ws-1", "message": "keep the markers"}),
    )
    .await;
    assert_eq!(error["code"], "unresolved_conflicts");

    let abandoned = call(
        &frontend,
        "abandon_workspace",
        json!({"ws": "ws-1", "reason": "the approach does not work"}),
    )
    .await;
    assert_eq!(
        abandoned,
        json!({"workspace": "ws-1", "task_status": "todo"})
    );
    let got = call(&frontend, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(got["state"], "abandoned");

    let types = fx.event_types().await;
    for expected in [
        "workspace.created",
        "checkpoint.recorded",
        "changeset.recorded",
        "workspace.synced",
        "checkpoint.restored",
        "workspace.abandoned",
    ] {
        assert!(types.iter().any(|found| found == expected), "{expected}");
    }
}

#[tokio::test]
async fn only_the_owning_worker_writes_in_a_workspace() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    let backend = fx.connect("backend").await;
    call(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    call(&frontend, "record_checkpoint", json!({"ws": "ws-1"})).await;

    for tool in WRITE_TOOLS {
        let arguments = match tool {
            "restore_checkpoint" => json!({"ck": "ck-1"}),
            "record_changes" => json!({"ws": "ws-1", "message": "take over"}),
            "abandon_workspace" => json!({"ws": "ws-1", "reason": "mine now"}),
            _ => json!({"ws": "ws-1"}),
        };
        let error = call_err(&backend, tool, arguments).await;
        assert_eq!(error["code"], "not_workspace_owner", "{tool}");
        assert!(
            error["hint"].as_str().unwrap().contains("get_workspace"),
            "{tool}"
        );
    }
    let got = call(&backend, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(got["worker"], "frontend");
    let listed = call(&backend, "list_checkpoints", json!({"ws": "ws-1"})).await;
    assert_eq!(listed["checkpoints"].as_array().unwrap().len(), 1);

    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let created = call(
        &frontend,
        "create_workspace",
        json!({"task": "T-2", "as": "frontend/ui"}),
    )
    .await;
    assert_eq!(created["workspace"], "ws-2");
    let got = call(&frontend, "get_workspace", json!({"ws": "ws-2"})).await;
    assert_eq!(got["worker"], "frontend/ui");
    let error = call_err(&frontend, "record_checkpoint", json!({"ws": "ws-2"})).await;
    assert_eq!(error["code"], "not_workspace_owner");
    let checkpoint = call(
        &frontend,
        "record_checkpoint",
        json!({"ws": "ws-2", "as": "frontend/ui"}),
    )
    .await;
    assert_eq!(checkpoint, json!({"checkpoint": "ck-2"}));
}

#[tokio::test]
async fn record_changes_waits_until_every_marker_is_gone() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    call(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    let lib = fx.file(".fold/ws/t-1", "src/lib.rs");
    fs::write(&lib, MINE).unwrap();
    fx.move_main(THEIRS).await;
    call(&frontend, "sync_workspace", json!({"ws": "ws-1"})).await;

    let message = json!({"ws": "ws-1", "message": "resolve"});
    let error = call_err(&frontend, "record_changes", message.clone()).await;
    assert_eq!(error["code"], "unresolved_conflicts");
    assert!(error["hint"].as_str().unwrap().contains("record_changes"));
    assert!(
        error["message"].as_str().unwrap().contains("src/lib.rs"),
        "{error}"
    );

    let merged = fs::read_to_string(&lib).unwrap();
    let partly: String = merged
        .lines()
        .filter(|line| !line.starts_with("<<<<<<<"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&lib, partly).unwrap();
    let error = call_err(&frontend, "record_changes", message.clone()).await;
    assert_eq!(error["code"], "unresolved_conflicts");

    fs::write(&lib, b"one\nmine and theirs\nthree\n").unwrap();
    let recorded = call(&frontend, "record_changes", message).await;
    assert_eq!(recorded["changeset"], "C-1");
    let got = call(&frontend, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(got["conflicted"], json!([]));
}

#[tokio::test]
async fn the_write_tools_refuse_a_closed_workspace_and_bad_input() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    call(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    call(&frontend, "record_checkpoint", json!({"ws": "ws-1"})).await;
    let synced = call(&frontend, "sync_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(
        synced,
        json!({"base": "r1", "conflicted": [], "checkpoint": null})
    );
    call(
        &frontend,
        "abandon_workspace",
        json!({"ws": "ws-1", "reason": "stop"}),
    )
    .await;
    for tool in ["record_checkpoint", "sync_workspace"] {
        let error = call_err(&frontend, tool, json!({"ws": "ws-1"})).await;
        assert_eq!(error["code"], "workspace_not_active", "{tool}");
    }
    let error = call_err(&frontend, "restore_checkpoint", json!({"ck": "ck-1"})).await;
    assert_eq!(error["code"], "workspace_not_active");
    let listed = call(&frontend, "list_checkpoints", json!({"ws": "ws-1"})).await;
    assert_eq!(listed["checkpoints"][0]["checkpoint"], "ck-1");

    let error = call_err(&frontend, "restore_checkpoint", json!({"ck": "ck-9"})).await;
    assert_eq!(error["code"], "checkpoint_not_found");
    assert_eq!(error["hint"], "call list_checkpoints");
    let error = call_err(&frontend, "list_checkpoints", json!({"ws": "ws-9"})).await;
    assert_eq!(error["code"], "workspace_not_found");
    let error = call_err(&frontend, "get_workspace", json!({"ws": "T-1"})).await;
    assert_eq!(error["code"], "invalid_id");
    let error = call_err(&frontend, "get_workspace", json!({})).await;
    assert_eq!(error["code"], "invalid_request");
    let error = call_err(
        &frontend,
        "get_workspace",
        json!({"ws": "ws-1", "task": "T-1"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = call_err(&frontend, "create_workspace", json!({"task": 1})).await;
    assert_eq!(error["code"], "invalid_request");
    let error = call_err(
        &frontend,
        "record_changes",
        json!({"ws": "ws-1", "message": "  "}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let long = "x".repeat(201);
    let error = call_err(
        &frontend,
        "record_checkpoint",
        json!({"ws": "ws-1", "label": long}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
}

#[tokio::test]
async fn the_workspace_tools_need_a_registered_agent() {
    let fx = Fixture::new().await;
    let events = EventService::new(fx.project.store().clone(), fx.bus.clone());
    let session = fx.agents.start_session(9).await.unwrap();
    let server = FoldServer::new(
        fx.agents.clone(),
        events,
        fx.workspaces.clone(),
        fx.tasks.clone(),
        fx.messages(),
        session,
    )
    .with_footer_source(Arc::new(OneUnread));
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client: Client = ().serve(client_end).await.unwrap();
    let error = call_err(&client, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(error["code"], "agent_not_registered");
}

#[test]
fn every_workspace_tool_field_has_a_description() {
    let tools = fold_daemon::mcp::workspaces::tools();
    assert_eq!(tools.len(), 14);
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

#[tokio::test]
async fn a_stale_claim_and_a_released_workspace_move_to_the_next_agent() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    let backend = fx.connect("backend").await;

    call(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    let lib = fx.file(".fold/ws/t-1", "src/lib.rs");
    fs::write(&lib, MINE).unwrap();
    fx.project
        .clock()
        .advance(std::time::Duration::from_secs(5 * 60))
        .unwrap();
    assert_eq!(
        call(&backend, "takeover_claim", json!({"id": "T-1"})).await,
        json!({"id": "T-1", "holder": "backend", "workspace": "ws-1"})
    );
    assert_eq!(fs::read(&lib).unwrap(), MINE);
    let workspace = call(&backend, "get_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(workspace["worker"], "backend");
    let old = call_err(&frontend, "record_checkpoint", json!({"ws": "ws-1"})).await;
    assert_eq!(old["code"], "not_workspace_owner");

    call(&frontend, "create_workspace", json!({"task": "T-2"})).await;
    call(&frontend, "release_task", json!({"id": "T-2"})).await;
    assert_eq!(
        call(&backend, "create_workspace", json!({"task": "T-2"})).await,
        json!({"workspace": "ws-2", "path": ".fold/ws/t-2", "base": "r1", "task": "T-2", "adopted": true})
    );
    assert!(
        fx.event_types()
            .await
            .iter()
            .filter(|kind| *kind == "workspace.adopted")
            .count()
            == 2
    );
}

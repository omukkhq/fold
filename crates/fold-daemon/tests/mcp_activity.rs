use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, EventFilter, TaskDescription, TaskId, TaskTitle, WorkerName,
};
use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBus, EventService};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::events;
use fold_store::testing::{TestProject, execute};
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const LEASE_MINUTES: u64 = 5;
const FOOTER: &str = "fold: 2 unread";

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

/// A footer source with a fixed count, so every result must carry the footer line.
#[cfg(test)]
struct Fixed;

impl FooterSource for Fixed {
    fn part(&self) -> FooterPart {
        FooterPart::Unread
    }

    fn count<'a>(
        &'a self,
        _session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async move { 2 })
    }
}

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn actor(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    agents: AgentService,
    tasks: TaskService,
    workspaces: WorkspaceService,
}

#[cfg(test)]
impl Fixture {
    /// `r1` holds one file and the project has three tasks. Claims use a lease of [`LEASE_MINUTES`].
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        let hash = objects.write(ObjectKind::Blob, b"# demo\n").unwrap();
        tree.insert(
            b"README.md".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash,
            },
        );
        let tree = write_initial_tree(&objects, tree).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone())
            .with_lease_minutes(LEASE_MINUTES);
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            LEASE_MINUTES,
        );
        let workspaces = WorkspaceService::new(
            project.store().clone(),
            bus,
            clock,
            project.paths().clone(),
            Git::default(),
            LEASE_MINUTES,
        );
        for _ in 0..3 {
            tasks
                .create_task(
                    &actor("planner"),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        Self {
            project,
            agents,
            tasks,
            workspaces,
        }
    }

    /// One adapter session whose results always carry a footer line.
    async fn connect(&self) -> (Client, SessionContext) {
        let events = EventService::new(self.project.store().clone(), EventBus::new());
        let session = self.agents.start_session(7).await.unwrap();
        let server = FoldServer::new(
            self.agents.clone(),
            events,
            self.workspaces.clone(),
            session.clone(),
        )
        .with_footer_source(Arc::new(Fixed));
        let (client_end, server_end) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });
        (().serve(client_end).await.unwrap(), session)
    }

    /// A session registered as `name`.
    async fn agent(&self, name: &str) -> (Client, SessionContext) {
        let (client, mut session) = self.connect().await;
        call(&client, "register_agent", json!({"name": name})).await;
        session.agent = Some(AgentName::parse(name).unwrap());
        (client, session)
    }

    fn advance(&self, by: Duration) {
        self.project.clock().advance(by).unwrap();
    }

    fn now(&self) -> i64 {
        self.project.now().as_millis()
    }

    async fn event_count(&self) -> usize {
        events::read_after(
            self.project.store().read(),
            0,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
        .len()
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

/// The JSON of a successful result; the footer from `mcp::footer` must be its last block. A registered caller's footer also counts its new events.
#[cfg(test)]
async fn call(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    assert_eq!(result.content.len(), 2, "{tool} has a footer line");
    assert!(
        result.content[1]
            .as_text()
            .unwrap()
            .text
            .starts_with(FOOTER)
    );
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// The `error` object of a failed result; the footer is added to errors too.
#[cfg(test)]
async fn call_err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    assert!(
        result.content[1]
            .as_text()
            .unwrap()
            .text
            .starts_with(FOOTER)
    );
    let body: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    body["error"].clone()
}

#[cfg(test)]
fn agent<'a>(listed: &'a Value, name: &str) -> &'a Value {
    listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not listed in {listed}"))
}

#[cfg(test)]
fn names(items: &Value, key: &str) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[key].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn list_agents_shows_presence_workers_claims_and_workspaces() {
    let fx = Fixture::new().await;
    let (frontend, frontend_session) = fx.agent("frontend").await;
    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let (_backend, _) = fx.agent("backend").await;
    let created = fx.now();
    fx.workspaces
        .create_workspace(&actor("frontend/ui"), t(1))
        .await
        .unwrap();
    fx.tasks.claim_task(&actor("backend"), t(2)).await.unwrap();
    let (reader, _) = fx.connect().await;
    let before = fx.event_count().await;

    let listed = call(&reader, "list_agents", json!({})).await;
    assert_eq!(names(&listed["agents"], "name"), ["frontend", "backend"]);
    let front = agent(&listed, "frontend");
    assert_eq!(front["presence"], "active");
    assert_eq!(front["tags"], json!([]));
    assert_eq!(front["last_call_at"], json!(created));
    assert_eq!(
        names(&front["workers"], "name"),
        ["frontend", "frontend/ui"]
    );
    let main = &front["workers"][0];
    assert_eq!(main["parent"], Value::Null);
    assert_eq!(main["activity"], "register_worker");
    assert_eq!(main["claims"], json!([]));
    let ui = &front["workers"][1];
    assert_eq!(ui["parent"], "frontend");
    assert_eq!(ui["last_call_at"], Value::Null);
    assert_eq!(
        ui["claims"],
        json!([{
            "task": "T-1",
            "kind": "implementation",
            "acquired_at": created,
            "renewed_at": created,
            "stale": false,
        }])
    );
    assert_eq!(
        ui["workspaces"],
        json!([{
            "ws": "ws-1",
            "task": "T-1",
            "state": "active",
            "base": "r1",
            "changed_files": 0,
            "last_activity_at": created,
            "latest_checkpoint": null,
        }])
    );
    assert!(
        ui.get("proposals").is_none(),
        "list_agents leaves proposals to get_activity"
    );
    let back = agent(&listed, "backend");
    assert_eq!(back["workers"][0]["claims"][0]["task"], "T-2");
    assert_eq!(back["workers"][0]["workspaces"], json!([]));
    assert_eq!(fx.event_count().await, before, "a read appends no event");

    fx.advance(Duration::from_millis(119_999));
    let listed = call(&reader, "list_agents", json!({})).await;
    assert_eq!(agent(&listed, "frontend")["presence"], "active");
    fx.advance(Duration::from_millis(1));
    let listed = call(&reader, "list_agents", json!({})).await;
    assert_eq!(agent(&listed, "frontend")["presence"], "idle");

    call(&frontend, "list_agents", json!({})).await;
    let listed = call(&reader, "list_agents", json!({})).await;
    assert_eq!(
        agent(&listed, "frontend")["presence"],
        "active",
        "a call makes it active again"
    );

    fx.agents.end_session(&frontend_session).await.unwrap();
    let listed = call(&reader, "list_agents", json!({})).await;
    let front = agent(&listed, "frontend");
    assert_eq!(front["presence"], "offline");
    assert_eq!(
        front["last_call_at"],
        json!(fx.now()),
        "an ended session keeps its last call"
    );
}

#[tokio::test]
async fn a_claim_past_its_lease_is_stale() {
    let fx = Fixture::new().await;
    let (_backend, _) = fx.agent("backend").await;
    let (review, _) = fx.agent("review").await;
    fx.tasks.claim_task(&actor("backend"), t(2)).await.unwrap();
    let (reader, _) = fx.connect().await;

    fx.advance(minutes(LEASE_MINUTES) - Duration::from_millis(1));
    let listed = call(&reader, "list_agents", json!({})).await;
    assert_eq!(
        agent(&listed, "backend")["workers"][0]["claims"][0]["stale"],
        false
    );

    fx.advance(Duration::from_millis(1));
    let listed = call(&reader, "list_agents", json!({})).await;
    assert_eq!(
        agent(&listed, "backend")["workers"][0]["claims"][0]["stale"],
        true
    );
    let activity = call(&review, "get_activity", json!({"agent": "backend"})).await;
    assert_eq!(activity["workers"][0]["claims"][0]["stale"], true);
}

#[tokio::test]
async fn get_activity_returns_the_workers_of_every_agent_or_of_one() {
    let fx = Fixture::new().await;
    let (frontend, _) = fx.agent("frontend").await;
    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let (_backend, _) = fx.agent("backend").await;
    let (review, _) = fx.agent("review").await;
    fx.workspaces
        .create_workspace(&actor("frontend/ui"), t(1))
        .await
        .unwrap();
    let before = fx.event_count().await;

    let all = call(&review, "get_activity", json!({})).await;
    assert_eq!(
        names(&all["workers"], "worker"),
        ["frontend", "frontend/ui", "backend", "review"]
    );
    let ui = &all["workers"][1];
    assert_eq!(ui["claims"][0]["task"], "T-1");
    assert_eq!(ui["workspaces"][0]["ws"], "ws-1");
    assert_eq!(ui["proposals"], json!([]), "empty while no proposal exists");

    let one = call(
        &review,
        "get_activity",
        json!({"agent": "frontend", "as": "review"}),
    )
    .await;
    assert_eq!(
        names(&one["workers"], "worker"),
        ["frontend", "frontend/ui"]
    );
    let backend = call(&review, "get_activity", json!({"agent": "backend"})).await;
    assert_eq!(
        backend,
        json!({"workers": [{"worker": "backend", "claims": [], "workspaces": [], "proposals": []}]})
    );
    assert_eq!(fx.event_count().await, before, "a read appends no event");

    let missing = call_err(&review, "get_activity", json!({"agent": "nobody"})).await;
    assert_eq!(missing["code"], "agent_not_found");
    assert_eq!(missing["hint"], "call list_agents to see the agents");
    let invalid = call_err(&review, "get_activity", json!({"agent": "No Body"})).await;
    assert_eq!(invalid["code"], "invalid_name");
    let (stranger, _) = fx.connect().await;
    let unregistered = call_err(&stranger, "get_activity", json!({})).await;
    assert_eq!(unregistered["code"], "agent_not_registered");
}

#[tokio::test]
async fn a_missing_workspace_a_proposal_and_a_review_claim_are_listed() {
    let fx = Fixture::new().await;
    let (frontend, _) = fx.agent("frontend").await;
    call(&frontend, "register_worker", json!({"name": "ui"})).await;
    let (review, _) = fx.agent("review").await;
    fx.workspaces
        .create_workspace(&actor("frontend/ui"), t(1))
        .await
        .unwrap();
    let now = fx.now();
    let hash = "a".repeat(64);
    execute(
        fx.project.store(),
        &format!(
            "UPDATE workspaces SET state = 'missing', changed_files = 3 WHERE number = 1;
             INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, created_at)
                 SELECT 4, id, '{hash}', 'explicit', {now} FROM workspaces WHERE number = 1;
             INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, created_at, pruned_at)
                 SELECT 5, id, '{hash}', 'auto', {now}, {now} FROM workspaces WHERE number = 1;
             INSERT INTO proposals (number, task_id, author_worker_id, title, state, close_reason, created_at, updated_at)
                 SELECT 1, t.id, w.id, 'Old attempt', 'closed', 'replaced', {now}, {now}
                 FROM tasks t, workers w WHERE t.number = 1 AND w.name = 'frontend/ui';
             INSERT INTO proposals (number, task_id, author_worker_id, title, conflicted, created_at, updated_at)
                 SELECT 2, t.id, w.id, 'Login form', 1, {now}, {now}
                 FROM tasks t, workers w WHERE t.number = 1 AND w.name = 'frontend/ui';
             INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at)
                 SELECT 'review', p.id, w.id, {now}, {now}
                 FROM proposals p, workers w WHERE p.number = 2 AND w.name = 'review';"
        ),
    )
    .await;

    let all = call(&review, "get_activity", json!({})).await;
    let ui = &all["workers"][1];
    assert_eq!(ui["worker"], "frontend/ui");
    assert_eq!(ui["workspaces"][0]["state"], "missing");
    assert_eq!(ui["workspaces"][0]["changed_files"], 3);
    assert_eq!(
        ui["workspaces"][0]["latest_checkpoint"], "ck-4",
        "a pruned checkpoint is skipped"
    );
    assert_eq!(
        ui["proposals"],
        json!([{"id": "P-2", "task": "T-1", "title": "Login form", "conflicted": true}])
    );
    let reviewer = &all["workers"][2];
    assert_eq!(reviewer["worker"], "review");
    assert_eq!(reviewer["claims"][0]["kind"], "review");
    assert_eq!(
        reviewer["claims"][0]["task"], "T-1",
        "a review claim names its proposal's task"
    );

    let (reader, _) = fx.connect().await;
    let listed = call(&reader, "list_agents", json!({})).await;
    let ui = &agent(&listed, "frontend")["workers"][1];
    assert_eq!(ui["workspaces"][0]["ws"], "ws-1");
    assert_eq!(ui["workspaces"][0]["state"], "missing");

    execute(
        fx.project.store(),
        "UPDATE workspaces SET state = 'archived' WHERE number = 1",
    )
    .await;
    let all = call(&review, "get_activity", json!({"agent": "frontend"})).await;
    assert_eq!(
        all["workers"][1]["workspaces"],
        json!([]),
        "a closed workspace is left out"
    );
}

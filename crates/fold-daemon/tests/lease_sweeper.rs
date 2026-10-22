use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::common::roles::give_role;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::events::types;
use fold_core::object::ObjectKind;
use fold_core::roles::Role;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventActor, EventFilter, ProposalId, TaskDescription,
    TaskId, TaskStatus, TaskTitle, Timestamp, WorkspaceId,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::events::LiveWorkspace;
use fold_daemon::http::router;
use fold_daemon::jobs::{JobProgress, spawn_lease_sweeper, spawn_stalls};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::BusMessage;
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::tasks::{CreateTask, sweep_leases};
use fold_daemon::services::workspaces::{WorkspaceService, check_stalls};
use fold_store::agents::WorkerRow;
use fold_store::projects::ProjectRow;
use fold_store::tasks::claims;
use fold_store::testing::{TestProject, eventually};
use fold_store::workspaces::{self, create_initial_revision, initial_revision};
use fold_store::{events, reviews};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::watch;
use tower::ServiceExt;

const WAIT: Duration = Duration::from_secs(20);
const MINUTE: Duration = Duration::from_secs(60);
/// The default `lease_minutes`.
const LEASE: Duration = Duration::from_secs(30 * 60);
/// The default `stall_minutes`.
const STALL: Duration = Duration::from_secs(5 * 60);

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

/// An agent with its open session, so its calls renew its claims as every MCP call does.
#[cfg(test)]
struct Agent {
    ctx: SessionContext,
    worker: WorkerRow,
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
    agents: AgentService,
    workspaces: WorkspaceService,
    frontend: Agent,
    backend: Agent,
}

#[cfg(test)]
impl Fixture {
    /// The project open as `demo` with the default configuration: `r1` holds `README.md`, tasks `T-1` and `T-2`, agents `frontend` and `backend`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let hash = objects.write(ObjectKind::Blob, b"# demo\n").unwrap();
        let mut tree = FlatTree::new();
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
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1:0".parse().unwrap(),
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        let agents = handle.agents();
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        // `frontend` leads the area of the tasks; `backend` stays free to be a reviewer.
        let (lead, area) = crate::common::roles::WORK_AREAS[0];
        give_role(&handle.store, lead, Role::AreaLead, Some(area)).await;
        for _ in 0..2 {
            handle
                .tasks
                .create_task(
                    &by(&frontend.worker),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: vec![fold_core::LabelName::parse(area).unwrap()],
                    },
                )
                .await
                .unwrap();
        }
        let workspaces = WorkspaceService::for_project(&handle);
        Self {
            project,
            state,
            handle,
            agents,
            workspaces,
            frontend,
            backend,
        }
    }

    fn advance(&self, by: Duration) -> Timestamp {
        self.project.clock().advance(by).unwrap();
        self.project.now()
    }

    /// Creates the workspace of `task` for `agent`, which claims the task, and returns its id and directory.
    async fn create(&self, agent: &Agent, task: u32) -> (WorkspaceId, PathBuf) {
        let view = self
            .workspaces
            .create_workspace(&by(&agent.worker), t(task))
            .await
            .unwrap()
            .workspace;
        (view.id, self.project.paths().root().join(view.path))
    }

    /// One tool call of `agent`, which renews every claim of the agent.
    async fn call(&self, agent: &Agent) {
        self.agents
            .record_call(&agent.ctx, &agent.worker, "get_task", None)
            .await
            .unwrap();
    }

    async fn sweep(&self) -> usize {
        sweep_leases(&self.handle, self.project.now())
            .await
            .unwrap()
            .len()
    }

    async fn check(&self) -> usize {
        check_stalls(&self.handle, self.project.now())
            .await
            .unwrap()
    }

    async fn stale_events(&self) -> Vec<Event> {
        let filter = EventFilter {
            types: vec![types::CLAIM_STALE],
            ..EventFilter::default()
        };
        events::read_after(self.project.store().read(), 0, &filter, 1000)
            .await
            .unwrap()
    }

    async fn stale_at(&self, task: u32) -> Option<Timestamp> {
        claims::unreleased_for_task(self.project.store().read(), t(task))
            .await
            .unwrap()
            .unwrap()
            .stale_at
    }

    fn stalled(&self, workspace: WorkspaceId) -> bool {
        self.handle.live.get(workspace).unwrap().view.stalled
    }

    /// The attention kinds of each task card on `GET /board`, by task id.
    async fn board(&self) -> BTreeMap<String, Vec<String>> {
        let board = self.get_json("/api/v1/projects/demo/board").await;
        board["columns"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|column| column["cards"].as_array().unwrap().iter())
            .map(|card| {
                let kinds = card["attention"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["kind"].as_str().unwrap().to_owned())
                    .collect();
                (card["task"].as_str().unwrap().to_owned(), kinds)
            })
            .collect()
    }

    /// The `attention` list of each agent on `GET /agents`, by agent name.
    async fn agent_attention(&self) -> BTreeMap<String, Value> {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/projects/demo/agents")
            .body(Body::empty())
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        body["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|agent| {
                (
                    agent["name"].as_str().unwrap().to_owned(),
                    agent["attention"].clone(),
                )
            })
            .collect()
    }

    async fn attention(&self, task: &str) -> Vec<String> {
        self.board().await.remove(task).unwrap()
    }

    /// The attention kinds of each entry on `GET /workspaces` and of `id` on `GET /workspaces/{id}`, which must agree.
    async fn workspace_attention(&self, id: WorkspaceId) -> Vec<String> {
        let kinds = |entry: &Value| -> Vec<String> {
            entry["attention"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["kind"].as_str().unwrap().to_owned())
                .collect()
        };
        let list = self.get_json("/api/v1/projects/demo/workspaces").await;
        let listed = list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == json!(id))
            .map(kinds)
            .unwrap();
        let one = self
            .get_json(&format!("/api/v1/projects/demo/workspaces/{id}"))
            .await;
        assert_eq!(kinds(&one), listed);
        listed
    }

    async fn get_json(&self, uri: &str) -> Value {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// The task, its status and its board column.
    async fn place(&self, task: u32) -> (TaskStatus, String) {
        let status = self.handle.tasks.get(t(task)).await.unwrap().status;
        let board = self.handle.tasks.board().await.unwrap();
        let column = board
            .columns
            .iter()
            .find(|column| column.cards.iter().any(|card| card.id == t(task)))
            .unwrap();
        (status, column.status.as_str().to_owned())
    }

    /// Opens a proposal of `T-1` from its workspace and gives `backend`, as a reviewer, its review claim.
    async fn review_claim(&self, dir: &std::path::Path) -> ProposalId {
        let reviews = ReviewService::new(
            self.handle.store.clone(),
            self.handle.bus.clone(),
            self.handle.clock.clone(),
            self.workspaces.clone(),
            self.handle.integration.clone(),
        );
        fs::write(dir.join("README.md"), "# demo\nproposed\n").unwrap();
        let proposal = reviews
            .create_proposal(&by(&self.frontend.worker), t(1), "Change the readme", None)
            .await
            .unwrap()
            .detail()
            .unwrap()
            .summary
            .id;
        give_role(&self.handle.store, "backend", Role::Reviewer, None).await;
        crate::common::roles::give_review_claim(
            &self.handle.store,
            "backend",
            proposal,
            self.project.now(),
        )
        .await;
        proposal
    }
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> Agent {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    let worker = agents.resolve_worker(&ctx).await.unwrap();
    Agent { ctx, worker }
}

/// The durable `claim.stale` events and the live workspace states the bus holds right now.
#[cfg(test)]
fn drain(receiver: &mut Receiver<BusMessage>) -> (Vec<Event>, Vec<LiveWorkspace>) {
    let (mut stale, mut live) = (Vec::new(), Vec::new());
    loop {
        match receiver.try_recv() {
            Ok(BusMessage::Durable(event)) if event.event_type == types::CLAIM_STALE => {
                stale.push(Event::clone(&event));
            }
            Ok(BusMessage::Live(event)) if event.event_type == types::WORKSPACE_CHANGED => {
                live.push(serde_json::from_value(event.data.clone()).unwrap());
            }
            Ok(_) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return (stale, live),
            Err(TryRecvError::Lagged(_)) => panic!("the test subscriber lagged"),
        }
    }
}

#[tokio::test]
async fn a_claim_idle_for_the_lease_gets_one_stale_event() {
    let fx = Fixture::new().await;
    let claimed_at = fx.project.now();
    fx.handle
        .tasks
        .claim_task(&by(&fx.frontend.worker), t(1))
        .await
        .unwrap();
    let mut bus = fx.handle.bus.subscribe();

    fx.advance(LEASE - Duration::from_millis(1));
    assert_eq!(fx.sweep().await, 0, "just inside the lease");
    assert_eq!(fx.stale_at(1).await, None);

    let stale_at = fx.advance(Duration::from_millis(1));
    assert_eq!(fx.sweep().await, 1);
    assert_eq!(fx.stale_at(1).await, Some(stale_at));
    let stored = fx.stale_events().await;
    assert_eq!(stored.len(), 1);
    let event = &stored[0];
    assert_eq!(event.actor, EventActor::fold());
    assert_eq!(event.task, Some(t(1)));
    assert_eq!(event.proposal, None);
    assert_eq!(
        event.data,
        json!({
            "kind": "implementation",
            "worker": "frontend",
            "renewed_at": claimed_at.as_millis(),
        })
    );
    assert_eq!(event.created_at, stale_at);
    let (published, _) = drain(&mut bus);
    assert_eq!(published, stored, "published after the commit");

    fx.advance(LEASE);
    assert_eq!(fx.sweep().await, 0, "a marked claim is not reported again");
    assert_eq!(fx.stale_events().await.len(), 1);
    assert_eq!(fx.stale_at(1).await, Some(stale_at));
    assert!(drain(&mut bus).0.is_empty());
}

#[tokio::test]
async fn a_renewal_clears_the_mark_and_a_later_lapse_marks_it_again() {
    let fx = Fixture::new().await;
    fx.handle
        .tasks
        .claim_task(&by(&fx.frontend.worker), t(1))
        .await
        .unwrap();
    fx.advance(LEASE);
    assert_eq!(fx.sweep().await, 1);
    assert!(fx.stale_at(1).await.is_some());

    let renewed_at = fx.advance(MINUTE);
    fx.call(&fx.frontend).await;
    assert_eq!(fx.stale_at(1).await, None, "the renewal clears stale_at");
    fx.advance(LEASE - MINUTE);
    assert_eq!(fx.sweep().await, 0);

    let again = fx.advance(MINUTE);
    assert_eq!(fx.sweep().await, 1);
    assert_eq!(fx.stale_at(1).await, Some(again));
    let stored = fx.stale_events().await;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[1].data["renewed_at"], json!(renewed_at.as_millis()));
}

#[tokio::test]
async fn a_stale_claim_keeps_its_workspace_status_and_column() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let before = fx.place(1).await;
    assert_eq!(before.0, TaskStatus::InProgress);

    fx.advance(LEASE);
    assert_eq!(fx.sweep().await, 1);
    let claim = claims::unreleased_for_task(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.worker, fx.frontend.worker.name);
    let row = workspaces::get_workspace(fx.project.store().read(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state.as_str(), "active");
    assert_eq!(row.worker.as_ref(), Some(&fx.frontend.worker.name));
    assert!(dir.is_dir());
    assert!(fx.handle.live.get(id).is_some());
    assert_eq!(fx.place(1).await, before);
}

#[tokio::test]
async fn a_review_claim_is_marked_and_reported_like_a_task_claim() {
    let fx = Fixture::new().await;
    let (_, dir) = fx.create(&fx.frontend, 1).await;
    let proposal = fx.review_claim(&dir).await;
    let reviewed_at = fx.project.now();

    fx.advance(LEASE);
    fx.call(&fx.frontend).await;
    assert_eq!(fx.sweep().await, 1, "only the review claim lapsed");
    let stored = fx.stale_events().await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].task, Some(t(1)));
    assert_eq!(stored[0].proposal, Some(proposal));
    assert_eq!(
        stored[0].data,
        json!({
            "kind": "review",
            "proposal": proposal.to_string(),
            "worker": "backend",
            "renewed_at": reviewed_at.as_millis(),
        })
    );
    let review = reviews::unreleased_review_claim(fx.project.store().read(), proposal)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review.stale_at, Some(fx.project.now()));
    assert_eq!(fx.stale_at(1).await, None);
    assert_eq!(fx.attention("T-1").await, ["stale_claim"]);
    assert_eq!(fx.sweep().await, 0);

    fx.call(&fx.backend).await;
    let review = reviews::unreleased_review_claim(fx.project.store().read(), proposal)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review.stale_at, None);
    assert!(fx.attention("T-1").await.is_empty());
}

#[tokio::test]
async fn a_worker_without_content_changes_stalls_until_a_content_change() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let mut bus = fx.handle.bus.subscribe();
    assert!(!fx.stalled(id));

    fx.advance(STALL - Duration::from_millis(1));
    assert_eq!(fx.check().await, 0, "just inside the window");
    assert!(!fx.stalled(id));

    fx.advance(Duration::from_millis(1));
    assert_eq!(fx.check().await, 1, "counted from the creation time");
    assert!(fx.stalled(id));
    let (_, live) = drain(&mut bus);
    assert_eq!(live.len(), 1);
    assert!(live[0].stalled);
    assert_eq!(fx.check().await, 0, "an unchanged flag publishes nothing");
    assert!(drain(&mut bus).1.is_empty());

    fx.advance(MINUTE);
    let readme = OpenOptions::new()
        .write(true)
        .open(dir.join("README.md"))
        .unwrap();
    readme
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(3_600))
        .unwrap();
    drop(readme);
    let touched = fx.handle.live.rescan(id).await.unwrap();
    assert!(
        touched.view.stalled,
        "a touch without a change is no activity"
    );
    assert_eq!(fx.check().await, 0);

    fx.advance(MINUTE);
    fs::write(dir.join("README.md"), "# demo\nedited\n").unwrap();
    let edited = fx.handle.live.rescan(id).await.unwrap();
    assert!(
        !edited.view.stalled,
        "the scan that found the change clears it"
    );
    assert!(
        !fx.handle
            .live
            .set_stalled(id, touched.view.last_activity_at, true),
        "a decision from before the change is dropped"
    );
    assert!(!fx.stalled(id));
    assert_eq!(fx.check().await, 0);
    let (_, live) = drain(&mut bus);
    assert!(!live.last().unwrap().stalled);

    fx.advance(STALL);
    assert_eq!(fx.check().await, 1, "counted from the last change");
    assert!(fx.stalled(id));
}

#[tokio::test]
async fn the_board_and_the_workspace_routes_show_stalled_and_stale_claim_until_each_clears() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    assert!(fx.attention("T-1").await.is_empty());

    fx.advance(STALL);
    fx.check().await;
    assert_eq!(fx.attention("T-1").await, ["stalled"]);
    assert!(fx.attention("T-2").await.is_empty());

    fx.advance(LEASE);
    fx.sweep().await;
    assert_eq!(fx.attention("T-1").await, ["stalled", "stale_claim"]);
    assert_eq!(fx.workspace_attention(id).await, ["stalled", "stale_claim"]);
    let detail = fx.handle.tasks.get_view(t(1)).await.unwrap();
    let kinds: Vec<&str> = detail
        .attention
        .iter()
        .map(|item| item.kind.as_str())
        .collect();
    assert_eq!(kinds, ["stalled", "stale_claim"]);

    fx.call(&fx.frontend).await;
    assert_eq!(fx.attention("T-1").await, ["stalled"]);
    assert_eq!(fx.workspace_attention(id).await, ["stalled"]);

    fs::write(dir.join("README.md"), "# demo\nedited\n").unwrap();
    fx.handle.live.rescan(id).await.unwrap();
    assert!(fx.attention("T-1").await.is_empty());
    assert!(fx.workspace_attention(id).await.is_empty());
}

#[tokio::test]
async fn the_agents_route_gives_each_agent_the_items_of_its_own_claims() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    fx.review_claim(&dir).await;
    // An edit after the version, so the author's workspace stalls while the reviewer holds the proposal.
    fs::write(dir.join("README.md"), "# demo\nafter the version\n").unwrap();
    fx.handle.live.rescan(id).await.unwrap();
    let none = BTreeMap::from([
        ("frontend".to_owned(), json!([])),
        ("backend".to_owned(), json!([])),
    ]);
    assert_eq!(fx.agent_attention().await, none);

    fx.advance(STALL);
    fx.check().await;
    assert!(fx.stalled(id));
    assert_eq!(
        fx.agent_attention().await,
        BTreeMap::from([
            (
                "frontend".to_owned(),
                json!([{"kind": "stalled", "task": "T-1"}])
            ),
            ("backend".to_owned(), json!([])),
        ]),
        "the reviewer of a stalled task is not stalled"
    );

    fx.advance(LEASE);
    fx.call(&fx.frontend).await;
    assert_eq!(fx.sweep().await, 1, "only the review claim lapsed");
    assert_eq!(
        fx.agent_attention().await,
        BTreeMap::from([
            (
                "frontend".to_owned(),
                json!([{"kind": "stalled", "task": "T-1"}])
            ),
            (
                "backend".to_owned(),
                json!([{"kind": "stale_claim", "task": "T-1"}])
            ),
        ]),
        "a lapsed review claim marks only its holder"
    );

    fx.call(&fx.backend).await;
    fs::write(dir.join("README.md"), "# demo\nedited again\n").unwrap();
    fx.handle.live.rescan(id).await.unwrap();
    assert_eq!(fx.agent_attention().await, none);
}

#[tokio::test]
async fn a_stalled_worker_that_keeps_calling_keeps_a_fresh_lease() {
    let fx = Fixture::new().await;
    let (id, _) = fx.create(&fx.frontend, 1).await;
    for _ in 0..8 {
        fx.advance(STALL);
        fx.call(&fx.frontend).await;
        fx.sweep().await;
        fx.check().await;
    }
    assert!(fx.stalled(id), "calls are not file changes");
    assert_eq!(fx.stale_at(1).await, None, "file changes are not calls");
    assert!(fx.stale_events().await.is_empty());
    let claim = claims::unreleased_for_task(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.renewed_at, fx.project.now());
    assert_eq!(fx.attention("T-1").await, ["stalled"]);
}

#[tokio::test]
async fn a_worker_whose_files_are_its_sent_version_never_stalls() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    fs::write(dir.join("README.md"), "# demo\nsent\n").unwrap();
    fx.handle.live.rescan(id).await.unwrap();
    fx.advance(STALL);
    assert_eq!(fx.check().await, 1);
    assert!(fx.stalled(id));

    let reviews = fx.handle.reviews(fx.project.registry());
    let proposal = reviews
        .create_proposal(&by(&fx.frontend.worker), t(1), "Change the readme", None)
        .await
        .unwrap()
        .detail()
        .unwrap()
        .summary
        .id;
    fx.handle.live.rescan(id).await.unwrap();
    assert!(
        !fx.stalled(id),
        "the scan after the proposal clears it without a new edit"
    );
    assert!(fx.attention("T-1").await.is_empty());
    fx.advance(STALL);
    assert_eq!(
        fx.check().await,
        0,
        "the version waits, the worker does not stall"
    );
    assert!(!fx.stalled(id));
    assert!(fx.attention("T-1").await.is_empty());

    fs::write(dir.join("README.md"), "# demo\nedited after\n").unwrap();
    fx.handle.live.rescan(id).await.unwrap();
    fx.advance(STALL);
    assert_eq!(
        fx.check().await,
        1,
        "an edit after the version counts again"
    );
    assert!(fx.stalled(id));
    assert_eq!(fx.attention("T-1").await, ["stalled"]);

    fs::write(dir.join("README.md"), "# demo\nsent\n").unwrap();
    fx.handle.live.rescan(id).await.unwrap();
    assert_eq!(
        fx.check().await,
        0,
        "the scan that found the change cleared it"
    );
    assert!(!fx.stalled(id));
    assert_eq!(
        fx.handle.live.get(id).unwrap().view.proposal,
        Some(proposal)
    );
}

#[tokio::test]
async fn a_released_claim_or_a_missing_directory_never_stalls() {
    let fx = Fixture::new().await;
    let (released, _) = fx.create(&fx.frontend, 1).await;
    let (missing, missing_dir) = fx.create(&fx.frontend, 2).await;
    fx.advance(STALL);
    assert_eq!(fx.check().await, 2);

    fx.handle
        .tasks
        .release_task(&by(&fx.frontend.worker), t(1))
        .await
        .unwrap();
    fs::remove_dir_all(&missing_dir).unwrap();
    assert!(fx.handle.live.rescan(missing).await.is_err());
    let gone = fx.handle.live.get(missing).unwrap();
    assert!(gone.missing);
    assert!(!gone.view.stalled, "the missing directory clears it");
    assert!(
        !fx.handle
            .live
            .set_stalled(missing, gone.view.last_activity_at, true),
        "a missing workspace never takes the flag"
    );
    assert_eq!(fx.check().await, 1, "the released claim clears it");
    assert!(!fx.stalled(released));

    fx.advance(STALL);
    assert_eq!(fx.check().await, 0);
    assert!(!fx.stalled(released));
    assert!(!fx.stalled(missing));
    let none: Vec<String> = Vec::new();
    assert_eq!(fx.attention("T-1").await, none);
    assert_eq!(fx.attention("T-2").await, none);
}

#[tokio::test]
async fn an_invalid_configuration_fails_the_sweep_and_the_check() {
    let fx = Fixture::new().await;
    fs::write(fx.project.paths().config(), "stall_minutes = \"soon\"\n").unwrap();
    let error = check_stalls(&fx.handle, fx.project.now())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("invalid_config"));
    let error = sweep_leases(&fx.handle, fx.project.now())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("invalid_config"));
}

#[tokio::test]
async fn the_jobs_sweep_leases_and_check_stalls_of_every_open_project() {
    let fx = Fixture::new().await;
    let (id, _) = fx.create(&fx.frontend, 1).await;
    let (stop, stopped) = watch::channel(false);
    let leases = spawn_lease_sweeper(fx.state.clone(), Duration::from_millis(5), stopped.clone());
    let stalls = spawn_stalls(fx.state.clone(), Duration::from_millis(5), stopped);
    let wait = async |progress: &mut watch::Receiver<JobProgress>,
                      done: &dyn Fn(&JobProgress) -> bool| {
        tokio::time::timeout(WAIT, progress.wait_for(|seen| done(seen)))
            .await
            .unwrap()
            .map(|seen| *seen)
            .unwrap()
    };
    let (mut lease_progress, mut stall_progress) = (leases.progress(), stalls.progress());
    let idle = wait(&mut lease_progress, &|seen| seen.checks >= 3).await;
    assert_eq!(idle.cycles, 0, "no lease ran out yet");
    let idle = wait(&mut stall_progress, &|seen| seen.checks >= 3).await;
    assert_eq!(idle.cycles, 0, "no workspace is quiet long enough yet");

    fx.advance(LEASE);
    wait(&mut lease_progress, &|seen| seen.cycles >= 1).await;
    wait(&mut stall_progress, &|seen| seen.cycles >= 1).await;
    eventually(WAIT, async || match fx.stale_events().await.len() {
        1 => Ok(()),
        other => Err(other),
    })
    .await;
    assert!(fx.stalled(id));
    stop.send(true).unwrap();
    leases.stopped(WAIT).await;
    stalls.stopped(WAIT).await;
    assert_eq!(fx.stale_events().await.len(), 1);
}

#[tokio::test]
async fn a_sent_version_that_waits_for_review_or_integration_never_stalls() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let proposal = fx.review_claim(&dir).await;
    fx.handle.live.rescan(id).await.unwrap();
    fx.advance(STALL);
    assert_eq!(fx.check().await, 0, "open, it waits for the review");
    fx.handle
        .reviews(fx.project.registry())
        .submit_review(
            &by(&fx.backend.worker),
            proposal,
            1,
            fold_core::reviews::Verdict::Approve,
            "fine",
            Vec::new(),
        )
        .await
        .unwrap();
    fx.advance(STALL * 3);
    assert_eq!(
        fx.check().await,
        0,
        "approved, it waits for the integration"
    );
    assert!(!fx.stalled(id));
    assert!(fx.attention("T-1").await.is_empty());
}

#[tokio::test]
async fn requested_changes_stall_the_author_from_the_verdict_without_an_edit() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let proposal = fx.review_claim(&dir).await;
    fx.handle.live.rescan(id).await.unwrap();
    fx.advance(STALL * 2);
    assert_eq!(fx.check().await, 0, "the version waits for the review");
    fx.handle
        .reviews(fx.project.registry())
        .submit_review(
            &by(&fx.backend.worker),
            proposal,
            1,
            fold_core::reviews::Verdict::RequestChanges,
            "rename it",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        fx.check().await,
        0,
        "the window counts from the verdict, not from the last edit"
    );
    fx.advance(STALL - Duration::from_secs(1));
    assert_eq!(fx.check().await, 0);
    assert!(!fx.stalled(id));
    fx.advance(Duration::from_secs(1));
    assert_eq!(fx.check().await, 1, "back with its author, nothing changed");
    assert!(fx.stalled(id));
    assert_eq!(fx.attention("T-1").await, ["stalled"]);
    assert_eq!(
        fx.handle.live.get(id).unwrap().view.edits_after_version,
        Some(false)
    );
    fx.handle.live.rescan(id).await.unwrap();
    assert!(fx.stalled(id), "a rescan without an edit leaves the flag");
}

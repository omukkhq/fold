use std::path::{Path, PathBuf};
use std::sync::Arc;

use fold_core::events::types;
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventActor, EventFilter, EventType, FoldError, HumanName,
    RevisionId, TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName, WorkspaceId,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::{BusMessage, EventBuffer, EventBus};
use fold_daemon::services::tasks::{self, CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::{
    self, ArchiveReason, WorkspaceError, WorkspaceRef, WorkspaceService, WorkspaceView,
};
use fold_store::agents::WorkerRow;
use fold_store::tasks::claims;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    WorkspaceKind, WorkspaceState, create_initial_revision, initial_revision,
};
use fold_store::{StoreError, events};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use serde_json::json;
use sqlx::AssertSqlSafe;

const LEASE_MINUTES: u64 = 5;
const LIB: &[u8] = b"pub fn x() {}\n";
const README: &[u8] = b"# demo\n";

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    tasks: TaskService,
    workspaces: WorkspaceService,
    frontend: WorkerRow,
    backend: WorkerRow,
}

#[cfg(test)]
impl Fixture {
    /// `r1` holds `src/lib.rs` and `README.md`. Three tasks, the agents `frontend` and `backend`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        for (path, content) in [("src/lib.rs", LIB), ("README.md", README)] {
            let hash = objects.write(ObjectKind::Blob, content).unwrap();
            tree.insert(
                path.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash,
                },
            );
        }
        let tree = write_initial_tree(&objects, tree).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            LEASE_MINUTES,
        );
        let workspaces = WorkspaceService::new(
            project.store().clone(),
            bus.clone(),
            clock,
            project.paths().clone(),
            Git::default(),
            LEASE_MINUTES,
        );
        let creator = agent("frontend");
        for _ in 0..3 {
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
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        Self {
            project,
            bus,
            tasks,
            workspaces,
            frontend,
            backend,
        }
    }

    fn dir(&self, path: &str) -> PathBuf {
        self.project.paths().root().join(path)
    }

    async fn create(&self, worker: &WorkerRow, task: u32) -> Result<WorkspaceView, WorkspaceError> {
        self.workspaces.create_workspace(&by(worker), t(task)).await
    }

    async fn abandon(
        &self,
        worker: &WorkerRow,
        workspace: u32,
    ) -> Result<WorkspaceView, WorkspaceError> {
        self.workspaces
            .abandon_workspace(&by(worker), ws(workspace), "the approach does not work")
            .await
    }

    async fn status(&self, task: u32) -> (TaskStatus, bool) {
        let view = self.tasks.get(t(task)).await.unwrap();
        (view.status, view.blocked)
    }

    async fn events(&self) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            0,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
    }

    async fn last_seq(&self) -> u64 {
        self.events().await.last().map_or(0, |event| event.seq)
    }

    async fn events_after(&self, seq: u64) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            seq,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }

    async fn execute(&self, sql: &str) {
        self.project
            .store()
            .write(async |tx| {
                sqlx::raw_sql(AssertSqlSafe(sql)).execute(tx.conn()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }

    async fn recompute(&self, task: u32) {
        let now = self.project.now();
        self.project
            .store()
            .write(async |tx| {
                tasks::recompute(tx, &[t(task)], now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }

    /// The names under `ws/`, sorted.
    fn ws_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = match std::fs::read_dir(self.project.paths().ws_dir()) {
            Ok(entries) => entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }
}

/// The acting agent of a worker row, as `AgentService::actor` builds it.
#[cfg(test)]
fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> WorkerRow {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    agents.resolve_worker(&ctx, None).await.unwrap()
}

#[cfg(test)]
fn refusal(error: WorkspaceError) -> FoldError {
    match error {
        WorkspaceError::Fold(error) | WorkspaceError::Task(TaskError::Fold(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
fn details(error: &FoldError) -> serde_json::Value {
    error
        .details()
        .cloned()
        .map_or(serde_json::Value::Null, serde_json::Value::Object)
}

#[cfg(test)]
fn of_type(events: &[Event], event_type: EventType) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

#[cfg(test)]
fn names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

#[tokio::test]
async fn create_takes_a_free_claim_writes_the_head_and_moves_the_task() {
    let fx = Fixture::new().await;
    let mut bus = fx.bus.subscribe();
    let before = fx.last_seq().await;
    let view = fx.create(&fx.frontend, 1).await.unwrap();

    assert_eq!(view.id, ws(1));
    assert_eq!(view.task, t(1));
    assert_eq!(view.path, "ws/t-1");
    assert_eq!(view.base, RevisionId::from_number(1).unwrap());
    assert_eq!(view.kind, WorkspaceKind::Task);
    assert_eq!(view.state, WorkspaceState::Active);
    assert_eq!(view.worker, Some(WorkerName::parse("frontend").unwrap()));
    assert_eq!(view.created_at, fx.project.now());
    assert_eq!(view.last_activity_at, Some(fx.project.now()));
    assert_eq!(view.closed_at, None);

    let dir = fx.dir("ws/t-1");
    assert_eq!(std::fs::read(dir.join("src/lib.rs")).unwrap(), LIB);
    assert_eq!(std::fs::read(dir.join("README.md")).unwrap(), README);
    assert_eq!(fx.ws_entries(), vec!["t-1"]);
    assert!(fx.project.paths().workspace_index(ws(1)).is_file());

    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        vec!["claim.acquired", "workspace.created", "task.status_changed"]
    );
    let created = &appended[1];
    assert_eq!(created.actor.as_str(), "frontend");
    assert_eq!(created.task, Some(t(1)));
    assert_eq!(created.workspace, Some(ws(1)));
    assert_eq!(created.revision, RevisionId::from_number(1));
    assert_eq!(
        created.data,
        json!({"path": "ws/t-1", "worker": "frontend"})
    );
    assert_eq!(
        appended[2].data,
        json!({"from": "todo", "to": "in_progress"})
    );
    for event in &appended {
        match bus.try_recv().unwrap() {
            BusMessage::Durable(published) => assert_eq!(published.seq, event.seq),
            BusMessage::Live(_) => panic!("a create sends no live event"),
        }
    }

    assert_eq!(fx.status(1).await, (TaskStatus::InProgress, false));
    let claim = claims::unreleased_for_task(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.worker_id, fx.frontend.id);
    assert_eq!(
        fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap(),
        view
    );
}

#[tokio::test]
async fn the_claim_holder_creates_without_a_second_claim_event() {
    let fx = Fixture::new().await;
    fx.tasks.claim_task(&by(&fx.frontend), t(1)).await.unwrap();
    let before = fx.last_seq().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    assert_eq!(
        names(&fx.events_after(before).await),
        vec!["workspace.created", "task.status_changed"]
    );
}

#[tokio::test]
async fn another_workers_claim_refuses_the_create() {
    let fx = Fixture::new().await;
    fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    let before = fx.last_seq().await;
    let error = refusal(fx.create(&fx.frontend, 1).await.unwrap_err());
    assert_eq!(error.code(), "already_claimed");
    assert_eq!(
        details(&error),
        json!({"holder": "backend", "stale": false})
    );
    assert!(fx.events_after(before).await.is_empty());
    assert!(fx.ws_entries().is_empty());
    assert_eq!(fx.count("SELECT COUNT(*) FROM workspaces").await, 0);
    assert_eq!(fx.status(1).await, (TaskStatus::Todo, false));
}

#[tokio::test]
async fn only_an_agent_creates_a_workspace() {
    let fx = Fixture::new().await;
    let before = fx.last_seq().await;
    let human = Actor::human(HumanName::parse("tanim").unwrap());
    for actor in [human, Actor::System] {
        let error = refusal(
            fx.workspaces
                .create_workspace(&actor, t(1))
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), "actor_cannot_claim");
    }
    assert!(fx.events_after(before).await.is_empty());
    assert!(fx.ws_entries().is_empty());
    assert_eq!(fx.status(1).await, (TaskStatus::Todo, false));
}

#[tokio::test]
async fn a_second_create_returns_workspace_exists() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    let error = refusal(fx.create(&fx.frontend, 1).await.unwrap_err());
    assert_eq!(error.code(), "workspace_exists");
    assert_eq!(
        details(&error),
        json!({"workspace": "ws-1", "path": "ws/t-1"})
    );
    assert!(error.hint().unwrap().contains("get_workspace"));
    assert_eq!(fx.ws_entries(), vec!["t-1"]);
}

#[tokio::test]
async fn unknown_and_archived_tasks_are_refused() {
    let fx = Fixture::new().await;
    let error = fx.create(&fx.frontend, 9).await.unwrap_err();
    assert_eq!(error.code(), Some("task_not_found"));
    fx.tasks
        .archive_task(&agent("frontend"), t(2))
        .await
        .unwrap();
    let error = refusal(fx.create(&fx.frontend, 2).await.unwrap_err());
    assert_eq!(error.code(), "task_closed");
    assert_eq!(details(&error), json!({"reason": "archived"}));
    assert!(fx.ws_entries().is_empty());
}

#[tokio::test]
async fn abandon_returns_the_task_to_todo_and_leaves_the_directory() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    let edited = fx.dir("ws/t-1/notes.txt");
    std::fs::write(&edited, b"draft").unwrap();
    let before = fx.last_seq().await;
    fx.project
        .clock()
        .advance(std::time::Duration::from_secs(60))
        .unwrap();

    let view = fx.abandon(&fx.frontend, 1).await.unwrap();
    assert_eq!(view.state, WorkspaceState::Abandoned);
    assert_eq!(view.closed_at, Some(fx.project.now()));
    assert_eq!(fx.status(1).await, (TaskStatus::Todo, false));
    assert_eq!(std::fs::read(&edited).unwrap(), b"draft");
    assert_eq!(std::fs::read(fx.dir("ws/t-1/src/lib.rs")).unwrap(), LIB);
    let claim = claims::unreleased_for_task(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.worker_id, fx.frontend.id);

    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        vec!["workspace.abandoned", "task.status_changed"]
    );
    assert_eq!(appended[0].workspace, Some(ws(1)));
    assert_eq!(appended[0].task, Some(t(1)));
    assert_eq!(
        appended[0].data,
        json!({"reason": "the approach does not work"})
    );
    assert_eq!(
        appended[1].data,
        json!({"from": "in_progress", "to": "todo"})
    );
}

#[tokio::test]
async fn a_backlog_task_takes_a_workspace_and_returns_to_backlog() {
    let fx = Fixture::new().await;
    let actor = agent("frontend");
    fx.tasks.add_dependency(&actor, t(2), t(3)).await.unwrap();
    assert_eq!(fx.status(2).await, (TaskStatus::Backlog, false));
    fx.create(&fx.frontend, 2).await.unwrap();
    assert_eq!(fx.status(2).await, (TaskStatus::InProgress, true));
    fx.abandon(&fx.frontend, 1).await.unwrap();
    assert_eq!(fx.status(2).await, (TaskStatus::Backlog, false));
}

#[tokio::test]
async fn a_missing_workspace_keeps_the_task_in_progress() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    fx.execute("UPDATE workspaces SET state = 'missing'").await;
    fx.recompute(1).await;
    assert_eq!(fx.status(1).await, (TaskStatus::InProgress, false));
    let error = refusal(fx.create(&fx.frontend, 1).await.unwrap_err());
    assert_eq!(error.code(), "workspace_exists");
    let view = fx.abandon(&fx.frontend, 1).await.unwrap();
    assert_eq!(view.state, WorkspaceState::Abandoned);
    assert_eq!(fx.status(1).await, (TaskStatus::Todo, false));
}

#[tokio::test]
async fn abandon_refuses_a_closed_workspace_another_worker_and_bad_input() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();

    let error = refusal(fx.abandon(&fx.backend, 1).await.unwrap_err());
    assert_eq!(error.code(), "not_workspace_owner");
    assert_eq!(
        details(&error),
        json!({"workspace": "ws-1", "owner": "frontend"})
    );
    let human = Actor::human(HumanName::parse("tanim").unwrap());
    for actor in [human, Actor::System] {
        let error = refusal(
            fx.workspaces
                .abandon_workspace(&actor, ws(1), "stop")
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), "not_workspace_owner");
    }

    let error = refusal(fx.abandon(&fx.frontend, 7).await.unwrap_err());
    assert_eq!(error.code(), "workspace_not_found");

    for reason in ["   ", &"x".repeat(501)] {
        let error = refusal(
            fx.workspaces
                .abandon_workspace(&by(&fx.frontend), ws(1), reason)
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), "invalid_request");
        assert_eq!(details(&error), json!({"field": "reason"}));
    }

    fx.abandon(&fx.frontend, 1).await.unwrap();
    let error = refusal(fx.abandon(&fx.frontend, 1).await.unwrap_err());
    assert_eq!(error.code(), "workspace_not_active");
    assert_eq!(
        details(&error),
        json!({"workspace": "ws-1", "state": "abandoned"})
    );
}

#[tokio::test]
async fn abandon_with_an_open_proposal_changes_nothing_until_it_is_closed() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    fx.execute(&format!(
        "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
         VALUES (1, (SELECT id FROM tasks WHERE number = 1), {}, 'proposal', 1, 1)",
        fx.frontend.id
    ))
    .await;
    fx.recompute(1).await;
    let status = fx.status(1).await;
    let before = fx.last_seq().await;

    let error = refusal(fx.abandon(&fx.frontend, 1).await.unwrap_err());
    assert_eq!(error.code(), "invalid_request");
    assert_eq!(
        details(&error),
        json!({"reason": "proposal_open", "task": "T-1"})
    );
    assert!(error.hint().unwrap().contains("close_proposal"));
    assert!(fx.events_after(before).await.is_empty());
    assert_eq!(fx.status(1).await, status);
    let view = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(view.state, WorkspaceState::Active);

    fx.execute("UPDATE proposals SET state = 'closed', close_reason = 'replaced'")
        .await;
    fx.recompute(1).await;
    let view = fx.abandon(&fx.frontend, 1).await.unwrap();
    assert_eq!(view.state, WorkspaceState::Abandoned);
    assert_eq!(fx.status(1).await, (TaskStatus::Todo, false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_creating_at_once_yield_one_workspace() {
    let fx = Fixture::new().await;
    let (first, second) = tokio::join!(fx.create(&fx.frontend, 1), fx.create(&fx.backend, 1));
    let (won, lost) = match (first, second) {
        (Ok(view), Err(error)) | (Err(error), Ok(view)) => (view, refusal(error)),
        other => panic!("expected one create to win, got {other:?}"),
    };
    assert!(
        ["already_claimed", "workspace_exists"].contains(&lost.code()),
        "{lost:?}"
    );
    assert_only_the_winner_is_on_disk(&fx, &won);
    assert_eq!(fx.count("SELECT COUNT(*) FROM workspaces").await, 1);
    assert_eq!(
        of_type(&fx.events().await, types::WORKSPACE_CREATED).len(),
        1
    );
    assert_eq!(of_type(&fx.events().await, types::CLAIM_ACQUIRED).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_worker_creating_twice_at_once_yields_one_workspace() {
    let fx = Fixture::new().await;
    let (first, second) = tokio::join!(fx.create(&fx.frontend, 1), fx.create(&fx.frontend, 1));
    let (won, lost) = match (first, second) {
        (Ok(view), Err(error)) | (Err(error), Ok(view)) => (view, refusal(error)),
        other => panic!("expected one create to win, got {other:?}"),
    };
    assert_eq!(lost.code(), "workspace_exists");
    assert_only_the_winner_is_on_disk(&fx, &won);
    assert_eq!(fx.count("SELECT COUNT(*) FROM workspaces").await, 1);
}

/// Both creates rename before either transaction runs, so the one that renamed second can commit first and keep `ws/t-1-2`. The loser removes only its own directory.
#[cfg(test)]
fn assert_only_the_winner_is_on_disk(fx: &Fixture, won: &WorkspaceView) {
    assert!(
        ["ws/t-1", "ws/t-1-2"].contains(&won.path.as_str()),
        "{won:?}"
    );
    let name = won.path.trim_start_matches("ws/");
    assert_eq!(fx.ws_entries(), vec![name]);
    assert_eq!(
        std::fs::read(fx.dir(&won.path).join("src/lib.rs")).unwrap(),
        LIB
    );
}

#[tokio::test]
async fn a_later_workspace_of_the_task_takes_the_next_name() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    fx.abandon(&fx.frontend, 1).await.unwrap();
    let second = fx.create(&fx.frontend, 1).await.unwrap();
    assert_eq!(second.id, ws(2));
    assert_eq!(second.path, "ws/t-1-2");
    assert_eq!(std::fs::read(fx.dir("ws/t-1-2/src/lib.rs")).unwrap(), LIB);
    assert_eq!(fx.ws_entries(), vec!["t-1", "t-1-2"]);
    assert_eq!(
        fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap(),
        second
    );

    fx.abandon(&fx.frontend, 2).await.unwrap();
    let third = fx.create(&fx.frontend, 1).await.unwrap();
    assert_eq!(third.path, "ws/t-1-3");
    fx.abandon(&fx.frontend, 3).await.unwrap();
    let latest = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(
        (latest.id, latest.state),
        (ws(3), WorkspaceState::Abandoned)
    );

    let error = fx
        .workspaces
        .get(WorkspaceRef::Task(t(2)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("workspace_not_found"));
    let error = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(9)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("workspace_not_found"));
}

#[tokio::test]
async fn a_name_held_on_disk_is_skipped_and_left_alone() {
    let fx = Fixture::new().await;
    std::fs::create_dir_all(fx.dir("ws/t-1-2")).unwrap();
    std::fs::write(fx.dir("ws/t-1"), b"a file").unwrap();
    std::fs::write(fx.dir("ws/t-1-2/keep"), b"kept").unwrap();
    let view = fx.create(&fx.frontend, 1).await.unwrap();
    assert_eq!(view.path, "ws/t-1-3");
    assert_eq!(std::fs::read(fx.dir("ws/t-1")).unwrap(), b"a file");
    assert_eq!(std::fs::read(fx.dir("ws/t-1-2/keep")).unwrap(), b"kept");
    assert_eq!(fx.ws_entries(), vec!["t-1", "t-1-2", "t-1-3"]);
}

#[tokio::test]
async fn a_name_a_closed_workspace_holds_is_skipped_after_its_directory_is_gone() {
    let fx = Fixture::new().await;
    std::fs::create_dir_all(fx.dir("ws/t-1")).unwrap();
    std::fs::write(fx.dir("ws/t-1/left"), b"left").unwrap();
    let first = fx.create(&fx.frontend, 1).await.unwrap();
    assert_eq!(first.path, "ws/t-1-2");
    fx.abandon(&fx.frontend, 1).await.unwrap();
    std::fs::remove_dir_all(fx.dir("ws/t-1-2")).unwrap();

    let second = fx.create(&fx.frontend, 1).await.unwrap();
    assert_eq!(second.path, "ws/t-1-3");
    assert_eq!(fx.ws_entries(), vec!["t-1", "t-1-3"]);
    assert_eq!(std::fs::read(fx.dir("ws/t-1/left")).unwrap(), b"left");
}

#[tokio::test]
async fn a_failed_insert_removes_the_directory_it_made() {
    let fx = Fixture::new().await;
    fx.execute(
        "CREATE TRIGGER refuse_workspaces BEFORE INSERT ON workspaces BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .await;
    let before = fx.last_seq().await;
    let error = fx.create(&fx.frontend, 1).await.unwrap_err();
    assert!(matches!(error, WorkspaceError::Store(_)), "{error:?}");
    assert!(fx.ws_entries().is_empty());
    assert!(fx.events_after(before).await.is_empty());
    assert!(
        claims::unreleased_for_task(fx.project.store().read(), t(1))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fx.status(1).await, (TaskStatus::Todo, false));
}

#[tokio::test]
async fn a_missing_base_object_returns_its_own_error_and_leaves_nothing() {
    let fx = Fixture::new().await;
    let head = fold_store::workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap();
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    std::fs::remove_file(objects.path(&head.tree_hash)).unwrap();
    let error = fx.create(&fx.frontend, 1).await.unwrap_err();
    assert_eq!(error.code(), Some("object_missing"));
    assert!(fx.ws_entries().is_empty());
    assert_eq!(fx.count("SELECT COUNT(*) FROM workspaces").await, 0);
}

#[tokio::test]
async fn a_project_without_a_revision_cannot_create() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    let clock = Arc::new(project.clock().clone());
    let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
    let tasks = TaskService::new(project.store().clone(), bus.clone(), clock.clone(), 5);
    let service = WorkspaceService::new(
        project.store().clone(),
        bus,
        clock,
        project.paths().clone(),
        Git::default(),
        5,
    );
    tasks
        .create_task(
            &agent("frontend"),
            CreateTask {
                title: TaskTitle::parse("task").unwrap(),
                description: TaskDescription::new("body"),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    let worker = register(&agents, "frontend").await;
    let error = service
        .create_workspace(&by(&worker), t(1))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("revision_not_found"));
}

#[tokio::test]
async fn archive_closes_a_live_workspace_once_and_keeps_the_directory() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    let actor = EventActor::fold();
    let now = fx.project.now();
    let mut buffer = EventBuffer::new();
    let (first, second, unknown) = fx
        .project
        .store()
        .write(async |tx| {
            let reason = ArchiveReason::Integrated;
            let first = workspaces::archive_in_tx::<WorkspaceError>(
                tx,
                &mut buffer,
                &actor,
                ws(1),
                reason,
                now,
            )
            .await?;
            let second = workspaces::archive_in_tx::<WorkspaceError>(
                tx,
                &mut buffer,
                &actor,
                ws(1),
                reason,
                now,
            )
            .await?;
            let unknown = workspaces::archive_in_tx::<WorkspaceError>(
                tx,
                &mut buffer,
                &actor,
                ws(5),
                reason,
                now,
            )
            .await?;
            Ok::<_, WorkspaceError>((first, second, unknown))
        })
        .await
        .unwrap();
    assert_eq!((first, second, unknown), (true, false, false));
    assert_eq!(buffer.len(), 1);
    let event = &buffer.events()[0];
    assert_eq!(event.event_type, types::WORKSPACE_ARCHIVED);
    assert_eq!((event.task, event.workspace), (Some(t(1)), Some(ws(1))));
    assert_eq!(event.data, json!({"reason": "integrated"}));
    let view = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(view.state, WorkspaceState::Archived);
    assert_eq!(view.closed_at, Some(now));
    assert!(fx.dir("ws/t-1/src/lib.rs").is_file());
    assert_eq!(ArchiveReason::TaskArchived.as_str(), "task_archived");
}

#[tokio::test]
async fn archiving_the_task_archives_its_workspace() {
    let fx = Fixture::new().await;
    fx.create(&fx.frontend, 1).await.unwrap();
    let before = fx.last_seq().await;
    fx.tasks
        .archive_task(&agent("frontend"), t(1))
        .await
        .unwrap();
    let appended = fx.events_after(before).await;
    let archived = of_type(&appended, types::WORKSPACE_ARCHIVED);
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].actor.as_str(), "frontend");
    assert_eq!(archived[0].data, json!({"reason": "task_archived"}));
    let view = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(view.state, WorkspaceState::Archived);
    assert!(Path::new(&fx.dir("ws/t-1")).is_dir());
}

#[tokio::test]
async fn a_local_service_takes_the_configured_lease_and_refuses_a_bad_config() {
    let fx = Fixture::new().await;
    let config = fx.project.paths().config();
    std::fs::write(&config, "lease_minute = 3").unwrap();
    let error = WorkspaceService::open_local(
        fx.project.paths().clone(),
        Git::default(),
        Arc::new(fx.project.clock().clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), Some("invalid_config"));
    std::fs::write(&config, "lease_minutes = 7").unwrap();
    let service = WorkspaceService::open_local(
        fx.project.paths().clone(),
        Git::default(),
        Arc::new(fx.project.clock().clone()),
    )
    .await
    .unwrap();
    assert!(format!("{service:?}").contains("Minutes(7)"));
    service.close().await;
}

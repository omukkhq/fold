use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fold_core::object::ObjectKind;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventFilter, FoldError, RevisionId, TaskDescription,
    TaskId, TaskTitle, Timestamp, WorkspaceId,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::tasks::{self, CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceLocks, WorkspaceService};
use fold_store::agents::WorkerRow;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    advance_main, create_initial_revision, head, initial_revision, insert_revision,
};
use fold_store::{StoreError, events};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use sqlx::AssertSqlSafe;

pub const LEASE_MINUTES: u64 = 5;
pub const LIB: &[u8] = b"pub fn x() {}\n";
pub const README: &[u8] = b"# demo\n";
pub const EDITED: &[u8] = b"pub fn x() { todo() }\n";

pub fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

pub fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

pub fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// The acting agent of a worker row, as `AgentService::actor` builds it.
pub fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

pub fn task_refusal(error: TaskError) -> FoldError {
    match error {
        TaskError::Fold(error) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub fn workspace_refusal(error: WorkspaceError) -> FoldError {
    match error {
        WorkspaceError::Fold(error) | WorkspaceError::Task(TaskError::Fold(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub fn details(error: &FoldError) -> serde_json::Value {
    error
        .details()
        .cloned()
        .map_or(serde_json::Value::Null, serde_json::Value::Object)
}

pub fn names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

/// Every file under `dir` with its content and modification time, in path order.
pub fn snapshot(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                pending.push(path);
            } else {
                let content = std::fs::read(&path).unwrap();
                found.push((path, content, meta.modified().unwrap()));
            }
        }
    }
    found.sort();
    found
}

pub struct Fixture {
    pub project: TestProject,
    pub bus: EventBus,
    pub tasks: TaskService,
    pub workspaces: WorkspaceService,
    /// The workspace locks both services share, as `ProjectHandle::open` shares them.
    pub ops: Arc<WorkspaceLocks>,
    pub frontend: WorkerRow,
    pub backend: WorkerRow,
}

impl Fixture {
    /// `r1` holds `src/lib.rs` and `README.md`. Three tasks, the agents `frontend` and `backend`, and a lease of [`LEASE_MINUTES`].
    pub async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let tree = write_initial_tree(&objects, flat(&objects, LIB)).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let ops = Arc::new(WorkspaceLocks::default());
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            LEASE_MINUTES,
        )
        .with_workspace_locks(ops.clone());
        let workspaces = WorkspaceService::new(
            project.store().clone(),
            bus.clone(),
            clock,
            project.paths().clone(),
            LEASE_MINUTES,
        )
        .with_locks(Arc::new(tokio::sync::RwLock::new(())), ops.clone());
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        for _ in 0..3 {
            tasks
                .create_task(
                    &by(&frontend),
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
            bus,
            tasks,
            workspaces,
            ops,
            frontend,
            backend,
        }
    }

    /// A review service over the fixture's project and workspaces, for the tests that open a proposal.
    pub fn reviews(&self) -> ReviewService {
        ReviewService::new(
            self.project.store().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
            self.workspaces.clone(),
            Arc::default(),
        )
    }

    pub fn dir(&self, path: &str) -> PathBuf {
        self.project.paths().root().join(path)
    }

    pub fn now(&self) -> Timestamp {
        self.project.now()
    }

    pub fn advance(&self, by: Duration) -> Timestamp {
        self.project.clock().advance(by).unwrap();
        self.project.now()
    }

    pub async fn events_after(&self, seq: u64) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            seq,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
    }

    pub async fn last_seq(&self) -> u64 {
        self.events_after(0)
            .await
            .last()
            .map_or(0, |event| event.seq)
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }

    pub async fn execute(&self, sql: &str) {
        self.project
            .store()
            .write(async |tx| {
                sqlx::raw_sql(AssertSqlSafe(sql)).execute(tx.conn()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }

    /// Marks `task` Done with a completion record, as an integration would.
    pub async fn complete(&self, task: u32) {
        let now = self.now();
        self.project
            .store()
            .write(async |tx| {
                completions::insert(tx, t(task), CompletionKind::Children, None, now).await?;
                tasks::recompute(tx, &[t(task)], now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }

    /// Records a revision on top of `main` with `src/lib.rs` holding `lib`, and moves `main` to it, as the integration procedure would.
    pub async fn move_main(&self, lib: &[u8]) -> RevisionId {
        let store = self.project.store();
        let head = head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.project.paths().fold_dir()).unwrap();
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, flat(&objects, lib)).unwrap(),
            parent: Some(head.hash),
            number,
            integration: None,
            created_at: self.now(),
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
}

async fn register(agents: &AgentService, name: &str) -> WorkerRow {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    agents.resolve_worker(&ctx, None).await.unwrap()
}

/// `README.md` and `src/lib.rs` holding `lib`, with the blobs stored.
fn flat(objects: &ObjectStore, lib: &[u8]) -> FlatTree {
    let mut tree = FlatTree::new();
    for (path, content) in [("src/lib.rs", lib), ("README.md", README)] {
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

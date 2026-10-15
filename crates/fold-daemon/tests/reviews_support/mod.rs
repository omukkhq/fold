use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventFilter, FoldError, HumanName, ProposalId,
    TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName, WorkspaceId,
};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::reviews::{ReviewError, ReviewService};
use fold_daemon::services::tasks::{self, CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceLocks, WorkspaceService};
use fold_store::agents::WorkerRow;
use fold_store::events;
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use sqlx::AssertSqlSafe;

pub const LEASE_MINUTES: u64 = 5;
pub const LIB: &[u8] = b"pub fn x() {}\n";
pub const README: &[u8] = b"# demo\n";

pub fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

pub fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

pub fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

/// A registered agent: its session, which `record_call` stamps, and its main worker.
pub struct Agent {
    pub ctx: SessionContext,
    pub worker: WorkerRow,
}

/// A project with `r1` (`src/lib.rs`, `README.md`), the tasks `T-1` and `T-2`, and three agents: `frontend` writes, `backend` and `review` review.
pub struct Fixture {
    pub project: TestProject,
    pub bus: EventBus,
    pub agents: AgentService,
    pub tasks: TaskService,
    pub workspaces: WorkspaceService,
    pub reviews: ReviewService,
    objects: Arc<tokio::sync::RwLock<()>>,
    locks: Arc<WorkspaceLocks>,
    /// The project's integration mutex, shared by every review service of the fixture.
    pub integration: Arc<tokio::sync::Mutex<()>>,
    pub frontend: Agent,
    pub backend: Agent,
    pub review: Agent,
}

impl Fixture {
    pub async fn new() -> Self {
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
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone())
            .with_lease_minutes(LEASE_MINUTES);
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            LEASE_MINUTES,
        );
        let objects = Arc::new(tokio::sync::RwLock::new(()));
        let locks = Arc::new(WorkspaceLocks::default());
        let workspaces = workspace_service(&project, &bus, &objects, &locks);
        let integration = Arc::new(tokio::sync::Mutex::new(()));
        let reviews = ReviewService::new(
            project.store().clone(),
            bus.clone(),
            clock,
            workspaces.clone(),
            Arc::clone(&integration),
        );
        let creator = agent("frontend");
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
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        let review = register(&agents, "review").await;
        Self {
            project,
            bus,
            agents,
            tasks,
            workspaces,
            reviews,
            objects,
            locks,
            integration,
            frontend,
            backend,
            review,
        }
    }

    /// Drops every service without a graceful shutdown and opens the project again, as a daemon that was killed and started again does. The agents keep their sessions, which live in the database; the locks are new, as in a new process.
    pub async fn restart(&mut self) {
        self.project.reopen_after_crash().await;
        let clock = Arc::new(self.project.clock().clone());
        self.bus = EventBus::new();
        self.objects = Arc::new(tokio::sync::RwLock::new(()));
        self.locks = Arc::new(WorkspaceLocks::default());
        self.integration = Arc::new(tokio::sync::Mutex::new(()));
        self.agents = AgentService::new(
            self.project.store().clone(),
            self.bus.clone(),
            clock.clone(),
        )
        .with_lease_minutes(LEASE_MINUTES);
        self.tasks = TaskService::new(
            self.project.store().clone(),
            self.bus.clone(),
            clock,
            LEASE_MINUTES,
        );
        self.workspaces = workspace_service(&self.project, &self.bus, &self.objects, &self.locks);
        self.reviews = self.review_service(self.workspaces.clone());
    }

    /// Another review service over the same project and its locks, as each request of a transport builds one.
    pub fn service(&self) -> ReviewService {
        let workspaces = workspace_service(&self.project, &self.bus, &self.objects, &self.locks);
        self.review_service(workspaces)
    }

    /// A review service whose workspace service reads the lease from the project's `config.toml`, as `WorkspaceService::for_project` does.
    pub fn configured_service(&self) -> ReviewService {
        let workspaces = workspace_service(&self.project, &self.bus, &self.objects, &self.locks)
            .with_project_config();
        self.review_service(workspaces)
    }

    fn review_service(&self, workspaces: WorkspaceService) -> ReviewService {
        ReviewService::new(
            self.project.store().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
            workspaces,
            Arc::clone(&self.integration),
        )
    }

    /// `frontend` takes `T-1`, changes `src/lib.rs`, adds `src/new.rs` and the symlink `docs/readme -> ../README.md`, and opens `P-1`.
    pub async fn proposal(&self) {
        let author = by(&self.frontend.worker);
        self.workspaces
            .create_workspace(&author, t(1))
            .await
            .unwrap();
        self.write("src/lib.rs", "pub fn y() {}\n");
        self.write("src/new.rs", "pub fn z() {}\n");
        let link = self.task_dir().join("docs/readme");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../README.md", link).unwrap();
        self.reviews
            .create_proposal(&author, t(1), "Add the report", Some("tests pass"))
            .await
            .unwrap();
    }

    pub fn task_dir(&self) -> PathBuf {
        self.project.paths().root().join("ws/t-1")
    }

    pub fn write(&self, path: &str, content: &str) {
        let file = self.task_dir().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    /// The names under `ws/`, sorted.
    pub fn ws_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.project.paths().ws_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    pub async fn status(&self, task: u32) -> TaskStatus {
        self.tasks.get(t(task)).await.unwrap().status
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

    pub async fn text(&self, sql: &str) -> Option<String> {
        sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }

    pub async fn execute(&self, sql: &str) {
        fold_store::testing::execute(self.project.store(), sql).await;
    }

    pub async fn recompute(&self, task: u32) {
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

    pub fn advance(&self, minutes: u64) {
        self.project
            .clock()
            .advance(Duration::from_secs(minutes * 60))
            .unwrap();
    }

    /// A subagent worker `<agent>/<segment>` of a registered agent.
    pub async fn subagent(&self, agent: &Agent, segment: &str) -> WorkerRow {
        self.agents
            .register_worker(&agent.ctx, segment, None)
            .await
            .unwrap()
            .worker
    }
}

/// A workspace service over the project that shares `objects` and `locks`, as `WorkspaceService::for_project` shares the project's.
fn workspace_service(
    project: &TestProject,
    bus: &EventBus,
    objects: &Arc<tokio::sync::RwLock<()>>,
    locks: &Arc<WorkspaceLocks>,
) -> WorkspaceService {
    WorkspaceService::new(
        project.store().clone(),
        bus.clone(),
        Arc::new(project.clock().clone()),
        project.paths().clone(),
        LEASE_MINUTES,
    )
    .with_locks(Arc::clone(objects), Arc::clone(locks))
}

async fn register(agents: &AgentService, name: &str) -> Agent {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    let worker = agents.resolve_worker(&ctx, None).await.unwrap();
    Agent { ctx, worker }
}

/// The acting agent of a worker row, as `AgentService::actor` builds it.
pub fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

pub fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

pub fn human() -> Actor {
    Actor::human(HumanName::parse("tanim").unwrap())
}

pub fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub fn workspace_refusal<T: std::fmt::Debug>(result: Result<T, WorkspaceError>) -> FoldError {
    match result {
        Err(WorkspaceError::Fold(error) | WorkspaceError::Task(TaskError::Fold(error))) => error,
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

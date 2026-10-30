use crate::common::intents::Covering as _;
use crate::common::intents::intent_of;
use crate::common::roles::give_role;
use fold_core::reviews::ProposalIntent;
use fold_core::roles::Role;
use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventActor, EventFilter, EventType, FoldError, HumanName,
    ProposalId, RevisionId, TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName,
    WorkspaceId, codes,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::{BusMessage, EventBuffer, EventBus};
use fold_daemon::services::reviews::{
    ProposalDetail, ProposalFilter, Retirement, ReviewError, ReviewService, Turn, retire_proposal,
};
use fold_daemon::services::tasks::{self, CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::{WorkspaceLocks, WorkspaceRef, WorkspaceService};
use fold_store::agents::WorkerRow;
use fold_store::events;
use fold_store::reviews::ProposalState;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    WorkspaceState, create_initial_revision, initial_revision, list_changesets, list_checkpoints,
};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use serde_json::json;
use sqlx::AssertSqlSafe;

const LEASE_MINUTES: u64 = 5;
const LIB: &[u8] = b"pub fn x() {}\n";
const README: &[u8] = b"# demo\n";
/// A file a sync left with conflict markers.
const CONFLICTED: &str = "<<<<<<< workspace ws-1\npub fn y() {}\n||||||| base r1\npub fn x() {}\n=======\npub fn w() {}\n>>>>>>> main r2\n";

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
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
    reviews: ReviewService,
    objects: Arc<tokio::sync::RwLock<()>>,
    locks: Arc<WorkspaceLocks>,
    frontend: WorkerRow,
    backend: WorkerRow,
}

#[cfg(test)]
impl Fixture {
    /// `r1` holds `src/lib.rs` and `README.md`. Two tasks, the agents `frontend` and `backend`.
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
        let objects = Arc::new(tokio::sync::RwLock::new(()));
        let locks = Arc::new(WorkspaceLocks::default());
        let workspaces = workspace_service(&project, &bus, &objects, &locks);
        let reviews = ReviewService::new(
            project.store().clone(),
            bus.clone(),
            clock,
            workspaces.clone(),
            Arc::default(),
        );
        let creator = agent("frontend");
        crate::common::roles::lead_work_areas(project.store()).await;
        // A task has one area: `T-1` is in the area that `frontend` leads, `T-2` in the one `backend` leads.
        for lead in ["frontend", "backend"] {
            tasks
                .create_task(
                    &creator,
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: crate::common::roles::work_labels_of(lead),
                    },
                )
                .await
                .unwrap();
        }
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        register(&agents, "review").await;
        // `backend` leads its area and may not decide on a proposal; `review` does as a reviewer.
        give_role(project.store(), "review", Role::Reviewer, None).await;
        Self {
            project,
            bus,
            tasks,
            workspaces,
            reviews,
            objects,
            locks,
            frontend,
            backend,
        }
    }

    /// Another service over the same project and its locks, as each request of a transport builds one.
    fn service(&self) -> ReviewService {
        let workspaces = workspace_service(&self.project, &self.bus, &self.objects, &self.locks);
        ReviewService::new(
            self.project.store().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
            workspaces,
            Arc::default(),
        )
    }

    /// The task workspace of `task` for `worker`, which takes the claim.
    async fn workspace(&self, worker: &WorkerRow, task: u32) {
        self.workspaces
            .create_workspace(&by(worker), t(task))
            .await
            .unwrap();
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.project.paths().root().join(".fold/ws/t-1").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    async fn create(&self, worker: &WorkerRow) -> Result<ProposalDetail, ReviewError> {
        self.reviews
            .create_proposal_covering(&by(worker), t(1), "Add the report", Some("tests pass"))
            .await
            .map(|proposed| proposed.detail().unwrap())
    }

    async fn status(&self, task: u32) -> TaskStatus {
        self.tasks.get(t(task)).await.unwrap().status
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

    async fn last_seq(&self) -> u64 {
        self.events_after(0)
            .await
            .last()
            .map_or(0, |event| event.seq)
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }

    async fn execute(&self, sql: &str) {
        fold_store::testing::execute(self.project.store(), sql).await;
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

    /// A review on the current version of `P-1` by `backend`, as the reviews card will write it.
    async fn review(&self, verdict: &str) {
        self.project
            .clock()
            .advance(Duration::from_secs(1))
            .unwrap();
        let now = self.project.now().as_millis();
        self.execute(&format!(
            "INSERT INTO reviews (version_id, reviewer_worker_id, verdict, body, created_at)
             VALUES ((SELECT MAX(id) FROM proposal_versions), {}, '{verdict}', 'review', {now})",
            self.backend.id
        ))
        .await;
        self.recompute(1).await;
    }
}

/// A workspace service over the project that shares `objects` and `locks`, as `WorkspaceService::for_project` shares the project's.
#[cfg(test)]
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

/// A worker of the agent `name`, which the fixture makes a reviewer when it is `review`.
#[cfg(test)]
fn reviewer(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn human() -> Actor {
    Actor::human(HumanName::parse("tanim").unwrap())
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> WorkerRow {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    agents.resolve_worker(&ctx).await.unwrap()
}

#[cfg(test)]
fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
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
fn names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

#[cfg(test)]
fn of_type(events: &[Event], event_type: EventType) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

#[tokio::test]
async fn create_proposal_opens_v1_and_moves_task_to_review() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    fx.write("src/new.rs", "pub fn z() {}\n");
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    let mut bus = fx.bus.subscribe();
    let before = fx.last_seq().await;

    let detail = fx.create(&fx.frontend).await.unwrap();

    let summary = &detail.summary;
    assert_eq!(summary.id, p(1));
    assert_eq!(summary.task, t(1));
    assert_eq!(summary.title, "Add the report");
    assert_eq!(summary.author, fx.frontend.name);
    assert_eq!(summary.state, ProposalState::Open);
    assert_eq!(summary.current_version, 1);
    assert_eq!(summary.turn, Some(Turn::WithReviewer));
    assert_eq!(summary.review_claim, None);
    assert!(!summary.conflicted);
    assert_eq!(summary.created_at, fx.project.now());
    assert_eq!(summary.closed_at, None);
    assert_eq!(detail.versions.len(), 1);
    let v1 = &detail.selected;
    assert_eq!(v1.version, 1);
    assert_eq!(v1.base, RevisionId::from_number(1).unwrap());
    assert_eq!(v1.evidence.as_deref(), Some("tests pass"));
    assert_eq!(v1.created_by, "frontend");
    assert_eq!(
        (
            v1.stats.files_added,
            v1.stats.files_modified,
            v1.stats.files_deleted
        ),
        (1, 1, 0)
    );
    assert_eq!(fx.status(1).await, TaskStatus::Review);

    let changesets = list_changesets(fx.project.store().read(), ws(1))
        .await
        .unwrap();
    assert_eq!(changesets.len(), 1);
    assert_eq!(changesets[0].id, v1.changeset);
    assert_eq!(changesets[0].tree, v1.tree);
    assert_eq!(changesets[0].message, "Add the report");
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    assert!(objects.contains(&v1.tree).unwrap());

    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        [
            "checkpoint.recorded",
            "changeset.recorded",
            "proposal.opened",
            "task.status_changed"
        ]
    );
    let opened = of_type(&appended, types::PROPOSAL_OPENED)[0];
    assert_eq!(opened.actor, EventActor::worker(&fx.frontend.name));
    assert_eq!(opened.task, Some(t(1)));
    assert_eq!(opened.proposal, Some(p(1)));
    assert_eq!(opened.workspace, Some(ws(1)));
    assert_eq!(opened.revision, RevisionId::from_number(1));
    assert_eq!(
        opened.data,
        json!({"version": 1, "changeset": v1.changeset.to_string(), "author": "frontend"})
    );
    assert!(
        appended
            .iter()
            .all(|event| event.created_at == summary.created_at)
    );
    let status = of_type(&appended, types::TASK_STATUS_CHANGED)[0];
    assert_eq!(status.data, json!({"from": "in_progress", "to": "review"}));
    let mut published = Vec::new();
    while let Ok(BusMessage::Durable(event)) = bus.try_recv() {
        published.push(event.seq);
    }
    assert_eq!(
        published,
        appended.iter().map(|event| event.seq).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn create_proposal_records_a_changeset_only_when_the_tree_changed() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    let first = fx.create(&fx.frontend).await.unwrap();
    let store = fx.project.store().read();
    assert_eq!(list_changesets(store, ws(1)).await.unwrap().len(), 1);
    assert_eq!(list_checkpoints(store, ws(1), true).await.unwrap().len(), 1);
    fx.reviews
        .close_proposal(&by(&fx.frontend), p(1), "split it")
        .await
        .unwrap();

    let before = fx.last_seq().await;
    let second = fx.create(&fx.frontend).await.unwrap();
    assert_eq!(second.summary.id, p(2));
    assert_eq!(second.selected.changeset, first.selected.changeset);
    assert_eq!(second.selected.tree, first.selected.tree);
    assert_eq!(list_changesets(store, ws(1)).await.unwrap().len(), 1);
    assert_eq!(list_checkpoints(store, ws(1), true).await.unwrap().len(), 1);
    assert_eq!(
        names(&fx.events_after(before).await),
        ["proposal.opened", "task.status_changed"]
    );

    fx.write("README.md", "# changed\n");
    let third = fx
        .reviews
        .update_proposal_covering(&by(&fx.frontend), p(2), None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_ne!(third.selected.changeset, first.selected.changeset);
    assert_eq!(list_changesets(store, ws(1)).await.unwrap().len(), 2);
    let checkpoints = list_checkpoints(store, ws(1), true).await.unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints[1].tree, third.selected.tree);
    assert_eq!(checkpoints[1].reason.as_str(), "before_proposal_update");
}

#[tokio::test]
async fn create_proposal_refuses_without_claim() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    let before = fx.last_seq().await;

    let error = refusal(fx.create(&fx.backend).await);
    assert_eq!(error.code(), codes::NOT_CLAIM_HOLDER);
    assert_eq!(
        details(&error),
        json!({"task": "T-1", "holder": "frontend"})
    );
    for actor in [human(), Actor::System] {
        let error = refusal(
            fx.reviews
                .create_proposal_covering(&actor, t(1), "Add the report", None)
                .await,
        );
        assert_eq!(error.code(), codes::NOT_CLAIM_HOLDER);
    }
    let error = refusal(
        fx.reviews
            .create_proposal_covering(&by(&fx.frontend), t(2), "Add the report", None)
            .await,
    );
    assert_eq!(error.code(), codes::NOT_CLAIM_HOLDER);
    assert_eq!(details(&error), json!({"task": "T-2", "holder": null}));
    let error = refusal(
        fx.reviews
            .create_proposal_covering(&by(&fx.frontend), t(9), "Add the report", None)
            .await,
    );
    assert_eq!(error.code(), codes::TASK_NOT_FOUND);
    assert!(fx.events_after(before).await.is_empty());
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM changesets").await, 0);
}

#[tokio::test]
async fn create_proposal_refuses_without_workspace() {
    let fx = Fixture::new().await;
    fx.tasks.claim_task(&by(&fx.frontend), t(1)).await.unwrap();
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::WORKSPACE_NOT_ACTIVE);
    assert_eq!(details(&error), json!({"task": "T-1"}));
    assert!(error.hint().unwrap().contains("create_workspace"));

    fx.workspace(&fx.frontend, 1).await;
    fx.execute("UPDATE workspaces SET state = 'missing'").await;
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::WORKSPACE_NOT_ACTIVE);
    assert_eq!(
        details(&error),
        json!({"workspace": "ws-1", "state": "missing"})
    );

    fx.execute(&format!(
        "UPDATE workspaces SET state = 'active', worker_id = {}",
        fx.backend.id
    ))
    .await;
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::NOT_WORKSPACE_OWNER);
    assert_eq!(
        details(&error),
        json!({"workspace": "ws-1", "owner": "backend"})
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);
}

#[tokio::test]
async fn create_proposal_refuses_with_conflict_markers_listed() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.execute(
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES
         (1, CAST('src/lib.rs' AS BLOB), 'both_modified_text'),
         (1, CAST('README.md' AS BLOB), 'modify_delete'),
         (1, CAST('docs/done.md' AS BLOB), 'add_add')",
    )
    .await;
    fx.write("src/lib.rs", CONFLICTED);
    fx.write("README.md.fold-main", "# main\n");
    let before = fx.last_seq().await;
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::UNRESOLVED_CONFLICTS);
    assert_eq!(
        details(&error),
        json!({"paths": ["README.md", "src/lib.rs"]})
    );
    assert!(fx.events_after(before).await.is_empty());
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);

    fx.write("src/lib.rs", "pub fn merged() {}\n");
    std::fs::remove_file(
        fx.project
            .paths()
            .root()
            .join(".fold/ws/t-1/README.md.fold-main"),
    )
    .unwrap();
    fx.create(&fx.frontend).await.unwrap();
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspace_conflicts").await,
        0
    );
}

#[tokio::test]
async fn a_proposal_waits_for_an_unfinished_sync_or_restore() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    fx.workspaces
        .record_checkpoint(&by(&fx.frontend), ws(1), None)
        .await
        .unwrap();
    let mark = "UPDATE workspaces SET sync_checkpoint_id = (SELECT MAX(id) FROM checkpoints)";
    fx.execute(mark).await;
    let before = fx.last_seq().await;
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::WORKSPACE_WRITE_UNFINISHED);
    assert_eq!(
        details(&error),
        json!({"workspace": "ws-1", "checkpoint": "ck-1"})
    );
    assert!(fx.events_after(before).await.is_empty());
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);

    fx.execute("UPDATE workspaces SET sync_checkpoint_id = NULL")
        .await;
    fx.create(&fx.frontend).await.unwrap();
    fx.write("src/lib.rs", "pub fn z() {}\n");
    fx.execute(mark).await;
    let error = refusal(
        fx.reviews
            .update_proposal_covering(&by(&fx.frontend), p(1), None)
            .await,
    );
    assert_eq!(error.code(), codes::WORKSPACE_WRITE_UNFINISHED);
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposal_versions").await, 1);
}

#[tokio::test]
async fn create_proposal_refuses_bad_input_and_a_closed_task() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    for title in ["", "   ", "two\nlines", &"x".repeat(201)] {
        let error = refusal(
            fx.reviews
                .create_proposal_covering(&by(&fx.frontend), t(1), title, None)
                .await,
        );
        assert_eq!(error.code(), codes::INVALID_REQUEST, "{title:?}");
        assert_eq!(details(&error), json!({"field": "title"}));
    }
    let detail = fx
        .reviews
        .create_proposal_covering(&by(&fx.frontend), t(1), "  Add it  ", Some("  "))
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_eq!(detail.summary.title, "Add it");
    assert_eq!(detail.selected.evidence, None);

    fx.execute("UPDATE tasks SET archived_at = 1 WHERE number = 1")
        .await;
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::TASK_CLOSED);
}

#[tokio::test]
async fn create_proposal_refuses_a_second_open_proposal() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.create(&fx.frontend).await.unwrap();
    let error = refusal(fx.create(&fx.frontend).await);
    assert_eq!(error.code(), codes::PROPOSAL_EXISTS);
    assert_eq!(details(&error), json!({"task": "T-1", "proposal": "P-1"}));
    assert!(error.hint().unwrap().contains("update_proposal"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_create_proposal_calls_give_one_proposal_exists() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    let (one, two) = (fx.service(), fx.service());
    let create = async |service: &ReviewService| {
        service
            .create_proposal_covering(
                &by(&fx.frontend),
                t(1),
                "Add the report",
                Some("tests pass"),
            )
            .await
    };
    let (first, second) = tokio::join!(create(&one), create(&two));
    let (won, lost) = match (first, second) {
        (Ok(won), lost @ Err(_)) | (lost @ Err(_), Ok(won)) => (won, lost),
        other => panic!("expected one proposal and one refusal, got {other:?}"),
    };
    assert_eq!(won.detail().unwrap().summary.id, p(1));
    let error = refusal(lost);
    assert_eq!(error.code(), codes::PROPOSAL_EXISTS);
    assert_eq!(details(&error), json!({"task": "T-1", "proposal": "P-1"}));
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposal_versions").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM changesets").await, 1);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM events WHERE type = 'proposal.opened'")
            .await,
        1
    );
}

#[tokio::test]
async fn update_proposal_adds_v2_and_returns_task_to_review() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    let v1 = fx.create(&fx.frontend).await.unwrap().selected;
    fx.review("request_changes").await;
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    fx.execute("UPDATE proposals SET conflicted = 1").await;

    fx.write("src/lib.rs", "pub fn fixed() {}\n");
    fx.project.clock().advance(Duration::from_secs(5)).unwrap();
    let before = fx.last_seq().await;
    let detail = fx
        .reviews
        .update_proposal_covering(&by(&fx.frontend), p(1), Some("fixed the review"))
        .await
        .unwrap()
        .detail()
        .unwrap();

    assert_eq!(detail.summary.current_version, 2);
    assert!(!detail.summary.conflicted);
    assert_eq!(detail.summary.turn, Some(Turn::WithReviewer));
    assert_eq!(detail.summary.updated_at, fx.project.now());
    assert_eq!(detail.versions.len(), 2);
    assert_eq!(detail.versions[0], v1);
    let v2 = &detail.selected;
    assert_eq!(v2.version, 2);
    assert_eq!(v2.evidence.as_deref(), Some("fixed the review"));
    assert_ne!(v2.tree, v1.tree);
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM reviews WHERE version_id = 1")
            .await,
        1
    );

    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        [
            "checkpoint.recorded",
            "changeset.recorded",
            "proposal.version_added",
            "task.status_changed"
        ]
    );
    let added = of_type(&appended, types::PROPOSAL_VERSION_ADDED)[0];
    assert_eq!(added.proposal, Some(p(1)));
    assert_eq!(added.task, Some(t(1)));
    assert_eq!(
        added.data,
        json!({"version": 2, "changeset": v2.changeset.to_string()})
    );

    let first = fx.reviews.get_proposal(p(1), Some(1)).await.unwrap();
    assert_eq!(first.selected, v1);
}

#[tokio::test]
async fn update_proposal_refuses_an_unchanged_version() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    fx.create(&fx.frontend).await.unwrap();
    let before = fx.last_seq().await;
    for evidence in [None, Some("tests pass")] {
        let error = refusal(
            fx.reviews
                .update_proposal_covering(&by(&fx.frontend), p(1), evidence)
                .await,
        );
        assert_eq!(error.code(), codes::VERSION_UNCHANGED);
        assert_eq!(details(&error), json!({"proposal": "P-1", "version": 1}));
    }
    assert!(fx.events_after(before).await.is_empty());

    let detail = fx
        .reviews
        .update_proposal_covering(&by(&fx.frontend), p(1), Some("more tests"))
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_eq!(detail.selected.version, 2);
    assert_eq!(detail.selected.changeset, detail.versions[0].changeset);
    assert_eq!(
        names(&fx.events_after(before).await),
        ["proposal.version_added"]
    );
}

#[tokio::test]
async fn update_proposal_refuses_others_and_proposals_that_are_not_open() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.create(&fx.frontend).await.unwrap();
    for actor in [by(&fx.backend), human(), reviewer("review")] {
        let error = refusal(
            fx.reviews
                .update_proposal_covering(&actor, p(1), Some("x"))
                .await,
        );
        assert_eq!(error.code(), codes::NOT_PROPOSAL_AUTHOR);
        assert_eq!(
            details(&error),
            json!({"proposal": "P-1", "author": "frontend"})
        );
    }
    let error = refusal(
        fx.reviews
            .update_proposal_covering(&by(&fx.frontend), p(7), Some("x"))
            .await,
    );
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
    fx.execute("INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (1, CAST('a' AS BLOB), 'add_add')")
        .await;
    fx.write("a", CONFLICTED);
    let error = refusal(
        fx.reviews
            .update_proposal_covering(&by(&fx.frontend), p(1), Some("x"))
            .await,
    );
    assert_eq!(error.code(), codes::UNRESOLVED_CONFLICTS);
    fx.reviews
        .close_proposal(&by(&fx.frontend), p(1), "done")
        .await
        .unwrap();
    let error = refusal(
        fx.reviews
            .update_proposal_covering(&by(&fx.frontend), p(1), Some("x"))
            .await,
    );
    assert_eq!(error.code(), codes::PROPOSAL_NOT_OPEN);
    assert_eq!(
        details(&error),
        json!({"proposal": "P-1", "state": "closed"})
    );
}

#[tokio::test]
async fn close_proposal_returns_task_to_in_progress_and_archives_review_checkouts() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.create(&fx.frontend).await.unwrap();
    fx.execute(&format!(
        "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 1, {0}, 0, 0);
         INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, proposal_id, state, created_at)
         VALUES (2, 1, {0}, 'review', '.fold/ws/r-1', 1, 1, 'active', 0), (3, 1, {0}, 'review', '.fold/ws/r-2', 1, 1, 'abandoned', 0)",
        fx.backend.id
    ))
    .await;
    let listed = fx
        .reviews
        .list_proposals(ProposalFilter {
            state: Some(ProposalState::Open),
            unclaimed: false,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(listed[0].review_claim, Some(fx.backend.name.clone()));
    fx.project.clock().advance(Duration::from_secs(3)).unwrap();
    let before = fx.last_seq().await;

    let detail = fx
        .reviews
        .close_proposal(&by(&fx.frontend), p(1), "  split into two  ")
        .await
        .unwrap();

    let summary = &detail.summary;
    assert_eq!(summary.state, ProposalState::Closed);
    assert_eq!(summary.close_reason.as_deref(), Some("split into two"));
    assert_eq!(summary.closed_at, Some(fx.project.now()));
    assert_eq!(summary.turn, None);
    assert_eq!(summary.review_claim, None);
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    let task_ws = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(task_ws.state, WorkspaceState::Active);
    let checkout = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(2)))
        .await
        .unwrap();
    assert_eq!(checkout.state, WorkspaceState::Archived);
    assert_eq!(checkout.closed_at, Some(fx.project.now()));
    let abandoned = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(3)))
        .await
        .unwrap();
    assert_eq!(abandoned.state, WorkspaceState::Abandoned);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE kind = 'review' AND release_reason = 'closed'")
            .await,
        1
    );

    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        [
            "claim.released",
            "workspace.archived",
            "proposal.closed",
            "task.status_changed"
        ]
    );
    let released = of_type(&appended, types::CLAIM_RELEASED)[0];
    assert_eq!(
        released.data,
        json!({"kind": "review", "proposal": "P-1", "worker": "backend", "reason": "closed"})
    );
    let archived = of_type(&appended, types::WORKSPACE_ARCHIVED)[0];
    assert_eq!(archived.workspace, Some(ws(2)));
    assert_eq!(archived.data, json!({"reason": "proposal_closed"}));
    let closed = of_type(&appended, types::PROPOSAL_CLOSED)[0];
    assert_eq!(closed.proposal, Some(p(1)));
    assert_eq!(closed.actor, EventActor::worker(&fx.frontend.name));
    assert_eq!(closed.data, json!({"reason": "split into two"}));

    let view = fx
        .workspaces
        .abandon_workspace(&by(&fx.frontend), ws(1), "start over")
        .await
        .unwrap()
        .workspace;
    assert_eq!(view.state, WorkspaceState::Abandoned);
}

#[tokio::test]
async fn close_proposal_is_for_the_author_agent_a_deciding_agent_or_a_human() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.create(&fx.frontend).await.unwrap();
    for actor in [by(&fx.backend), Actor::System] {
        let error = refusal(fx.reviews.close_proposal(&actor, p(1), "no").await);
        assert_eq!(error.code(), codes::NOT_PROPOSAL_AUTHOR);
        assert_eq!(
            details(&error),
            json!({"proposal": "P-1", "author": "frontend"})
        );
    }
    for reason in ["", "  ", &"x".repeat(501)] {
        let error = refusal(fx.reviews.close_proposal(&human(), p(1), reason).await);
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(details(&error), json!({"field": "reason"}));
    }
    let detail = fx
        .reviews
        .close_proposal(&reviewer("review"), p(1), "superseded")
        .await
        .unwrap();
    assert_eq!(detail.summary.state, ProposalState::Closed);
    let error = refusal(fx.reviews.close_proposal(&human(), p(1), "again").await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_OPEN);
    let error = refusal(fx.reviews.close_proposal(&human(), p(4), "gone").await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);

    fx.create(&fx.frontend).await.unwrap();
    let detail = fx
        .reviews
        .close_proposal(&human(), p(2), "not needed")
        .await
        .unwrap();
    assert_eq!(detail.summary.state, ProposalState::Closed);
    let closed = fx
        .events_after(0)
        .await
        .into_iter()
        .filter(|event| event.event_type == types::PROPOSAL_CLOSED)
        .map(|event| event.actor.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(closed, ["review", "human:tanim"]);
}

#[tokio::test]
async fn get_proposal_reports_the_turn() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.create(&fx.frontend).await.unwrap();
    let turn = async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .unwrap()
            .summary
            .turn
    };
    assert_eq!(turn().await, Some(Turn::WithReviewer));
    fx.review("request_changes").await;
    assert_eq!(turn().await, Some(Turn::WithAuthor));
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    fx.review("comment").await;
    assert_eq!(turn().await, Some(Turn::WithAuthor));
    fx.review("approve").await;
    assert_eq!(turn().await, Some(Turn::WithReviewer));
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    fx.execute("UPDATE proposals SET conflicted = 1").await;
    fx.recompute(1).await;
    assert_eq!(turn().await, Some(Turn::WithAuthor));
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    assert_eq!(Turn::WithAuthor.as_str(), "with_author");
    assert_eq!(Turn::WithReviewer.as_str(), "with_reviewer");

    let error = refusal(fx.reviews.get_proposal(p(1), Some(3)).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
    assert_eq!(
        details(&error),
        json!({"proposal": "P-1", "version": 3, "current_version": 1})
    );
    let error = refusal(fx.reviews.get_proposal(p(2), None).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
}

#[tokio::test]
async fn list_proposals_filters_by_state_and_review_claim() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.workspace(&fx.backend, 2).await;
    fx.create(&fx.frontend).await.unwrap();
    fx.reviews
        .close_proposal(&by(&fx.frontend), p(1), "redo")
        .await
        .unwrap();
    fx.create(&fx.frontend).await.unwrap();
    fx.reviews
        .create_proposal_covering(&by(&fx.backend), t(2), "Add the API", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let renewed = fx.project.now().as_millis();
    fx.execute(&format!(
        "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 3, {}, {renewed}, {renewed})",
        fx.frontend.id
    ))
    .await;
    let ids = async |filter: ProposalFilter| {
        fx.reviews
            .list_proposals(filter)
            .await
            .unwrap()
            .into_iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(ProposalFilter::default()).await, [p(1), p(2), p(3)]);
    let open = ProposalFilter {
        state: Some(ProposalState::Open),
        unclaimed: false,
        limit: None,
    };
    assert_eq!(ids(open).await, [p(2), p(3)]);
    let unclaimed = ProposalFilter {
        state: Some(ProposalState::Open),
        unclaimed: true,
        limit: None,
    };
    assert_eq!(ids(unclaimed).await, [p(2)]);
    let closed = ProposalFilter {
        state: Some(ProposalState::Closed),
        unclaimed: false,
        limit: None,
    };
    assert_eq!(ids(closed).await, [p(1)]);
    let newest = ProposalFilter {
        state: None,
        unclaimed: false,
        limit: Some(2),
    };
    assert_eq!(
        ids(newest).await,
        [p(2), p(3)],
        "a limit keeps the newest, in number order"
    );
    let none = fx
        .reviews
        .list_proposals(ProposalFilter {
            limit: Some(0),
            ..ProposalFilter::default()
        })
        .await
        .unwrap_err();
    assert_eq!(fold_core::FoldError::from(none).code(), "invalid_request");

    // A review claim whose lease has run out counts as unclaimed, since claim_review takes it over; the list still names its holder.
    let lease = Duration::from_secs(LEASE_MINUTES * 60);
    fx.project
        .clock()
        .advance(lease - Duration::from_millis(1))
        .unwrap();
    assert_eq!(ids(unclaimed).await, [p(2)]);
    fx.project
        .clock()
        .advance(Duration::from_millis(1))
        .unwrap();
    assert_eq!(ids(unclaimed).await, [p(2), p(3)]);
    let stale = fx.reviews.list_proposals(unclaimed).await.unwrap();
    assert_eq!(stale[1].review_claim, Some(fx.frontend.name.clone()));
}

#[tokio::test]
async fn retire_proposal_ends_a_proposal_for_reject_and_integration() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.workspace(&fx.backend, 2).await;
    fx.create(&fx.frontend).await.unwrap();
    fx.reviews
        .create_proposal_covering(&by(&fx.backend), t(2), "Add the API", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    fx.execute(&format!(
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, proposal_id, state, created_at)
         VALUES (3, 1, {0}, 'review', '.fold/ws/r-1', 1, 1, 'active', 0), (4, 2, {0}, 'review', '.fold/ws/r-2', 1, 2, 'missing', 0);
         INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 2, {0}, 0, 0)",
        fx.frontend.id
    ))
    .await;
    let now = fx.project.now();
    let fold = EventActor::fold();
    let retire = async |proposal: ProposalId, outcome: Retirement<'static>| {
        let mut buffer = EventBuffer::new();
        let task = fx
            .project
            .store()
            .write(async |tx| retire_proposal(tx, &mut buffer, &fold, proposal, outcome, now).await)
            .await;
        (task, buffer)
    };

    let (task, buffer) = retire(
        p(1),
        Retirement::Rejected {
            reason: "wrong approach",
        },
    )
    .await;
    assert_eq!(task.unwrap(), t(1));
    assert_eq!(names(buffer.events()), ["workspace.archived"]);
    assert_eq!(
        buffer.events()[0].data,
        json!({"reason": "proposal_rejected"})
    );
    let rejected = fx.reviews.get_proposal(p(1), None).await.unwrap().summary;
    assert_eq!(rejected.state, ProposalState::Closed);
    assert_eq!(rejected.close_reason.as_deref(), Some("wrong approach"));

    let (task, buffer) = retire(p(2), Retirement::Integrated).await;
    assert_eq!(task.unwrap(), t(2));
    assert_eq!(
        names(buffer.events()),
        ["claim.released", "workspace.archived"]
    );
    assert_eq!(
        buffer.events()[0].data,
        json!({"kind": "review", "proposal": "P-2", "worker": "frontend", "reason": "integrated"})
    );
    assert_eq!(buffer.events()[1].data, json!({"reason": "integrated"}));
    let integrated = fx.reviews.get_proposal(p(2), None).await.unwrap().summary;
    assert_eq!(integrated.state, ProposalState::Integrated);
    assert_eq!(integrated.close_reason, None);
    assert_eq!(integrated.closed_at, Some(now));
    assert_eq!(Retirement::Integrated.as_str(), "integrated");

    let (again, _) = retire(p(2), Retirement::Closed { reason: "x" }).await;
    assert_eq!(refusal(again).code(), codes::PROPOSAL_NOT_OPEN);
    let (missing, _) = retire(p(9), Retirement::Integrated).await;
    assert_eq!(refusal(missing).code(), codes::PROPOSAL_NOT_FOUND);
}

#[tokio::test]
async fn every_version_holds_its_own_checked_intent_whose_parts_cover_its_files() {
    let fx = Fixture::new().await;
    fx.workspace(&fx.frontend, 1).await;
    fx.write("src/lib.rs", "pub fn y() {}\n");
    fx.write("src/report.rs", "pub fn total() {}\n");
    let before = fx.last_seq().await;
    let report = intent_of(
        "  The report sums the tasks.\r\n\r\nIt leaves the schema as it is.  ",
        &[
            ("Rename", "Renames x to y.", &["src/lib.rs"]),
            ("Report", "Adds total.", &["src/report.rs"]),
        ],
    );
    let refusals: [(ProposalIntent, serde_json::Value); 5] = [
        (
            ProposalIntent {
                summary: " \n ".to_owned(),
                ..report.clone()
            },
            json!({"field": "intent.summary", "reason": "empty"}),
        ),
        (
            ProposalIntent {
                summary: "x".repeat(1201),
                ..report.clone()
            },
            json!({"field": "intent.summary", "reason": "too_long", "max_chars": 1200}),
        ),
        (
            intent_of(
                "Renames x.",
                &[("Rename", "Renames x to y.", &["src/lib.rs"])],
            ),
            json!({"field": "intent.parts", "reason": "part_files_uncovered", "uncovered": ["src/report.rs"], "uncovered_count": 1, "outside": [], "outside_count": 0}),
        ),
        (
            intent_of(
                "Renames x.",
                &[
                    ("Rename", "Renames x to y.", &["src/lib.rs", "README.md"]),
                    ("Report", "Adds total.", &["src/report.rs"]),
                ],
            ),
            json!({"field": "intent.parts", "reason": "part_file_outside_diff", "uncovered": [], "uncovered_count": 0, "outside": ["README.md"], "outside_count": 1}),
        ),
        (
            intent_of(
                "Renames x.",
                &[
                    ("Rename", "Renames x to y.", &["src/lib.rs"]),
                    ("Report", "Adds total.", &["src/report.rs", "src/lib.rs"]),
                ],
            ),
            json!({"field": "intent.parts", "reason": "file_in_two_parts", "files": ["src/lib.rs"], "files_count": 1}),
        ),
    ];
    for (intent, expected) in refusals {
        let error = refusal(
            fx.reviews
                .create_proposal(&by(&fx.frontend), t(1), "Add the report", intent, None)
                .await,
        );
        assert_eq!(error.code(), codes::INVALID_INTENT);
        assert_eq!(details(&error), expected);
        assert!(error.hint().unwrap().contains("call create_proposal again"));
    }
    let outside = refusal(
        fx.reviews
            .create_proposal(
                &by(&fx.frontend),
                t(1),
                "Add the report",
                intent_of(
                    "Renames x.",
                    &[("Rename", "Renames x to y.", &["../lib.rs"])],
                ),
                None,
            )
            .await,
    );
    assert_eq!(outside.code(), codes::INVALID_PATH);
    assert!(fx.events_after(before).await.is_empty());
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);

    let v1 = fx
        .reviews
        .create_proposal(
            &by(&fx.frontend),
            t(1),
            "Add the report",
            report.clone(),
            None,
        )
        .await
        .unwrap()
        .detail()
        .unwrap();
    let stored = v1.selected.intent.clone().unwrap();
    assert_eq!(
        stored.summary,
        "The report sums the tasks.\n\nIt leaves the schema as it is."
    );
    assert_eq!(stored.parts, report.parts);
    assert!(stored.contracts.is_empty() && stored.decisions.is_empty());
    let error = refusal(
        fx.reviews
            .update_proposal(&by(&fx.frontend), p(1), ProposalIntent::default(), None)
            .await,
    );
    assert_eq!(error.code(), codes::INVALID_INTENT);
    assert!(
        error
            .hint()
            .unwrap()
            .starts_with("call update_proposal again")
    );

    // Only the intent changes, so the new version holds the same files.
    let total = ProposalIntent {
        summary: "The report sums the tasks and their total.".to_owned(),
        decisions: vec!["The total counts closed tasks too.".to_owned()],
        ..report.clone()
    };
    let v2 = fx
        .reviews
        .update_proposal(&by(&fx.frontend), p(1), total.clone(), None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_eq!(v2.selected.version, 2);
    assert_eq!(v2.selected.tree, v1.selected.tree);
    assert_eq!(
        v2.versions
            .iter()
            .map(|v| v.intent.clone())
            .collect::<Vec<_>>(),
        [Some(stored), Some(total.clone())]
    );
    let error = refusal(
        fx.reviews
            .update_proposal(&by(&fx.frontend), p(1), total, None)
            .await,
    );
    assert_eq!(error.code(), codes::VERSION_UNCHANGED);
    assert!(error.hint().unwrap().contains("the intent"));
}

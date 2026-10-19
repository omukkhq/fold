use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::object::ObjectKind;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventFilter, RevisionId, TaskDescription, TaskId,
    TaskTitle, WorkerName, WorkspaceId,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::router;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::events;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    self, advance_main, create_initial_revision, initial_revision, insert_revision,
};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// The files of `r1`.
pub const BASE: &[(&str, &[u8])] = &[
    ("README.md", b"# demo\n"),
    ("data/noisy.bin", b"seed\n"),
    ("docs/old.md", b"# Old\nkeep this\nand this\n"),
    ("logo.png", b"\x89PNG\0\x01"),
    ("src/lib.rs", b"pub fn x() {}\n"),
];

pub fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

pub fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

pub fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

/// [`BASE`] with `changed` replacing or adding files, every blob written.
pub fn tree(objects: &ObjectStore, changed: &[(&str, &[u8])]) -> FlatTree {
    let mut tree = FlatTree::new();
    for (path, content) in BASE.iter().chain(changed) {
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, content).unwrap(),
            },
        );
    }
    tree
}

/// A project open in daemon state as `demo`, the way the daemon opens it: the open reconciles its workspaces.
pub struct Opened {
    pub project: TestProject,
    pub state: AppState,
    pub handle: Arc<ProjectHandle>,
}

impl Opened {
    /// `r1` holds [`BASE`]; agent `frontend`; tasks `T-1` and `T-2`.
    pub async fn new() -> Self {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let root = write_initial_tree(&objects, tree(&objects, &[])).unwrap();
        let revision = initial_revision(root, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let (state, handle) = open(&project).await;
        let agents = AgentService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        let mut session = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut session, &AgentName::parse("frontend").unwrap(), &[])
            .await
            .unwrap();
        for _ in 0..2 {
            crate::common::roles::lead_work_areas(&handle.store).await;
            handle
                .tasks
                .create_task(
                    &agent("frontend"),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: crate::common::roles::work_labels(),
                    },
                )
                .await
                .unwrap();
        }
        Self {
            project,
            state,
            handle,
        }
    }

    /// Closes the project as a stopping daemon does, runs `while_closed`, and opens the project again, which reconciles its workspaces.
    pub async fn restart(&mut self, while_closed: impl FnOnce()) {
        for live in self.handle.live.all() {
            self.handle.live.detach(live.view.workspace).await;
        }
        self.handle.store.close().await.unwrap();
        while_closed();
        let (state, handle) = open(&self.project).await;
        self.state = state;
        self.handle = handle;
    }

    pub fn workspaces(&self) -> WorkspaceService {
        WorkspaceService::for_project(&self.handle)
    }

    /// Creates the workspace of `task` for `frontend` and returns its id and directory.
    pub async fn create(&self, task: u32) -> (WorkspaceId, PathBuf) {
        let view = self
            .workspaces()
            .create_workspace(&agent("frontend"), t(task))
            .await
            .unwrap();
        (
            view.workspace.id,
            self.project.paths().root().join(view.workspace.path),
        )
    }

    /// Records a revision on top of `main` holding [`BASE`] with `changed`, and moves `main` to it.
    pub async fn move_main(&self, changed: &[(&str, &[u8])]) -> RevisionId {
        let store = &self.handle.store;
        let head = workspaces::head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.project.paths().fold_dir()).unwrap();
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, tree(&objects, changed)).unwrap(),
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

    /// `GET /api/v1/projects/demo{path}` through the router.
    pub async fn get(&self, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(format!("/api/v1/projects/demo{path}"))
            .body(Body::empty())
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// The stored events of `event_type`, oldest first.
    pub async fn events(&self, event_type: &str) -> Vec<Event> {
        events::read_after(self.handle.store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type.as_str() == event_type)
            .collect()
    }
}

async fn open(project: &TestProject) -> (AppState, Arc<ProjectHandle>) {
    let clock = Arc::new(project.clock().clone());
    let state: AppState = Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default(),
        clock: clock.clone(),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1".parse().unwrap(),
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    });
    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "Demo".to_owned(),
        root: project.paths().root().to_string_lossy().into_owned(),
    };
    let handle = state.projects.open(&row, clock).await.unwrap();
    caught_up(&handle).await;
    (state, handle)
}

/// Waits until the project's commit job has recorded the commit of every revision, so the `git.committed` events of its first run are stored before a test counts events.
pub async fn caught_up(handle: &ProjectHandle) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !workspaces::uncommitted_revisions(handle.store.read())
            .await
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the commit job records every revision's commit");
}

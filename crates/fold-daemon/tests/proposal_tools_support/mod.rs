use std::sync::Arc;

use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{Actor, AgentName, HumanName, TaskDescription, TaskId, TaskTitle};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::projects::{DEFAULT_SESSION_LIFETIME, ensure_human};
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};

pub const LIB: &str = "pub fn x() {}\n";

pub fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

pub fn human(name: &str) -> Actor {
    Actor::human(HumanName::parse(name).unwrap())
}

/// An open project the way the daemon holds it: `r1` with `src/lib.rs` and `README.md`, the tasks `T-1` and `T-2`, and the human `tanim` in the registry. Every service comes from the project handle, as the transports build them.
pub struct Fixture {
    pub project: TestProject,
    pub state: AppState,
    pub handle: Arc<ProjectHandle>,
}

impl Fixture {
    pub async fn new() -> Self {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        for (path, content) in [("src/lib.rs", LIB), ("README.md", "# demo\n")] {
            let hash = objects.write(ObjectKind::Blob, content.as_bytes()).unwrap();
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
        ensure_human(
            project.registry(),
            project.clock(),
            &HumanName::parse("tanim").unwrap(),
        )
        .await
        .unwrap();
        for title in ["report", "export"] {
            handle
                .tasks
                .create_task(
                    &human("tanim"),
                    CreateTask {
                        title: TaskTitle::parse(title).unwrap(),
                        description: TaskDescription::new("what to build"),
                        labels: Vec::new(),
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

    pub fn agents(&self) -> AgentService {
        self.handle.agents()
    }

    pub fn workspaces(&self) -> WorkspaceService {
        WorkspaceService::for_project(&self.handle, Git::default())
    }

    pub fn reviews(&self) -> ReviewService {
        self.handle.reviews(&self.state.registry)
    }

    /// Registers `name` in a session of its own and returns its main worker's actor with its tags as they are now.
    pub async fn agent(&self, name: &str) -> Actor {
        let agents = self.agents();
        let mut session = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut session, &AgentName::parse(name).unwrap(), &[])
            .await
            .unwrap();
        agents.actor(&session, None).await.unwrap()
    }

    /// Writes `content` to `path` in the task workspace of `task`.
    pub fn write(&self, task: u32, path: &str, content: &str) {
        let file = self
            .project
            .paths()
            .root()
            .join(format!("ws/t-{task}"))
            .join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    /// `author` creates the workspace of `task`, writes `content` to `src/lib.rs` and opens a proposal.
    pub async fn proposal(&self, author: &Actor, task: u32, content: &str) {
        self.workspaces()
            .create_workspace(author, t(task))
            .await
            .unwrap();
        self.write(task, "src/lib.rs", content);
        self.reviews()
            .create_proposal(author, t(task), "Change the library", Some("tests pass"))
            .await
            .unwrap();
    }
}

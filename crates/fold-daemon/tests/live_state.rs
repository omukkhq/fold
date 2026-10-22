use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use fold_core::events::types;
use fold_core::object::ObjectKind;
use fold_core::reviews::Verdict;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, HumanName, ProjectConfig, ProposalId, RevisionId,
    TaskDescription, TaskId, TaskTitle, Timestamp, WorkerName, WorkspaceId,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::events::{ChangeKind, LiveWorkspace};
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::jobs::{JobProgress, spawn_live};
use fold_daemon::live::{LiveConfig, LiveState, WatchMode, WorkspaceLive};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_daemon::services::projects::{DEFAULT_SESSION_LIFETIME, gc};
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceService};
use fold_store::agents::WorkerRow;
use fold_store::projects::ProjectRow;
use fold_store::testing::{TestProject, eventually};
use fold_store::workspaces::{
    self, CheckpointKind, CheckpointReason, advance_main, create_initial_revision,
    initial_revision, insert_revision,
};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use fold_vcs::scan::SkipReason;
use fold_vcs::snapshot::WarningReason;
use fold_vcs::watch::{WatchBackend, WatchConfig, WatchRefused};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::watch;
use tower::ServiceExt;

const LEASE_MINUTES: u64 = 5;
const WAIT: Duration = Duration::from_secs(20);
/// Longer than the test debounce and maximum wait together, so a second batch would have left by then.
const QUIET: Duration = Duration::from_millis(1_500);

const BASE: &[(&str, &[u8])] = &[
    ("README.md", b"# demo\n"),
    ("data/noisy.bin", b"seed\n"),
    ("docs/guide.md", b"# Guide\nread me\n"),
    ("src/lib.rs", b"pub fn x() {}\n"),
];

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

#[cfg(test)]
fn config() -> LiveConfig {
    LiveConfig {
        watch: WatchConfig {
            debounce: Duration::from_millis(200),
            max_wait: Duration::from_millis(800),
            degraded_interval: Duration::from_secs(10),
        },
        rescan: Duration::from_secs(60),
        max_file_bytes: 1 << 26,
    }
}

/// A watcher whose notifications never arrive.
struct Silent;

impl WatchBackend for Silent {
    fn watch_dir(&mut self, _dir: &Path) -> Result<(), WatchRefused> {
        Ok(())
    }

    fn unwatch_dir(&mut self, _dir: &Path) {}
}

/// A platform without native notifications.
struct Unsupported;

impl WatchBackend for Unsupported {
    fn watch_dir(&mut self, _dir: &Path) -> Result<(), WatchRefused> {
        Err(WatchRefused::Unsupported)
    }

    fn unwatch_dir(&mut self, _dir: &Path) {}
}

/// Builds the live state of a fixture from its project and bus.
type MakeLive = Box<dyn FnOnce(&TestProject, &EventBus) -> LiveState>;

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    live: LiveState,
    tasks: TaskService,
    workspaces: WorkspaceService,
    frontend: WorkerRow,
    backend: WorkerRow,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        Self::build(None).await
    }

    async fn with_backend(backend: impl WatchBackend + 'static) -> Self {
        Self::build(Some(Box::new(
            move |project: &TestProject, bus: &EventBus| {
                LiveState::with_backend(
                    project.store().clone(),
                    bus.clone(),
                    Arc::new(project.clock().clone()),
                    project.paths().clone(),
                    config(),
                    Arc::default(),
                    backend,
                )
            },
        )))
        .await
    }

    /// `r1` holds [`BASE`]; two tasks and the agents `frontend` and `backend`.
    async fn build(make: Option<MakeLive>) -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let tree = write_initial_tree(&objects, base_tree(&objects, &[])).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let live = match make {
            Some(make) => make(&project, &bus),
            None => LiveState::new(
                project.store().clone(),
                bus.clone(),
                clock.clone(),
                project.paths().clone(),
                config(),
                Arc::default(),
            ),
        };
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            LEASE_MINUTES,
        )
        .with_live(live.clone());
        let workspaces = WorkspaceService::new(
            project.store().clone(),
            bus.clone(),
            clock,
            project.paths().clone(),
            LEASE_MINUTES,
        )
        .with_live(live.clone());
        let creator = agent("frontend");
        crate::common::roles::lead_frontend_area(project.store()).await;
        for _ in 0..2 {
            tasks
                .create_task(
                    &creator,
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: crate::common::roles::work_labels(),
                    },
                )
                .await
                .unwrap();
        }
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        // The tasks are in the area that `frontend` leads, so the main agent `backend` is the other agent that claims them.
        crate::common::roles::give_main_tag(project.store(), "backend").await;
        Self {
            project,
            bus,
            live,
            tasks,
            workspaces,
            frontend,
            backend,
        }
    }

    /// Creates the workspace of `task` for `worker` and returns its id and directory.
    async fn create(&self, worker: &WorkerRow, task: u32) -> (WorkspaceId, PathBuf) {
        let view = self
            .workspaces
            .create_workspace(&by(worker), t(task))
            .await
            .unwrap()
            .workspace;
        (view.id, self.project.paths().root().join(view.path))
    }

    /// Records a revision on top of `main` that holds [`BASE`] with `changed` replacing its files, and moves `main` to it, as the integration procedure would.
    async fn move_main(&self, changed: &[(&str, &[u8])]) -> RevisionId {
        let store = self.project.store();
        let head = workspaces::head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.project.paths().fold_dir()).unwrap();
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, base_tree(&objects, changed)).unwrap(),
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

    fn advance(&self, seconds: u64) -> Timestamp {
        self.project
            .clock()
            .advance(Duration::from_secs(seconds))
            .unwrap()
    }

    /// Waits until the state of `workspace` passes `check`, letting the pipelines look at the clock as the live job does.
    async fn wait_for(
        &self,
        workspace: WorkspaceId,
        check: impl Fn(&WorkspaceLive) -> bool,
    ) -> Arc<WorkspaceLive> {
        eventually(WAIT, async || {
            self.live.tick();
            match self.live.get(workspace) {
                Some(state) if check(&state) => Ok(state),
                other => Err(other.map(|state| files(&state.view))),
            }
        })
        .await
    }

    async fn row(&self, workspace: WorkspaceId) -> (u32, Option<Timestamp>) {
        self.live.tick();
        let row = workspaces::get_workspace(self.project.store().read(), workspace)
            .await
            .unwrap()
            .unwrap();
        (row.changed_files, row.last_activity_at)
    }
}

/// [`BASE`] as a flat tree with `changed` replacing its files, with every blob written.
#[cfg(test)]
fn base_tree(objects: &ObjectStore, changed: &[(&str, &[u8])]) -> FlatTree {
    let mut tree = FlatTree::new();
    for (path, content) in BASE.iter().chain(changed) {
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
    agents.resolve_worker(&ctx).await.unwrap()
}

type FileRow = (String, Option<String>, ChangeKind, u32, u32);

#[cfg(test)]
fn files(view: &LiveWorkspace) -> Vec<FileRow> {
    view.files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                file.previous_path.clone(),
                file.change,
                file.added_lines,
                file.removed_lines,
            )
        })
        .collect()
}

#[cfg(test)]
fn row(
    path: &str,
    previous: Option<&str>,
    change: ChangeKind,
    added: u32,
    removed: u32,
) -> FileRow {
    (
        path.to_owned(),
        previous.map(str::to_owned),
        change,
        added,
        removed,
    )
}

/// The live events for `workspace` that the bus holds right now.
#[cfg(test)]
fn drain_live(receiver: &mut Receiver<BusMessage>, workspace: WorkspaceId) -> Vec<LiveWorkspace> {
    let mut found = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(BusMessage::Live(event)) if event.workspace == Some(workspace) => {
                assert_eq!(event.event_type, types::WORKSPACE_CHANGED);
                found.push(serde_json::from_value(event.data.clone()).unwrap());
            }
            Ok(_) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return found,
            Err(TryRecvError::Lagged(_)) => panic!("the test subscriber lagged"),
        }
    }
}

/// Waits up to [`QUIET`] for another live event for `workspace`; `None` when none came.
#[cfg(test)]
async fn next_live(
    receiver: &mut Receiver<BusMessage>,
    workspace: WorkspaceId,
) -> Option<LiveWorkspace> {
    let wait = async {
        loop {
            match receiver.recv().await {
                Ok(BusMessage::Live(event)) if event.workspace == Some(workspace) => {
                    return serde_json::from_value(event.data.clone()).unwrap();
                }
                Ok(_) => {}
                Err(error) => panic!("the bus failed: {error}"),
            }
        }
    };
    tokio::time::timeout(QUIET, wait).await.ok()
}

#[tokio::test]
async fn adds_edits_deletes_and_renames_show_in_the_live_state() {
    let fx = Fixture::new().await;
    let mut bus = fx.bus.subscribe();
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let first = fx.live.get(id).unwrap();
    assert_eq!(first.view.task, Some(t(1)));
    assert_eq!(first.view.worker, "frontend");
    assert_eq!(first.view.base.number(), 1);
    assert!(first.view.files.is_empty());
    assert_eq!(first.view.summary, "no changes");
    assert_eq!(first.watch, WatchMode::Watching);
    let created = first.view.last_activity_at;
    assert_eq!(drain_live(&mut bus, id), std::slice::from_ref(&first.view));

    let added_at = fx.advance(5);
    fs::write(dir.join("notes.txt"), "one\ntwo\n").unwrap();
    let added = fx.wait_for(id, |state| !state.view.files.is_empty()).await;
    assert_eq!(
        files(&added.view),
        [row("notes.txt", None, ChangeKind::Added, 2, 0)]
    );
    assert_eq!(added.view.last_activity_at, added_at);
    assert!(added_at > created);

    fx.advance(5);
    fs::write(dir.join("README.md"), "# demo\nmore\n").unwrap();
    let edited = fx.wait_for(id, |state| state.view.files.len() == 2).await;
    assert_eq!(
        files(&edited.view)[0],
        row("README.md", None, ChangeKind::Modified, 1, 0)
    );

    fx.advance(5);
    fs::remove_file(dir.join("src/lib.rs")).unwrap();
    let deleted = fx.wait_for(id, |state| state.view.files.len() == 3).await;
    assert_eq!(
        files(&deleted.view)[2],
        row("src/lib.rs", None, ChangeKind::Deleted, 0, 1)
    );

    let renamed_at = fx.advance(5);
    fs::rename(dir.join("docs/guide.md"), dir.join("docs/manual.md")).unwrap();
    let renamed = fx
        .wait_for(id, |state| {
            state
                .view
                .files
                .iter()
                .any(|file| file.change == ChangeKind::Renamed)
        })
        .await;
    assert_eq!(
        files(&renamed.view),
        [
            row("README.md", None, ChangeKind::Modified, 1, 0),
            row(
                "docs/manual.md",
                Some("docs/guide.md"),
                ChangeKind::Renamed,
                0,
                0
            ),
            row("notes.txt", None, ChangeKind::Added, 2, 0),
            row("src/lib.rs", None, ChangeKind::Deleted, 0, 1),
        ]
    );
    assert_eq!(
        renamed.view.summary,
        "4 files: 1 modified, 1 added, 1 deleted, 1 renamed (+3 -1)"
    );
    assert_eq!(renamed.view.last_activity_at, renamed_at);
    assert_eq!(renamed.updated_at, renamed_at);
    assert!(renamed.warnings.is_empty());
    assert_eq!(fx.live.all(), std::slice::from_ref(&renamed));
    let streamed = drain_live(&mut bus, id);
    assert_eq!(streamed.last(), Some(&renamed.view));
}

#[tokio::test]
async fn ten_writes_inside_one_debounce_window_publish_one_event() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let mut bus = fx.bus.subscribe();
    for round in 0..10 {
        fs::write(dir.join("README.md"), format!("# demo\nround {round}\n")).unwrap();
    }
    let done = fx
        .wait_for(id, |state| {
            files(&state.view) == [row("README.md", None, ChangeKind::Modified, 1, 0)]
        })
        .await;
    let first = next_live(&mut bus, id).await;
    assert_eq!(first.as_ref(), Some(&done.view));
    assert_eq!(next_live(&mut bus, id).await, None);
    let reread = fs::read_to_string(dir.join("README.md")).unwrap();
    assert_eq!(reread, "# demo\nround 9\n");
}

#[tokio::test]
async fn a_touch_and_an_ignored_write_leave_the_last_activity_alone() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let ignored_at = fx.advance(5);
    fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    let ruled = fx.live.rescan(id).await.unwrap();
    assert_eq!(ruled.view.last_activity_at, ignored_at);
    assert_eq!(ruled.view.files.len(), 1);

    let later = fx.advance(30);
    let readme = OpenOptions::new()
        .write(true)
        .open(dir.join("README.md"))
        .unwrap();
    readme
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(3_600))
        .unwrap();
    drop(readme);
    fs::create_dir_all(dir.join("target")).unwrap();
    fs::write(dir.join("target/out.o"), b"\x7fELF").unwrap();
    let after = fx.live.rescan(id).await.unwrap();
    assert_eq!(after.view.last_activity_at, ignored_at);
    assert_eq!(after.view.files, ruled.view.files);
    assert_eq!(after.updated_at, later);

    let edited_at = fx.advance(1);
    fs::write(dir.join("README.md"), "# demo\nchanged\n").unwrap();
    let edited = fx.live.rescan(id).await.unwrap();
    assert_eq!(edited.view.last_activity_at, edited_at);
}

#[tokio::test]
async fn two_workspaces_of_two_workers_stay_independent() {
    let fx = Fixture::new().await;
    let (front, front_dir) = fx.create(&fx.frontend, 1).await;
    let (back, back_dir) = fx.create(&fx.backend, 2).await;
    assert_ne!(front_dir, back_dir);
    assert_eq!(fx.live.get(back).unwrap().view.worker, "backend");
    fs::write(front_dir.join("only-front.txt"), "front\n").unwrap();
    fx.wait_for(front, |state| state.view.files.len() == 1)
        .await;
    fs::write(back_dir.join("README.md"), "# back\n").unwrap();
    let back_state = fx.wait_for(back, |state| state.view.files.len() == 1).await;
    assert_eq!(
        files(&back_state.view),
        [row("README.md", None, ChangeKind::Modified, 1, 1)]
    );
    let front_state = fx.live.rescan(front).await.unwrap();
    assert_eq!(
        files(&front_state.view),
        [row("only-front.txt", None, ChangeKind::Added, 1, 0)]
    );
    assert_eq!(
        fx.live
            .all()
            .iter()
            .map(|state| state.view.workspace)
            .collect::<Vec<_>>(),
        [front, back]
    );
}

#[tokio::test]
async fn the_rescan_tick_finds_a_change_without_the_watcher() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    fs::write(dir.join("unseen.txt"), "nobody told\n").unwrap();
    fx.advance(59);
    let early =
        tokio::time::timeout(QUIET, fx.wait_for(id, |state| !state.view.files.is_empty())).await;
    assert!(early.is_err(), "nothing scans before the tick is due");
    fx.advance(1);
    let found = fx.wait_for(id, |state| !state.view.files.is_empty()).await;
    assert_eq!(
        files(&found.view),
        [row("unseen.txt", None, ChangeKind::Added, 1, 0)]
    );
}

#[tokio::test]
async fn abandon_and_archive_detach_the_state_and_stop_the_pipeline() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let mut bus = fx.bus.subscribe();
    fx.workspaces
        .abandon_workspace(&by(&fx.frontend), id, "the approach does not work")
        .await
        .unwrap();
    assert!(fx.live.get(id).is_none());
    assert!(fx.live.all().is_empty());
    let WorkspaceError::Fold(error) = fx.live.rescan(id).await.unwrap_err() else {
        panic!("a detached workspace is a refusal");
    };
    assert_eq!(error.code(), "workspace_not_active");
    fs::write(dir.join("late.txt"), "after the end\n").unwrap();
    assert_eq!(next_live(&mut bus, id).await, None);
    assert!(fx.live.get(id).is_none());
    // Detaching twice, or a workspace never attached, does nothing.
    fx.live.detach(id).await;
    fx.live.detach(ws(99)).await;

    let (second, _) = fx.create(&fx.backend, 2).await;
    assert!(fx.live.get(second).is_some());
    fx.tasks
        .archive_task(&agent("backend"), t(2))
        .await
        .unwrap();
    assert!(fx.live.get(second).is_none());
    let WorkspaceError::Fold(error) = fx.live.attach(second).await.unwrap_err() else {
        panic!("an archived workspace is a refusal");
    };
    assert_eq!(error.code(), "workspace_not_active");
    let WorkspaceError::Fold(error) = fx.live.attach(ws(99)).await.unwrap_err() else {
        panic!("an unknown workspace is a refusal");
    };
    assert_eq!(error.code(), "workspace_not_found");
}

#[tokio::test]
async fn the_persisted_columns_update_at_most_once_per_ten_seconds() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let created = fx.row(id).await;
    assert_eq!(created.0, 0);
    let first_at = fx.advance(1);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    eventually(WAIT, async || match fx.row(id).await {
        (1, Some(at)) if at == first_at => Ok(()),
        other => Err(other),
    })
    .await;

    let second_at = fx.advance(2);
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    fx.wait_for(id, |state| state.view.files.len() == 2).await;
    let held = tokio::time::timeout(
        QUIET,
        eventually(WAIT, async || match fx.row(id).await {
            (2, _) => Ok(()),
            other => Err(other),
        }),
    )
    .await;
    assert!(held.is_err(), "a second write inside 10 s waits");
    assert_eq!(fx.row(id).await, (1, Some(first_at)));

    fx.advance(8);
    eventually(WAIT, async || match fx.row(id).await {
        (2, Some(at)) if at == second_at => Ok(()),
        other => Err(other),
    })
    .await;
}

#[tokio::test]
async fn a_detach_writes_the_summary_the_interval_held_back() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let first_at = fx.advance(1);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    eventually(WAIT, async || match fx.row(id).await {
        (1, Some(at)) if at == first_at => Ok(()),
        other => Err(other),
    })
    .await;
    let second_at = fx.advance(2);
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    fx.wait_for(id, |state| state.view.files.len() == 2).await;
    assert_eq!(
        fx.row(id).await,
        (1, Some(first_at)),
        "the interval holds it"
    );

    fx.live.detach(id).await;
    assert_eq!(fx.row(id).await, (2, Some(second_at)));
}

#[tokio::test]
async fn a_file_rewritten_on_every_read_keeps_its_base_entry_with_a_warning() {
    let fx = Fixture::new().await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let noisy = dir.join("data/noisy.bin");
    let file = OpenOptions::new().write(true).open(&noisy).unwrap();
    file.set_len(8 << 20).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let writer = {
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            let mut step = 0u64;
            while running.load(Ordering::Relaxed) {
                step += 1;
                file.set_len((8 << 20) + step % 2).unwrap();
            }
        })
    };
    fs::write(dir.join("README.md"), "# demo\nsteady\n").unwrap();
    // A writer thread the scheduler holds back for three whole reads lets one scan see the file steady, so the workspace is scanned until a scan sees it change.
    let state = tokio::time::timeout(WAIT, async {
        loop {
            match fx.live.rescan(id).await {
                Ok(state) if state.warnings.is_empty() => {}
                other => return other,
            }
        }
    })
    .await
    .expect("no scan saw the file change");
    running.store(false, Ordering::Relaxed);
    writer.join().unwrap();
    let state = state.unwrap();
    assert_eq!(state.warnings.len(), 1);
    assert_eq!(state.warnings[0].path.to_string(), "data/noisy.bin");
    assert_eq!(state.warnings[0].reason, WarningReason::Unstable);
    assert_eq!(
        files(&state.view),
        [row("README.md", None, ChangeKind::Modified, 1, 0)]
    );
}

#[tokio::test]
async fn a_file_over_the_size_limit_keeps_its_base_entry_with_a_warning() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    fs::write(dir.join("data/noisy.bin"), vec![1u8; (1 << 26) + 1]).unwrap();
    let state = fx.live.rescan(id).await.unwrap();
    assert!(state.view.files.is_empty());
    assert_eq!(state.warnings.len(), 1);
    assert_eq!(
        state.warnings[0].reason,
        WarningReason::Skipped(SkipReason::TooLarge)
    );
    fs::write(dir.join("data/noisy.bin"), "small again\n").unwrap();
    let cleared = fx.live.rescan(id).await.unwrap();
    assert!(cleared.warnings.is_empty());
    assert_eq!(cleared.view.files.len(), 1);
}

#[tokio::test]
async fn a_degraded_watch_is_recorded_with_its_reason() {
    let fx = Fixture::with_backend(Unsupported).await;
    let mut bus = fx.bus.subscribe();
    let (id, _) = fx.create(&fx.frontend, 1).await;
    let event = eventually(WAIT, async || {
        loop {
            match bus.try_recv() {
                Ok(BusMessage::Durable(event))
                    if event.event_type == types::WORKSPACE_WATCH_DEGRADED =>
                {
                    return Ok(event);
                }
                Ok(_) => {}
                Err(error) => return Err(error),
            }
        }
    })
    .await;
    assert_eq!(event.workspace, Some(id));
    assert_eq!(event.task, Some(t(1)));
    assert_eq!(event.actor.as_str(), "fold");
    assert_eq!(event.data["reason"], "no_native_watcher");
    fx.wait_for(id, |state| state.watch == WatchMode::Degraded)
        .await;
}

#[tokio::test]
async fn proposals_and_checkpoints_update_the_state_after_their_commit() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, _) = fx.create(&fx.frontend, 1).await;
    let mut bus = fx.bus.subscribe();
    let proposal: ProposalId = "P-3".parse().unwrap();
    fx.live.set_proposal(id, Some(proposal), None).await;
    assert_eq!(fx.live.get(id).unwrap().view.proposal, Some(proposal));
    fx.live.set_proposal(id, Some(proposal), None).await;
    assert_eq!(drain_live(&mut bus, id).len(), 1);

    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    let tree = write_initial_tree(&objects, FlatTree::new()).unwrap();
    let now = fx.project.now();
    let checkpoints = fx
        .project
        .store()
        .write(async |tx| {
            let auto = workspaces::insert_checkpoint(
                tx,
                id,
                tree,
                CheckpointKind::Auto,
                CheckpointReason::BeforeSync,
                None,
                now,
            )
            .await?;
            let mut pruned = workspaces::insert_checkpoint(
                tx,
                id,
                tree,
                CheckpointKind::Explicit,
                CheckpointReason::Explicit,
                Some("first try"),
                now,
            )
            .await?;
            pruned.pruned_at = Some(now);
            Ok::<_, fold_store::StoreError>(vec![auto, pruned])
        })
        .await
        .unwrap();
    fx.live.set_checkpoints(id, &checkpoints);
    let state = fx.live.get(id).unwrap();
    assert_eq!(state.view.checkpoints.len(), 1);
    assert!(state.view.checkpoints[0].automatic);
    assert_eq!(state.view.checkpoints[0].id, checkpoints[0].id);
    assert_eq!(drain_live(&mut bus, id).len(), 1);
    // A detached workspace ignores later updates.
    fx.live.detach(id).await;
    fx.live.set_proposal(id, None, None).await;
    assert!(fx.live.get(id).is_none());

    // Attaching again reads the stored checkpoints, which the store has not pruned, and attaching twice only rescans.
    let again = fx.live.attach(id).await.unwrap();
    assert_eq!(again.view.checkpoints.len(), 2);
    assert_eq!(
        again.view.checkpoints[1].label.as_deref(),
        Some("first try")
    );
    assert_eq!(fx.live.attach(id).await.unwrap().view, again.view);
}

#[tokio::test]
async fn sync_and_restore_move_the_base_the_live_state_compares_against() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let actor = by(&fx.frontend);
    fs::write(dir.join("README.md"), "# demo\nmine\n").unwrap();
    let mine = [row("README.md", None, ChangeKind::Modified, 1, 0)];
    let main = fx.move_main(&[("src/lib.rs", b"pub fn y() {}\n")]).await;
    let mut bus = fx.bus.subscribe();

    let synced = fx.workspaces.sync_workspace(&actor, id).await.unwrap();
    assert_eq!(synced.base, main);
    assert_eq!(
        fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
        "pub fn y() {}\n"
    );
    let after_sync = fx.live.get(id).unwrap();
    assert_eq!(after_sync.view.base, main);
    assert_eq!(
        files(&after_sync.view),
        mine,
        "main's own change is not the workspace's"
    );
    let before_sync = synced.checkpoint.unwrap();
    assert_eq!(
        after_sync
            .view
            .checkpoints
            .iter()
            .map(|checkpoint| checkpoint.id)
            .collect::<Vec<_>>(),
        [before_sync]
    );
    assert_eq!(drain_live(&mut bus, id).last(), Some(&after_sync.view));

    let restored = fx
        .workspaces
        .restore_checkpoint(&actor, before_sync)
        .await
        .unwrap();
    assert_eq!(restored.base.number(), 1);
    let after_restore = fx.live.get(id).unwrap();
    assert_eq!(after_restore.view.base, restored.base);
    assert_eq!(files(&after_restore.view), mine);
    assert_eq!(after_restore.view.checkpoints.len(), 2);
    assert_eq!(after_restore.view.checkpoints[1].id, restored.safety);

    let recorded = fx
        .workspaces
        .record_checkpoint(&actor, id, Some("after the restore"))
        .await
        .unwrap();
    let after_record = fx.live.get(id).unwrap();
    assert_eq!(after_record.view.checkpoints.len(), 3);
    assert_eq!(after_record.view.checkpoints[2].id, recorded.id);
    let changes = fx
        .workspaces
        .record_changes(&actor, id, "keep mine")
        .await
        .unwrap();
    // The explicit checkpoint already holds the recorded files on the same base, so the record adds none.
    let after_changes = fx.live.get(id).unwrap();
    assert_eq!(after_changes.view.checkpoints.len(), 3);
    assert_eq!(changes.checkpoint, recorded.id);
}

#[cfg(unix)]
#[tokio::test]
async fn a_mode_change_is_a_modification_without_lines_and_no_activity() {
    use std::os::unix::fs::PermissionsExt;

    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let created = fx.live.get(id).unwrap().view.last_activity_at;
    fx.advance(5);
    fs::set_permissions(dir.join("README.md"), fs::Permissions::from_mode(0o755)).unwrap();
    let state = fx.live.rescan(id).await.unwrap();
    assert_eq!(
        files(&state.view),
        [row("README.md", None, ChangeKind::Modified, 0, 0)]
    );
    assert_eq!(state.view.summary, "1 file: 1 modified (+0 -0)");
    assert_eq!(state.view.last_activity_at, created);
}

#[tokio::test]
async fn a_missing_directory_keeps_the_last_state() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    fs::write(dir.join("kept.txt"), "kept\n").unwrap();
    let before = fx.live.rescan(id).await.unwrap();
    assert_eq!(before.view.files.len(), 1);
    fs::remove_dir_all(&dir).unwrap();
    let WorkspaceError::Fold(error) = fx.live.rescan(id).await.unwrap_err() else {
        panic!("a missing directory is a refusal");
    };
    assert_eq!(error.code(), "workspace_not_active");
    assert_eq!(error.details().unwrap()["state"], "missing");
    assert_eq!(fx.live.get(id).unwrap().view, before.view);
    let reattached = fx.live.attach(id).await;
    assert!(reattached.is_err());
}

/// The daemon state with the fixture's project open as `demo`.
#[cfg(test)]
async fn daemon(fx: &Fixture) -> (AppState, Arc<ProjectHandle>) {
    let clock = Arc::new(fx.project.clock().clone());
    let state: AppState = Arc::new(DaemonState {
        registry: fx.project.registry().clone(),
        projects: Projects::default(),
        clock: clock.clone(),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1:0".parse().unwrap(),
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    });
    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "Demo".to_owned(),
        root: fx.project.paths().root().to_string_lossy().into_owned(),
    };
    let handle = state.projects.open(&row, clock).await.unwrap();
    (state, handle)
}

#[tokio::test]
async fn archiving_a_task_through_the_routes_detaches_its_workspace() {
    let fx = Fixture::with_backend(Silent).await;
    let (state, handle) = daemon(&fx).await;
    let view = WorkspaceService::for_project(&handle)
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap()
        .workspace;
    assert!(handle.live.get(view.id).is_some());
    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri("/api/v1/projects/demo/tasks/T-1/archive")
        .body(Body::empty())
        .unwrap();
    let response = crate::common::signed_in_router(&state, "ana")
        .await
        .oneshot(request)
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    assert!(handle.live.get(view.id).is_none());
    let WorkspaceError::Fold(error) = handle.live.rescan(view.id).await.unwrap_err() else {
        panic!("a detached workspace is a refusal");
    };
    assert_eq!(error.code(), "workspace_not_active");
}

#[tokio::test]
async fn proposal_commands_update_the_proposal_and_checkpoints_of_the_state() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let reviews = ReviewService::new(
        fx.project.store().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
        fx.workspaces.clone(),
        Arc::default(),
    );
    let actor = by(&fx.frontend);
    fs::write(dir.join("README.md"), "# demo\nfirst\n").unwrap();
    let opened = reviews
        .create_proposal(&actor, t(1), "Change the readme", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let proposal = opened.summary.id;
    let state = fx.live.get(id).unwrap();
    assert_eq!(state.view.proposal, Some(proposal));
    let checkpoints = state.view.checkpoints.len();
    assert_eq!(checkpoints, 1, "the before_proposal checkpoint");

    fs::write(dir.join("README.md"), "# demo\nsecond\n").unwrap();
    reviews
        .update_proposal(&actor, proposal, Some("tests pass"))
        .await
        .unwrap()
        .detail()
        .unwrap();
    let state = fx.live.get(id).unwrap();
    assert_eq!(state.view.proposal, Some(proposal));
    assert_eq!(state.view.checkpoints.len(), checkpoints + 1);

    reviews
        .close_proposal(&actor, proposal, "another approach")
        .await
        .unwrap();
    assert_eq!(fx.live.get(id).unwrap().view.proposal, None);
}

#[tokio::test]
async fn the_working_files_count_as_sent_while_they_are_the_newest_version() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let reviews = ReviewService::new(
        fx.project.store().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
        fx.workspaces.clone(),
        Arc::default(),
    );
    let actor = by(&fx.frontend);
    // A proposal command does not wait for the comparison; a rescan queues behind it, so it returns the compared state.
    let edits = async || {
        let live = fx.live.rescan(id).await.unwrap().view.edits_after_version;
        let view = fx.tasks.get_view(t(1)).await.unwrap().edits_after_version;
        let board = fx.tasks.board().await.unwrap();
        let card = board
            .columns
            .iter()
            .flat_map(|column| &column.cards)
            .find(|card| card.id == t(1))
            .unwrap()
            .edits_after_version;
        assert_eq!((view, card), (live, live), "the views read the live state");
        live
    };
    fs::write(dir.join("README.md"), "# demo\nfirst\n").unwrap();
    fx.live.rescan(id).await.unwrap();
    assert_eq!(edits().await, None, "no proposal yet");

    let proposal = reviews
        .create_proposal(&actor, t(1), "Change the readme", None)
        .await
        .unwrap()
        .detail()
        .unwrap()
        .summary
        .id;
    assert_eq!(edits().await, Some(false));
    fx.workspaces
        .record_checkpoint(&actor, id, Some("same files"))
        .await
        .unwrap();
    fx.live.rescan(id).await.unwrap();
    assert_eq!(edits().await, Some(false), "a checkpoint of the same files");

    fs::write(dir.join("README.md"), "# demo\nsecond\n").unwrap();
    fx.live.rescan(id).await.unwrap();
    assert_eq!(edits().await, Some(true), "the first file that differs");
    fs::write(dir.join("README.md"), "# demo\nfirst\n").unwrap();
    fx.live.rescan(id).await.unwrap();
    assert_eq!(edits().await, Some(false), "the content, not the time");

    fs::write(dir.join("README.md"), "# demo\nsecond\n").unwrap();
    reviews
        .update_proposal(&actor, proposal, Some("tests pass"))
        .await
        .unwrap();
    assert_eq!(edits().await, Some(false), "the new version holds the edit");

    fx.live.detach(id).await;
    assert_eq!(
        fx.live.attach(id).await.unwrap().view.edits_after_version,
        Some(false),
        "an attach compares against the open proposal again"
    );
    assert_eq!(fx.live.get(id).unwrap().view.proposal, Some(proposal));

    fs::write(dir.join("README.md"), "# demo\nthird\n").unwrap();
    fx.live.rescan(id).await.unwrap();
    let mut bus = fx.bus.subscribe();
    reviews
        .close_proposal(&actor, proposal, "another approach")
        .await
        .unwrap();
    assert_eq!(edits().await, None);
    let frames = drain_live(&mut bus, id);
    assert!(
        frames
            .iter()
            .any(|frame| frame.proposal.is_none() && frame.edits_after_version.is_none()),
        "{frames:?}"
    );
    assert!(
        frames
            .iter()
            .all(|frame| frame.proposal.is_some() || frame.edits_after_version.is_none()),
        "no frame keeps the comparison of a closed proposal: {frames:?}"
    );
}

#[tokio::test]
async fn a_version_tree_that_cannot_be_read_counts_as_edited() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let reviews = ReviewService::new(
        fx.project.store().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
        fx.workspaces.clone(),
        Arc::default(),
    );
    fs::write(dir.join("README.md"), "# demo\nsent\n").unwrap();
    fx.live.rescan(id).await.unwrap();
    let proposal = reviews
        .create_proposal(&by(&fx.frontend), t(1), "Change the readme", None)
        .await
        .unwrap()
        .detail()
        .unwrap()
        .summary
        .id;
    assert_eq!(
        fx.live.rescan(id).await.unwrap().view.edits_after_version,
        Some(false)
    );
    let tree = fold_store::reviews::list_versions(fx.project.store().read(), proposal)
        .await
        .unwrap()
        .pop()
        .unwrap()
        .tree;
    fx.live.detach(id).await;
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    assert!(objects.remove(&tree).unwrap());
    assert_eq!(
        fx.live.attach(id).await.unwrap().view.edits_after_version,
        Some(true),
        "a version it cannot read never shows the holder as sent"
    );
}

#[tokio::test]
async fn a_reject_clears_the_proposal_of_the_state() {
    let fx = Fixture::with_backend(Silent).await;
    let (id, dir) = fx.create(&fx.frontend, 1).await;
    let reviews = ReviewService::new(
        fx.project.store().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
        fx.workspaces.clone(),
        Arc::default(),
    );
    fs::write(dir.join("README.md"), "# demo\nfirst\n").unwrap();
    let opened = reviews
        .create_proposal(&by(&fx.frontend), t(1), "Change the readme", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let proposal = opened.summary.id;
    assert_eq!(fx.live.get(id).unwrap().view.proposal, Some(proposal));
    let human = Actor::human(HumanName::parse("tanim").unwrap());
    reviews
        .submit_review(
            &human,
            proposal,
            1,
            Verdict::Reject,
            "wrong approach",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(fx.live.get(id).unwrap().view.proposal, None);
}

#[tokio::test]
async fn the_live_job_ticks_every_open_project() {
    let fx = Fixture::new().await;
    let clock = fx.project.clock().clone();
    let (state, handle) = daemon(&fx).await;
    let (stop, stopped) = watch::channel(false);
    let job = spawn_live(state, Duration::from_millis(5), stopped);
    let mut progress = job.progress();
    let wait = async |progress: &mut watch::Receiver<JobProgress>,
                      done: &dyn Fn(&JobProgress) -> bool| {
        tokio::time::timeout(WAIT, progress.wait_for(|seen| done(seen)))
            .await
            .unwrap()
            .map(|seen| *seen)
            .unwrap()
    };
    let idle = wait(&mut progress, &|seen| seen.checks >= 3).await;
    assert_eq!(idle.cycles, 0, "no workspace is attached yet");

    let workspaces = WorkspaceService::for_project(&handle);
    let view = workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap()
        .workspace;
    wait(&mut progress, &|seen| seen.cycles >= 1).await;
    assert_eq!(handle.live.all().len(), 1);
    let dir = fx.project.paths().root().join(&view.path);
    fs::write(dir.join("ticked.txt"), "found by a rescan\n").unwrap();
    clock.advance(Duration::from_secs(60)).unwrap();
    eventually(WAIT, async || match handle.live.get(view.id) {
        Some(state) if !state.view.files.is_empty() => Ok(()),
        other => Err(other.map(|state| files(&state.view))),
    })
    .await;
    stop.send(true).unwrap();
    job.stopped(WAIT).await;

    // Garbage collection hands the live state the checkpoints it kept.
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    let tree = write_initial_tree(&objects, FlatTree::new()).unwrap();
    let now = fx.project.now();
    let kept = usize::try_from(ProjectConfig::default().auto_checkpoints_kept).unwrap();
    handle
        .store
        .write(async |tx| {
            for _ in 0..=kept {
                workspaces::insert_checkpoint(
                    tx,
                    view.id,
                    tree,
                    CheckpointKind::Auto,
                    CheckpointReason::Quiet,
                    None,
                    now,
                )
                .await?;
            }
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    assert!(
        handle
            .live
            .get(view.id)
            .unwrap()
            .view
            .checkpoints
            .is_empty()
    );
    let report = gc(&handle, false).await.unwrap();
    assert_eq!(report.checkpoints_pruned, 1);
    assert_eq!(
        handle.live.get(view.id).unwrap().view.checkpoints.len(),
        kept
    );
}

/// The worker named in the newest live event for `workspace` on the bus right now.
#[cfg(test)]
fn published_worker(receiver: &mut Receiver<BusMessage>, workspace: WorkspaceId) -> String {
    drain_live(receiver, workspace)
        .pop()
        .expect("a live event for the workspace")
        .worker
}

#[tokio::test]
async fn an_adopted_workspace_names_its_new_owner_in_the_live_state() {
    let fx = Fixture::new().await;
    let (first, _) = fx.create(&fx.frontend, 1).await;
    fx.wait_for(first, |state| state.view.worker == "frontend")
        .await;
    let mut bus = fx.bus.subscribe();
    fx.advance(LEASE_MINUTES * 60);
    let taken = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert!(taken.adopted);
    assert_eq!(fx.live.get(first).unwrap().view.worker, "backend");
    assert_eq!(published_worker(&mut bus, first), "backend");

    let (second, _) = fx.create(&fx.frontend, 2).await;
    fx.wait_for(second, |state| state.view.worker == "frontend")
        .await;
    fx.tasks
        .release_task(&by(&fx.frontend), t(2))
        .await
        .unwrap();
    drain_live(&mut bus, second);
    let claimed = fx.tasks.claim_task(&by(&fx.backend), t(2)).await.unwrap();
    assert!(claimed.adopted);
    assert_eq!(fx.live.get(second).unwrap().view.worker, "backend");
    assert_eq!(published_worker(&mut bus, second), "backend");

    fx.tasks.release_task(&by(&fx.backend), t(2)).await.unwrap();
    let created = fx
        .workspaces
        .create_workspace(&by(&fx.frontend), t(2))
        .await
        .unwrap();
    assert!(created.adopted);
    assert_eq!(created.workspace.id, second);
    assert_eq!(fx.live.get(second).unwrap().view.worker, "frontend");
    assert_eq!(published_worker(&mut bus, second), "frontend");
    assert_eq!(fx.live.all().len(), 2, "an adoption keeps the pipeline");
}

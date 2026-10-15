use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use fold_core::object::ObjectKind;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentName, ChangesetId, CheckpointId, EventFilter, RevisionId, TaskDescription, TaskId,
    TaskTitle, Timestamp, WorkspaceId,
};
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::projects::gc;
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceService};
use fold_store::Store;
use fold_store::events;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    self, CheckpointReason, advance_main, create_initial_revision, initial_revision,
    insert_revision,
};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};

const BASE: &[u8] = b"one\ntwo\nthree\n";
const MINE: &[u8] = b"one\nmine\nthree\n";
const THEIRS: &[u8] = b"one\ntheirs\nthree\n";
const STALL: Duration = Duration::from_millis(300);
const WAIT: Duration = Duration::from_secs(10);

#[cfg(test)]
fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

/// Services over `store`, the way the daemon builds them for one project.
#[cfg(test)]
struct Project {
    store: Store,
    root: std::path::PathBuf,
    agents: AgentService,
    tasks: TaskService,
    workspaces: WorkspaceService,
}

#[cfg(test)]
impl Project {
    fn over(project: &TestProject, store: &Store, workspaces: Option<WorkspaceService>) -> Self {
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let workspaces = workspaces.unwrap_or_else(|| {
            WorkspaceService::new(
                store.clone(),
                bus.clone(),
                clock.clone(),
                project.paths().clone(),
                Git::default(),
                5,
            )
        });
        Self {
            store: store.clone(),
            root: project.paths().root().to_owned(),
            agents: AgentService::new(store.clone(), bus.clone(), clock.clone()),
            tasks: TaskService::new(store.clone(), bus, clock, 5),
            workspaces,
        }
    }

    /// `r1` holds `lib.txt`; task `T-1`; the agent `frontend` owns `ws-1` for it, where `lib.txt` holds `MINE`.
    async fn seed(&self) -> Actor {
        let objects = ObjectStore::open(&self.root.join(".fold")).unwrap();
        let tree = write_initial_tree(&objects, flat(&objects, BASE)).unwrap();
        let revision = initial_revision(tree, Timestamp::from_millis(1).unwrap()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(&self.store, &stored, None)
            .await
            .unwrap();
        let mut session = self.agents.start_session(1).await.unwrap();
        self.agents
            .register_agent(&mut session, &AgentName::parse("frontend").unwrap(), &[])
            .await
            .unwrap();
        let actor = self.agents.actor(&session, None).await.unwrap();
        self.tasks
            .create_task(
                &actor,
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new("body"),
                    labels: Vec::new(),
                },
            )
            .await
            .unwrap();
        self.workspaces
            .create_workspace(&actor, TaskId::from_number(1).unwrap())
            .await
            .unwrap();
        fs::write(self.lib(), MINE).unwrap();
        actor
    }

    fn lib(&self) -> std::path::PathBuf {
        self.root.join("ws/t-1/lib.txt")
    }

    /// Moves `main` to a new revision where `lib.txt` holds `lib`.
    async fn move_main(&self, lib: &[u8]) {
        let objects = ObjectStore::open(&self.root.join(".fold")).unwrap();
        let head = workspaces::head(self.store.read()).await.unwrap().unwrap();
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, flat(&objects, lib)).unwrap(),
            parent: Some(head.hash),
            number,
            integration: None,
            created_at: Timestamp::from_millis(2).unwrap(),
        };
        let stored = write_revision(&objects, &revision).unwrap();
        self.store
            .write(async |tx| {
                insert_revision(tx, &stored).await?;
                advance_main(tx, head.number, number).await
            })
            .await
            .unwrap();
    }

    async fn event_seq(&self, event_type: &str) -> Vec<u64> {
        events::read_after(self.store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .iter()
            .filter(|event| event.event_type.as_str() == event_type)
            .map(|event| event.seq)
            .collect()
    }
}

#[cfg(test)]
fn flat(objects: &ObjectStore, lib: &[u8]) -> FlatTree {
    let mut tree = FlatTree::new();
    tree.insert(
        b"lib.txt".to_vec(),
        FlatEntry {
            kind: EntryKind::File,
            hash: objects.write(ObjectKind::Blob, lib).unwrap(),
        },
    );
    tree
}

/// Every file below `dir` outside `.fold`, with its bytes.
#[cfg(test)]
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".fold" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            for (path, bytes) in files(&entry.path()) {
                found.insert(format!("{name}/{path}"), bytes);
            }
        } else {
            found.insert(name, fs::read(entry.path()).unwrap());
        }
    }
    found
}

/// A failpoint that stops the thread that reaches it until `open` is called, and tells the test when a thread is there.
#[cfg(test)]
#[derive(Clone)]
struct Gate {
    reached: Arc<AtomicBool>,
    open: Arc<AtomicBool>,
}

#[cfg(test)]
impl Gate {
    fn install(name: &str) -> Self {
        let gate = Self {
            reached: Arc::new(AtomicBool::new(false)),
            open: Arc::new(AtomicBool::new(false)),
        };
        let held = gate.clone();
        fail::cfg_callback(name, move || {
            held.reached.store(true, Ordering::SeqCst);
            while !held.open.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .unwrap();
        gate
    }

    async fn wait_reached(&self) {
        tokio::time::timeout(WAIT, async {
            while !self.reached.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

/// The failpoint stops a sync after it wrote the merged files and before its rows commit, as a crash there would. After a restart the next sync restores the checkpoint taken before the stopped one and merges again, so the files end as an uninterrupted sync leaves them, with no markers inside markers.
#[tokio::test]
async fn a_sync_stopped_after_its_writes_ends_like_an_uninterrupted_one() {
    let scenario = fail::FailScenario::setup();
    let clean = TestProject::new().await;
    let fixture = Project::over(&clean, clean.store(), None);
    let actor = fixture.seed().await;
    fixture.move_main(THEIRS).await;
    let expected = fixture
        .workspaces
        .sync_workspace(&actor, ws(1))
        .await
        .unwrap();
    let expected_files = files(&fixture.root.join("ws/t-1"));

    let mut stopped = TestProject::new().await;
    let fixture = Project::over(&stopped, stopped.store(), None);
    let actor = fixture.seed().await;
    fixture.move_main(THEIRS).await;
    fail::cfg("workspace_sync_after_write", "return").unwrap();
    let error = fixture
        .workspaces
        .sync_workspace(&actor, ws(1))
        .await
        .unwrap_err();
    fail::remove("workspace_sync_after_write");
    match &error {
        WorkspaceError::Fold(fold) => assert_eq!(fold.code(), "failpoint"),
        other => panic!("expected the failpoint, got {other:?}"),
    }
    let half = fs::read_to_string(fixture.lib()).unwrap();
    assert!(half.contains("<<<<<<< workspace"), "{half}");

    stopped.reopen().await;
    let fixture = Project::over(&stopped, stopped.store(), None);
    assert_eq!(
        workspaces::sync_checkpoint(fixture.store.read(), ws(1))
            .await
            .unwrap(),
        Some(CheckpointId::from_number(1).unwrap())
    );
    let refused = fixture
        .workspaces
        .record_changes(&actor, ws(1), "half merged")
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("workspace_write_unfinished"));
    let retried = fixture
        .workspaces
        .sync_workspace(&actor, ws(1))
        .await
        .unwrap();
    assert_eq!(retried.base, expected.base);
    assert_eq!(retried.conflicted, expected.conflicted);
    assert_eq!(files(&fixture.root.join("ws/t-1")), expected_files);
    let merged = fs::read_to_string(fixture.lib()).unwrap();
    assert_eq!(merged.matches("<<<<<<<").count(), 1, "{merged}");
    assert_eq!(fixture.event_seq("checkpoint.restored").await.len(), 1);
    assert_eq!(
        workspaces::sync_checkpoint(fixture.store.read(), ws(1))
            .await
            .unwrap(),
        None
    );
    scenario.teardown();
}

/// The failpoint stops a restore after it wrote the files and before the base and `checkpoint.restored` commit, as a crash there would. After a restart the rows show none of the restore and its safety checkpoint marks the unfinished write, so `record_changes` refuses the half-written files; running the restore again finishes it, and the safety checkpoint still brings back the files from before.
#[tokio::test]
async fn a_restore_stopped_after_its_writes_finishes_when_it_runs_again() {
    let scenario = fail::FailScenario::setup();
    let mut project = TestProject::new().await;
    let fixture = Project::over(&project, project.store(), None);
    let actor = fixture.seed().await;
    let first = fixture
        .workspaces
        .record_checkpoint(&actor, ws(1), None)
        .await
        .unwrap();
    fixture.move_main(THEIRS).await;
    fixture
        .workspaces
        .sync_workspace(&actor, ws(1))
        .await
        .unwrap();
    let synced_files = files(&fixture.root.join("ws/t-1"));
    let r1 = RevisionId::from_number(1).unwrap();
    let r2 = RevisionId::from_number(2).unwrap();

    fail::cfg("workspace_restore_after_write", "return").unwrap();
    let error = fixture
        .workspaces
        .restore_checkpoint(&actor, first.id)
        .await
        .unwrap_err();
    fail::remove("workspace_restore_after_write");
    assert_eq!(error.code(), Some("failpoint"));
    assert_eq!(fs::read(fixture.lib()).unwrap(), MINE);

    project.reopen().await;
    let fixture = Project::over(&project, project.store(), None);
    let row = workspaces::get_workspace(fixture.store.read(), ws(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.base, r2);
    assert!(fixture.event_seq("checkpoint.restored").await.is_empty());
    let safety = workspaces::sync_checkpoint(fixture.store.read(), ws(1))
        .await
        .unwrap()
        .unwrap();
    let marked = workspaces::get_checkpoint(fixture.store.read(), safety)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marked.reason, CheckpointReason::BeforeRestore);
    let refused = fixture
        .workspaces
        .record_changes(&actor, ws(1), "half restored")
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("workspace_write_unfinished"));

    let restored = fixture
        .workspaces
        .restore_checkpoint(&actor, first.id)
        .await
        .unwrap();
    assert_eq!(restored.base, r1);
    assert_eq!(fs::read(fixture.lib()).unwrap(), MINE);
    assert_eq!(
        workspaces::sync_checkpoint(fixture.store.read(), ws(1))
            .await
            .unwrap(),
        None
    );
    assert_eq!(fixture.event_seq("checkpoint.restored").await.len(), 1);

    let undone = fixture
        .workspaces
        .restore_checkpoint(&actor, safety)
        .await
        .unwrap();
    assert_eq!(undone.base, r2);
    assert_eq!(files(&fixture.root.join("ws/t-1")), synced_files);
    scenario.teardown();
}

/// An edit that lands after a sync's checkpoint and before its merge refuses the sync before anything is written. The refusal clears the mark, so the next sync merges the edited files without restoring the checkpoint first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sync_refused_for_an_edit_clears_its_mark() {
    let scenario = fail::FailScenario::setup();
    let project = TestProject::new().await;
    let fixture = Arc::new(Project::over(&project, project.store(), None));
    let actor = fixture.seed().await;
    fixture.move_main(THEIRS).await;
    let edited: &[u8] = b"one\nedited here\nthree\n";

    let gate = Gate::install("workspace_sync_before_merge");
    let sync = tokio::spawn({
        let (fixture, actor) = (Arc::clone(&fixture), actor.clone());
        async move { fixture.workspaces.sync_workspace(&actor, ws(1)).await }
    });
    gate.wait_reached().await;
    fs::write(fixture.lib(), edited).unwrap();
    gate.open();
    let error = tokio::time::timeout(WAIT, sync)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    fail::remove("workspace_sync_before_merge");
    assert_eq!(error.code(), Some("workspace_changed_during_sync"));
    assert_eq!(fs::read(fixture.lib()).unwrap(), edited);
    assert_eq!(
        workspaces::sync_checkpoint(fixture.store.read(), ws(1))
            .await
            .unwrap(),
        None
    );

    let synced = fixture
        .workspaces
        .sync_workspace(&actor, ws(1))
        .await
        .unwrap();
    assert_eq!(synced.base, RevisionId::from_number(2).unwrap());
    assert_eq!(synced.conflicted, vec!["lib.txt".to_owned()]);
    let merged = fs::read_to_string(fixture.lib()).unwrap();
    assert!(merged.contains("edited here"), "{merged}");
    assert_eq!(merged.matches("<<<<<<<").count(), 1, "{merged}");
    assert!(fixture.event_seq("checkpoint.restored").await.is_empty());
    scenario.teardown();
}

/// A record that arrives while a restore is between its safety checkpoint and its writes waits for the restore, so it records the restored files and its events come after the restore's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_and_a_record_on_one_workspace_never_interleave() {
    let scenario = fail::FailScenario::setup();
    let project = TestProject::new().await;
    let fixture = Arc::new(Project::over(&project, project.store(), None));
    let actor = fixture.seed().await;
    let first = fixture
        .workspaces
        .record_checkpoint(&actor, ws(1), Some("mine"))
        .await
        .unwrap();
    fs::write(fixture.lib(), b"later\n").unwrap();

    let gate = Gate::install("workspace_restore_before_write");
    let restore = tokio::spawn({
        let (fixture, actor) = (Arc::clone(&fixture), actor.clone());
        async move {
            fixture
                .workspaces
                .restore_checkpoint(&actor, first.id)
                .await
        }
    });
    gate.wait_reached().await;
    let mut record = tokio::spawn({
        let (fixture, actor) = (Arc::clone(&fixture), actor.clone());
        async move {
            fixture
                .workspaces
                .record_changes(&actor, ws(1), "after the restore")
                .await
        }
    });
    let waited = tokio::time::timeout(STALL, &mut record).await.is_err();
    gate.open();
    assert!(waited, "the record ran while the restore was writing");
    let restored = restore.await.unwrap().unwrap();
    let recorded = tokio::time::timeout(WAIT, record)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fail::remove("workspace_restore_before_write");

    assert_eq!(restored.restored, first.id);
    let changeset =
        workspaces::get_changeset(fixture.store.read(), ChangesetId::from_number(1).unwrap())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(recorded.changeset, changeset.id);
    let checkpoint = workspaces::get_checkpoint(fixture.store.read(), first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changeset.tree, checkpoint.tree);
    let restored_at = fixture.event_seq("checkpoint.restored").await;
    let recorded_at = fixture.event_seq("changeset.recorded").await;
    assert!(restored_at[0] < recorded_at[0]);
    scenario.teardown();
}

/// Garbage collection started while a record holds objects it has written but not yet named waits until the record commits, so it never sweeps them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garbage_collection_waits_for_a_record_to_commit() {
    let scenario = fail::FailScenario::setup();
    let project = TestProject::new().await;
    project
        .clock()
        .set(Timestamp::from_system_time(SystemTime::now()).unwrap());
    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "demo".to_owned(),
        root: project.paths().root().to_string_lossy().into_owned(),
    };
    let handle = Arc::new(
        ProjectHandle::open(&row, Arc::new(project.clock().clone()))
            .await
            .unwrap(),
    );
    let service = WorkspaceService::for_project(&handle, Git::default())
        .await
        .unwrap();
    let fixture = Arc::new(Project::over(&project, &handle.store, Some(service)));
    let actor = fixture.seed().await;
    fs::write(fixture.lib(), b"a file no object holds yet\n").unwrap();

    let gate = Gate::install("workspace_record_after_snapshot");
    let record = tokio::spawn({
        let (fixture, actor) = (Arc::clone(&fixture), actor.clone());
        async move {
            fixture
                .workspaces
                .record_changes(&actor, ws(1), "a new file")
                .await
        }
    });
    gate.wait_reached().await;
    let mut collect = tokio::spawn({
        let handle = Arc::clone(&handle);
        async move { gc(&handle, false).await }
    });
    let waited = tokio::time::timeout(STALL, &mut collect).await.is_err();
    gate.open();
    assert!(
        waited,
        "garbage collection swept while a record held unnamed objects"
    );
    let recorded = record.await.unwrap().unwrap();
    tokio::time::timeout(WAIT, collect)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fail::remove("workspace_record_after_snapshot");

    let changeset = workspaces::get_changeset(fixture.store.read(), recorded.changeset)
        .await
        .unwrap()
        .unwrap();
    let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
    assert!(objects.contains(&changeset.tree).unwrap());
    scenario.teardown();
}

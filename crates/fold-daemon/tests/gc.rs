#![cfg(test)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::events::types;
use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::tree::{EntryKind, Tree, TreeEntry};
use fold_core::{EventFilter, ManualClock, RevisionId, TaskId, Timestamp, WorkspaceId};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::{router, socket_router};
use fold_daemon::jobs::{JobProgress, spawn_gc_daily};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::projects::{GcReport, gc};
use fold_store::events::read_after;
use fold_store::projects::ProjectRow;
use fold_store::testing::{TestProject, eventually, execute};
use fold_store::workspaces::{
    ChangesetStats, CheckpointReason, create_initial_revision, gc_roots, initial_revision,
    insert_changeset, insert_checkpoint, list_changesets, list_checkpoints, prune_auto,
};
use fold_store::{Store, StoreError};
use fold_vcs::gc::{GRACE, mark, sweep};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_revision;
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::watch;
use tower::ServiceExt;

const OLD: Duration = Duration::from_secs(2 * 60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const WAIT: Duration = Duration::from_secs(10);

/// A project whose clock reads the real time, so object modification times and the run's `now` share one clock domain.
struct Fixture {
    project: TestProject,
    objects: ObjectStore,
    handle: Arc<ProjectHandle>,
}

impl Fixture {
    async fn new(slug: &str) -> Self {
        let project = TestProject::new().await;
        project
            .clock()
            .set(Timestamp::from_system_time(SystemTime::now()).unwrap());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let handle = ProjectHandle::open(&row(slug, &project), Arc::new(project.clock().clone()))
            .await
            .unwrap();
        let fixture = Self {
            project,
            objects,
            handle: Arc::new(handle),
        };
        fixture.seed().await;
        fixture
    }

    fn store(&self) -> &Store {
        self.project.store()
    }

    fn root(&self) -> &Path {
        self.project.paths().root()
    }

    /// Agent, worker, task `T-1` and revision `r1` over a tree holding the file `base`.
    async fn seed(&self) {
        execute(
            self.store(),
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
             INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
             INSERT INTO tasks (number, title, created_by, created_at, updated_at) VALUES (1, 'task', 'frontend', 0, 0);",
        )
        .await;
        let base = self.tree("base");
        let r1 = initial_revision(base, self.project.now()).unwrap();
        let stored = write_revision(&self.objects, &r1).unwrap();
        create_initial_revision(self.store(), &stored, None)
            .await
            .unwrap();
    }

    /// A tree holding one file whose content is `label`.
    fn tree(&self, label: &str) -> ObjectHash {
        let blob = self
            .objects
            .write(ObjectKind::Blob, label.as_bytes())
            .unwrap();
        let tree = Tree {
            entries: vec![TreeEntry {
                name: b"file".to_vec(),
                kind: EntryKind::File,
                hash: blob,
            }],
        };
        self.objects
            .write(ObjectKind::Tree, &tree.encode().unwrap())
            .unwrap()
    }

    /// Workspace `ws-n` of `T-1` with its directory. `closed` is the days since it closed, for a closed state.
    async fn workspace(&self, n: u32, kind: &str, state: &str, closed: Option<u32>, dir: &str) {
        let closed_at = closed.map_or("NULL".to_owned(), |days| {
            self.project
                .now()
                .checked_sub(DAY * days)
                .unwrap()
                .as_millis()
                .to_string()
        });
        execute(
            self.store(),
            &format!(
                "INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at, closed_at)
                 VALUES ({n}, 1, '{kind}', 'ws/{dir}', 1, '{state}', 0, {closed_at});"
            ),
        )
        .await;
        let path = self.root().join("ws").join(dir);
        fs::create_dir_all(path.join("src")).unwrap();
        fs::write(path.join("src").join("lib.rs"), b"secret content").unwrap();
        fs::set_permissions(
            path.join("src").join("lib.rs"),
            fs::Permissions::from_mode(0o444),
        )
        .unwrap();
    }

    async fn checkpoint(&self, n: u32, label: &str, reason: CheckpointReason) -> ObjectHash {
        let tree = self.tree(label);
        let now = self.project.now();
        self.store()
            .write(async |tx| {
                insert_checkpoint(tx, ws(n), tree, reason.kind(), reason, None, now).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        tree
    }

    fn age_all(&self) {
        for entry in self.objects.entries().unwrap() {
            age(&self.objects.path(&entry.hash), OLD);
        }
    }

    async fn unpruned(&self, n: u32) -> Vec<u32> {
        list_checkpoints(self.store().read(), ws(n), false)
            .await
            .unwrap()
            .iter()
            .map(|checkpoint| checkpoint.id.number())
            .collect()
    }

    async fn pruned_events(&self) -> Vec<(u32, String)> {
        let filter = EventFilter {
            types: vec![types::CHECKPOINT_PRUNED],
            ..EventFilter::default()
        };
        read_after(self.store().read(), 0, &filter, 1000)
            .await
            .unwrap()
            .into_iter()
            .map(|event| {
                assert_eq!(event.actor.as_str(), "fold");
                (
                    event.workspace.unwrap().number(),
                    event.data["checkpoint"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    /// Every object with its modification time, every scratch file and every checkpoint row.
    async fn state(
        &self,
    ) -> (
        BTreeMap<ObjectHash, SystemTime>,
        Vec<String>,
        Vec<(i64, Option<i64>)>,
        i64,
    ) {
        let objects = self
            .objects
            .entries()
            .unwrap()
            .into_iter()
            .map(|entry| (entry.hash, entry.modified))
            .collect();
        let mut scratch: Vec<String> = fs::read_dir(self.objects.scratch_dir())
            .unwrap()
            .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        scratch.sort();
        let checkpoints =
            sqlx::query_as("SELECT number, pruned_at FROM checkpoints ORDER BY number")
                .fetch_all(self.store().read())
                .await
                .unwrap();
        let events = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(self.store().read())
            .await
            .unwrap();
        (objects, scratch, checkpoints, events)
    }
}

fn row(slug: &str, project: &TestProject) -> ProjectRow {
    ProjectRow {
        slug: slug.to_owned(),
        name: slug.to_owned(),
        root: project.paths().root().to_string_lossy().into_owned(),
    }
}

fn ws(n: u32) -> WorkspaceId {
    WorkspaceId::from_number(n).unwrap()
}

fn age(path: &Path, by: Duration) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - by)
        .unwrap();
}

fn counts(report: &GcReport) -> [u64; 6] {
    [
        report.deleted,
        report.bytes_freed,
        report.tmp_removed,
        report.workspaces_removed,
        report.checkpoints_pruned,
        report.examined,
    ]
}

#[tokio::test]
async fn automatic_checkpoints_beyond_twenty_are_pruned_before_marking_and_a_dry_run_matches() {
    let fixture = Fixture::new("demo").await;
    fixture.workspace(1, "task", "active", None, "t-1").await;
    let mut auto = Vec::new();
    for n in 0..23 {
        auto.push(
            fixture
                .checkpoint(1, &format!("auto {n}"), CheckpointReason::Quiet)
                .await,
        );
    }
    let explicit = fixture
        .checkpoint(1, "explicit", CheckpointReason::Explicit)
        .await;
    let orphan = fixture.objects.write(ObjectKind::Blob, b"orphan").unwrap();
    let scratch = fixture.objects.scratch_dir().join("stale");
    fs::write(&scratch, b"x").unwrap();
    fixture.age_all();
    age(&scratch, OLD);

    let before = fixture.state().await;
    let planned = gc(&fixture.handle, true).await.unwrap();
    assert_eq!(fixture.state().await, before);
    assert!(fixture.root().join("ws").join("t-1").is_dir());

    let report = gc(&fixture.handle, false).await.unwrap();
    assert_eq!(counts(&planned), counts(&report));
    assert_eq!(report.checkpoints_pruned, 3);
    assert_eq!(report.deleted, 7);
    assert_eq!(report.tmp_removed, 1);
    assert_eq!(report.workspaces_removed, 0);
    assert!(report.missing.is_empty());
    for tree in &auto[..3] {
        assert!(!fixture.objects.contains(tree).unwrap());
    }
    for tree in auto[3..].iter().chain([&explicit]) {
        assert!(fixture.objects.contains(tree).unwrap());
    }
    assert!(!fixture.objects.contains(&orphan).unwrap() && !scratch.exists());
    assert_eq!(fixture.unpruned(1).await, (4..=24).collect::<Vec<_>>());
    let events = fixture.pruned_events().await;
    assert_eq!(
        events,
        ["ck-1", "ck-2", "ck-3"]
            .map(|id| (1, id.to_owned()))
            .to_vec()
    );

    let again = gc(&fixture.handle, false).await.unwrap();
    assert_eq!((again.deleted, again.checkpoints_pruned), (0, 0));
}

#[tokio::test]
async fn a_second_run_while_one_holds_the_project_is_refused() {
    let fixture = Fixture::new("demo").await;
    let running = fixture.handle.gc.try_lock().unwrap();
    for dry_run in [true, false] {
        let error = gc(&fixture.handle, dry_run).await.unwrap_err();
        assert_eq!(error.code(), Some("gc_in_progress"));
    }
    drop(running);
    gc(&fixture.handle, false).await.unwrap();
}

#[tokio::test]
async fn the_sweep_waits_while_a_writer_holds_the_object_lock() {
    let fixture = Fixture::new("demo").await;
    let orphan = fixture.objects.write(ObjectKind::Blob, b"orphan").unwrap();
    fixture.age_all();
    let writer = fixture.handle.objects.read().await;

    let planned = gc(&fixture.handle, true).await.unwrap();
    assert_eq!(planned.deleted, 1);
    let handle = fixture.handle.clone();
    let mut run = tokio::spawn(async move { gc(&handle, false).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut run)
            .await
            .is_err()
    );
    assert!(fixture.handle.gc.try_lock().is_err());
    assert!(fixture.objects.contains(&orphan).unwrap());

    drop(writer);
    let report = tokio::time::timeout(WAIT, run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(report.deleted, 1);
    assert!(!fixture.objects.contains(&orphan).unwrap());
}

#[tokio::test]
async fn closed_workspaces_past_their_retention_lose_directories_and_checkpoints() {
    let fixture = Fixture::new("demo").await;
    fixture
        .workspace(1, "task", "archived", Some(15), "t-1")
        .await;
    fixture
        .workspace(2, "review", "archived", Some(15), "review-p-1")
        .await;
    fixture
        .workspace(3, "task", "abandoned", Some(13), "t-1-2")
        .await;
    fixture.workspace(4, "task", "active", None, "t-1-3").await;
    let gone = [
        fixture
            .checkpoint(1, "one auto", CheckpointReason::BeforeRecord)
            .await,
        fixture
            .checkpoint(1, "one explicit", CheckpointReason::Explicit)
            .await,
        fixture
            .checkpoint(2, "two explicit", CheckpointReason::Explicit)
            .await,
    ];
    let kept = [
        fixture
            .checkpoint(3, "three", CheckpointReason::Explicit)
            .await,
        fixture
            .checkpoint(4, "four", CheckpointReason::Explicit)
            .await,
    ];
    let recorded = fixture.tree("changeset of ws-1");
    let now = fixture.project.now();
    fixture
        .store()
        .write(async |tx| {
            insert_changeset(
                tx,
                ws(1),
                TaskId::from_number(1).unwrap(),
                RevisionId::from_number(1).unwrap(),
                recorded,
                "record",
                ChangesetStats::default(),
                "frontend",
                now,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    fixture.age_all();

    let before = fixture.state().await;
    let planned = gc(&fixture.handle, true).await.unwrap();
    assert_eq!(fixture.state().await, before);
    for name in ["t-1", "review-p-1", "t-1-2", "t-1-3"] {
        let file = fixture
            .root()
            .join("ws")
            .join(name)
            .join("src")
            .join("lib.rs");
        assert!(file.is_file(), "{name}");
    }
    let report = gc(&fixture.handle, false).await.unwrap();
    assert_eq!(counts(&planned), counts(&report));
    assert_eq!(report.workspaces_removed, 2);
    assert_eq!(report.checkpoints_pruned, 3);
    assert_eq!(report.deleted, 6);

    let dir = |name: &str| fixture.root().join("ws").join(name);
    assert!(!dir("t-1").exists() && !dir("review-p-1").exists());
    assert!(dir("t-1-2").join("src").join("lib.rs").is_file());
    assert!(dir("t-1-3").join("src").join("lib.rs").is_file());
    assert!(fixture.unpruned(1).await.is_empty() && fixture.unpruned(2).await.is_empty());
    assert_eq!(fixture.unpruned(3).await, vec![4]);
    assert_eq!(fixture.unpruned(4).await, vec![5]);
    for tree in gone {
        assert!(!fixture.objects.contains(&tree).unwrap());
    }
    for tree in kept.iter().chain([&recorded]) {
        assert!(fixture.objects.contains(tree).unwrap());
    }
    let changesets = list_changesets(fixture.store().read(), ws(1))
        .await
        .unwrap();
    assert_eq!(changesets.len(), 1);
    assert_eq!(
        fixture.pruned_events().await,
        vec![
            (1, "ck-1".to_owned()),
            (1, "ck-2".to_owned()),
            (2, "ck-3".to_owned())
        ]
    );

    let again = gc(&fixture.handle, false).await.unwrap();
    assert_eq!((again.workspaces_removed, again.checkpoints_pruned), (0, 0));
}

#[tokio::test]
async fn a_closed_workspace_whose_directory_escapes_ws_is_left_alone() {
    let fixture = Fixture::new("demo").await;
    fixture
        .workspace(1, "task", "archived", Some(30), "t-1")
        .await;
    fixture
        .checkpoint(1, "checkpoint", CheckpointReason::Explicit)
        .await;
    execute(
        fixture.store(),
        "UPDATE workspaces SET path = 'ws/../outside' WHERE number = 1;",
    )
    .await;
    for dry_run in [true, false] {
        let report = gc(&fixture.handle, dry_run).await.unwrap();
        assert_eq!(
            (report.workspaces_removed, report.checkpoints_pruned),
            (0, 0)
        );
    }
    assert_eq!(fixture.unpruned(1).await, vec![1]);
    assert!(fixture.root().join("ws").join("t-1").is_dir());
}

#[tokio::test]
async fn a_closed_workspace_already_gone_counts_once() {
    let fixture = Fixture::new("demo").await;
    fixture
        .workspace(1, "task", "abandoned", Some(30), "t-1")
        .await;
    fixture
        .checkpoint(1, "checkpoint", CheckpointReason::Explicit)
        .await;
    let path = fixture.root().join("ws").join("t-1");
    fs::set_permissions(
        path.join("src").join("lib.rs"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    fs::remove_dir_all(&path).unwrap();
    let planned = gc(&fixture.handle, true).await.unwrap();
    let report = gc(&fixture.handle, false).await.unwrap();
    assert_eq!(counts(&planned), counts(&report));
    assert_eq!(
        (report.workspaces_removed, report.checkpoints_pruned),
        (1, 1)
    );
}

async fn state_with(fixtures: &[&Fixture], clock: &ManualClock) -> AppState {
    let state = Arc::new(DaemonState {
        registry: fixtures[0].project.registry().clone(),
        projects: Projects::default(),
        clock: Arc::new(clock.clone()),
        assets: AssetSource::Embedded,
    });
    for (n, fixture) in fixtures.iter().enumerate() {
        state
            .projects
            .open(
                &row(&format!("p{n}"), &fixture.project),
                Arc::new(clock.clone()),
            )
            .await
            .unwrap();
    }
    state
}

async fn call(router: axum::Router, path: &str, body: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-fold-human", "ana")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn the_route_answers_on_the_socket_only() {
    let fixture = Fixture::new("p0").await;
    let orphan = fixture.objects.write(ObjectKind::Blob, b"orphan").unwrap();
    fixture.age_all();
    let state = state_with(&[&fixture], fixture.project.clock()).await;

    let (status, body) = call(router(state.clone()), "/api/v1/projects/p0/gc", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let socket = || socket_router(state.clone());
    let (status, body) = call(socket(), "/api/v1/projects/p0/gc", r#"{"dry_run": true}"#).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["deleted"], 1);
    assert!(fixture.objects.contains(&orphan).unwrap());

    let (status, body) = call(socket(), "/api/v1/projects/p0/gc", "{}").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dry_run"], false);
    for field in [
        "examined",
        "bytes_freed",
        "tmp_removed",
        "missing",
        "workspaces_removed",
        "checkpoints_pruned",
    ] {
        assert!(body[field].is_u64(), "{field}");
    }
    assert_eq!(body["deleted"], 1);
    assert!(!fixture.objects.contains(&orphan).unwrap());

    let handle = state.projects.handles().pop().unwrap();
    let running = handle.gc.try_lock().unwrap();
    let (status, body) = call(socket(), "/api/v1/projects/p0/gc", "{}").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "gc_in_progress");
    drop(running);

    let (status, body) = call(socket(), "/api/v1/projects/absent/gc", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "project_not_found");
    let (status, body) = call(socket(), "/api/v1/projects/p0/gc", r#"{"dry_run": 3}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
}

async fn wait_for(
    progress: &mut watch::Receiver<JobProgress>,
    done: impl Fn(&JobProgress) -> bool,
) -> JobProgress {
    tokio::time::timeout(WAIT, progress.wait_for(|seen| done(seen)))
        .await
        .unwrap()
        .map(|seen| *seen)
        .unwrap()
}

#[tokio::test]
async fn gc_daily_runs_every_24_hours_of_the_clock_and_never_at_start() {
    let one = Fixture::new("p0").await;
    let two = Fixture::new("p1").await;
    let clock = one.project.clock().clone();
    let orphans =
        [&one, &two].map(|fixture| fixture.objects.write(ObjectKind::Blob, b"orphan").unwrap());
    one.age_all();
    two.age_all();
    let state = state_with(&[&one, &two], &clock).await;
    let (stop, stopped) = watch::channel(false);
    let job = spawn_gc_daily(state, Duration::from_millis(5), stopped);
    let mut progress = job.progress();

    let seen = wait_for(&mut progress, |seen| seen.checks >= 3).await;
    assert_eq!(seen.cycles, 0);
    assert!(
        one.objects.contains(&orphans[0]).unwrap() && two.objects.contains(&orphans[1]).unwrap()
    );

    clock.advance(DAY).unwrap();
    wait_for(&mut progress, |seen| seen.cycles == 1).await;
    assert!(!one.objects.contains(&orphans[0]).unwrap());
    assert!(!two.objects.contains(&orphans[1]).unwrap());

    let later = one
        .objects
        .write(ObjectKind::Blob, b"later orphan")
        .unwrap();
    age(&one.objects.path(&later), OLD);
    clock.advance(DAY - Duration::from_secs(60 * 60)).unwrap();
    let checks = progress.borrow().checks;
    let seen = wait_for(&mut progress, |seen| seen.checks >= checks + 3).await;
    assert_eq!(seen.cycles, 1);
    assert!(one.objects.contains(&later).unwrap());

    clock.advance(Duration::from_secs(60 * 60)).unwrap();
    wait_for(&mut progress, |seen| seen.cycles == 2).await;
    eventually(WAIT, async || {
        if one.objects.contains(&later).unwrap() {
            Err("kept")
        } else {
            Ok(())
        }
    })
    .await;

    stop.send(true).unwrap();
    job.stopped(WAIT).await;
}

fn blob_of(label: &str) -> ObjectHash {
    hash_object(ObjectKind::Blob, label.as_bytes())
}

/// The hash [`Fixture::tree`] gives `label`, without writing the objects again.
fn tree_of(label: &str) -> ObjectHash {
    let tree = Tree {
        entries: vec![TreeEntry {
            name: b"file".to_vec(),
            kind: EntryKind::File,
            hash: blob_of(label),
        }],
    };
    hash_object(ObjectKind::Tree, &tree.encode().unwrap())
}

#[tokio::test]
async fn the_roots_keep_what_rows_reach_and_old_orphans_go() {
    let fixture = Fixture::new("demo").await;
    fixture.workspace(1, "task", "active", None, "t-1").await;
    let pruned = fixture
        .checkpoint(1, "auto, pruned", CheckpointReason::Quiet)
        .await;
    let now = fixture.project.now();
    fixture
        .store()
        .write(async |tx| {
            prune_auto(tx, ws(1), 0, now).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let auto = fixture
        .checkpoint(1, "auto, kept", CheckpointReason::BeforeSync)
        .await;
    let explicit = fixture
        .checkpoint(1, "explicit", CheckpointReason::Explicit)
        .await;
    let recorded = fixture.tree("changeset");
    let proposed = fixture.tree("proposal version");
    fixture
        .store()
        .write(async |tx| {
            insert_changeset(
                tx,
                ws(1),
                TaskId::from_number(1).unwrap(),
                RevisionId::from_number(1).unwrap(),
                recorded,
                "record",
                ChangesetStats::default(),
                "frontend",
                now,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    execute(
        fixture.store(),
        &format!(
            "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 'proposal', 0, 0);
             INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_at)
             VALUES (1, 1, 1, '{proposed}', 1, 0);"
        ),
    )
    .await;
    let old_orphan = fixture
        .objects
        .write(ObjectKind::Blob, b"old orphan")
        .unwrap();
    fixture.age_all();
    let young_orphan = fixture
        .objects
        .write(ObjectKind::Blob, b"young orphan")
        .unwrap();

    let roots = gc_roots(fixture.store().read()).await.unwrap();
    let marked = mark(&fixture.objects, roots).unwrap();
    let report = sweep(&fixture.objects, &marked, SystemTime::now(), GRACE).unwrap();

    let r1 = initial_revision(tree_of("base"), now).unwrap().hash();
    let kept = [
        r1,
        tree_of("base"),
        blob_of("base"),
        auto,
        blob_of("auto, kept"),
        explicit,
        blob_of("explicit"),
        recorded,
        blob_of("changeset"),
        proposed,
        blob_of("proposal version"),
        young_orphan,
    ];
    for hash in kept {
        assert!(fixture.objects.contains(&hash).unwrap(), "{hash}");
    }
    for hash in [pruned, blob_of("auto, pruned"), old_orphan] {
        assert!(!fixture.objects.contains(&hash).unwrap(), "{hash}");
    }
    assert_eq!((report.examined, report.deleted), (15, 3));
    assert!(report.missing.is_empty());
}

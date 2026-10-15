use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{IntegrationId, RevisionId, Timestamp};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError, workspaces};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};

/// The project rows a proposal integration and a workspace point at: agent `backend` with worker `backend/api`, task `T-3`, its workspace `ws-1` at `ws/t-3` on `r1`, and proposal `P-5` "Add the guide" with one version.
const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend/api', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 3, 'three', 'backend', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-3', 1, 'active', 0);
";

pub const R1: &[(&str, &[u8])] = &[
    ("README.md", b"hello\n"),
    ("logo.png", b"\x89PNG\0\x01"),
    ("src/lib.rs", b"fn a() {}\n"),
];

pub const R2: &[(&str, &[u8])] = &[
    ("README.md", b"hello\n"),
    ("docs/guide.md", b"# Guide\n"),
    ("logo.png", b"\x89PNG\0\x01"),
    ("src/lib.rs", b"fn a() {}\nfn b() {}\n"),
];

pub const R3: &[(&str, &[u8])] = &[
    ("docs/guide.md", b"# Guide\n"),
    ("logo.png", b"\x89PNG\0\x01"),
    ("src/lib.rs", b"fn a() {}\nfn b() {}\n"),
    ("src/main.rs", b"fn main() {}\n"),
];

const LEASE_MINUTES: u64 = 30;

const STATS: &str = r#"{"files_added":0,"files_modified":0,"files_deleted":0}"#;

pub fn objects(project: &TestProject) -> ObjectStore {
    ObjectStore::open(&project.paths().fold_dir()).unwrap()
}

pub fn service(project: &TestProject) -> WorkspaceService {
    WorkspaceService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
        project.paths().clone(),
        LEASE_MINUTES,
    )
}

/// Stores the blobs and trees of `files`, all plain files, and returns the root tree.
pub fn tree(objects: &ObjectStore, files: &[(&str, &[u8])]) -> ObjectHash {
    let entries: FlatTree = files
        .iter()
        .map(|(path, content)| {
            let hash = objects.write(ObjectKind::Blob, content).unwrap();
            (
                path.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash,
                },
            )
        })
        .collect();
    write_initial_tree(objects, entries).unwrap()
}

pub async fn exec(store: &Store, sql: &str) {
    store
        .write(async |tx| {
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

fn id(n: u32) -> RevisionId {
    RevisionId::from_number(n).unwrap()
}

/// `r1` from `fld init`, `r2` integrating proposal `P-5` of `T-3` by `backend`, and `r3` pulled from remote commit `abc123` by `human`, each with real objects. The rows of [`ROWS`] exist too.
pub async fn three_revisions(project: &TestProject) {
    let objects = objects(project);
    let r1 =
        workspaces::initial_revision(tree(&objects, R1), Timestamp::from_millis(1_000).unwrap())
            .unwrap();
    let stored = write_revision(&objects, &r1).unwrap();
    workspaces::create_initial_revision(project.store(), &stored, None)
        .await
        .unwrap();
    exec(project.store(), ROWS).await;
    let r1_tree = r1.tree.to_string();
    exec(
        project.store(),
        &format!(
            "INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (100, 100, 1, 1, 1, '{r1_tree}', 'seed', 'backend/api', 0);
             INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 5, 1, 1, 'Add the guide', 0, 0);
             INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, '{r1_tree}', 100, 'backend/api', 0);
             INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 1, 'proposal', 1, 1, 'succeeded', 'review', 0);
             INSERT INTO integrations (id, number, kind, remote_commit, result, actor, created_at) VALUES (2, 2, 'pull', 'abc123', 'succeeded', 'human', 0);"
        ),
    )
    .await;
    let mut parent = r1;
    for (number, files) in [(2, R2), (3, R3)] {
        let revision = Revision {
            tree: tree(&objects, files),
            parent: Some(parent.hash()),
            number: id(number),
            integration: IntegrationId::from_number(number - 1),
            created_at: Timestamp::from_millis(1_000 + i64::from(number)).unwrap(),
        };
        let stored = write_revision(&objects, &revision).unwrap();
        project
            .store()
            .write(async |tx| {
                workspaces::insert_revision(tx, &stored).await?;
                workspaces::advance_main(tx, parent.number, revision.number).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        parent = revision;
    }
}

/// Inserts changeset `C-<number>` of `ws-1` holding `files`.
pub async fn changeset(project: &TestProject, number: u32, files: &[(&str, &[u8])]) {
    let tree = tree(&objects(project), files).to_string();
    exec(
        project.store(),
        &format!(
            "INSERT INTO changesets (number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES ({number}, 1, 1, 1, '{tree}', 'step', '{STATS}', 'backend/api', 0);"
        ),
    )
    .await;
}

/// Inserts checkpoint `ck-<number>` of `ws-1` holding `files`.
pub async fn checkpoint(project: &TestProject, number: u32, files: &[(&str, &[u8])]) {
    let tree = tree(&objects(project), files).to_string();
    exec(
        project.store(),
        &format!(
            "INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, reason, created_at) VALUES ({number}, 1, '{tree}', 'explicit', 'explicit', 0);"
        ),
    )
    .await;
}

/// The directory of `ws-1`, filled with `files`.
pub fn workspace_dir(project: &TestProject, files: &[(&str, &[u8])]) -> PathBuf {
    let root = project.paths().root().join("ws/t-3");
    for (path, content) in files {
        let at = root.join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, content).unwrap();
    }
    root
}

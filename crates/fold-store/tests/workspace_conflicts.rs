#![cfg(test)]

use crate::common::{count, exec, open};
use fold_core::WorkspaceId;
use fold_core::merge::ConflictKind;
use fold_store::{Store, StoreError, workspaces};
use tempfile::TempDir;

const SEED: &str = "
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'frontend', 0, 0), (2, 2, 'two', 'frontend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, kind, path, base_revision_id, state, created_at) VALUES (10, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (20, 2, 2, 'task', 'ws/t-2', 1, 'active', 0);
";

/// Holds the directory so the database outlives the test body.
struct Project {
    _dir: TempDir,
    store: Store,
}

impl Project {
    fn store(&self) -> &Store {
        &self.store
    }
}

async fn seeded() -> Project {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    Project { _dir: dir, store }
}

fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

fn row(path: &str, kind: ConflictKind) -> workspaces::ConflictRow {
    workspaces::ConflictRow {
        path: path.as_bytes().to_vec(),
        kind,
    }
}

async fn set(
    project: &Project,
    workspace: WorkspaceId,
    rows: &[workspaces::ConflictRow],
) -> Result<(), StoreError> {
    project
        .store()
        .write(async |tx| workspaces::set_conflicts(tx, workspace, rows).await)
        .await
}

async fn clear(project: &Project, workspace: WorkspaceId, path: &str) -> bool {
    project
        .store()
        .write(async |tx| workspaces::clear_conflict(tx, workspace, path.as_bytes()).await)
        .await
        .unwrap()
}

async fn list(project: &Project, workspace: WorkspaceId) -> Vec<workspaces::ConflictRow> {
    workspaces::list_conflicts(project.store().read(), workspace)
        .await
        .unwrap()
}

#[tokio::test]
async fn three_conflicts_list_in_path_order_and_one_clears() {
    let project = seeded().await;
    let three = [
        row("src/z.rs", ConflictKind::BothModifiedText),
        row("a.bin", ConflictKind::BothModifiedBinary),
        row("src/b", ConflictKind::TypeChange),
    ];
    set(&project, ws(1), &three).await.unwrap();
    set(&project, ws(2), &[row("other", ConflictKind::AddAdd)])
        .await
        .unwrap();
    assert_eq!(
        list(&project, ws(1)).await,
        [
            row("a.bin", ConflictKind::BothModifiedBinary),
            row("src/b", ConflictKind::TypeChange),
            row("src/z.rs", ConflictKind::BothModifiedText),
        ]
    );

    assert!(clear(&project, ws(1), "src/b").await);
    assert!(!clear(&project, ws(1), "src/b").await);
    assert_eq!(
        list(&project, ws(1)).await,
        [
            row("a.bin", ConflictKind::BothModifiedBinary),
            row("src/z.rs", ConflictKind::BothModifiedText),
        ]
    );
    assert_eq!(
        list(&project, ws(2)).await,
        [row("other", ConflictKind::AddAdd)]
    );
}

#[tokio::test]
async fn a_duplicate_path_in_one_workspace_is_refused_by_the_database() {
    let project = seeded().await;
    set(&project, ws(1), &[row("kept", ConflictKind::ModifyDelete)])
        .await
        .unwrap();
    let twice = [
        row("dup", ConflictKind::BothModifiedText),
        row("dup", ConflictKind::AddAdd),
    ];
    let error = set(&project, ws(1), &twice).await.unwrap_err();
    assert!(error.unique_violation(), "{error}");
    assert_eq!(
        list(&project, ws(1)).await,
        [row("kept", ConflictKind::ModifyDelete)]
    );

    let raw = exec(
        project.store(),
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (10, CAST('kept' AS BLOB), 'add_add')",
    )
    .await
    .unwrap_err();
    assert!(raw.unique_violation(), "{raw}");
    set(&project, ws(2), &[row("kept", ConflictKind::AddAdd)])
        .await
        .unwrap();
}

#[tokio::test]
async fn setting_replaces_the_list_and_an_empty_list_clears_it() {
    let project = seeded().await;
    set(&project, ws(1), &[row("a", ConflictKind::AddAdd)])
        .await
        .unwrap();
    set(&project, ws(1), &[row("b", ConflictKind::ModifyDelete)])
        .await
        .unwrap();
    assert_eq!(
        list(&project, ws(1)).await,
        [row("b", ConflictKind::ModifyDelete)]
    );
    set(&project, ws(1), &[]).await.unwrap();
    assert!(list(&project, ws(1)).await.is_empty());

    let missing = set(&project, ws(9), &[row("a", ConflictKind::AddAdd)])
        .await
        .unwrap_err();
    assert!(matches!(missing, StoreError::Invalid(_)));
    assert!(list(&project, ws(9)).await.is_empty());
}

#[tokio::test]
async fn paths_must_be_bytes_and_kinds_must_be_known() {
    let project = seeded().await;
    for sql in [
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (10, 'text', 'add_add')",
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (10, X'', 'add_add')",
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (10, X'61', 'renamed')",
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (99, X'61', 'add_add')",
    ] {
        assert!(exec(project.store(), sql).await.is_err(), "{sql}");
    }
    assert_eq!(
        count(project.store(), "SELECT COUNT(*) FROM workspace_conflicts").await,
        0
    );
    assert_eq!(
        count(
            project.store(),
            "SELECT COUNT(*) FROM pragma_table_info('workspaces') WHERE name = 'conflicted_paths'"
        )
        .await,
        0
    );
}

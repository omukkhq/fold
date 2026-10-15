#![cfg(test)]

#[cfg(test)]
mod common;

use common::{count, exec, open};
use fold_core::{RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId};
use fold_store::workspaces::{
    self, ClosedState, NewTaskWorkspace, WorkspaceKind, WorkspaceRow, WorkspaceState,
};
use fold_store::{Store, StoreError};
use tempfile::TempDir;

const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0), (2, 2, 'two', 'backend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
";

struct Project {
    _dir: TempDir,
    store: Store,
}

async fn seeded() -> Project {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    Project { _dir: dir, store }
}

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

impl Project {
    async fn insert(&self, number: u32, task: u32, path: &str) -> Result<(), StoreError> {
        let new = NewTaskWorkspace {
            id: ws(number),
            task: t(task),
            worker_id: 1,
            path,
            base: RevisionId::from_number(1).unwrap(),
            now: at(10),
        };
        self.store
            .write(async |tx| workspaces::insert_task_workspace(tx, &new).await)
            .await
    }

    async fn close(&self, number: u32, state: ClosedState) -> bool {
        self.store
            .write(async |tx| workspaces::close(tx, ws(number), state, at(20)).await)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_second_live_task_workspace_is_refused_by_the_index() {
    let project = seeded().await;
    project.insert(1, 1, "ws/t-1").await.unwrap();
    let error = project.insert(2, 1, "ws/t-1-2").await.unwrap_err();
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("workspaces.task_id"));
    exec(&project.store, "UPDATE workspaces SET state = 'missing'")
        .await
        .unwrap();
    let missing = project.insert(2, 1, "ws/t-1-2").await.unwrap_err();
    assert_eq!(missing.constraint().as_deref(), Some("workspaces.task_id"));
    assert_eq!(
        count(&project.store, "SELECT COUNT(*) FROM workspaces").await,
        1
    );
}

#[tokio::test]
async fn an_abandoned_workspace_allows_a_new_one() {
    let project = seeded().await;
    project.insert(1, 1, "ws/t-1").await.unwrap();
    assert!(project.close(1, ClosedState::Abandoned).await);
    project.insert(2, 1, "ws/t-1-2").await.unwrap();
    assert!(project.close(2, ClosedState::Archived).await);
    project.insert(3, 1, "ws/t-1-3").await.unwrap();
    assert_eq!(
        workspaces::task_workspace_paths(project.store.read(), t(1))
            .await
            .unwrap(),
        ["ws/t-1", "ws/t-1-2", "ws/t-1-3"]
    );
}

#[tokio::test]
async fn two_workspaces_never_share_a_path() {
    let project = seeded().await;
    project.insert(1, 1, "ws/t-1").await.unwrap();
    assert!(project.close(1, ClosedState::Abandoned).await);
    let error = project.insert(2, 2, "ws/t-1").await.unwrap_err();
    assert_eq!(error.constraint().as_deref(), Some("workspaces.path"));
}

#[tokio::test]
async fn an_unknown_task_or_base_inserts_nothing() {
    let project = seeded().await;
    let error = project.insert(1, 9, "ws/t-9").await.unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)));
    let new = NewTaskWorkspace {
        id: ws(1),
        task: t(1),
        worker_id: 1,
        path: "ws/t-1",
        base: RevisionId::from_number(7).unwrap(),
        now: at(10),
    };
    let error = project
        .store
        .write(async |tx| workspaces::insert_task_workspace(tx, &new).await)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)));
    assert_eq!(
        count(&project.store, "SELECT COUNT(*) FROM workspaces").await,
        0
    );
}

#[tokio::test]
async fn reads_map_the_row_with_public_ids() {
    let project = seeded().await;
    let read = project.store.read();
    assert_eq!(workspaces::get_workspace(read, ws(1)).await.unwrap(), None);
    assert_eq!(
        workspaces::live_task_workspace(read, t(1)).await.unwrap(),
        None
    );
    assert_eq!(
        workspaces::task_workspace_paths(read, t(1)).await.unwrap(),
        Vec::<String>::new()
    );
    project.insert(1, 1, "ws/t-1").await.unwrap();
    exec(
        &project.store,
        "INSERT INTO workspace_conflicts (workspace_id, path, kind) SELECT id, CAST('a' AS BLOB), 'add_add' FROM workspaces",
    )
    .await
    .unwrap();
    let row = workspaces::get_workspace(read, ws(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row,
        WorkspaceRow {
            id: ws(1),
            task: t(1),
            worker_id: Some(1),
            worker: Some(WorkerName::parse("backend").unwrap()),
            kind: WorkspaceKind::Task,
            path: "ws/t-1".to_owned(),
            base: RevisionId::from_number(1).unwrap(),
            proposal: None,
            state: WorkspaceState::Active,
            changed_files: 0,
            conflicted: 1,
            last_activity_at: Some(at(10)),
            created_at: at(10),
            closed_at: None,
        }
    );
    assert_eq!(
        workspaces::live_task_workspace(read, t(1)).await.unwrap(),
        Some(row.clone())
    );

    assert!(project.close(1, ClosedState::Abandoned).await);
    assert!(!project.close(1, ClosedState::Archived).await);
    assert!(!project.close(9, ClosedState::Archived).await);
    assert_eq!(
        workspaces::live_task_workspace(read, t(1)).await.unwrap(),
        None
    );
    let closed = workspaces::latest_task_workspace(read, t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed.state, WorkspaceState::Abandoned);
    assert_eq!(closed.closed_at, Some(at(20)));

    project.insert(2, 1, "ws/t-1-2").await.unwrap();
    let latest = workspaces::latest_task_workspace(read, t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, ws(2));
    assert_eq!(
        workspaces::latest_task_workspace(read, t(2)).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_review_checkout_is_not_a_task_workspace() {
    let project = seeded().await;
    exec(
        &project.store,
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, NULL, 'review', 'ws/review-p-1', 1, 'missing', 0)",
    )
    .await
    .unwrap();
    let read = project.store.read();
    assert_eq!(
        workspaces::live_task_workspace(read, t(1)).await.unwrap(),
        None
    );
    assert_eq!(
        workspaces::task_workspace_paths(read, t(1)).await.unwrap(),
        Vec::<String>::new()
    );
    let row = workspaces::get_workspace(read, ws(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, WorkspaceKind::Review);
    assert_eq!(row.state, WorkspaceState::Missing);
    assert!(row.state.is_live());
    assert_eq!(row.worker, None);
    assert_eq!(row.last_activity_at, None);
}

#[test]
fn names_round_trip() {
    for state in [
        WorkspaceState::Active,
        WorkspaceState::Missing,
        WorkspaceState::Archived,
        WorkspaceState::Abandoned,
    ] {
        assert_eq!(
            state.is_live(),
            matches!(state.as_str(), "active" | "missing")
        );
    }
    assert_eq!(WorkspaceKind::Task.as_str(), "task");
    assert_eq!(WorkspaceKind::Review.as_str(), "review");
    assert_eq!(ClosedState::Archived.as_str(), "archived");
    assert_eq!(ClosedState::Abandoned.as_str(), "abandoned");
}

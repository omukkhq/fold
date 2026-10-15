use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::{RevisionId, TaskId, WorkspaceId};
use fold_store::{Store, StoreError};

/// The rows a checkpoint or changeset points at.
#[derive(Debug, Clone, Copy)]
pub struct Seeded {
    pub task: TaskId,
    pub revision: RevisionId,
    pub workspace: WorkspaceId,
}

/// Inserts task `T-n`, revision `rn` (whose parent is `r(n-1)`, so call it with 1, 2, … in order) and active task workspace `ws-n` with raw SQL. Each workspace gets its own task, so several live task workspaces can coexist.
pub async fn workspace(store: &Store, n: u32) -> Seeded {
    let number = i64::from(n);
    store
        .write(async |tx| {
            sqlx::query(
                "INSERT INTO tasks (number, title, created_by, created_at, updated_at) VALUES (?1, 'task', 'frontend', 0, 0)",
            )
            .bind(number)
            .execute(tx.conn())
            .await?;
            sqlx::query(
                "INSERT INTO revisions (number, parent_id, hash, tree_hash, created_at) VALUES (?1, (SELECT id FROM revisions WHERE number = ?1 - 1), ?2, ?3, 0)",
            )
            .bind(number)
            .bind(tree(&format!("revision {n}")).to_string())
            .bind(tree("revision tree").to_string())
            .execute(tx.conn())
            .await?;
            sqlx::query(
                "INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at)
                 VALUES (?1, (SELECT id FROM tasks WHERE number = ?1), 'task', 'ws/' || ?1, (SELECT id FROM revisions WHERE number = ?1), 'active', 0)",
            )
            .bind(number)
            .execute(tx.conn())
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    Seeded {
        task: TaskId::from_number(n).unwrap(),
        revision: RevisionId::from_number(n).unwrap(),
        workspace: WorkspaceId::from_number(n).unwrap(),
    }
}

/// A tree hash for `label`; the object store is not involved, since the store only keeps the hash.
pub fn tree(label: &str) -> ObjectHash {
    hash_object(ObjectKind::Tree, label.as_bytes())
}

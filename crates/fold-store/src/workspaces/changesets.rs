use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{ChangesetId, IdKind, RevisionId, TaskId, Timestamp, WorkspaceId};
use serde_json::{Value, json};
use sqlx::SqliteExecutor;

use super::columns::{number, timestamp, tree_hash};
use crate::counters;
use crate::error::StoreError;
use crate::store::WriteTx;

/// File counts of the changed paths a changeset records. Line counts are not stored: they are computed when a comparison is read.
pub type ChangesetStats = ChangeCounts;

fn stats_json(stats: ChangesetStats) -> String {
    json!({
        "files_added": stats.files_added,
        "files_modified": stats.files_modified,
        "files_deleted": stats.files_deleted,
    })
    .to_string()
}

pub(crate) fn parse_stats(text: &str) -> Option<ChangesetStats> {
    let value: Value = serde_json::from_str(text).ok()?;
    let count = |key: &str| u32::try_from(value.get(key)?.as_u64()?).ok();
    Some(ChangesetStats {
        files_added: count("files_added")?,
        files_modified: count("files_modified")?,
        files_deleted: count("files_deleted")?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    pub id: ChangesetId,
    pub workspace: WorkspaceId,
    pub task: TaskId,
    /// The changeset recorded before this one in the same workspace.
    pub parent: Option<ChangesetId>,
    pub base_revision: RevisionId,
    pub tree: ObjectHash,
    pub message: String,
    pub stats: ChangesetStats,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// Inserts one changeset in the caller's write, chained to the workspace's latest changeset. `tree` must already be in the object store. A workspace, task or base revision that does not exist is refused, and so is a task the workspace does not belong to.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one column of the append-only row"
)]
pub async fn insert_changeset(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    task: TaskId,
    base_revision: RevisionId,
    tree: ObjectHash,
    message: &str,
    stats: ChangesetStats,
    created_by: &str,
    now: Timestamp,
) -> Result<Changeset, StoreError> {
    let workspace_number = workspace.to_db();
    let revision_number = base_revision.to_db();
    let found = sqlx::query!(
        r#"SELECT w.id AS "workspace_id!: i64", w.task_id AS "task_id!: i64",
                  t.number AS "task_number!: i64",
                  (SELECT id FROM revisions WHERE number = ?2) AS "revision_id?: i64"
           FROM workspaces w JOIN tasks t ON t.id = w.task_id
           WHERE w.number = ?1"#,
        workspace_number,
        revision_number
    )
    .fetch_optional(tx.conn())
    .await?
    .ok_or(StoreError::Invalid("the workspace does not exist"))?;
    if found.task_number != task.to_db() {
        return Err(StoreError::Invalid("the workspace belongs to another task"));
    }
    let revision_id = found
        .revision_id
        .ok_or(StoreError::Invalid("the base revision does not exist"))?;
    let parent = sqlx::query!(
        r#"SELECT id AS "id!: i64", number AS "number!: i64" FROM changesets
           WHERE workspace_id = ?
           ORDER BY number DESC
           LIMIT 1"#,
        found.workspace_id
    )
    .fetch_optional(tx.conn())
    .await?;
    let parent_id = parent.as_ref().map(|row| row.id);
    let parent = parent
        .map(|row| number(row.number, ChangesetId::try_from_db, "changesets.number"))
        .transpose()?;

    let id = counters::allocate(tx, IdKind::Changeset).await?;
    let id = ChangesetId::from_number(id).ok_or(StoreError::Corrupt("counters.next"))?;
    let changeset_number = id.to_db();
    let tree_text = tree.to_string();
    let stats_text = stats_json(stats);
    let created_at = now.as_millis();
    sqlx::query!(
        r#"INSERT INTO changesets (number, workspace_id, task_id, parent_changeset_id,
                                   base_revision_id, tree_hash, message, stats, created_by, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        changeset_number,
        found.workspace_id,
        found.task_id,
        parent_id,
        revision_id,
        tree_text,
        message,
        stats_text,
        created_by,
        created_at
    )
    .execute(tx.conn())
    .await?;
    Ok(Changeset {
        id,
        workspace,
        task,
        parent,
        base_revision,
        tree,
        message: message.to_owned(),
        stats,
        created_by: created_by.to_owned(),
        created_at: now,
    })
}

/// The newest changeset of the workspace, the parent of the next one.
pub async fn latest_changeset<'e>(
    db: impl SqliteExecutor<'e>,
    workspace: WorkspaceId,
) -> Result<Option<Changeset>, StoreError> {
    let workspace_number = workspace.to_db();
    let row = sqlx::query_as!(
        Row,
        r#"SELECT c.number AS "number!: i64", w.number AS "workspace!: i64",
                  t.number AS "task!: i64", p.number AS "parent?: i64",
                  r.number AS "base_revision!: i64", c.tree_hash, c.message, c.stats,
                  c.created_by, c.created_at
           FROM changesets c
           JOIN workspaces w ON w.id = c.workspace_id
           JOIN tasks t ON t.id = c.task_id
           JOIN revisions r ON r.id = c.base_revision_id
           LEFT JOIN changesets p ON p.id = c.parent_changeset_id
           WHERE w.number = ?
           ORDER BY c.number DESC
           LIMIT 1"#,
        workspace_number
    )
    .fetch_optional(db)
    .await?;
    row.map(Row::into_changeset).transpose()
}

pub async fn get_changeset<'e>(
    db: impl SqliteExecutor<'e>,
    id: ChangesetId,
) -> Result<Option<Changeset>, StoreError> {
    let changeset_number = id.to_db();
    let row = sqlx::query_as!(
        Row,
        r#"SELECT c.number AS "number!: i64", w.number AS "workspace!: i64",
                  t.number AS "task!: i64", p.number AS "parent?: i64",
                  r.number AS "base_revision!: i64", c.tree_hash, c.message, c.stats,
                  c.created_by, c.created_at
           FROM changesets c
           JOIN workspaces w ON w.id = c.workspace_id
           JOIN tasks t ON t.id = c.task_id
           JOIN revisions r ON r.id = c.base_revision_id
           LEFT JOIN changesets p ON p.id = c.parent_changeset_id
           WHERE c.number = ?"#,
        changeset_number
    )
    .fetch_optional(db)
    .await?;
    row.map(Row::into_changeset).transpose()
}

/// The changesets of one workspace, oldest first.
pub async fn list_changesets<'e>(
    db: impl SqliteExecutor<'e>,
    workspace: WorkspaceId,
) -> Result<Vec<Changeset>, StoreError> {
    let workspace_number = workspace.to_db();
    let rows = sqlx::query_as!(
        Row,
        r#"SELECT c.number AS "number!: i64", w.number AS "workspace!: i64",
                  t.number AS "task!: i64", p.number AS "parent?: i64",
                  r.number AS "base_revision!: i64", c.tree_hash, c.message, c.stats,
                  c.created_by, c.created_at
           FROM changesets c
           JOIN workspaces w ON w.id = c.workspace_id
           JOIN tasks t ON t.id = c.task_id
           JOIN revisions r ON r.id = c.base_revision_id
           LEFT JOIN changesets p ON p.id = c.parent_changeset_id
           WHERE w.number = ?
           ORDER BY c.number ASC"#,
        workspace_number
    )
    .fetch_all(db)
    .await?;
    rows.into_iter().map(Row::into_changeset).collect()
}

struct Row {
    number: i64,
    workspace: i64,
    task: i64,
    parent: Option<i64>,
    base_revision: i64,
    tree_hash: String,
    message: String,
    stats: String,
    created_by: String,
    created_at: i64,
}

impl Row {
    fn into_changeset(self) -> Result<Changeset, StoreError> {
        Ok(Changeset {
            id: number(self.number, ChangesetId::try_from_db, "changesets.number")?,
            workspace: number(
                self.workspace,
                WorkspaceId::try_from_db,
                "workspaces.number",
            )?,
            task: number(self.task, TaskId::try_from_db, "tasks.number")?,
            parent: self
                .parent
                .map(|value| number(value, ChangesetId::try_from_db, "changesets.number"))
                .transpose()?,
            base_revision: number(
                self.base_revision,
                RevisionId::try_from_db,
                "revisions.number",
            )?,
            tree: tree_hash(&self.tree_hash, "changesets.tree_hash")?,
            message: self.message,
            stats: parse_stats(&self.stats).ok_or(StoreError::Corrupt("changesets.stats"))?,
            created_by: self.created_by,
            created_at: timestamp(self.created_at, "changesets.created_at")?,
        })
    }
}

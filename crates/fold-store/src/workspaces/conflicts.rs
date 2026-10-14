use fold_core::WorkspaceId;
use fold_core::merge::ConflictKind;
use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;

/// One conflicted path of a workspace. The path is the workspace path's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRow {
    pub path: Vec<u8>,
    pub kind: ConflictKind,
}

/// Replaces the workspace's whole conflict list, as a sync does; an empty slice clears it. A path given twice is refused by the primary key and fails the transaction.
pub async fn set_conflicts(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    conflicts: &[ConflictRow],
) -> Result<(), StoreError> {
    let number = workspace.to_db();
    let id = sqlx::query_scalar!(
        r#"SELECT id AS "id!: i64" FROM workspaces WHERE number = ?"#,
        number
    )
    .fetch_optional(tx.conn())
    .await?
    .ok_or(StoreError::Invalid("the workspace does not exist"))?;
    sqlx::query!("DELETE FROM workspace_conflicts WHERE workspace_id = ?", id)
        .execute(tx.conn())
        .await?;
    for conflict in conflicts {
        let kind = conflict.kind.as_str();
        sqlx::query!(
            "INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (?, ?, ?)",
            id,
            conflict.path,
            kind
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Takes one resolved path off the list. False when it was not listed.
pub async fn clear_conflict(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    path: &[u8],
) -> Result<bool, StoreError> {
    let number = workspace.to_db();
    let removed = sqlx::query!(
        "DELETE FROM workspace_conflicts
         WHERE workspace_id = (SELECT id FROM workspaces WHERE number = ?) AND path = ?",
        number,
        path
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(removed == 1)
}

/// In path byte order, which is the order trees use.
pub async fn list_conflicts(
    executor: impl SqliteExecutor<'_>,
    workspace: WorkspaceId,
) -> Result<Vec<ConflictRow>, StoreError> {
    let number = workspace.to_db();
    let rows = sqlx::query!(
        r#"SELECT c.path AS "path!: Vec<u8>", c.kind
           FROM workspace_conflicts c JOIN workspaces w ON w.id = c.workspace_id
           WHERE w.number = ?
           ORDER BY c.path"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ConflictRow {
                path: row.path,
                kind: ConflictKind::parse(&row.kind)
                    .ok_or(StoreError::Corrupt("workspace_conflicts.kind"))?,
            })
        })
        .collect()
}

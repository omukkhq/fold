use fold_core::{TaskId, Timestamp, WorkspaceId};

use crate::error::StoreError;
use crate::store::WriteTx;

/// Moves the live task workspace of `task` (state `active` or `missing`) to `archived`, stamps `closed_at`, and returns its number. Only the row changes; the directory stays until garbage collection.
pub async fn archive_live_task_workspace(
    tx: &mut WriteTx,
    task: TaskId,
    now: Timestamp,
) -> Result<Option<WorkspaceId>, StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    sqlx::query_scalar!(
        r#"UPDATE workspaces SET state = 'archived', closed_at = ?2
           WHERE kind = 'task' AND state IN ('active', 'missing')
             AND task_id = (SELECT id FROM tasks WHERE number = ?1)
           RETURNING number AS "number!: i64""#,
        number,
        now
    )
    .fetch_optional(tx.conn())
    .await?
    .map(|workspace| {
        WorkspaceId::try_from_db(workspace).ok_or(StoreError::Corrupt("workspaces.number"))
    })
    .transpose()
}

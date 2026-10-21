use fold_core::{ChecklistItemId, TaskId, Timestamp};
use sqlx::SqliteConnection;

use crate::error::StoreError;
use crate::store::WriteTx;

/// What a task has left behind that `delete_task` may not erase. Workspaces and proposals count in any state and of any kind; a message counts when it names the task, and a worker's turn when it counts for the task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskHistory {
    pub workspace: bool,
    pub proposal: bool,
    pub completion: bool,
    pub message: bool,
    pub report: bool,
    pub turn: bool,
}

impl TaskHistory {
    pub fn any(self) -> bool {
        self.workspace
            || self.proposal
            || self.completion
            || self.message
            || self.report
            || self.turn
    }
}

pub async fn history(conn: &mut SqliteConnection, task: TaskId) -> Result<TaskHistory, StoreError> {
    let number = task.to_db();
    let row = sqlx::query!(
        r#"SELECT EXISTS (SELECT 1 FROM workspaces w WHERE w.task_id = t.id) AS "workspace!: bool",
                  EXISTS (SELECT 1 FROM proposals p WHERE p.task_id = t.id) AS "proposal!: bool",
                  EXISTS (SELECT 1 FROM completions c WHERE c.task_id = t.id) AS "completion!: bool",
                  EXISTS (SELECT 1 FROM messages m WHERE m.task_id = t.id) AS "message!: bool",
                  EXISTS (SELECT 1 FROM task_reports r WHERE r.task_id = t.id) AS "report!: bool",
                  EXISTS (SELECT 1 FROM worker_turns x WHERE x.task_id = t.id) AS "turn!: bool"
           FROM tasks t WHERE t.number = ?"#,
        number
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map_or_else(TaskHistory::default, |row| TaskHistory {
        workspace: row.workspace,
        proposal: row.proposal,
        completion: row.completion,
        message: row.message,
        report: row.report,
        turn: row.turn,
    }))
}

/// Each linked checklist item pointing at `task` with the task that owns it, in owner number order, then item number order. An owner appears twice when two of its items link to `task`.
pub async fn linked_item_owners(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Vec<(TaskId, ChecklistItemId)>, StoreError> {
    let number = task.to_db();
    sqlx::query!(
        r#"SELECT owner.number AS "owner!: i64", i.number AS "item!: i64"
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks owner ON owner.id = c.task_id
           WHERE i.linked_task_id = (SELECT id FROM tasks WHERE number = ?)
           ORDER BY owner.number, i.number"#,
        number
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            TaskId::try_from_db(row.owner).ok_or(StoreError::Corrupt("tasks.number"))?,
            ChecklistItemId::try_from_db(row.item)
                .ok_or(StoreError::Corrupt("checklist_items.number"))?,
        ))
    })
    .collect()
}

/// Deletes every linked checklist item pointing at `task` and returns how many there were. The edges those items made are not touched.
pub async fn delete_linked_items(tx: &mut WriteTx, task: TaskId) -> Result<u64, StoreError> {
    let number = task.to_db();
    let result = sqlx::query!(
        "DELETE FROM checklist_items WHERE linked_task_id = (SELECT id FROM tasks WHERE number = ?)",
        number
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// Deletes every edge into or out of `task` and returns how many there were.
pub async fn delete_edges_touching(tx: &mut WriteTx, task: TaskId) -> Result<u64, StoreError> {
    let number = task.to_db();
    let result = sqlx::query!(
        "DELETE FROM dependencies
         WHERE task_id = (SELECT id FROM tasks WHERE number = ?1)
            OR needs_task_id = (SELECT id FROM tasks WHERE number = ?1)",
        number
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// Stamps `archived_at` and `updated_at`. The status cache keeps its value: recompute skips archived tasks.
pub async fn set_archived(
    tx: &mut WriteTx,
    task: TaskId,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks SET archived_at = ?2, updated_at = ?2 WHERE number = ?1 AND archived_at IS NULL",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the task does not exist or is archived",
        ))
    }
}

/// Deletes the task row, its claim rows and, through the cascades, its labels, its own checklists with their items and its edges. Linked items pointing at it must be gone first, and a task with any [`TaskHistory`] cannot be deleted: those rows refer to it without a cascade.
pub async fn delete(tx: &mut WriteTx, task: TaskId) -> Result<(), StoreError> {
    let number = task.to_db();
    sqlx::query!(
        "DELETE FROM claims WHERE task_id = (SELECT id FROM tasks WHERE number = ?)",
        number
    )
    .execute(tx.conn())
    .await?;
    let result = sqlx::query!("DELETE FROM tasks WHERE number = ?", number)
        .execute(tx.conn())
        .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the task does not exist"))
    }
}

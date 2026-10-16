use fold_core::{HoldMark, HoldReason, TaskId, TaskTitle, Timestamp};
use sqlx::SqliteExecutor;

use super::{expect_one, timestamp};
use crate::error::StoreError;
use crate::store::WriteTx;

/// Puts the task on hold, or replaces the reason, setter and time of its hold, and stamps `updated_at`. The task must exist.
pub async fn set_hold(
    tx: &mut WriteTx,
    task: TaskId,
    mark: &HoldMark,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let reason = mark.reason.as_str();
    let set_by = mark.set_by.as_str();
    let set_at = mark.set_at.as_millis();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks
         SET hold_reason = ?2, hold_set_by = ?3, hold_set_at = ?4, updated_at = ?5
         WHERE number = ?1",
        number,
        reason,
        set_by,
        set_at,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Takes the task off hold with every column back to null, and stamps `updated_at`. The task must exist.
pub async fn clear_hold(tx: &mut WriteTx, task: TaskId, now: Timestamp) -> Result<(), StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks
         SET hold_reason = NULL, hold_set_by = NULL, hold_set_at = NULL, updated_at = ?2
         WHERE number = ?1",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// A task that waits for a person: a human task in Todo, or a task on hold with its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingTask {
    pub id: TaskId,
    pub title: TaskTitle,
    /// When it started to wait: the last status change of a human task (its creation when it never changed), the time a hold was set.
    pub since: Timestamp,
    /// `None` for a human task without a hold.
    pub reason: Option<HoldReason>,
}

/// The human tasks in Todo that are not archived, oldest wait first: every prerequisite is Done, so a person can do the work now.
pub async fn ready_human_tasks(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<WaitingTask>, StoreError> {
    sqlx::query!(
        r#"SELECT t.number AS "number!: i64", t.title,
                  COALESCE(
                      (SELECT MAX(e.created_at) FROM events e
                       WHERE e.task_id = t.number AND e.type = 'task.status_changed'),
                      t.created_at
                  ) AS "since!: i64"
           FROM tasks t
           WHERE t.kind = 'human' AND t.status = 'todo' AND t.archived_at IS NULL
           ORDER BY 3, t.number"#
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| waiting(row.number, &row.title, row.since, None))
    .collect()
}

/// The tasks on hold that are neither Done nor archived, oldest hold first.
pub async fn held_tasks(executor: impl SqliteExecutor<'_>) -> Result<Vec<WaitingTask>, StoreError> {
    sqlx::query!(
        r#"SELECT number AS "number!: i64", title, hold_reason AS "hold_reason!: String",
                  hold_set_at AS "hold_set_at!: i64"
           FROM tasks
           WHERE hold_reason IS NOT NULL AND status <> 'done' AND archived_at IS NULL
           ORDER BY hold_set_at, number"#
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        let reason = HoldReason::parse(&row.hold_reason)
            .map_err(|_| StoreError::Corrupt("tasks.hold_reason"))?;
        waiting(row.number, &row.title, row.hold_set_at, Some(reason))
    })
    .collect()
}

fn waiting(
    number: i64,
    title: &str,
    since: i64,
    reason: Option<HoldReason>,
) -> Result<WaitingTask, StoreError> {
    Ok(WaitingTask {
        id: TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number"))?,
        title: TaskTitle::parse(title).map_err(|_| StoreError::Corrupt("tasks.title"))?,
        since: timestamp(since, "tasks.since")?,
        reason,
    })
}

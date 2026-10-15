use fold_core::{DependencyOrigin, TaskId, TaskStatus, Timestamp};
use sqlx::{SqliteConnection, SqliteExecutor, SqlitePool};

use crate::error::StoreError;
use crate::store::WriteTx;

/// One stored edge: `task` needs `needs`. Both ends are public task numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeRow {
    pub task: TaskId,
    pub needs: TaskId,
    pub origin: DependencyOrigin,
    pub created_by: String,
    pub created_at: Timestamp,
}

struct RawEdge {
    task: i64,
    needs: i64,
    origin: String,
    created_by: String,
    created_at: i64,
}

impl RawEdge {
    fn into_row(self) -> Result<EdgeRow, StoreError> {
        Ok(EdgeRow {
            task: TaskId::try_from_db(self.task).ok_or(StoreError::Corrupt("tasks.number"))?,
            needs: TaskId::try_from_db(self.needs).ok_or(StoreError::Corrupt("tasks.number"))?,
            origin: DependencyOrigin::parse(&self.origin)
                .ok_or(StoreError::Corrupt("dependencies.origin"))?,
            created_by: self.created_by,
            created_at: Timestamp::from_millis(self.created_at)
                .map_err(|_| StoreError::Corrupt("dependencies.created_at"))?,
        })
    }
}

/// Inserts the edge `task` needs `needs`. The database refuses a self edge (`dependencies_no_self_edge`) and a second row for the same pair (a unique violation); it does not look for cycles. A task number that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_edge(
    tx: &mut WriteTx,
    task: TaskId,
    needs: TaskId,
    origin: DependencyOrigin,
    created_by: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let task = task.to_db();
    let needs = needs.to_db();
    let origin = origin.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO dependencies (task_id, needs_task_id, origin, created_by, created_at)
         SELECT t.id, n.id, ?3, ?4, ?5 FROM tasks t, tasks n WHERE t.number = ?1 AND n.number = ?2",
        task,
        needs,
        origin,
        created_by,
        now
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the task does not exist"))
    }
}

/// Deletes the edge and returns whether one existed.
pub async fn delete_edge(
    tx: &mut WriteTx,
    task: TaskId,
    needs: TaskId,
) -> Result<bool, StoreError> {
    let task = task.to_db();
    let needs = needs.to_db();
    let result = sqlx::query!(
        "DELETE FROM dependencies
         WHERE task_id = (SELECT id FROM tasks WHERE number = ?1)
           AND needs_task_id = (SELECT id FROM tasks WHERE number = ?2)",
        task,
        needs
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The edge `task` needs `needs`, read on the given connection so a command sees its own uncommitted rows.
pub async fn fetch_edge(
    conn: &mut SqliteConnection,
    task: TaskId,
    needs: TaskId,
) -> Result<Option<EdgeRow>, StoreError> {
    let task = task.to_db();
    let needs = needs.to_db();
    sqlx::query_as!(
        RawEdge,
        r#"SELECT t.number AS "task!: i64", n.number AS "needs!: i64", d.origin, d.created_by,
                  d.created_at
           FROM dependencies d
           JOIN tasks t ON t.id = d.task_id
           JOIN tasks n ON n.id = d.needs_task_id
           WHERE t.number = ?1 AND n.number = ?2"#,
        task,
        needs
    )
    .fetch_optional(conn)
    .await?
    .map(RawEdge::into_row)
    .transpose()
}

/// Every edge of the project, ordered by task number and then by the number of the task it needs.
pub async fn edges(pool: &SqlitePool) -> Result<Vec<EdgeRow>, StoreError> {
    let mut conn = pool.acquire().await?;
    edges_conn(&mut conn).await
}

/// The same read on the writer's connection, so a command checks the graph it is about to change.
pub async fn edges_conn(conn: &mut SqliteConnection) -> Result<Vec<EdgeRow>, StoreError> {
    sqlx::query_as!(
        RawEdge,
        r#"SELECT t.number AS "task!: i64", n.number AS "needs!: i64", d.origin, d.created_by,
                  d.created_at
           FROM dependencies d
           JOIN tasks t ON t.id = d.task_id
           JOIN tasks n ON n.id = d.needs_task_id
           ORDER BY t.number, n.number"#
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(RawEdge::into_row)
    .collect()
}

/// The tasks `task` needs directly, by either origin, with their cached status, in task number order.
pub async fn prerequisite_statuses(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Vec<(TaskId, TaskStatus)>, StoreError> {
    let number = task.to_db();
    let rows = sqlx::query!(
        r#"SELECT n.number AS "needs!: i64", n.status
           FROM dependencies d
           JOIN tasks t ON t.id = d.task_id
           JOIN tasks n ON n.id = d.needs_task_id
           WHERE t.number = ?
           ORDER BY n.number"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let needs =
                TaskId::try_from_db(row.needs).ok_or(StoreError::Corrupt("tasks.number"))?;
            let status = row
                .status
                .parse()
                .map_err(|_| StoreError::Corrupt("tasks.status"))?;
            Ok((needs, status))
        })
        .collect()
}

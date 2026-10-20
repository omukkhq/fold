use std::collections::BTreeMap;

use fold_core::{TaskId, TaskStatus, TaskTitle, Timestamp, WorkerName};
use sqlx::SqlitePool;

use super::{LabelRow, edges_conn, labels_for_conn, timestamp};
use crate::error::StoreError;

/// A Todo task that is not archived, has no unreleased implementation claim, is no human task and is not on hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyRow {
    pub id: TaskId,
    pub title: TaskTitle,
    pub created_at: Timestamp,
    pub labels: Vec<LabelRow>,
}

/// A Todo task that would be a [`ReadyRow`] but for its unreleased implementation claim, which `holder` holds. A lead reads these to find the tasks of a worker that has ended, whose claim a start moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTodoRow {
    pub id: TaskId,
    pub created_at: Timestamp,
    pub labels: Vec<LabelRow>,
    pub holder: WorkerName,
}

/// The inputs of the ready order, read in one transaction so the candidates, the statuses and the graph agree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadySnapshot {
    pub candidates: Vec<ReadyRow>,
    /// The stored status of every task that is not archived.
    pub statuses: BTreeMap<TaskId, TaskStatus>,
    /// Every edge as `(task, needs)`.
    pub edges: Vec<(TaskId, TaskId)>,
}

pub async fn ready_snapshot(pool: &SqlitePool) -> Result<ReadySnapshot, StoreError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", t.title, t.created_at
           FROM tasks t
           WHERE t.status = 'todo' AND t.archived_at IS NULL
             AND t.kind <> 'human' AND t.hold_reason IS NULL
             AND NOT EXISTS (
                 SELECT 1 FROM claims c
                 WHERE c.task_id = t.id AND c.kind = 'implementation' AND c.released_at IS NULL
             )
           ORDER BY t.number"#
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut candidates = Vec::with_capacity(rows.len());
    for row in rows {
        let id = TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?;
        candidates.push(ReadyRow {
            id,
            title: TaskTitle::parse(&row.title).map_err(|_| StoreError::Corrupt("tasks.title"))?,
            created_at: timestamp(row.created_at, "tasks.created_at")?,
            labels: labels_for_conn(&mut tx, id).await?,
        });
    }
    let statuses = sqlx::query!(
        r#"SELECT number AS "number!: i64", status FROM tasks WHERE archived_at IS NULL"#
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?,
            row.status
                .parse()
                .map_err(|_| StoreError::Corrupt("tasks.status"))?,
        ))
    })
    .collect::<Result<_, StoreError>>()?;
    let edges = edges_conn(&mut tx)
        .await?
        .into_iter()
        .map(|edge| (edge.task, edge.needs))
        .collect();
    tx.commit().await?;
    Ok(ReadySnapshot {
        candidates,
        statuses,
        edges,
    })
}

/// The Todo tasks that are not archived, are no human task and are not on hold, and that an unreleased implementation claim holds, in number order.
pub async fn held_todo(pool: &SqlitePool) -> Result<Vec<HeldTodoRow>, StoreError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", t.created_at, w.name AS holder
           FROM tasks t
           JOIN claims c ON c.task_id = t.id AND c.kind = 'implementation' AND c.released_at IS NULL
           JOIN workers w ON w.id = c.worker_id
           WHERE t.status = 'todo' AND t.archived_at IS NULL
             AND t.kind <> 'human' AND t.hold_reason IS NULL
           ORDER BY t.number"#
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut held = Vec::with_capacity(rows.len());
    for row in rows {
        let id = TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?;
        held.push(HeldTodoRow {
            id,
            created_at: timestamp(row.created_at, "tasks.created_at")?,
            labels: labels_for_conn(&mut tx, id).await?,
            holder: WorkerName::parse(&row.holder)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
        });
    }
    tx.commit().await?;
    Ok(held)
}

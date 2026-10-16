use std::collections::BTreeMap;

use fold_core::{TaskId, TaskStatus, TaskTitle, Timestamp, WorkerName, WorkspaceId};
use sqlx::{SqliteConnection, SqlitePool};

use super::{EdgeRow, LabelRow, RawTask, TaskRow, edges_conn, label_row, timestamp};
use crate::error::StoreError;
use crate::reviews::{ProposalSummary, proposal_summaries};

/// A task named from another task's view: an edge end or a linked checklist item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRef {
    pub id: TaskId,
    pub title: TaskTitle,
    pub status: TaskStatus,
}

/// Ticked and all items of one task, plain steps and linked items counted apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChecklistCounts {
    pub steps_done: u32,
    pub steps_total: u32,
    pub linked_done: u32,
    pub linked_total: u32,
}

/// Everything the board and the graph draw, read in one transaction so the tasks, edges and claims agree. Archived tasks are left out of every map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardSnapshot {
    /// In number order.
    pub tasks: Vec<TaskRow>,
    pub labels: BTreeMap<TaskId, Vec<LabelRow>>,
    pub checklists: BTreeMap<TaskId, ChecklistCounts>,
    pub edges: Vec<EdgeRow>,
    /// The worker holding the unreleased implementation claim.
    pub holders: BTreeMap<TaskId, WorkerName>,
    /// The live (`active` or `missing`) task workspace.
    pub workspaces: BTreeMap<TaskId, WorkspaceId>,
    /// How many messages name the task.
    pub messages: BTreeMap<TaskId, u32>,
    /// When the completion record was written.
    pub completions: BTreeMap<TaskId, Timestamp>,
    /// The last tracked edit in the live task workspace, for a workspace that has one.
    pub activity: BTreeMap<TaskId, Timestamp>,
    /// The open or integrated proposal of each task.
    pub proposals: BTreeMap<TaskId, ProposalSummary>,
    /// The note of each open report, on tasks without a proposal.
    pub reports: BTreeMap<TaskId, String>,
}

fn task_id(number: i64) -> Result<TaskId, StoreError> {
    TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number"))
}

fn count(value: i64, column: &'static str) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::Corrupt(column))
}

/// How many unarchived tasks hold each status; a status without tasks is absent.
pub async fn status_counts(pool: &SqlitePool) -> Result<Vec<(TaskStatus, u32)>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT status, COUNT(*) AS "count!: i64" FROM tasks WHERE archived_at IS NULL GROUP BY status"#
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let status = row
                .status
                .parse::<TaskStatus>()
                .map_err(|_| StoreError::Corrupt("tasks.status"))?;
            Ok((status, count(row.count, "tasks.status")?))
        })
        .collect()
}

pub async fn board_snapshot(pool: &SqlitePool) -> Result<BoardSnapshot, StoreError> {
    let mut tx = pool.begin().await?;
    let tasks = sqlx::query_as!(
        RawTask,
        r#"SELECT number, title, description, created_by, created_at, updated_at, archived_at,
                  human_review AS "human_review: bool", human_reviewer, human_review_reason,
                  human_review_set_by, status, blocked AS "blocked: bool", kind
           FROM tasks WHERE archived_at IS NULL ORDER BY number"#
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(RawTask::into_row)
    .collect::<Result<Vec<_>, _>>()?;

    let mut labels: BTreeMap<TaskId, Vec<LabelRow>> = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", l.name, l.color, l.created_at
           FROM task_labels tl
           JOIN tasks t ON t.id = tl.task_id
           JOIN labels l ON l.id = tl.label_id
           WHERE t.archived_at IS NULL
           ORDER BY t.number, l.name"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        labels
            .entry(task_id(row.number)?)
            .or_default()
            .push(label_row(&row.name, &row.color, row.created_at)?);
    }

    let mut checklists = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64",
                  SUM(i.text IS NOT NULL AND i.done_at IS NOT NULL) AS "steps_done!: i64",
                  SUM(i.text IS NOT NULL) AS "steps_total!: i64",
                  SUM(i.linked_task_id IS NOT NULL AND i.done_at IS NOT NULL) AS "linked_done!: i64",
                  SUM(i.linked_task_id IS NOT NULL) AS "linked_total!: i64"
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks t ON t.id = c.task_id
           WHERE t.archived_at IS NULL
           GROUP BY t.number"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        checklists.insert(
            task_id(row.number)?,
            ChecklistCounts {
                steps_done: count(row.steps_done, "checklist_items.done_at")?,
                steps_total: count(row.steps_total, "checklist_items.text")?,
                linked_done: count(row.linked_done, "checklist_items.done_at")?,
                linked_total: count(row.linked_total, "checklist_items.linked_task_id")?,
            },
        );
    }

    let edges = edges_conn(&mut tx).await?;

    let mut holders = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", w.name AS worker
           FROM claims c
           JOIN tasks t ON t.id = c.task_id
           JOIN workers w ON w.id = c.worker_id
           WHERE c.kind = 'implementation' AND c.released_at IS NULL AND t.archived_at IS NULL"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        let worker =
            WorkerName::parse(&row.worker).map_err(|_| StoreError::Corrupt("workers.name"))?;
        holders.insert(task_id(row.number)?, worker);
    }

    let mut workspaces = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", w.number AS "workspace!: i64"
           FROM workspaces w
           JOIN tasks t ON t.id = w.task_id
           WHERE w.kind = 'task' AND w.state IN ('active', 'missing') AND t.archived_at IS NULL"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        let workspace = WorkspaceId::try_from_db(row.workspace)
            .ok_or(StoreError::Corrupt("workspaces.number"))?;
        workspaces.insert(task_id(row.task)?, workspace);
    }

    let mut messages = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", COUNT(*) AS "count!: i64"
           FROM messages m
           JOIN tasks t ON t.id = m.task_id
           WHERE t.archived_at IS NULL
           GROUP BY t.number"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        messages.insert(task_id(row.number)?, count(row.count, "messages.task_id")?);
    }

    let mut completions = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", c.created_at
           FROM completions c
           JOIN tasks t ON t.id = c.task_id
           WHERE t.archived_at IS NULL"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        completions.insert(
            task_id(row.number)?,
            timestamp(row.created_at, "completions.created_at")?,
        );
    }

    let mut activity = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", w.last_activity_at AS "at!: i64"
           FROM workspaces w
           JOIN tasks t ON t.id = w.task_id
           WHERE w.kind = 'task' AND w.state IN ('active', 'missing') AND t.archived_at IS NULL
             AND w.last_activity_at IS NOT NULL"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        activity.insert(
            task_id(row.task)?,
            timestamp(row.at, "workspaces.last_activity_at")?,
        );
    }

    let proposals = proposal_summaries(&mut tx, None)
        .await?
        .into_iter()
        .map(|summary| (summary.task, summary))
        .collect();

    let mut reports = BTreeMap::new();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", r.note
           FROM task_reports r
           JOIN tasks t ON t.id = r.task_id
           WHERE r.resolution IS NULL AND t.archived_at IS NULL"#
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        reports.insert(task_id(row.task)?, row.note);
    }

    tx.commit().await?;
    Ok(BoardSnapshot {
        tasks,
        labels,
        checklists,
        edges,
        holders,
        workspaces,
        messages,
        completions,
        activity,
        proposals,
        reports,
    })
}

/// The title and status of each task in `ids` that exists, in number order.
pub async fn task_refs(
    conn: &mut SqliteConnection,
    ids: &[TaskId],
) -> Result<Vec<TaskRef>, StoreError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let numbers =
        serde_json::Value::from(ids.iter().map(|id| id.to_db()).collect::<Vec<_>>()).to_string();
    sqlx::query!(
        r#"SELECT number AS "number!: i64", title, status FROM tasks
           WHERE number IN (SELECT value FROM json_each(?1))
           ORDER BY number"#,
        numbers
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        Ok(TaskRef {
            id: task_id(row.number)?,
            title: TaskTitle::parse(&row.title).map_err(|_| StoreError::Corrupt("tasks.title"))?,
            status: row
                .status
                .parse()
                .map_err(|_| StoreError::Corrupt("tasks.status"))?,
        })
    })
    .collect()
}

/// The number of the live (`active` or `missing`) task workspace of `task`.
pub async fn live_workspace(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Option<WorkspaceId>, StoreError> {
    let number = task.to_db();
    sqlx::query_scalar!(
        r#"SELECT w.number AS "number!: i64" FROM workspaces w
           JOIN tasks t ON t.id = w.task_id
           WHERE t.number = ? AND w.kind = 'task' AND w.state IN ('active', 'missing')"#,
        number
    )
    .fetch_optional(conn)
    .await?
    .map(|number| WorkspaceId::try_from_db(number).ok_or(StoreError::Corrupt("workspaces.number")))
    .transpose()
}

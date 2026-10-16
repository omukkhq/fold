use fold_core::{ProposalId, TaskId, TaskStatus, Timestamp};
use sqlx::SqliteExecutor;

use super::time;
use crate::error::StoreError;

/// A stored hand-off note. The body is content: it goes to the worker's next session and to `list_workers`, never into a log or an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRow {
    pub session_id: String,
    pub body: String,
    pub created_at: Timestamp,
}

/// One open task or proposal of the fold summary a refreshed lead reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTaskRow {
    pub id: TaskId,
    pub status: TaskStatus,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenProposalRow {
    pub id: ProposalId,
    pub task: TaskId,
    pub title: String,
}

fn count(value: i64, column: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt(column))
}

pub async fn insert_handoff(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    session_id: &str,
    body: &str,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let now = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO handoffs (agent_id, session_id, body, created_at) VALUES (?, ?, ?, ?)
           RETURNING id AS "id!: i64""#,
        agent_id,
        session_id,
        body,
        now
    )
    .fetch_one(executor)
    .await?)
}

/// The newest note of the agent, from any of its sessions.
pub async fn latest_handoff(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Option<HandoffRow>, StoreError> {
    let row = sqlx::query!(
        "SELECT session_id, body, created_at FROM handoffs WHERE agent_id = ? ORDER BY id DESC LIMIT 1",
        agent_id
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(HandoffRow {
            session_id: row.session_id,
            body: row.body,
            created_at: time(row.created_at, "handoffs.created_at")?,
        })
    })
    .transpose()
}

/// The newest note that session `session_id` wrote.
pub async fn session_handoff(
    executor: impl SqliteExecutor<'_>,
    session_id: &str,
) -> Result<Option<HandoffRow>, StoreError> {
    let row = sqlx::query!(
        "SELECT session_id, body, created_at FROM handoffs WHERE session_id = ? ORDER BY id DESC LIMIT 1",
        session_id
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(HandoffRow {
            session_id: row.session_id,
            body: row.body,
            created_at: time(row.created_at, "handoffs.created_at")?,
        })
    })
    .transpose()
}

/// The row's `turns` when fold asked its current session for a note, or `None` when it has not asked.
pub async fn handoff_asked(
    executor: impl SqliteExecutor<'_>,
    worker_process_id: i64,
) -> Result<Option<u64>, StoreError> {
    let turns = sqlx::query_scalar!(
        "SELECT handoff_asked_turns FROM worker_processes WHERE id = ?",
        worker_process_id
    )
    .fetch_optional(executor)
    .await?
    .flatten();
    turns
        .map(|turns| count(turns, "worker_processes.handoff_asked_turns"))
        .transpose()
}

/// Records that fold asked the row's session for a note after its `turns`-th turn. A row that was asked already keeps its first ask; returns whether this call asked.
pub async fn set_handoff_asked(
    executor: impl SqliteExecutor<'_>,
    worker_process_id: i64,
    turns: u64,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let turns = i64::try_from(turns).unwrap_or(i64::MAX);
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE worker_processes SET handoff_asked_at = ?2, handoff_asked_turns = ?3
         WHERE id = ?1 AND handoff_asked_at IS NULL",
        worker_process_id,
        now,
        turns
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Starts the row again in the new session `session_id` with an empty context and no ask, for a fresh process of the same worker.
pub async fn refresh_session(
    executor: impl SqliteExecutor<'_>,
    worker_process_id: i64,
    session_id: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes
         SET session_id = ?2, state = 'starting', started_at = ?3, stopped_at = NULL, idle_since = NULL,
             pid = NULL, exit_code = NULL, context_tokens = 0, handoff_asked_at = NULL,
             handoff_asked_turns = NULL
         WHERE id = ?1",
        worker_process_id,
        session_id,
        now
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// The first `limit` tasks that are neither done nor archived, in number order, with how many there are in all; with `label_id`, only the tasks with that label.
pub async fn open_area_tasks(
    conn: &mut sqlx::SqliteConnection,
    label_id: Option<i64>,
    limit: u64,
) -> Result<(Vec<OpenTaskRow>, u64), StoreError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let total = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM tasks t
           WHERE t.status <> 'done' AND t.archived_at IS NULL
             AND (?1 IS NULL OR EXISTS (SELECT 1 FROM task_labels l WHERE l.task_id = t.id AND l.label_id = ?1))"#,
        label_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64", t.status, t.title FROM tasks t
           WHERE t.status <> 'done' AND t.archived_at IS NULL
             AND (?1 IS NULL OR EXISTS (SELECT 1 FROM task_labels l WHERE l.task_id = t.id AND l.label_id = ?1))
           ORDER BY t.number LIMIT ?2"#,
        label_id,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let tasks = rows
        .into_iter()
        .map(|row| {
            Ok(OpenTaskRow {
                id: TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?,
                status: row
                    .status
                    .parse()
                    .map_err(|_| StoreError::Corrupt("tasks.status"))?,
                title: row.title,
            })
        })
        .collect::<Result<_, StoreError>>()?;
    Ok((tasks, count(total, "tasks")?))
}

/// The first `limit` open proposals in number order, with how many there are in all; with `label_id`, only the proposals of tasks with that label.
pub async fn open_area_proposals(
    conn: &mut sqlx::SqliteConnection,
    label_id: Option<i64>,
    limit: u64,
) -> Result<(Vec<OpenProposalRow>, u64), StoreError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let total = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM proposals p
           WHERE p.state = 'open'
             AND (?1 IS NULL OR EXISTS (SELECT 1 FROM task_labels l WHERE l.task_id = p.task_id AND l.label_id = ?1))"#,
        label_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        r#"SELECT p.number AS "number!: i64", t.number AS "task!: i64", p.title FROM proposals p
           JOIN tasks t ON t.id = p.task_id
           WHERE p.state = 'open'
             AND (?1 IS NULL OR EXISTS (SELECT 1 FROM task_labels l WHERE l.task_id = p.task_id AND l.label_id = ?1))
           ORDER BY p.number LIMIT ?2"#,
        label_id,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let proposals = rows
        .into_iter()
        .map(|row| {
            Ok(OpenProposalRow {
                id: ProposalId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                title: row.title,
            })
        })
        .collect::<Result<_, StoreError>>()?;
    Ok((proposals, count(total, "proposals")?))
}

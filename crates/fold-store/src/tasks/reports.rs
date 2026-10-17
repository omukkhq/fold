use fold_core::{AgentName, TaskId, Timestamp, WorkerName, WorkerState};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::columns::time;
use crate::error::StoreError;
use crate::store::WriteTx;

/// How a lead check answered a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Completed,
    SentBack,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Completed => "completed",
            Resolution::SentBack => "sent_back",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "completed" => Ok(Resolution::Completed),
            "sent_back" => Ok(Resolution::SentBack),
            _ => Err(StoreError::Corrupt("task_reports.resolution")),
        }
    }
}

/// A task worker's report on a task without a proposal, with the lead check that answered it once there is one. `worker_id` is a row id and stays inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRow {
    pub task: TaskId,
    pub worker_id: i64,
    pub worker: WorkerName,
    pub note: String,
    pub created_at: Timestamp,
    pub resolution: Option<Resolution>,
    /// A worker name or `human:<name>`.
    pub resolved_by: Option<String>,
    pub resolved_at: Option<Timestamp>,
    pub send_back_note: Option<String>,
}

impl ReportRow {
    pub fn is_open(&self) -> bool {
        self.resolution.is_none()
    }
}

struct RawReport {
    task: i64,
    worker_id: i64,
    worker: String,
    note: String,
    created_at: i64,
    resolution: Option<String>,
    resolved_by: Option<String>,
    resolved_at: Option<i64>,
    send_back_note: Option<String>,
}

impl RawReport {
    fn into_row(self) -> Result<ReportRow, StoreError> {
        Ok(ReportRow {
            task: TaskId::try_from_db(self.task).ok_or(StoreError::Corrupt("tasks.number"))?,
            worker_id: self.worker_id,
            worker: WorkerName::parse(&self.worker)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
            note: self.note,
            created_at: time(self.created_at, "task_reports.created_at")?,
            resolution: self
                .resolution
                .as_deref()
                .map(Resolution::parse)
                .transpose()?,
            resolved_by: self.resolved_by,
            resolved_at: self
                .resolved_at
                .map(|millis| time(millis, "task_reports.resolved_at"))
                .transpose()?,
            send_back_note: self.send_back_note,
        })
    }
}

/// Opens a report on `task` by `worker_id`. A second open report fails on `task_reports.task_id` (a unique violation), and a task with a proposal is refused by `task_reports_open_on_no_proposal_task`. A task number that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert(
    tx: &mut WriteTx,
    task: TaskId,
    worker_id: i64,
    note: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO task_reports (task_id, worker_id, note, created_at)
         SELECT t.id, ?2, ?3, ?4 FROM tasks t WHERE t.number = ?1",
        number,
        worker_id,
        note,
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

/// Answers the open report on `task`. `send_back_note` goes with `SentBack` only; the table refuses any other pairing. Without an open report it changes nothing and fails with `StoreError::Invalid`.
pub async fn resolve(
    tx: &mut WriteTx,
    task: TaskId,
    resolution: Resolution,
    by: &str,
    send_back_note: Option<&str>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let resolution = resolution.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE task_reports SET resolution = ?2, resolved_by = ?3, resolved_at = ?4, send_back_note = ?5
         WHERE resolution IS NULL AND task_id = (SELECT id FROM tasks WHERE number = ?1)",
        number,
        resolution,
        by,
        now,
        send_back_note
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the task has no open report"))
    }
}

/// The reports on `task`, newest first, at most `limit`.
pub async fn for_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
    limit: u32,
) -> Result<Vec<ReportRow>, StoreError> {
    let number = task.to_db();
    let limit = i64::from(limit);
    sqlx::query_as!(
        RawReport,
        r#"SELECT t.number AS "task!: i64", r.worker_id, w.name AS worker, r.note, r.created_at,
                  r.resolution, r.resolved_by, r.resolved_at, r.send_back_note
           FROM task_reports r
           JOIN tasks t ON t.id = r.task_id
           JOIN workers w ON w.id = r.worker_id
           WHERE t.number = ?1
           ORDER BY r.id DESC
           LIMIT ?2"#,
        number,
        limit
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(RawReport::into_row)
    .collect()
}

/// The open report on `task`, read on the caller's connection.
pub async fn open_for_task(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Option<ReportRow>, StoreError> {
    let number = task.to_db();
    sqlx::query_as!(
        RawReport,
        r#"SELECT t.number AS "task!: i64", r.worker_id, w.name AS worker, r.note, r.created_at,
                  r.resolution, r.resolved_by, r.resolved_at, r.send_back_note
           FROM task_reports r
           JOIN tasks t ON t.id = r.task_id
           JOIN workers w ON w.id = r.worker_id
           WHERE t.number = ?1 AND r.resolution IS NULL"#,
        number
    )
    .fetch_optional(conn)
    .await?
    .map(RawReport::into_row)
    .transpose()
}

/// The newest report on `task`, open or answered.
pub async fn latest_for_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<ReportRow>, StoreError> {
    Ok(for_task(executor, task, 1).await?.pop())
}

/// A send-back that waits for the task worker who wrote the report: the task worker's process is in one of the asked states, such as stopped or failed, and has not started since the task was sent back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedSendBack {
    pub agent: AgentName,
    pub task: TaskId,
}

/// The send-backs no start of their task worker has served yet, such as one the worker limits refused: each start moves `started_at`, so a send-back newer than it is still owed. Only the newest report of a task counts, so a task reported again owes nothing.
pub async fn owed_send_backs(
    executor: impl SqliteExecutor<'_>,
    states: &[WorkerState],
) -> Result<Vec<OwedSendBack>, StoreError> {
    let states = serde_json::Value::from(
        states
            .iter()
            .map(|state| state.as_str())
            .collect::<Vec<_>>(),
    )
    .to_string();
    let rows = sqlx::query!(
        r#"SELECT g.name AS "agent!: String", t.number AS "task!: i64"
           FROM task_reports r
           JOIN tasks t ON t.id = r.task_id
           JOIN workers k ON k.id = r.worker_id
           JOIN worker_processes w ON w.agent_id = k.agent_id
           JOIN agents g ON g.id = w.agent_id
           WHERE r.resolution = 'sent_back' AND r.resolved_at > w.started_at
             AND w.role = 'task_worker' AND w.state IN (SELECT value FROM json_each(?1))
             AND t.archived_at IS NULL
             AND r.id = (SELECT MAX(x.id) FROM task_reports x WHERE x.task_id = r.task_id)
           ORDER BY w.id, t.number"#,
        states
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OwedSendBack {
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
            })
        })
        .collect()
}

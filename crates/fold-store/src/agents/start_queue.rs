use std::collections::{BTreeMap, HashMap};

use fold_core::roles::Role;
use fold_core::{AreaName, ProposalId, ReviewPlanId, TaskId, Timestamp};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::columns::{optional_time, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// Why a queued start ended without a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropReason {
    /// Another worker claimed one of its tasks.
    TaskClaimed,
    /// One of its tasks became Done.
    TaskDone,
    /// One of its tasks was archived.
    TaskArchived,
    /// One of its proposals, or its review plan, ended.
    ProposalEnded,
    /// The starter or a human took the start back.
    Cancelled,
}

impl DropReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskClaimed => "task_claimed",
            Self::TaskDone => "task_done",
            Self::TaskArchived => "task_archived",
            Self::ProposalEnded => "proposal_ended",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "task_claimed" => Ok(Self::TaskClaimed),
            "task_done" => Ok(Self::TaskDone),
            "task_archived" => Ok(Self::TaskArchived),
            "proposal_ended" => Ok(Self::ProposalEnded),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(StoreError::Corrupt("worker_start_queue.drop_reason")),
        }
    }
}

/// How a queued start ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEnd {
    Started {
        worker_process_id: i64,
        at: Timestamp,
    },
    Dropped {
        reason: DropReason,
        at: Timestamp,
    },
}

/// A start to put in the queue: what the starter gave `start_worker`. `area_id` is set exactly for a task worker.
#[derive(Debug, Clone, Copy)]
pub struct NewQueuedStart<'a> {
    pub role: Role,
    pub area_id: Option<i64>,
    pub name: Option<&'a str>,
    pub tasks: &'a [TaskId],
    pub proposals: &'a [ProposalId],
    pub review_plan: Option<ReviewPlanId>,
    pub model: Option<&'a str>,
    pub instructions: Option<&'a str>,
    pub queued_by: &'a str,
    pub now: Timestamp,
}

/// A queued start with its tasks and proposals in the order the starter gave them. `id` stays inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedStartRow {
    pub id: i64,
    pub role: Role,
    pub area_id: Option<i64>,
    pub area: Option<AreaName>,
    pub name: Option<String>,
    pub tasks: Vec<TaskId>,
    pub proposals: Vec<ProposalId>,
    pub review_plan: Option<ReviewPlanId>,
    pub model: Option<String>,
    pub instructions: Option<String>,
    pub queued_by: String,
    pub queued_at: Timestamp,
    pub end: Option<QueueEnd>,
}

/// Puts a start in the queue and returns its row id. A role without a worker limit, or an unknown task, proposal or review plan, is refused with `Invalid` and writes nothing that stays, since the caller's transaction rolls back.
pub async fn insert_queued_start(
    tx: &mut WriteTx,
    new: &NewQueuedStart<'_>,
) -> Result<i64, StoreError> {
    if !matches!(new.role, Role::TaskWorker | Role::Reviewer) {
        return Err(StoreError::Invalid(
            "only a task worker or a reviewer start can wait in the queue",
        ));
    }
    let review_plan_id = match new.review_plan {
        None => None,
        Some(plan) => {
            let number = plan.to_db();
            Some(
                sqlx::query_scalar!(
                    r#"SELECT id AS "id!: i64" FROM review_plans WHERE number = ?"#,
                    number
                )
                .fetch_optional(tx.conn())
                .await?
                .ok_or(StoreError::Invalid("the review plan does not exist"))?,
            )
        }
    };
    let role = new.role.as_str();
    let now = new.now.as_millis();
    let id = sqlx::query_scalar!(
        r#"INSERT INTO worker_start_queue (role, area_id, name, review_plan_id, model, instructions, queued_by, queued_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id AS "id!: i64""#,
        role,
        new.area_id,
        new.name,
        review_plan_id,
        new.model,
        new.instructions,
        new.queued_by,
        now
    )
    .fetch_one(tx.conn())
    .await?;
    for (position, task) in (0_i64..).zip(new.tasks) {
        let number = task.to_db();
        let inserted = sqlx::query!(
            "INSERT INTO worker_start_queue_tasks (queued_start_id, position, task_id)
             SELECT ?, ?, id FROM tasks WHERE number = ?",
            id,
            position,
            number
        )
        .execute(tx.conn())
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(StoreError::Invalid("a task of the start does not exist"));
        }
    }
    for (position, proposal) in (0_i64..).zip(new.proposals) {
        let number = proposal.to_db();
        let inserted = sqlx::query!(
            "INSERT INTO worker_start_queue_proposals (queued_start_id, position, proposal_id)
             SELECT ?, ?, id FROM proposals WHERE number = ?",
            id,
            position,
            number
        )
        .execute(tx.conn())
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(StoreError::Invalid(
                "a proposal of the start does not exist",
            ));
        }
    }
    Ok(id)
}

/// Ends a waiting start with the worker it started. Returns false and changes nothing when the start has ended already or does not exist.
pub async fn mark_queued_start_started(
    tx: &mut WriteTx,
    id: i64,
    worker_process_id: i64,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE worker_start_queue SET ended_at = ?, outcome = 'started', worker_process_id = ?
         WHERE id = ? AND ended_at IS NULL",
        now,
        worker_process_id,
        id
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Ends a waiting start without a worker. Returns false and changes nothing when the start has ended already or does not exist.
pub async fn drop_queued_start(
    tx: &mut WriteTx,
    id: i64,
    reason: DropReason,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let reason = reason.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE worker_start_queue SET ended_at = ?, outcome = 'dropped', drop_reason = ?
         WHERE id = ? AND ended_at IS NULL",
        now,
        reason,
        id
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The starts that still wait, oldest first.
pub async fn waiting_starts(
    conn: &mut SqliteConnection,
) -> Result<Vec<QueuedStartRow>, StoreError> {
    read_starts(conn, None).await
}

/// One queued start, waiting or ended; `None` when it does not exist.
pub async fn queued_start(
    conn: &mut SqliteConnection,
    id: i64,
) -> Result<Option<QueuedStartRow>, StoreError> {
    Ok(read_starts(conn, Some(id)).await?.into_iter().next())
}

/// For each area, when a queued start of it last started a worker; the key `None` holds the starts outside an area. It is the rotation input of [`fold_core::start_queue::queue_order`].
pub async fn last_queued_starts_by_area(
    executor: impl SqliteExecutor<'_>,
) -> Result<BTreeMap<Option<AreaName>, Timestamp>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT a.name AS "area?: String", MAX(q.ended_at) AS "ended_at!: i64"
           FROM worker_start_queue q
           LEFT JOIN areas a ON a.id = q.area_id
           WHERE q.outcome = 'started'
           GROUP BY q.area_id"#
    )
    .fetch_all(executor)
    .await?;
    let mut last = BTreeMap::new();
    for row in rows {
        last.insert(
            parse_area(row.area)?,
            time(row.ended_at, "worker_start_queue.ended_at")?,
        );
    }
    Ok(last)
}

/// The waiting starts when `only` is `None`, else the start with that id.
async fn read_starts(
    conn: &mut SqliteConnection,
    only: Option<i64>,
) -> Result<Vec<QueuedStartRow>, StoreError> {
    let raw = sqlx::query!(
        r#"SELECT q.id AS "id!: i64", q.role, q.area_id, a.name AS "area?: String", q.name,
                  p.number AS "plan_number?: i64", q.model, q.instructions, q.queued_by, q.queued_at,
                  q.ended_at, q.outcome, q.drop_reason, q.worker_process_id
           FROM worker_start_queue q
           LEFT JOIN areas a ON a.id = q.area_id
           LEFT JOIN review_plans p ON p.id = q.review_plan_id
           WHERE (?1 IS NULL AND q.ended_at IS NULL) OR q.id = ?1
           ORDER BY q.queued_at, q.id"#,
        only
    )
    .fetch_all(&mut *conn)
    .await?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut tasks: HashMap<i64, Vec<TaskId>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT s.queued_start_id AS "start!: i64", t.number AS "number!: i64"
           FROM worker_start_queue_tasks s
           JOIN worker_start_queue q ON q.id = s.queued_start_id
           JOIN tasks t ON t.id = s.task_id
           WHERE (?1 IS NULL AND q.ended_at IS NULL) OR q.id = ?1
           ORDER BY s.queued_start_id, s.position"#,
        only
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let task = TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?;
        tasks.entry(row.start).or_default().push(task);
    }
    let mut proposals: HashMap<i64, Vec<ProposalId>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT s.queued_start_id AS "start!: i64", p.number AS "number!: i64"
           FROM worker_start_queue_proposals s
           JOIN worker_start_queue q ON q.id = s.queued_start_id
           JOIN proposals p ON p.id = s.proposal_id
           WHERE (?1 IS NULL AND q.ended_at IS NULL) OR q.id = ?1
           ORDER BY s.queued_start_id, s.position"#,
        only
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let proposal =
            ProposalId::try_from_db(row.number).ok_or(StoreError::Corrupt("proposals.number"))?;
        proposals.entry(row.start).or_default().push(proposal);
    }
    raw.into_iter()
        .map(|row| {
            let ended_at = optional_time(row.ended_at, "worker_start_queue.ended_at")?;
            let end = match (row.outcome.as_deref(), ended_at) {
                (None, None) => None,
                (Some("started"), Some(at)) => Some(QueueEnd::Started {
                    worker_process_id: row
                        .worker_process_id
                        .ok_or(StoreError::Corrupt("worker_start_queue.worker_process_id"))?,
                    at,
                }),
                (Some("dropped"), Some(at)) => Some(QueueEnd::Dropped {
                    reason: DropReason::parse(
                        row.drop_reason
                            .as_deref()
                            .ok_or(StoreError::Corrupt("worker_start_queue.drop_reason"))?,
                    )?,
                    at,
                }),
                _ => return Err(StoreError::Corrupt("worker_start_queue.outcome")),
            };
            let review_plan = row
                .plan_number
                .map(|number| {
                    ReviewPlanId::try_from_db(number)
                        .ok_or(StoreError::Corrupt("review_plans.number"))
                })
                .transpose()?;
            Ok(QueuedStartRow {
                id: row.id,
                role: Role::parse(&row.role)
                    .ok_or(StoreError::Corrupt("worker_start_queue.role"))?,
                area_id: row.area_id,
                area: parse_area(row.area)?,
                name: row.name,
                tasks: tasks.remove(&row.id).unwrap_or_default(),
                proposals: proposals.remove(&row.id).unwrap_or_default(),
                review_plan,
                model: row.model,
                instructions: row.instructions,
                queued_by: row.queued_by,
                queued_at: time(row.queued_at, "worker_start_queue.queued_at")?,
                end,
            })
        })
        .collect()
}

fn parse_area(name: Option<String>) -> Result<Option<AreaName>, StoreError> {
    name.map(|name| AreaName::parse(&name).map_err(|_| StoreError::Corrupt("areas.name")))
        .transpose()
}

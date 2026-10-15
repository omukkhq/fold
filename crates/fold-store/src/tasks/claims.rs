use std::collections::BTreeSet;

use fold_core::{ProposalId, TaskId, Timestamp, WorkerName};
use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;

/// Why a claim ended, as stored in `claims.release_reason`. Every command that ends a claim passes its reason to `release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    Released,
    TaskArchived,
    TakenOver,
    /// The task's proposal integrated, which completes the task.
    Integrated,
}

impl ReleaseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ReleaseReason::Released => "released",
            ReleaseReason::TaskArchived => "task_archived",
            ReleaseReason::TakenOver => "taken_over",
            ReleaseReason::Integrated => "integrated",
        }
    }
}

/// An unreleased implementation claim. `worker_id` is a row id and stays inside the daemon; the public handle of the holder is `worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRow {
    pub task: TaskId,
    pub worker_id: i64,
    pub worker: WorkerName,
    pub acquired_at: Timestamp,
    pub renewed_at: Timestamp,
    pub stale_at: Option<Timestamp>,
}

fn time(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}

/// Inserts an implementation claim on `task` for `worker_id`, acquired and renewed at `now`. An unreleased claim on the task fails on `claims.task_id`, so the unique index decides between two workers. A task number that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_implementation(
    tx: &mut WriteTx,
    task: TaskId,
    worker_id: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at)
         SELECT 'implementation', t.id, ?2, ?3, ?3 FROM tasks t WHERE t.number = ?1",
        number,
        worker_id,
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

/// The unreleased implementation claim on `task` with its holder's name, or `None`.
pub async fn unreleased_for_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<ClaimRow>, StoreError> {
    let number = task.to_db();
    let row = sqlx::query!(
        r#"SELECT c.worker_id, w.name AS worker, c.acquired_at, c.renewed_at, c.stale_at
           FROM claims c
           JOIN tasks t ON t.id = c.task_id
           JOIN workers w ON w.id = c.worker_id
           WHERE t.number = ? AND c.kind = 'implementation' AND c.released_at IS NULL"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(ClaimRow {
            task,
            worker_id: row.worker_id,
            worker: WorkerName::parse(&row.worker)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
            acquired_at: time(row.acquired_at, "claims.acquired_at")?,
            renewed_at: time(row.renewed_at, "claims.renewed_at")?,
            stale_at: row
                .stale_at
                .map(|millis| time(millis, "claims.stale_at"))
                .transpose()?,
        })
    })
    .transpose()
}

/// Sets `renewed_at = now` and clears `stale_at` on every unreleased claim, of either kind, held by one of `worker_ids`. Returns how many claims it renewed.
pub async fn renew(
    tx: &mut WriteTx,
    worker_ids: &[i64],
    now: Timestamp,
) -> Result<u64, StoreError> {
    if worker_ids.is_empty() {
        return Ok(0);
    }
    let ids = serde_json::Value::from(worker_ids.to_vec()).to_string();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE claims SET renewed_at = ?2, stale_at = NULL
         WHERE released_at IS NULL AND worker_id IN (SELECT value FROM json_each(?1))",
        ids,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// Releases the unreleased implementation claim that `worker_id` holds on `task`. When that worker holds none there it changes nothing and fails with `StoreError::Invalid`.
pub async fn release(
    tx: &mut WriteTx,
    task: TaskId,
    worker_id: i64,
    reason: ReleaseReason,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let reason = reason.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE claims SET released_at = ?4, release_reason = ?3
         WHERE kind = 'implementation' AND released_at IS NULL AND worker_id = ?2
           AND task_id = (SELECT id FROM tasks WHERE number = ?1)",
        number,
        worker_id,
        reason,
        now
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the worker holds no claim on the task"))
    }
}

/// What a claim holds: a task for an implementation claim, a proposal of a task for a review claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimTarget {
    Task(TaskId),
    Review { proposal: ProposalId, task: TaskId },
}

/// An unreleased claim that [`mark_stale`] marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleClaim {
    pub target: ClaimTarget,
    pub worker: WorkerName,
    pub renewed_at: Timestamp,
}

/// Sets `stale_at = now` on each unreleased claim, of either kind, that has no `stale_at` yet and whose `renewed_at` passes `is_stale`, and returns them in the order they were taken. A marked claim is passed over until `renew` clears its `stale_at`, so each lapse is marked once.
pub async fn mark_stale(
    tx: &mut WriteTx,
    now: Timestamp,
    is_stale: impl Fn(Timestamp) -> bool,
) -> Result<Vec<StaleClaim>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", c.kind, t.number AS "task!: i64",
                  p.number AS "proposal?: i64", w.name AS worker, c.renewed_at
           FROM claims c
           JOIN workers w ON w.id = c.worker_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = COALESCE(c.task_id, p.task_id)
           WHERE c.released_at IS NULL AND c.stale_at IS NULL
           ORDER BY c.id"#
    )
    .fetch_all(tx.conn())
    .await?;
    let stamp = now.as_millis();
    let mut marked = Vec::new();
    for row in rows {
        let renewed_at = time(row.renewed_at, "claims.renewed_at")?;
        if !is_stale(renewed_at) {
            continue;
        }
        let task = TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?;
        let target = match (row.kind.as_str(), row.proposal) {
            ("implementation", None) => ClaimTarget::Task(task),
            ("review", Some(number)) => ClaimTarget::Review {
                proposal: ProposalId::try_from_db(number)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task,
            },
            _ => return Err(StoreError::Corrupt("claims.kind")),
        };
        sqlx::query!(
            "UPDATE claims SET stale_at = ?2 WHERE id = ?1 AND stale_at IS NULL AND released_at IS NULL",
            row.id,
            stamp
        )
        .execute(tx.conn())
        .await?;
        marked.push(StaleClaim {
            target,
            worker: WorkerName::parse(&row.worker)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
            renewed_at,
        });
    }
    Ok(marked)
}

/// The tasks with a marked claim: an unreleased implementation claim with `stale_at` set, or such a review claim on the task's open proposal.
pub async fn tasks_with_stale_claims(
    executor: impl SqliteExecutor<'_>,
) -> Result<BTreeSet<TaskId>, StoreError> {
    let rows = sqlx::query_scalar!(
        r#"SELECT t.number AS "task!: i64"
           FROM claims c JOIN tasks t ON t.id = c.task_id
           WHERE c.kind = 'implementation' AND c.released_at IS NULL AND c.stale_at IS NOT NULL
           UNION
           SELECT t.number
           FROM claims c
           JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = p.task_id
           WHERE c.kind = 'review' AND c.released_at IS NULL AND c.stale_at IS NOT NULL
             AND p.state = 'open'"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|number| TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number")))
        .collect()
}

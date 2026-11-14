use std::collections::BTreeSet;

use fold_core::claims::ClaimWait;
use fold_core::{CheckRunId, ProposalId, TaskId, Timestamp, WorkerName};
use sqlx::SqliteExecutor;

use crate::columns::time;
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
    /// A lead check completed the task without a proposal.
    Completed,
    /// The task was deleted with its claim rows, so only the `claim.released` event carries this reason.
    TaskDeleted,
    /// The worker that its lead gave the claim failed before its first turn.
    WorkerFailed,
}

impl ReleaseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ReleaseReason::Released => "released",
            ReleaseReason::TaskArchived => "task_archived",
            ReleaseReason::TakenOver => "taken_over",
            ReleaseReason::Integrated => "integrated",
            ReleaseReason::Completed => "completed",
            ReleaseReason::TaskDeleted => "task_deleted",
            ReleaseReason::WorkerFailed => "worker_failed",
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

/// A claim whose stale mark a renewal cleared: its task, the proposal's task for a review claim, and its holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewedClaim {
    pub task: TaskId,
    pub worker: WorkerName,
}

/// What a renewal did: how many claims it renewed, and those of them whose stale mark it cleared, which a `claim.renewed` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Renewal {
    pub count: u64,
    pub cleared: Vec<RenewedClaim>,
}

/// The unreleased claims with a stale mark that a renewal of `worker_ids` (a JSON array), or of every worker for `None`, clears, in claim order.
async fn marked(
    tx: &mut WriteTx,
    worker_ids: Option<&str>,
) -> Result<Vec<RenewedClaim>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", w.name AS worker
           FROM claims c
           JOIN workers w ON w.id = c.worker_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = COALESCE(c.task_id, p.task_id)
           WHERE c.released_at IS NULL
             AND (c.stale_at IS NOT NULL OR c.stale_needs_person_at IS NOT NULL)
             AND (?1 IS NULL OR c.worker_id IN (SELECT value FROM json_each(?1)))
           ORDER BY c.id"#,
        worker_ids
    )
    .fetch_all(tx.conn())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RenewedClaim {
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                worker: WorkerName::parse(&row.worker)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
            })
        })
        .collect()
}

/// Sets `renewed_at = now` and clears `stale_at` and `stale_needs_person_at` on every unreleased claim, of either kind, held by one of `worker_ids`.
pub async fn renew(
    tx: &mut WriteTx,
    worker_ids: &[i64],
    now: Timestamp,
) -> Result<Renewal, StoreError> {
    if worker_ids.is_empty() {
        return Ok(Renewal::default());
    }
    let ids = serde_json::Value::from(worker_ids.to_vec()).to_string();
    let cleared = marked(tx, Some(&ids)).await?;
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE claims SET renewed_at = ?2, stale_at = NULL, stale_needs_person_at = NULL
         WHERE released_at IS NULL AND worker_id IN (SELECT value FROM json_each(?1))",
        ids,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(Renewal {
        count: result.rows_affected(),
        cleared,
    })
}

/// [`renew`] for the holders that wait, which make no calls: only the claims renewed at or before `due_at`, or marked stale, so a wait renews each claim once per half lease instead of at every sweep.
pub async fn renew_due(
    tx: &mut WriteTx,
    worker_ids: &[i64],
    due_at: Timestamp,
    now: Timestamp,
) -> Result<Renewal, StoreError> {
    if worker_ids.is_empty() {
        return Ok(Renewal::default());
    }
    let ids = serde_json::Value::from(worker_ids.to_vec()).to_string();
    let cleared = marked(tx, Some(&ids)).await?;
    let (due_at, now) = (due_at.as_millis(), now.as_millis());
    let result = sqlx::query!(
        "UPDATE claims SET renewed_at = ?3, stale_at = NULL, stale_needs_person_at = NULL
         WHERE released_at IS NULL AND worker_id IN (SELECT value FROM json_each(?1))
           AND (renewed_at <= ?2 OR stale_at IS NOT NULL OR stale_needs_person_at IS NOT NULL)",
        ids,
        due_at,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(Renewal {
        count: result.rows_affected(),
        cleared,
    })
}

/// [`renew`] for every worker of the agent `agent_id`, as when the agent's worker starts to wait for others.
pub async fn renew_of_agent(
    tx: &mut WriteTx,
    agent_id: i64,
    now: Timestamp,
) -> Result<Renewal, StoreError> {
    let ids: Vec<i64> = sqlx::query_scalar!(
        r#"SELECT id AS "id!: i64" FROM workers WHERE agent_id = ?"#,
        agent_id
    )
    .fetch_all(tx.conn())
    .await?;
    renew(tx, &ids, now).await
}

/// Renews every unreleased claim of the worker that started proposal-stage run `run`, at its end `now`, so the claims get a full lease after the run the worker waited for. A run of another stage renews nothing.
pub async fn renew_for_run(
    tx: &mut WriteTx,
    run: CheckRunId,
    now: Timestamp,
) -> Result<Renewal, StoreError> {
    let number = run.to_db();
    let worker: Option<i64> = sqlx::query_scalar!(
        r#"SELECT worker_id AS "worker!: i64" FROM check_runs WHERE number = ? AND stage = 'proposal'"#,
        number
    )
    .fetch_optional(tx.conn())
    .await?;
    let Some(worker) = worker else {
        return Ok(Renewal::default());
    };
    let ids = serde_json::Value::from(vec![worker]).to_string();
    let cleared = marked(tx, Some(&ids)).await?;
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE claims SET renewed_at = MAX(renewed_at, ?2), stale_at = NULL, stale_needs_person_at = NULL
         WHERE released_at IS NULL AND worker_id = ?1",
        worker,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(Renewal {
        count: result.rows_affected(),
        cleared,
    })
}

/// Gives every unreleased claim of the project a full lease from `now` and clears its stale mark, as the end of a pause does.
pub async fn renew_all(tx: &mut WriteTx, now: Timestamp) -> Result<Renewal, StoreError> {
    let cleared = marked(tx, None).await?;
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE claims SET renewed_at = ?, stale_at = NULL, stale_needs_person_at = NULL WHERE released_at IS NULL",
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(Renewal {
        count: result.rows_affected(),
        cleared,
    })
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

/// An unreleased claim of either kind with its holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldClaim {
    pub target: ClaimTarget,
    pub worker_id: i64,
    pub worker: WorkerName,
}

/// The unreleased claims, of either kind, that the workers of the agent `agent_id` hold, in the order they were taken.
pub async fn unreleased_of_agent(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Vec<HeldClaim>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.kind, t.number AS "task!: i64", p.number AS "proposal?: i64",
                  c.worker_id, w.name AS worker
           FROM claims c
           JOIN workers w ON w.id = c.worker_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = COALESCE(c.task_id, p.task_id)
           WHERE w.agent_id = ? AND c.released_at IS NULL
           ORDER BY c.id"#,
        agent_id
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
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
            Ok(HeldClaim {
                target,
                worker_id: row.worker_id,
                worker: WorkerName::parse(&row.worker)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
            })
        })
        .collect()
}

/// What one unreleased claim waits for from others, by its holder's `workers.id`; see [`fold_core::claims::ClaimWait`].
pub async fn claim_waits(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<(i64, ClaimWait)>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.worker_id AS "worker!: i64",
                  (c.kind = 'implementation' AND EXISTS (
                       SELECT 1 FROM proposals p
                       WHERE p.task_id = c.task_id AND p.state = 'open'
                         AND NOT p.conflicted AND NOT p.checks_failed
                         AND NOT (
                             (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) = 'request_changes'
                             AND EXISTS (SELECT 1 FROM reviews r
                                         WHERE r.verdict = 'request_changes'
                                           AND r.version_id = (SELECT v.id FROM proposal_versions v
                                                               WHERE v.proposal_id = p.id
                                                               ORDER BY v.version DESC LIMIT 1))))) AS "sent!: bool",
                  (c.kind = 'implementation' AND EXISTS (
                       SELECT 1 FROM open_human_questions q WHERE q.task_id = c.task_id)) AS "asks_person!: bool",
                  (c.kind = 'implementation' AND EXISTS (
                       SELECT 1 FROM task_reports r
                       WHERE r.task_id = c.task_id AND r.resolution IS NULL)) AS "reported!: bool",
                  (c.kind = 'review' AND EXISTS (
                       SELECT 1 FROM reviews r
                       JOIN workers rw ON rw.id = r.reviewer_worker_id
                       WHERE rw.agent_id = (SELECT w.agent_id FROM workers w WHERE w.id = c.worker_id)
                         AND r.version_id = (SELECT v.id FROM proposal_versions v
                                             WHERE v.proposal_id = c.proposal_id
                                             ORDER BY v.version DESC LIMIT 1))) AS "verdict_given!: bool"
           FROM claims c
           WHERE c.released_at IS NULL
           ORDER BY c.id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.worker,
                ClaimWait {
                    sent: row.sent,
                    asks_person: row.asks_person,
                    reported: row.reported,
                    verdict_given: row.verdict_given,
                },
            )
        })
        .collect())
}

/// An unreleased claim that [`mark_stale`] marked, or that [`mark_needs_person`] passed to a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleClaim {
    /// The claim's row id, which stays inside the daemon.
    pub id: i64,
    pub target: ClaimTarget,
    pub worker: WorkerName,
    pub renewed_at: Timestamp,
}

/// Sets `stale_at = now` on each unreleased claim, of either kind, that has no `stale_at` yet and whose `renewed_at` and holder's row id pass `is_stale`, and returns them in the order they were taken. A marked claim is passed over until `renew` clears its `stale_at`, so each lapse is marked once.
pub async fn mark_stale(
    tx: &mut WriteTx,
    now: Timestamp,
    is_stale: impl Fn(Timestamp, i64) -> bool,
) -> Result<Vec<StaleClaim>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", c.kind, c.worker_id, t.number AS "task!: i64",
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
        if !is_stale(renewed_at, row.worker_id) {
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
            id: row.id,
            target,
            worker: WorkerName::parse(&row.worker)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
            renewed_at,
        });
    }
    Ok(marked)
}

/// Sets `stale_needs_person_at = now` on each unreleased claim of `ids` that is marked stale and does not need a person yet.
pub async fn set_needs_person(
    tx: &mut WriteTx,
    ids: &[i64],
    now: Timestamp,
) -> Result<(), StoreError> {
    if ids.is_empty() {
        return Ok(());
    }
    let ids = serde_json::Value::from(ids.to_vec()).to_string();
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE claims SET stale_needs_person_at = ?2
         WHERE id IN (SELECT value FROM json_each(?1))
           AND released_at IS NULL AND stale_at IS NOT NULL AND stale_needs_person_at IS NULL",
        ids,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Sets `stale_needs_person_at = now` on each unreleased claim marked stale that does not need a person yet and whose `stale_at` passes `needs_person`, and returns them in the order they were taken.
pub async fn mark_needs_person(
    tx: &mut WriteTx,
    now: Timestamp,
    needs_person: impl Fn(Timestamp) -> bool,
) -> Result<Vec<StaleClaim>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", c.kind, t.number AS "task!: i64",
                  p.number AS "proposal?: i64", w.name AS worker, c.renewed_at,
                  c.stale_at AS "stale_at!: i64"
           FROM claims c
           JOIN workers w ON w.id = c.worker_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = COALESCE(c.task_id, p.task_id)
           WHERE c.released_at IS NULL AND c.stale_at IS NOT NULL
             AND c.stale_needs_person_at IS NULL
           ORDER BY c.id"#
    )
    .fetch_all(tx.conn())
    .await?;
    let mut marked = Vec::new();
    for row in rows {
        if !needs_person(time(row.stale_at, "claims.stale_at")?) {
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
        marked.push(StaleClaim {
            id: row.id,
            target,
            worker: WorkerName::parse(&row.worker)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
            renewed_at: time(row.renewed_at, "claims.renewed_at")?,
        });
    }
    let ids: Vec<i64> = marked.iter().map(|claim| claim.id).collect();
    set_needs_person(tx, &ids, now).await?;
    Ok(marked)
}

/// A stale claim that needs a person, as Needs you lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimForPerson {
    pub target: ClaimTarget,
    pub worker: WorkerName,
    /// When it started to need a person.
    pub since: Timestamp,
}

/// The unreleased claims that need a person, oldest first: an implementation claim, or a review claim on an open proposal.
pub async fn claims_for_person(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<ClaimForPerson>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.kind, t.number AS "task!: i64", p.number AS "proposal?: i64",
                  w.name AS worker, c.stale_needs_person_at AS "since!: i64"
           FROM claims c
           JOIN workers w ON w.id = c.worker_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = COALESCE(c.task_id, p.task_id)
           WHERE c.released_at IS NULL AND c.stale_needs_person_at IS NOT NULL
             AND (c.kind = 'implementation' OR p.state = 'open')
           ORDER BY c.stale_needs_person_at, c.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
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
            Ok(ClaimForPerson {
                target,
                worker: WorkerName::parse(&row.worker)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                since: time(row.since, "claims.stale_needs_person_at")?,
            })
        })
        .collect()
}

/// The tasks with a stale claim that needs a person: an unreleased implementation claim with `stale_needs_person_at` set, or such a review claim on the task's open proposal. A claim that is only marked stale waits for the agent fold told of it.
pub async fn tasks_with_stale_claims(
    executor: impl SqliteExecutor<'_>,
) -> Result<BTreeSet<TaskId>, StoreError> {
    let rows = sqlx::query_scalar!(
        r#"SELECT t.number AS "task!: i64"
           FROM claims c JOIN tasks t ON t.id = c.task_id
           WHERE c.kind = 'implementation' AND c.released_at IS NULL AND c.stale_needs_person_at IS NOT NULL
           UNION
           SELECT t.number
           FROM claims c
           JOIN proposals p ON p.id = c.proposal_id
           JOIN tasks t ON t.id = p.task_id
           WHERE c.kind = 'review' AND c.released_at IS NULL AND c.stale_needs_person_at IS NOT NULL
             AND p.state = 'open'"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|number| TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number")))
        .collect()
}

/// How many unreleased claims, of either kind, the worker of agent `agent_id` holds with a `renewed_at` at or before `cutoff`.
pub async fn count_renewed_by(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    cutoff: Timestamp,
) -> Result<u64, StoreError> {
    let cutoff = cutoff.as_millis();
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM claims c JOIN workers w ON w.id = c.worker_id
           WHERE w.agent_id = ?1 AND w.parent_worker_id IS NULL
             AND c.released_at IS NULL AND c.renewed_at <= ?2"#,
        agent_id,
        cutoff
    )
    .fetch_one(executor)
    .await?;
    u64::try_from(count).map_err(|_| StoreError::Corrupt("claims"))
}

use fold_core::cost::TurnGroup;
use fold_core::{AgentName, AreaName, ProposalId, Role, TaskId, Timestamp, WorkerState};
use sqlx::SqliteExecutor;

use super::workers::{TurnTotals, db_count};
use crate::columns::{count, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// Appends the turn that just ended for worker row `id`, which added `cost_micro_usd` to the row's total. It copies the row's role, area and model, and counts the turn for one subject. A failed turn of a row that is stopping or has ended is the interrupt of a stop, which the row's state shows before its result comes. A task worker's turn counts for the task of its agent's newest unreleased implementation claim; without one, for its one bound task; with several bound tasks, for the bound task its agent claimed last. A reviewer's turn counts the same way for a proposal, through review claims and bound proposals. A lead's turn, and a turn none of these name, counts for no subject.
pub(super) async fn insert_turn(
    tx: &mut WriteTx,
    id: i64,
    cost_micro_usd: u64,
    totals: &TurnTotals,
    now: Timestamp,
) -> Result<(), StoreError> {
    let cost = db_count(cost_micro_usd);
    let input = db_count(totals.input_tokens);
    let output = db_count(totals.output_tokens);
    let read = db_count(totals.cache_read_tokens);
    let write = db_count(totals.cache_write_tokens);
    let failed = totals.failed;
    let now = now.as_millis();
    sqlx::query!(
        r#"INSERT INTO worker_turns (
               worker_process_id, role, area_id, task_id, proposal_id, model, cost_micro_usd,
               input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, failed, interrupted,
               created_at
           )
           SELECT w.id, w.role, w.area_id,
                  CASE WHEN w.role = 'task_worker' THEN COALESCE(
                      (SELECT c.task_id FROM claims c JOIN workers k ON k.id = c.worker_id
                       WHERE k.agent_id = w.agent_id AND c.kind = 'implementation' AND c.released_at IS NULL
                       ORDER BY c.renewed_at DESC, c.id DESC LIMIT 1),
                      (SELECT CASE WHEN COUNT(*) = 1 THEN MIN(b.task_id) END
                       FROM worker_tasks b WHERE b.worker_process_id = w.id),
                      (SELECT c.task_id FROM claims c JOIN workers k ON k.id = c.worker_id
                       JOIN worker_tasks b ON b.worker_process_id = w.id AND b.task_id = c.task_id
                       WHERE k.agent_id = w.agent_id AND c.kind = 'implementation'
                       ORDER BY c.renewed_at DESC, c.id DESC LIMIT 1)
                  ) END,
                  CASE WHEN w.role = 'reviewer' THEN COALESCE(
                      (SELECT c.proposal_id FROM claims c JOIN workers k ON k.id = c.worker_id
                       WHERE k.agent_id = w.agent_id AND c.kind = 'review' AND c.released_at IS NULL
                       ORDER BY c.renewed_at DESC, c.id DESC LIMIT 1),
                      (SELECT CASE WHEN COUNT(*) = 1 THEN MIN(b.proposal_id) END
                       FROM worker_proposals b WHERE b.worker_process_id = w.id),
                      (SELECT c.proposal_id FROM claims c JOIN workers k ON k.id = c.worker_id
                       JOIN worker_proposals b ON b.worker_process_id = w.id AND b.proposal_id = c.proposal_id
                       WHERE k.agent_id = w.agent_id AND c.kind = 'review'
                       ORDER BY c.renewed_at DESC, c.id DESC LIMIT 1)
                  ) END,
                  w.model, ?2, ?3, ?4, ?5, ?6, ?7,
                  ?7 AND w.state NOT IN ('starting', 'working', 'idle'),
                  ?8
           FROM worker_processes w WHERE w.id = ?1"#,
        id,
        cost,
        input,
        output,
        read,
        write,
        failed,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The turns that ended at or after `since`, every turn without it, in groups of one worker, model and subject, in worker start order. A stop's interrupt adds its cost but is not counted as a turn. With `task`, only the turns that built the task or reviewed one of its proposals.
pub async fn cost_groups(
    executor: impl SqliteExecutor<'_>,
    since: Option<Timestamp>,
    task: Option<TaskId>,
) -> Result<Vec<TurnGroup>, StoreError> {
    let since = since.map_or(i64::MIN, Timestamp::as_millis);
    let task = task.map(TaskId::to_db);
    let rows = sqlx::query!(
        r#"SELECT g.name AS "agent!: String", w.role AS "role!: String", a.name AS "area?: String",
                  w.state AS "state!: String", x.model AS "model?: String",
                  t.number AS "task?: i64", p.number AS "proposal?: i64",
                  COALESCE(t.number, pt.number) AS "subject?: i64",
                  COALESCE(t.title, pt.title) AS "title?: String",
                  SUM(1 - x.interrupted) AS "turns!: i64", SUM(x.cost_micro_usd) AS "cost!: i64",
                  MIN(x.created_at) AS "first_at!: i64", MAX(x.created_at) AS "last_at!: i64"
           FROM worker_turns x
           JOIN worker_processes w ON w.id = x.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           LEFT JOIN areas a ON a.id = x.area_id
           LEFT JOIN tasks t ON t.id = x.task_id
           LEFT JOIN proposals p ON p.id = x.proposal_id
           LEFT JOIN tasks pt ON pt.id = p.task_id
           WHERE x.created_at >= ?1 AND (?2 IS NULL OR t.number = ?2 OR pt.number = ?2)
           GROUP BY x.worker_process_id, x.model, x.task_id, x.proposal_id
           ORDER BY x.worker_process_id, MIN(x.created_at)"#,
        since,
        task
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let task_of = |number: i64| {
                TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number"))
            };
            Ok(TurnGroup {
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                role: Role::parse(&row.role).ok_or(StoreError::Corrupt("worker_processes.role"))?,
                area: row
                    .area
                    .map(|area| {
                        AreaName::parse(&area).map_err(|_| StoreError::Corrupt("areas.name"))
                    })
                    .transpose()?,
                state: WorkerState::parse(&row.state)
                    .ok_or(StoreError::Corrupt("worker_processes.state"))?,
                model: row.model,
                task: row.task.map(task_of).transpose()?,
                proposal: row
                    .proposal
                    .map(|number| {
                        ProposalId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("proposals.number"))
                    })
                    .transpose()?,
                subject: match (row.subject, row.title) {
                    (Some(number), Some(title)) => Some((task_of(number)?, title)),
                    _ => None,
                },
                turns: count(row.turns, "worker_turns")?,
                cost_micro_usd: count(row.cost, "worker_turns.cost_micro_usd")?,
                first_at: time(row.first_at, "worker_turns.created_at")?,
                last_at: time(row.last_at, "worker_turns.created_at")?,
            })
        })
        .collect()
}

/// The cost of the turns of one role in one slot, whose length [`fold_core::cost::slot_millis`] picks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostSlot {
    pub start: Timestamp,
    pub role: Role,
    pub cost_micro_usd: u64,
}

/// The cost of the turns that ended at or after `since` by slot of `slot_millis` and role, oldest first; a slot without a turn is left out.
pub async fn cost_slots(
    executor: impl SqliteExecutor<'_>,
    since: Option<Timestamp>,
    slot_millis: i64,
) -> Result<Vec<CostSlot>, StoreError> {
    let since = since.map_or(i64::MIN, Timestamp::as_millis);
    let rows = sqlx::query!(
        r#"SELECT (created_at / ?2) * ?2 AS "start!: i64", role AS "role!: String",
                  SUM(cost_micro_usd) AS "cost!: i64"
           FROM worker_turns WHERE created_at >= ?1
           GROUP BY created_at / ?2, role
           ORDER BY created_at / ?2, role"#,
        since,
        slot_millis
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(CostSlot {
                start: time(row.start, "worker_turns.created_at")?,
                role: Role::parse(&row.role).ok_or(StoreError::Corrupt("worker_turns.role"))?,
                cost_micro_usd: count(row.cost, "worker_turns.cost_micro_usd")?,
            })
        })
        .collect()
}

/// The cost of every turn, and of the turns that ended at or after `today` and `week`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CostTotals {
    pub all_time: u64,
    pub today: u64,
    pub week: u64,
}

pub async fn cost_totals(
    executor: impl SqliteExecutor<'_>,
    today: Timestamp,
    week: Timestamp,
) -> Result<CostTotals, StoreError> {
    let today = today.as_millis();
    let week = week.as_millis();
    let row = sqlx::query!(
        r#"SELECT COALESCE(SUM(cost_micro_usd), 0) AS "all_time!: i64",
                  COALESCE(SUM(CASE WHEN created_at >= ?1 THEN cost_micro_usd END), 0) AS "today!: i64",
                  COALESCE(SUM(CASE WHEN created_at >= ?2 THEN cost_micro_usd END), 0) AS "week!: i64"
           FROM worker_turns"#,
        today,
        week
    )
    .fetch_one(executor)
    .await?;
    Ok(CostTotals {
        all_time: count(row.all_time, "worker_turns.cost_micro_usd")?,
        today: count(row.today, "worker_turns.cost_micro_usd")?,
        week: count(row.week, "worker_turns.cost_micro_usd")?,
    })
}

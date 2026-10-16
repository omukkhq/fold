use fold_core::{AgentName, AreaName, LabelName, ProposalId, Role, TaskId, Timestamp, WorkerState};
use sqlx::{SqliteConnection, SqliteExecutor};

use super::{optional_time, time};
use crate::error::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaRow {
    pub id: i64,
    pub name: AreaName,
    pub label_id: i64,
    pub label: LabelName,
    pub lead: Option<AgentName>,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A worker agent's supervised process with what it is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerProcessRow {
    pub id: i64,
    pub agent_id: i64,
    pub agent: AgentName,
    pub role: Role,
    pub area_id: Option<i64>,
    pub area: Option<AreaName>,
    pub model: Option<String>,
    pub session_id: String,
    pub pid: Option<i64>,
    pub state: WorkerState,
    pub started_by: String,
    pub started_at: Timestamp,
    pub stopped_at: Option<Timestamp>,
    pub idle_since: Option<Timestamp>,
    pub turns: u64,
    pub cost_micro_usd: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub context_tokens: u64,
    pub exit_code: Option<i64>,
    pub tasks: Vec<TaskId>,
    pub proposals: Vec<ProposalId>,
}

pub struct NewWorkerProcess<'a> {
    pub agent_id: i64,
    pub role: Role,
    pub area_id: Option<i64>,
    pub model: Option<&'a str>,
    pub session_id: &'a str,
    pub started_by: &'a str,
    pub now: Timestamp,
}

/// The totals one stdout line of a worker adds. `cost_micro_usd` is the new total of the row, because Claude Code reports a running total per process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnTotals {
    pub cost_micro_usd: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

fn count(value: i64, column: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt(column))
}

fn db_count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn area_row(
    id: i64,
    name: &str,
    label_id: i64,
    label: &str,
    lead: Option<String>,
    created_by: String,
    created_at: i64,
) -> Result<AreaRow, StoreError> {
    Ok(AreaRow {
        id,
        name: AreaName::parse(name).map_err(|_| StoreError::Corrupt("areas.name"))?,
        label_id,
        label: LabelName::parse(label).map_err(|_| StoreError::Corrupt("labels.name"))?,
        lead: lead
            .map(|lead| AgentName::parse(&lead).map_err(|_| StoreError::Corrupt("agents.name")))
            .transpose()?,
        created_by,
        created_at: time(created_at, "areas.created_at")?,
    })
}

/// The label's row id, or `None` when no label has the name.
pub async fn label_id(
    executor: impl SqliteExecutor<'_>,
    label: &LabelName,
) -> Result<Option<i64>, StoreError> {
    let name = label.as_str();
    Ok(sqlx::query_scalar!(
        r#"SELECT id AS "id!: i64" FROM labels WHERE name = ?"#,
        name
    )
    .fetch_optional(executor)
    .await?)
}

/// Inserts an area. A taken name fails on `areas.name`, a label of another area on `areas.label_id`.
pub async fn insert_area(
    executor: impl SqliteExecutor<'_>,
    name: &AreaName,
    label_id: i64,
    created_by: &str,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let name = name.as_str();
    let now = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO areas (name, label_id, created_by, created_at) VALUES (?, ?, ?, ?)
           RETURNING id AS "id!: i64""#,
        name,
        label_id,
        created_by,
        now
    )
    .fetch_one(executor)
    .await?)
}

pub async fn find_area(
    executor: impl SqliteExecutor<'_>,
    name: &AreaName,
) -> Result<Option<AreaRow>, StoreError> {
    let name = name.as_str();
    let row = sqlx::query!(
        r#"SELECT a.id AS "id!: i64", a.name, a.label_id, l.name AS label, g.name AS "lead?: String",
                  a.created_by, a.created_at
           FROM areas a JOIN labels l ON l.id = a.label_id LEFT JOIN agents g ON g.id = a.lead_agent_id
           WHERE a.name = ?"#,
        name
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        area_row(
            row.id,
            &row.name,
            row.label_id,
            &row.label,
            row.lead,
            row.created_by,
            row.created_at,
        )
    })
    .transpose()
}

/// Every area in name order.
pub async fn list_areas(executor: impl SqliteExecutor<'_>) -> Result<Vec<AreaRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT a.id AS "id!: i64", a.name, a.label_id, l.name AS label, g.name AS "lead?: String",
                  a.created_by, a.created_at
           FROM areas a JOIN labels l ON l.id = a.label_id LEFT JOIN agents g ON g.id = a.lead_agent_id
           ORDER BY a.name"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            area_row(
                row.id,
                &row.name,
                row.label_id,
                &row.label,
                row.lead,
                row.created_by,
                row.created_at,
            )
        })
        .collect()
}

/// Makes `agent_id` the area's lead. The `areas_lead_once` trigger refuses a second lead.
pub async fn set_area_lead(
    executor: impl SqliteExecutor<'_>,
    area_id: i64,
    agent_id: i64,
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE areas SET lead_agent_id = ? WHERE id = ?",
        agent_id,
        area_id
    )
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn insert_worker_process(
    executor: impl SqliteExecutor<'_>,
    new: &NewWorkerProcess<'_>,
) -> Result<i64, StoreError> {
    let role = new.role.as_str();
    let now = new.now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO worker_processes (agent_id, role, area_id, model, session_id, state, started_by, started_at)
           VALUES (?, ?, ?, ?, ?, 'starting', ?, ?) RETURNING id AS "id!: i64""#,
        new.agent_id,
        role,
        new.area_id,
        new.model,
        new.session_id,
        new.started_by,
        now
    )
    .fetch_one(executor)
    .await?)
}

/// Binds a task coder to `task`; an unknown task binds nothing and returns false.
pub async fn bind_task(
    conn: &mut SqliteConnection,
    worker_process_id: i64,
    task: TaskId,
) -> Result<bool, StoreError> {
    let number = task.to_db();
    let result = sqlx::query!(
        "INSERT OR IGNORE INTO worker_tasks (worker_process_id, task_id)
         SELECT ?, id FROM tasks WHERE number = ?",
        worker_process_id,
        number
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Binds a batch reviewer to `proposal`; an unknown proposal binds nothing and returns false.
pub async fn bind_proposal(
    conn: &mut SqliteConnection,
    worker_process_id: i64,
    proposal: ProposalId,
) -> Result<bool, StoreError> {
    let number = proposal.to_db();
    let result = sqlx::query!(
        "INSERT OR IGNORE INTO worker_proposals (worker_process_id, proposal_id)
         SELECT ?, id FROM proposals WHERE number = ?",
        worker_process_id,
        number
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

struct RawProcess {
    id: i64,
    agent_id: i64,
    agent: String,
    role: String,
    area_id: Option<i64>,
    area: Option<String>,
    model: Option<String>,
    session_id: String,
    pid: Option<i64>,
    state: String,
    started_by: String,
    started_at: i64,
    stopped_at: Option<i64>,
    idle_since: Option<i64>,
    turns: i64,
    cost_micro_usd: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    context_tokens: i64,
    exit_code: Option<i64>,
}

impl RawProcess {
    fn into_row(self) -> Result<WorkerProcessRow, StoreError> {
        Ok(WorkerProcessRow {
            id: self.id,
            agent_id: self.agent_id,
            agent: AgentName::parse(&self.agent).map_err(|_| StoreError::Corrupt("agents.name"))?,
            role: Role::parse(&self.role).ok_or(StoreError::Corrupt("worker_processes.role"))?,
            area_id: self.area_id,
            area: self
                .area
                .map(|area| AreaName::parse(&area).map_err(|_| StoreError::Corrupt("areas.name")))
                .transpose()?,
            model: self.model,
            session_id: self.session_id,
            pid: self.pid,
            state: WorkerState::parse(&self.state)
                .ok_or(StoreError::Corrupt("worker_processes.state"))?,
            started_by: self.started_by,
            started_at: time(self.started_at, "worker_processes.started_at")?,
            stopped_at: optional_time(self.stopped_at, "worker_processes.stopped_at")?,
            idle_since: optional_time(self.idle_since, "worker_processes.idle_since")?,
            turns: count(self.turns, "worker_processes.turns")?,
            cost_micro_usd: count(self.cost_micro_usd, "worker_processes.cost_micro_usd")?,
            cache_read_tokens: count(self.cache_read_tokens, "worker_processes.cache_read_tokens")?,
            cache_write_tokens: count(
                self.cache_write_tokens,
                "worker_processes.cache_write_tokens",
            )?,
            context_tokens: count(self.context_tokens, "worker_processes.context_tokens")?,
            exit_code: self.exit_code,
            tasks: Vec::new(),
            proposals: Vec::new(),
        })
    }
}

async fn with_bindings(
    conn: &mut SqliteConnection,
    raw: Vec<RawProcess>,
) -> Result<Vec<WorkerProcessRow>, StoreError> {
    let mut rows = Vec::with_capacity(raw.len());
    for raw in raw {
        let mut row = raw.into_row()?;
        let tasks = sqlx::query_scalar!(
            r#"SELECT t.number AS "number!: i64" FROM worker_tasks w JOIN tasks t ON t.id = w.task_id
               WHERE w.worker_process_id = ? ORDER BY t.number"#,
            row.id
        )
        .fetch_all(&mut *conn)
        .await?;
        row.tasks = tasks
            .into_iter()
            .map(|number| TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number")))
            .collect::<Result<_, _>>()?;
        let proposals = sqlx::query_scalar!(
            r#"SELECT p.number AS "number!: i64" FROM worker_proposals w JOIN proposals p ON p.id = w.proposal_id
               WHERE w.worker_process_id = ? ORDER BY p.number"#,
            row.id
        )
        .fetch_all(&mut *conn)
        .await?;
        row.proposals = proposals
            .into_iter()
            .map(|number| {
                ProposalId::try_from_db(number).ok_or(StoreError::Corrupt("proposals.number"))
            })
            .collect::<Result<_, _>>()?;
        rows.push(row);
    }
    Ok(rows)
}

/// The worker process of `agent`, or `None` for an agent fold does not supervise.
pub async fn find_worker_process(
    conn: &mut SqliteConnection,
    agent: &AgentName,
) -> Result<Option<WorkerProcessRow>, StoreError> {
    let name = agent.as_str();
    let raw = sqlx::query_as!(
        RawProcess,
        r#"SELECT w.id AS "id!: i64", w.agent_id, g.name AS agent, w.role, w.area_id, a.name AS "area?: String",
                  w.model, w.session_id, w.pid, w.state, w.started_by, w.started_at, w.stopped_at,
                  w.idle_since, w.turns, w.cost_micro_usd, w.cache_read_tokens, w.cache_write_tokens,
                  w.context_tokens, w.exit_code
           FROM worker_processes w JOIN agents g ON g.id = w.agent_id LEFT JOIN areas a ON a.id = w.area_id
           WHERE g.name = ?"#,
        name
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(with_bindings(conn, raw.into_iter().collect()).await?.pop())
}

/// Every worker process in start order.
pub async fn list_worker_processes(
    conn: &mut SqliteConnection,
) -> Result<Vec<WorkerProcessRow>, StoreError> {
    let raw = sqlx::query_as!(
        RawProcess,
        r#"SELECT w.id AS "id!: i64", w.agent_id, g.name AS agent, w.role, w.area_id, a.name AS "area?: String",
                  w.model, w.session_id, w.pid, w.state, w.started_by, w.started_at, w.stopped_at,
                  w.idle_since, w.turns, w.cost_micro_usd, w.cache_read_tokens, w.cache_write_tokens,
                  w.context_tokens, w.exit_code
           FROM worker_processes w JOIN agents g ON g.id = w.agent_id LEFT JOIN areas a ON a.id = w.area_id
           ORDER BY w.id"#
    )
    .fetch_all(&mut *conn)
    .await?;
    with_bindings(conn, raw).await
}

/// How many worker processes are running, in the whole project and in area `area_id`.
pub async fn running_counts(
    executor: impl SqliteExecutor<'_>,
    area_id: Option<i64>,
) -> Result<(u64, u64), StoreError> {
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "total!: i64",
                  COALESCE(SUM(area_id IS ?1), 0) AS "in_area!: i64"
           FROM worker_processes WHERE state NOT IN ('stopped', 'failed')"#,
        area_id
    )
    .fetch_one(executor)
    .await?;
    Ok((
        count(row.total, "worker_processes")?,
        count(row.in_area, "worker_processes")?,
    ))
}

/// Moves the row to `state`. `idle_since` follows an idle state and `stopped_at` a final one, as the table's checks require.
pub async fn set_worker_state(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    state: WorkerState,
    now: Timestamp,
) -> Result<(), StoreError> {
    let state = state.as_str();
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes
         SET state = ?2,
             idle_since = CASE WHEN ?2 = 'idle' THEN COALESCE(idle_since, ?3) ELSE NULL END,
             stopped_at = CASE WHEN ?2 IN ('stopped', 'failed') THEN ?3 ELSE NULL END
         WHERE id = ?1",
        id,
        state,
        now
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// A turn began: a starting or idle worker is working. A stopping or ended worker keeps its state.
pub async fn mark_worker_working(
    executor: impl SqliteExecutor<'_>,
    id: i64,
) -> Result<bool, StoreError> {
    let result = sqlx::query!(
        "UPDATE worker_processes SET state = 'working', idle_since = NULL
         WHERE id = ? AND state IN ('starting', 'idle')",
        id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_worker_pid(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    pid: Option<u32>,
) -> Result<(), StoreError> {
    let pid = pid.map(i64::from);
    sqlx::query!("UPDATE worker_processes SET pid = ? WHERE id = ?", pid, id)
        .execute(executor)
        .await?;
    Ok(())
}

/// The model Claude Code reports, which can resolve an alias, and the context size of its newest request.
pub async fn set_worker_context(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    model: Option<&str>,
    context_tokens: Option<u64>,
) -> Result<(), StoreError> {
    let context = context_tokens.map(db_count);
    sqlx::query!(
        "UPDATE worker_processes SET model = COALESCE(?2, model), context_tokens = COALESCE(?3, context_tokens)
         WHERE id = ?1",
        id,
        model,
        context
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Counts one finished turn and makes the worker idle.
pub async fn record_turn(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    totals: TurnTotals,
    now: Timestamp,
) -> Result<(), StoreError> {
    let cost = db_count(totals.cost_micro_usd);
    let read = db_count(totals.cache_read_tokens);
    let write = db_count(totals.cache_write_tokens);
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes
         SET turns = turns + 1, cost_micro_usd = ?2,
             cache_read_tokens = cache_read_tokens + ?3, cache_write_tokens = cache_write_tokens + ?4,
             state = CASE WHEN state IN ('starting', 'working') THEN 'idle' ELSE state END,
             idle_since = CASE WHEN state IN ('starting', 'working') THEN ?5 ELSE idle_since END
         WHERE id = ?1",
        id,
        cost,
        read,
        write,
        now
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Ends the process of the row: `stopped` or `failed`, with its exit code.
pub async fn record_exit(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    state: WorkerState,
    exit_code: Option<i32>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let state = state.as_str();
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes SET state = ?2, stopped_at = ?3, idle_since = NULL, pid = NULL, exit_code = ?4
         WHERE id = ?1",
        id,
        state,
        now,
        exit_code
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Starts the row again for a new process of the same session.
pub async fn restart_worker_process(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes
         SET state = 'starting', started_at = ?2, stopped_at = NULL, idle_since = NULL, pid = NULL, exit_code = NULL
         WHERE id = ?1",
        id,
        now
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// The running task coders bound to `task`.
pub async fn coders_of_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Vec<AgentName>, StoreError> {
    let number = task.to_db();
    let names = sqlx::query_scalar!(
        r#"SELECT g.name FROM worker_tasks b
           JOIN tasks t ON t.id = b.task_id
           JOIN worker_processes w ON w.id = b.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           WHERE t.number = ? AND w.state NOT IN ('stopped', 'failed', 'stopping')
           ORDER BY w.id"#,
        number
    )
    .fetch_all(executor)
    .await?;
    names
        .iter()
        .map(|name| AgentName::parse(name).map_err(|_| StoreError::Corrupt("agents.name")))
        .collect()
}

/// The running batch reviewers bound to `proposal`.
pub async fn reviewers_of_proposal(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<AgentName>, StoreError> {
    let number = proposal.to_db();
    let names = sqlx::query_scalar!(
        r#"SELECT g.name FROM worker_proposals b
           JOIN proposals p ON p.id = b.proposal_id
           JOIN worker_processes w ON w.id = b.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           WHERE p.number = ? AND w.state NOT IN ('stopped', 'failed', 'stopping')
           ORDER BY w.id"#,
        number
    )
    .fetch_all(executor)
    .await?;
    names
        .iter()
        .map(|name| AgentName::parse(name).map_err(|_| StoreError::Corrupt("agents.name")))
        .collect()
}

/// How many bound tasks of the worker still wait for its work: not Done, and with no proposal that is open or integrated. A proposal can integrate before fold looks, so an integrated one counts as sent.
pub async fn tasks_awaiting_proposal(
    executor: impl SqliteExecutor<'_>,
    worker_process_id: i64,
) -> Result<u64, StoreError> {
    let missing = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM worker_tasks b JOIN tasks t ON t.id = b.task_id
           WHERE b.worker_process_id = ? AND t.status <> 'done'
             AND NOT EXISTS (
                 SELECT 1 FROM proposals p WHERE p.task_id = b.task_id AND p.state IN ('open', 'integrated')
             )"#,
        worker_process_id
    )
    .fetch_one(executor)
    .await?;
    count(missing, "worker_tasks")
}

/// How many bound proposals of the worker are open and have no verdict on their newest version. A comment is not a verdict.
pub async fn proposals_without_verdict(
    executor: impl SqliteExecutor<'_>,
    worker_process_id: i64,
) -> Result<u64, StoreError> {
    let missing = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM worker_proposals b
           JOIN proposals p ON p.id = b.proposal_id
           WHERE b.worker_process_id = ? AND p.state = 'open'
             AND NOT EXISTS (
                 SELECT 1 FROM reviews r JOIN proposal_versions v ON v.id = r.version_id
                 WHERE v.proposal_id = p.id
                   AND v.version = (SELECT MAX(version) FROM proposal_versions WHERE proposal_id = p.id)
                   AND r.verdict <> 'comment'
             )"#,
        worker_process_id
    )
    .fetch_one(executor)
    .await?;
    count(missing, "worker_proposals")
}

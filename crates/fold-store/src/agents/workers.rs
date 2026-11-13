use std::collections::HashMap;

use fold_core::roles::AreaProfile;
use fold_core::{AgentName, AreaName, LabelName, ProposalId, Role, TaskId, Timestamp, WorkerState};
use sqlx::{SqliteConnection, SqliteExecutor};

use super::optional_time;
use crate::columns::{count, time};
use crate::error::StoreError;
use crate::store::WriteTx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaRow {
    pub id: i64,
    pub name: AreaName,
    pub label_id: i64,
    pub label: LabelName,
    pub lead: Option<AgentName>,
    pub profile: AreaProfile,
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
    /// Why the latest process failed, such as `exited` or `mcp_unavailable`; `None` unless the row is `failed`, and for a row that failed before fold stored the reason.
    pub failed_reason: Option<String>,
    /// Claude Code's error of the latest failed turn, or the stderr of the latest process that exited with an error; `None` once a turn ends without an error.
    pub last_error: Option<String>,
    pub last_error_at: Option<Timestamp>,
    /// The turns in a row that failed before any token reached the model.
    pub failed_turns: u64,
    /// The fingerprint of the skill texts that the current session last read; `None` for a session that a fold without fingerprints started. See [`fold_core::roles::session_fingerprint`].
    pub fold_fingerprint: Option<String>,
    /// The agent that stopped the worker to wait for its go, with what it waits for; `None` for every other worker. Every start of the row clears it.
    pub park: Option<WorkerPark>,
    pub tasks: Vec<TaskId>,
    pub proposals: Vec<ProposalId>,
}

/// An agent stopped the worker with `stop_worker`'s `wait_for`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPark {
    pub by: AgentName,
    /// What the worker waits for, in the parking agent's words.
    pub wait_for: String,
    pub at: Timestamp,
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

/// The totals one stdout line of a worker adds. `reported_micro_usd` is the running total of the row's session that Claude Code reported; the token counts are the turn's own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnTotals {
    pub reported_micro_usd: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The turn ended in Claude Code's error.
    pub failed: bool,
}

pub(super) fn db_count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn id_list(ids: &[i64]) -> String {
    serde_json::Value::from(ids.to_vec()).to_string()
}

/// The counts of `rows` by worker, with 0 for each id of `ids` that has no row.
fn counts_by_worker(
    ids: &[i64],
    rows: impl Iterator<Item = (i64, i64)>,
    column: &'static str,
) -> Result<HashMap<i64, u64>, StoreError> {
    let mut counts: HashMap<i64, u64> = ids.iter().map(|id| (*id, 0)).collect();
    for (worker, value) in rows {
        counts.insert(worker, count(value, column)?);
    }
    Ok(counts)
}

/// One `areas` row with its label, lead and profile, as the area queries select it.
struct RawArea {
    id: i64,
    name: String,
    label_id: i64,
    label: String,
    lead: Option<String>,
    instructions: Option<String>,
    model: Option<String>,
    mcp: String,
    created_by: String,
    created_at: i64,
}

fn area_row(raw: RawArea) -> Result<AreaRow, StoreError> {
    let mcp: Vec<String> =
        serde_json::from_str(&raw.mcp).map_err(|_| StoreError::Corrupt("area_mcp_servers"))?;
    Ok(AreaRow {
        id: raw.id,
        name: AreaName::parse(&raw.name).map_err(|_| StoreError::Corrupt("areas.name"))?,
        label_id: raw.label_id,
        label: LabelName::parse(&raw.label).map_err(|_| StoreError::Corrupt("labels.name"))?,
        lead: raw
            .lead
            .map(|lead| AgentName::parse(&lead).map_err(|_| StoreError::Corrupt("agents.name")))
            .transpose()?,
        profile: AreaProfile {
            instructions: raw.instructions,
            model: raw.model,
            mcp: (!mcp.is_empty()).then_some(mcp),
        },
        created_by: raw.created_by,
        created_at: time(raw.created_at, "areas.created_at")?,
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
    tx: &mut WriteTx,
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
    .fetch_one(tx.conn())
    .await?)
}

pub async fn find_area(
    executor: impl SqliteExecutor<'_>,
    name: &AreaName,
) -> Result<Option<AreaRow>, StoreError> {
    let name = name.as_str();
    let row = sqlx::query_as!(
        RawArea,
        r#"SELECT a.id AS "id!: i64", a.name, a.label_id, l.name AS label, g.name AS "lead?: String",
                  a.instructions, a.model,
                  (SELECT json_group_array(m.name ORDER BY m.position) FROM area_mcp_servers m
                   WHERE m.area_id = a.id) AS "mcp!: String",
                  a.created_by, a.created_at
           FROM areas a JOIN labels l ON l.id = a.label_id LEFT JOIN agents g ON g.id = a.lead_agent_id
           WHERE a.name = ?"#,
        name
    )
    .fetch_optional(executor)
    .await?;
    row.map(area_row).transpose()
}

/// Every area in name order.
pub async fn list_areas(executor: impl SqliteExecutor<'_>) -> Result<Vec<AreaRow>, StoreError> {
    let rows = sqlx::query_as!(
        RawArea,
        r#"SELECT a.id AS "id!: i64", a.name, a.label_id, l.name AS label, g.name AS "lead?: String",
                  a.instructions, a.model,
                  (SELECT json_group_array(m.name ORDER BY m.position) FROM area_mcp_servers m
                   WHERE m.area_id = a.id) AS "mcp!: String",
                  a.created_by, a.created_at
           FROM areas a JOIN labels l ON l.id = a.label_id LEFT JOIN agents g ON g.id = a.lead_agent_id
           ORDER BY a.name"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(area_row).collect()
}

/// Replaces the area's profile: its instructions, its model and its MCP servers in their order.
pub async fn set_area_profile(
    tx: &mut WriteTx,
    area_id: i64,
    profile: &AreaProfile,
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE areas SET instructions = ?, model = ? WHERE id = ?",
        profile.instructions,
        profile.model,
        area_id
    )
    .execute(tx.conn())
    .await?;
    sqlx::query!("DELETE FROM area_mcp_servers WHERE area_id = ?", area_id)
        .execute(tx.conn())
        .await?;
    for (position, name) in profile.mcp.iter().flatten().enumerate() {
        let position = i64::try_from(position).unwrap_or(i64::MAX);
        sqlx::query!(
            "INSERT INTO area_mcp_servers (area_id, position, name) VALUES (?, ?, ?)",
            area_id,
            position,
            name
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Makes `agent_id` the area's lead. The `areas_lead_once` trigger refuses a second lead.
pub async fn set_area_lead(
    tx: &mut WriteTx,
    area_id: i64,
    agent_id: i64,
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE areas SET lead_agent_id = ? WHERE id = ?",
        agent_id,
        area_id
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub async fn insert_worker_process(
    tx: &mut WriteTx,
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
    .fetch_one(tx.conn())
    .await?)
}

/// Binds a task worker to `task`. A task bound already stays bound and counts as bound, so a list that names a task twice binds it once; an unknown task binds nothing and returns false.
pub async fn bind_task(
    tx: &mut WriteTx,
    worker_process_id: i64,
    task: TaskId,
) -> Result<bool, StoreError> {
    let number = task.to_db();
    sqlx::query!(
        "INSERT OR IGNORE INTO worker_tasks (worker_process_id, task_id)
         SELECT ?, id FROM tasks WHERE number = ?",
        worker_process_id,
        number
    )
    .execute(tx.conn())
    .await?;
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM tasks WHERE number = ?) AS "known!: bool""#,
        number
    )
    .fetch_one(tx.conn())
    .await?)
}

/// Binds a reviewer to `proposal`. A proposal bound already stays bound and counts as bound, so a list that names a proposal twice binds it once; an unknown proposal binds nothing and returns false.
pub async fn bind_proposal(
    tx: &mut WriteTx,
    worker_process_id: i64,
    proposal: ProposalId,
) -> Result<bool, StoreError> {
    let number = proposal.to_db();
    sqlx::query!(
        "INSERT OR IGNORE INTO worker_proposals (worker_process_id, proposal_id)
         SELECT ?, id FROM proposals WHERE number = ?",
        worker_process_id,
        number
    )
    .execute(tx.conn())
    .await?;
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM proposals WHERE number = ?) AS "known!: bool""#,
        number
    )
    .fetch_one(tx.conn())
    .await?)
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
    failed_reason: Option<String>,
    last_error: Option<String>,
    last_error_at: Option<i64>,
    failed_turns: i64,
    fold_fingerprint: Option<String>,
    parked_by: Option<String>,
    parked_for: Option<String>,
    parked_at: Option<i64>,
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
            failed_reason: self.failed_reason,
            last_error: self.last_error,
            last_error_at: optional_time(self.last_error_at, "worker_processes.last_error_at")?,
            failed_turns: count(self.failed_turns, "worker_processes.failed_turns")?,
            fold_fingerprint: self.fold_fingerprint,
            park: match (self.parked_by, self.parked_for, self.parked_at) {
                (Some(by), Some(wait_for), Some(at)) => Some(WorkerPark {
                    by: AgentName::parse(&by).map_err(|_| StoreError::Corrupt("agents.name"))?,
                    wait_for,
                    at: time(at, "worker_processes.parked_at")?,
                }),
                _ => None,
            },
            tasks: Vec::new(),
            proposals: Vec::new(),
        })
    }
}

/// The columns of [`RawProcess`] and the joins that give them, followed by the tail of each read: every worker row maps the same way.
macro_rules! select_processes {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            RawProcess,
            r#"SELECT w.id AS "id!: i64", w.agent_id, g.name AS agent, w.role, w.area_id, a.name AS "area?: String",
                      w.model, w.session_id, w.pid, w.state, w.started_by, w.started_at, w.stopped_at,
                      w.idle_since, w.turns, w.cost_micro_usd, w.cache_read_tokens, w.cache_write_tokens,
                      w.context_tokens, w.exit_code, w.failed_reason, w.last_error, w.last_error_at,
                      w.failed_turns, w.fold_fingerprint, pa.name AS "parked_by?: String", w.parked_for,
                      w.parked_at
               FROM worker_processes w JOIN agents g ON g.id = w.agent_id LEFT JOIN areas a ON a.id = w.area_id
               LEFT JOIN agents pa ON pa.id = w.parked_by
               "# + $tail
            $(, $arg)*
        )
    };
}

/// The rows with the tasks and proposals each is bound to, read with one query per binding table.
async fn with_bindings(
    conn: &mut SqliteConnection,
    raw: Vec<RawProcess>,
) -> Result<Vec<WorkerProcessRow>, StoreError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let ids =
        serde_json::Value::from(raw.iter().map(|row| row.id).collect::<Vec<i64>>()).to_string();
    let mut tasks: HashMap<i64, Vec<TaskId>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT w.worker_process_id AS "worker!: i64", t.number AS "number!: i64"
           FROM worker_tasks w JOIN tasks t ON t.id = w.task_id
           WHERE w.worker_process_id IN (SELECT value FROM json_each(?1))
           ORDER BY w.worker_process_id, t.number"#,
        ids
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let task = TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?;
        tasks.entry(row.worker).or_default().push(task);
    }
    let mut proposals: HashMap<i64, Vec<ProposalId>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT w.worker_process_id AS "worker!: i64", p.number AS "number!: i64"
           FROM worker_proposals w JOIN proposals p ON p.id = w.proposal_id
           WHERE w.worker_process_id IN (SELECT value FROM json_each(?1))
           ORDER BY w.worker_process_id, p.number"#,
        ids
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let proposal =
            ProposalId::try_from_db(row.number).ok_or(StoreError::Corrupt("proposals.number"))?;
        proposals.entry(row.worker).or_default().push(proposal);
    }
    raw.into_iter()
        .map(|raw| {
            let mut row = raw.into_row()?;
            row.tasks = tasks.remove(&row.id).unwrap_or_default();
            row.proposals = proposals.remove(&row.id).unwrap_or_default();
            Ok(row)
        })
        .collect()
}

/// The worker process of `agent`, or `None` for an agent fold does not supervise.
pub async fn find_worker_process(
    conn: &mut SqliteConnection,
    agent: &AgentName,
) -> Result<Option<WorkerProcessRow>, StoreError> {
    let name = agent.as_str();
    let raw = select_processes!("WHERE g.name = ?", name)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(with_bindings(conn, raw.into_iter().collect()).await?.pop())
}

/// Every worker process in start order.
pub async fn list_worker_processes(
    conn: &mut SqliteConnection,
) -> Result<Vec<WorkerProcessRow>, StoreError> {
    let raw = select_processes!("ORDER BY w.id")
        .fetch_all(&mut *conn)
        .await?;
    with_bindings(conn, raw).await
}

/// The time each supervised agent's process last started: a start, a resume or a hand-off moves it.
pub async fn process_starts(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<(AgentName, Timestamp)>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT g.name AS agent, w.started_at
           FROM worker_processes w JOIN agents g ON g.id = w.agent_id
           ORDER BY w.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let agent =
                AgentName::parse(&row.agent).map_err(|_| StoreError::Corrupt("agents.name"))?;
            Ok((agent, time(row.started_at, "worker_processes.started_at")?))
        })
        .collect()
}

/// The agent of the project's review lead, the first one started, or `None` before any.
pub async fn review_lead(
    executor: impl SqliteExecutor<'_>,
) -> Result<Option<AgentName>, StoreError> {
    let name = sqlx::query_scalar!(
        "SELECT g.name FROM worker_processes w JOIN agents g ON g.id = w.agent_id
         WHERE w.role = 'review_lead' ORDER BY w.id LIMIT 1"
    )
    .fetch_optional(executor)
    .await?;
    name.map(|name| AgentName::parse(&name).map_err(|_| StoreError::Corrupt("agents.name")))
        .transpose()
}

/// How many worker processes of `role` are running, in the whole project and in area `area_id`. Running is [`WorkerState::is_running`]: a `stopping` worker still has its process, so it holds its place under the limits until it exits.
pub async fn running_counts(
    executor: impl SqliteExecutor<'_>,
    role: Role,
    area_id: Option<i64>,
) -> Result<(u64, u64), StoreError> {
    let role = role.as_str();
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "total!: i64",
                  COALESCE(SUM(area_id IS ?2), 0) AS "in_area!: i64"
           FROM worker_processes WHERE role = ?1 AND state NOT IN ('stopped', 'failed')"#,
        role,
        area_id
    )
    .fetch_one(executor)
    .await?;
    Ok((
        count(row.total, "worker_processes")?,
        count(row.in_area, "worker_processes")?,
    ))
}

/// Moves the row to `state`. `idle_since` follows an idle state and `stopped_at` a final one, as the table's checks require, and a running state ends the park.
pub async fn set_worker_state(
    tx: &mut WriteTx,
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
             stopped_at = CASE WHEN ?2 IN ('stopped', 'failed') THEN ?3 ELSE NULL END,
             failed_reason = CASE WHEN ?2 = 'failed' THEN failed_reason ELSE NULL END,
             parked_by = CASE WHEN ?2 IN ('stopping', 'stopped', 'failed') THEN parked_by ELSE NULL END,
             parked_for = CASE WHEN ?2 IN ('stopping', 'stopped', 'failed') THEN parked_for ELSE NULL END,
             parked_at = CASE WHEN ?2 IN ('stopping', 'stopped', 'failed') THEN parked_at ELSE NULL END
         WHERE id = ?1",
        id,
        state,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A turn began: a starting or idle worker is working. A stopping or ended worker keeps its state.
pub async fn mark_worker_working(tx: &mut WriteTx, id: i64) -> Result<bool, StoreError> {
    let result = sqlx::query!(
        "UPDATE worker_processes SET state = 'working', idle_since = NULL
         WHERE id = ? AND state IN ('starting', 'idle')",
        id
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Records that the row's current session read the role text of `fingerprint`.
pub async fn set_fold_fingerprint(
    tx: &mut WriteTx,
    id: i64,
    fingerprint: &str,
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE worker_processes SET fold_fingerprint = ? WHERE id = ?",
        fingerprint,
        id
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub async fn set_worker_pid(tx: &mut WriteTx, id: i64, pid: Option<u32>) -> Result<(), StoreError> {
    let pid = pid.map(i64::from);
    sqlx::query!("UPDATE worker_processes SET pid = ? WHERE id = ?", pid, id)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// Records that a human stopped the supervised main of row `id`, which must be stopping or ended: fold then starts it for no work of its own until a start of the row clears the mark.
pub async fn hold_worker(tx: &mut WriteTx, id: i64, now: Timestamp) -> Result<(), StoreError> {
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes SET held_at = ?2 WHERE id = ?1",
        id,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Records that the agent `by_agent_id` parked the stopping or ended row `id` at `now` to wait for its go, for `wait_for`. A later park replaces an earlier one.
pub async fn park_worker(
    tx: &mut WriteTx,
    id: i64,
    by_agent_id: i64,
    wait_for: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes SET parked_by = ?2, parked_for = ?3, parked_at = ?4 WHERE id = ?1",
        id,
        by_agent_id,
        wait_for,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The workers, by `workers.id`, whose agent's row is parked, each with whether the agent that parked it runs: its process runs, or it has an open session. Their claims hold their lease while it runs.
pub async fn parked_holders(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<(i64, bool)>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT w.id AS "worker!: i64",
                  (EXISTS (SELECT 1 FROM worker_processes q
                           WHERE q.agent_id = p.parked_by AND q.state IN ('starting', 'working', 'idle', 'stopping'))
                   OR EXISTS (SELECT 1 FROM sessions s
                              WHERE s.agent_id = p.parked_by AND s.ended_at IS NULL)) AS "parker_runs!: bool"
           FROM worker_processes p JOIN workers w ON w.agent_id = p.agent_id
           WHERE p.parked_at IS NOT NULL
           ORDER BY w.id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.worker, row.parker_runs))
        .collect())
}

/// Whether a human stopped row `id` and nothing started it since.
pub async fn worker_held<'e>(db: impl SqliteExecutor<'e>, id: i64) -> Result<bool, StoreError> {
    let held = sqlx::query_scalar!(
        r#"SELECT held_at IS NOT NULL AS "held!: bool" FROM worker_processes WHERE id = ?"#,
        id
    )
    .fetch_optional(db)
    .await?;
    Ok(held.unwrap_or(false))
}

/// Forgets the pid of the row's ended process `pid`. A row that records another pid keeps it: a new start of the row can run while the process it let go of is still ending.
pub async fn clear_worker_pid(tx: &mut WriteTx, id: i64, pid: u32) -> Result<(), StoreError> {
    let pid = i64::from(pid);
    sqlx::query!(
        "UPDATE worker_processes SET pid = NULL WHERE id = ?1 AND pid = ?2",
        id,
        pid
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The model Claude Code reports, which can resolve an alias, and the context size of its newest request.
pub async fn set_worker_context(
    tx: &mut WriteTx,
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
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Forgets a model name that Claude Code made up, such as `<synthetic>`, which no start asked for; a real name stays.
pub async fn forget_made_up_model(tx: &mut WriteTx, id: i64) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE worker_processes SET model = NULL WHERE id = ? AND model LIKE '<%'",
        id
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Counts one finished turn, appends its `worker_turns` row with the cost [`fold_core::cost::session_turn`] finds in the reported total, adds that cost to the row and makes the worker idle.
pub async fn record_turn(
    tx: &mut WriteTx,
    id: i64,
    totals: TurnTotals,
    now: Timestamp,
) -> Result<(), StoreError> {
    let before = sqlx::query!(
        r#"SELECT cost_micro_usd AS "cost!: i64", session_cost_micro_usd AS "session!: i64"
           FROM worker_processes WHERE id = ?"#,
        id
    )
    .fetch_optional(tx.conn())
    .await?;
    let Some(before) = before else {
        return Ok(());
    };
    let (added, session) = fold_core::cost::session_turn(
        totals.reported_micro_usd,
        count(before.session, "worker_processes.session_cost_micro_usd")?,
    );
    super::turns::insert_turn(tx, id, added, &totals, now).await?;
    let cost = db_count(
        count::<u64>(before.cost, "worker_processes.cost_micro_usd")?.saturating_add(added),
    );
    let session = db_count(session);
    let read = db_count(totals.cache_read_tokens);
    let write = db_count(totals.cache_write_tokens);
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes
         SET turns = turns + 1, cost_micro_usd = ?2, session_cost_micro_usd = ?3,
             cache_read_tokens = cache_read_tokens + ?4, cache_write_tokens = cache_write_tokens + ?5,
             state = CASE WHEN state IN ('starting', 'working') THEN 'idle' ELSE state END,
             idle_since = CASE WHEN state IN ('starting', 'working') THEN ?6 ELSE idle_since END
         WHERE id = ?1",
        id,
        cost,
        session,
        read,
        write,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Records a turn that ended in Claude Code's `error`: it becomes the row's last error, and the count of failed turns in a row grows by one, or starts again at 0 for a turn that `reached_model`. A row that is stopping or has ended is left as it is, because the interrupt of a stop ends its turn in an error too. Returns the new count, or `None` for a row left as it was.
pub async fn record_turn_error(
    tx: &mut WriteTx,
    id: i64,
    error: &str,
    reached_model: bool,
    now: Timestamp,
) -> Result<Option<u64>, StoreError> {
    let now = now.as_millis();
    let turns = sqlx::query_scalar!(
        r#"UPDATE worker_processes
           SET last_error = ?2, last_error_at = ?3,
               failed_turns = CASE WHEN ?4 THEN 0 ELSE failed_turns + 1 END
           WHERE id = ?1 AND state IN ('starting', 'working', 'idle')
           RETURNING failed_turns AS "failed_turns!: i64""#,
        id,
        error,
        now,
        reached_model
    )
    .fetch_optional(tx.conn())
    .await?;
    turns
        .map(|turns| count(turns, "worker_processes.failed_turns"))
        .transpose()
}

/// A turn ended without an error, so the worker reaches the model: the last error goes and the count of failed turns starts again.
pub async fn clear_turn_error(tx: &mut WriteTx, id: i64) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE worker_processes SET last_error = NULL, last_error_at = NULL, failed_turns = 0
         WHERE id = ?",
        id
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Makes `error`, the stderr of a process that exited with an error, the row's last error.
pub async fn set_exit_error(
    tx: &mut WriteTx,
    id: i64,
    error: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes SET last_error = ?2, last_error_at = ?3 WHERE id = ?1",
        id,
        error,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// How a worker's process ended: `stopped`, or `failed` for a reason such as `exited`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEnd<'a> {
    Stopped,
    Failed(&'a str),
}

/// Ends the process of the row as `end` says, with its exit code.
pub async fn record_exit(
    tx: &mut WriteTx,
    id: i64,
    end: ProcessEnd<'_>,
    exit_code: Option<i32>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (state, reason) = match end {
        ProcessEnd::Stopped => (WorkerState::Stopped.as_str(), None),
        ProcessEnd::Failed(reason) => (WorkerState::Failed.as_str(), Some(reason)),
    };
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes SET state = ?2, stopped_at = ?3, idle_since = NULL, pid = NULL, exit_code = ?4,
             failed_reason = ?5
         WHERE id = ?1",
        id,
        state,
        now,
        exit_code,
        reason
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Starts the row again for a new process of the same session.
pub async fn restart_worker_process(
    tx: &mut WriteTx,
    id: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE worker_processes
         SET state = 'starting', started_at = ?2, stopped_at = NULL, idle_since = NULL, pid = NULL, exit_code = NULL,
             failed_reason = NULL, held_at = NULL, parked_by = NULL, parked_for = NULL, parked_at = NULL
         WHERE id = ?1",
        id,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Records on the failed row `id` that its Claude Code session is gone, as the failure reason `reason`, so a message to it goes on to a lead without another `--resume`. The next start of the row clears it.
pub async fn mark_session_lost(tx: &mut WriteTx, id: i64, reason: &str) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE worker_processes SET failed_reason = ?2 WHERE id = ?1 AND state = 'failed'",
        id,
        reason
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A running worker process bound to a task or proposal: its row id and its agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundWorker {
    pub id: i64,
    pub agent: AgentName,
}

/// The running task workers bound to `task`. A `stopping` task worker is left out with the ended ones: it takes no more work, so nothing waits for it.
pub async fn task_workers_of_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Vec<BoundWorker>, StoreError> {
    let number = task.to_db();
    let rows = sqlx::query!(
        r#"SELECT w.id AS "id!: i64", g.name FROM worker_tasks b
           JOIN tasks t ON t.id = b.task_id
           JOIN worker_processes w ON w.id = b.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           WHERE t.number = ? AND w.state NOT IN ('stopped', 'failed', 'stopping')
           ORDER BY w.id"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BoundWorker {
                id: row.id,
                agent: AgentName::parse(&row.name)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
            })
        })
        .collect()
}

/// The running reviewers bound to `proposal`. A `stopping` reviewer is left out with the ended ones: it takes no more work, so nothing waits for it.
pub async fn reviewers_of_proposal(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<BoundWorker>, StoreError> {
    let number = proposal.to_db();
    let rows = sqlx::query!(
        r#"SELECT w.id AS "id!: i64", g.name FROM worker_proposals b
           JOIN proposals p ON p.id = b.proposal_id
           JOIN worker_processes w ON w.id = b.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           WHERE p.number = ? AND w.state NOT IN ('stopped', 'failed', 'stopping')
           ORDER BY w.id"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BoundWorker {
                id: row.id,
                agent: AgentName::parse(&row.name)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
            })
        })
        .collect()
}

/// How many bound tasks of each worker in `worker_process_ids` still wait for its work; every id has an entry. A task is finished when it is Done, when it is archived, when it has an integrated proposal (a proposal can integrate before fold looks), when it is a task without a proposal whose report waits for a lead check, or when it has an open proposal that is not conflicted, whose integration checks did not fail and whose latest verdict on its newest version is not `request_changes`, read with the rule `reviews::get_proposal` uses. A requested change, a conflict or failed integration checks therefore keep the task worker bound until it sends the next version.
pub async fn tasks_awaiting_proposal(
    executor: impl SqliteExecutor<'_>,
    worker_process_ids: &[i64],
) -> Result<HashMap<i64, u64>, StoreError> {
    let ids = id_list(worker_process_ids);
    let rows = sqlx::query!(
        r#"SELECT b.worker_process_id AS "worker!: i64", COUNT(*) AS "count!: i64"
           FROM worker_tasks b JOIN tasks t ON t.id = b.task_id
           WHERE b.worker_process_id IN (SELECT value FROM json_each(?1)) AND t.status <> 'done' AND t.archived_at IS NULL
             AND NOT EXISTS (SELECT 1 FROM proposals p WHERE p.task_id = b.task_id AND p.state = 'integrated')
             AND NOT EXISTS (SELECT 1 FROM task_reports r WHERE r.task_id = b.task_id AND r.resolution IS NULL)
             AND NOT EXISTS (
                 SELECT 1 FROM proposals p
                 WHERE p.task_id = b.task_id AND p.state = 'open' AND NOT p.conflicted AND NOT p.checks_failed
                   AND COALESCE((
                       SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id
                   ), '') <> 'request_changes'
             )
           GROUP BY b.worker_process_id"#,
        ids
    )
    .fetch_all(executor)
    .await?;
    counts_by_worker(
        worker_process_ids,
        rows.into_iter().map(|row| (row.worker, row.count)),
        "worker_tasks",
    )
}

/// How many unreleased claims, of either kind, the workers of each worker process's agent in `worker_process_ids` hold; every id has an entry.
pub async fn claims_held(
    executor: impl SqliteExecutor<'_>,
    worker_process_ids: &[i64],
) -> Result<HashMap<i64, u64>, StoreError> {
    let ids = id_list(worker_process_ids);
    let rows = sqlx::query!(
        r#"SELECT w.id AS "worker!: i64", COUNT(*) AS "count!: i64"
           FROM worker_processes w
           JOIN workers k ON k.agent_id = w.agent_id
           JOIN claims c ON c.worker_id = k.id AND c.released_at IS NULL
           WHERE w.id IN (SELECT value FROM json_each(?1))
           GROUP BY w.id"#,
        ids
    )
    .fetch_all(executor)
    .await?;
    counts_by_worker(
        worker_process_ids,
        rows.into_iter().map(|row| (row.worker, row.count)),
        "claims",
    )
}

/// The newest version of an open proposal that waits on its author, a task worker, for a change that task worker has not been started for since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingResume {
    pub agent: AgentName,
    pub proposal: ProposalId,
    pub version: u32,
    /// The latest verdict on the version requests changes, given after the task worker last started.
    pub changes: bool,
    /// The proposal is conflicted, by an integration of the version after the task worker last started.
    pub conflict: bool,
    /// The integration checks of the version failed after the task worker last started.
    pub checks: bool,
}

/// The requested changes, conflicts and failed integration checks that no start of their author has served, for task workers in one of `states`: each start moves the row's `started_at`, so a change recorded later than `started_at` is still owed. A resume that the worker limits refused leaves the row as it was, so it stays here until a start succeeds. For a running task worker it is the change its current process has been running beside.
pub async fn pending_resumes(
    executor: impl SqliteExecutor<'_>,
    states: &[WorkerState],
) -> Result<Vec<PendingResume>, StoreError> {
    let states = serde_json::Value::from(
        states
            .iter()
            .map(|state| state.as_str())
            .collect::<Vec<_>>(),
    )
    .to_string();
    let rows = sqlx::query!(
        r#"WITH owed AS (
               SELECT w.id AS worker, g.name AS agent, p.number AS proposal, v.version AS version,
                      COALESCE((
                          SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id
                      ), '') = 'request_changes'
                      AND EXISTS (
                          SELECT 1 FROM reviews r
                          WHERE r.version_id = v.id AND r.verdict = 'request_changes' AND r.created_at > w.started_at
                      ) AS changes,
                      p.conflicted AND EXISTS (
                          SELECT 1 FROM integrations i
                          WHERE i.version_id = v.id AND i.result = 'conflicted' AND i.created_at > w.started_at
                      ) AS conflict,
                      p.checks_failed AND EXISTS (
                          SELECT 1 FROM check_runs r
                          WHERE r.version_id = v.id AND r.stage = 'integration' AND r.state = 'failed'
                            AND r.finished_at > w.started_at
                      ) AS checks
               FROM worker_processes w
               JOIN agents g ON g.id = w.agent_id
               JOIN worker_tasks b ON b.worker_process_id = w.id
               JOIN proposals p ON p.task_id = b.task_id AND p.state = 'open'
               JOIN workers k ON k.id = p.author_worker_id AND k.agent_id = w.agent_id
               JOIN proposal_versions v ON v.proposal_id = p.id
               WHERE w.role = 'task_worker' AND w.state IN (SELECT value FROM json_each(?1))
                 AND v.version = (SELECT MAX(x.version) FROM proposal_versions x WHERE x.proposal_id = p.id)
           )
           SELECT agent AS "agent!: String", proposal AS "proposal!: i64", version AS "version!: i64",
                  changes AS "changes!: bool", conflict AS "conflict!: bool", checks AS "checks!: bool"
           FROM owed WHERE changes OR conflict OR checks
           ORDER BY worker, proposal"#,
        states
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(PendingResume {
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                version: count(row.version, "proposal_versions.version")?,
                changes: row.changes,
                conflict: row.conflict,
                checks: row.checks,
            })
        })
        .collect()
}

/// The newest version of an open proposal that waits on a reviewer that has ended: the reviewer is bound to the proposal and its agent still holds the unreleased review claim, the version has no verdict, and the version came, or a human sent it for review again, after the reviewer last started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedReview {
    pub agent: AgentName,
    pub proposal: ProposalId,
    pub version: u32,
}

/// The reviews owed to stopped or failed reviewers, by reviewer and proposal. Each start moves the row's `started_at`, so a version that a resumed reviewer did not review before it stopped again is not owed a second time; a resume that the worker limits or a pause refused leaves the row as it was, so the review stays owed until a start succeeds. With `proposal`, only that proposal, whose new version an event just announced, whatever the times: the event itself says the version came after the reviewer's verdict.
pub async fn reviews_owed_to_ended_reviewers(
    executor: impl SqliteExecutor<'_>,
    proposal: Option<ProposalId>,
) -> Result<Vec<OwedReview>, StoreError> {
    let number = proposal.map(ProposalId::to_db);
    let rows = sqlx::query!(
        r#"SELECT g.name AS "agent!: String", p.number AS "proposal!: i64", v.version AS "version!: i64"
           FROM worker_processes w
           JOIN agents g ON g.id = w.agent_id
           JOIN worker_proposals b ON b.worker_process_id = w.id
           JOIN proposals p ON p.id = b.proposal_id AND p.state = 'open'
           JOIN proposal_versions v ON v.proposal_id = p.id
           JOIN claims c ON c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL
           JOIN workers k ON k.id = c.worker_id AND k.agent_id = w.agent_id
           WHERE w.role = 'reviewer' AND w.state IN ('stopped', 'failed')
             AND v.version = (SELECT MAX(x.version) FROM proposal_versions x WHERE x.proposal_id = p.id)
             AND (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) IS NULL
             AND CASE WHEN ?1 IS NULL
                      THEN v.created_at > w.started_at
                           OR EXISTS (SELECT 1 FROM review_again_requests q
                                      WHERE q.version_id = v.id AND q.created_at > w.started_at)
                      ELSE p.number = ?1 END
           ORDER BY w.id, p.number"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OwedReview {
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                version: count(row.version, "proposal_versions.version")?,
            })
        })
        .collect()
}

/// A resume that fold owes a stopped or failed task worker or reviewer, with the role it starts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedStart {
    pub role: Role,
    pub resume: fold_core::roles::OwedResume,
}

/// Every resume that fold owes a stopped or failed task worker or reviewer, one per worker, oldest first: the requested changes, conflicts and failed integration checks of [`pending_resumes`], the send-backs of `reports::owed_send_backs`, the failed proposal checks of `checks::owed_gate_runs` and the reviews of [`reviews_owed_to_ended_reviewers`], each dated by the time its work came after the worker last started. A worker owed several is dated by its oldest. Each comes with the running workers of its role in its area, so the limits can say whether it can take a place.
pub async fn owed_starts(executor: impl SqliteExecutor<'_>) -> Result<Vec<OwedStart>, StoreError> {
    let rows = sqlx::query!(
        r#"WITH latest AS (
               SELECT p.id AS proposal_id, v.id AS version_id, v.created_at AS created_at
               FROM proposals p JOIN proposal_versions v ON v.proposal_id = p.id
               WHERE p.state = 'open'
                 AND v.version = (SELECT MAX(x.version) FROM proposal_versions x WHERE x.proposal_id = p.id)
           ),
           authored AS (
               SELECT w.id AS worker, w.started_at AS started_at, p.id AS proposal_id, p.number AS proposal,
                      t.number AS task, l.version_id AS version_id, p.conflicted AS conflicted,
                      p.checks_failed AS checks_failed,
                      COALESCE((SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id), '') AS verdict
               FROM worker_processes w
               JOIN worker_tasks b ON b.worker_process_id = w.id
               JOIN proposals p ON p.task_id = b.task_id AND p.state = 'open'
               JOIN tasks t ON t.id = p.task_id
               JOIN workers k ON k.id = p.author_worker_id AND k.agent_id = w.agent_id
               JOIN latest l ON l.proposal_id = p.id
               WHERE w.role = 'task_worker' AND w.state IN ('stopped', 'failed')
           ),
           items(worker, since, task, proposal) AS (
               SELECT a.worker,
                      (SELECT MIN(r.created_at) FROM reviews r
                       WHERE r.version_id = a.version_id AND r.verdict = 'request_changes' AND r.created_at > a.started_at),
                      a.task, a.proposal
               FROM authored a WHERE a.verdict = 'request_changes'
               UNION ALL
               SELECT a.worker,
                      (SELECT MIN(i.created_at) FROM integrations i
                       WHERE i.version_id = a.version_id AND i.result = 'conflicted' AND i.created_at > a.started_at),
                      a.task, a.proposal
               FROM authored a WHERE a.conflicted
               UNION ALL
               SELECT a.worker,
                      (SELECT MIN(r.finished_at) FROM check_runs r
                       WHERE r.version_id = a.version_id AND r.stage = 'integration' AND r.state = 'failed'
                         AND r.finished_at > a.started_at),
                      a.task, a.proposal
               FROM authored a WHERE a.checks_failed
               UNION ALL
               SELECT w.id, r.resolved_at, t.number, NULL
               FROM task_reports r
               JOIN tasks t ON t.id = r.task_id
               JOIN workers k ON k.id = r.worker_id
               JOIN worker_processes w ON w.agent_id = k.agent_id
               WHERE r.resolution = 'sent_back' AND r.resolved_at > w.started_at
                 AND w.role = 'task_worker' AND w.state IN ('stopped', 'failed')
                 AND t.archived_at IS NULL
                 AND r.id = (SELECT MAX(x.id) FROM task_reports x WHERE x.task_id = r.task_id)
               UNION ALL
               SELECT w.id, c.finished_at, t.number, NULL
               FROM check_runs c
               JOIN tasks t ON t.id = c.task_id
               JOIN workers k ON k.id = c.worker_id
               JOIN worker_processes w ON w.agent_id = k.agent_id
               WHERE c.stage = 'proposal' AND c.purpose IN ('create_proposal', 'update_proposal')
                 AND c.state IN ('failed', 'stopped') AND c.finished_at > w.started_at
                 AND w.role = 'task_worker' AND w.state IN ('stopped', 'failed')
                 AND c.id = (
                     SELECT MAX(x.id) FROM check_runs x
                     WHERE x.workspace_id = c.workspace_id AND x.state <> 'running'
                 )
               UNION ALL
               SELECT w.id,
                      CASE WHEN l.created_at > w.started_at THEN l.created_at
                           ELSE (SELECT MIN(q.created_at) FROM review_again_requests q
                                 WHERE q.version_id = l.version_id AND q.created_at > w.started_at) END,
                      t.number, p.number
               FROM worker_processes w
               JOIN worker_proposals b ON b.worker_process_id = w.id
               JOIN proposals p ON p.id = b.proposal_id AND p.state = 'open'
               JOIN tasks t ON t.id = p.task_id
               JOIN latest l ON l.proposal_id = p.id
               JOIN claims c ON c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL
               JOIN workers k ON k.id = c.worker_id AND k.agent_id = w.agent_id
               WHERE w.role = 'reviewer' AND w.state IN ('stopped', 'failed')
                 AND (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) IS NULL
           )
           SELECT g.name AS "agent!: String", w.role AS "role!: String", ar.name AS "area?: String",
                  (SELECT COUNT(*) FROM worker_processes y
                   WHERE y.role = w.role AND y.area_id IS w.area_id AND y.state NOT IN ('stopped', 'failed'))
                      AS "area_running!: i64",
                  i.since AS "since!: i64", i.task AS "task?: i64", i.proposal AS "proposal?: i64"
           FROM items i
           JOIN worker_processes w ON w.id = i.worker
           JOIN agents g ON g.id = w.agent_id
           LEFT JOIN areas ar ON ar.id = w.area_id
           WHERE i.since IS NOT NULL
           ORDER BY i.since, g.name, i.task, i.proposal"#
    )
    .fetch_all(executor)
    .await?;
    let mut owed: Vec<OwedStart> = Vec::new();
    for row in rows {
        let agent = AgentName::parse(&row.agent).map_err(|_| StoreError::Corrupt("agents.name"))?;
        if owed.iter().any(|seen| seen.resume.agent == agent) {
            continue;
        }
        let area = match row.area {
            Some(name) => Some((
                AreaName::parse(&name).map_err(|_| StoreError::Corrupt("areas.name"))?,
                count(row.area_running, "worker_processes")?,
            )),
            None => None,
        };
        owed.push(OwedStart {
            role: Role::parse(&row.role).ok_or(StoreError::Corrupt("worker_processes.role"))?,
            resume: fold_core::roles::OwedResume {
                agent,
                area,
                since: time(row.since, "owed resume time")?,
                task: row
                    .task
                    .map(|task| {
                        TaskId::try_from_db(task).ok_or(StoreError::Corrupt("tasks.number"))
                    })
                    .transpose()?,
                proposal: row
                    .proposal
                    .map(|number| {
                        ProposalId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("proposals.number"))
                    })
                    .transpose()?,
            },
        });
    }
    Ok(owed)
}

/// The open proposals, in number order, whose unreleased review claim a worker of `agent` holds while its reviewer process has ended, and whose newest version has no verdict. A comment is not a verdict.
pub async fn unreviewed_claims_of_ended_reviewer(
    executor: impl SqliteExecutor<'_>,
    agent: &AgentName,
) -> Result<Vec<ProposalId>, StoreError> {
    let name = agent.as_str();
    let rows = sqlx::query_scalar!(
        r#"SELECT p.number AS "proposal!: i64"
           FROM worker_processes w
           JOIN agents g ON g.id = w.agent_id
           JOIN workers k ON k.agent_id = w.agent_id
           JOIN claims c ON c.worker_id = k.id AND c.kind = 'review' AND c.released_at IS NULL
           JOIN proposals p ON p.id = c.proposal_id AND p.state = 'open'
           WHERE g.name = ?1 AND w.role = 'reviewer' AND w.state IN ('stopped', 'failed')
             AND (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) IS NULL
           ORDER BY p.number"#,
        name
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|number| {
            ProposalId::try_from_db(number).ok_or(StoreError::Corrupt("proposals.number"))
        })
        .collect()
}

/// How many bound proposals of each worker in `worker_process_ids` are open and have no verdict on their newest version; every id has an entry. A comment is not a verdict.
pub async fn proposals_without_verdict(
    executor: impl SqliteExecutor<'_>,
    worker_process_ids: &[i64],
) -> Result<HashMap<i64, u64>, StoreError> {
    let ids = id_list(worker_process_ids);
    let rows = sqlx::query!(
        r#"SELECT b.worker_process_id AS "worker!: i64", COUNT(*) AS "count!: i64"
           FROM worker_proposals b
           JOIN proposals p ON p.id = b.proposal_id
           WHERE b.worker_process_id IN (SELECT value FROM json_each(?1)) AND p.state = 'open'
             AND (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) IS NULL
           GROUP BY b.worker_process_id"#,
        ids
    )
    .fetch_all(executor)
    .await?;
    counts_by_worker(
        worker_process_ids,
        rows.into_iter().map(|row| (row.worker, row.count)),
        "worker_proposals",
    )
}

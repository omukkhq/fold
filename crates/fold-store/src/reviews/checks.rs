use fold_core::object::ObjectHash;
use fold_core::reviews::checks::{Outcome, Purpose, RunReason, RunState, Stage};
use fold_core::{
    CheckRunId, ProposalId, ReviewBatchId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId,
};
use sqlx::SqliteExecutor;

use crate::columns::{count, optional_time, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// A run to insert as `running`. The ids are public ones; the rows they name must exist.
#[derive(Debug, Clone)]
pub struct NewCheckRun<'a> {
    pub id: CheckRunId,
    pub stage: Stage,
    pub purpose: Purpose,
    pub task: TaskId,
    pub workspace: Option<WorkspaceId>,
    /// The row id of the requesting worker.
    pub worker_id: Option<i64>,
    pub proposal: Option<ProposalId>,
    /// The version number of `proposal` the run checks, for an integration run.
    pub version: Option<u32>,
    pub base: RevisionId,
    pub tree: ObjectHash,
    pub actor: &'a str,
    pub title: Option<&'a str>,
    pub evidence: Option<&'a str>,
    /// The submitted review batch whose ready members an integration run checks together.
    pub batch: Option<ReviewBatchId>,
}

/// One stored run with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRunRow {
    pub id: CheckRunId,
    pub stage: Stage,
    pub purpose: Purpose,
    pub task: TaskId,
    pub workspace: Option<WorkspaceId>,
    pub worker: Option<WorkerName>,
    pub proposal: Option<ProposalId>,
    pub version: Option<u32>,
    pub base: RevisionId,
    pub tree: ObjectHash,
    pub actor: String,
    pub title: Option<String>,
    pub evidence: Option<String>,
    pub state: RunState,
    pub reason: Option<RunReason>,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// The review batch whose members the integration run checked together.
    pub batch: Option<ReviewBatchId>,
}

/// One command of a run as the runner reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCheckResult<'a> {
    pub position: u32,
    /// `None` for the setup command.
    pub check: Option<&'a str>,
    pub required: bool,
    pub outcome: Outcome,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub log: &'a [u8],
    pub log_bytes: u64,
}

/// One stored command result. `log` is empty when the read left the logs out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResultRow {
    pub position: u32,
    /// `None` for the setup command.
    pub check: Option<String>,
    pub required: bool,
    pub outcome: Outcome,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub log: Vec<u8>,
    pub log_bytes: u64,
    pub truncated: bool,
}

/// The name a setup result is stored under.
const SETUP: &str = "setup";

/// Inserts a running run in the caller's write. A second running run of the same workspace, or a second running integration run, fails on its unique index, so the database decides a race.
pub async fn insert_run(
    tx: &mut WriteTx,
    new: &NewCheckRun<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, task) = (new.id.to_db(), new.task.to_db());
    let workspace = new.workspace.map(WorkspaceId::to_db);
    let proposal = new.proposal.map(ProposalId::to_db);
    let version = new.version.map(i64::from);
    let (stage, purpose) = (new.stage.as_str(), new.purpose.as_str());
    let (base, tree, now) = (new.base.to_db(), new.tree.to_string(), now.as_millis());
    let batch = new.batch.map(ReviewBatchId::to_db);
    let inserted = sqlx::query!(
        "INSERT INTO check_runs (number, stage, purpose, task_id, workspace_id, worker_id, proposal_id,
                                 version_id, base_revision_id, tree_hash, actor, title, evidence, created_at,
                                 review_batch_id)
         SELECT ?1, ?2, ?3, t.id,
                (SELECT w.id FROM workspaces w WHERE w.number = ?4),
                ?5,
                (SELECT p.id FROM proposals p WHERE p.number = ?6),
                (SELECT v.id FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
                 WHERE p.number = ?6 AND v.version = ?7),
                r.id, ?9, ?10, ?11, ?12, ?13,
                (SELECT b.id FROM review_batches b WHERE b.number = ?15)
         FROM tasks t, revisions r WHERE t.number = ?14 AND r.number = ?8",
        number,
        stage,
        purpose,
        workspace,
        new.worker_id,
        proposal,
        version,
        base,
        tree,
        new.actor,
        new.title,
        new.evidence,
        now,
        task,
        batch
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the task or base revision does not exist",
        ))
    }
}

/// Names, in the caller's write, the version of each member that a batch's integration run checks together; `versions` are proposal version row ids.
pub async fn insert_run_members(
    tx: &mut WriteTx,
    run: CheckRunId,
    versions: &[i64],
) -> Result<(), StoreError> {
    let number = run.to_db();
    for version in versions {
        sqlx::query!(
            "INSERT INTO check_run_members (run_id, version_id)
             SELECT r.id, ?2 FROM check_runs r WHERE r.number = ?1",
            number,
            version
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Writes one result of a running run in the caller's write; the run's end follows in the same write.
pub async fn insert_result(
    tx: &mut WriteTx,
    run: CheckRunId,
    result: &NewCheckResult<'_>,
) -> Result<(), StoreError> {
    let number = run.to_db();
    let position = i64::from(result.position);
    let kind = if result.check.is_some() {
        "check"
    } else {
        SETUP
    };
    let name = result.check.unwrap_or(SETUP);
    let outcome = result.outcome.as_str();
    let duration = i64::try_from(result.duration_ms).unwrap_or(i64::MAX);
    let log_bytes = i64::try_from(result.log_bytes).unwrap_or(i64::MAX);
    let truncated = result.log_bytes > result.log.len() as u64;
    let inserted = sqlx::query!(
        "INSERT INTO check_results (run_id, position, kind, name, required, outcome, exit_code,
                                    duration_ms, log, log_bytes, truncated)
         SELECT r.id, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11 FROM check_runs r WHERE r.number = ?1",
        number,
        position,
        kind,
        name,
        result.required,
        outcome,
        result.exit_code,
        duration,
        result.log,
        log_bytes,
        truncated
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the check run does not exist"))
    }
}

/// Ends a running run with `state` and `reason`. False, with nothing changed, when the run is not running.
pub async fn finish_run(
    tx: &mut WriteTx,
    run: CheckRunId,
    state: RunState,
    reason: Option<RunReason>,
    now: Timestamp,
) -> Result<bool, StoreError> {
    if state == RunState::Running {
        return Err(StoreError::Invalid(
            "a run ends in passed, failed or stopped",
        ));
    }
    let (number, state, now) = (run.to_db(), state.as_str(), now.as_millis());
    let reason = reason.map(RunReason::as_str);
    let updated = sqlx::query!(
        "UPDATE check_runs SET state = ?2, reason = ?3, finished_at = ?4
         WHERE number = ?1 AND state = 'running'",
        number,
        state,
        reason,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// One run row as the reads select it, before its columns are checked.
struct RawRun {
    number: i64,
    stage: String,
    purpose: String,
    task: i64,
    workspace: Option<i64>,
    worker: Option<String>,
    proposal: Option<i64>,
    version: Option<i64>,
    base: i64,
    tree_hash: String,
    actor: String,
    title: Option<String>,
    evidence: Option<String>,
    state: String,
    reason: Option<String>,
    created_at: i64,
    finished_at: Option<i64>,
    batch: Option<i64>,
}

impl RawRun {
    fn into_row(self) -> Result<CheckRunRow, StoreError> {
        let row = self;
        Ok(CheckRunRow {
            id: CheckRunId::try_from_db(row.number)
                .ok_or(StoreError::Corrupt("check_runs.number"))?,
            stage: Stage::parse(&row.stage).ok_or(StoreError::Corrupt("check_runs.stage"))?,
            purpose: Purpose::parse(&row.purpose)
                .ok_or(StoreError::Corrupt("check_runs.purpose"))?,
            task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
            workspace: row
                .workspace
                .map(|number| {
                    WorkspaceId::try_from_db(number).ok_or(StoreError::Corrupt("workspaces.number"))
                })
                .transpose()?,
            worker: row
                .worker
                .map(|name| {
                    WorkerName::parse(&name).map_err(|_| StoreError::Corrupt("workers.name"))
                })
                .transpose()?,
            proposal: row
                .proposal
                .map(|number| {
                    ProposalId::try_from_db(number).ok_or(StoreError::Corrupt("proposals.number"))
                })
                .transpose()?,
            version: row
                .version
                .map(|version| count(version, "proposal_versions.version"))
                .transpose()?,
            base: RevisionId::try_from_db(row.base)
                .ok_or(StoreError::Corrupt("revisions.number"))?,
            tree: row
                .tree_hash
                .parse()
                .map_err(|_| StoreError::Corrupt("check_runs.tree_hash"))?,
            actor: row.actor,
            title: row.title,
            evidence: row.evidence,
            state: RunState::parse(&row.state).ok_or(StoreError::Corrupt("check_runs.state"))?,
            reason: row
                .reason
                .map(|reason| {
                    RunReason::parse(&reason).ok_or(StoreError::Corrupt("check_runs.reason"))
                })
                .transpose()?,
            created_at: time(row.created_at, "check_runs.created_at")?,
            finished_at: optional_time(row.finished_at, "check_runs.finished_at")?,
            batch: row
                .batch
                .map(|number| {
                    ReviewBatchId::try_from_db(number)
                        .ok_or(StoreError::Corrupt("review_batches.number"))
                })
                .transpose()?,
        })
    }
}

/// The columns of [`RawRun`] and the joins that give them, followed by the tail of each read.
macro_rules! select_runs {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            RawRun,
            r#"SELECT c.number AS "number!: i64", c.stage, c.purpose,
                      t.number AS "task!: i64",
                      w.number AS "workspace?: i64",
                      k.name AS "worker?: String",
                      p.number AS "proposal?: i64",
                      v.version AS "version?: i64",
                      r.number AS "base!: i64",
                      c.tree_hash, c.actor, c.title, c.evidence, c.state, c.reason,
                      c.created_at, c.finished_at, b.number AS "batch?: i64"
               FROM check_runs c
               JOIN tasks t ON t.id = c.task_id
               JOIN revisions r ON r.id = c.base_revision_id
               LEFT JOIN workspaces w ON w.id = c.workspace_id
               LEFT JOIN workers k ON k.id = c.worker_id
               LEFT JOIN proposals p ON p.id = c.proposal_id
               LEFT JOIN proposal_versions v ON v.id = c.version_id
               LEFT JOIN review_batches b ON b.id = c.review_batch_id
               "# + $tail
            $(, $arg)*
        )
    };
}

fn rows(raw: Vec<RawRun>) -> Result<Vec<CheckRunRow>, StoreError> {
    raw.into_iter().map(RawRun::into_row).collect()
}

/// The run with this id.
pub async fn get_run(
    executor: impl SqliteExecutor<'_>,
    run: CheckRunId,
) -> Result<Option<CheckRunRow>, StoreError> {
    let number = run.to_db();
    let raw = select_runs!("WHERE c.number = ?1", number)
        .fetch_all(executor)
        .await?;
    Ok(rows(raw)?.pop())
}

/// The newest proposal-stage run of the workspace that has ended: what the tracker knows of the workspace's last run, its base and its tree.
pub async fn last_finished_proposal_run(
    executor: impl SqliteExecutor<'_>,
    workspace: WorkspaceId,
) -> Result<Option<CheckRunRow>, StoreError> {
    let number = workspace.to_db();
    let raw = select_runs!(
        "WHERE w.number = ?1 AND c.stage = 'proposal' AND c.state <> 'running'
         ORDER BY c.id DESC LIMIT 1",
        number
    )
    .fetch_all(executor)
    .await?;
    Ok(rows(raw)?.pop())
}

/// The running run of the workspace, if one is going.
pub async fn running_in_workspace(
    executor: impl SqliteExecutor<'_>,
    workspace: WorkspaceId,
) -> Result<Option<CheckRunRow>, StoreError> {
    let number = workspace.to_db();
    let raw = select_runs!("WHERE w.number = ?1 AND c.state = 'running'", number)
        .fetch_all(executor)
        .await?;
    Ok(rows(raw)?.pop())
}

/// The runs of one proposal version, newest first: the ended proposal-stage runs of its task on its base and tree, which checked exactly its files, the integration runs of the version, and the integration runs of a review batch that checked the version together with other members.
pub async fn runs_of_version(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
    proposal: ProposalId,
    version: u32,
) -> Result<Vec<CheckRunRow>, StoreError> {
    let (task, proposal, version) = (task.to_db(), proposal.to_db(), i64::from(version));
    let raw = select_runs!(
        "WHERE (t.number = ?1 AND (
               (c.stage = 'proposal' AND c.state <> 'running' AND EXISTS (
                   SELECT 1 FROM proposal_versions x JOIN proposals y ON y.id = x.proposal_id
                   WHERE y.number = ?2 AND x.version = ?3
                     AND x.base_revision_id = c.base_revision_id AND x.tree_hash = c.tree_hash
               ))
               OR (p.number = ?2 AND v.version = ?3)
           ))
           OR c.id IN (
               SELECT m.run_id FROM check_run_members m
               JOIN proposal_versions x ON x.id = m.version_id
               JOIN proposals y ON y.id = x.proposal_id
               WHERE y.number = ?2 AND x.version = ?3
           )
         ORDER BY c.id DESC",
        task,
        proposal,
        version
    )
    .fetch_all(executor)
    .await?;
    rows(raw)
}

/// The newest integration run of the review batch's members together, if one started.
pub async fn newest_batch_run(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Option<CheckRunRow>, StoreError> {
    let number = batch.to_db();
    let raw = select_runs!("WHERE b.number = ?1 ORDER BY c.id DESC LIMIT 1", number)
        .fetch_all(executor)
        .await?;
    Ok(rows(raw)?.pop())
}

/// The version row ids of the members a batch's run checked together.
pub async fn run_member_versions(
    executor: impl SqliteExecutor<'_>,
    run: CheckRunId,
) -> Result<Vec<i64>, StoreError> {
    let number = run.to_db();
    Ok(sqlx::query_scalar!(
        r#"SELECT m.version_id AS "version!: i64" FROM check_run_members m
           JOIN check_runs r ON r.id = m.run_id
           WHERE r.number = ?
           ORDER BY m.version_id"#,
        number
    )
    .fetch_all(executor)
    .await?)
}

/// Every run that has not ended, oldest first: after a daemon start, the runs a stop cut short.
pub async fn running_runs(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<CheckRunRow>, StoreError> {
    let raw = select_runs!("WHERE c.state = 'running' ORDER BY c.id")
        .fetch_all(executor)
        .await?;
    rows(raw)
}

/// The results of a run in run order, with their logs when `logs` is true.
pub async fn list_results(
    executor: impl SqliteExecutor<'_>,
    run: CheckRunId,
    logs: bool,
) -> Result<Vec<CheckResultRow>, StoreError> {
    let number = run.to_db();
    let raw = sqlx::query!(
        r#"SELECT s.position, s.kind, s.name, s.required AS "required: bool", s.outcome, s.exit_code,
                  s.duration_ms, CASE WHEN ?2 THEN s.log ELSE x'' END AS "log!: Vec<u8>",
                  s.log_bytes, s.truncated AS "truncated: bool"
           FROM check_results s JOIN check_runs c ON c.id = s.run_id
           WHERE c.number = ?1
           ORDER BY s.position"#,
        number,
        logs
    )
    .fetch_all(executor)
    .await?;
    raw.into_iter()
        .map(|row| {
            Ok(CheckResultRow {
                position: count(row.position, "check_results.position")?,
                check: (row.kind != SETUP).then_some(row.name),
                required: row.required,
                outcome: Outcome::parse(&row.outcome)
                    .ok_or(StoreError::Corrupt("check_results.outcome"))?,
                exit_code: row
                    .exit_code
                    .map(|code| count(code, "check_results.exit_code"))
                    .transpose()?,
                duration_ms: count(row.duration_ms, "check_results.duration_ms")?,
                log: row.log,
                log_bytes: count(row.log_bytes, "check_results.log_bytes")?,
                truncated: row.truncated,
            })
        })
        .collect()
}

/// One integration that waits in a project's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRequestRow {
    /// The row id: the queue serves requests in this order.
    pub id: i64,
    pub proposal: ProposalId,
    /// The proposal's current version when the request was made.
    pub version: u32,
    pub expected_version: Option<u32>,
    /// The event actor label of the caller.
    pub actor: String,
}

/// Writes a queued integration of `version`, the proposal's current version, in the caller's write and returns its row id. The unique index refuses a second waiting request for the same version.
pub async fn insert_integration_request(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    expected_version: Option<u32>,
    actor: &str,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let (number, now) = (proposal.to_db(), now.as_millis());
    let (version, expected) = (i64::from(version), expected_version.map(i64::from));
    let id = sqlx::query_scalar!(
        r#"INSERT INTO integration_requests (proposal_id, version, expected_version, actor, created_at)
           SELECT p.id, ?2, ?3, ?4, ?5 FROM proposals p WHERE p.number = ?1
           RETURNING id AS "id!: i64""#,
        number,
        version,
        expected,
        actor,
        now
    )
    .fetch_optional(tx.conn())
    .await?;
    id.ok_or(StoreError::Invalid("the proposal does not exist"))
}

/// Whether a request to integrate `version` of `proposal` waits in the queue.
pub async fn integration_queued(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: u32,
) -> Result<bool, StoreError> {
    let (number, version) = (proposal.to_db(), i64::from(version));
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM integration_requests q JOIN proposals p ON p.id = q.proposal_id
               WHERE p.number = ?1 AND q.version = ?2 AND q.ended_at IS NULL
           ) AS "queued!: bool""#,
        number,
        version
    )
    .fetch_one(executor)
    .await?)
}

/// Ends a queued integration. False when it had ended already.
pub async fn end_integration_request(
    tx: &mut WriteTx,
    id: i64,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let now = now.as_millis();
    let updated = sqlx::query!(
        "UPDATE integration_requests SET ended_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
        id,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Every queued integration without an end, in queue order.
pub async fn waiting_integration_requests(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<IntegrationRequestRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT q.id AS "id!: i64", p.number AS "proposal!: i64", q.version, q.expected_version, q.actor
           FROM integration_requests q JOIN proposals p ON p.id = q.proposal_id
           WHERE q.ended_at IS NULL
           ORDER BY q.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(IntegrationRequestRow {
                id: row.id,
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                version: count(row.version, "integration_requests.version")?,
                expected_version: row
                    .expected_version
                    .map(|version| count(version, "integration_requests.expected_version"))
                    .transpose()?,
                actor: row.actor,
            })
        })
        .collect()
}

/// A failed or stopped run of a proposal command that no start of its task worker has served yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedGateRun {
    pub agent: fold_core::AgentName,
    pub run: CheckRunId,
}

/// The runs of `create_proposal` and `update_proposal` that did not pass and that no start of their task worker, in one of `states`, has served: each start moves the row's `started_at`, so a run that ended later is still owed. Only the newest ended run of a workspace counts, so a later run owes nothing for an earlier one.
pub async fn owed_gate_runs(
    executor: impl SqliteExecutor<'_>,
    states: &[fold_core::WorkerState],
) -> Result<Vec<OwedGateRun>, StoreError> {
    let states = serde_json::Value::from(
        states
            .iter()
            .map(|state| state.as_str())
            .collect::<Vec<_>>(),
    )
    .to_string();
    let rows = sqlx::query!(
        r#"SELECT g.name AS "agent!: String", c.number AS "run!: i64"
           FROM check_runs c
           JOIN workers k ON k.id = c.worker_id
           JOIN worker_processes w ON w.agent_id = k.agent_id
           JOIN agents g ON g.id = w.agent_id
           WHERE c.stage = 'proposal' AND c.purpose IN ('create_proposal', 'update_proposal')
             AND c.state IN ('failed', 'stopped') AND c.finished_at > w.started_at
             AND w.role = 'task_worker' AND w.state IN (SELECT value FROM json_each(?1))
             AND c.id = (
                 SELECT MAX(x.id) FROM check_runs x
                 WHERE x.workspace_id = c.workspace_id AND x.state <> 'running'
             )
           ORDER BY w.id, c.id"#,
        states
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OwedGateRun {
                agent: fold_core::AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                run: CheckRunId::try_from_db(row.run)
                    .ok_or(StoreError::Corrupt("check_runs.number"))?,
            })
        })
        .collect()
}

/// The first `integration.held` event on main's current head, when there is one: since when integrations wait for main's checks file, and the problem the event stored. `None` once main moved past that head.
pub async fn held_by_main(
    executor: impl SqliteExecutor<'_>,
) -> Result<Option<(Timestamp, String)>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT e.created_at AS "created_at!: i64",
                  json_extract(e.data, '$.problem') AS "problem?: String"
           FROM events e JOIN refs r ON r.name = 'main' AND r.revision_id = e.revision_id
           WHERE e.type = 'integration.held'
           ORDER BY e.seq
           LIMIT 1"#
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok((
            time(row.created_at, "events.created_at")?,
            row.problem.unwrap_or_default(),
        ))
    })
    .transpose()
}

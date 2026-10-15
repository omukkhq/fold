use fold_core::{CheckpointId, ProposalId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId};
use sqlx::SqliteExecutor;

use super::columns::{number, timestamp};
use crate::error::StoreError;
use crate::store::WriteTx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceKind {
    Task,
    Review,
}

impl WorkspaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceKind::Task => "task",
            WorkspaceKind::Review => "review",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "task" => Ok(WorkspaceKind::Task),
            "review" => Ok(WorkspaceKind::Review),
            _ => Err(StoreError::Corrupt("workspaces.kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceState {
    Active,
    Missing,
    Archived,
    Abandoned,
}

impl WorkspaceState {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceState::Active => "active",
            WorkspaceState::Missing => "missing",
            WorkspaceState::Archived => "archived",
            WorkspaceState::Abandoned => "abandoned",
        }
    }

    /// A missing workspace is still the task's live workspace: its directory is gone from disk, but it holds the task in progress and blocks a second one.
    pub fn is_live(self) -> bool {
        matches!(self, WorkspaceState::Active | WorkspaceState::Missing)
    }

    pub(crate) fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "active" => Ok(WorkspaceState::Active),
            "missing" => Ok(WorkspaceState::Missing),
            "archived" => Ok(WorkspaceState::Archived),
            "abandoned" => Ok(WorkspaceState::Abandoned),
            _ => Err(StoreError::Corrupt("workspaces.state")),
        }
    }
}

/// The two states a live workspace closes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedState {
    Archived,
    Abandoned,
}

impl ClosedState {
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedState::Archived => WorkspaceState::Archived.as_str(),
            ClosedState::Abandoned => WorkspaceState::Abandoned.as_str(),
        }
    }
}

/// One workspace row. `worker_id` is a row id and stays inside the daemon; the public handle of the owner is `worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: WorkspaceId,
    pub task: TaskId,
    pub worker_id: Option<i64>,
    pub worker: Option<WorkerName>,
    pub kind: WorkspaceKind,
    /// Relative to the project root.
    pub path: String,
    pub base: RevisionId,
    pub proposal: Option<ProposalId>,
    pub state: WorkspaceState,
    pub changed_files: u32,
    /// How many paths `workspace_conflicts` lists for the workspace.
    pub conflicted: u32,
    pub last_activity_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct NewTaskWorkspace<'a> {
    pub id: WorkspaceId,
    pub task: TaskId,
    pub worker_id: i64,
    pub path: &'a str,
    pub base: RevisionId,
    pub now: Timestamp,
}

/// Inserts an active task workspace. A second live task workspace for the task fails on `workspaces.task_id` and a reused directory on `workspaces.path`, so the unique indexes decide a race. A task or base revision that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_task_workspace(
    tx: &mut WriteTx,
    new: &NewTaskWorkspace<'_>,
) -> Result<(), StoreError> {
    let (id, task, base, now) = (
        new.id.to_db(),
        new.task.to_db(),
        new.base.to_db(),
        new.now.as_millis(),
    );
    let inserted = sqlx::query!(
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, last_activity_at, created_at)
         SELECT ?1, t.id, ?3, 'task', ?4, r.id, 'active', ?6, ?6
         FROM tasks t, revisions r
         WHERE t.number = ?2 AND r.number = ?5",
        id,
        task,
        new.worker_id,
        new.path,
        base,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the task or the base revision does not exist",
        ))
    }
}

pub async fn get_workspace(
    executor: impl SqliteExecutor<'_>,
    workspace: WorkspaceId,
) -> Result<Option<WorkspaceRow>, StoreError> {
    select(executor, Some(workspace), None, false).await
}

/// The task workspace of `task` in state `active` or `missing`; the partial unique index allows at most one.
pub async fn live_task_workspace(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<WorkspaceRow>, StoreError> {
    select(executor, None, Some(task), true).await
}

/// The newest task workspace of `task` in any state.
pub async fn latest_task_workspace(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<WorkspaceRow>, StoreError> {
    select(executor, None, Some(task), false).await
}

/// The directories of every task workspace `task` has had, in any state. Their count gives the next workspace's directory name, and a create skips each of them, so a name is never handed out twice, also after a closed workspace's directory is gone.
pub async fn task_workspace_paths(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Vec<String>, StoreError> {
    let task = task.to_db();
    let paths = sqlx::query_scalar!(
        "SELECT w.path
         FROM workspaces w JOIN tasks t ON t.id = w.task_id
         WHERE t.number = ? AND w.kind = 'task'
         ORDER BY w.number",
        task
    )
    .fetch_all(executor)
    .await?;
    Ok(paths)
}

/// Moves a live workspace to `state` and stamps `closed_at`. Only the row changes; the directory stays until garbage collection. False when the workspace is not live or does not exist, and then nothing changes.
pub async fn close(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    state: ClosedState,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (workspace, state, now) = (workspace.to_db(), state.as_str(), now.as_millis());
    let closed = sqlx::query!(
        "UPDATE workspaces SET state = ?2, closed_at = ?3
         WHERE number = ?1 AND state IN ('active', 'missing')",
        workspace,
        state,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(closed == 1)
}

/// Mirrors the live state's change count and last activity into the row of a live workspace, so the board query needs no live state. It appends no event and runs no `recompute`: the columns only summarise working files. False when the workspace is not live or does not exist.
pub async fn touch(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    changed_files: u32,
    last_activity_at: Timestamp,
) -> Result<bool, StoreError> {
    let (workspace, at) = (workspace.to_db(), last_activity_at.as_millis());
    let touched = sqlx::query!(
        "UPDATE workspaces SET changed_files = ?2, last_activity_at = ?3
         WHERE number = ?1 AND state IN ('active', 'missing')",
        workspace,
        changed_files,
        at
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(touched == 1)
}

/// Gives a live workspace to the worker `worker_id`, as an adoption does. Only `worker_id` changes: the directory, the base, the changes and the conflicted paths stay. False when the workspace is not live or does not exist, and then nothing changes.
pub async fn set_worker(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    worker_id: i64,
) -> Result<bool, StoreError> {
    let workspace = workspace.to_db();
    let changed = sqlx::query!(
        "UPDATE workspaces SET worker_id = ?2
         WHERE number = ?1 AND state IN ('active', 'missing')",
        workspace,
        worker_id
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
}

/// One statement serves every read so the rows always map the same way. `task` with `live_only` reads the live task workspace; `task` alone the newest task workspace.
async fn select(
    executor: impl SqliteExecutor<'_>,
    workspace: Option<WorkspaceId>,
    task: Option<TaskId>,
    live_only: bool,
) -> Result<Option<WorkspaceRow>, StoreError> {
    let workspace = workspace.map(WorkspaceId::to_db);
    let task = task.map(TaskId::to_db);
    let row = sqlx::query!(
        r#"SELECT w.number AS "number!: i64",
                  t.number AS "task!: i64",
                  w.worker_id,
                  k.name AS "worker?: String",
                  w.kind,
                  w.path,
                  r.number AS "base!: i64",
                  p.number AS "proposal?: i64",
                  w.state,
                  w.changed_files,
                  (SELECT COUNT(*) FROM workspace_conflicts c WHERE c.workspace_id = w.id) AS "conflicted!: i64",
                  w.last_activity_at,
                  w.created_at,
                  w.closed_at
           FROM workspaces w
           JOIN tasks t ON t.id = w.task_id
           JOIN revisions r ON r.id = w.base_revision_id
           LEFT JOIN workers k ON k.id = w.worker_id
           LEFT JOIN proposals p ON p.id = w.proposal_id
           WHERE (?1 IS NULL OR w.number = ?1)
             AND (?2 IS NULL OR (t.number = ?2 AND w.kind = 'task'))
             AND (NOT ?3 OR w.state IN ('active', 'missing'))
           ORDER BY w.number DESC
           LIMIT 1"#,
        workspace,
        task,
        live_only
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(WorkspaceRow {
            id: number(row.number, WorkspaceId::try_from_db, "workspaces.number")?,
            task: number(row.task, TaskId::try_from_db, "tasks.number")?,
            worker_id: row.worker_id,
            worker: row
                .worker
                .map(|name| {
                    WorkerName::parse(&name).map_err(|_| StoreError::Corrupt("workers.name"))
                })
                .transpose()?,
            kind: WorkspaceKind::parse(&row.kind)?,
            path: row.path,
            base: number(row.base, RevisionId::try_from_db, "revisions.number")?,
            proposal: row
                .proposal
                .map(|value| number(value, ProposalId::try_from_db, "proposals.number"))
                .transpose()?,
            state: WorkspaceState::parse(&row.state)?,
            changed_files: u32::try_from(row.changed_files)
                .map_err(|_| StoreError::Corrupt("workspaces.changed_files"))?,
            conflicted: u32::try_from(row.conflicted)
                .map_err(|_| StoreError::Corrupt("workspace_conflicts count"))?,
            last_activity_at: row
                .last_activity_at
                .map(|millis| timestamp(millis, "workspaces.last_activity_at"))
                .transpose()?,
            created_at: timestamp(row.created_at, "workspaces.created_at")?,
            closed_at: row
                .closed_at
                .map(|millis| timestamp(millis, "workspaces.closed_at"))
                .transpose()?,
        })
    })
    .transpose()
}

/// Moves the workspace's base to `base`, as a finished sync and a restore do. False when the workspace or the revision does not exist, and then nothing changes.
pub async fn set_base(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    base: RevisionId,
) -> Result<bool, StoreError> {
    let (workspace, base) = (workspace.to_db(), base.to_db());
    let moved = sqlx::query!(
        "UPDATE workspaces SET base_revision_id = r.id
         FROM (SELECT id FROM revisions WHERE number = ?2) AS r
         WHERE workspaces.number = ?1",
        workspace,
        base
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(moved == 1)
}

/// The checkpoint of the last complete state of the workspace before a sync or a restore began to write files, while that command has not finished. A finished sync or restore clears it.
pub async fn sync_checkpoint(
    executor: impl SqliteExecutor<'_>,
    workspace: WorkspaceId,
) -> Result<Option<CheckpointId>, StoreError> {
    let workspace = workspace.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT c.number AS "number!: i64"
           FROM workspaces w JOIN checkpoints c ON c.id = w.sync_checkpoint_id
           WHERE w.number = ?"#,
        workspace
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| number(value, CheckpointId::try_from_db, "checkpoints.number"))
        .transpose()
}

/// Sets or clears the checkpoint of a sync or a restore in progress. False when the workspace or the checkpoint does not exist, and then nothing changes.
pub async fn set_sync_checkpoint(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    checkpoint: Option<CheckpointId>,
) -> Result<bool, StoreError> {
    let workspace = workspace.to_db();
    let checkpoint = checkpoint.map(CheckpointId::to_db);
    let changed = sqlx::query!(
        "UPDATE workspaces
         SET sync_checkpoint_id = (SELECT id FROM checkpoints WHERE number = ?2)
         WHERE number = ?1 AND (?2 IS NULL OR EXISTS (SELECT 1 FROM checkpoints WHERE number = ?2))",
        workspace,
        checkpoint
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
}

#[derive(Debug, Clone)]
pub struct NewReviewCheckout<'a> {
    pub id: WorkspaceId,
    pub proposal: ProposalId,
    pub version: u32,
    pub worker_id: i64,
    pub path: &'a str,
    pub now: Timestamp,
}

/// Inserts an active review checkout of `version` of the proposal, owned by `worker_id`. It takes the proposal's task and the version's base revision. A second active checkout of the same version for the worker fails on `workspaces.version_id`, and a reused directory on `workspaces.path`, so the unique indexes decide a race. A proposal or version that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_review_checkout(
    tx: &mut WriteTx,
    new: &NewReviewCheckout<'_>,
) -> Result<(), StoreError> {
    let (id, proposal, version, now) = (
        new.id.to_db(),
        new.proposal.to_db(),
        i64::from(new.version),
        new.now.as_millis(),
    );
    let inserted = sqlx::query!(
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, proposal_id, version_id, state, last_activity_at, created_at)
         SELECT ?1, p.task_id, ?3, 'review', ?4, v.base_revision_id, p.id, v.id, 'active', ?6, ?6
         FROM proposals p JOIN proposal_versions v ON v.proposal_id = p.id
         WHERE p.number = ?2 AND v.version = ?5",
        id,
        proposal,
        new.worker_id,
        new.path,
        version,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the proposal or the version does not exist",
        ))
    }
}

/// The active review checkout of `version` of the proposal that `worker_id` owns; the partial unique index allows at most one.
pub async fn active_review_checkout(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: u32,
    worker_id: i64,
) -> Result<Option<WorkspaceId>, StoreError> {
    let (proposal, version) = (proposal.to_db(), i64::from(version));
    let found = sqlx::query_scalar!(
        r#"SELECT w.number AS "number!: i64"
           FROM workspaces w
           JOIN proposals p ON p.id = w.proposal_id
           JOIN proposal_versions v ON v.id = w.version_id
           WHERE p.number = ?1 AND v.version = ?2 AND w.worker_id = ?3
             AND w.kind = 'review' AND w.state = 'active'"#,
        proposal,
        version,
        worker_id
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| number(value, WorkspaceId::try_from_db, "workspaces.number"))
        .transpose()
}

/// The directories of every review checkout the proposal has had, in any state. Their count gives the next checkout's directory name, and a create skips each of them, so a name is never handed out twice.
pub async fn review_checkout_paths(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<String>, StoreError> {
    let proposal = proposal.to_db();
    let paths = sqlx::query_scalar!(
        "SELECT w.path
         FROM workspaces w JOIN proposals p ON p.id = w.proposal_id
         WHERE p.number = ? AND w.kind = 'review'
         ORDER BY w.number",
        proposal
    )
    .fetch_all(executor)
    .await?;
    Ok(paths)
}

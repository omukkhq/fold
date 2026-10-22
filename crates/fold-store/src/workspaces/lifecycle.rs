use fold_core::{
    CheckpointId, ProposalId, ReviewBatchId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId,
};
use sqlx::SqliteExecutor;

use super::columns::{number, timestamp};
use crate::error::StoreError;
use crate::store::WriteTx;

/// The columns of [`RawWorkspace`] and the joins that give them, followed by the tail of each read: every workspace row maps the same way.
macro_rules! select_workspaces {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            RawWorkspace,
            r#"SELECT w.number AS "number!: i64",
                      t.number AS "task?: i64",
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
               LEFT JOIN tasks t ON t.id = w.task_id
               JOIN revisions r ON r.id = w.base_revision_id
               LEFT JOIN workers k ON k.id = w.worker_id
               LEFT JOIN proposals p ON p.id = w.proposal_id
               "# + $tail
            $(, $arg)*
        )
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceKind {
    Task,
    Review,
    /// The main agent's workspace, which belongs to no task: its changes reach main directly, on the user's word.
    Main,
}

impl WorkspaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceKind::Task => "task",
            WorkspaceKind::Review => "review",
            WorkspaceKind::Main => "main",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "task" => Ok(WorkspaceKind::Task),
            "review" => Ok(WorkspaceKind::Review),
            "main" => Ok(WorkspaceKind::Main),
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
    /// `None` for the main workspace only.
    pub task: Option<TaskId>,
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
    /// `None` for the workspace of a sync task, which waits for the first claimant to adopt it.
    pub worker_id: Option<i64>,
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
    let number = workspace.to_db();
    one(select_workspaces!("WHERE w.number = ?1", number)
        .fetch_optional(executor)
        .await?)
}

/// The task workspace of `task` in state `active` or `missing`; the partial unique index allows at most one.
pub async fn live_task_workspace(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<WorkspaceRow>, StoreError> {
    let number = task.to_db();
    one(select_workspaces!(
        "WHERE t.number = ?1 AND w.kind = 'task' AND w.state IN ('active', 'missing')
         ORDER BY w.number DESC
         LIMIT 1",
        number
    )
    .fetch_optional(executor)
    .await?)
}

/// The newest task workspace of `task` in any state.
pub async fn latest_task_workspace(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<WorkspaceRow>, StoreError> {
    let number = task.to_db();
    one(select_workspaces!(
        "WHERE t.number = ?1 AND w.kind = 'task' ORDER BY w.number DESC LIMIT 1",
        number
    )
    .fetch_optional(executor)
    .await?)
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

#[derive(Debug, Clone)]
pub struct NewMainWorkspace<'a> {
    pub id: WorkspaceId,
    pub worker_id: i64,
    pub path: &'a str,
    pub base: RevisionId,
    pub now: Timestamp,
}

/// Inserts the active main workspace of `worker_id`, which belongs to no task. A second live main workspace fails on `workspaces.kind` and a reused directory on `workspaces.path`, so the unique indexes decide a race. A base revision that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_main_workspace(
    tx: &mut WriteTx,
    new: &NewMainWorkspace<'_>,
) -> Result<(), StoreError> {
    let (id, base, now) = (new.id.to_db(), new.base.to_db(), new.now.as_millis());
    let inserted = sqlx::query!(
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, last_activity_at, created_at)
         SELECT ?1, NULL, ?2, 'main', ?3, r.id, 'active', ?5, ?5
         FROM revisions r
         WHERE r.number = ?4",
        id,
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
        Err(StoreError::Invalid("the base revision does not exist"))
    }
}

/// The main workspace in state `active` or `missing`; the partial unique index allows at most one.
pub async fn live_main_workspace(
    executor: impl SqliteExecutor<'_>,
) -> Result<Option<WorkspaceRow>, StoreError> {
    one(select_workspaces!(
        "WHERE w.kind = 'main' AND w.state IN ('active', 'missing')
         ORDER BY w.number DESC
         LIMIT 1"
    )
    .fetch_optional(executor)
    .await?)
}

/// The directories of every main workspace the project has had, in any state, so a new one never takes the name of an earlier one.
pub async fn main_workspace_paths(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<String>, StoreError> {
    let paths =
        sqlx::query_scalar!("SELECT path FROM workspaces WHERE kind = 'main' ORDER BY number")
            .fetch_all(executor)
            .await?;
    Ok(paths)
}

/// Moves a live workspace to `state` and stamps `closed_at`. Only the row changes; the daemon removes the directory after the commit. False when the workspace is not live or does not exist, and then nothing changes.
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

/// A workspace row as the reads select it, before its columns are checked.
struct RawWorkspace {
    number: i64,
    task: Option<i64>,
    worker_id: Option<i64>,
    worker: Option<String>,
    kind: String,
    path: String,
    base: i64,
    proposal: Option<i64>,
    state: String,
    changed_files: i64,
    conflicted: i64,
    last_activity_at: Option<i64>,
    created_at: i64,
    closed_at: Option<i64>,
}

/// The one workspace a `select_workspaces!` read found, mapped.
fn one(row: Option<RawWorkspace>) -> Result<Option<WorkspaceRow>, StoreError> {
    row.map(workspace_row).transpose()
}

/// Every task workspace in state `active` or `missing`, by number. Review checkouts are left out.
pub async fn list_live_task_workspaces(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<WorkspaceRow>, StoreError> {
    let rows = select_workspaces!(
        "WHERE w.kind = 'task' AND w.state IN ('active', 'missing') ORDER BY w.number"
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(workspace_row).collect()
}

/// An active review checkout and the proposal versions its files hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCheckoutRow {
    pub workspace: WorkspaceRow,
    pub target: CheckoutTarget,
}

/// What a review checkout holds: one proposal version, or the members of a review batch merged onto main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutTarget {
    Proposal {
        proposal: ProposalId,
        version: u32,
    },
    /// The members in merge order, each at the version it joined the batch with.
    Batch {
        batch: ReviewBatchId,
        members: Vec<(ProposalId, u32)>,
    },
}

/// Every active review checkout of the project, oldest first: the checkouts of one proposal and those of a review batch. A batch checkout holds the batch's members that have not left it; a member that leaves archives the checkout.
pub async fn list_active_review_checkouts(
    executor: &mut sqlx::SqliteConnection,
) -> Result<Vec<ReviewCheckoutRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT w.number AS "number!: i64",
                  t.number AS "task?: i64",
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
                  w.closed_at,
                  v.version AS "version?: i64",
                  b.number AS "batch?: i64"
           FROM workspaces w
           JOIN tasks t ON t.id = w.task_id
           JOIN revisions r ON r.id = w.base_revision_id
           LEFT JOIN proposal_versions v ON v.id = w.version_id
           LEFT JOIN proposals p ON p.id = w.proposal_id
           LEFT JOIN review_batches b ON b.id = w.review_batch_id
           LEFT JOIN workers k ON k.id = w.worker_id
           WHERE w.kind = 'review' AND w.state = 'active'
           ORDER BY w.number"#
    )
    .fetch_all(&mut *executor)
    .await?;
    let members = sqlx::query!(
        r#"SELECT w.number AS "workspace!: i64",
                  p.number AS "proposal!: i64",
                  v.version AS "version!: i64"
           FROM workspaces w
           JOIN review_batch_members m ON m.batch_id = w.review_batch_id AND m.released_at IS NULL
           JOIN proposals p ON p.id = m.proposal_id
           JOIN proposal_versions v ON v.id = m.version_id
           WHERE w.kind = 'review' AND w.state = 'active'
           ORDER BY w.number, m.position"#
    )
    .fetch_all(&mut *executor)
    .await?;
    let version = |value: i64| {
        u32::try_from(value).map_err(|_| StoreError::Corrupt("proposal_versions.version"))
    };
    let proposal =
        |value: i64| ProposalId::try_from_db(value).ok_or(StoreError::Corrupt("proposals.number"));
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let target = match (row.proposal, row.version, row.batch) {
            (Some(number), Some(held), None) => CheckoutTarget::Proposal {
                proposal: proposal(number)?,
                version: version(held)?,
            },
            (None, None, Some(batch)) => CheckoutTarget::Batch {
                batch: ReviewBatchId::try_from_db(batch)
                    .ok_or(StoreError::Corrupt("review_batches.number"))?,
                members: members
                    .iter()
                    .filter(|member| member.workspace == row.number)
                    .map(|member| Ok((proposal(member.proposal)?, version(member.version)?)))
                    .collect::<Result<_, StoreError>>()?,
            },
            _ => return Err(StoreError::Corrupt("workspaces.review_batch_id")),
        };
        let workspace = workspace_row(RawWorkspace {
            number: row.number,
            task: row.task,
            worker_id: row.worker_id,
            worker: row.worker,
            kind: row.kind,
            path: row.path,
            base: row.base,
            proposal: row.proposal,
            state: row.state,
            changed_files: row.changed_files,
            conflicted: row.conflicted,
            last_activity_at: row.last_activity_at,
            created_at: row.created_at,
            closed_at: row.closed_at,
        })?;
        out.push(ReviewCheckoutRow { workspace, target });
    }
    Ok(out)
}

fn workspace_row(row: RawWorkspace) -> Result<WorkspaceRow, StoreError> {
    Ok(WorkspaceRow {
        id: number(row.number, WorkspaceId::try_from_db, "workspaces.number")?,
        task: row
            .task
            .map(|value| number(value, TaskId::try_from_db, "tasks.number"))
            .transpose()?,
        worker_id: row.worker_id,
        worker: row
            .worker
            .map(|name| WorkerName::parse(&name).map_err(|_| StoreError::Corrupt("workers.name")))
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
}

/// Moves a live task workspace or the main workspace between `active` and `missing` as its directory comes and goes: `present` moves a missing one to `active`, and an absent directory moves an active one to `missing`. Both states keep the workspace live, so nothing else changes. False, with nothing changed, when the workspace is neither a task workspace nor the main workspace in the other state.
pub async fn set_directory_present(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    present: bool,
) -> Result<bool, StoreError> {
    let workspace = workspace.to_db();
    let (from, to) = if present {
        (
            WorkspaceState::Missing.as_str(),
            WorkspaceState::Active.as_str(),
        )
    } else {
        (
            WorkspaceState::Active.as_str(),
            WorkspaceState::Missing.as_str(),
        )
    };
    let changed = sqlx::query!(
        "UPDATE workspaces SET state = ?3
         WHERE number = ?1 AND kind IN ('task', 'main') AND state = ?2",
        workspace,
        from,
        to
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
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

/// Sets or clears the checkpoint of a sync or a restore in progress. False when the workspace does not exist or the checkpoint is not one of its own, and then nothing changes.
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
         WHERE number = ?1
           AND (?2 IS NULL OR EXISTS (SELECT 1 FROM checkpoints WHERE number = ?2 AND workspace_id = workspaces.id))",
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

/// Points the active review checkout `workspace` of the proposal at its `version`, whose tree its files hold once the reviewer's fix in it became that version. False, with nothing changed, when the checkout is not an active checkout of the proposal or the version does not exist.
pub async fn move_review_checkout(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    proposal: ProposalId,
    version: u32,
) -> Result<bool, StoreError> {
    let (workspace, proposal, version) = (workspace.to_db(), proposal.to_db(), i64::from(version));
    let moved = sqlx::query!(
        "UPDATE workspaces
         SET version_id = (SELECT v.id FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
                           WHERE p.number = ?2 AND v.version = ?3)
         WHERE number = ?1 AND kind = 'review' AND state = 'active'
           AND proposal_id = (SELECT id FROM proposals WHERE number = ?2)
           AND EXISTS (SELECT 1 FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
                       WHERE p.number = ?2 AND v.version = ?3)",
        workspace,
        proposal,
        version
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(moved == 1)
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

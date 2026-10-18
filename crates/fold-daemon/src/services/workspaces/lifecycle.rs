use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::status::ProposalFact;
use fold_core::{
    Actor, AgentActor, ChangesetId, CheckpointId, EventActor, EventError, FoldError, IdKind,
    NewEvent, ProposalId, ReviewBatchId, RevisionId, TaskId, TaskStatus, Timestamp, WorkerName,
    WorkspaceId, codes, policy, review_batch_checkout_path, review_checkout_path,
    task_workspace_path,
};
use fold_store::agents::WorkerRow;
use fold_store::tasks::claims;
use fold_store::workspaces::{
    self, ClosedState, NewTaskWorkspace, WorkspaceKind, WorkspaceRow, WorkspaceState,
};
use fold_store::{StoreError, WriteTx, counters, tasks};
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::{MaterialiseOptions, materialise};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::WorkspaceError as VcsError;
use fold_vcs::platform::{create_dir_below, sync_dir};
use serde_json::json;

use super::sync::path_text;
use super::{WorkspaceError, WorkspaceService, blocking, capabilities};
use crate::services::events::EventBuffer;
use crate::services::tasks::{
    agent_refusal, already_claimed, claim_in_tx, closed_refusal, proposal_kind_refusal, recompute,
    task_not_found, worker_row,
};

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// How many occupied names a create steps over before it gives up.
const MAX_NAME_ATTEMPTS: u32 = 64;

/// The longest abandon reason, in characters.
const MAX_REASON_CHARS: usize = 500;

/// An abandoned workspace and the status its task went back to, read in the transaction that abandoned it; `None` for the main workspace, which has no task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abandoned {
    pub workspace: WorkspaceView,
    pub task_status: Option<TaskStatus>,
}

/// A workspace with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: WorkspaceId,
    /// `None` for the main workspace only.
    pub task: Option<TaskId>,
    pub worker: Option<WorkerName>,
    pub kind: WorkspaceKind,
    /// Relative to the project root.
    pub path: String,
    pub base: RevisionId,
    pub proposal: Option<ProposalId>,
    pub state: WorkspaceState,
    pub changed_files: u32,
    pub conflicted: u32,
    pub last_activity_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

impl From<WorkspaceRow> for WorkspaceView {
    fn from(row: WorkspaceRow) -> Self {
        Self {
            id: row.id,
            task: row.task,
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
        }
    }
}

/// A workspace with what an agent needs to pick up its work: the conflicted paths and the newest checkpoint and changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDetails {
    pub workspace: WorkspaceView,
    /// In path order.
    pub conflicted: Vec<String>,
    pub latest_checkpoint: Option<CheckpointId>,
    pub latest_changeset: Option<ChangesetId>,
}

/// What `get` looks up: a workspace by id, or the workspace of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRef {
    Workspace(WorkspaceId),
    Task(TaskId),
}

/// Why a workspace was archived, as `workspace.archived` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveReason {
    Integrated,
    TaskArchived,
    /// A review checkout whose proposal was closed by its author, an agent that decides reviews or a human.
    ProposalClosed,
    /// A review checkout whose proposal was rejected.
    ProposalRejected,
    /// A batch checkout whose batch lost a member, so its files hold a member the batch does not.
    BatchChanged,
    /// A batch checkout whose batch was submitted.
    BatchSubmitted,
    /// A batch checkout whose batch closed because no member was left to review.
    BatchClosed,
}

impl ArchiveReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ArchiveReason::Integrated => "integrated",
            ArchiveReason::TaskArchived => "task_archived",
            ArchiveReason::ProposalClosed => "proposal_closed",
            ArchiveReason::ProposalRejected => "proposal_rejected",
            ArchiveReason::BatchChanged => "batch_changed",
            ArchiveReason::BatchSubmitted => "batch_submitted",
            ArchiveReason::BatchClosed => "batch_closed",
        }
    }
}

/// What `create_workspace` returns: the task's workspace, and whether the create adopted an orphaned one instead of making a new directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWorkspace {
    pub workspace: WorkspaceView,
    pub adopted: bool,
}

/// What step 1 of a create decided.
enum CreatePlan {
    /// The task's live workspace, which no claim holder owns.
    Adopt(WorkspaceRow),
    Create {
        base: Box<workspaces::RevisionRow>,
        taken: Vec<String>,
    },
}

/// The directory a create renamed into place, with the index of what it wrote.
pub(super) struct Placed {
    pub(super) path: String,
    pub(super) dir: PathBuf,
    pub(super) index: StatIndex,
}

impl WorkspaceService {
    /// Creates the task workspace of `task` for the actor's worker: a copy of the current main revision under `.fold/ws/`, recorded with that revision as its base. A workspace belongs to the implementation claim, so `policy::can_claim` admits only an agent; its worker must hold the claim or the task must be unclaimed, and then the create takes it. The task moves to In Progress, also from Backlog, where it keeps its blocked indicator.
    ///
    /// A task whose live workspace is orphaned, with no owner or an owner that does not hold the claim, keeps that workspace: the create takes the claim, adopts it and makes nothing new. A live workspace whose owner holds the claim, or that is already the actor's own, is `workspace_exists`.
    ///
    /// The files are written and renamed into place before the transaction that inserts the row, so no row ever names a directory that is not complete. When the transaction fails the directory is removed again: it has no row, and nobody was told its path.
    pub async fn create_workspace(
        &self,
        actor: &Actor,
        task: TaskId,
    ) -> Result<CreatedWorkspace, WorkspaceError> {
        let claimant = policy::can_claim(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        let lease_minutes = self.lease.staleness_minutes(&self.store).await?;
        let plan = match self
            .check_create(claimant, task, now, lease_minutes)
            .await?
        {
            CreatePlan::Adopt(orphan) => {
                match self
                    .adopt_on_create(claimant, task, orphan.id, now, lease_minutes)
                    .await?
                {
                    Some(workspace) => {
                        return Ok(CreatedWorkspace {
                            workspace,
                            adopted: true,
                        });
                    }
                    None => {
                        self.check_create(claimant, task, now, lease_minutes)
                            .await?
                    }
                }
            }
            plan @ CreatePlan::Create { .. } => plan,
        };
        let (base, taken) = match plan {
            CreatePlan::Create { base, taken } => (base, taken),
            CreatePlan::Adopt(live) => return Err(workspace_exists(&live).into()),
        };
        let placed = self.place(task, taken, base.tree_hash).await?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("workspace_create_after_rename", |_| {
            Err(WorkspaceError::from(FoldError::new(
                codes::FAILPOINT,
                "workspace_create_after_rename fired",
            )))
        });
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                let worker = worker_row(tx, claimant).await?;
                insert_in_tx(
                    tx,
                    &mut buffer,
                    &worker,
                    task,
                    &placed.path,
                    base.number,
                    now,
                    lease_minutes,
                )
                .await
            })
            .await;
        let Placed {
            path,
            dir,
            mut index,
        } = placed;
        let id = match written {
            Ok(id) => id,
            Err(error) => {
                let removed = tokio::task::spawn_blocking(move || fs::remove_dir_all(dir)).await;
                if !matches!(removed, Ok(Ok(()))) {
                    tracing::warn!(task = %task, path = %path, "a workspace directory without a row was not removed");
                }
                return Err(error);
            }
        };
        self.bus.publish(buffer);
        let index_path = self.paths.workspace_index(id);
        let saved = tokio::task::spawn_blocking(move || index.save(&index_path)).await;
        if !matches!(saved, Ok(Ok(()))) {
            tracing::warn!(workspace = %id, "the stat index was not saved; the next scan rebuilds it");
        }
        if let Some(live) = &self.live
            && let Err(error) = live.attach(id).await
        {
            tracing::warn!(workspace = %id, code = error.log_code(), "the live state did not start");
        }
        Ok(CreatedWorkspace {
            workspace: self.get(WorkspaceRef::Workspace(id)).await?,
            adopted: false,
        })
    }

    /// Abandons a live workspace of the actor's worker, which `policy::can_abandon_workspace` requires to be the owner: it becomes `abandoned` and `closed_at` is set, and the task goes back to Todo or Backlog. The directory and the claim stay. Refused while the task has an open proposal; the author closes it first, and nothing closes it on the worker's behalf. The main workspace has no task, so abandoning it only closes it and discards its unsubmitted change. `reason` is the worker's short explanation, which `workspace.abandoned` carries.
    pub async fn abandon_workspace(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
        reason: &str,
    ) -> Result<Abandoned, WorkspaceError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the reason must be 1 to {MAX_REASON_CHARS} characters"),
            )
            .with_details([("field", json!("reason"))])
            .with_hint("call abandon_workspace again with a short reason")
            .into());
        }
        let _op = self.ops.lock(workspace).await;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let (row, task_status) = self
            .store
            .write(async |tx| abandon_in_tx(tx, &mut buffer, actor, workspace, reason, now).await)
            .await?;
        self.bus.publish(buffer);
        if let Some(live) = &self.live {
            live.detach(workspace).await;
        }
        Ok(Abandoned {
            workspace: row.into(),
            task_status,
        })
    }

    /// A workspace by id, or the workspace of a task: its live one, else its newest.
    pub async fn get(&self, target: WorkspaceRef) -> Result<WorkspaceView, WorkspaceError> {
        let read = self.store.read();
        let row = match target {
            WorkspaceRef::Workspace(id) => workspaces::get_workspace(read, id).await?,
            WorkspaceRef::Task(task) => match workspaces::live_task_workspace(read, task).await? {
                Some(row) => Some(row),
                None => workspaces::latest_task_workspace(read, task).await?,
            },
        };
        row.map(WorkspaceView::from).ok_or_else(|| {
            let name = match target {
                WorkspaceRef::Workspace(id) => id.to_string(),
                WorkspaceRef::Task(task) => format!("a workspace of {task}"),
            };
            FoldError::new(codes::WORKSPACE_NOT_FOUND, format!("{name} does not exist"))
                .with_hint("call create_workspace to start one")
                .into()
        })
    }

    /// [`WorkspaceService::get`] with the conflicted paths and the newest unpruned checkpoint and changeset.
    pub async fn details(&self, target: WorkspaceRef) -> Result<WorkspaceDetails, WorkspaceError> {
        let workspace = self.get(target).await?;
        let read = self.store.read();
        let conflicted = workspaces::list_conflicts(read, workspace.id)
            .await?
            .iter()
            .map(path_text)
            .collect();
        let latest_checkpoint = workspaces::list_checkpoints(read, workspace.id, false)
            .await?
            .last()
            .map(|checkpoint| checkpoint.id);
        let latest_changeset = workspaces::latest_changeset(read, workspace.id)
            .await?
            .map(|changeset| changeset.id);
        Ok(WorkspaceDetails {
            workspace,
            conflicted,
            latest_checkpoint,
            latest_changeset,
        })
    }

    /// Step 1 of a create: the refusals that need no files, read before anything is written, and whether an orphaned workspace is adopted instead. The transaction checks the claim and the live workspace again.
    async fn check_create(
        &self,
        claimant: &AgentActor,
        task: TaskId,
        now: Timestamp,
        lease_minutes: u64,
    ) -> Result<CreatePlan, WorkspaceError> {
        let read = self.store.read();
        let row = tasks::get_by_number(read, task)
            .await?
            .ok_or_else(|| task_not_found(task))?;
        if let Some(refusal) = closed_refusal(&row)
            .or_else(|| proposal_kind_refusal(&row))
            .or_else(|| agent_refusal(&row).map(Into::into))
        {
            return Err(refusal.into());
        }
        let holder = claims::unreleased_for_task(read, task).await?;
        if let Some(holder) = &holder
            && holder.worker != claimant.worker
        {
            return Err(already_claimed(holder, now, lease_minutes).into());
        }
        if let Some(live) = workspaces::live_task_workspace(read, task).await? {
            let held = holder.is_some_and(|holder| live.worker_id == Some(holder.worker_id));
            if held || live.worker.as_ref() == Some(&claimant.worker) {
                return Err(workspace_exists(&live).into());
            }
            return Ok(CreatePlan::Adopt(live));
        }
        let base = workspaces::head(read).await?.ok_or_else(|| {
            FoldError::new(codes::REVISION_NOT_FOUND, "main has no revision")
                .with_hint("run fld init to create the first revision")
        })?;
        let taken = workspaces::task_workspace_paths(read, task).await?;
        Ok(CreatePlan::Create {
            base: Box::new(base),
            taken,
        })
    }

    /// Step 2 of a create: materialises `tree` into a staging directory under `.fold/ws/` and renames it to the task's next directory name. A name that `taken` lists for an earlier workspace of the task, or that some directory already holds, such as one left by a create that stopped before its row, is skipped, so a name is never shared.
    async fn place(
        &self,
        task: TaskId,
        taken: Vec<String>,
        tree: ObjectHash,
    ) -> Result<Placed, WorkspaceError> {
        let name = move |earlier| task_workspace_path(task, earlier);
        self.place_named(name, taken, tree, false).await
    }

    /// Materialises `tree` writable into the proposal's next review checkout directory, by the naming rules of a task workspace's create, and returns the directory relative to the project root with its full path. The reviewer fixes what it finds there, and `submit_review_fix` records the files; no stat index is kept until that record makes one.
    pub(crate) async fn place_review_checkout(
        &self,
        proposal: ProposalId,
        taken: Vec<String>,
        tree: ObjectHash,
    ) -> Result<(String, PathBuf), WorkspaceError> {
        let name = move |earlier| review_checkout_path(proposal, earlier);
        let placed = self.place_named(name, taken, tree, false).await?;
        Ok((placed.path, placed.dir))
    }

    /// Materialises `tree` read-only into the batch's next checkout directory, by the naming rules of a proposal's checkout, and returns the directory relative to the project root with its full path. Its files mix every member's work, so no fix is made there.
    pub(crate) async fn place_review_batch_checkout(
        &self,
        batch: ReviewBatchId,
        taken: Vec<String>,
        tree: ObjectHash,
    ) -> Result<(String, PathBuf), WorkspaceError> {
        let name = move |earlier| review_batch_checkout_path(batch, earlier);
        let placed = self.place_named(name, taken, tree, true).await?;
        Ok((placed.path, placed.dir))
    }

    pub(super) async fn place_named(
        &self,
        name: impl Fn(u32) -> String + Send + 'static,
        taken: Vec<String>,
        tree: ObjectHash,
        read_only: bool,
    ) -> Result<Placed, WorkspaceError> {
        let paths = self.paths.clone();
        let ws_dir = self.paths.ws_dir();
        let root = self.paths.root().to_path_buf();
        let probed = Arc::clone(&self.caps);
        blocking(move || {
            create_dir_below(&paths.fold_dir(), &ws_dir).map_err(io_error)?;
            let objects = ObjectStore::open_existing(&paths.fold_dir()).map_err(FoldError::from)?;
            let caps = capabilities(&probed, &paths)?;
            let staging = staging_dir(&ws_dir)?;
            let opts = MaterialiseOptions { read_only, caps };
            let index = match materialise(&objects, &tree, &staging, &opts) {
                Ok(done) => done.index,
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    return Err(error.into());
                }
            };
            match rename_into_place(&staging, &root, &name, &taken) {
                Ok((path, dir)) => match sync_dir(&ws_dir) {
                    Ok(()) => Ok(Placed { path, dir, index }),
                    Err(error) => {
                        let _ = fs::remove_dir_all(&dir);
                        Err(io_error(error))
                    }
                },
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    Err(error)
                }
            }
        })
        .await
    }
}

/// Moves a live workspace to `archived` and stamps `closed_at`, inside the caller's transaction, and appends `workspace.archived`. Used when the task's proposal integrates, when the task is archived and for the review checkouts of a proposal that ends. Only the row changes; the directory stays until garbage collection. False, with nothing changed, when the workspace is not live. The caller runs `recompute` for the task.
pub async fn archive_in_tx<E: From<StoreError> + From<EventError>>(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    workspace: WorkspaceId,
    reason: ArchiveReason,
    now: Timestamp,
) -> Result<bool, E> {
    let Some(row) = workspaces::get_workspace(tx.conn(), workspace).await? else {
        return Ok(false);
    };
    if !workspaces::close(tx, workspace, ClosedState::Archived, now).await? {
        return Ok(false);
    }
    let event = NewEvent::new(types::WORKSPACE_ARCHIVED, actor.clone())?
        .task(row.task)
        .workspace(workspace)
        .data(json!({"reason": reason.as_str()}))?;
    buffer.append(tx, &event, now).await?;
    Ok(true)
}

/// Step 3 of a create. The claim is taken, or renewed for its holder, before the row goes in, so a race with another worker ends in `already_claimed` and a race with the same worker in `workspace_exists` from the unique index.
#[expect(
    clippy::too_many_arguments,
    reason = "the step of one command, with the inputs its earlier steps produced"
)]
async fn insert_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    worker: &WorkerRow,
    task: TaskId,
    path: &str,
    base: RevisionId,
    now: Timestamp,
    lease_minutes: u64,
) -> Result<WorkspaceId, WorkspaceError> {
    claim_in_tx(tx, buffer, worker, task, now, lease_minutes).await?;
    let number = counters::allocate(tx, IdKind::Workspace).await?;
    let id = WorkspaceId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    let new = NewTaskWorkspace {
        id,
        task,
        worker_id: Some(worker.id),
        path,
        base,
        now,
    };
    match workspaces::insert_task_workspace(tx, &new).await {
        Ok(()) => {}
        Err(error) if error.unique_violation() => {
            return match workspaces::live_task_workspace(tx.conn(), task).await? {
                Some(live) => Err(workspace_exists(&live).into()),
                None => Err(error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    }
    let event = NewEvent::new(types::WORKSPACE_CREATED, EventActor::worker(&worker.name))?
        .task(task)
        .workspace(id)
        .revision(base)
        .data(json!({"path": path, "worker": worker.name.as_str()}))?;
    buffer.append(tx, &event, now).await?;
    buffer.extend(recompute(tx, &[task], now).await?.events);
    Ok(id)
}

async fn abandon_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    workspace: WorkspaceId,
    reason: &str,
    now: Timestamp,
) -> Result<(WorkspaceRow, Option<TaskStatus>), WorkspaceError> {
    let row = workspaces::get_workspace(tx.conn(), workspace)
        .await?
        .ok_or_else(|| {
            FoldError::new(
                codes::WORKSPACE_NOT_FOUND,
                format!("{workspace} does not exist"),
            )
            .with_hint("call get_workspace with the task to find its workspace")
        })?;
    if !row.state.is_live() {
        return Err(FoldError::new(
            codes::WORKSPACE_NOT_ACTIVE,
            format!("{workspace} is {}", row.state.as_str()),
        )
        .with_details([
            ("workspace", json!(workspace.to_string())),
            ("state", json!(row.state.as_str())),
        ])
        .with_hint("the workspace is already closed; call create_workspace to start again")
        .into());
    }
    if let Err(denied) = policy::can_abandon_workspace(actor, row.worker.as_ref()) {
        let owner = row.worker.as_ref().map(WorkerName::as_str);
        return Err(FoldError::from(denied)
            .with_details([
                ("workspace", json!(workspace.to_string())),
                ("owner", json!(owner)),
            ])
            .into());
    }
    if let Some(task) = row.task {
        let open_proposal = tasks::load_facts(tx.conn(), &[task])
            .await?
            .pop()
            .is_some_and(|loaded| loaded.facts.proposal != ProposalFact::None);
        if open_proposal {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{task} has an open proposal"),
            )
            .with_details([
                ("reason", json!("proposal_open")),
                ("task", json!(task.to_string())),
            ])
            .with_hint("call close_proposal first, then abandon_workspace")
            .into());
        }
    }
    workspaces::close(tx, workspace, ClosedState::Abandoned, now).await?;
    let event = NewEvent::new(types::WORKSPACE_ABANDONED, actor.event_actor())?
        .task(row.task)
        .workspace(workspace)
        .data(json!({"reason": reason}))?;
    buffer.append(tx, &event, now).await?;
    let status = match row.task {
        Some(task) => {
            buffer.extend(recompute(tx, &[task], now).await?.events);
            let fetched = tasks::fetch(tx.conn(), task)
                .await?
                .ok_or_else(|| task_not_found(task))?;
            Some(fetched.status)
        }
        None => None,
    };
    let closed = workspaces::get_workspace(tx.conn(), workspace)
        .await?
        .ok_or(StoreError::Corrupt("workspaces.number"))?;
    Ok((closed, status))
}

pub(super) fn workspace_exists(live: &WorkspaceRow) -> FoldError {
    let owner = live
        .task
        .map_or_else(|| "main".to_owned(), |task| task.to_string());
    FoldError::new(
        codes::WORKSPACE_EXISTS,
        format!("{owner} already has the workspace {}", live.id),
    )
    .with_details([
        ("workspace", json!(live.id.to_string())),
        ("path", json!(live.path)),
    ])
    .with_hint("call get_workspace to use the existing workspace")
}

pub(super) fn io_error(error: io::Error) -> FoldError {
    VcsError::from(error).into()
}

/// A new empty directory `.fold/ws/.new-<pid>-<n>`. A name that exists, from an earlier daemon with the same process id, is skipped rather than reused.
pub(super) fn staging_dir(ws_dir: &Path) -> Result<PathBuf, FoldError> {
    loop {
        let number = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = ws_dir.join(format!(".new-{}-{number}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(error)),
        }
    }
}

/// Renames `staging` to the first name after the `taken` ones that no row lists and nothing holds on disk, and returns the name relative to `root` with the full path. `name` gives the directory for a count of earlier directories. A rename never replaces a directory with entries, a file or a symlink, so whatever already holds a name stays untouched. It does replace an empty directory, which holds nothing to lose; a concurrent create's directory is never empty, because materialise leaves `.fold/tmp` in it.
pub(super) fn rename_into_place(
    staging: &Path,
    root: &Path,
    name: &dyn Fn(u32) -> String,
    taken: &[String],
) -> Result<(String, PathBuf), FoldError> {
    let earlier = u32::try_from(taken.len()).unwrap_or(u32::MAX);
    let mut last = None;
    for step in 0..MAX_NAME_ATTEMPTS {
        let path = name(earlier.saturating_add(step));
        if taken.contains(&path) {
            last = Some(path);
            continue;
        }
        let dir = root.join(&path);
        match fs::rename(staging, &dir) {
            Ok(()) => return Ok((path, dir)),
            Err(error) if occupied(&error) => last = Some(path),
            Err(error) => return Err(io_error(error)),
        }
    }
    let path = last.unwrap_or_else(|| name(earlier));
    Err(VcsError::TargetNotEmpty(path).into())
}

fn occupied(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::DirectoryNotEmpty
            | io::ErrorKind::AlreadyExists
            | io::ErrorKind::NotADirectory
    )
}

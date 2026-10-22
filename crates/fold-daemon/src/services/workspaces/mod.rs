use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fold_core::{
    Clock, EventError, FoldError, ProjectConfig, ProjectPaths, ProposalId, RevisionId, WorkspaceId,
    codes,
};
use fold_store::{Store, StoreError, reviews};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use thiserror::Error;
use tokio::sync::{OwnedMutexGuard, RwLock};

use super::events::EventBus;
use super::projects::{Lease, ProjectError, load_config};
use super::tasks::TaskError;
use crate::live::LiveState;
use crate::projects::ProjectHandle;
use crate::support::{self, lock};

mod adoption;
mod attachments;
mod auto;
mod changes;
mod changesets;
mod checkpoints;
mod checks;
mod cleanup;
mod compare;
mod direct;
mod exclude;
mod files;
mod human_change;
mod human_edit;
mod integration;
mod lifecycle;
mod reconcile;
mod repository;
mod review_fix;
mod stalls;
mod sync;
mod sync_task;

pub use adoption::AdoptReason;
pub(crate) use adoption::{adopt_if_orphaned, hand_over_proposal};
pub use attachments::WorkspaceFile;
pub use auto::{AutoCheckpoint, QuietCheckpoint};
pub(crate) use auto::{auto_checkpoint_in_tx, pruned_event};
pub use changes::{WorkspaceChanges, WorkspaceEntry};
pub(crate) use changesets::ProposalSnapshot;
pub use changesets::RecordedChanges;
pub use checkpoints::{CheckpointView, Restored};
pub use cleanup::{DirectoryRemoval, follow_closed_workspaces};
pub(crate) use compare::CARRY_CACHE_ENTRIES;
pub use compare::{
    CarriedComparison, Carry, CarryCache, ResolvedState, StateComparison, StateKind, StateRef,
    StateSummary,
};
pub use direct::OpenedMainWorkspace;
pub(crate) use direct::{main_agent, main_agent_in_tx};
pub(crate) use files::checkpoint_in_tx;
pub use human_change::TouchedTask;
pub use lifecycle::{
    Abandoned, ArchiveReason, CreatedWorkspace, WorkspaceDetails, WorkspaceRef, WorkspaceView,
    archive_in_tx,
};
pub use reconcile::{Reconciled, Reconciliation};
pub(crate) use repository::file_path;
pub use repository::{
    DEFAULT_REVISION_PAGE, RECENT_REVISIONS, RepositoryState, RevisionDetail, RevisionOrigin,
    RevisionPage, RevisionSummary,
};
pub(crate) use review_fix::ReviewFixTree;
pub use stalls::check_stalls;
pub use sync::Synced;
pub use sync_task::{SyncTaskCreated, SyncWorkspace, short_commit};

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A refusal of the claim or the status step a workspace command runs, such as `already_claimed`.
    #[error(transparent)]
    Task(#[from] TaskError),
    /// A command built an event the declared types refuse: a defect in the command, not in the input.
    #[error(transparent)]
    Event(#[from] EventError),
}

impl WorkspaceError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Store(error) => error.code(),
            Self::Task(error) => error.code(),
            Self::Event(_) => None,
        }
    }

    /// The code a log line names: the stable code, or `internal_error` for an internal failure.
    pub fn log_code(&self) -> &'static str {
        self.code().unwrap_or(codes::INTERNAL_ERROR)
    }
}

impl From<ProjectError> for WorkspaceError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Fold(error) => Self::Fold(error),
            ProjectError::Store(error) => Self::Store(error),
        }
    }
}

/// Commands and reads for the revisions, `main` and the workspaces of one project. Each command is one write transaction; its events reach the bus only after the commit. `lease` is the project's claim lease, so the claim a create takes follows the rules of `claim_task`.
#[derive(Clone)]
pub struct WorkspaceService {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    paths: ProjectPaths,
    lease: Lease,
    /// Probed by the first command that touches files and shared by every clone of the service; in the daemon, by every service of the project.
    caps: Arc<OnceLock<Capabilities>>,
    /// The project's object lock (`ProjectHandle::objects`): a command that writes objects and then records a row naming them holds it shared from its first object write until that row commits.
    objects: Arc<RwLock<()>>,
    ops: Arc<WorkspaceLocks>,
    live: Option<LiveState>,
    /// The project's carried states (`ProjectHandle::carries`), so every request's reads of one revised proposal share one merge.
    carries: Arc<CarryCache>,
}

impl std::fmt::Debug for WorkspaceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceService")
            .field("paths", &self.paths)
            .field("lease", &self.lease)
            .finish_non_exhaustive()
    }
}

impl WorkspaceService {
    pub fn new(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        paths: ProjectPaths,
        lease_minutes: u64,
    ) -> Self {
        Self {
            store,
            bus,
            clock,
            paths,
            lease: Lease::Minutes(lease_minutes),
            caps: Arc::new(OnceLock::new()),
            objects: Arc::new(RwLock::new(())),
            ops: Arc::new(WorkspaceLocks::default()),
            live: None,
            carries: Arc::new(CarryCache::default()),
        }
    }

    /// The service of an open project in the daemon. It shares the project's object lock, workspace locks and capabilities probe, so every session's commands and garbage collection keep out of each other's way. The claim lease is read from `config.toml` by each command that needs it, so an invalid file refuses no session or route.
    pub fn for_project(handle: &ProjectHandle) -> Self {
        Self::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
            handle.paths.clone(),
            ProjectConfig::default().lease_minutes,
        )
        .with_project_config()
        .with_locks(handle.objects.clone(), handle.workspace_locks.clone())
        .with_capabilities(handle.capabilities.clone())
        .with_live(handle.live.clone())
        .with_carries(handle.carries.clone())
    }

    /// Reads the claim lease from the service's project `config.toml` on every command that takes a claim; an invalid file fails that command with `invalid_config`.
    #[must_use]
    pub fn with_project_config(mut self) -> Self {
        self.lease = Lease::config(self.paths.clone());
        self
    }

    /// Where the claim lease comes from, so the review claims of the project follow the same lease.
    pub(crate) fn lease(&self) -> &Lease {
        &self.lease
    }

    /// Shares one capabilities probe with every other service of the project.
    #[must_use]
    pub fn with_capabilities(mut self, caps: Arc<OnceLock<Capabilities>>) -> Self {
        self.caps = caps;
        self
    }

    /// Shares one cache of carried states with every other service of the project.
    #[must_use]
    pub fn with_carries(mut self, carries: Arc<CarryCache>) -> Self {
        self.carries = carries;
        self
    }

    /// Replaces the service's own locks with shared ones. A service built by [`WorkspaceService::new`] alone holds locks nobody else sees.
    #[must_use]
    pub fn with_locks(mut self, objects: Arc<RwLock<()>>, ops: Arc<WorkspaceLocks>) -> Self {
        self.objects = objects;
        self.ops = ops;
        self
    }

    /// Keeps the live state of the workspaces this service creates, adopts and closes. A service without it, such as one a host command opens, starts no watcher.
    #[must_use]
    pub fn with_live(mut self, live: LiveState) -> Self {
        self.live = Some(live);
        self
    }

    /// Brings the live state up to date before a command reads the working files. A workspace with no live state, such as one created before the daemon started, is left alone.
    async fn live_rescan(&self, workspace: WorkspaceId) {
        if let Some(live) = &self.live
            && let Err(error) = live.rescan(workspace).await
        {
            tracing::debug!(workspace = %workspace, code = error.log_code(), "the live state was not rescanned");
        }
    }

    /// Gives the live state the workspace's checkpoints after a command that recorded one, whether or not the command finished.
    async fn live_checkpoints(&self, workspace: WorkspaceId) {
        if let Some(live) = &self.live {
            live.reload_checkpoints(workspace).await;
        }
    }

    /// Gives the live state of `workspace` its open proposal, the tree of the proposal's newest version to compare the working files against, and its checkpoints after the commit of a proposal command.
    pub(crate) async fn proposal_changed(
        &self,
        workspace: WorkspaceId,
        proposal: Option<ProposalId>,
    ) {
        let Some(live) = &self.live else {
            return;
        };
        let tree = match proposal {
            Some(proposal) => match reviews::list_versions(self.store.read(), proposal).await {
                Ok(mut versions) => versions.pop().map(|version| version.tree),
                Err(error) => {
                    tracing::warn!(workspace = %workspace, code = error.log_code(), "the version tree was not read");
                    None
                }
            },
            None => None,
        };
        live.set_proposal(workspace, proposal, tree).await;
        live.reload_checkpoints(workspace).await;
    }

    /// After the commit of a sync or restore: the live state compares against the new base and scans the files the command wrote.
    async fn live_base(&self, workspace: WorkspaceId, base: RevisionId) {
        let Some(live) = &self.live else {
            return;
        };
        let moved = match self.tree_of(base).await {
            Ok(tree) => live.set_base(workspace, base, tree).await,
            Err(error) => Err(error),
        };
        if let Err(error) = moved {
            tracing::debug!(workspace = %workspace, code = error.log_code(), "the live state did not take the new base");
        }
    }

    /// The service of the project at `paths` for a host command that runs without the daemon, with a bus nobody listens to and the project's configured lease. A directory without a project database is `project_not_found`, and nothing is created in it.
    pub async fn open_local(
        paths: ProjectPaths,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, WorkspaceError> {
        let db = paths.db();
        let found = tokio::task::spawn_blocking(move || db.is_file())
            .await
            .unwrap_or(false);
        if !found {
            return Err(FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("{} is not a fold project", paths.root().display()),
            )
            .with_hint("run the command inside a project directory or pass --project")
            .into());
        }
        let config = load_config(&paths).await?;
        let store = Store::open_project(&paths).await?;
        Ok(Self::new(
            store,
            EventBus::new(),
            clock,
            paths,
            config.lease_minutes,
        ))
    }

    /// Closes the database this service opened. A failed close only delays the WAL checkpoint.
    pub async fn close(self) {
        let _ = self.store.close().await;
    }
}

/// One operation per workspace at a time. Restore, sync and record each scan a workspace and then write files or rows from that scan, so a second command between the two would act on a stale scan.
#[derive(Debug, Default)]
pub struct WorkspaceLocks(Mutex<HashMap<WorkspaceId, Arc<tokio::sync::Mutex<()>>>>);

impl WorkspaceLocks {
    /// Waits until no other operation runs on `workspace`. A lock that no guard or waiter holds any more is dropped from the map here.
    pub async fn lock(&self, workspace: WorkspaceId) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = lock(&self.0);
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            locks.entry(workspace).or_default().clone()
        };
        lock.lock_owned().await
    }

    /// The lock of `workspace` when no other operation holds it, for work that is skipped rather than queued behind a command.
    pub fn try_lock(&self, workspace: WorkspaceId) -> Option<OwnedMutexGuard<()>> {
        let lock = {
            let mut locks = lock(&self.0);
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            locks.entry(workspace).or_default().clone()
        };
        lock.try_lock_owned().ok()
    }
}

/// The platform capabilities, probed in `.fold` on first use and shared by every clone of the service. Blocking work only.
pub(crate) fn capabilities(
    probed: &OnceLock<Capabilities>,
    paths: &ProjectPaths,
) -> Result<Capabilities, FoldError> {
    if let Some(caps) = probed.get() {
        return Ok(*caps);
    }
    let caps = Capabilities::probe(&paths.fold_dir())
        .map_err(|error| FoldError::from(fold_vcs::paths::WorkspaceError::from(error)))?;
    Ok(*probed.get_or_init(|| caps))
}

/// Opening creates the store's directories, so it runs inside the blocking work.
fn open_objects(paths: &ProjectPaths) -> Result<ObjectStore, FoldError> {
    ObjectStore::open_existing(&paths.fold_dir()).map_err(FoldError::from)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, FoldError> + Send + 'static,
) -> Result<T, WorkspaceError> {
    Ok(support::blocking(work).await?)
}

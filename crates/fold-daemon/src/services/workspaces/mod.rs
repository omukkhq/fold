use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use fold_core::{Clock, EventError, FoldError, ProjectConfig, ProjectPaths, WorkspaceId, codes};
use fold_store::{Store, StoreError};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use thiserror::Error;
use tokio::sync::{OwnedMutexGuard, RwLock};

use super::events::EventBus;
use super::projects::{Lease, ProjectError, load_config};
use super::tasks::TaskError;
use crate::projects::ProjectHandle;

mod changesets;
mod checkpoints;
mod compare;
mod files;
mod git;
mod lifecycle;
mod repository;
mod sync;

pub(crate) use changesets::ProposalSnapshot;
pub use changesets::RecordedChanges;
pub use checkpoints::{CheckpointView, Restored};
pub use compare::{ResolvedState, StateComparison, StateKind, StateRef, StateSummary};
pub(crate) use files::checkpoint_in_tx;
pub(crate) use git::unsynced;
pub use git::{GitExport, MAIN_REF, commit_message};
pub use lifecycle::{ArchiveReason, WorkspaceDetails, WorkspaceRef, WorkspaceView, archive_in_tx};
pub use repository::{
    DEFAULT_REVISION_PAGE, RECENT_REVISIONS, RepositoryState, RevisionDetail, RevisionOrigin,
    RevisionPage, RevisionSummary,
};
pub use sync::Synced;

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
    git: Git,
    lease: Lease,
    /// Probed by the first command that touches files and shared by every clone of the service; in the daemon, by every service of the project.
    caps: Arc<OnceLock<Capabilities>>,
    /// The project's object lock (`ProjectHandle::objects`): a command that writes objects and then records a row naming them holds it shared from its first object write until that row commits.
    objects: Arc<RwLock<()>>,
    ops: Arc<WorkspaceLocks>,
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
        git: Git,
        lease_minutes: u64,
    ) -> Self {
        Self {
            store,
            bus,
            clock,
            paths,
            git,
            lease: Lease::Minutes(lease_minutes),
            caps: Arc::new(OnceLock::new()),
            objects: Arc::new(RwLock::new(())),
            ops: Arc::new(WorkspaceLocks::default()),
        }
    }

    /// The service of an open project in the daemon. It shares the project's object lock, workspace locks and capabilities probe, so every session's commands and garbage collection keep out of each other's way. The claim lease is read from `config.toml` by each command that needs it, so an invalid file refuses no session or route.
    pub fn for_project(handle: &ProjectHandle, git: Git) -> Self {
        Self::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
            handle.paths.clone(),
            git,
            ProjectConfig::default().lease_minutes,
        )
        .with_project_config()
        .with_locks(handle.objects.clone(), handle.workspace_locks.clone())
        .with_capabilities(handle.capabilities.clone())
    }

    /// Reads the claim lease from the service's project `config.toml` on every command that takes a claim; an invalid file fails that command with `invalid_config`.
    #[must_use]
    pub fn with_project_config(mut self) -> Self {
        self.lease = Lease::Config(self.paths.clone());
        self
    }

    /// Shares one capabilities probe with every other service of the project.
    #[must_use]
    pub fn with_capabilities(mut self, caps: Arc<OnceLock<Capabilities>>) -> Self {
        self.caps = caps;
        self
    }

    /// Replaces the service's own locks with shared ones. A service built by [`WorkspaceService::new`] alone holds locks nobody else sees.
    #[must_use]
    pub fn with_locks(mut self, objects: Arc<RwLock<()>>, ops: Arc<WorkspaceLocks>) -> Self {
        self.objects = objects;
        self.ops = ops;
        self
    }

    /// The service of the project at `paths` for a host command that runs without the daemon, with a bus nobody listens to and the project's configured lease. A directory without a project database is `project_not_found`, and nothing is created in it.
    pub async fn open_local(
        paths: ProjectPaths,
        git: Git,
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
            git,
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
            let mut locks = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            locks.entry(workspace).or_default().clone()
        };
        lock.lock_owned().await
    }
}

/// The platform capabilities, probed in `.fold` on first use and shared by every clone of the service. Blocking work only.
fn capabilities(
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
    ObjectStore::open(&paths.fold_dir()).map_err(FoldError::from)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, FoldError> + Send + 'static,
) -> Result<T, WorkspaceError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(
                codes::INTERNAL_ERROR,
                format!("blocking work stopped: {error}"),
            ))
        })
        .map_err(WorkspaceError::from)
}

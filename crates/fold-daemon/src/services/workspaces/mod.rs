use std::sync::{Arc, OnceLock};

use fold_core::{Clock, EventError, FoldError, ProjectPaths, codes};
use fold_store::{Store, StoreError};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use thiserror::Error;

use super::events::EventBus;
use super::projects::{ProjectError, load_config};
use super::tasks::TaskError;

mod compare;
mod git;
mod lifecycle;
mod repository;

pub use compare::{ResolvedState, StateComparison, StateKind, StateRef, StateSummary};
pub(crate) use git::unsynced;
pub use git::{GitExport, MAIN_REF, commit_message};
pub use lifecycle::{ArchiveReason, WorkspaceRef, WorkspaceView, archive_in_tx};
pub use repository::{
    DEFAULT_REVISION_PAGE, RECENT_REVISIONS, RepositoryState, RevisionDetail, RevisionOrigin,
    RevisionPage, RevisionSummary,
};

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

/// Commands and reads for the revisions, `main` and the workspaces of one project. Each command is one write transaction; its events reach the bus only after the commit. `lease_minutes` is the project's configured claim lease, so the claim a create takes follows the rules of `claim_task`.
#[derive(Clone)]
pub struct WorkspaceService {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    paths: ProjectPaths,
    git: Git,
    lease_minutes: u64,
    /// Probed by the first create or working-files read and shared by every clone of the service.
    caps: Arc<OnceLock<Capabilities>>,
}

impl std::fmt::Debug for WorkspaceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceService")
            .field("paths", &self.paths)
            .field("lease_minutes", &self.lease_minutes)
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
            lease_minutes,
            caps: Arc::new(OnceLock::new()),
        }
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

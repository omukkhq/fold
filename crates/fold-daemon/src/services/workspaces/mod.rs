use fold_core::{FoldError, ProjectPaths, codes};
use fold_store::{Store, StoreError};
use fold_vcs::git::Git;
use thiserror::Error;

mod git;

pub use git::{GitExport, MAIN_REF, commit_message};

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl WorkspaceError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Store(error) => error.code(),
        }
    }
}

/// Commands and reads for the revisions, `main` and the workspaces of one project.
#[derive(Debug, Clone)]
pub struct WorkspaceService {
    store: Store,
    paths: ProjectPaths,
    git: Git,
}

impl WorkspaceService {
    pub fn new(store: Store, paths: ProjectPaths, git: Git) -> Self {
        Self { store, paths, git }
    }

    /// The service of the project at `paths` for a host command that runs without the daemon. A directory without a project database is `project_not_found`, and nothing is created in it.
    pub async fn open_local(paths: ProjectPaths, git: Git) -> Result<Self, WorkspaceError> {
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
        let store = Store::open_project(&paths).await?;
        Ok(Self::new(store, paths, git))
    }

    /// Closes the database this service opened. A failed close only delays the WAL checkpoint.
    pub async fn close(self) {
        let _ = self.store.close().await;
    }
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

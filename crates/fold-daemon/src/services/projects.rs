use std::sync::Arc;

use fold_core::{FoldError, codes};
use fold_store::StoreError;
use thiserror::Error;

use crate::daemon::DaemonState;
use crate::projects::{ProjectHandle, Slot};

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A registry row and whether the daemon has the project open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    pub slug: String,
    pub name: String,
    pub available: bool,
}

/// Every registered project, ordered by slug.
pub async fn list(state: &DaemonState) -> Result<Vec<ProjectEntry>, ProjectError> {
    let rows = fold_store::projects::list_projects(state.registry.read()).await?;
    Ok(rows
        .into_iter()
        .map(|row| ProjectEntry {
            available: matches!(state.projects.slot(&row.slug), Some(Slot::Open(_))),
            slug: row.slug,
            name: row.name,
        })
        .collect())
}

/// Opens a project that `fold init` registered after the daemon started. An open project returns its handle again; a slug the registry does not hold is `project_not_found`.
pub async fn open(state: &DaemonState, slug: &str) -> Result<Arc<ProjectHandle>, ProjectError> {
    let row = fold_store::projects::find_project(state.registry.read(), slug)
        .await?
        .ok_or_else(|| {
            FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("project {slug} is not registered"),
            )
        })?;
    Ok(state.projects.open(&row, state.clock.clone()).await?)
}

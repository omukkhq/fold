use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fold_core::{
    CONFIG_TEMPLATE, Clock, FoldError, HostPaths, ProjectConfig, ProjectPaths, choose_slug, codes,
    slugify,
};
use fold_store::projects::{self, ProjectRow};
use fold_store::{Store, StoreError};
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

impl ProjectError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Store(error) => error.code(),
        }
    }
}

/// A project that `init_project` registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializedProject {
    pub slug: String,
    pub name: String,
    /// The canonical project directory.
    pub root: PathBuf,
}

/// Creates `.fold/` in `root`, migrates the project database, writes the commented config and registers the project under a free slug. Init is a host command, so it has no actor and appends no event. A failure after `.fold/` exists removes the directory this call made.
pub async fn init_project(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let root = prepare_root(root).await?;
    let root_text = root
        .to_str()
        .ok_or_else(|| {
            FoldError::new(
                codes::INVALID_PATH,
                "the project directory is not valid UTF-8",
            )
        })?
        .to_owned();
    let name = match name {
        Some(name) => name.to_owned(),
        None => root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let registry = Store::open_registry(&host.registry_db()).await?;
    let outcome = create_and_register(&registry, &root, &root_text, name, clock).await;
    // The outcome is what the caller needs; a failed close only delays the WAL checkpoint.
    let _ = registry.close().await;
    outcome
}

async fn create_and_register(
    registry: &Store,
    root: &Path,
    root_text: &str,
    name: String,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    if projects::find_by_root(registry.read(), root_text)
        .await?
        .is_some()
    {
        return Err(exists(root).into());
    }
    let paths = ProjectPaths::new(root);
    create_fold_dir(&paths).await?;
    match populate_and_register(registry, &paths, root_text, name, clock).await {
        Ok(project) => Ok(project),
        Err(error) => {
            remove_fold_dir(&paths).await;
            Err(error)
        }
    }
}

async fn populate_and_register(
    registry: &Store,
    paths: &ProjectPaths,
    root_text: &str,
    name: String,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let project_store = Store::open_project(paths).await?;
    let closed = project_store.close().await;
    write_config(paths).await?;
    closed?;
    let now = clock.now();
    let base = slugify(&name);
    let row = registry
        .write(async |tx| -> Result<ProjectRow, ProjectError> {
            let existing: BTreeSet<String> = projects::slugs(tx).await?.into_iter().collect();
            let row = ProjectRow {
                slug: choose_slug(&base, &existing),
                name,
                root: root_text.to_owned(),
            };
            match projects::insert_project(tx, &row, now).await {
                Ok(()) => Ok(row),
                Err(error) if error.unique_violation() => Err(exists(Path::new(root_text)).into()),
                Err(error) => Err(error.into()),
            }
        })
        .await?;
    Ok(InitializedProject {
        slug: row.slug,
        name: row.name,
        root: PathBuf::from(row.root),
    })
}

/// The settings of a project, or the defaults when `.fold/config.toml` does not exist.
pub async fn load_config(paths: &ProjectPaths) -> Result<ProjectConfig, ProjectError> {
    let path = paths.config();
    let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(path)).await;
    match flatten(read) {
        Ok(text) => Ok(ProjectConfig::from_toml(&text)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ProjectConfig::default()),
        Err(error) => Err(io_error(&error)),
    }
}

async fn prepare_root(root: &Path) -> Result<PathBuf, ProjectError> {
    let root = root.to_owned();
    let made = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&root)?;
        std::fs::canonicalize(&root)
    })
    .await;
    flatten(made).map_err(|error| io_error(&error))
}

/// `create_dir` fails when `.fold/` exists, so two inits of one directory cannot both proceed.
async fn create_fold_dir(paths: &ProjectPaths) -> Result<(), ProjectError> {
    let dir = paths.fold_dir();
    let made = tokio::task::spawn_blocking(move || std::fs::create_dir(dir)).await;
    match flatten(made) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(exists(paths.root()).into())
        }
        Err(error) => Err(io_error(&error)),
    }
}

async fn write_config(paths: &ProjectPaths) -> Result<(), ProjectError> {
    let path = paths.config();
    let wrote = tokio::task::spawn_blocking(move || std::fs::write(path, CONFIG_TEMPLATE)).await;
    flatten(wrote).map_err(|error| io_error(&error))
}

async fn remove_fold_dir(paths: &ProjectPaths) {
    let dir = paths.fold_dir();
    // The original error is what the caller needs; a directory that stays behind makes the next init refuse with `project_exists`.
    let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await;
}

fn flatten<T>(joined: Result<io::Result<T>, tokio::task::JoinError>) -> io::Result<T> {
    joined.unwrap_or_else(|error| Err(io::Error::other(error)))
}

fn exists(root: &Path) -> FoldError {
    FoldError::new(
        codes::PROJECT_EXISTS,
        format!("{} is already a fold project", root.display()),
    )
    .with_hint("open the project that exists there instead of running init again")
}

fn io_error(error: &io::Error) -> ProjectError {
    ProjectError::Fold(FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot set up the project layout: {}", error.kind()),
    ))
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
    let rows = projects::list_projects(state.registry.read()).await?;
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
    let row = projects::find_project(state.registry.read(), slug)
        .await?
        .ok_or_else(|| {
            FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("project {slug} is not registered"),
            )
        })?;
    Ok(state.projects.open(&row, state.clock.clone()).await?)
}

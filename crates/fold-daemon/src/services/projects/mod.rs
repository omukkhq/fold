use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fold_core::projects::find_root;
use fold_core::tree::FlatTree;
use fold_core::{
    CONFIG_TEMPLATE, Clock, FoldError, HostPaths, HumanName, ProjectConfig, ProjectPaths,
    RevisionId, Timestamp, choose_slug, codes, slugify,
};
use fold_store::projects::{self, HumanRow, ProjectRow};
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_store::{Store, StoreError};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_import::{GitImportOptions, archive_tree};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::import::{ImportOptions, ImportResult, import_dir};
use fold_vcs::materialise::{MaterialiseOptions, materialise};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::WorkspaceError;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use thiserror::Error;

use crate::daemon::DaemonState;
use crate::projects::{ProjectHandle, Slot};

mod gc;
mod remote;

pub use gc::{GcReport, gc};
pub use remote::{PushOutcome, push};

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
    /// `r1`, which `main` names.
    pub revision: RevisionId,
    /// What `r1` holds: the empty tree when nothing was imported.
    pub imported: ImportResult,
    /// The commit a Git import read, which `.fold/git-sync.map` records as `r1`.
    pub git_commit: Option<CommitId>,
}

/// An import source that passed the checks: an existing directory, canonical, with a UTF-8 path.
struct Source {
    path: PathBuf,
    text: String,
    kind: SourceKind,
}

enum SourceKind {
    Directory,
    Git(Git),
}

/// [`init_project_from`] with no import source, so `r1` is the empty tree.
pub async fn init_project(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    init_project_from(host, root, name, None, clock).await
}

/// Creates `.fold/` in `root`, migrates the project database, writes the commented config, records `r1` from `source` (or the empty tree) as `main`, fills `main/` read-only and only then registers the project under a free slug. Init is a host command, so it has no actor and appends no event. A failure after `.fold/` exists removes the directory this call made and puts `main/` back as it was: absent or empty, the only states init accepts. `source` is only read, so a project directory at or below it is refused.
pub async fn init_project_from(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    source: Option<&Path>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let source = match source {
        Some(source) => Some(check_source(source, SourceKind::Directory, "--from").await?),
        None => None,
    };
    init_with(host, root, name, source, clock).await
}

/// [`init_project_from`] with `r1` holding the committed tree at the `HEAD` of the Git repository whose top is `repo`, free of ignore rules, and `.fold/git-sync.map` starting with `r1 <commit>`, the first sync point. `HEAD` is resolved to its commit first, so the map names exactly the commit whose tree was read.
pub async fn init_project_from_git(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    repo: &Path,
    git: Git,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let source = check_source(repo, SourceKind::Git(git.clone()), "--from-git").await?;
    let path = source.path.clone();
    let found = blocking(move || Ok(git.repository(&path)?)).await?;
    if found.is_none() {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{} is not the top of a Git repository", source.text),
        )
        .with_hint("pass the top directory of a Git repository to --from-git")
        .into());
    }
    init_with(host, root, name, Some(source), clock).await
}

async fn init_with(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    source: Option<Source>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    if let Some(source) = &source {
        check_apart(source, root).await?;
    }
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
    let outcome =
        create_and_register(&registry, &root, &root_text, name, source.as_ref(), clock).await;
    // The outcome is what the caller needs; a failed close only delays the WAL checkpoint.
    let _ = registry.close().await;
    outcome
}

async fn create_and_register(
    registry: &Store,
    root: &Path,
    root_text: &str,
    name: String,
    source: Option<&Source>,
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
    let main = match check_main(&paths).await {
        Ok(main) => main,
        Err(error) => {
            remove_created(&paths, None).await;
            return Err(error);
        }
    };
    match populate_and_register(registry, &paths, root_text, name, source, clock).await {
        Ok(project) => Ok(project),
        Err(error) => {
            remove_created(&paths, Some(main)).await;
            Err(error)
        }
    }
}

async fn populate_and_register(
    registry: &Store,
    paths: &ProjectPaths,
    root_text: &str,
    name: String,
    source: Option<&Source>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let now = clock.now();
    let project_store = Store::open_project(paths).await?;
    let first = async {
        write_config(paths).await?;
        let config = load_config(paths).await?;
        create_first_revision(&project_store, paths, source, config.max_file_bytes, now).await
    }
    .await;
    let closed = project_store.close().await;
    let (revision, imported, git_commit) = first?;
    closed?;
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
        revision,
        imported,
        git_commit,
    })
}

/// Objects first, then the one transaction that inserts `r1` and creates `main`, then the sync map that names `r1`, then `main/`.
async fn create_first_revision(
    store: &Store,
    paths: &ProjectPaths,
    source: Option<&Source>,
    max_file_bytes: u64,
    now: Timestamp,
) -> Result<(RevisionId, ImportResult, Option<CommitId>), ProjectError> {
    let fold_dir = paths.fold_dir();
    let source_path = source.map(|source| source.path.clone());
    let git = match source.map(|source| &source.kind) {
        Some(SourceKind::Git(git)) => Some(git.clone()),
        _ => None,
    };
    let (objects, caps, imported, stored, git_commit) = blocking(move || {
        let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
        let caps = Capabilities::probe(&fold_dir).map_err(|error| io_error(&error))?;
        let (imported, git_commit) = match (source_path, git) {
            (Some(repo), Some(git)) => {
                let commit = git.resolve_commit(&repo, "HEAD")?;
                let opts = GitImportOptions { max_file_bytes };
                let imported = archive_tree(&objects, &git, &repo, commit.as_str(), &opts)?;
                (imported, Some(commit))
            }
            (Some(source), None) => {
                let opts = ImportOptions {
                    max_file_bytes,
                    caps,
                };
                let rules = IgnoreRules::new(&source);
                (import_dir(&objects, &source, &rules, &opts)?, None)
            }
            (None, _) => {
                let empty = ImportResult {
                    tree: write_initial_tree(&objects, FlatTree::new())?,
                    files: 0,
                    bytes: 0,
                    skipped: Vec::new(),
                };
                (empty, None)
            }
        };
        let revision = initial_revision(imported.tree, now)?;
        let stored = write_revision(&objects, &revision).map_err(FoldError::from)?;
        Ok((objects, caps, imported, stored, git_commit))
    })
    .await?;
    let import_source = source.map(|source| source.text.as_str());
    let row = create_initial_revision(store, &stored, import_source).await?;
    let main = paths.main_dir();
    let tree = imported.tree;
    let fold_dir = paths.fold_dir();
    let synced = git_commit.clone().map(|commit| (row.number, commit));
    blocking(move || {
        if let Some(synced) = synced {
            SyncMap::load(&fold_dir)?.append(&[synced])?;
        }
        let opts = MaterialiseOptions {
            read_only: true,
            caps,
        };
        materialise(&objects, &tree, &main, &opts).map_err(FoldError::from)?;
        Ok(())
    })
    .await?;
    Ok((row.number, imported, git_commit))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ProjectError> + Send + 'static,
) -> Result<T, ProjectError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| Err(io_error(&io::Error::other(error))))
}

/// Init writes `.fold/` and `main/` into the project directory, so one at or below the source would change the source.
async fn check_apart(source: &Source, root: &Path) -> Result<(), ProjectError> {
    let given = root.to_owned();
    let planned = tokio::task::spawn_blocking(move || planned_path(&given)).await;
    let planned = flatten(planned).map_err(|error| io_error(&error))?;
    if planned.starts_with(&source.path) {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!(
                "the project directory {} is inside the import source {}",
                root.display(),
                source.path.display()
            ),
        )
        .with_hint("create the project in a directory outside the source")
        .into());
    }
    Ok(())
}

/// The canonical path `root` has once `create_dir_all` makes it: its deepest existing part canonicalised, then the rest with `.` and `..` applied in order, as the kernel applies them once the parents exist.
fn planned_path(root: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(root)?;
    let parts: Vec<Component<'_>> = absolute.components().collect();
    for split in (1..=parts.len()).rev() {
        let existing: PathBuf = parts[..split].iter().collect();
        let mut planned = match std::fs::canonicalize(&existing) {
            Ok(found) => found,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for part in &parts[split..] {
            match part {
                Component::ParentDir => {
                    planned.pop();
                }
                Component::Normal(name) => planned.push(name),
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
        return Ok(planned);
    }
    Err(io::ErrorKind::NotFound.into())
}

async fn check_source(source: &Path, kind: SourceKind, flag: &str) -> Result<Source, ProjectError> {
    let given = source.to_owned();
    let found = tokio::task::spawn_blocking(move || {
        let path = std::fs::canonicalize(&given).ok()?;
        std::fs::metadata(&path).ok()?.is_dir().then_some(path)
    })
    .await
    .ok()
    .flatten();
    let refused = |reason: &str| {
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("the import source {} {reason}", source.display()),
        )
        .with_hint(format!("pass an existing directory to {flag}"))
    };
    let path = found.ok_or_else(|| refused("is not an existing directory"))?;
    let text = path
        .to_str()
        .ok_or_else(|| refused("is not a valid UTF-8 path"))?
        .to_owned();
    Ok(Source { path, text, kind })
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

/// What `main/` was before this init filled it.
#[derive(Debug, Clone, Copy)]
enum MainDir {
    Absent,
    Empty,
}

/// `main/` must be absent or an empty directory. Checked before any import, so a large source is not read only to be refused; a symlink in its place is refused too, so nothing is written through it.
async fn check_main(paths: &ProjectPaths) -> Result<MainDir, ProjectError> {
    let main = paths.main_dir();
    let found = tokio::task::spawn_blocking(move || -> io::Result<Option<MainDir>> {
        match std::fs::symlink_metadata(&main) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Some(MainDir::Absent)),
            Err(error) => Err(error),
            Ok(meta) if meta.is_dir() && std::fs::read_dir(&main)?.next().is_none() => {
                Ok(Some(MainDir::Empty))
            }
            Ok(_) => Ok(None),
        }
    })
    .await;
    match flatten(found) {
        Ok(Some(main)) => Ok(main),
        Ok(None) => {
            let main = paths.main_dir().to_string_lossy().into_owned();
            Err(FoldError::from(WorkspaceError::TargetNotEmpty(main)).into())
        }
        Err(error) => Err(io_error(&error)),
    }
}

/// `main/` is the read-only view of `r1`, never a workspace: no agent wrote to it and no checkpoint refers to it, so the files this failed run put there are the only ones removed.
async fn remove_created(paths: &ProjectPaths, main: Option<MainDir>) {
    let fold = paths.fold_dir();
    let main_dir = paths.main_dir();
    // The original error is what the caller needs; a `.fold/` that stays behind makes the next init refuse with `project_exists`.
    let _ = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(fold);
        let _ = match main {
            None => Ok(()),
            Some(MainDir::Absent) => std::fs::remove_dir_all(main_dir),
            Some(MainDir::Empty) => empty_dir(&main_dir),
        };
    })
    .await;
}

fn empty_dir(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
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

/// Opens the project whose registered root holds `cwd`, the nearest one when roots nest, so a session started in a task workspace finds its project.
pub async fn open_for_dir(
    state: &DaemonState,
    cwd: &Path,
) -> Result<Arc<ProjectHandle>, ProjectError> {
    let slug = slug_for_dir(&state.registry, cwd).await?;
    open(state, &slug).await
}

/// The slug of the registered project whose root holds `cwd`, the nearest one when roots nest.
pub async fn slug_for_dir(registry: &Store, cwd: &Path) -> Result<String, ProjectError> {
    let rows = projects::list_projects(registry.read()).await?;
    let root = find_root(cwd, rows.iter().map(|row| Path::new(&row.root)));
    root.and_then(|root| rows.iter().find(|row| Path::new(&row.root) == root))
        .map(|row| row.slug.clone())
        .ok_or_else(|| {
            FoldError::new(
                codes::PROJECT_NOT_FOUND,
                "no registered project holds this directory",
            )
            .with_hint("run the command inside a project directory, or fold init one")
            .into()
        })
}

/// [`slug_for_dir`] for a CLI command, which reads the host registry without a running daemon.
pub async fn slug_for_dir_on_host(host: &HostPaths, cwd: &Path) -> Result<String, ProjectError> {
    let registry = Store::open_registry(&host.registry_db()).await?;
    let slug = slug_for_dir(&registry, cwd).await;
    registry.close().await?;
    slug
}

/// The registry row of the human `name`, inserted on first use with `created_at` from `clock`. A known human costs one read and no write. Humans belong to the host, so this appends no project event.
pub async fn ensure_human(
    registry: &Store,
    clock: &dyn Clock,
    name: &HumanName,
) -> Result<HumanRow, StoreError> {
    if let Some(human) = projects::find_human(registry.read(), name).await? {
        return Ok(human);
    }
    let now = clock.now();
    registry
        .write(async |tx| projects::ensure_human(tx, name, now).await)
        .await
}

/// How long a sign-in leaves `web_tokens.last_used_at` alone, so checking a token on every request is not a write on every request.
pub const WEB_TOKEN_TOUCH_INTERVAL: Duration = Duration::from_secs(60);

/// Makes a web sign-in token for `human`, creating the human when absent, and returns its plaintext. This is the only time the plaintext exists: the registry keeps its BLAKE3 hash.
pub async fn create_web_token(
    registry: &Store,
    human: &HumanName,
    label: Option<&str>,
    now: Timestamp,
) -> Result<String, ProjectError> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|error| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            format!("the OS random source failed: {error}"),
        )
    })?;
    let token = URL_SAFE_NO_PAD.encode(secret);
    let hash = blake3::hash(token.as_bytes());
    registry
        .write(async |tx| {
            let row = projects::ensure_human(tx, human, now).await?;
            projects::insert_web_token(tx, hash.as_bytes(), row.id, label, now).await
        })
        .await?;
    Ok(token)
}

/// [`create_web_token`] for `fold token create`, which writes the host registry without a running daemon.
pub async fn create_web_token_on_host(
    host: &HostPaths,
    clock: &dyn Clock,
    human: &HumanName,
    label: Option<&str>,
) -> Result<String, ProjectError> {
    let registry = Store::open_registry(&host.registry_db()).await?;
    let token = create_web_token(&registry, human, label, clock.now()).await;
    registry.close().await?;
    token
}

/// The human that `token` signs in, or `None` for a token the registry does not hold. It touches `last_used_at` at most once per [`WEB_TOKEN_TOUCH_INTERVAL`].
pub async fn check_web_token(
    registry: &Store,
    clock: &dyn Clock,
    token: &str,
) -> Result<Option<HumanName>, StoreError> {
    let hash = blake3::hash(token.as_bytes());
    let Some(row) = projects::find_web_token(registry.read(), hash.as_bytes()).await? else {
        return Ok(None);
    };
    // The lookup found the row by its hash; this comparison is the constant-time one `blake3::Hash` provides.
    let stored = <[u8; 32]>::try_from(row.token_hash.as_slice())
        .map_err(|_| StoreError::Corrupt("web_tokens.token_hash"))?;
    if blake3::Hash::from_bytes(stored) != hash {
        return Ok(None);
    }
    let now = clock.now();
    let stale_before = now
        .checked_sub(WEB_TOKEN_TOUCH_INTERVAL)
        .unwrap_or(Timestamp::EPOCH);
    if row.last_used_at.is_none_or(|used| used <= stale_before) {
        registry
            .write(async |tx| projects::touch_web_token(tx, row.id, now, stale_before).await)
            .await?;
    }
    Ok(Some(row.human))
}

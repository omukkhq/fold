use std::collections::BTreeSet;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::projects::find_root;
use fold_core::tree::{FlatTree, Tree};
use fold_core::{
    AgentName, CONFIG_TEMPLATE, Clock, FoldError, HostPaths, HumanName, Presence, ProjectConfig,
    ProjectPaths, RevisionId, SessionFact, TaskStatus, Timestamp, choose_slug, codes, presence_of,
    slugify,
};
use fold_store::projects::{self, HumanRow, ProjectRow};
use fold_store::workspaces::{RevisionRow, create_initial_revision, head, initial_revision};
use fold_store::{Store, StoreError};
use fold_vcs::gc::mark;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_import::{GitImportOptions, archive_tree};
use fold_vcs::git_remote::{MIRROR_DIR, fetch_imported};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::import::{ImportOptions, ImportResult, import_dir};
use fold_vcs::main_view;
use fold_vcs::materialise::{MaterialiseOptions, fill_missing, materialise};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::WorkspaceError;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::{read_revision, write_initial_tree, write_revision};
use thiserror::Error;

use crate::daemon::DaemonState;
use crate::projects::{ProjectHandle, Slot};

mod commits;
mod gc;
mod pull;
mod remote;
mod web_auth;

pub(crate) use commits::error_code;
pub use commits::{commit_main, commit_message};
pub use gc::{GcReport, gc};
pub use pull::{PullOutcome, PulledConflict, pull};
pub(crate) use remote::push_with;
pub use remote::{PushOutcome, RemoteContext, push};
pub use web_auth::{
    DEFAULT_SESSION_LIFETIME, SignedIn, WEB_TOUCH_INTERVAL, check_web_session, create_web_session,
    create_web_token, create_web_token_on_host, end_web_session, list_web_tokens,
    list_web_tokens_on_host, revoke_web_token, revoke_web_token_on_host,
};

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
    /// True when this init finished one that had stopped before it registered the project. `imported` then holds what `revision` records, with no skipped paths: the stopped run's list is gone.
    pub resumed: bool,
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

/// Creates `.fold/` in `root`, migrates the project database, writes the commented config, records `r1` from `source` (or the empty tree) as `main`, fills `main/` read-only and only then registers the project under a free slug. Init is a host command, so it has no actor and appends no event. A failure after `.fold/` exists removes the directory this call made and puts `main/` back as it was: absent or empty, the only states init accepts. A `.fold/` that the registry does not know is an init that stopped part-way: when its database is complete the init finishes it, and otherwise it is refused with `project_incomplete` and nothing is removed. `source` is only read, so a project directory at or below it is refused.
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
    if !create_fold_dir(&paths).await? {
        return finish_stopped(registry, &paths, root_text, name, clock).await;
    }
    let _init = match lock_fold_dir(&paths).await {
        Ok(lock) => lock,
        Err(error) => {
            remove_created(&paths, None).await;
            return Err(error);
        }
    };
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
            // A fired failpoint stands for a crash at that point, and a crashed process removes nothing.
            #[cfg(feature = "failpoints")]
            if error.code() == Some(codes::FAILPOINT) {
                return Err(error);
            }
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
    #[cfg(feature = "failpoints")]
    fail::fail_point!("init_before_register", |_| Err(fired(
        "init_before_register"
    )));
    let row = register(registry, root_text, name, now).await?;
    Ok(InitializedProject {
        slug: row.slug,
        name: row.name,
        root: PathBuf::from(row.root),
        revision,
        imported,
        git_commit,
        resumed: false,
    })
}

/// Inserts the registry row under a free slug made from `name`.
async fn register(
    registry: &Store,
    root_text: &str,
    name: String,
    now: Timestamp,
) -> Result<ProjectRow, ProjectError> {
    let base = slugify(&name);
    registry
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
        .await
}

/// Finishes an init that stopped after it made `.fold/` and before it registered the project. A migrated database whose `main` names a revision with every object below it in the store is a project the stopped run completed, except perhaps for `main/` and the registry row: the files `main/` lacks are written and the project is registered. Anything less is refused with `project_incomplete`, and so is a `.fold/` that a running init holds. Nothing is removed in either case, and no database is created in a `.fold/` that has none.
async fn finish_stopped(
    registry: &Store,
    paths: &ProjectPaths,
    root_text: &str,
    name: String,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let Some(_init) = try_lock_fold_dir(paths).await? else {
        return Err(incomplete(paths).into());
    };
    let db = paths.db();
    let has_db = tokio::task::spawn_blocking(move || db.is_file()).await;
    if !has_db.unwrap_or(false) {
        return Err(incomplete(paths).into());
    }
    let store = Store::open_project(paths).await?;
    let row = head(store.read()).await;
    let closed = store.close().await;
    let row = row?;
    closed?;
    let Some(head) = row else {
        return Err(incomplete(paths).into());
    };
    check_kept_main(paths).await?;
    let fold_dir = paths.fold_dir();
    let main = paths.main_dir();
    let revision = head.number;
    let finished = blocking(move || {
        let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
        let Some(imported) = stored_contents(&objects, &head)? else {
            return Ok(None);
        };
        let caps = Capabilities::probe(&fold_dir).map_err(|error| io_error(&error))?;
        let opts = MaterialiseOptions {
            read_only: true,
            caps,
        };
        fill_missing(&objects, &head.tree_hash, &main, &opts).map_err(FoldError::from)?;
        main_view::write_marker(&fold_dir, head.number).map_err(|error| io_error(&error))?;
        let git_commit = SyncMap::load(&fold_dir)?.commit_of(head.number).cloned();
        Ok(Some((imported, git_commit)))
    })
    .await?;
    let Some((imported, git_commit)) = finished else {
        return Err(incomplete(paths).into());
    };
    let row = register(registry, root_text, name, clock.now()).await?;
    Ok(InitializedProject {
        slug: row.slug,
        name: row.name,
        root: PathBuf::from(row.root),
        revision,
        imported,
        git_commit,
        resumed: true,
    })
}

/// What `row` records, when its revision object holds the row's tree and every object it reaches is in the store; `None` when one is missing or the object names another tree.
fn stored_contents(
    objects: &ObjectStore,
    row: &RevisionRow,
) -> Result<Option<ImportResult>, ProjectError> {
    for hash in mark(objects, [row.hash])? {
        if !objects.contains(&hash).map_err(FoldError::from)? {
            return Ok(None);
        }
    }
    if read_revision(objects, &row.hash)?.tree != row.tree_hash {
        return Ok(None);
    }
    let load = |hash: &ObjectHash| -> Result<Tree, FoldError> {
        let object = objects.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    };
    let entries = fold_core::tree::flatten(&row.tree_hash, &load)?;
    let mut bytes = 0u64;
    for entry in entries.values() {
        bytes = bytes.saturating_add(objects.blob_size(&entry.hash).map_err(FoldError::from)?);
    }
    Ok(Some(ImportResult {
        tree: row.tree_hash,
        files: entries.len(),
        bytes,
        skipped: Vec::new(),
    }))
}

/// The error a fired failpoint returns, as a crash at that point would stop the init.
#[cfg(feature = "failpoints")]
fn fired(name: &str) -> ProjectError {
    FoldError::new(codes::FAILPOINT, format!("{name} fired")).into()
}

/// Objects first, then the sync map that names `r1`, then the one transaction that inserts `r1` and creates `main`, then `main/`. The map comes before the row, so a database that holds `r1` always has the map that a Git import needs.
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
                // The commits fold writes after `r1` name this commit as their parent.
                fetch_imported(&git, &fold_dir.join(MIRROR_DIR), &repo, &commit)?;
                (imported, Some(commit))
            }
            (Some(source), None) => {
                let opts = ImportOptions {
                    max_file_bytes,
                    caps,
                };
                let rules = IgnoreRules::new(&source);
                let imported = import_dir(&objects, &source, &rules, &opts)?;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("init_after_import", |_| Err(fired("init_after_import")));
                (imported, None)
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
    if let Some(commit) = git_commit.clone() {
        let fold_dir = paths.fold_dir();
        let synced = (stored.revision().number, commit);
        blocking(move || Ok(SyncMap::load(&fold_dir)?.append(&[synced])?)).await?;
    }
    let import_source = source.map(|source| source.text.as_str());
    let row = create_initial_revision(store, &stored, import_source).await?;
    let main = paths.main_dir();
    let tree = imported.tree;
    let fold_dir = paths.fold_dir();
    let number = row.number;
    blocking(move || {
        let opts = MaterialiseOptions {
            read_only: true,
            caps,
        };
        materialise(&objects, &tree, &main, &opts).map_err(FoldError::from)?;
        main_view::write_marker(&fold_dir, number).map_err(|error| io_error(&error))?;
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

/// Where a service takes the claim lease from: a fixed length, or the project's `.fold/config.toml` read on each use. Reading on use means an edit applies without a restart and an invalid file fails only the calls that need the lease, never a session or a whole project.
#[derive(Debug, Clone)]
pub enum Lease {
    Minutes(u64),
    Config(ProjectPaths),
}

impl Lease {
    /// The lease length now. An invalid file is its `invalid_config` refusal with a hint to fix it.
    pub async fn minutes(&self) -> Result<u64, ProjectError> {
        match self {
            Lease::Minutes(minutes) => Ok(*minutes),
            Lease::Config(paths) => match load_config(paths).await {
                Ok(config) => Ok(config.lease_minutes),
                Err(ProjectError::Fold(error)) => Err(ProjectError::Fold(
                    error.with_hint("ask a human to fix .fold/config.toml"),
                )),
                Err(error) => Err(error),
            },
        }
    }
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

/// False when `.fold/` already exists. `create_dir` fails then, so two inits of one directory cannot both create it.
async fn create_fold_dir(paths: &ProjectPaths) -> Result<bool, ProjectError> {
    let dir = paths.fold_dir();
    let made = tokio::task::spawn_blocking(move || std::fs::create_dir(dir)).await;
    match flatten(made) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(io_error(&error)),
    }
}

/// Every init holds this lock on `.fold/` until it returns, so a second init never finishes or registers a project that a running one is still writing, and a crashed one leaves no lock behind. A creating init waits for it: only a second init that found no database can hold it then.
async fn lock_fold_dir(paths: &ProjectPaths) -> Result<File, ProjectError> {
    let dir = paths.fold_dir();
    let locked = tokio::task::spawn_blocking(move || {
        let file = File::open(dir)?;
        file.lock()?;
        Ok(file)
    })
    .await;
    flatten(locked).map_err(|error| io_error(&error))
}

/// [`lock_fold_dir`] without waiting: `None` while another init holds the lock.
async fn try_lock_fold_dir(paths: &ProjectPaths) -> Result<Option<File>, ProjectError> {
    let dir = paths.fold_dir();
    let locked = tokio::task::spawn_blocking(move || {
        let file = File::open(dir)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    })
    .await;
    flatten(locked).map_err(|error| io_error(&error))
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

/// A stopped init may have filled `main/` in part, so any directory there is kept; a symlink or a file in its place is refused, so nothing is written through it.
async fn check_kept_main(paths: &ProjectPaths) -> Result<(), ProjectError> {
    let main = paths.main_dir();
    let found = tokio::task::spawn_blocking(move || match std::fs::symlink_metadata(&main) {
        Ok(meta) => Ok(meta.is_dir()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    })
    .await;
    match flatten(found) {
        Ok(true) => Ok(()),
        Ok(false) => {
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

fn incomplete(paths: &ProjectPaths) -> FoldError {
    FoldError::new(
        codes::PROJECT_INCOMPLETE,
        format!(
            "{} holds a fold project that is not registered and not complete",
            paths.root().display()
        ),
    )
    .with_hint(format!(
        "if no other fld init is running there, delete {} and run fld init again",
        paths.fold_dir().display()
    ))
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
    /// Read from the store of an open project; `None` for an unavailable one, and for an open one whose store could not be read.
    pub overview: Option<ProjectOverview>,
}

/// What the project list shows of an open project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOverview {
    /// Unarchived tasks per board column, in board order.
    pub counts: Vec<(TaskStatus, u32)>,
    /// Agents with a connected session, by name; presence needs no claim lease, so an invalid configuration cannot fail the read.
    pub agents: Vec<(AgentName, Presence)>,
    /// The revision `main` names; `None` before `r1` exists.
    pub head: Option<RevisionId>,
    /// When the newest stored event happened.
    pub last_change_at: Option<Timestamp>,
}

/// Reads the overview of an open project from its store.
pub async fn overview(handle: &ProjectHandle) -> Result<ProjectOverview, ProjectError> {
    let pool = handle.store.read();
    let stored = fold_store::tasks::status_counts(pool).await?;
    let counts = TaskStatus::ALL
        .iter()
        .map(|status| {
            let count = stored
                .iter()
                .find(|(stored, _)| stored == status)
                .map_or(0, |(_, count)| *count);
            (*status, count)
        })
        .collect();
    let now = handle.clock.now();
    let mut calls: Vec<(AgentName, Vec<SessionFact>)> = Vec::new();
    for (name, last_call_at) in fold_store::agents::open_session_calls_by_agent(pool).await? {
        let fact = SessionFact {
            ended: false,
            last_call_at,
        };
        match calls.iter_mut().find(|(known, _)| *known == name) {
            Some((_, facts)) => facts.push(fact),
            None => calls.push((name, vec![fact])),
        }
    }
    let mut agents: Vec<(AgentName, Presence)> = calls
        .into_iter()
        .map(|(name, facts)| (name, presence_of(now, &facts)))
        .filter(|(_, presence)| *presence != Presence::Offline)
        .collect();
    agents.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(ProjectOverview {
        counts,
        agents,
        head: fold_store::workspaces::head(pool)
            .await?
            .map(|revision| revision.number),
        last_change_at: fold_store::events::newest_at(pool).await?,
    })
}

/// Every registered project, ordered by slug, with the overview of each open one. A project whose overview cannot be read is listed without it, so one damaged store does not hide the others.
pub async fn list(state: &DaemonState) -> Result<Vec<ProjectEntry>, ProjectError> {
    let rows = projects::list_projects(state.registry.read()).await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let (available, overview) = match state.projects.slot(&row.slug) {
            Some(Slot::Open(handle)) => match self::overview(&handle).await {
                Ok(overview) => (true, Some(overview)),
                Err(error) => {
                    tracing::warn!(project = %row.slug, %error, "project overview could not be read");
                    (true, None)
                }
            },
            _ => (false, None),
        };
        entries.push(ProjectEntry {
            available,
            overview,
            slug: row.slug,
            name: row.name,
        });
    }
    Ok(entries)
}

/// Opens a project that `fld init` registered after the daemon started. An open project returns its handle again; a slug the registry does not hold is `project_not_found`.
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
    Ok(row_for_dir(registry, cwd).await?.slug)
}

async fn row_for_dir(registry: &Store, cwd: &Path) -> Result<projects::ProjectRow, ProjectError> {
    let rows = projects::list_projects(registry.read()).await?;
    let root = find_root(cwd, rows.iter().map(|row| Path::new(&row.root)));
    root.and_then(|root| rows.iter().find(|row| Path::new(&row.root) == root))
        .cloned()
        .ok_or_else(|| {
            FoldError::new(
                codes::PROJECT_NOT_FOUND,
                "no registered project holds this directory",
            )
            .with_hint("run the command inside a project directory, or fld init one")
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

/// The registered root of the project that holds `cwd`, read from the host registry without a running daemon.
pub async fn root_for_dir_on_host(host: &HostPaths, cwd: &Path) -> Result<PathBuf, ProjectError> {
    let registry = Store::open_registry(&host.registry_db()).await?;
    let row = row_for_dir(&registry, cwd).await;
    registry.close().await?;
    Ok(PathBuf::from(row?.root))
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

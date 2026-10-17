use std::collections::BTreeSet;
use std::fs::{File, TryLockError};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use fold_core::config::GIT_SECTION;
use fold_core::projects::find_root;
use fold_core::{
    AgentName, CONFIG_TEMPLATE, Clock, FoldError, GitConfig, HostPaths, HumanName, Presence,
    ProjectConfig, ProjectPaths, RevisionId, SessionFact, TaskStatus, Timestamp, choose_slug,
    codes, presence_of, slugify,
};
use fold_store::projects::{self, HumanRow, ProjectRow};
use fold_store::workspaces::{RevisionRow, create_initial_revision, head, initial_revision};
use fold_store::{Store, StoreError};
use fold_vcs::atomic::write_atomic;
use fold_vcs::gc::mark;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_import::{GitImportOptions, import_commit};
use fold_vcs::git_remote::{checked_out_branch, exclude_fold_dir, folder_clean};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::{FOLDIGNORE, GITIGNORE, IgnoreRules};
use fold_vcs::import::{ImportOptions, ImportResult, import_dir};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::{read_revision, write_revision};
use thiserror::Error;

use crate::daemon::DaemonState;
use crate::projects::{
    ProjectFolder, ProjectHandle, Slot, database_missing_hint, project_folder, removed, unopened,
};
#[cfg(feature = "failpoints")]
use crate::services::fired;

mod commits;
mod gc;
mod git_status;
mod pull;
mod remote;
mod web_auth;

pub(crate) use commits::error_code;
pub use commits::{Committed, commit_main, commit_message};
pub use gc::{GcReport, gc};
pub use git_status::{GitStatus, PushFailed, Pushed, RemoteStatus, git_status, public_location};
pub use pull::{PullOutcome, PulledConflict, pull};
pub(crate) use remote::push_with;
pub use remote::{PushOutcome, RemoteContext, push};
pub use web_auth::{
    DEFAULT_SESSION_LIFETIME, SignedIn, WEB_TOUCH_INTERVAL, check_web_session, create_web_session,
    create_web_token, create_web_token_on_host, end_web_session, list_humans, list_web_tokens,
    list_web_tokens_on_host, purge_expired_web_sessions, revoke_web_token,
    revoke_web_token_on_host,
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

    /// The code a log line names: the stable code, or `internal_error` for an internal failure.
    pub fn log_code(&self) -> &'static str {
        self.code().unwrap_or(codes::INTERNAL_ERROR)
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
    /// True when the registry held the folder for a project whose `.fold/` was deleted: this project took that registration's place and its slug.
    pub replaced: bool,
}

/// Files a caller writes into a new project's folder before init records it, so they are part of `r1` and its first commit. It returns the files and folders it created, which a failed init removes; a file it changed stays changed. Blocking.
pub type FolderFiles = Box<dyn FnOnce(&Path) -> Result<Vec<PathBuf>, FoldError> + Send>;

/// How init takes the project folder.
enum Start {
    /// The folder is not a Git repository: init runs `git init`, and the folder's files, less the ignore rules, with the `.gitignore` and `.foldignore` init writes or changes and the `files` it writes, become `r1` and its first commit.
    New { files: Option<FolderFiles> },
    /// The folder is a Git repository with a clean tree on `branch`: `r1` is the committed tree at `head`, whose commit is `head` itself.
    FromGit { head: CommitId, branch: String },
}

/// What a failed init made in the folder, so it removes exactly that and never a file of the folder.
#[derive(Debug, Default)]
struct Created {
    repository: bool,
    files: Vec<PathBuf>,
    /// Files of the folder that init changed, with the bytes they held before, which a failed init writes back.
    restore: Vec<(PathBuf, Vec<u8>)>,
}

/// Makes `root` a fold project whose folder is main: creates `.fold/`, migrates the project database, writes the commented config, records the folder's files, less the ignore rules, as `r1`, runs `git init` and writes `r1`'s commit, and only then registers the project under a free slug. A folder that is a Git repository is refused with a hint to use `--from-git`. A folder that is inside another repository's working tree is refused with a hint to use `--from-git` at that repository's top, so no repository is nested in another. Git ignores `.fold/` in the new repository, so init writes the ignore files before the folder's files are recorded and both are in `r1` and its commit, and the folder is clean after init. Without a `.gitignore`, init writes one as a regular file that copies `.foldignore` (a comment when the folder has no `.foldignore`) and ends with a `.fold/` line, and also writes an empty `.foldignore` when the folder has none. An existing `.gitignore` is kept and gets a `.fold/` line only when no line of it is `.fold`, `.fold/`, `/.fold` or `/.fold/`. The copy happens once: `.foldignore` only adds rules on top of `.gitignore` or brings back a path it ignores. Init is a host command, so it has no actor and appends no event. A failure after `.fold/` exists removes `.fold/` and what init created beside it and writes back the bytes of a `.gitignore` that init changed, never touching another file of the folder. A `.fold/` that the registry does not know is an init that stopped part-way, or a project that fold removed while its `.fold/` was away: when its database is complete the init finishes it and registers it, and otherwise it is refused with `project_incomplete` and nothing is removed. A registered folder that holds `.fold/` is refused with `project_exists`, or with `project_incomplete` when `.fold/` holds project content without a database, or a database without `main`; a registered folder without `.fold/`, or whose `.fold/` holds only what a daemon that still had the project open wrote after it was deleted, holds no project, so the new project replaces that registration in the transaction that registers it and keeps its slug.
pub async fn init_project(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    init_new_project(host, root, name, Git::default(), None, clock).await
}

/// What a plain init finds where the project folder is.
enum Found {
    Nothing,
    /// The folder is the top of a Git repository.
    Repository,
    /// The folder is below the top of the working tree at this path, whose repository would hold the new one.
    Inside(PathBuf),
}

/// The folder is below the top of another repository's working tree, where a repository of its own would be nested in that one.
fn inside_repository(root: &Path, top: &Path) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "{} is inside the Git repository at {}",
            root.display(),
            top.display()
        ),
    )
    .with_hint(format!(
        "run fld init --from-git in {} to make the whole repository the project, or move the folder out of it",
        top.display()
    ))
    .with_details([("reason", serde_json::json!("inside_git_repository"))])
}

/// [`init_project`] with the given `git` program, writing `files` into the folder with the ignore files.
pub async fn init_new_project(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    git: Git,
    files: Option<FolderFiles>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let root = prepare_root(root).await?;
    let (checked, git_checked) = (root.clone(), git.clone());
    // A folder with `.fold/` is an init that stopped or a project, which `create_and_register` tells apart; the stopped init may have made the repository already.
    let found = blocking(move || {
        let paths = ProjectPaths::new(&checked);
        if paths.fold_dir().exists() {
            return Ok(Found::Nothing);
        }
        // A `.git` that Git does not read as a repository is refused too, so init never writes into it or deletes it.
        if std::fs::symlink_metadata(paths.git_dir()).is_ok()
            || git_checked.repository(&checked)?.is_some()
        {
            return Ok(Found::Repository);
        }
        Ok(match git_checked.working_tree_top(&checked)? {
            Some(top) => Found::Inside(top),
            None => Found::Nothing,
        })
    })
    .await?;
    match found {
        Found::Nothing => {}
        Found::Repository => {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{} is a Git repository", root.display()),
            )
            .with_hint("run fld init --from-git there, so the repository keeps its history")
            .with_details([("reason", serde_json::json!("git_repository"))])
            .into());
        }
        Found::Inside(top) => return Err(inside_repository(&root, &top).into()),
    }
    init_with(host, &root, name, git, Start::New { files }, clock).await
}

/// Makes the Git repository whose top is `root` a fold project whose folder is main, as [`init_project`] does, keeping the repository and its history: `r1` is the committed tree at `HEAD`, free of ignore rules, and its commit is `HEAD` itself, which `.fold/git-sync.map` records as its first line. The folder must be the top of a repository whose `.git` is a directory, with a branch checked out that has a commit and no change in the index or a tracked file; anything else is refused with a hint. The `[git]` branch becomes the checked-out branch. Nothing in the repository changes but `info/exclude`, which gains `/.fold/`.
pub async fn init_project_from_git(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    git: Git,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let given = root.to_owned();
    let found = tokio::task::spawn_blocking(move || std::fs::canonicalize(given))
        .await
        .ok()
        .and_then(Result::ok);
    let not_repository = |root: &Path| {
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("{} is not the top of a Git repository", root.display()),
        )
        .with_hint("run fld init there to make a new project with a new repository")
        .with_details([("reason", serde_json::json!("not_git_repository"))])
    };
    let Some(root) = found else {
        return Err(not_repository(root).into());
    };
    let (checked, git_checked) = (root.clone(), git.clone());
    let start = blocking(move || match git_start(&git_checked, &checked)? {
        Some(start) => Ok(Ok(start)),
        None => Ok(Err(git_checked.working_tree_top(&checked)?)),
    })
    .await?;
    let start = match start {
        Ok(start) => start,
        Err(Some(top)) => return Err(inside_repository(&root, &top).into()),
        Err(None) => return Err(not_repository(&root).into()),
    };
    init_with(host, &root, name, git, start, clock).await
}

/// The start of a `--from-git` init in `root`, or `None` when `root` is not the top of a repository. Blocking.
fn git_start(git: &Git, root: &Path) -> Result<Option<Start>, ProjectError> {
    if git.repository(root)?.is_none() {
        return Ok(None);
    }
    let paths = ProjectPaths::new(root);
    let refused = |message: &str, hint: &str, reason: &str| -> ProjectError {
        FoldError::new(codes::INVALID_REQUEST, message.to_owned())
            .with_hint(hint.to_owned())
            .with_details([("reason", serde_json::json!(reason))])
            .into()
    };
    if !std::fs::symlink_metadata(paths.git_dir()).is_ok_and(|meta| meta.is_dir()) {
        return Err(refused(
            "the repository keeps its Git data elsewhere, as a linked worktree or a submodule does",
            "run fld init --from-git in a repository whose .git is a directory",
            "git_dir_elsewhere",
        ));
    }
    let Some(branch) = checked_out_branch(git, &paths.git_dir())? else {
        return Err(refused(
            "the repository has no branch checked out",
            "check out a branch, then run fld init --from-git again",
            "detached_head",
        ));
    };
    let Ok(head) = git.resolve_commit(root, "HEAD") else {
        return Err(refused(
            "the checked-out branch has no commit yet",
            "commit the files, then run fld init --from-git again",
            "no_commit",
        ));
    };
    if !folder_clean(git, &paths, &head)? {
        return Err(refused(
            "the repository has changes that are not committed",
            "commit or stash them, then run fld init --from-git again",
            "uncommitted_changes",
        ));
    }
    Ok(Some(Start::FromGit { head, branch }))
}

async fn init_with(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    git: Git,
    start: Start,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
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
    let outcome = create_and_register(&registry, root, &root_text, name, &git, start, clock).await;
    // The outcome is what the caller needs; a failed close only delays the WAL checkpoint.
    let _ = registry.close().await;
    outcome
}

async fn create_and_register(
    registry: &Store,
    root: &Path,
    root_text: &str,
    name: String,
    git: &Git,
    start: Start,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let registered = projects::find_by_root(registry.read(), root_text).await?;
    let paths = ProjectPaths::new(root);
    let mut made = create_fold_dir(&paths).await?;
    if !made && registered.is_some() && remove_leftovers(&paths).await? {
        made = create_fold_dir(&paths).await?;
    }
    if !made {
        if registered.is_some() {
            return Err(registered_refusal(&paths).await.into());
        }
        return finish_stopped(registry, &paths, root_text, name, git, clock).await;
    }
    let replaces = registered.map(|row| row.slug);
    let _init = match lock_fold_dir(&paths).await {
        Ok(lock) => lock,
        Err(error) => {
            remove_created(&paths, Created::default()).await;
            return Err(error);
        }
    };
    let mut created = Created::default();
    match populate_and_register(
        registry,
        &paths,
        root_text,
        name,
        replaces.as_deref(),
        git,
        start,
        &mut created,
        clock,
    )
    .await
    {
        Ok(project) => Ok(project),
        Err(error) => {
            // A fired failpoint stands for a crash at that point, and a crashed process removes nothing.
            #[cfg(feature = "failpoints")]
            if error.code() == Some(codes::FAILPOINT) {
                return Err(error);
            }
            remove_created(&paths, created).await;
            Err(error)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one input of the init, and grouping them would only rename them"
)]
async fn populate_and_register(
    registry: &Store,
    paths: &ProjectPaths,
    root_text: &str,
    name: String,
    replaces: Option<&str>,
    git: &Git,
    start: Start,
    created: &mut Created,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let now = clock.now();
    let project_store = Store::open_project(paths).await?;
    let first = async {
        let branch = match &start {
            Start::FromGit { branch, .. } => Some(branch.as_str()),
            Start::New { .. } => None,
        };
        write_config(paths, branch).await?;
        let config = load_config(paths).await?;
        let first =
            create_first_revision(&project_store, paths, git, start, &config, created, now).await?;
        // The daemon writes a commit that is missing when it opens the project, so a failure here only delays `r1`'s.
        if let Err(error) = commit_main(&project_store, paths, git, clock).await {
            tracing::warn!(code = error.code().unwrap_or(codes::INTERNAL_ERROR), "the Git commit of r1 was not written; the daemon writes it when it opens the project");
        }
        Ok::<_, ProjectError>(first)
    }
    .await;
    let closed = project_store.close().await;
    let (revision, imported, git_commit) = first?;
    closed?;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("init_before_register", |_| Err(fired(
        "init_before_register"
    )
    .into()));
    let (row, replaced) = register(registry, root_text, name, replaces, now).await?;
    Ok(InitializedProject {
        slug: row.slug,
        name: row.name,
        root: PathBuf::from(row.root),
        revision,
        imported,
        git_commit,
        resumed: false,
        replaced,
    })
}

/// Inserts the registry row under a free slug made from `name`. With `replaces`, the slug of the folder's registration whose `.fold/` was deleted, that row goes in the same transaction and the new row takes its slug, so the web addresses of the project stay; a row that changed since init read it stays, and the insert then refuses the folder with `project_exists`. The flag tells whether a row went.
async fn register(
    registry: &Store,
    root_text: &str,
    name: String,
    replaces: Option<&str>,
    now: Timestamp,
) -> Result<(ProjectRow, bool), ProjectError> {
    let base = slugify(&name);
    registry
        .write(async |tx| -> Result<(ProjectRow, bool), ProjectError> {
            let replaced = match replaces {
                Some(slug) => projects::delete_project(tx, slug, root_text).await?,
                None => false,
            };
            let existing: BTreeSet<String> = projects::slugs(tx).await?.into_iter().collect();
            let slug = match replaces {
                Some(slug) if replaced => slug.to_owned(),
                _ => choose_slug(&base, &existing),
            };
            let row = ProjectRow {
                slug,
                name,
                root: root_text.to_owned(),
            };
            match projects::insert_project(tx, &row, now).await {
                Ok(()) => Ok((row, replaced)),
                Err(error) if error.unique_violation() => Err(exists(Path::new(root_text)).into()),
                Err(error) => Err(error.into()),
            }
        })
        .await
}

/// Finishes an init that stopped after it made `.fold/` and before it registered the project. A migrated database whose `main` names a revision with every object below it in the store is a project the stopped run completed, except perhaps for the new repository, which is then created, and the registry row, which is then inserted; the daemon writes a missing commit when it opens the project. Anything less is refused with `project_incomplete`, and so is a `.fold/` that a running init holds. Nothing is removed in either case, and no database is created in a `.fold/` that has none.
async fn finish_stopped(
    registry: &Store,
    paths: &ProjectPaths,
    root_text: &str,
    name: String,
    git: &Git,
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
    let fold_dir = paths.fold_dir();
    let revision = head.number;
    let branch = load_config(paths).await?.git.branch;
    let (git, paths_owned) = (git.clone(), paths.clone());
    let finished = blocking(move || {
        let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
        let Some(imported) = stored_contents(&objects, &head)? else {
            return Ok(None);
        };
        if !paths_owned.git_dir().exists() {
            git.init_checkout(paths_owned.root(), &branch)?;
            exclude_fold_dir(&paths_owned.git_dir())?;
        }
        let git_commit = match head.import_source {
            Some(_) => SyncMap::load(&fold_dir)?.commit_of(head.number).cloned(),
            None => None,
        };
        Ok(Some((imported, git_commit)))
    })
    .await?;
    let Some((imported, git_commit)) = finished else {
        return Err(incomplete(paths).into());
    };
    let (row, _) = register(registry, root_text, name, None, clock.now()).await?;
    Ok(InitializedProject {
        slug: row.slug,
        name: row.name,
        root: PathBuf::from(row.root),
        revision,
        imported,
        git_commit,
        resumed: true,
        replaced: false,
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
    let entries = fold_vcs::compare::load_flat(objects, &row.tree_hash)?;
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

/// The comment of the `.gitignore` init writes when the folder has neither ignore file.
const GITIGNORE_COMMENT: &str = "# Paths that Git and fold leave out, one pattern per line. Add a .foldignore for rules that only fold uses.\n";

/// The line init adds to a `.gitignore` that does not ignore `.fold`, before its line ending.
const FOLD_DIR_LINE: &str = ".fold/";

/// A new project's ignore files (see [`plan_ignore_files`]) and the caller's `files` first, so `r1` holds them and the folder is clean against `r1`'s commit; then the objects of `r1`, the sync map that names `r1` for [`Start::FromGit`], and the one transaction that inserts `r1` and creates `main`. The map comes before the row, so a database that holds `r1` always has the map that names its commit. A new project's repository comes last, so an init that stops before its database is complete leaves no repository; one that stops after has it created when the init runs again. Both kinds get `/.fold/` in `info/exclude`.
async fn create_first_revision(
    store: &Store,
    paths: &ProjectPaths,
    git: &Git,
    start: Start,
    config: &ProjectConfig,
    created: &mut Created,
    now: Timestamp,
) -> Result<(RevisionId, ImportResult, Option<CommitId>), ProjectError> {
    let (max_file_bytes, branch) = (config.max_file_bytes, config.git.branch.clone());
    let head = match start {
        Start::FromGit { head, .. } => Some(head),
        Start::New { files } => {
            let root = paths.root().to_owned();
            let plan = blocking(move || plan_ignore_files(&root)).await?;
            // The old bytes are kept before the file changes, so a failure in between can still write them back.
            created.files = plan.create.iter().map(|(path, _)| path.clone()).collect();
            created.restore = plan
                .change
                .iter()
                .map(|(path, old, _)| (path.clone(), old.clone()))
                .collect();
            blocking(move || write_ignore_files(plan)).await?;
            if let Some(files) = files {
                let root = paths.root().to_owned();
                let written = blocking(move || Ok(files(&root)?)).await?;
                created.files.extend(written);
            }
            None
        }
    };
    let (git_owned, paths_owned, head_owned) = (git.clone(), paths.clone(), head.clone());
    let (imported, stored) = blocking(move || {
        let (git, paths) = (git_owned, paths_owned);
        let root = paths.root();
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
        let imported = match &head_owned {
            Some(head) => {
                exclude_fold_dir(&paths.git_dir())?;
                let opts = GitImportOptions { max_file_bytes };
                import_commit(&objects, &git, root, head.as_str(), &opts)?
            }
            None => {
                let caps = Capabilities::probe(&fold_dir).map_err(|error| io_error(&error))?;
                let opts = ImportOptions {
                    max_file_bytes,
                    caps,
                };
                let imported = import_dir(&objects, root, &IgnoreRules::new(root), &opts)?;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("init_after_import", |_| Err(
                    fired("init_after_import").into()
                ));
                imported
            }
        };
        let revision = initial_revision(imported.tree, now)?;
        let stored = write_revision(&objects, &revision).map_err(FoldError::from)?;
        Ok((imported, stored))
    })
    .await?;
    if let Some(commit) = head.clone() {
        let fold_dir = paths.fold_dir();
        let synced = (stored.revision().number, commit);
        blocking(move || Ok(SyncMap::load(&fold_dir)?.append(&[synced])?)).await?;
    }
    // A Git import names the repository it read, so a stopped init that is finished later still reports `r1`'s commit; a new project's `r1` has none.
    let import_source = match head {
        Some(_) => paths.root().to_str(),
        None => None,
    };
    let row = create_initial_revision(store, &stored, import_source).await?;
    if head.is_none() {
        let (git, paths) = (git.clone(), paths.clone());
        created.repository = true;
        blocking(move || {
            git.init_checkout(paths.root(), &branch)?;
            exclude_fold_dir(&paths.git_dir())?;
            Ok(())
        })
        .await?;
    }
    Ok((row.number, imported, head))
}

/// What init does to the ignore files of a new project's folder.
struct IgnorePlan {
    /// Files that do not exist yet, with their content.
    create: Vec<(PathBuf, Vec<u8>)>,
    /// A `.gitignore` that exists, with its bytes now and the bytes it gets.
    change: Option<(PathBuf, Vec<u8>, Vec<u8>)>,
}

/// True when a line of `text` is exactly `.fold`, `.fold/`, `/.fold` or `/.fold/` once trailing spaces are trimmed, as Git trims them; leading spaces stay part of a Git pattern, and a comment or a negation is not one of the four.
fn ignores_fold_dir(text: &str) -> bool {
    text.lines().any(|line| {
        matches!(
            line.trim_end_matches(' '),
            ".fold" | ".fold/" | "/.fold" | "/.fold/"
        )
    })
}

/// `text` with the `.fold/` line added unless a line of it already ignores `.fold`. The line ends the way the lines of `text` end, CRLF when `text` has one, and a line ending comes first when `text` does not end with one.
fn with_fold_dir_line(mut text: Vec<u8>) -> Vec<u8> {
    if ignores_fold_dir(&String::from_utf8_lossy(&text)) {
        return text;
    }
    let eol: &[u8] = if text.windows(2).any(|pair| pair == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    if !text.is_empty() && !text.ends_with(b"\n") {
        text.extend_from_slice(eol);
    }
    text.extend_from_slice(FOLD_DIR_LINE.as_bytes());
    text.extend_from_slice(eol);
    text
}

/// The bytes of a regular file, or `None` when the path is missing; a symlink or another kind of entry is not followed and reads as `None` too, and `exists` tells it apart. Blocking.
fn read_regular(path: &Path) -> Result<(bool, Option<Vec<u8>>), ProjectError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {
            Ok((true, Some(std::fs::read(path).map_err(|e| io_error(&e))?)))
        }
        Ok(_) => Ok((true, None)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((false, None)),
        Err(error) => Err(io_error(&error)),
    }
}

/// Decides the ignore files of a new project's folder, because Git starts with them. A folder without a `.gitignore` gets one that copies `.foldignore` (or a comment when there is none) and ends with a `.fold/` line, and an empty `.foldignore` when it has none. An existing `.gitignore` gets the `.fold/` line when it lacks one, and no `.foldignore` is written. A `.gitignore` that is not a regular file is left alone. Blocking.
fn plan_ignore_files(root: &Path) -> Result<IgnorePlan, ProjectError> {
    let gitignore = root.join(GITIGNORE);
    let foldignore = root.join(FOLDIGNORE);
    let (git_exists, git_text) = read_regular(&gitignore)?;
    let (fold_exists, fold_text) = read_regular(&foldignore)?;
    let mut plan = IgnorePlan {
        create: Vec::new(),
        change: None,
    };
    if git_exists {
        if let Some(old) = git_text {
            let new = with_fold_dir_line(old.clone());
            if new != old {
                plan.change = Some((gitignore, old, new));
            }
        }
        return Ok(plan);
    }
    let text = match fold_text {
        Some(text) => with_fold_dir_line(text),
        None if fold_exists => with_fold_dir_line(Vec::new()),
        None => with_fold_dir_line(GITIGNORE_COMMENT.as_bytes().to_vec()),
    };
    plan.create.push((gitignore, text));
    if !fold_exists {
        plan.create.push((foldignore, Vec::new()));
    }
    Ok(plan)
}

/// Writes what [`plan_ignore_files`] decided, each file through [`write_atomic`]. Blocking.
fn write_ignore_files(plan: IgnorePlan) -> Result<(), ProjectError> {
    for (path, text) in plan.create {
        write_atomic(&path, &text).map_err(|error| io_error(&error))?;
    }
    if let Some((path, _, new)) = plan.change {
        write_atomic(&path, &new).map_err(|error| io_error(&error))?;
    }
    Ok(())
}

/// [`crate::support::blocking`] for work that fails with a [`ProjectError`]: a panic on the blocking pool is an `internal_error`, never a layout failure.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ProjectError> + Send + 'static,
) -> Result<T, ProjectError> {
    crate::support::blocking(move || Ok(work())).await?
}

/// Where a service takes the claim lease from: a fixed length, or the project's `.fold/config.toml` read on each use, except for the footer count of [`Lease::recent_minutes`]. Reading on use means an edit applies without a restart and an invalid file fails only the calls that need the lease, never a session or a whole project.
#[derive(Debug, Clone)]
pub enum Lease {
    Minutes(u64),
    Config(ConfigLease),
}

/// The configuration file a lease is read from, with the last valid lease parsed from it for [`Lease::recent_minutes`]. Clones share that parse.
#[derive(Debug, Clone)]
pub struct ConfigLease {
    paths: ProjectPaths,
    parsed: Arc<std::sync::Mutex<Option<(FileStamp, u64)>>>,
}

/// What a stat shows of the configuration file. A rewrite changes its size or its modification time, and an editor that replaces the file changes its inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    inode: u64,
}

impl Lease {
    /// The lease of the project at `paths`, read from its configuration file.
    pub fn config(paths: ProjectPaths) -> Self {
        Self::Config(ConfigLease {
            paths,
            parsed: Arc::default(),
        })
    }

    /// The lease length now. An invalid file is its `invalid_config` refusal with a hint to fix it.
    pub async fn minutes(&self) -> Result<u64, ProjectError> {
        match self {
            Lease::Minutes(minutes) => Ok(*minutes),
            Lease::Config(config) => with_fix_hint(config.read().await),
        }
    }

    /// [`Lease::minutes`] for a count that every tool call shows: while a stat finds the file as it was at the last valid parse, that parse answers and the file is not read again. A rewrite that keeps the size, the modification time and the inode keeps the old lease here until the file changes again; commands read the file through [`Lease::minutes`].
    pub async fn recent_minutes(&self) -> Result<u64, ProjectError> {
        match self {
            Lease::Minutes(minutes) => Ok(*minutes),
            Lease::Config(config) => with_fix_hint(config.recent().await),
        }
    }

    /// [`Lease::minutes`] for deciding whether a claim is stale: while the project's agents are paused no claim goes stale and none can be taken over, so the lease has no end.
    pub async fn staleness_minutes(&self, store: &Store) -> Result<u64, ProjectError> {
        let minutes = self.minutes().await?;
        endless_while_paused(store, minutes).await
    }

    /// [`Lease::recent_minutes`] with the rule of [`Lease::staleness_minutes`].
    pub async fn recent_staleness_minutes(&self, store: &Store) -> Result<u64, ProjectError> {
        let minutes = self.recent_minutes().await?;
        endless_while_paused(store, minutes).await
    }
}

async fn endless_while_paused(store: &Store, minutes: u64) -> Result<u64, ProjectError> {
    let paused = fold_store::agents::pause_state(store.read())
        .await?
        .is_paused();
    Ok(if paused { u64::MAX } else { minutes })
}

impl ConfigLease {
    pub fn paths(&self) -> &ProjectPaths {
        &self.paths
    }

    async fn read(&self) -> Result<u64, ProjectError> {
        Ok(load_config(&self.paths).await?.lease_minutes)
    }

    async fn recent(&self) -> Result<u64, ProjectError> {
        let path = self.paths.config();
        let stamp = tokio::task::spawn_blocking(move || std::fs::metadata(path))
            .await
            .ok()
            .and_then(Result::ok)
            .map(|meta| FileStamp {
                len: meta.len(),
                modified: meta.modified().ok(),
                inode: meta.ino(),
            });
        // A missing or unreadable file is read every time: `load_config` decides its defaults or its error.
        let Some(stamp) = stamp else {
            return self.read().await;
        };
        if let Some(minutes) = self.parsed_for(stamp) {
            return Ok(minutes);
        }
        let minutes = self.read().await?;
        if let Ok(mut parsed) = self.parsed.lock() {
            *parsed = Some((stamp, minutes));
        }
        Ok(minutes)
    }

    fn parsed_for(&self, stamp: FileStamp) -> Option<u64> {
        let parsed = self.parsed.lock().ok()?;
        parsed
            .filter(|(seen, _)| *seen == stamp)
            .map(|(_, minutes)| minutes)
    }
}

fn with_fix_hint(lease: Result<u64, ProjectError>) -> Result<u64, ProjectError> {
    match lease {
        Err(ProjectError::Fold(error)) => Err(ProjectError::Fold(
            error.with_hint("ask a human to fix .fold/config.toml"),
        )),
        other => other,
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

/// The commented template, with a `[git]` section naming `branch` when it is not the default, as for a repository checked out on another branch.
async fn write_config(paths: &ProjectPaths, branch: Option<&str>) -> Result<(), ProjectError> {
    let path = paths.config();
    let text = match branch {
        Some(branch) if branch != GitConfig::default().branch => {
            // A JSON string is a valid TOML basic string.
            let quoted = serde_json::to_string(branch)
                .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
            format!("{CONFIG_TEMPLATE}\n[{GIT_SECTION}]\nbranch = {quoted}\n")
        }
        _ => CONFIG_TEMPLATE.to_owned(),
    };
    let wrote = tokio::task::spawn_blocking(move || std::fs::write(path, text)).await;
    flatten(wrote).map_err(|error| io_error(&error))
}

/// Removes `.fold/` and what this failed init wrote beside it: the repository it created and the files and folders it created, and writes back a `.gitignore` it changed. The folder's other files are never touched.
async fn remove_created(paths: &ProjectPaths, created: Created) {
    let fold = paths.fold_dir();
    let repository = created.repository.then(|| paths.git_dir());
    // The original error is what the caller needs; a `.fold/` that stays behind makes the next init refuse with `project_exists`.
    let _ = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(fold);
        if let Some(repository) = repository {
            let _ = std::fs::remove_dir_all(repository);
        }
        for (path, old) in created.restore {
            let _ = write_atomic(&path, &old);
        }
        for path in created.files {
            let _ = match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
                _ => std::fs::remove_file(path),
            };
        }
    })
    .await;
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

/// Removes the `.fold/` of a registered folder when it has no database and holds only what a daemon that still had the project open writes after `.fold/` was deleted, so the folder holds no project and the init goes on. Any other content keeps `.fold/` as it is, and so does a running init, which holds the lock.
async fn remove_leftovers(paths: &ProjectPaths) -> Result<bool, ProjectError> {
    let Some(_init) = try_lock_fold_dir(paths).await? else {
        return Ok(false);
    };
    let paths = paths.clone();
    let removed = tokio::task::spawn_blocking(move || {
        if std::fs::symlink_metadata(paths.db()).is_ok() || !only_leftovers(&paths)? {
            return Ok(false);
        }
        std::fs::remove_dir_all(paths.fold_dir())?;
        Ok(true)
    })
    .await;
    flatten(removed).map_err(|error| io_error(&error))
}

/// True when every file below `.fold/` is one that a project handle on a deleted database can write: a stat index below `index/`, a staged object below `objects/tmp/`, a capability probe, or an empty WAL or shared-memory file. Directories may be anywhere; a symbolic link or a published object is project content.
fn only_leftovers(paths: &ProjectPaths) -> io::Result<bool> {
    let fold = paths.fold_dir();
    let index = paths.index();
    let staged = paths.objects().join("tmp");
    let empty_journals = ["-wal", "-shm"].map(|suffix| {
        let mut name = paths.db().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    });
    let mut pending = vec![fold.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                pending.push(path);
                continue;
            }
            let probe = path.strip_prefix(&fold).ok().and_then(|relative| {
                relative
                    .components()
                    .next()
                    .and_then(|first| first.as_os_str().to_str())
            });
            let leftover = meta.is_file()
                && (path.starts_with(&index)
                    || path.starts_with(&staged)
                    || probe.is_some_and(|first| first.starts_with("probe-"))
                    || (empty_journals.contains(&path) && meta.len() == 0));
            if !leftover {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// The refusal of an init in a registered folder that holds `.fold/`: `project_exists` while the project database is there with its `main`, and `project_incomplete` when `.fold/` has no database but holds more than [`remove_leftovers`] removes, or a database without `main`, as an init that replaced the registration and stopped before its first revision leaves, or that is running there.
async fn registered_refusal(paths: &ProjectPaths) -> FoldError {
    let refusal = database_refusal(paths).await;
    if refusal.code() == codes::PROJECT_EXISTS && !holds_main(paths).await {
        return incomplete(paths);
    }
    refusal
}

/// True when `.fold/` is free of a running init and its database names `main`. A folder or database that cannot be read counts as holding it, so the refusal stays `project_exists`.
async fn holds_main(paths: &ProjectPaths) -> bool {
    let _init = match try_lock_fold_dir(paths).await {
        Ok(Some(lock)) => lock,
        Ok(None) => return false,
        Err(_) => return true,
    };
    let Ok(store) = Store::open_project(paths).await else {
        return true;
    };
    let main = head(store.read()).await;
    let _ = store.close().await;
    !matches!(main, Ok(None))
}

async fn database_refusal(paths: &ProjectPaths) -> FoldError {
    let db = paths.db();
    let missing = tokio::task::spawn_blocking(move || std::fs::symlink_metadata(db).is_err())
        .await
        .unwrap_or(false);
    if missing {
        FoldError::new(
            codes::PROJECT_INCOMPLETE,
            format!(
                "{} is registered, but its project database {} is missing",
                paths.root().display(),
                paths.db().display()
            ),
        )
        .with_hint(database_missing_hint(paths))
    } else {
        exists(paths.root())
    }
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
    /// What needs a person in the project as a whole, beside the attention items of its tasks.
    pub attention: Vec<ProjectAttention>,
    /// When the project's agents were paused, while they are.
    pub paused_since: Option<Timestamp>,
    /// The event actor label of whoever paused them, while they are paused.
    pub paused_by: Option<String>,
}

/// Why the project, rather than one of its tasks, needs a person. The daemon decides every kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAttention {
    /// The `[git]` branch has commits fold did not write, so the project folder stops following main until a human runs `fld pull`, which brings them in.
    OutsideCommits,
    /// The project folder has changes that are not committed, so it stops following main until the human commits or stashes them.
    UncommittedChanges,
}

impl ProjectAttention {
    /// The name the HTTP view and the `git.attention_cleared` event give the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutsideCommits => "outside_commits",
            Self::UncommittedChanges => "uncommitted_changes",
        }
    }
}

/// What the commit writer's last run found that needs a person. It is derived from the repository, never stored: the writer's first run after the project opens finds it again.
#[derive(Debug, Default)]
pub struct GitAttention {
    outside: AtomicBool,
    uncommitted: AtomicBool,
}

/// How a run of the commit writer changed the project's Git attention.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AttentionChange {
    pub(crate) raised: Vec<ProjectAttention>,
    pub(crate) cleared: Vec<ProjectAttention>,
}

impl GitAttention {
    pub fn kinds(&self) -> Vec<ProjectAttention> {
        let mut kinds = Vec::new();
        if self.outside.load(Ordering::Relaxed) {
            kinds.push(ProjectAttention::OutsideCommits);
        }
        if self.uncommitted.load(Ordering::Relaxed) {
            kinds.push(ProjectAttention::UncommittedChanges);
        }
        kinds
    }

    /// Records what a run found and returns what changed since the run before; the first run after the project opens compares with nothing found.
    pub(crate) fn set(&self, outside: bool, uncommitted: bool) -> AttentionChange {
        let mut change = AttentionChange::default();
        for (flag, now, kind) in [
            (&self.outside, outside, ProjectAttention::OutsideCommits),
            (
                &self.uncommitted,
                uncommitted,
                ProjectAttention::UncommittedChanges,
            ),
        ] {
            match (flag.swap(now, Ordering::Relaxed), now) {
                (false, true) => change.raised.push(kind),
                (true, false) => change.cleared.push(kind),
                _ => {}
            }
        }
        change
    }
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
    let pause = fold_store::agents::pause_state(pool).await?;
    Ok(ProjectOverview {
        counts,
        agents,
        head: fold_store::workspaces::head(pool)
            .await?
            .map(|revision| revision.number),
        last_change_at: fold_store::events::newest_at(pool).await?,
        attention: handle.git_attention.kinds(),
        paused_since: pause.paused_at,
        paused_by: pause.paused_by,
    })
}

/// Every registered project, ordered by slug, with the overview of each open one. A project whose overview cannot be read is listed without it, so one damaged store does not hide the others. A project whose folder is there without `.fold/` is removed from fold first and is not listed.
pub async fn list(state: &DaemonState) -> Result<Vec<ProjectEntry>, ProjectError> {
    let rows = projects::list_projects(state.registry.read()).await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        if project_folder(Path::new(&row.root)).await == ProjectFolder::FoldMissing
            && state.projects.remove_if_deleted(&row).await
        {
            continue;
        }
        let (available, overview) = match state.projects.slot(&row.slug) {
            Some(Slot::Open(handle)) => match self::overview(&handle).await {
                Ok(overview) => (true, Some(overview)),
                Err(error) => {
                    tracing::warn!(project = %row.slug, code = error.log_code(), "project overview could not be read");
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

/// The open handle of `slug` for a request. A handle whose database was replaced or is missing, or whose folder is missing, is closed and the project is unavailable, with what to do for its folder; a project whose folder is there without `.fold/`, open or not, is removed from fold and is `project_not_found`, like a slug the registry does not hold.
pub async fn handle(state: &DaemonState, slug: &str) -> Result<Arc<ProjectHandle>, ProjectError> {
    let root = match state.projects.slot(slug) {
        Some(Slot::Open(handle)) if !handle.database_replaced().await => return Ok(handle),
        Some(Slot::Open(handle)) => {
            state.projects.close_if_stale(slug).await;
            Some(handle.paths.root().to_owned())
        }
        Some(Slot::Unavailable) => match projects::find_project(state.registry.read(), slug).await?
        {
            Some(row) => {
                state.projects.remove_if_deleted(&row).await;
                Some(PathBuf::from(row.root))
            }
            None => None,
        },
        None => None,
    };
    match (state.projects.slot(slug), root) {
        (Some(_), Some(root)) => Err(unopened(slug, &root).await.into()),
        (Some(_), None) => Err(FoldError::new(
            codes::PROJECT_UNAVAILABLE,
            format!("project {slug} is unavailable"),
        )
        .into()),
        (None, root) => match state.projects.take_removed(slug).or(root) {
            Some(root) => Err(removed(slug, &root).into()),
            None => Err(FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("project {slug} is not registered"),
            )
            .into()),
        },
    }
}

/// Removes from fold every registered project whose folder is there without `.fold/`, closes every open project whose folder or database went away or whose database was replaced, and opens again every project whose missing folder came back. The daemon's live job runs it on every tick, so each change takes effect within a tick even when nothing opens the project. Returns how many projects it removed, closed or opened.
pub async fn sweep_deleted(state: &DaemonState) -> Result<usize, ProjectError> {
    let mut changed = state.projects.close_stale().await;
    for row in projects::list_projects(state.registry.read()).await? {
        let changes = match project_folder(Path::new(&row.root)).await {
            ProjectFolder::FoldMissing => state.projects.remove_if_deleted(&row).await,
            ProjectFolder::Present if state.projects.is_away(&row.slug) => {
                open_returned(state, &row).await
            }
            _ => false,
        };
        changed += usize::from(changes);
    }
    Ok(changed)
}

/// Opens a project whose missing folder is back, unless an init holds its `.fold/`, which leaves the open to the next tick. True when it opened.
async fn open_returned(state: &DaemonState, row: &projects::ProjectRow) -> bool {
    let paths = ProjectPaths::new(&row.root);
    let Ok(Some(_init)) = try_lock_fold_dir(&paths).await else {
        return false;
    };
    match state.projects.open(row, state.clock.clone()).await {
        Ok(_) => {
            tracing::info!(project = %row.slug, root = %row.root, "the project folder is back; the project is open again");
            true
        }
        Err(error) => {
            tracing::warn!(project = %row.slug, code = error.code(), "the project folder is back, but the project did not open");
            false
        }
    }
}

/// Removes from the host registry every project whose folder is there without `.fold/`, for `fld doctor` when no daemon runs, and returns their slugs in slug order. A missing folder or a `.fold/` without its database keeps its row. A missing registry holds no project.
pub async fn remove_deleted_on_host(host: &HostPaths) -> Result<Vec<String>, ProjectError> {
    if !tokio::fs::try_exists(host.registry_db())
        .await
        .unwrap_or(false)
    {
        return Ok(Vec::new());
    }
    let registry = Store::open_registry(&host.registry_db()).await?;
    let removed = remove_deleted_rows(&registry).await;
    registry.close().await?;
    removed
}

async fn remove_deleted_rows(registry: &Store) -> Result<Vec<String>, ProjectError> {
    let mut removed = Vec::new();
    for row in projects::list_projects(registry.read()).await? {
        let gone =
            async || project_folder(Path::new(&row.root)).await == ProjectFolder::FoldMissing;
        if gone().await
            && registry
                .write(async |tx| -> Result<bool, StoreError> {
                    // Looked at again under the registry's write lock, which an init's registration also takes, so a project that an init registered meanwhile stays.
                    Ok(gone().await && projects::delete_project(tx, &row.slug, &row.root).await?)
                })
                .await?
        {
            tracing::warn!(project = %row.slug, root = %row.root, "the project folder has no .fold; fold removed the project");
            removed.push(row.slug);
        }
    }
    Ok(removed)
}

/// Opens a project that `fld init` registered after the daemon started. An open project returns its handle again; a slug the registry does not hold is `project_not_found`.
pub async fn open(state: &DaemonState, slug: &str) -> Result<Arc<ProjectHandle>, ProjectError> {
    let row = projects::find_project(state.registry.read(), slug)
        .await?
        .ok_or_else(|| match state.projects.take_removed(slug) {
            Some(root) => removed(slug, &root),
            None => FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("project {slug} is not registered"),
            ),
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

/// The slug the host registry holds for the folder `root` when the folder has no `.fold/fold.db`, as after its fold data was deleted; an init there replaces that registration when `.fold/` is gone too. `None` when the folder or the registry does not exist, the folder is not registered or it holds the database. It reads the registry without a running daemon, so `fld init` can have the daemon let go of the old project before the new one is created.
pub async fn deleted_project_slug(
    host: &HostPaths,
    root: &Path,
) -> Result<Option<String>, ProjectError> {
    let (given, registry_db) = (root.to_owned(), host.registry_db());
    let found = blocking(move || {
        let Ok(root) = std::fs::canonicalize(given) else {
            return Ok(None);
        };
        let without_data = std::fs::symlink_metadata(ProjectPaths::new(&root).db()).is_err();
        Ok((without_data && registry_db.is_file()).then_some(root))
    })
    .await?;
    let Some(root_text) = found.as_deref().and_then(Path::to_str) else {
        return Ok(None);
    };
    let registry = Store::open_registry(&host.registry_db()).await?;
    let row = projects::find_by_root(registry.read(), root_text).await;
    registry.close().await?;
    Ok(row?.map(|row| row.slug))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panic_on_the_blocking_pool_is_an_internal_error() {
        let stopped = blocking(|| -> Result<(), ProjectError> { panic!("the work panics") }).await;
        assert_eq!(
            stopped.unwrap_err().code(),
            Some(codes::INTERNAL_ERROR),
            "never workspace_io"
        );
        assert_eq!(blocking(|| Ok(3)).await.unwrap(), 3);
    }

    #[test]
    fn each_change_of_the_git_attention_is_reported_once() {
        let attention = GitAttention::default();
        let change = |raised: &[ProjectAttention], cleared: &[ProjectAttention]| AttentionChange {
            raised: raised.to_vec(),
            cleared: cleared.to_vec(),
        };
        use ProjectAttention::{OutsideCommits, UncommittedChanges};
        assert_eq!(attention.set(false, false), change(&[], &[]));
        assert_eq!(
            attention.set(true, true),
            change(&[OutsideCommits, UncommittedChanges], &[])
        );
        assert_eq!(attention.kinds(), [OutsideCommits, UncommittedChanges]);
        assert_eq!(attention.set(true, true), change(&[], &[]));
        assert_eq!(
            attention.set(true, false),
            change(&[], &[UncommittedChanges])
        );
        assert_eq!(attention.set(false, false), change(&[], &[OutsideCommits]));
        assert!(attention.kinds().is_empty());
        assert_eq!(
            [OutsideCommits.as_str(), UncommittedChanges.as_str()],
            ["outside_commits", "uncommitted_changes"]
        );
    }
}

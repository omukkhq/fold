use std::collections::BTreeSet;
use std::fs::{File, TryLockError};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use fold_core::config::GIT_SECTION;
use fold_core::events::types;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::projects::find_root;
use fold_core::revision::{Revision, StoredRevision};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentName, CONFIG_TEMPLATE, Clock, EventActor, FoldError, GitConfig, HostPaths,
    HumanName, IdKind, IntegrationId, NewEvent, Presence, ProjectConfig, ProjectPaths, RevisionId,
    SessionFact, TaskStatus, Timestamp, choose_slug, codes, policy, presence_of, slugify,
};
use fold_store::counters;
use fold_store::projects::{self, HumanRow, ProjectRow};
use fold_store::reviews::{IntegrationSubject, NewIntegration, insert_succeeded};
use fold_store::workspaces::{
    Checkout, NewHumanChange, RevisionRow, advance_main, create_main, genesis_revision, head,
    insert_recorded_human_change, insert_revision, newest_human_change, set_checkout,
};
use fold_store::{Store, StoreError, WriteTx};
use fold_vcs::atomic::write_atomic;
use fold_vcs::gc::mark;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_discovery::Enclosing;
use fold_vcs::git_import::{GitImportOptions, import_commit};
use fold_vcs::git_remote::{
    FOLD_BRANCH_REF, FOLD_STASH_REF, STASH_REF, checked_out_branch, exclude_claude_files,
    exclude_fold_dir, folder_clean, set_ref,
};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::{FOLDIGNORE, GITIGNORE, IgnoreRules};
use fold_vcs::import::{ImportOptions, ImportResult, import_dir};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::{read_revision, write_initial_tree, write_revision};
use thiserror::Error;

use crate::daemon::DaemonState;
use crate::projects::{
    ProjectFolder, ProjectHandle, Slot, database_missing_hint, project_folder, removed,
    removed_by_person, unopened,
};
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

mod commits;
mod enable_git;
mod folder;
mod folder_status;
mod gc;
mod git_status;
pub mod notify;
mod pull;
mod record;
mod remote;
mod stash;
mod untrack;
mod web_auth;

pub(crate) use commits::error_code;
pub use commits::{Committed, commit_main, commit_message};
pub use enable_git::{GitEnabled, GitIdentity, enable_git};
pub use folder::{FolderFollowed, FolderSync, sync_folder};
pub(crate) use folder::{append_alone, note_attention};
pub use folder_status::{
    FolderChange, FolderSkipped, FolderStashEntry, FolderStatus, StashEntryState, folder_status,
};
pub use gc::{GcReport, gc};
pub use git_status::{GitStatus, PushFailed, Pushed, RemoteStatus, git_status, public_location};
pub use pull::{PullOutcome, PulledConflict, pull};
pub use record::{RecordOutcome, TouchedTask, record};
pub(crate) use remote::push_with;
pub use remote::{PushOutcome, RemoteContext, push};
pub use stash::{
    AfterUnstash, StashEntry, StashOutcome, UnstashConflict, UnstashOutcome, stash, stash_list,
    unstash,
};
pub use untrack::{FOLD_SKILLS_TITLE, FoldSkills, UntrackOutcome, keep_fold_skills_out, untrack};
pub use web_auth::{
    DEFAULT_SESSION_LIFETIME, SIGN_IN_LINK_LIFETIME, SignInLink, SignedIn, WEB_TOUCH_INTERVAL,
    check_web_session, create_sign_in_link, create_web_session, create_web_token, end_web_session,
    list_humans, list_web_tokens, list_web_tokens_on_host, open_sign_in_link,
    purge_expired_web_sessions, revoke_web_token, revoke_web_token_on_host,
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
    /// The revision that holds the folder's files: `r1`, the human's first change on top of the empty genesis `r0`, or `r0` itself when it is a Git repository's `HEAD` or the folder held no file.
    pub revision: RevisionId,
    /// What `revision` holds.
    pub imported: ImportResult,
    /// The commit a Git import read, which `.fold/git-sync.map` records as the genesis `r0`.
    pub git_commit: Option<CommitId>,
    /// The revision that records the `.gitignore` init wrote or changed when it turned Git on.
    pub gitignore: Option<RevisionId>,
    /// True when Git sync is on: the project folder has its own Git repository, which holds a commit of every revision.
    pub git_sync: bool,
    /// True when this init finished one that had stopped before it registered the project. `imported` then holds what `revision` records, with no skipped paths: the stopped run's list is gone.
    pub resumed: bool,
    /// True when the registry held the folder for a project whose `.fold/` was deleted: this project took that registration's place and its slug.
    pub replaced: bool,
    /// True when the folder held the complete project that a person removed from fold: init made nothing new and brought that project back under its slug. `imported` then holds what `revision` records, with no skipped paths.
    pub restored: bool,
}

/// Files a caller writes into a new project's folder before init records it, so they are part of `r1` unless an ignore rule leaves them out. It returns the files and folders it created, which a failed init removes; a file it changed stays changed. Blocking.
pub type FolderFiles = Box<dyn FnOnce(&Path) -> Result<Vec<PathBuf>, FoldError> + Send>;

/// Work a caller does in a new project's folder right after init creates its Git repository and before the commits are written, such as giving the repository's own config the Git identity that every commit fold writes takes. Blocking.
pub type RepositorySetup = Box<dyn FnOnce(&Path) + Send>;

/// How init takes the project folder.
enum Start {
    /// The folder is not a Git repository: the genesis `r0` is the empty tree, and the folder's files, less the ignore rules, with the empty `.foldignore` init writes when the folder has none and the `files` it writes, become `r1`, the change of the human who runs init. With `repository`, init then turns Git on as [`init_new_project`] describes.
    New {
        files: Option<FolderFiles>,
        repository: Option<RepositorySetup>,
    },
    /// The folder is a Git repository with a clean tree on `branch`: the genesis `r0` is the committed tree at `head`, whose commit is `head` itself, and Git sync is on.
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

/// Makes `root` a fold project whose folder is main: creates `.fold/`, migrates the project database, writes the commented config, writes an empty `.foldignore` when the folder has none and fold's lines into `.fold/exclude`, so neither the skills fold writes nor `.claude/settings.local.json` become part of `r1`, and in one transaction makes the genesis `r0` with the empty tree and records the folder's files, less the ignore rules, as `r1`, the change of the human `human` (an integration of kind `record` with its human change and events), then registers the project under a free slug. It needs no `git` and writes no Git data: Git sync stays off. A folder that is a Git repository, or inside another repository's working tree, is refused with a hint to use `--with-git` at the repository's top, and a folder in a Git directory, the `.git` of a repository or a bare repository, with a hint to choose a folder outside it. A failure after `.fold/` exists removes `.fold/` and what init created beside it and writes back the bytes of a file that init changed, never touching another file of the folder. A `.fold/` that the registry does not know is an init that stopped part-way, or a project that fold removed while its `.fold/` was away: when its database is complete the init finishes it and registers it, and otherwise it is refused with `project_incomplete` and nothing is removed. A registered folder that holds `.fold/` is refused with `project_exists`, or with `project_incomplete` when `.fold/` holds project content without a database, or a database without `main`; a registered folder without `.fold/`, or whose `.fold/` holds only what a daemon that still had the project open wrote after it was deleted, holds no project, so the new project replaces that registration in the transaction that registers it and keeps its slug.
pub async fn init_project(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    human: &HumanName,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    init_new_project(host, root, name, human, Git::default(), None, None, clock).await
}

/// What a plain init finds where the project folder is.
enum Found {
    Nothing,
    /// The folder is the top of a Git repository.
    Repository,
    /// The folder is below the top of the working tree at this path, whose repository would hold the new one.
    Inside(PathBuf),
    /// The folder is this Git directory or inside it, where Git keeps a repository's own data.
    InGitDirectory(PathBuf),
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
        "run `fld init --with-git` in {} to make the whole repository the project, or move the folder out of it",
        top.display()
    ))
    .with_details([("reason", serde_json::json!("inside_git_repository"))])
}

/// The folder is a Git directory or inside one, the `.git` of a repository or a bare repository, whose files Git alone writes.
fn in_git_directory(root: &Path, git_dir: &Path) -> FoldError {
    let message = if root == git_dir {
        format!("{} is a Git directory", root.display())
    } else {
        format!(
            "{} is inside the Git directory at {}",
            root.display(),
            git_dir.display()
        )
    };
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_hint(format!(
            "choose a folder outside {}, which holds a Git repository's own data",
            git_dir.display()
        ))
        .with_details([("reason", serde_json::json!("inside_git_directory"))])
}

/// [`init_project`] with the given `git` program, writing `files` into the folder with `.foldignore`. With `repository`, init also turns Git on once the folder's files are `r1`: a missing `.gitignore` is written as a copy of `.foldignore` with a `.fold/` line, and an existing one gets the `.fold/` line when no line of it is `.fold`, `.fold/`, `/.fold` or `/.fold/`; the new or changed `.gitignore` is the human's next change, its own revision, in the same transaction, which also turns Git sync on with `git.sync_enabled`. The repository comes after that transaction: `git init` on the `[git]` branch, `info/exclude` as the link to `.fold/exclude`, and `repository` run in the folder; then the commits are written from `r1`, because the empty genesis gets none, and the branch is created on the newest with the index set to it. A stop before the repository exists leaves it to the init that finishes this one.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one input of the init, and grouping them would only rename them"
)]
pub async fn init_new_project(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    human: &HumanName,
    git: Git,
    files: Option<FolderFiles>,
    repository: Option<RepositorySetup>,
    clock: &dyn Clock,
) -> Result<InitializedProject, ProjectError> {
    let root = prepare_root(root).await?;
    let (checked, git_checked) = (root.clone(), git.clone());
    let needs_git = repository.is_some();
    // A folder with `.fold/` is an init that stopped or a project, which `create_and_register` tells apart; the stopped init may have made the repository already.
    let found = blocking(move || {
        let paths = ProjectPaths::new(&checked);
        if paths.fold_dir().exists() {
            return Ok(Found::Nothing);
        }
        // A `.git` that Git does not read as a repository is refused too, so init never writes into it or deletes it.
        if std::fs::symlink_metadata(paths.git_dir()).is_ok() {
            return Ok(Found::Repository);
        }
        // A plain init needs no `git`, so without one on the PATH the repository around the folder is found from the `.git` entries above it.
        Ok(
            match git_checked.enclosing_repository(&checked, needs_git)? {
                Some(Enclosing::WorkingTree(top)) if top == checked => Found::Repository,
                Some(Enclosing::WorkingTree(top)) => Found::Inside(top),
                Some(Enclosing::GitDirectory(git_dir)) => Found::InGitDirectory(git_dir),
                None => Found::Nothing,
            },
        )
    })
    .await?;
    match found {
        Found::Nothing => {}
        Found::Repository => {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{} is a Git repository", root.display()),
            )
            .with_hint("run `fld init --with-git` there, so the repository keeps its history")
            .with_details([("reason", serde_json::json!("git_repository"))])
            .into());
        }
        Found::Inside(top) => return Err(inside_repository(&root, &top).into()),
        Found::InGitDirectory(git_dir) => return Err(in_git_directory(&root, &git_dir).into()),
    }
    let start = Start::New { files, repository };
    init_with(host, &root, name, human, git, start, clock).await
}

/// Makes the Git repository whose top is `root` a fold project whose folder is main, as [`init_project`] does, keeping the repository and its history: the genesis `r0` is the committed tree at `HEAD`, free of ignore rules, and its commit is `HEAD` itself, which `.fold/git-sync.map` records as its first line. Git sync is on from the start, turned on by `human`. The folder must be the top of a repository whose `.git` is a directory, with a branch checked out that has a commit and no change in the index or a tracked file; anything else is refused with a hint. The `[git]` branch becomes the checked-out branch. Nothing in the repository changes but `info/exclude`, which becomes the link to `.fold/exclude` with `/.fold/` and fold's Claude Code lines in it; files below fold's skill folders that `HEAD` tracks stay in `r0`, and the daemon's first open of the project stops tracking them.
pub async fn init_project_from_git(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    human: &HumanName,
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
        .with_hint("run `fld init` there to make a new project")
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
    init_with(host, &root, name, human, git, start, clock).await
}

/// The start of a `--with-git` init in `root`, or `None` when `root` is not the top of a repository. Blocking.
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
            "run `fld init --with-git` in a repository whose .git is a directory",
            "git_dir_elsewhere",
        ));
    }
    let Some(branch) = checked_out_branch(git, &paths.git_dir())? else {
        return Err(refused(
            "the repository has no branch checked out",
            "check out a branch, then run `fld init --with-git` again",
            "detached_head",
        ));
    };
    let Ok(head) = git.resolve_commit(root, "HEAD") else {
        return Err(refused(
            "the checked-out branch has no commit yet",
            "commit the files, then run `fld init --with-git` again",
            "no_commit",
        ));
    };
    if !folder_clean(git, &paths, &head)? {
        return Err(refused(
            "the repository has changes that are not committed",
            "commit or stash them, then run `fld init --with-git` again",
            "uncommitted_changes",
        ));
    }
    Ok(Some(Start::FromGit { head, branch }))
}

async fn init_with(
    host: &HostPaths,
    root: &Path,
    name: Option<&str>,
    human: &HumanName,
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
    let notify = notify::NotifySettings::new(host.notify_dir());
    let init = Init {
        registry: &registry,
        notify: &notify,
        root_text: &root_text,
        human,
        git: &git,
        clock,
    };
    let outcome = init.create_and_register(root, name, start).await;
    // The outcome is what the caller needs; a failed close only delays the WAL checkpoint.
    let _ = registry.close().await;
    outcome
}

/// The inputs every step of one init shares.
struct Init<'a> {
    registry: &'a Store,
    /// A project that takes over the slug of a registration whose `.fold/` was deleted starts with no notice targets.
    notify: &'a notify::NotifySettings,
    root_text: &'a str,
    human: &'a HumanName,
    git: &'a Git,
    clock: &'a dyn Clock,
}

impl Init<'_> {
    async fn create_and_register(
        &self,
        root: &Path,
        name: String,
        start: Start,
    ) -> Result<InitializedProject, ProjectError> {
        let registered =
            projects::find_registration_by_root(self.registry.read(), self.root_text).await?;
        let paths = ProjectPaths::new(root);
        let mut made = create_fold_dir(&paths).await?;
        if !made && registered.is_some() && remove_leftovers(&paths).await? {
            made = create_fold_dir(&paths).await?;
        }
        if !made {
            let restores = match &registered {
                Some(found) if found.removed_at.is_some() => Some(found.row.slug.as_str()),
                Some(_) => return Err(registered_refusal(&paths).await.into()),
                None => None,
            };
            let setup = match start {
                Start::New { repository, .. } => repository,
                Start::FromGit { .. } => None,
            };
            return self.finish_stopped(&paths, name, setup, restores).await;
        }
        let replaces = registered.map(|found| found.row.slug);
        let _init = match lock_fold_dir(&paths).await {
            Ok(lock) => lock,
            Err(error) => {
                remove_created(&paths, Created::default()).await;
                return Err(error);
            }
        };
        let mut created = Created::default();
        match self
            .populate_and_register(&paths, name, replaces.as_deref(), start, &mut created)
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

    async fn populate_and_register(
        &self,
        paths: &ProjectPaths,
        name: String,
        replaces: Option<&str>,
        start: Start,
        created: &mut Created,
    ) -> Result<InitializedProject, ProjectError> {
        let now = self.clock.now();
        let project_store = Store::open_project(paths).await?;
        let first = async {
            let branch = match &start {
                Start::FromGit { branch, .. } => Some(branch.as_str()),
                Start::New { .. } => None,
            };
            write_config(paths, branch).await?;
            let config = load_config(paths).await?;
            let first = self
                .create_first_revisions(&project_store, paths, start, &config, created, now)
                .await?;
            // The daemon writes the commits that are missing when it opens the project, so a failure here only delays them.
            // The folder holds main's files when init returns.
            let at = Some(first.gitignore.unwrap_or(first.revision));
            if let Err(error) = commit_main(&project_store, paths, self.git, at, self.clock).await {
                tracing::warn!(code = error.code().unwrap_or(codes::INTERNAL_ERROR), "the Git commits of main were not written; the daemon writes them when it opens the project");
            }
            Ok::<_, ProjectError>(first)
        }
        .await;
        let closed = project_store.close().await;
        let first = first?;
        closed?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("init_before_register", |_| Err(fired(
            "init_before_register"
        )
        .into()));
        let (row, replaced) = register(self.registry, self.root_text, name, replaces, now).await?;
        if replaced {
            self.notify.forget_project(&row.slug).await;
        }
        Ok(InitializedProject {
            slug: row.slug,
            name: row.name,
            root: PathBuf::from(row.root),
            revision: first.revision,
            imported: first.imported,
            git_commit: first.git_commit,
            gitignore: first.gitignore,
            git_sync: first.git_sync,
            resumed: false,
            replaced,
            restored: false,
        })
    }

    /// Finishes an init that stopped after it made `.fold/` and before it registered the project, or with `restores`, the slug of the folder's registration that a person removed, brings that project back into fold under its slug and name. A migrated database whose `main` names a revision with every object below it in the store is a project the stopped run completed, except perhaps for the repository of a project whose Git sync is on, which is then created, and the registry row, which is then inserted; the daemon writes the missing commits when it opens the project. Anything less is refused with `project_incomplete`, and so is a `.fold/` that a running init holds. Nothing is removed in either case, and no database is created in a `.fold/` that has none.
    async fn finish_stopped(
        &self,
        paths: &ProjectPaths,
        name: String,
        setup: Option<RepositorySetup>,
        restores: Option<&str>,
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
        let read = async {
            let head = head(store.read()).await?;
            let genesis = fold_store::workspaces::genesis(store.read()).await?;
            let synced = fold_store::workspaces::git_sync(store.read())
                .await?
                .is_some();
            Ok::<_, StoreError>((head, genesis, synced))
        }
        .await;
        let closed = store.close().await;
        let (row, genesis, git_sync) = read?;
        closed?;
        let (Some(head), Some(genesis)) = (row, genesis) else {
            return Err(incomplete(paths).into());
        };
        let fold_dir = paths.fold_dir();
        let revision = head.number;
        let branch = load_config(paths).await?.git.branch;
        let (git, paths_owned) = (self.git.clone(), paths.clone());
        let finished = blocking(move || {
            let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
            let Some(imported) = stored_contents(&objects, &head)? else {
                return Ok(None);
            };
            if !git_sync {
                return Ok(Some((imported, None)));
            }
            if !paths_owned.git_dir().exists() {
                git.init_checkout(paths_owned.root(), &branch)?;
                exclude_fold_dir(&paths_owned.git_dir())?;
                if let Some(setup) = setup {
                    setup(paths_owned.root());
                }
            }
            let git_commit = SyncMap::load(&fold_dir)?.commit_of(genesis.number).cloned();
            Ok(Some((imported, git_commit)))
        })
        .await?;
        let Some((imported, git_commit)) = finished else {
            return Err(incomplete(paths).into());
        };
        let row = match restores {
            Some(slug) => restore_registration(self.registry, slug, self.root_text).await?,
            None => {
                register(self.registry, self.root_text, name, None, self.clock.now())
                    .await?
                    .0
            }
        };
        Ok(InitializedProject {
            slug: row.slug,
            name: row.name,
            root: PathBuf::from(row.root),
            revision,
            imported,
            git_commit,
            gitignore: None,
            git_sync,
            resumed: restores.is_none(),
            replaced: false,
            restored: restores.is_some(),
        })
    }

    /// A new project's revisions, as [`Start`] describes them. Every file init writes into the folder and every object is in place before the one transaction that inserts the revisions, moves `main` by compare-and-swap from each to the next and appends their events, so a database that holds `main` holds the whole start; the sync map line of a Git genesis comes before that transaction too. A new repository comes after it, so an init that stops before its database is complete leaves no repository; one that stops after has it created when the init runs again. Both kinds of repository get `info/exclude` as the link to `.fold/exclude`, with `/.fold/` in it.
    async fn create_first_revisions(
        &self,
        store: &Store,
        paths: &ProjectPaths,
        start: Start,
        config: &ProjectConfig,
        created: &mut Created,
        now: Timestamp,
    ) -> Result<FirstRevisions, ProjectError> {
        let max_file_bytes = config.max_file_bytes;
        let label = Actor::Human {
            name: self.human.clone(),
        }
        .event_actor();
        let (files, repository) = match start {
            Start::FromGit { head, .. } => {
                return self
                    .genesis_from_git(store, paths, head, max_file_bytes, now)
                    .await;
            }
            Start::New { files, repository } => (files, repository),
        };
        let root = paths.root().to_owned();
        if let Some(foldignore) = blocking(move || write_foldignore(&root)).await? {
            created.files.push(foldignore);
        }
        let fold_dir = paths.fold_dir();
        blocking(move || Ok(exclude_claude_files(&fold_dir).map(drop)?)).await?;
        if let Some(files) = files {
            let root = paths.root().to_owned();
            let written = blocking(move || Ok(files(&root)?)).await?;
            created.files.extend(written);
        }
        let paths_owned = paths.clone();
        let (imported, genesis, mut changes) = blocking(move || {
            let paths = paths_owned;
            let root = paths.root();
            let fold_dir = paths.fold_dir();
            let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
            let caps = Capabilities::probe(&fold_dir).map_err(|error| io_error(&error))?;
            let opts = ImportOptions {
                max_file_bytes,
                caps,
            };
            let imported = import_dir(&objects, root, &IgnoreRules::new(root), None, &opts)?;
            #[cfg(feature = "failpoints")]
            fail::fail_point!("init_after_import", |_| Err(
                fired("init_after_import").into()
            ));
            let empty = write_initial_tree(&objects, FlatTree::new())?;
            let genesis =
                write_revision(&objects, &genesis_revision(empty, now)).map_err(FoldError::from)?;
            let mut changes = Vec::new();
            if imported.tree != empty {
                changes.push(first_change(&objects, &genesis, empty, imported.tree, now)?);
            }
            Ok((imported, genesis, changes))
        })
        .await?;
        let mut gitignore_change = None;
        if repository.is_some() {
            let root = paths.root().to_owned();
            if let Some(plan) = blocking(move || plan_gitignore(&root)).await? {
                // The old bytes are kept before the file changes, so a failure in between can still write them back.
                match &plan.old {
                    Some(old) => created.restore.push((plan.path.clone(), old.clone())),
                    None => created.files.push(plan.path.clone()),
                }
                let previous = changes.last().map_or(genesis, |change| change.stored);
                let fold_dir = paths.fold_dir();
                gitignore_change =
                    blocking(move || gitignore_change_after(&fold_dir, &plan, &previous, now))
                        .await?;
            }
        }
        let gitignore = gitignore_change
            .as_ref()
            .map(|change| change.stored.revision().number);
        changes.extend(gitignore_change);
        let git_sync = repository.is_some();
        let human = self.human.as_str().to_owned();
        store
            .write(async |tx| -> Result<(), ProjectError> {
                insert_revision(tx, &genesis).await?;
                create_main(tx, RevisionId::GENESIS).await?;
                let mut buffer = EventBuffer::new();
                for change in &changes {
                    change.record(tx, &mut buffer, &label, &human, now).await?;
                }
                let held = changes
                    .last()
                    .map_or(*genesis.revision(), |change| *change.stored.revision());
                let newest = newest_human_change(tx.conn(), true).await?;
                let record = Checkout {
                    tree: held.tree,
                    revision: held.number,
                    human_change: newest.map(|change| change.id),
                };
                set_checkout(tx, &record, now).await?;
                if git_sync {
                    enable_sync(tx, &mut buffer, &label, &config.git.branch, now).await?;
                }
                Ok(())
            })
            .await?;
        if let Some(setup) = repository {
            let (git, paths, branch) = (self.git.clone(), paths.clone(), config.git.branch.clone());
            created.repository = true;
            blocking(move || {
                git.init_checkout(paths.root(), &branch)?;
                exclude_fold_dir(&paths.git_dir())?;
                setup(paths.root());
                Ok(())
            })
            .await?;
        }
        let revision = changes.first().map_or(RevisionId::GENESIS, |change| {
            change.stored.revision().number
        });
        Ok(FirstRevisions {
            revision,
            imported,
            git_commit: None,
            gitignore,
            git_sync,
        })
    }

    /// The genesis of a Git repository's project: `HEAD`'s tree as `r0`, its map line, and Git sync on.
    async fn genesis_from_git(
        &self,
        store: &Store,
        paths: &ProjectPaths,
        head: CommitId,
        max_file_bytes: u64,
        now: Timestamp,
    ) -> Result<FirstRevisions, ProjectError> {
        let (git, paths_owned, commit) = (self.git.clone(), paths.clone(), head.clone());
        let (imported, genesis) = blocking(move || {
            let paths = paths_owned;
            let objects = ObjectStore::open(&paths.fold_dir()).map_err(FoldError::from)?;
            exclude_fold_dir(&paths.git_dir())?;
            exclude_claude_files(&paths.fold_dir())?;
            // A stash the repository holds already is the human's, and only a change of it counts as one fold did not make.
            let repo = paths.git_dir();
            let stash = git.ref_commit(&repo, STASH_REF)?;
            if let Some(stash) = &stash {
                set_ref(&git, &repo, FOLD_STASH_REF, Some(stash))?;
            }
            set_ref(&git, &repo, FOLD_BRANCH_REF, Some(&commit))?;
            let opts = GitImportOptions { max_file_bytes };
            let imported = import_commit(&objects, &git, paths.root(), commit.as_str(), &opts)?;
            let genesis = write_revision(&objects, &genesis_revision(imported.tree, now))
                .map_err(FoldError::from)?;
            Ok((imported, genesis))
        })
        .await?;
        let fold_dir = paths.fold_dir();
        let synced = (RevisionId::GENESIS, head.clone());
        blocking(move || Ok(SyncMap::load(&fold_dir)?.append(&[synced])?)).await?;
        let label = Actor::Human {
            name: self.human.clone(),
        }
        .event_actor();
        let branch = load_config(paths).await?.git.branch;
        store
            .write(async |tx| -> Result<(), ProjectError> {
                insert_revision(tx, &genesis).await?;
                create_main(tx, RevisionId::GENESIS).await?;
                let record = Checkout {
                    tree: genesis.revision().tree,
                    revision: RevisionId::GENESIS,
                    human_change: None,
                };
                set_checkout(tx, &record, now).await?;
                enable_sync(tx, &mut EventBuffer::new(), &label, &branch, now).await?;
                Ok(())
            })
            .await?;
        Ok(FirstRevisions {
            revision: RevisionId::GENESIS,
            imported,
            git_commit: Some(head),
            gitignore: None,
            git_sync: true,
        })
    }
}

/// What [`Init::create_first_revisions`] made.
struct FirstRevisions {
    revision: RevisionId,
    imported: ImportResult,
    git_commit: Option<CommitId>,
    gitignore: Option<RevisionId>,
    git_sync: bool,
}

/// One change of the human that a new project starts with: the folder's files as `r1`, or the `.gitignore` init wrote when it turned Git on.
struct FirstChange {
    stored: StoredRevision,
    base: ObjectHash,
    tree: ObjectHash,
    /// The paths it changes, in path order.
    files: Vec<String>,
}

impl FirstChange {
    /// Records the change as the `record` integration its revision names, in the init's transaction: the integration and the revision, `main` moved from the parent by compare-and-swap, the human change, and `integration.succeeded`, `main.advanced` and `human_change.recorded`. The integration number is the next one, as the revision encodes it; a fresh database gives the numbers in order.
    async fn record(
        &self,
        tx: &mut WriteTx,
        buffer: &mut EventBuffer,
        label: &EventActor,
        human: &str,
        now: Timestamp,
    ) -> Result<(), ProjectError> {
        let revision = self.stored.revision();
        let integration = revision
            .integration
            .ok_or(StoreError::Corrupt("revisions.integration_id"))?;
        let allocated = counters::allocate(tx, IdKind::Integration).await?;
        if allocated != integration.number() {
            return Err(StoreError::Corrupt("counters.next").into());
        }
        let previous = RevisionId::from_number(revision.number.number().saturating_sub(1))
            .ok_or(StoreError::Corrupt("revisions.number"))?;
        let new = NewIntegration {
            number: integration,
            subject: IntegrationSubject::Record {
                remote_commit: None,
                message: None,
            },
            actor: label.as_str(),
            created_at: now,
        };
        insert_succeeded(tx, &new, &self.stored).await?;
        advance_main(tx, previous, revision.number).await?;
        let change = NewHumanChange {
            commit: None,
            human,
            base_tree: self.base,
            tree: self.tree,
            now,
        };
        insert_recorded_human_change(tx, &change, integration).await?;
        let event = |kind, data: serde_json::Value| {
            NewEvent::new(kind, label.clone())
                .and_then(|event| event.data(data))
                .map(|event| event.revision(revision.number))
                .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
        };
        let events = [
            event(
                types::INTEGRATION_SUCCEEDED,
                serde_json::json!({
                    "integration": integration.to_string(),
                    "kind": "record",
                    "commit": null,
                }),
            )?,
            event(
                types::MAIN_ADVANCED,
                serde_json::json!({
                    "revision": revision.number.to_string(),
                    "previous": previous.to_string(),
                }),
            )?,
            event(
                types::HUMAN_CHANGE_RECORDED,
                serde_json::json!({
                    "commit": null,
                    "files": self.files.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
                    "file_count": self.files.len(),
                    "tasks": [],
                    "workspaces": [],
                    "leads": [],
                    "integration": integration.to_string(),
                }),
            )?,
        ];
        for event in &events {
            buffer.append(tx, event, now).await?;
        }
        Ok(())
    }
}

/// The most paths a human-change event lists; the rest are counted.
const LISTED_PATHS: usize = 50;

/// Writes the `.gitignore` of `plan` and returns its change on top of `previous`, the revision before it; `None` when the tree stays as it was. Blocking.
fn gitignore_change_after(
    fold_dir: &Path,
    plan: &GitignorePlan,
    previous: &StoredRevision,
    now: Timestamp,
) -> Result<Option<FirstChange>, ProjectError> {
    write_atomic(&plan.path, &plan.new).map_err(|error| io_error(&error))?;
    let objects = ObjectStore::open(fold_dir).map_err(FoldError::from)?;
    let blob = objects
        .write(ObjectKind::Blob, &plan.new)
        .map_err(FoldError::from)?;
    let base = previous.revision().tree;
    let mut entries = fold_vcs::compare::load_flat(&objects, &base)?;
    entries.insert(
        GITIGNORE.as_bytes().to_vec(),
        FlatEntry {
            kind: EntryKind::File,
            hash: blob,
        },
    );
    let tree = write_initial_tree(&objects, entries)?;
    if tree == base {
        return Ok(None);
    }
    first_change(&objects, previous, base, tree, now).map(Some)
}

/// The change from `base` to `tree` as the revision after `parent`, with the next integration number, its object written. Blocking.
fn first_change(
    objects: &ObjectStore,
    parent: &StoredRevision,
    base: ObjectHash,
    tree: ObjectHash,
    now: Timestamp,
) -> Result<FirstChange, ProjectError> {
    let parent_revision = parent.revision();
    let number = parent_revision
        .number
        .next()
        .ok_or(StoreError::Corrupt("revisions.number"))?;
    // The genesis has no integration, and each change after it takes the next number.
    let integration = IntegrationId::from_number(number.number())
        .ok_or(StoreError::Corrupt("integrations.number"))?;
    let revision = Revision {
        tree,
        parent: Some(parent.hash()),
        number,
        integration: Some(integration),
        created_at: now,
    };
    let stored = write_revision(objects, &revision).map_err(FoldError::from)?;
    let (before, after) = (
        fold_vcs::compare::load_flat(objects, &base)?,
        fold_vcs::compare::load_flat(objects, &tree)?,
    );
    let files = fold_vcs::scan::changes(&before, &after)
        .into_iter()
        .map(|change| String::from_utf8_lossy(&change.path).into_owned())
        .collect();
    Ok(FirstChange {
        stored,
        base,
        tree,
        files,
    })
}

/// Turns Git sync on with `git.sync_enabled` in the caller's transaction, for the human `label`. Nothing happens when it is on already.
pub(crate) async fn enable_sync(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    label: &EventActor,
    branch: &str,
    now: Timestamp,
) -> Result<bool, ProjectError> {
    if !fold_store::workspaces::enable_git_sync(tx, label.as_str(), now).await? {
        return Ok(false);
    }
    let event = NewEvent::new(types::GIT_SYNC_ENABLED, label.clone())
        .and_then(|event| event.data(serde_json::json!({ "branch": branch })))
        .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
    buffer.append(tx, &event, now).await?;
    Ok(true)
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

/// Clears the removed mark of the registration `slug` at `root` and returns its row. A row that went or changed since init read it refuses the folder with `project_exists`.
async fn restore_registration(
    registry: &Store,
    slug: &str,
    root_text: &str,
) -> Result<ProjectRow, ProjectError> {
    registry
        .write(async |tx| -> Result<ProjectRow, ProjectError> {
            projects::clear_removed(tx, slug).await?;
            match projects::find_project(tx.conn(), slug).await? {
                Some(row) if row.root == root_text => Ok(row),
                _ => Err(exists(Path::new(root_text)).into()),
            }
        })
        .await
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

/// The line init adds to a `.gitignore` that does not ignore `.fold`, before its line ending.
const FOLD_DIR_LINE: &str = ".fold/";

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

/// Writes an empty `.foldignore` into the folder when it has none, through [`write_atomic`], and returns its path; anything at the path, a symlink included, is left alone. Blocking.
fn write_foldignore(root: &Path) -> Result<Option<PathBuf>, ProjectError> {
    let foldignore = root.join(FOLDIGNORE);
    if read_regular(&foldignore)?.0 {
        return Ok(None);
    }
    write_atomic(&foldignore, b"").map_err(|error| io_error(&error))?;
    Ok(Some(foldignore))
}

/// What turning Git on does to the folder's `.gitignore`.
pub(crate) struct GitignorePlan {
    pub(crate) path: PathBuf,
    /// The bytes it holds now; `None` when there is none.
    pub(crate) old: Option<Vec<u8>>,
    pub(crate) new: Vec<u8>,
}

/// Decides the `.gitignore` that turning Git on writes, so Git ignores `.fold/` and leaves out what fold leaves out. A folder without one gets a regular file that copies `.foldignore` (empty when there is none) and ends with a `.fold/` line; the copy happens once, and later `.foldignore` only adds rules on top of `.gitignore` or brings back a path it ignores. An existing `.gitignore` gets the `.fold/` line only when it lacks one. `None` when nothing changes, and for a `.gitignore` that is not a regular file, which is left alone. Blocking.
pub(crate) fn plan_gitignore(root: &Path) -> Result<Option<GitignorePlan>, ProjectError> {
    let path = root.join(GITIGNORE);
    let (exists, current) = read_regular(&path)?;
    if exists {
        let Some(old) = current else {
            return Ok(None);
        };
        let new = with_fold_dir_line(old.clone());
        return Ok((new != old).then_some(GitignorePlan {
            path,
            old: Some(old),
            new,
        }));
    }
    let (_, foldignore) = read_regular(&root.join(FOLDIGNORE))?;
    Ok(Some(GitignorePlan {
        path,
        old: None,
        new: with_fold_dir_line(foldignore.unwrap_or_default()),
    }))
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
        "if no other `fld init` is running there, delete {} and run `fld init` again",
        paths.fold_dir().display()
    ))
}

fn io_error(error: &io::Error) -> ProjectError {
    ProjectError::Fold(FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot set up the project layout: {}", error.kind()),
    ))
}

/// Refuses a Git command while the project's Git sync is off: a plain folder has no Git repository until a human turns Git on.
pub(crate) async fn require_git_sync(store: &Store, command: &str) -> Result<(), ProjectError> {
    if fold_store::workspaces::git_sync(store.read())
        .await?
        .is_some()
    {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!("Git sync is off in this project, so {command} has no Git repository to use"),
    )
    .with_hint("run `fld init --with-git` in the project folder to turn Git on")
    .with_details([("reason", serde_json::json!("git_sync_off"))])
    .into())
}

/// A registry row and whether the daemon has the project open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    pub slug: String,
    pub name: String,
    /// The project folder, which holds `.fold/`.
    pub root: PathBuf,
    pub available: bool,
    /// When a person last opened the project or its newest event happened, whichever is later; the registration time before either.
    pub last_used_at: Timestamp,
    /// When a person removed the project from fold; `None` while it is in fold.
    pub removed_at: Option<Timestamp>,
    /// What is at the project folder: an open project's is there; any other's is looked at for the list.
    pub folder: ProjectFolder,
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
    /// What the project's supervised workers cost: in all, since the start of the day and since the start of the week of [`CostDays`].
    pub cost: fold_store::agents::CostTotals,
}

/// Where the viewer's day and week start, so the project list sums the cost of the viewer's own today and last seven days. Either left out starts at midnight UTC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CostDays {
    pub today: Option<Timestamp>,
    pub week: Option<Timestamp>,
}

/// Why the project, rather than one of its tasks, needs a person. The daemon decides every kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAttention {
    /// The project folder has changes that no revision records, which the human records with `fld record` or puts aside with `fld stash`. Such a change on a path main changed, or the conflict markers of `fld unstash`, keep the whole folder, and while Git sync is on the branch too, where they are until it is recorded or taken back.
    UncommittedChanges,
    /// The `[git]` branch or `HEAD` shows a change fold did not make, such as another branch checked out, a reset or a stash, so the project folder stops following main until the human brings the branch back.
    HeadMoved,
}

impl ProjectAttention {
    pub const ALL: [ProjectAttention; 2] = [
        ProjectAttention::UncommittedChanges,
        ProjectAttention::HeadMoved,
    ];

    /// The name the HTTP view and the `git.attention_cleared` event give the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UncommittedChanges => "uncommitted_changes",
            Self::HeadMoved => "head_moved",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::UncommittedChanges => 0,
            Self::HeadMoved => 1,
        }
    }
}

/// What the project folder's last follow and the commit writer's last run found that needs a person. It is derived from the folder and the repository, never stored: the first run after the project opens finds it again.
#[derive(Debug, Default)]
pub struct GitAttention {
    raised: [AtomicBool; 2],
    /// Since when the branch shows a change fold did not make, with the command that brings it back.
    moved: std::sync::Mutex<Option<(Timestamp, String)>>,
    /// Since when the project folder waits on changes of the human's that no revision records, with the paths main changed that hold them.
    held: std::sync::Mutex<Option<(Timestamp, Vec<String>)>>,
    /// True once a run that raises attention noted the branch and the held folder since the project opened. Until then a missing item says nothing, so the notifier keeps what it knew of these items across a restart.
    observed: AtomicBool,
}

/// How a run changed the project's attention.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AttentionChange {
    pub(crate) raised: Vec<ProjectAttention>,
    pub(crate) cleared: Vec<ProjectAttention>,
}

impl GitAttention {
    pub fn kinds(&self) -> Vec<ProjectAttention> {
        ProjectAttention::ALL
            .into_iter()
            .filter(|kind| self.raised[kind.index()].load(Ordering::Relaxed))
            .collect()
    }

    /// Since when the branch shows a change fold did not make, with the command that brings it back.
    pub fn head_moved(&self) -> Option<(Timestamp, String)> {
        crate::support::lock(&self.moved).clone()
    }

    /// Notes the hint of a branch change fold did not make, keeping the time it was first seen, or clears it with `None`.
    pub(crate) fn note_moved(&self, hint: Option<String>, now: Timestamp) {
        let mut moved = crate::support::lock(&self.moved);
        *moved = hint.map(|hint| {
            let since = moved.as_ref().map_or(now, |(since, _)| *since);
            (since, hint)
        });
    }

    /// Whether a run noted the branch and the held folder since the project opened.
    pub fn observed(&self) -> bool {
        self.observed.load(Ordering::Relaxed)
    }

    /// Marks that a run noted the branch and the held folder, so their absence now means they ended.
    pub fn mark_observed(&self) {
        self.observed.store(true, Ordering::Relaxed);
    }

    /// Since when the project folder waits on changes of the human's that no revision records, with the paths main changed that hold them.
    pub fn folder_held(&self) -> Option<(Timestamp, Vec<String>)> {
        crate::support::lock(&self.held).clone()
    }

    /// Notes the paths that keep the project folder where it is, keeping the time the folder first waited, or clears them with an empty list. Returns whether the set of paths changed.
    pub(crate) fn note_held(&self, paths: &[String], now: Timestamp) -> bool {
        let mut held = crate::support::lock(&self.held);
        let before: &[String] = held.as_ref().map_or(&[], |(_, before)| before);
        if before == paths {
            return false;
        }
        *held = (!paths.is_empty()).then(|| {
            let since = held.as_ref().map_or(now, |(since, _)| *since);
            (since, paths.to_vec())
        });
        true
    }

    /// Records the kinds a run found and returns what changed since the run before; the first run after the project opens compares with nothing found.
    pub(crate) fn set(&self, found: &[ProjectAttention]) -> AttentionChange {
        let mut change = AttentionChange::default();
        for kind in ProjectAttention::ALL {
            let now = found.contains(&kind);
            match (self.raised[kind.index()].swap(now, Ordering::Relaxed), now) {
                (false, true) => change.raised.push(kind),
                (true, false) => change.cleared.push(kind),
                _ => {}
            }
        }
        change
    }
}

/// Reads the overview of an open project from its store.
pub async fn overview(
    handle: &ProjectHandle,
    days: CostDays,
) -> Result<ProjectOverview, ProjectError> {
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
        cost: crate::services::agents::cost_totals(&handle.store, days.today, days.week, now)
            .await?,
    })
}

/// [`list_for`] with the cost of today and the week from midnight UTC.
pub async fn list(
    state: &DaemonState,
    include_removed: bool,
) -> Result<Vec<ProjectEntry>, ProjectError> {
    list_for(state, include_removed, CostDays::default()).await
}

/// Every project in fold, ordered by slug, with the overview of each open one, and with `include_removed` the projects a person removed too. A project whose overview cannot be read is listed without it, so one damaged store does not hide the others. A project in fold whose folder is there without `.fold/` is removed from fold first and is not listed.
pub async fn list_for(
    state: &DaemonState,
    include_removed: bool,
    days: CostDays,
) -> Result<Vec<ProjectEntry>, ProjectError> {
    let registrations = projects::list_registrations(state.registry.read()).await?;
    let mut entries = Vec::with_capacity(registrations.len());
    for registration in registrations {
        if registration.removed_at.is_some() {
            if include_removed {
                entries.push(entry_of(state, registration, days).await);
            }
            continue;
        }
        if project_folder(Path::new(&registration.row.root)).await == ProjectFolder::FoldMissing
            && state.projects.remove_if_deleted(&registration.row).await
        {
            continue;
        }
        entries.push(entry_of(state, registration, days).await);
    }
    Ok(entries)
}

/// What the list shows of one registration: the overview of an open project, and what is at the folder of any other.
async fn entry_of(
    state: &DaemonState,
    registration: projects::Registration,
    days: CostDays,
) -> ProjectEntry {
    let projects::Registration {
        row,
        last_used_at,
        removed_at,
    } = registration;
    let root = PathBuf::from(&row.root);
    let open = match state.projects.slot(&row.slug) {
        Some(Slot::Open(handle)) if removed_at.is_none() => Some(handle),
        _ => None,
    };
    let (available, overview, folder) = match open {
        Some(handle) => match self::overview(&handle, days).await {
            Ok(overview) => (true, Some(overview), ProjectFolder::Present),
            Err(error) => {
                tracing::warn!(project = %row.slug, code = error.log_code(), "project overview could not be read");
                (true, None, ProjectFolder::Present)
            }
        },
        None => (false, None, project_folder(&root).await),
    };
    ProjectEntry {
        available,
        last_used_at: last_use(last_used_at, overview.as_ref()),
        removed_at,
        folder,
        overview,
        root,
        slug: row.slug,
        name: row.name,
    }
}

/// How long a person's use of a project stands before the next one is written, so a screen that reads the project again and again writes the registry at most once per period.
const USE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// One open project as the project list shows it, for a person who opens it now: the registry notes the use, at most once per [`USE_INTERVAL`]. A use that cannot be written is logged and the project is shown with the use before it, so the registry never fails the read.
pub async fn show(
    state: &DaemonState,
    handle: &ProjectHandle,
    days: CostDays,
) -> Result<ProjectEntry, ProjectError> {
    let overview = overview(handle, days).await?;
    let used = match note_use(state, &handle.slug).await {
        Ok(used) => used,
        Err(error) => {
            tracing::warn!(project = %handle.slug, code = error.log_code(), "the use of the project could not be noted");
            None
        }
    };
    Ok(ProjectEntry {
        slug: handle.slug.clone(),
        name: handle.name.clone(),
        root: handle.paths.root().to_owned(),
        available: true,
        last_used_at: last_use(used.unwrap_or(Timestamp::EPOCH), Some(&overview)),
        removed_at: None,
        folder: ProjectFolder::Present,
        overview: Some(overview),
    })
}

/// A named human takes the project `slug` off fold: the registry marks it removed and the daemon closes it, as [`Projects::remove_for_person`] describes, and answers with the project as the list shows it. The data in the folder's `.fold/` stays, and `fld claude`, `fld new` or `fld init` there, or [`restore`], brings the project back under its slug. A project removed already stays as it is. While a worker of the project runs, the removal is refused with `project_workers_running`.
pub async fn remove(
    state: &DaemonState,
    actor: &Actor,
    slug: &str,
    days: CostDays,
) -> Result<ProjectEntry, ProjectError> {
    policy::can_remove_project(actor).map_err(FoldError::from)?;
    let found = registration(state, slug).await?;
    if found.removed_at.is_none() {
        state
            .projects
            .remove_for_person(&state.registry, slug, state.clock.now())
            .await?;
    }
    Ok(entry_of(state, registration(state, slug).await?, days).await)
}

/// A named human brings back the project `slug` that a person removed, while its folder holds the project's `.fold/`: the removed mark goes and the daemon opens the project, and the answer is the project as the list shows it. A project in fold stays as it is. A removed project whose folder is missing or whose `.fold/` holds no database keeps its mark and is refused with `project_unavailable`, and so is one whose `.fold/` is gone, where `fld init` makes a new project.
pub async fn restore(
    state: &DaemonState,
    actor: &Actor,
    slug: &str,
    days: CostDays,
) -> Result<ProjectEntry, ProjectError> {
    policy::can_remove_project(actor).map_err(FoldError::from)?;
    let found = registration(state, slug).await?;
    if found.removed_at.is_some() {
        refuse_return(slug, Path::new(&found.row.root)).await?;
        bring_back(state, slug).await?;
        if let Err(error) = open_registered(state, slug).await {
            tracing::warn!(project = %slug, code = error.log_code(), "the project is back in fold but did not open");
        }
        if let Err(error) = note_use(state, slug).await {
            tracing::warn!(project = %slug, code = error.log_code(), "the use of the project could not be noted");
        }
    }
    Ok(entry_of(state, registration(state, slug).await?, days).await)
}

/// Refuses to bring back the removed project `slug` unless its folder at `root` holds the project's `.fold/` with its database, so the mark stays on a project that cannot open: a missing folder or database is `project_unavailable` with its reason, and a missing `.fold/` is `project_unavailable` with `fold_missing`, where `fld init` makes a new project.
async fn refuse_return(slug: &str, root: &Path) -> Result<(), ProjectError> {
    match project_folder(root).await {
        ProjectFolder::Present => Ok(()),
        ProjectFolder::FoldMissing => Err(FoldError::new(
            codes::PROJECT_UNAVAILABLE,
            format!(
                "project {slug} cannot come back: {} is missing",
                ProjectPaths::new(root).fold_dir().display()
            ),
        )
        .with_hint(format!(
            "run `fld init` in {} to make a new project there",
            root.display()
        ))
        .with_details([("reason", serde_json::json!("fold_missing"))])
        .into()),
        ProjectFolder::RootMissing | ProjectFolder::DatabaseMissing => {
            Err(unopened(slug, root).await.into())
        }
    }
}

/// The registry row of `slug`, removed or not; a slug the registry does not hold is `project_not_found`.
async fn registration(
    state: &DaemonState,
    slug: &str,
) -> Result<projects::Registration, ProjectError> {
    projects::find_registration(state.registry.read(), slug)
        .await?
        .ok_or_else(|| {
            FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("project {slug} is not registered"),
            )
            .into()
        })
}

/// Clears the removed mark of `slug`. True when the project was removed.
async fn bring_back(state: &DaemonState, slug: &str) -> Result<bool, ProjectError> {
    let back = state
        .registry
        .write(async |tx| projects::clear_removed(tx, slug).await)
        .await?;
    if back {
        tracing::info!(project = %slug, "the removed project is back in fold");
    }
    Ok(back)
}

/// Notes that a person uses `slug` now, unless a use inside the last [`USE_INTERVAL`] stands, and returns the project's last use; `None` for a slug the registry does not hold.
pub async fn note_use(state: &DaemonState, slug: &str) -> Result<Option<Timestamp>, ProjectError> {
    let Some(found) = projects::find_registration(state.registry.read(), slug).await? else {
        return Ok(None);
    };
    let now = state.clock.now();
    let stale_before = now.checked_sub(USE_INTERVAL).unwrap_or(Timestamp::EPOCH);
    if found.last_used_at > stale_before {
        return Ok(Some(found.last_used_at));
    }
    let touched = state
        .registry
        .write(async |tx| projects::touch_project(tx, slug, now, stale_before).await)
        .await?;
    Ok(Some(if touched { now } else { found.last_used_at }))
}

fn last_use(used: Timestamp, overview: Option<&ProjectOverview>) -> Timestamp {
    overview
        .and_then(|overview| overview.last_change_at)
        .map_or(used, |changed| changed.max(used))
}

/// The open handle of `slug` for a request. A handle whose database was replaced or is missing, or whose folder is missing, is closed and the project is unavailable, with what to do for its folder; a project whose folder is there without `.fold/`, open or not, is removed from fold and is `project_not_found`, like a slug the registry does not hold, and so is a project a person removed, with the way to bring it back.
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
            None => match projects::find_registration(state.registry.read(), slug).await? {
                Some(found) if found.removed_at.is_some() => {
                    Err(removed_by_person(slug, Path::new(&found.row.root)).into())
                }
                _ => Err(FoldError::new(
                    codes::PROJECT_NOT_FOUND,
                    format!("project {slug} is not registered"),
                )
                .into()),
            },
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
    let removed =
        remove_deleted_rows(&registry, &notify::NotifySettings::new(host.notify_dir())).await;
    registry.close().await?;
    removed
}

async fn remove_deleted_rows(
    registry: &Store,
    notify: &notify::NotifySettings,
) -> Result<Vec<String>, ProjectError> {
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
            notify.forget_project(&row.slug).await;
            removed.push(row.slug);
        }
    }
    Ok(removed)
}

/// Opens a project that `fld init` registered after the daemon started, for a `fld` command in its folder: a project a person removed comes back into fold first, under its slug, while its folder holds its `.fold/` with the database, and keeps its mark otherwise, as [`restore`] does. An open project returns its handle again; a slug the registry does not hold is `project_not_found`.
pub async fn open(state: &DaemonState, slug: &str) -> Result<Arc<ProjectHandle>, ProjectError> {
    if let Some(found) = projects::find_registration(state.registry.read(), slug).await?
        && found.removed_at.is_some()
    {
        refuse_return(slug, Path::new(&found.row.root)).await?;
        bring_back(state, slug).await?;
    }
    let handle = open_registered(state, slug).await?;
    if let Err(error) = note_use(state, slug).await {
        tracing::warn!(project = %slug, code = error.log_code(), "the use of the project could not be noted");
    }
    Ok(handle)
}

async fn open_registered(
    state: &DaemonState,
    slug: &str,
) -> Result<Arc<ProjectHandle>, ProjectError> {
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

/// Opens the project whose registered root holds `cwd`, the nearest one when roots nest, so a session started in a task workspace finds its project. A session or hook in the folder of a project a person removed never brings it back, so a session that outlived the removal does not undo it; it is refused with the commands that do.
pub async fn open_for_dir(
    state: &DaemonState,
    cwd: &Path,
) -> Result<Arc<ProjectHandle>, ProjectError> {
    let found = row_for_dir(state.registry.read(), cwd).await?;
    if found.removed_at.is_some() {
        return Err(removed_by_person(&found.row.slug, Path::new(&found.row.root)).into());
    }
    open_registered(state, &found.row.slug).await
}

/// The slug of the registered project whose root holds `cwd`, the nearest one when roots nest.
pub async fn slug_for_dir(registry: &Store, cwd: &Path) -> Result<String, ProjectError> {
    Ok(row_for_dir(registry.read(), cwd).await?.row.slug)
}

/// The registration whose root holds `cwd`, the nearest one when roots nest, a project a person removed too, so a `fld` command there finds it and can bring it back.
async fn row_for_dir(
    registry: &fold_store::SqlitePool,
    cwd: &Path,
) -> Result<projects::Registration, ProjectError> {
    let rows = projects::list_registrations(registry).await?;
    let root = find_root(cwd, rows.iter().map(|found| Path::new(&found.row.root)));
    root.and_then(|root| rows.iter().find(|found| Path::new(&found.row.root) == root))
        .cloned()
        .ok_or_else(|| {
            FoldError::new(
                codes::PROJECT_NOT_FOUND,
                "no registered project holds this directory",
            )
            .with_hint("run the command inside a project directory, or `fld init` one")
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
    let found = row_for_dir(registry.read(), cwd).await;
    registry.close().await?;
    Ok(PathBuf::from(found?.row.root))
}

/// The registration of the project that holds `cwd`, a project a person removed too, read from the host registry without a running daemon and without writing it: a missing registry is not created, a registry of another schema version is refused, and a lock is waited for at most `busy_timeout`.
pub async fn row_for_dir_on_host(
    host: &HostPaths,
    cwd: &Path,
    busy_timeout: std::time::Duration,
) -> Result<projects::Registration, ProjectError> {
    let registry = Store::read_registry(&host.registry_db(), busy_timeout).await?;
    let found = row_for_dir(&registry, cwd).await;
    registry.close().await;
    found
}

/// The registration of the project that holds `cwd`, a project a person removed too, read from the host registry without a running daemon.
pub async fn registration_for_dir_on_host(
    host: &HostPaths,
    cwd: &Path,
) -> Result<projects::Registration, ProjectError> {
    let registry = Store::open_registry(&host.registry_db()).await?;
    let found = row_for_dir(registry.read(), cwd).await;
    registry.close().await?;
    found
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
        use ProjectAttention::{HeadMoved, UncommittedChanges};
        assert_eq!(attention.set(&[]), change(&[], &[]));
        assert_eq!(
            attention.set(&[HeadMoved, UncommittedChanges]),
            change(&[UncommittedChanges, HeadMoved], &[])
        );
        assert_eq!(attention.kinds(), [UncommittedChanges, HeadMoved]);
        assert_eq!(
            attention.set(&[UncommittedChanges, HeadMoved]),
            change(&[], &[])
        );
        assert_eq!(
            attention.set(&[HeadMoved]),
            change(&[], &[UncommittedChanges])
        );
        assert_eq!(attention.set(&[]), change(&[], &[HeadMoved]));
        assert!(attention.kinds().is_empty());
        assert_eq!(
            ProjectAttention::ALL.map(ProjectAttention::as_str),
            ["uncommitted_changes", "head_moved"]
        );
        attention.note_moved(
            Some("run `git switch main`".to_owned()),
            Timestamp::from_millis(5).unwrap(),
        );
        attention.note_moved(
            Some("run `git switch main`".to_owned()),
            Timestamp::from_millis(9).unwrap(),
        );
        assert_eq!(
            attention.head_moved(),
            Some((
                Timestamp::from_millis(5).unwrap(),
                "run `git switch main`".to_owned()
            ))
        );
        attention.note_moved(None, Timestamp::from_millis(9).unwrap());
        assert_eq!(attention.head_moved(), None);
    }

    #[test]
    fn the_held_folder_keeps_the_time_it_first_waited_and_the_newest_paths() {
        let attention = GitAttention::default();
        let at = |millis| Timestamp::from_millis(millis).unwrap();
        assert!(!attention.note_held(&[], at(1)));
        assert!(attention.note_held(&["a.txt".to_owned()], at(5)));
        assert!(!attention.note_held(&["a.txt".to_owned()], at(7)));
        assert!(attention.note_held(&["a.txt".to_owned(), "b.txt".to_owned()], at(9)));
        assert_eq!(
            attention.folder_held(),
            Some((at(5), vec!["a.txt".to_owned(), "b.txt".to_owned()]))
        );
        assert!(attention.note_held(&[], at(12)));
        assert_eq!(attention.folder_held(), None);
        assert!(attention.note_held(&["c.txt".to_owned()], at(20)));
        assert_eq!(
            attention.folder_held(),
            Some((at(20), vec!["c.txt".to_owned()]))
        );
    }
}

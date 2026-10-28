use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use fold_core::{Clock, FoldError, ProjectConfig, ProjectPaths, codes};
use fold_store::Store;
use fold_store::projects::ProjectRow;
use fold_vcs::git::Git;
use fold_vcs::platform::Capabilities;

use crate::daemon::{DRAIN_TIMEOUT, DaemonStart};
use crate::jobs::{self, AutoCommit, ProjectTask};
use crate::live::{LiveConfig, LiveState};
use crate::push::{self, PushTargets, PushTask};
use crate::services::agents::environment::WorkerEnvironment;
use crate::services::agents::{AgentService, Launcher, SeenPresence, Supervisor, code_of};
use crate::services::events::EventBus;
use crate::services::messages::MessageService;
use crate::services::projects::{GitAttention, ProjectError, RemoteContext, load_config};
use crate::services::reviews::ReviewService;
use crate::services::reviews::checks::{CheckPermits, CheckRunner, recover_check_runs};
use crate::services::tasks::TaskService;
use crate::services::workspaces::{
    BatchMergeCache, CarryCache, WorkspaceLocks, WorkspaceService, follow_closed_workspaces,
};
use crate::support::unpoisoned;

/// An open project: its database, its clock and the live state shared by every request. Later domains add their state here.
pub struct ProjectHandle {
    pub slug: String,
    pub name: String,
    pub paths: ProjectPaths,
    pub store: Store,
    /// Services and jobs read the time only through this clock.
    pub clock: Arc<dyn Clock>,
    pub bus: EventBus,
    /// What the calls and the presence job last saw of each agent's presence.
    pub presence: SeenPresence,
    /// Reads the claim lease from the project's configuration on each use, so every transport applies the same lease; the footer's near-stale count reads it again only when the file changed.
    pub tasks: TaskService,
    /// Held for the whole integration procedure, so a proposal integration and a pull integration never run at the same time. Every review service of the project shares it.
    pub integration: Arc<tokio::sync::Mutex<()>>,
    /// Held for a whole garbage collection run, so a second run is refused instead of queued.
    pub gc: tokio::sync::Mutex<()>,
    /// Keeps the sweep away from objects that are about to be named. A reused object must not be deleted between its refresh and the row that names it, and the sweep cannot see a row that is not committed. So every path that writes or refreshes objects and then records a row naming them holds this shared from its first object write until that transaction commits, and writes or refreshes each object less than the grace period before the row names it. The sweep holds it exclusive while it lists, checks and deletes objects. It is an `Arc` so a guard can move into blocking work and outlive an aborted caller.
    pub objects: Arc<tokio::sync::RwLock<()>>,
    /// Held for a whole push or pull, so two of them never run together in one project; a pull takes the integration mutex after it. It is an `Arc` so the guard can move into the blocking Git work and outlive an aborted caller.
    pub remote_lock: Arc<tokio::sync::Mutex<()>>,
    /// Held for each follow of main in the project folder and for a whole `fld record`.
    pub folder_lock: Arc<tokio::sync::Mutex<()>>,
    /// This handle once [`ProjectHandle::share`] made it an `Arc`, for the jobs that need its services but must not keep it open.
    pub(crate) shared: Arc<std::sync::OnceLock<std::sync::Weak<ProjectHandle>>>,
    /// What the folder's last follow and the commit writer last found that needs a person: changes in the project folder that no revision records, or a branch or `HEAD` that shows a change fold did not make.
    pub git_attention: Arc<GitAttention>,
    /// One restore, sync or record per workspace at a time, across every session of the project.
    pub workspace_locks: Arc<WorkspaceLocks>,
    /// What the file system under `.fold/` supports, probed by the first workspace command of any session or the first live attach.
    pub capabilities: Arc<OnceLock<Capabilities>>,
    /// The in-memory state of the project's active workspaces.
    pub live: LiveState,
    /// The earlier proposal versions that reads carried onto a newer base, shared by every review read of the project.
    pub carries: Arc<CarryCache>,
    /// The batches that reads merged in memory, shared by every review read of the project.
    pub batch_merges: Arc<BatchMergeCache>,
    /// The Claude Code inbox sockets of the project's agents. They live in memory only, so a restarted daemon has none until the session-start hooks run again.
    pub push: PushTargets,
    push_task: PushTask,
    /// The Claude Code processes of the project's worker agents.
    pub workers: Supervisor,
    /// The project's background runs of its checks, which share the machine-wide permits of the daemon's projects.
    pub checks: CheckRunner,
    /// Brings the project folder to main after each integration and when its Git branch changes, commits each revision to the project's repository while Git sync is on, and pushes it when the configuration asks.
    auto_commit: AutoCommit,
    /// Removes the directory of each closed workspace once its owning agent has no running worker process and no open session.
    workspace_cleanup: ProjectTask,
    /// Wakes `workspace_cleanup` for every closed workspace that is due, not only those its events name.
    cleanup_due: Arc<tokio::sync::Notify>,
    /// The device and inode of the database file this handle opened. SQLite keeps reading a file that was deleted, so only this tells the handle's database from a new one at the same path.
    database_file: Option<(u64, u64)>,
}

impl std::fmt::Debug for ProjectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectHandle")
            .field("slug", &self.slug)
            .finish_non_exhaustive()
    }
}

/// Files only the old layout had: a `main/` copy of main with its marker, and a Git repository inside `.fold/`.
const OLD_LAYOUT: [&str; 2] = ["main-materialised", "git"];

/// A project made when fold kept main in `main/`, the workspaces in `ws/` and its Git repository in `.fold/git` is refused: fold does not move it to the layout where the project folder is main.
async fn refuse_old_layout(slug: &str, paths: &ProjectPaths) -> Result<(), FoldError> {
    for name in OLD_LAYOUT {
        if tokio::fs::symlink_metadata(paths.fold_dir().join(name))
            .await
            .is_ok()
        {
            tracing::warn!(project = %slug, "project has the old layout with main/ and ws/");
            return Err(FoldError::new(
                codes::PROJECT_UNAVAILABLE,
                format!("project {slug} has the old layout with main/ and ws/, which this fold cannot open"),
            )
            .with_hint("re-create it with `fld init` in a new folder, or with `fld init --with-git` in a Git repository of its code")
            .with_details([("reason", serde_json::json!("old_layout"))]));
        }
    }
    Ok(())
}

impl ProjectHandle {
    /// Opens the project database, which applies its pending migrations. A missing database is not created: the registry only lists projects that `fld init` set up. The live state takes its intervals from the configuration read here; an invalid configuration gives it the defaults, and the commands that read the configuration report the error.
    pub async fn open(row: &ProjectRow, clock: Arc<dyn Clock>) -> Result<Self, FoldError> {
        Self::open_with(row, clock, Launcher::default()).await
    }

    /// The handle as the `Arc` every request shares, made known to its own jobs.
    pub fn share(self) -> Arc<Self> {
        let handle = Arc::new(self);
        let _ = handle.shared.set(Arc::downgrade(&handle));
        handle
    }

    /// [`ProjectHandle::open`] with the programs its worker processes run.
    pub async fn open_with(
        row: &ProjectRow,
        clock: Arc<dyn Clock>,
        launcher: Launcher,
    ) -> Result<Self, FoldError> {
        let paths = ProjectPaths::new(&row.root);
        let present = tokio::fs::try_exists(paths.db()).await.unwrap_or(false);
        if !present {
            tracing::warn!(project = %row.slug, "project database is missing");
            return Err(unavailable(&row.slug));
        }
        refuse_old_layout(&row.slug, &paths).await?;
        let config = load_config(&paths).await.unwrap_or_else(|error| {
            tracing::warn!(project = %row.slug, code = error.log_code(), "project configuration is invalid; the live state uses the defaults");
            ProjectConfig::default()
        });
        let store = Store::open_project(&paths).await.map_err(|error| {
            tracing::warn!(project = %row.slug, code = error.log_code(), "project failed to open");
            unavailable(&row.slug)
        })?;
        let database_file = tokio::fs::metadata(paths.db())
            .await
            .ok()
            .map(|meta| (meta.dev(), meta.ino()));
        let bus = EventBus::new();
        let capabilities = Arc::new(OnceLock::new());
        let live = LiveState::new(
            store.clone(),
            bus.clone(),
            clock.clone(),
            paths.clone(),
            LiveConfig::from_project(&config),
            Arc::clone(&capabilities),
        );
        let workspace_locks = Arc::new(WorkspaceLocks::default());
        let objects = Arc::new(tokio::sync::RwLock::new(()));
        let tasks = TaskService::new(
            store.clone(),
            bus.clone(),
            clock.clone(),
            ProjectConfig::default().lease_minutes,
        )
        .with_project_config(paths.clone())
        .with_live(live.clone())
        .with_workspace_locks(workspace_locks.clone())
        .with_objects(&paths, objects.clone());
        let push = PushTargets::new();
        let push_task = push::run(store.clone(), clock.clone(), &bus, push.clone());
        let workers = Supervisor::new(
            store.clone(),
            bus.clone(),
            clock.clone(),
            paths.clone(),
            push.clone(),
            launcher,
        );
        let remote_lock = Arc::new(tokio::sync::Mutex::new(()));
        let folder_lock = Arc::new(tokio::sync::Mutex::new(()));
        let shared: Arc<std::sync::OnceLock<std::sync::Weak<ProjectHandle>>> = Arc::default();
        let git_attention = Arc::new(GitAttention::default());
        let auto_commit = jobs::auto_commit(
            RemoteContext {
                slug: row.slug.clone(),
                paths: paths.clone(),
                store: store.clone(),
                clock: clock.clone(),
                bus: bus.clone(),
                remote_lock: remote_lock.clone(),
                folder_lock: folder_lock.clone(),
                objects: objects.clone(),
                git_attention: git_attention.clone(),
                handle: shared.clone(),
            },
            Git::default(),
        );
        let cleanup_due = Arc::new(tokio::sync::Notify::new());
        workers.set_cleanup_due(cleanup_due.clone());
        let cleanup = WorkspaceService::new(
            store.clone(),
            bus.clone(),
            clock.clone(),
            paths.clone(),
            ProjectConfig::default().lease_minutes,
        )
        .with_project_config()
        .with_locks(objects.clone(), workspace_locks.clone())
        .with_capabilities(capabilities.clone())
        .with_live(live.clone());
        let workspace_cleanup = ProjectTask::spawn(follow_closed_workspaces(
            cleanup,
            bus.clone(),
            bus.subscribe(),
            cleanup_due.clone(),
        ));
        Ok(Self {
            slug: row.slug.clone(),
            name: row.name.clone(),
            paths,
            store,
            clock,
            bus,
            presence: SeenPresence::new(),
            tasks,
            integration: Arc::new(tokio::sync::Mutex::new(())),
            gc: tokio::sync::Mutex::new(()),
            objects,
            remote_lock,
            folder_lock,
            shared,
            git_attention,
            workspace_locks,
            capabilities,
            live,
            carries: Arc::new(CarryCache::default()),
            batch_merges: Arc::new(BatchMergeCache::default()),
            push,
            push_task,
            workers,
            checks: CheckRunner::default().with_project(&row.slug),
            auto_commit,
            workspace_cleanup,
            cleanup_due,
            database_file,
        })
    }

    /// What is at the path of the project's database now. A file that cannot be read for another reason counts as the opened one, so a passing permission problem never closes a working project.
    pub async fn database_file(&self) -> DatabaseFile {
        let Some(opened) = self.database_file else {
            return DatabaseFile::Opened;
        };
        match tokio::fs::metadata(self.paths.db()).await {
            Ok(meta) if (meta.dev(), meta.ino()) == opened => DatabaseFile::Opened,
            Ok(_) => DatabaseFile::Replaced,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => DatabaseFile::Missing,
            Err(_) => DatabaseFile::Opened,
        }
    }

    /// True when the project's database file is gone or is another file than the one this handle opened, as after the project's `.fold/` was deleted, with or without a new `fld init` in the folder.
    pub async fn database_replaced(&self) -> bool {
        self.database_file().await != DatabaseFile::Opened
    }

    /// Ends the worker processes, closes the bus and then the project, as the daemon's stop does for each project, so nothing of the handle runs on after it.
    pub(crate) async fn close(&self) {
        self.workers.shutdown().await;
        self.bus.close();
        self.stop().await;
    }

    /// Closes the project after its workers stopped and its bus closed: the live state of its workspaces is written and let go, its commit writer and push delivery end, every open session ends and its database is checkpointed and closed.
    pub(crate) async fn stop(&self) {
        self.checks.shutdown().await;
        for live in self.live.all() {
            self.live.detach(live.view.workspace).await;
        }
        self.stop_tasks(DRAIN_TIMEOUT).await;
        if let Err(error) = self.agents().close_sessions_at_shutdown().await {
            tracing::error!(project = %self.slug, %error, "could not end sessions");
        }
        if let Err(error) = self.store.close().await {
            tracing::error!(project = %self.slug, %error, "could not close the project database");
        }
    }

    /// Waits up to `timeout` for the commit writer, the push delivery and the removal of closed workspace directories to end after [`EventBus::close`], then aborts them, so none outlives the project's database.
    pub async fn stop_tasks(&self, timeout: Duration) {
        self.auto_commit.stopped(timeout).await;
        self.push_task.stopped(timeout).await;
        self.workspace_cleanup.stopped(timeout).await;
    }

    /// Asks the project's cleanup task to remove every closed workspace directory that is due, such as those a stopped daemon left. It returns at once; the removal runs in the background.
    pub fn remove_closed_directories(&self) {
        self.cleanup_due.notify_one();
    }

    /// The agent service of this project: its store, bus, clock, configured lease, seen presence and push targets.
    pub fn agents(&self) -> AgentService {
        AgentService::new(self.store.clone(), self.bus.clone(), self.clock.clone())
            .with_project_config(self.paths.clone())
            .with_seen_presence(self.presence.clone())
            .with_push_targets(self.push.clone())
            .with_live(self.live.clone())
            .with_workers(self.workers.clone())
            .with_workspace_locks(self.workspace_locks.clone())
    }

    /// The message service of this project, which reaches the humans of `registry`.
    pub fn messages(&self, registry: &Store) -> MessageService {
        MessageService::new(
            self.store.clone(),
            registry.clone(),
            self.bus.clone(),
            self.clock.clone(),
        )
        .with_push_targets(self.push.clone())
    }

    /// The review service of this project. Every transport builds it here, per request or session, so it always shares the project's integration mutex, object lock and workspace locks, and knows the humans of `registry`.
    pub fn reviews(&self, registry: &Store) -> ReviewService {
        ReviewService::new(
            self.store.clone(),
            self.bus.clone(),
            self.clock.clone(),
            WorkspaceService::for_project(self),
            self.integration.clone(),
        )
        .with_registry(registry.clone())
        .with_checks(self.checks.clone())
    }

    /// The handle with its check runs taking `permits`, the ones every project of the daemon shares.
    #[must_use]
    pub fn with_check_permits(mut self, permits: CheckPermits) -> Self {
        self.checks = CheckRunner::new(permits).with_project(&self.slug);
        self
    }
}

/// What a project handle finds at the path of its database. SQLite keeps reading a file that was deleted, so a handle on a file that is not there any more still works, and only this tells the cases apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseFile {
    /// The file the handle opened.
    Opened,
    /// No file, as after the project's `.fold/` was deleted.
    Missing,
    /// Another file, as after a new `fld init` in the folder.
    Replaced,
}

/// What is at a registered project's folder. Only a path that is not there counts as missing: one that cannot be looked at for another reason, as for a permission problem, a stale network mount or an I/O error, counts as there, so such a problem never closes or removes a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFolder {
    /// `.fold/fold.db` is there, or cannot be looked at.
    Present,
    /// The folder itself is gone, as on a disk that is not mounted. The project stays registered and is unavailable, and the daemon opens it again when the folder comes back.
    RootMissing,
    /// The folder is there without `.fold/`: its fold data was deleted, and fold removes the project.
    FoldMissing,
    /// `.fold/` is there without its database. The project stays registered and is unavailable until the database is restored or `fld init` makes a new project there.
    DatabaseMissing,
}

/// Looks at `root`, then `.fold/`, then the database. The root is followed through a symbolic link, so a link to a disk that is not mounted counts as a missing folder; `.fold/` and the database are not.
pub async fn project_folder(root: &Path) -> ProjectFolder {
    fn gone(looked: std::io::Result<std::fs::Metadata>) -> bool {
        matches!(looked, Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    }
    let paths = ProjectPaths::new(root);
    if gone(tokio::fs::metadata(root).await) {
        ProjectFolder::RootMissing
    } else if gone(tokio::fs::symlink_metadata(paths.fold_dir()).await) {
        ProjectFolder::FoldMissing
    } else if gone(tokio::fs::symlink_metadata(paths.db()).await) {
        ProjectFolder::DatabaseMissing
    } else {
        ProjectFolder::Present
    }
}

/// The refusal of a registered project that the daemon cannot open now, with what the human can do for its folder.
pub async fn unopened(slug: &str, root: &Path) -> FoldError {
    let paths = ProjectPaths::new(root);
    match project_folder(root).await {
        ProjectFolder::RootMissing => FoldError::new(
            codes::PROJECT_UNAVAILABLE,
            format!(
                "project {slug} is unavailable: its folder {} is missing",
                root.display()
            ),
        )
        .with_hint(format!(
            "mount the disk that holds {} or put the folder back; fold opens the project again when it is there",
            root.display()
        ))
        .with_details([("reason", serde_json::json!("folder_missing"))]),
        ProjectFolder::DatabaseMissing => database_missing(slug, &paths),
        ProjectFolder::Present | ProjectFolder::FoldMissing => unavailable(slug),
    }
}

/// The refusal of a project whose `.fold/` holds no database.
pub fn database_missing(slug: &str, paths: &ProjectPaths) -> FoldError {
    FoldError::new(
        codes::PROJECT_UNAVAILABLE,
        format!(
            "project {slug} is unavailable: its project database {} is missing",
            paths.db().display()
        ),
    )
    .with_hint(database_missing_hint(paths))
    .with_details([("reason", serde_json::json!("database_missing"))])
}

/// The way back for a `.fold/` without its database, shared by the daemon, `fld init` and `fld doctor`.
pub fn database_missing_hint(paths: &ProjectPaths) -> String {
    format!(
        "restore {} from a backup, or delete {} and run `fld init` again to make a new project there",
        paths.db().display(),
        paths.fold_dir().display()
    )
}

/// The refusal of a project that fold removed because its folder was there without `.fold/`. A `.fold/` that comes back holds a complete project that is not registered, and `fld init` there registers it again.
pub fn removed(slug: &str, root: &Path) -> FoldError {
    FoldError::new(
        codes::PROJECT_NOT_FOUND,
        format!(
            "project {slug} is not registered: {} is missing, so fold removed the project",
            ProjectPaths::new(root).fold_dir().display()
        ),
    )
    .with_hint(format!(
        "if {} comes back, run `fld init` in {} to register the project again",
        ProjectPaths::new(root).fold_dir().display(),
        root.display()
    ))
}

/// The refusal of a project a person removed from fold. Its data is still in `.fold/`, and a `fld` command that registers the folder brings it back under its slug.
pub fn removed_by_person(slug: &str, root: &Path) -> FoldError {
    FoldError::new(
        codes::PROJECT_NOT_FOUND,
        format!(
            "project {slug} was removed from fold; its data stays in {}",
            ProjectPaths::new(root).fold_dir().display()
        ),
    )
    .with_hint(format!(
        "run `fld claude`, `fld new` or `fld init` in {}, or add it back on the projects page",
        root.display()
    ))
    .with_details([("reason", serde_json::json!("removed"))])
}

fn unavailable(slug: &str) -> FoldError {
    FoldError::new(
        codes::PROJECT_UNAVAILABLE,
        format!("project {slug} is unavailable"),
    )
}

/// What the daemon knows about a registered project.
#[derive(Debug, Clone)]
pub enum Slot {
    Open(Arc<ProjectHandle>),
    /// The open failed; the other projects keep running.
    Unavailable,
}

/// Writes the files that the program which runs the daemon keeps in every project folder, such as `fld`'s Claude Code skills, which this crate does not hold. The daemon calls it with the project root each time it opens a project, before any worker of the project starts, so every worker finds the files of the running binary also in a project that an older fold set up.
#[derive(Clone, Default)]
pub struct FolderFiles(Option<Arc<WriteFolder>>);

/// The write of [`FolderFiles`], given the project root.
type WriteFolder = dyn Fn(&Path) + Send + Sync;

impl FolderFiles {
    pub fn new(write: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(write)))
    }

    async fn write(&self, root: &Path) {
        let Some(write) = self.0.clone() else {
            return;
        };
        let root = root.to_owned();
        if tokio::task::spawn_blocking(move || write(&root))
            .await
            .is_err()
        {
            tracing::error!("the write of the project folder's files stopped");
        }
    }
}

impl std::fmt::Debug for FolderFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FolderFiles")
            .field(&self.0.is_some())
            .finish()
    }
}

/// The projects the daemon has tried to open, by slug.
#[derive(Debug, Default)]
pub struct Projects {
    slots: RwLock<HashMap<String, Slot>>,
    /// Serialises opens so one database never gets two writers.
    opening: tokio::sync::Mutex<()>,
    /// The programs every project's worker processes run.
    launcher: Launcher,
    /// What the daemon found at its start about the environment of the workers.
    environment: WorkerEnvironment,
    /// When and how the daemon process started.
    start: DaemonStart,
    /// The host registry, whose humans a worker's `AskUserQuestion` call reaches, and from which a project whose `.fold/` is gone is removed.
    registry: Option<Store>,
    /// The runs of checks that may go at once across every project.
    check_permits: CheckPermits,
    /// The root of each project removed because its `.fold/` was gone, kept until the next answer for its slug, so that answer carries the `fld init` hint also when the live job removed the project.
    removed: RwLock<HashMap<String, PathBuf>>,
    /// The projects that are unavailable because their folder is missing, which the live job opens again when the folder is back.
    away: RwLock<HashSet<String>>,
    /// The daemon's folder of kept push targets, one subfolder per project slug; `None` keeps them in memory only.
    push_files: Option<PathBuf>,
    /// The notice settings of the host and its projects; `None` sends no notices.
    notify: Option<crate::services::projects::notify::NotifySettings>,
    folder_files: FolderFiles,
}

impl Projects {
    #[must_use]
    pub fn with_launcher(mut self, launcher: Launcher) -> Self {
        self.launcher = launcher;
        self
    }

    #[must_use]
    pub fn with_environment(mut self, environment: WorkerEnvironment) -> Self {
        self.environment = environment;
        self
    }

    #[must_use]
    pub fn with_start(mut self, start: DaemonStart) -> Self {
        self.start = start;
        self
    }

    pub fn start(&self) -> DaemonStart {
        self.start
    }

    pub fn launcher(&self) -> &Launcher {
        &self.launcher
    }

    pub fn environment(&self) -> &WorkerEnvironment {
        &self.environment
    }

    #[must_use]
    pub fn with_registry(mut self, registry: Store) -> Self {
        self.registry = Some(registry);
        self
    }

    #[must_use]
    pub fn with_folder_files(mut self, files: FolderFiles) -> Self {
        self.folder_files = files;
        self
    }

    /// Keeps the notice settings of the host and its projects below `dir`.
    #[must_use]
    pub fn with_notify_dir(mut self, dir: PathBuf) -> Self {
        self.notify = Some(crate::services::projects::notify::NotifySettings::new(dir));
        self
    }

    pub fn notify(&self) -> Option<&crate::services::projects::notify::NotifySettings> {
        self.notify.as_ref()
    }

    /// Keeps each project's push targets below `dir`, in the folder named by the project's slug.
    #[must_use]
    pub fn with_push_files(mut self, dir: PathBuf) -> Self {
        self.push_files = Some(dir);
        self
    }

    pub fn slot(&self, slug: &str) -> Option<Slot> {
        unpoisoned(self.slots.read()).get(slug).cloned()
    }

    pub fn handles(&self) -> Vec<Arc<ProjectHandle>> {
        unpoisoned(self.slots.read())
            .values()
            .filter_map(|slot| match slot {
                Slot::Open(handle) => Some(handle.clone()),
                Slot::Unavailable => None,
            })
            .collect()
    }

    /// Opens `row` unless it is already open, in which case the same handle comes back. A folder that is there without `.fold/`, as after its fold data was deleted, is removed from fold instead, with `project_not_found`; a missing folder or a `.fold/` without its database leaves the project registered and unavailable. An open handle whose database was replaced is closed first, with its workers, and leaves the slot, so the new project that `fld init` made there opens fresh. A new handle runs its start-up sequence before any request can reach it: the database opens with SQLite's own WAL recovery, the workspaces are reconciled with their directories, and every session a previous run left open ends at the start time with its `session.ended`, so presence shows offline until the adapters reconnect; the inbox socket that the daemon kept for each agent, outside the project folder, is then restored while its session still listens, so a session that outlived the daemon gets its notices again. The handle's commit writer runs as it starts, which writes the commit of a revision an integration cut off after its own commit never got and brings the project folder to it, and changes nothing otherwise. No session of this run can exist yet, because a session starts only through an open handle. Once the handle is in place, every worker that was working or idle runs again with its session resumed; a resumed worker's adapter connects through the daemon's socket, which is bound by then, and its handshake opens this same project, so the resume runs after the open has let go of its lock.
    pub async fn open(
        &self,
        row: &ProjectRow,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<ProjectHandle>, FoldError> {
        let (handle, opened) = self.open_once(row, clock).await?;
        if opened {
            resume_workers(&handle).await;
        }
        Ok(handle)
    }

    /// [`Projects::open`] without the resume of the workers, for the daemon's start: it opens its projects before it binds its socket, and resumes their workers with [`resume_workers`] once the socket and the HTTP listener are bound, so a resumed worker's adapter always finds the daemon.
    pub async fn open_at_start(
        &self,
        row: &ProjectRow,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<ProjectHandle>, FoldError> {
        self.open_once(row, clock).await.map(|(handle, _)| handle)
    }

    /// The handle of `row` and whether this call opened it.
    async fn open_once(
        &self,
        row: &ProjectRow,
        clock: Arc<dyn Clock>,
    ) -> Result<(Arc<ProjectHandle>, bool), FoldError> {
        let _opening = self.opening.lock().await;
        let root = Path::new(&row.root);
        if self.remove_locked(&row.slug, root).await {
            return Err(removed(&row.slug, root));
        }
        if let Some(handle) = self.current(&row.slug).await {
            return Ok((handle, false));
        }
        // Read again under the open lock, which a removal holds too, so an open that read the row before a person removed it does not open the project again.
        if self.removed_by_person(&row.slug).await {
            return Err(removed_by_person(&row.slug, root));
        }
        let folder = project_folder(root).await;
        if folder != ProjectFolder::Present {
            // A missing folder or database keeps the registration: the folder can come back, and the database can be restored.
            unpoisoned(self.slots.write()).insert(row.slug.clone(), Slot::Unavailable);
            self.mark_away(&row.slug, folder == ProjectFolder::RootMissing);
            tracing::warn!(project = %row.slug, root = %root.display(), ?folder, "the project folder or its database is missing; the project is unavailable");
            return Err(unopened(&row.slug, root).await);
        }
        // An open of a folder that is there settles it, so a project that fails to open is not tried again on every tick.
        self.mark_away(&row.slug, false);
        let outcome = ProjectHandle::open_with(row, clock, self.launcher.clone())
            .await
            .map(|handle| handle.with_check_permits(self.check_permits.clone()))
            .map(ProjectHandle::share);
        if let Ok(handle) = &outcome {
            self.folder_files.write(root).await;
            unpoisoned(self.removed.write()).remove(&row.slug);
            if let Some(registry) = &self.registry {
                handle.workers.set_registry(registry.clone());
            }
            jobs::reconcile_workspaces(handle).await;
            handle.remove_closed_directories();
            if let Err(error) = recover_check_runs(&handle.store, &handle.bus, &*handle.clock).await
            {
                tracing::error!(project = %row.slug, code = error.log_code(), "could not end the check runs a stop cut short");
            }
            let reviews = match &self.registry {
                Some(registry) => handle.reviews(registry),
                None => ReviewService::new(
                    handle.store.clone(),
                    handle.bus.clone(),
                    handle.clock.clone(),
                    WorkspaceService::for_project(handle),
                    handle.integration.clone(),
                )
                .with_checks(handle.checks.clone()),
            };
            if let Err(error) = reviews.requeue_integrations().await {
                tracing::error!(project = %row.slug, code = error.log_code(), "could not queue again the integrations a stop left waiting");
            }
            if let Err(error) = handle.agents().close_orphan_sessions().await {
                tracing::error!(project = %row.slug, code = code_of(&error), "could not end orphan sessions");
            }
            if let Some(dir) = &self.push_files {
                handle.push.keep_files_in(dir.join(&row.slug));
            }
            if let Err(error) = handle.agents().restore_push_targets().await {
                tracing::error!(project = %row.slug, code = code_of(&error), "could not restore the kept inbox sockets");
            }
        }
        if let (Ok(handle), Some(notify)) = (&outcome, &self.notify) {
            notify.register(handle);
        }
        let slot = match &outcome {
            Ok(handle) => Slot::Open(handle.clone()),
            Err(_) => Slot::Unavailable,
        };
        unpoisoned(self.slots.write()).insert(row.slug.clone(), slot);
        outcome.map(|handle| (handle, true))
    }

    /// The open handle of `slug` while its database file is the one it opened. A handle whose file was deleted or replaced is closed, with its workers, and leaves the slot, so nothing of it runs on and the next open finds the folder as it is now. The caller holds `opening`.
    async fn current(&self, slug: &str) -> Option<Arc<ProjectHandle>> {
        let Some(Slot::Open(handle)) = self.slot(slug) else {
            return None;
        };
        let file = handle.database_file().await;
        if file == DatabaseFile::Opened {
            return Some(handle);
        }
        tracing::warn!(project = %slug, missing = file == DatabaseFile::Missing, "project database was deleted or replaced; the open project closes");
        unpoisoned(self.slots.write()).remove(slug);
        handle.close().await;
        None
    }

    fn mark_away(&self, slug: &str, away: bool) {
        let mut marks = unpoisoned(self.away.write());
        if away {
            marks.insert(slug.to_owned());
        } else {
            marks.remove(slug);
        }
    }

    /// True while `slug` is unavailable because its folder was missing when the daemon last looked.
    pub fn is_away(&self, slug: &str) -> bool {
        unpoisoned(self.away.read()).contains(slug)
    }

    /// Removes the project `slug` at `root` from fold when the folder is there without `.fold/`, as after its fold data was deleted: the registry row goes in one registry write, and an open handle closes with its workers, live state, jobs and store and leaves the slot. A missing folder or a `.fold/` without its database removes nothing. A daemon state without a registry removes nothing. The caller holds `opening`. True when the project was removed.
    async fn remove_locked(&self, slug: &str, root: &Path) -> bool {
        let Some(registry) = &self.registry else {
            return false;
        };
        if project_folder(root).await != ProjectFolder::FoldMissing {
            return false;
        }
        let root_text = root.to_string_lossy();
        let deleted = registry
            .write(async |tx| -> Result<bool, fold_store::StoreError> {
                // An init registers a project only after it made `.fold/`, inside a registry write like this one, so a project that an init registered at this root since the first look stays.
                if project_folder(root).await != ProjectFolder::FoldMissing {
                    return Ok(false);
                }
                fold_store::projects::delete_project(tx, slug, &root_text).await?;
                Ok(true)
            })
            .await;
        match deleted {
            Ok(true) => {}
            Ok(false) => return false,
            Err(error) => {
                tracing::error!(project = %slug, code = error.log_code(), "the project whose .fold is gone could not be removed from the registry");
                return false;
            }
        }
        unpoisoned(self.removed.write()).insert(slug.to_owned(), root.to_owned());
        if let Some(notify) = &self.notify {
            notify.forget_project(slug).await;
        }
        self.mark_away(slug, false);
        let slot = {
            let mut slots = unpoisoned(self.slots.write());
            // A handle of another folder under the slug is another project, which stays.
            let other =
                matches!(slots.get(slug), Some(Slot::Open(handle)) if handle.paths.root() != root);
            if other { None } else { slots.remove(slug) }
        };
        if let Some(Slot::Open(handle)) = slot {
            handle.close().await;
        }
        tracing::warn!(project = %slug, root = %root.display(), "the project folder has no .fold; fold removed the project");
        true
    }

    /// True when the registry marks `slug` removed. A registry that cannot be read, or a daemon state without one, counts as not removed.
    async fn removed_by_person(&self, slug: &str) -> bool {
        let Some(registry) = &self.registry else {
            return false;
        };
        matches!(
            fold_store::projects::find_registration(registry.read(), slug).await,
            Ok(Some(registration)) if registration.removed_at.is_some()
        )
    }

    /// Takes the project `slug` off fold for a person, under the open lock: in one write to `registry` its row gets the removed mark, and an open handle closes with its workers, live state, jobs and store and leaves the slot, as the daemon's stop closes a project. Nothing in the folder is written or deleted besides what that close keeps in `.fold/`. While a worker process of the project runs, nothing changes and the refusal names the workers. False when the registry holds no project `slug` in fold.
    pub async fn remove_for_person(
        &self,
        registry: &Store,
        slug: &str,
        now: fold_core::Timestamp,
    ) -> Result<bool, ProjectError> {
        let _opening = self.opening.lock().await;
        if let Some(Slot::Open(handle)) = self.slot(slug) {
            let running = handle.workers.running_agents();
            if !running.is_empty() {
                let names: Vec<&str> = running.iter().map(fold_core::AgentName::as_str).collect();
                return Err(FoldError::new(
                    codes::PROJECT_WORKERS_RUNNING,
                    format!("project {slug} has running workers: {}", names.join(", ")),
                )
                .with_hint(
                    "pause the project's agents or stop its workers, then remove the project",
                )
                .with_details([("workers", serde_json::json!(names))])
                .into());
            }
        }
        let marked = registry
            .write(async |tx| fold_store::projects::set_removed(tx, slug, now).await)
            .await?;
        if !marked {
            return Ok(false);
        }
        self.mark_away(slug, false);
        unpoisoned(self.removed.write()).remove(slug);
        let slot = unpoisoned(self.slots.write()).remove(slug);
        if let Some(Slot::Open(handle)) = slot {
            handle.close().await;
        }
        if let Some(notify) = &self.notify {
            notify.forget_project(slug).await;
        }
        tracing::info!(project = %slug, "a person removed the project from fold");
        Ok(true)
    }

    /// The root of `slug` when fold removed that project because its `.fold/` was gone and no answer for the slug has told it yet.
    pub fn take_removed(&self, slug: &str) -> Option<PathBuf> {
        unpoisoned(self.removed.write()).remove(slug)
    }

    /// [`Projects::remove_locked`] for a registered project, under the open lock.
    pub async fn remove_if_deleted(&self, row: &ProjectRow) -> bool {
        let _opening = self.opening.lock().await;
        self.remove_locked(&row.slug, Path::new(&row.root)).await
    }

    /// Closes the open handle of `slug` when its database file was deleted or replaced, as an open does, without opening the folder again. A folder without `.fold/` removes the project from fold. A missing folder leaves it unavailable until the folder comes back; a missing or replaced database leaves it unavailable until the next open, so a request never opens a project that `fld init` is still making. True when it closed the handle.
    pub async fn close_if_stale(&self, slug: &str) -> bool {
        let _opening = self.opening.lock().await;
        let Some(Slot::Open(handle)) = self.slot(slug) else {
            return false;
        };
        if self.remove_locked(slug, handle.paths.root()).await {
            return true;
        }
        if self.current(slug).await.is_some() {
            return false;
        }
        unpoisoned(self.slots.write()).insert(slug.to_owned(), Slot::Unavailable);
        let folder = project_folder(handle.paths.root()).await;
        self.mark_away(slug, folder == ProjectFolder::RootMissing);
        true
    }

    /// [`Projects::close_if_stale`] for every open project, and how many it closed. The daemon's live job runs it on every tick, so a project whose folder, `.fold/` or database went away stops within a tick even when nothing opens it.
    pub async fn close_stale(&self) -> usize {
        let mut closed = 0;
        for handle in self.handles() {
            if handle.database_replaced().await && self.close_if_stale(&handle.slug).await {
                closed += 1;
            }
        }
        closed
    }
}

/// Runs again every worker of `handle` that was working or idle when the last daemon stopped.
pub async fn resume_workers(handle: &ProjectHandle) {
    if let Err(error) = handle.workers.resume_after_restart().await {
        tracing::error!(project = %handle.slug, code = code_of(&error), "could not resume the workers");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_slot_map_keeps_its_slots() {
        let projects = Projects::default();
        projects
            .slots
            .write()
            .unwrap()
            .insert("web".to_owned(), Slot::Unavailable);
        let poisoned = std::panic::catch_unwind(|| {
            let _guard = projects.slots.write().unwrap();
            panic!("poison the lock");
        });
        assert!(poisoned.is_err());
        assert!(projects.slots.is_poisoned());
        assert!(matches!(projects.slot("web"), Some(Slot::Unavailable)));
        assert!(projects.handles().is_empty());
    }
}

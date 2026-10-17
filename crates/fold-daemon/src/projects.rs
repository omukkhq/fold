use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use fold_core::{Clock, FoldError, ProjectConfig, ProjectPaths, codes};
use fold_store::Store;
use fold_store::projects::ProjectRow;
use fold_vcs::git::Git;
use fold_vcs::platform::Capabilities;

use crate::daemon::DRAIN_TIMEOUT;
use crate::jobs::{self, AutoCommit};
use crate::live::{LiveConfig, LiveState};
use crate::push::{self, PushTargets, PushTask};
use crate::services::agents::{AgentService, Launcher, SeenPresence, Supervisor, code_of};
use crate::services::events::EventBus;
use crate::services::messages::MessageService;
use crate::services::projects::{GitAttention, RemoteContext, load_config};
use crate::services::reviews::ReviewService;
use crate::services::tasks::TaskService;
use crate::services::workspaces::{WorkspaceLocks, WorkspaceService};
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
    /// What the commit writer last found that needs a person: commits made outside fold, or changes in the project folder that keep it from following main.
    pub git_attention: Arc<GitAttention>,
    /// One restore, sync or record per workspace at a time, across every session of the project.
    pub workspace_locks: Arc<WorkspaceLocks>,
    /// What the file system under `.fold/` supports, probed by the first workspace command of any session or the first live attach.
    pub capabilities: Arc<OnceLock<Capabilities>>,
    /// The in-memory state of the project's active workspaces.
    pub live: LiveState,
    /// The Claude Code inbox sockets of the project's agents. They live in memory only, so a restarted daemon has none until the session-start hooks run again.
    pub push: PushTargets,
    push_task: PushTask,
    /// The Claude Code processes of the project's worker agents.
    pub workers: Supervisor,
    /// Commits each integration to the project's repository, moves the project folder to it and pushes it when the configuration asks.
    auto_commit: AutoCommit,
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
            .with_hint("re-create it with fld init in a new folder, or with fld init --from-git in a Git repository of its code")
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
        let push_task = push::run(store.clone(), &bus, push.clone());
        let workers = Supervisor::new(
            store.clone(),
            bus.clone(),
            clock.clone(),
            paths.clone(),
            push.clone(),
            launcher,
        );
        let remote_lock = Arc::new(tokio::sync::Mutex::new(()));
        let git_attention = Arc::new(GitAttention::default());
        let auto_commit = jobs::auto_commit(
            RemoteContext {
                slug: row.slug.clone(),
                paths: paths.clone(),
                store: store.clone(),
                clock: clock.clone(),
                bus: bus.clone(),
                remote_lock: remote_lock.clone(),
                git_attention: git_attention.clone(),
            },
            Git::default(),
        );
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
            git_attention,
            workspace_locks,
            capabilities,
            live,
            push,
            push_task,
            workers,
            auto_commit,
            database_file,
        })
    }

    /// True when the project's database file is gone or is another file than the one this handle opened, as after the project's `.fold/` was deleted, with or without a new `fld init` in the folder. A file that cannot be read for another reason is not replaced, so a passing permission problem never closes a working project.
    pub async fn database_replaced(&self) -> bool {
        let Some(opened) = self.database_file else {
            return false;
        };
        match tokio::fs::metadata(self.paths.db()).await {
            Ok(meta) => (meta.dev(), meta.ino()) != opened,
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    }

    /// Ends the worker processes, closes the bus and then the project, as the daemon's stop does for each project, so nothing of the handle runs on after it.
    pub(crate) async fn close(&self) {
        self.workers.shutdown().await;
        self.bus.close();
        self.stop().await;
    }

    /// Closes the project after its workers stopped and its bus closed: the live state of its workspaces is written and let go, its commit writer and push delivery end, every open session ends and its database is checkpointed and closed.
    pub(crate) async fn stop(&self) {
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

    /// Waits up to `timeout` for the commit writer and the push delivery to end after [`EventBus::close`], then aborts them, so neither outlives the project's database.
    pub async fn stop_tasks(&self, timeout: Duration) {
        self.auto_commit.stopped(timeout).await;
        self.push_task.stopped(timeout).await;
    }

    /// The agent service of this project: its store, bus, clock, configured lease, seen presence and push targets.
    pub fn agents(&self) -> AgentService {
        AgentService::new(self.store.clone(), self.bus.clone(), self.clock.clone())
            .with_project_config(self.paths.clone())
            .with_seen_presence(self.presence.clone())
            .with_push_targets(self.push.clone())
            .with_live(self.live.clone())
            .with_workers(self.workers.clone())
    }

    /// The message service of this project, which reaches the humans of `registry`.
    pub fn messages(&self, registry: &Store) -> MessageService {
        MessageService::new(
            self.store.clone(),
            registry.clone(),
            self.bus.clone(),
            self.clock.clone(),
        )
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
    }
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

/// The projects the daemon has tried to open, by slug.
#[derive(Debug, Default)]
pub struct Projects {
    slots: RwLock<HashMap<String, Slot>>,
    /// Serialises opens so one database never gets two writers.
    opening: tokio::sync::Mutex<()>,
    /// The programs every project's worker processes run.
    launcher: Launcher,
    /// The host registry, whose humans a worker's `AskUserQuestion` call reaches.
    registry: Option<Store>,
}

impl Projects {
    #[must_use]
    pub fn with_launcher(mut self, launcher: Launcher) -> Self {
        self.launcher = launcher;
        self
    }

    #[must_use]
    pub fn with_registry(mut self, registry: Store) -> Self {
        self.registry = Some(registry);
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

    /// Opens `row` unless it is already open, in which case the same handle comes back. An open handle whose database was deleted or replaced is closed first, with its workers, and leaves the slot, so the folder opens again as it is now: a new project that `fld init` made there opens fresh, and a folder without one is unavailable. A new handle runs its start-up sequence before any request can reach it: the database opens with SQLite's own WAL recovery, the workspaces are reconciled with their directories, and every session a previous run left open ends at the start time with its `session.ended`, so presence shows offline until the adapters reconnect. The handle's commit writer runs as it starts, which writes the commit of a revision an integration cut off after its own commit never got and brings the project folder to it, and changes nothing otherwise. No session of this run can exist yet, because a session starts only through an open handle. Once the handle is in place, every worker that was working or idle runs again with its session resumed; a resumed worker's adapter connects through the daemon's socket, which is bound by then, and its handshake opens this same project, so the resume runs after the open has let go of its lock.
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
        if let Some(Slot::Open(handle)) = self.slot(&row.slug) {
            if !handle.database_replaced().await {
                return Ok((handle, false));
            }
            tracing::warn!(project = %row.slug, "project database was deleted or replaced; the open project closes");
            unpoisoned(self.slots.write()).remove(&row.slug);
            handle.close().await;
        }
        let outcome = ProjectHandle::open_with(row, clock, self.launcher.clone())
            .await
            .map(Arc::new);
        if let Ok(handle) = &outcome {
            if let Some(registry) = &self.registry {
                handle.workers.set_registry(registry.clone());
            }
            jobs::reconcile_workspaces(handle).await;
            if let Err(error) = handle.agents().close_orphan_sessions().await {
                tracing::error!(project = %row.slug, code = code_of(&error), "could not end orphan sessions");
            }
        }
        let slot = match &outcome {
            Ok(handle) => Slot::Open(handle.clone()),
            Err(_) => Slot::Unavailable,
        };
        unpoisoned(self.slots.write()).insert(row.slug.clone(), slot);
        outcome.map(|handle| (handle, true))
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

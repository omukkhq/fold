use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use fold_core::{Clock, FoldError, ProjectConfig, ProjectPaths, codes};
use fold_store::Store;
use fold_store::projects::ProjectRow;
use fold_vcs::platform::Capabilities;

use crate::jobs;
use crate::live::{LiveConfig, LiveState};
use crate::push::{self, PushTargets, PushTask};
use crate::services::agents::{AgentService, SeenPresence};
use crate::services::events::EventBus;
use crate::services::projects::load_config;
use crate::services::tasks::TaskService;
use crate::services::workspaces::WorkspaceLocks;

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
    /// Reads the claim lease from the project's configuration on each use, so every transport applies the same lease.
    pub tasks: TaskService,
    /// Held for the whole integration procedure.
    pub integration: tokio::sync::Mutex<()>,
    /// Held for a whole garbage collection run, so a second run is refused instead of queued.
    pub gc: tokio::sync::Mutex<()>,
    /// Keeps the sweep away from objects that are about to be named. A reused object must not be deleted between its refresh and the row that names it, and the sweep cannot see a row that is not committed. So every path that writes or refreshes objects and then records a row naming them holds this shared from its first object write until that transaction commits, and writes or refreshes each object less than the grace period before the row names it. The sweep holds it exclusive while it lists, checks and deletes objects. It is an `Arc` so a guard can move into blocking work and outlive an aborted caller.
    pub objects: Arc<tokio::sync::RwLock<()>>,
    /// Held for a whole push, so two pushes of one project never run together. It is an `Arc` so the guard can move into the blocking Git work and outlive an aborted caller.
    pub remote_lock: Arc<tokio::sync::Mutex<()>>,
    /// One restore, sync or record per workspace at a time, across every session of the project.
    pub workspace_locks: Arc<WorkspaceLocks>,
    /// What the file system under `.fold/` supports, probed by the first workspace command of any session or the first live attach.
    pub capabilities: Arc<OnceLock<Capabilities>>,
    /// The in-memory state of the project's active workspaces.
    pub live: LiveState,
    /// The Claude Code inbox sockets of the project's agents. They live in memory only, so a restarted daemon has none until the session-start hooks run again.
    pub push: PushTargets,
    _push_task: PushTask,
}

impl std::fmt::Debug for ProjectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectHandle")
            .field("slug", &self.slug)
            .finish_non_exhaustive()
    }
}

impl ProjectHandle {
    /// Opens the project database, which applies its pending migrations. A missing database is not created: the registry only lists projects that `fold init` set up. The live state takes its intervals from the configuration read here; an invalid configuration gives it the defaults, and the commands that read the configuration report the error.
    pub async fn open(row: &ProjectRow, clock: Arc<dyn Clock>) -> Result<Self, FoldError> {
        let paths = ProjectPaths::new(&row.root);
        let present = tokio::fs::try_exists(paths.db()).await.unwrap_or(false);
        if !present {
            tracing::warn!(project = %row.slug, "project database is missing");
            return Err(unavailable(&row.slug));
        }
        let config = load_config(&paths).await.unwrap_or_else(|error| {
            tracing::warn!(project = %row.slug, code = error.code().unwrap_or("store_error"), "project configuration is invalid; the live state uses the defaults");
            ProjectConfig::default()
        });
        let store = Store::open_project(&paths).await.map_err(|error| {
            tracing::warn!(project = %row.slug, code = error.code().unwrap_or("store_error"), %error, "project failed to open");
            unavailable(&row.slug)
        })?;
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
        let tasks = TaskService::new(
            store.clone(),
            bus.clone(),
            clock.clone(),
            ProjectConfig::default().lease_minutes,
        )
        .with_project_config(paths.clone())
        .with_live(live.clone())
        .with_workspace_locks(workspace_locks.clone());
        let push = PushTargets::new();
        let push_task = push::run(store.clone(), &bus, push.clone());
        Ok(Self {
            slug: row.slug.clone(),
            name: row.name.clone(),
            paths,
            store,
            clock,
            bus,
            presence: SeenPresence::new(),
            tasks,
            integration: tokio::sync::Mutex::new(()),
            gc: tokio::sync::Mutex::new(()),
            objects: Arc::new(tokio::sync::RwLock::new(())),
            remote_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_locks,
            capabilities,
            live,
            push,
            _push_task: push_task,
        })
    }

    /// The agent service of this project: its store, bus, clock, configured lease, seen presence and push targets.
    pub fn agents(&self) -> AgentService {
        AgentService::new(self.store.clone(), self.bus.clone(), self.clock.clone())
            .with_project_config(self.paths.clone())
            .with_seen_presence(self.presence.clone())
            .with_push_targets(self.push.clone())
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
}

impl Projects {
    pub fn slot(&self, slug: &str) -> Option<Slot> {
        self.slots
            .read()
            .ok()
            .and_then(|slots| slots.get(slug).cloned())
    }

    pub fn handles(&self) -> Vec<Arc<ProjectHandle>> {
        let Ok(slots) = self.slots.read() else {
            return Vec::new();
        };
        slots
            .values()
            .filter_map(|slot| match slot {
                Slot::Open(handle) => Some(handle.clone()),
                Slot::Unavailable => None,
            })
            .collect()
    }

    /// Opens `row` unless it is already open, in which case the same handle comes back. A new handle has its workspaces reconciled with their directories before any request can reach it.
    pub async fn open(
        &self,
        row: &ProjectRow,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<ProjectHandle>, FoldError> {
        let _opening = self.opening.lock().await;
        if let Some(Slot::Open(handle)) = self.slot(&row.slug) {
            return Ok(handle);
        }
        let outcome = ProjectHandle::open(row, clock).await.map(Arc::new);
        if let Ok(handle) = &outcome {
            jobs::reconcile_workspaces(handle).await;
        }
        let slot = match &outcome {
            Ok(handle) => Slot::Open(handle.clone()),
            Err(_) => Slot::Unavailable,
        };
        if let Ok(mut slots) = self.slots.write() {
            slots.insert(row.slug.clone(), slot);
        }
        outcome
    }
}

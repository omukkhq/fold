use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::tree::{ChangeKind as FileChangeKind, ChangeTally, FlatTree};
use fold_core::{
    CheckpointId, Clock, EventActor, FoldError, LiveEvent, NewEvent, ProjectConfig, ProjectPaths,
    ProposalId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId, codes,
};
use fold_store::Store;
use fold_store::workspaces::{self, Checkpoint, CheckpointKind, WorkspaceKind, WorkspaceState};
use fold_vcs::compare::{self as vcs, FileChange, LineCache, State};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::ScanOptions;
use fold_vcs::snapshot::{Warning, WarningReason};
use fold_vcs::watch::{
    DegradedReason, WatchBackend, WatchConfig, WatchEvent, WatchHub, WatchRefused,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use utoipa::ToSchema;

use crate::services::events::{EventBuffer, EventBus};
use crate::services::workspaces::{WorkspaceError, capabilities};
use crate::support::{blocking, lock, unpoisoned};

/// How many `(old hash, new hash)` line counts one workspace keeps.
const LINE_CACHE_PAIRS: usize = 4096;

/// The persisted summary columns of a workspace change at most this often.
pub const TOUCH_INTERVAL: Duration = Duration::from_secs(10);

/// How the live state of a project is fed. Built from the project's configuration; tests shorten the intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveConfig {
    pub watch: WatchConfig,
    /// Time on the project's clock between two full rescans of a workspace (`rescan_seconds`).
    pub rescan: Duration,
    /// A larger file keeps its base entry and gives a warning (`max_file_bytes`).
    pub max_file_bytes: u64,
}

impl LiveConfig {
    pub fn from_project(config: &ProjectConfig) -> Self {
        let defaults = WatchConfig::default();
        let debounce = Duration::from_millis(config.debounce_ms);
        Self {
            watch: WatchConfig {
                debounce,
                max_wait: defaults.max_wait.max(debounce),
                degraded_interval: defaults.degraded_interval,
            },
            rescan: Duration::from_secs(config.rescan_seconds),
            max_file_bytes: config.max_file_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    Watching,
    /// The platform refused a watch or has no native notifications, so the watcher scans on a timer.
    Degraded,
}

impl WatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Watching => "watching",
            Self::Degraded => "degraded",
        }
    }
}

/// One workspace's live state. `view` is what the event stream and the routes send; the rest stays in the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceLive {
    pub view: LiveWorkspace,
    pub watch: WatchMode,
    /// Files the scans could not read, sorted by path. Each keeps its base entry, so it never shows as deleted.
    pub warnings: Vec<Warning>,
    /// Time of the last recalculation.
    pub updated_at: Timestamp,
    /// The last scan found the directory gone. A missing workspace never stalls.
    pub missing: bool,
}

/// What an active workspace holds right now. These are working changes, never history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LiveWorkspace {
    #[schema(value_type = String, example = "ws-9")]
    pub workspace: WorkspaceId,
    #[schema(value_type = String, example = "T-12")]
    pub task: TaskId,
    /// The worker label, `agent` or `agent/worker`.
    #[schema(example = "frontend/ui")]
    pub worker: String,
    /// The revision the workspace is based on.
    #[schema(value_type = String, example = "r17")]
    pub base: RevisionId,
    pub files: Vec<ChangedFile>,
    /// A one-line change summary, such as `3 files: 2 modified, 1 added (+40 -5)`, or `no changes`.
    pub summary: String,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub last_activity_at: Timestamp,
    pub checkpoints: Vec<CheckpointSummary>,
    #[schema(value_type = Option<String>, required = true, example = "P-3")]
    pub proposal: Option<ProposalId>,
    /// The worker holds the task's implementation claim and nothing in the workspace changed for `stall_minutes`. Calls do not count, so a stalled worker can hold a fresh lease.
    pub stalled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChangedFile {
    /// Relative to the workspace root.
    pub path: String,
    /// The path before a rename; null for every other change.
    #[schema(required = true)]
    pub previous_path: Option<String>,
    pub change: ChangeKind,
    pub added_lines: u32,
    pub removed_lines: u32,
    /// A binary file is compared by hash only, so its line counts are 0.
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CheckpointSummary {
    #[schema(value_type = String, example = "ck-88")]
    pub id: CheckpointId,
    #[schema(required = true)]
    pub label: Option<String>,
    /// False for a checkpoint an agent recorded explicitly.
    pub automatic: bool,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
}

impl From<&Checkpoint> for CheckpointSummary {
    fn from(checkpoint: &Checkpoint) -> Self {
        Self {
            id: checkpoint.id,
            label: checkpoint.label.clone(),
            automatic: checkpoint.kind == CheckpointKind::Auto,
            created_at: checkpoint.created_at,
        }
    }
}

type StateMap = HashMap<WorkspaceId, Arc<WorkspaceLive>>;

/// Every attached workspace's state. A change takes `map` for writing and then `order` before it lets `map` go, and publishes holding only `order`: the bus carries each workspace's states in the order they were made, and a reader never waits for a state to serialise.
#[derive(Default)]
struct States {
    map: RwLock<StateMap>,
    order: Mutex<()>,
}

impl States {
    fn read(&self) -> RwLockReadGuard<'_, StateMap> {
        unpoisoned(self.map.read())
    }

    fn write(&self) -> RwLockWriteGuard<'_, StateMap> {
        unpoisoned(self.map.write())
    }

    /// Stores `next` under the write guard `map`, and publishes its view when `changed`.
    fn store(
        &self,
        mut map: RwLockWriteGuard<'_, StateMap>,
        bus: &EventBus,
        workspace: WorkspaceId,
        next: Arc<WorkspaceLive>,
        changed: bool,
    ) {
        if !changed {
            map.insert(workspace, next);
            return;
        }
        let order = lock(&self.order);
        map.insert(workspace, Arc::clone(&next));
        drop(map);
        publish(bus, &next.view);
        drop(order);
    }
}
type Routes = Mutex<HashMap<WorkspaceId, Route>>;

/// How many commands wait for a busy pipeline. A pipeline takes every waiting command in one round, so the queue fills only while a scan runs.
const QUEUE: usize = 256;

/// Where one workspace's commands go.
#[derive(Clone)]
struct Route {
    sender: mpsc::Sender<Command>,
    overflow: Arc<Overflow>,
}

/// What the watcher could not queue for a pipeline whose queue was full. Changed paths, rescans and a missing root are covered by a full scan; a degraded watch is not, so its reason is kept for the next round.
#[derive(Default)]
struct Overflow {
    full_scan: AtomicBool,
    degraded: Mutex<Option<DegradedReason>>,
}

impl Overflow {
    fn keep(&self, command: Command) {
        if let Command::Degraded(reason) = command {
            *lock(&self.degraded) = Some(reason);
        }
        self.full_scan.store(true, Ordering::Relaxed);
    }

    /// Adds what was kept to `round` and forgets it.
    fn take_into(&self, round: &mut Round) {
        if let Some(reason) = lock(&self.degraded).take() {
            round.degraded = Some(reason);
        }
        if self.full_scan.swap(false, Ordering::Relaxed) {
            round.full = true;
        }
    }
}
type Answer = oneshot::Sender<Result<Arc<WorkspaceLive>, FoldError>>;

/// The in-memory state of every attached workspace of one project, kept by one pipeline task per workspace that the watcher and the rescan tick feed. Nothing here is trusted across a restart: attaching scans the directory again. Nothing here writes into a workspace directory.
#[derive(Clone)]
pub struct LiveState {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for LiveState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveState")
            .field("config", &self.shared.config)
            .finish_non_exhaustive()
    }
}

struct Shared {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    paths: ProjectPaths,
    config: LiveConfig,
    states: Arc<States>,
    /// Where the watcher's sink delivers each workspace's events. The sink holds this map and not the state, so the state can drop while the watcher thread runs.
    routes: Arc<Routes>,
    pipelines: Mutex<HashMap<WorkspaceId, JoinHandle<()>>>,
    /// Started by the first attach, so a project nobody works in holds no watcher thread.
    hub: OnceLock<Arc<WatchHub>>,
    /// The backend the hub starts with; `None` takes the platform's native one.
    backend: Mutex<Option<Box<dyn WatchBackend>>>,
    /// The project's capabilities probe (`ProjectHandle::capabilities`), shared with its workspace services.
    caps: Arc<OnceLock<Capabilities>>,
    /// Attach and detach of the project run one at a time.
    lifecycle: tokio::sync::Mutex<()>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let pipelines = unpoisoned(self.pipelines.get_mut());
        for (_, pipeline) in pipelines.drain() {
            pipeline.abort();
        }
    }
}

impl Shared {
    fn hub(&self) -> Arc<WatchHub> {
        Arc::clone(self.hub.get_or_init(|| {
            let routes = Arc::clone(&self.routes);
            let sink = move |event: WatchEvent| route(&routes, event);
            let backend = lock(&self.backend).take();
            Arc::new(match backend {
                Some(backend) => WatchHub::start_with(Boxed(backend), self.config.watch, sink),
                None => WatchHub::start(self.config.watch, sink),
            })
        }))
    }

    fn sender(&self, workspace: WorkspaceId) -> Option<mpsc::Sender<Command>> {
        lock(&self.routes)
            .get(&workspace)
            .map(|route| route.sender.clone())
    }
}

/// Runs on the watcher thread, so it only hands the event on.
fn route(routes: &Routes, event: WatchEvent) {
    let (workspace, command) = match event {
        WatchEvent::Changed { workspace, paths } => (workspace, Command::Paths(paths)),
        WatchEvent::RescanRequested { workspace } => (workspace, Command::Rescan),
        WatchEvent::Degraded { workspace, reason } => (workspace, Command::Degraded(reason)),
        WatchEvent::RootMissing { workspace } => (workspace, Command::RootMissing),
    };
    let routes = lock(routes);
    if let Some(route) = routes.get(&workspace)
        && let Err(TrySendError::Full(command)) = route.sender.try_send(command)
    {
        route.overflow.keep(command);
    }
}

/// Lets a boxed backend start a hub, which takes a backend by value.
struct Boxed(Box<dyn WatchBackend>);

impl WatchBackend for Boxed {
    fn watch_dir(&mut self, dir: &Path) -> Result<(), WatchRefused> {
        self.0.watch_dir(dir)
    }

    fn unwatch_dir(&mut self, dir: &Path) {
        self.0.unwatch_dir(dir);
    }

    fn backslash_separates(&self) -> bool {
        self.0.backslash_separates()
    }
}

impl LiveState {
    /// `caps` is the project's capabilities probe, so the live state and the workspace services probe `.fold/` once between them.
    pub fn new(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        paths: ProjectPaths,
        config: LiveConfig,
        caps: Arc<OnceLock<Capabilities>>,
    ) -> Self {
        Self::build(store, bus, clock, paths, config, caps, None)
    }

    /// A live state whose watcher runs on `backend` instead of the platform's notifications.
    pub fn with_backend(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        paths: ProjectPaths,
        config: LiveConfig,
        caps: Arc<OnceLock<Capabilities>>,
        backend: impl WatchBackend + 'static,
    ) -> Self {
        Self::build(
            store,
            bus,
            clock,
            paths,
            config,
            caps,
            Some(Box::new(backend)),
        )
    }

    fn build(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        paths: ProjectPaths,
        config: LiveConfig,
        caps: Arc<OnceLock<Capabilities>>,
        backend: Option<Box<dyn WatchBackend>>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                store,
                bus,
                clock,
                paths,
                config,
                states: Arc::default(),
                routes: Arc::default(),
                pipelines: Mutex::default(),
                hub: OnceLock::new(),
                backend: Mutex::new(backend),
                caps,
                lifecycle: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Starts the watcher and the pipeline of an active task workspace and returns the state of its first scan. Called after the commit that created the workspace. Attaching an attached workspace only rescans it. A review checkout is refused: nothing in it is ever recorded, so it has no live state and the live job never reaches it. The main workspace is refused too.
    pub async fn attach(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Arc<WorkspaceLive>, WorkspaceError> {
        {
            let _serial = self.shared.lifecycle.lock().await;
            if self.shared.sender(workspace).is_none() {
                self.start(workspace).await?;
            }
        }
        self.rescan(workspace).await
    }

    async fn start(&self, workspace: WorkspaceId) -> Result<(), WorkspaceError> {
        let shared = &self.shared;
        let read = shared.store.read();
        let row = workspaces::get_workspace(read, workspace)
            .await?
            .ok_or_else(|| {
                FoldError::new(
                    codes::WORKSPACE_NOT_FOUND,
                    format!("{workspace} does not exist"),
                )
            })?;
        if row.kind == WorkspaceKind::Review {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{workspace} is a review checkout"),
            )
            .with_details([("reason", json!("review_checkout"))])
            .into());
        }
        let Some(task) = row.task else {
            return Err(main_without_live(workspace).into());
        };
        if row.state != WorkspaceState::Active {
            return Err(FoldError::new(
                codes::WORKSPACE_NOT_ACTIVE,
                format!("{workspace} is {}", row.state.as_str()),
            )
            .into());
        }
        let base = workspaces::get_by_number(read, row.base).await?;
        let checkpoints = workspaces::list_checkpoints(read, workspace, false).await?;
        let root = shared.paths.root().join(&row.path);
        let last_activity = row.last_activity_at.unwrap_or(row.created_at);
        let seed = Seed {
            workspace,
            root: root.clone(),
            base_tree: base.tree_hash,
            max_file_bytes: shared.config.max_file_bytes,
        };
        let (caps, opened) = {
            let shared = Arc::clone(shared);
            let seed = seed.clone();
            blocking(move || {
                let caps = capabilities(&shared.caps, &shared.paths)?;
                Ok((caps, Scanner::open(&shared.paths, caps, &seed)?))
            })
            .await?
        };
        let rules = Arc::clone(&opened.rules);
        let (sender, commands) = mpsc::channel(QUEUE);
        let overflow = Arc::new(Overflow::default());
        lock(&shared.routes).insert(
            workspace,
            Route {
                sender,
                overflow: Arc::clone(&overflow),
            },
        );
        let watching = {
            let shared = Arc::clone(shared);
            tokio::task::spawn_blocking(move || shared.hub().add_workspace(workspace, &root, rules))
                .await
        };
        if watching.is_err() {
            tracing::warn!(workspace = %workspace, "the watcher did not take the workspace; the rescan tick still covers it");
        }
        let pipeline = Pipeline {
            workspace,
            task,
            store: shared.store.clone(),
            bus: shared.bus.clone(),
            clock: Arc::clone(&shared.clock),
            config: shared.config,
            states: Arc::clone(&shared.states),
            paths: shared.paths.clone(),
            caps,
            seed,
            scanner: Some(opened),
            rebased: None,
            initial: LiveWorkspace {
                workspace,
                task,
                worker: row
                    .worker
                    .map(|worker| worker.to_string())
                    .unwrap_or_default(),
                base: row.base,
                files: Vec::new(),
                summary: ChangeTally::default().summary(),
                last_activity_at: last_activity,
                checkpoints: checkpoints.iter().map(CheckpointSummary::from).collect(),
                proposal: row.proposal,
                stalled: false,
            },
            last_activity,
            last_full: shared.clock.now(),
            persisted: (row.changed_files, row.last_activity_at),
            touched_at: None,
            watch: WatchMode::Watching,
            missing: false,
        };
        let handle = tokio::spawn(pipeline.run(commands, overflow));
        lock(&shared.pipelines).insert(workspace, handle);
        Ok(())
    }

    /// Stops the watcher and the pipeline of a workspace and removes its state. Called after the commit that archived or abandoned it. A workspace that is not attached is left alone.
    pub async fn detach(&self, workspace: WorkspaceId) {
        let shared = &self.shared;
        let _serial = shared.lifecycle.lock().await;
        let sender = lock(&shared.routes).remove(&workspace);
        if let Some(route) = sender {
            let _ = route.sender.send(Command::Stop).await;
        }
        let pipeline = lock(&shared.pipelines).remove(&workspace);
        if let Some(pipeline) = pipeline {
            let _ = pipeline.await;
        }
        if let Some(hub) = shared.hub.get() {
            let hub = Arc::clone(hub);
            let _ = tokio::task::spawn_blocking(move || hub.remove_workspace(workspace)).await;
        }
        shared.states.write().remove(&workspace);
    }

    /// Scans the whole workspace now and returns the fresh state. Commands that record or replace working files call it first, so they never act on a state the watcher has not delivered yet.
    pub async fn rescan(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Arc<WorkspaceLive>, WorkspaceError> {
        self.ask(workspace, Command::Now).await
    }

    /// Compares the workspace against `tree`, the tree of `base`, from now on, and scans it in full. Called after the commit of a sync or restore, which move the base and write the working files.
    pub async fn set_base(
        &self,
        workspace: WorkspaceId,
        base: RevisionId,
        tree: ObjectHash,
    ) -> Result<Arc<WorkspaceLive>, WorkspaceError> {
        self.ask(workspace, |answer| Command::Rebase { base, tree, answer })
            .await
    }

    async fn ask(
        &self,
        workspace: WorkspaceId,
        command: impl FnOnce(Answer) -> Command,
    ) -> Result<Arc<WorkspaceLive>, WorkspaceError> {
        let (answer, answered) = oneshot::channel();
        let sent = match self.shared.sender(workspace) {
            Some(sender) => sender.send(command(answer)).await.is_ok(),
            None => false,
        };
        if !sent {
            return Err(not_attached(workspace).into());
        }
        match answered.await {
            Ok(result) => Ok(result?),
            Err(_) => Err(not_attached(workspace).into()),
        }
    }

    /// Replaces the checkpoints of the state after the commit that recorded or pruned one; `checkpoints` are the workspace's rows, and pruned ones are left out.
    pub fn set_checkpoints(&self, workspace: WorkspaceId, checkpoints: &[Checkpoint]) {
        let summaries: Vec<CheckpointSummary> = checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.pruned_at.is_none())
            .map(CheckpointSummary::from)
            .collect();
        self.update(workspace, |view| view.checkpoints = summaries);
    }

    /// Reads the checkpoints of an attached workspace from the store after a commit that recorded or pruned one.
    pub async fn reload_checkpoints(&self, workspace: WorkspaceId) {
        if self.get(workspace).is_none() {
            return;
        }
        match workspaces::list_checkpoints(self.shared.store.read(), workspace, false).await {
            Ok(checkpoints) => self.set_checkpoints(workspace, &checkpoints),
            Err(error) => {
                tracing::warn!(workspace = %workspace, code = error.log_code(), "the live checkpoints were not read");
            }
        }
    }

    /// Names the new owner after the commit that adopted the workspace. The pipeline keeps running: the directory, the base and the changes are the same.
    pub fn set_worker(&self, workspace: WorkspaceId, worker: &WorkerName) {
        self.update(workspace, |view| view.worker = worker.to_string());
    }

    /// Sets the open proposal after the commit that opened or closed it.
    pub fn set_proposal(&self, workspace: WorkspaceId, proposal: Option<ProposalId>) {
        self.update(workspace, |view| view.proposal = proposal);
    }

    /// Sets the flag the stall check decided from `last_activity_at` and returns whether it changed. A scan that found activity or a missing directory in the meantime wins: the flag is left as that scan set it.
    pub fn set_stalled(
        &self,
        workspace: WorkspaceId,
        last_activity_at: Timestamp,
        stalled: bool,
    ) -> bool {
        let shared = &self.shared;
        let map = shared.states.write();
        let Some(current) = map.get(&workspace) else {
            return false;
        };
        if current.missing
            || current.view.last_activity_at != last_activity_at
            || current.view.stalled == stalled
        {
            return false;
        }
        let mut next = WorkspaceLive::clone(current);
        next.view.stalled = stalled;
        next.updated_at = shared.clock.now();
        shared
            .states
            .store(map, &shared.bus, workspace, Arc::new(next), true);
        true
    }

    fn update(&self, workspace: WorkspaceId, change: impl FnOnce(&mut LiveWorkspace)) {
        let shared = &self.shared;
        amend(
            &shared.states,
            &shared.bus,
            workspace,
            shared.clock.now(),
            change,
        );
    }

    /// Lets every pipeline look at the clock: a workspace whose full rescan is due (`rescan_seconds` after the last) scans, and a held summary write goes out once its interval has passed. The live job calls it every [`crate::jobs::LIVE_TICK`]. Returns how many workspaces it reached.
    pub fn tick(&self) -> usize {
        let routes = lock(&self.shared.routes);
        routes
            .values()
            // A pipeline whose queue is full is busy scanning; the next tick reaches it.
            .filter(|route| route.sender.try_send(Command::Tick).is_ok())
            .count()
    }

    pub fn get(&self, workspace: WorkspaceId) -> Option<Arc<WorkspaceLive>> {
        self.shared.states.read().get(&workspace).cloned()
    }

    /// Every attached workspace that has finished its first scan, by id.
    pub fn all(&self) -> Vec<Arc<WorkspaceLive>> {
        let mut all: Vec<Arc<WorkspaceLive>> =
            self.shared.states.read().values().cloned().collect();
        all.sort_by_key(|state| state.view.workspace);
        all
    }
}

/// The main workspace has no live state: its change reaches main only after the user saw it in the conversation, so the stream and the board never show its working files.
fn main_without_live(workspace: WorkspaceId) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_NOT_ACTIVE,
        format!("{workspace} is the main workspace, which has no live state"),
    )
    .with_details([
        ("workspace", json!(workspace)),
        ("reason", json!("main_workspace")),
    ])
    .with_hint("compare the workspace with its base revision through compare_revisions")
}

fn not_attached(workspace: WorkspaceId) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_NOT_ACTIVE,
        format!("{workspace} has no live state"),
    )
    .with_details([("workspace", json!(workspace))])
}

/// Changes the view of an attached workspace that has finished its first scan, and publishes it when it moved.
fn amend(
    states: &States,
    bus: &EventBus,
    workspace: WorkspaceId,
    now: Timestamp,
    change: impl FnOnce(&mut LiveWorkspace),
) {
    let map = states.write();
    let Some(current) = map.get(&workspace) else {
        return;
    };
    let mut next = WorkspaceLive::clone(current);
    change(&mut next.view);
    if next.view == current.view {
        return;
    }
    next.updated_at = now;
    states.store(map, bus, workspace, Arc::new(next), true);
}

/// Sent while holding the states' order lock, so the bus carries a workspace's states in the order they were made.
fn publish(bus: &EventBus, view: &LiveWorkspace) {
    let event = serde_json::to_value(view).map_err(|_| ()).and_then(|data| {
        LiveEvent::new(types::WORKSPACE_CHANGED, Some(view.workspace), data).map_err(|_| ())
    });
    match event {
        Ok(event) => bus.publish_live(event),
        Err(()) => {
            tracing::error!(workspace = %view.workspace, "a live workspace state did not serialise");
        }
    }
}

enum Command {
    Paths(Vec<RepoPath>),
    Rescan,
    Degraded(DegradedReason),
    RootMissing,
    Now(Answer),
    Rebase {
        base: RevisionId,
        tree: ObjectHash,
        answer: Answer,
    },
    Tick,
    Stop,
}

/// Everything that queued up while the pipeline was busy, handled as one round with at most one scan.
#[derive(Default)]
struct Round {
    full: bool,
    paths: BTreeSet<RepoPath>,
    tick: bool,
    degraded: Option<DegradedReason>,
    missing: bool,
    /// The newest base a sync or restore moved the workspace to.
    rebase: Option<(RevisionId, ObjectHash)>,
    answers: Vec<Answer>,
    stop: bool,
}

impl Round {
    fn take(&mut self, command: Command) {
        match command {
            Command::Paths(paths) => self.paths.extend(paths),
            Command::Rescan => self.full = true,
            Command::Degraded(reason) => self.degraded = Some(reason),
            Command::RootMissing => self.missing = true,
            Command::Now(answer) => {
                self.full = true;
                self.answers.push(answer);
            }
            Command::Rebase { base, tree, answer } => {
                self.rebase = Some((base, tree));
                self.full = true;
                self.answers.push(answer);
            }
            Command::Tick => self.tick = true,
            Command::Stop => self.stop = true,
        }
    }
}

/// What a pipeline needs to open its scanner again.
#[derive(Debug, Clone)]
struct Seed {
    workspace: WorkspaceId,
    root: PathBuf,
    base_tree: ObjectHash,
    max_file_bytes: u64,
}

struct Pipeline {
    workspace: WorkspaceId,
    task: TaskId,
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    config: LiveConfig,
    states: Arc<States>,
    paths: ProjectPaths,
    caps: Capabilities,
    seed: Seed,
    /// Moved into each scan's blocking work and back.
    scanner: Option<Scanner>,
    /// A base tree the open scanner has not loaded yet.
    rebased: Option<ObjectHash>,
    /// The state before the first scan.
    initial: LiveWorkspace,
    last_activity: Timestamp,
    last_full: Timestamp,
    /// What the row's `changed_files` and `last_activity_at` hold.
    persisted: (u32, Option<Timestamp>),
    touched_at: Option<Timestamp>,
    watch: WatchMode,
    missing: bool,
}

/// The outcome of one scan.
enum Scan {
    /// The workspace directory is gone; the state stays as it was.
    Missing,
    Done(Scanned),
}

struct Scanned {
    files: Vec<FileChange>,
    /// The `(path, content hash)` pairs differ from the previous scan's.
    moved: bool,
    warnings: Vec<Warning>,
}

impl Pipeline {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>, overflow: Arc<Overflow>) {
        while let Some(first) = commands.recv().await {
            let mut round = Round::default();
            round.take(first);
            // A request that came in during the last scan is handled by this one scan, however many came.
            while let Ok(command) = commands.try_recv() {
                round.take(command);
            }
            overflow.take_into(&mut round);
            if round.stop {
                for answer in round.answers {
                    let _ = answer.send(Err(not_attached(self.workspace)));
                }
                // A summary the interval still holds back would be lost with the pipeline.
                self.flush_summary(self.clock.now(), true).await;
                break;
            }
            self.handle(round).await;
        }
    }

    async fn handle(&mut self, mut round: Round) {
        let now = self.clock.now();
        if let Some(reason) = round.degraded {
            self.degrade(reason, now).await;
        }
        if round.missing && !self.missing {
            self.missing = true;
            self.mark_missing(now);
            tracing::warn!(workspace = %self.workspace, "the workspace directory is gone; its live state stays as it was");
        }
        if let Some((base, tree)) = round.rebase {
            self.seed.base_tree = tree;
            self.rebased = Some(tree);
            self.initial.base = base;
        }
        let due = self
            .last_full
            .checked_add(self.config.rescan)
            .is_some_and(|due| now >= due);
        if round.tick && due {
            round.full = true;
        }
        if round.full || !round.paths.is_empty() {
            let touched = (!round.full).then(|| round.paths.into_iter().collect::<Vec<_>>());
            if round.full {
                self.last_full = now;
            }
            let outcome = self.recalculate(touched).await;
            if let Some((base, _)) = round.rebase
                && outcome.is_err()
            {
                amend(&self.states, &self.bus, self.workspace, now, |view| {
                    view.base = base;
                });
            }
            for answer in round.answers {
                let _ = answer.send(outcome.clone());
            }
        }
        self.flush_summary(self.clock.now(), false).await;
    }

    /// One scan, and one full scan more when a working file moved between the scan and the comparison.
    async fn recalculate(
        &mut self,
        touched: Option<Vec<RepoPath>>,
    ) -> Result<Arc<WorkspaceLive>, FoldError> {
        let scanned = match self.scan(touched).await {
            Err(error) if error.code() == codes::WORKSPACE_IO => self.scan(None).await,
            other => other,
        };
        match scanned {
            Ok(Scan::Done(scanned)) => {
                self.missing = false;
                Ok(self.apply(scanned))
            }
            Ok(Scan::Missing) => {
                if !self.missing {
                    self.missing = true;
                    self.mark_missing(self.clock.now());
                    tracing::warn!(workspace = %self.workspace, "the workspace directory is gone; its live state stays as it was");
                }
                Err(FoldError::new(
                    codes::WORKSPACE_NOT_ACTIVE,
                    format!("the directory of {} is gone", self.workspace),
                )
                .with_details([
                    ("workspace", json!(self.workspace)),
                    ("state", json!(WorkspaceState::Missing.as_str())),
                ]))
            }
            Err(error) => {
                tracing::warn!(workspace = %self.workspace, code = error.code(), "a live scan failed; the state stays as it was");
                Err(error)
            }
        }
    }

    async fn scan(&mut self, touched: Option<Vec<RepoPath>>) -> Result<Scan, FoldError> {
        let scanner = self.scanner.take();
        let rebased = self.rebased.take();
        let (paths, caps, seed) = (self.paths.clone(), self.caps, self.seed.clone());
        let done = tokio::task::spawn_blocking(move || {
            let mut scanner = match scanner {
                Some(mut scanner) => {
                    if let Some(tree) = rebased {
                        scanner.rebase(&tree)?;
                    }
                    scanner
                }
                None => Scanner::open(&paths, caps, &seed)?,
            };
            let outcome = scanner.scan(touched.as_deref());
            Ok::<_, FoldError>((scanner, outcome))
        })
        .await;
        match done {
            Ok(Ok((scanner, outcome))) => {
                self.scanner = Some(scanner);
                outcome
            }
            Ok(Err(error)) => Err(error),
            Err(error) => Err(FoldError::new(
                codes::INTERNAL_ERROR,
                format!("a live scan stopped: {error}"),
            )),
        }
    }

    fn apply(&mut self, scanned: Scanned) -> Arc<WorkspaceLive> {
        let now = self.clock.now();
        if scanned.moved {
            self.last_activity = now;
        }
        let mut tally = ChangeTally::default();
        for file in &scanned.files {
            // The summary counts the kinds the wire shows, so a mode change is a modification there too.
            let kind = match file.change {
                FileChangeKind::ModeChanged => FileChangeKind::Modified,
                kind => kind,
            };
            tally.add(kind, file.added_lines, file.removed_lines);
        }
        let files = scanned.files.iter().map(changed_file).collect();
        let map = self.states.write();
        let current = map.get(&self.workspace).cloned();
        let mut view = current
            .as_ref()
            .map_or_else(|| self.initial.clone(), |current| current.view.clone());
        view.base = self.initial.base;
        view.files = files;
        view.summary = tally.summary();
        view.last_activity_at = self.last_activity;
        if scanned.moved {
            view.stalled = false;
        }
        let warned = current
            .as_ref()
            .map_or(&[][..], |current| current.warnings.as_slice());
        if scanned.warnings != warned && !scanned.warnings.is_empty() {
            tracing::warn!(workspace = %self.workspace, files = scanned.warnings.len(), "files that could not be read keep their base entry");
        }
        let next = Arc::new(WorkspaceLive {
            view,
            watch: self.watch,
            warnings: scanned.warnings,
            updated_at: now,
            missing: false,
        });
        let changed = current
            .as_ref()
            .is_none_or(|current| current.view != next.view);
        self.states
            .store(map, &self.bus, self.workspace, Arc::clone(&next), changed);
        next
    }

    /// Marks the state missing and clears its stalled flag.
    fn mark_missing(&self, now: Timestamp) {
        let map = self.states.write();
        let Some(current) = map.get(&self.workspace) else {
            return;
        };
        let mut next = WorkspaceLive::clone(current);
        next.missing = true;
        next.view.stalled = false;
        let changed = next.view != current.view;
        if changed {
            next.updated_at = now;
        }
        self.states
            .store(map, &self.bus, self.workspace, Arc::new(next), changed);
    }

    async fn degrade(&mut self, reason: DegradedReason, now: Timestamp) {
        if self.watch == WatchMode::Degraded {
            return;
        }
        self.watch = WatchMode::Degraded;
        {
            let mut map = self.states.write();
            if let Some(current) = map.get(&self.workspace) {
                let mut next = WorkspaceLive::clone(current);
                next.watch = WatchMode::Degraded;
                map.insert(self.workspace, Arc::new(next));
            }
        }
        tracing::warn!(workspace = %self.workspace, reason = reason.as_str(), "the watch fell back to timer scans");
        let event = NewEvent::new(types::WORKSPACE_WATCH_DEGRADED, EventActor::fold())
            .map(|event| {
                event
                    .task(self.task)
                    .workspace(self.workspace)
                    .reason(reason.as_str())
            })
            .and_then(|event| event.data(json!({(DegradedReason::DATA_KEY): reason.as_str()})));
        let Ok(event) = event else {
            return;
        };
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| buffer.append(tx, &event, now).await.map(|_| ()))
            .await;
        match written {
            Ok(()) => self.bus.publish(buffer),
            Err(error) => {
                tracing::warn!(workspace = %self.workspace, code = error.log_code(), "the degraded watch was not recorded");
            }
        }
    }

    /// Mirrors the state into the row's summary columns, at most once per [`TOUCH_INTERVAL`] unless `at_stop`, which the pipeline sets as it stops. A change inside the interval is written by the first tick after it.
    async fn flush_summary(&mut self, now: Timestamp, at_stop: bool) {
        let Some(state) = self.states.read().get(&self.workspace).cloned() else {
            return;
        };
        let changed = u32::try_from(state.view.files.len()).unwrap_or(u32::MAX);
        let activity = state.view.last_activity_at;
        if self.persisted == (changed, Some(activity)) {
            return;
        }
        let held = self
            .touched_at
            .and_then(|at| at.checked_add(TOUCH_INTERVAL))
            .is_some_and(|due| now < due);
        if held && !at_stop {
            return;
        }
        let workspace = self.workspace;
        let written = self
            .store
            .write(async |tx| workspaces::touch(tx, workspace, changed, activity).await)
            .await;
        match written {
            Ok(_) => {
                self.persisted = (changed, Some(activity));
                self.touched_at = Some(now);
            }
            Err(error) => {
                tracing::warn!(workspace = %workspace, code = error.log_code(), "the workspace summary was not saved");
            }
        }
    }
}

fn changed_file(file: &FileChange) -> ChangedFile {
    // The wire has no mode change; it shows as a modification without line changes.
    let change = match file.change {
        FileChangeKind::Added => ChangeKind::Added,
        FileChangeKind::Modified | FileChangeKind::ModeChanged => ChangeKind::Modified,
        FileChangeKind::Deleted => ChangeKind::Deleted,
        FileChangeKind::Renamed => ChangeKind::Renamed,
    };
    ChangedFile {
        path: file.path.to_string(),
        previous_path: file.old_path.as_ref().map(ToString::to_string),
        change,
        added_lines: file.added_lines,
        removed_lines: file.removed_lines,
        binary: file.binary,
    }
}

/// The blocking half of a pipeline: the stat index, the entries of the last scan and the line counts, kept in memory between scans. It reads the workspace and the object store and writes neither; the stat index is never saved here, only its boundary is moved.
struct Scanner {
    root: PathBuf,
    boundary_dir: PathBuf,
    objects: ObjectStore,
    caps: Capabilities,
    rules: Arc<IgnoreRules>,
    index: StatIndex,
    base: State,
    current: FlatTree,
    scanned: bool,
    warnings: BTreeMap<RepoPath, WarningReason>,
    cache: LineCache,
    max_file_bytes: u64,
}

impl Scanner {
    /// A project whose database is gone, as after its `.fold/` was deleted, is refused, so the scan never makes `.fold/` again.
    fn open(paths: &ProjectPaths, caps: Capabilities, seed: &Seed) -> Result<Self, FoldError> {
        if !paths.db().is_file() {
            return Err(FoldError::new(
                codes::PROJECT_UNAVAILABLE,
                "the project database is missing",
            ));
        }
        let objects = ObjectStore::open_existing(&paths.fold_dir()).map_err(FoldError::from)?;
        let base = vcs::load_flat(&objects, &seed.base_tree)?;
        let index = StatIndex::load(&paths.workspace_index(seed.workspace), caps);
        let boundary_dir = paths.index();
        fold_vcs::platform::create_dir_below(&paths.fold_dir(), &boundary_dir)
            .map_err(|error| FoldError::from(fold_vcs::paths::WorkspaceError::from(error)))?;
        Ok(Self {
            root: seed.root.clone(),
            boundary_dir,
            objects,
            caps,
            rules: Arc::new(IgnoreRules::new(&seed.root)),
            index,
            base: State::recorded(base),
            current: FlatTree::new(),
            scanned: false,
            warnings: BTreeMap::new(),
            cache: LineCache::new(LINE_CACHE_PAIRS),
            max_file_bytes: seed.max_file_bytes,
        })
    }

    /// Compares against `tree` from the next scan on. The stat index and the line counts stay: both are keyed by content.
    fn rebase(&mut self, tree: &ObjectHash) -> Result<(), FoldError> {
        self.base = State::recorded(vcs::load_flat(&self.objects, tree)?);
        Ok(())
    }

    /// Scans `touched`, or the whole workspace when it is `None` or nothing was scanned yet, and compares the result to the base tree. A partial scan that fails is repeated as a full one.
    fn scan(&mut self, touched: Option<&[RepoPath]>) -> Result<Scan, FoldError> {
        if !self.root.is_dir() {
            return Ok(Scan::Missing);
        }
        let touched = touched.filter(|_| self.scanned);
        let before = self.scanned.then(|| self.current.clone());
        let opts = ScanOptions {
            rules: &self.rules,
            max_file_bytes: self.max_file_bytes,
            caps: self.caps,
            recorded: None,
        };
        let base = self.base.entries();
        let refreshed = match vcs::refresh_working(
            &self.root,
            &mut self.index,
            &mut self.current,
            base,
            touched,
            &opts,
        ) {
            Err(_) if touched.is_some() => vcs::refresh_working(
                &self.root,
                &mut self.index,
                &mut self.current,
                base,
                None,
                &opts,
            )?,
            other => other?,
        };
        // The racy-timestamp rule: a file changed in the same clock tick as this scan is hashed again next time.
        let _ = self.index.mark_boundary(&self.boundary_dir);
        match &refreshed.scope {
            None => self.warnings.clear(),
            Some(targets) => self
                .warnings
                .retain(|path, _| !targets.iter().any(|target| path.is_at_or_below(target))),
        }
        for skipped in refreshed.skipped {
            self.warnings
                .insert(skipped.path, WarningReason::Skipped(skipped.reason));
        }
        for path in refreshed.unstable {
            self.warnings.insert(path, WarningReason::Unstable);
        }
        let moved = before.is_some_and(|before| !same_content(&before, &self.current));
        self.scanned = true;
        let working = State::working(self.root.clone(), self.current.clone());
        let comparison = vcs::compare_cached(&self.objects, &self.base, &working, &mut self.cache)?;
        Ok(Scan::Done(Scanned {
            files: comparison.files,
            moved,
            warnings: self
                .warnings
                .iter()
                .map(|(path, reason)| Warning {
                    path: path.clone(),
                    reason: *reason,
                })
                .collect(),
        }))
    }
}

/// The same paths with the same content hashes; a kind change alone, such as a new executable bit, is not activity.
fn same_content(before: &FlatTree, after: &FlatTree) -> bool {
    before.len() == after.len()
        && before
            .iter()
            .zip(after)
            .all(|((a, a_entry), (b, b_entry))| a == b && a_entry.hash == b_entry.hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> RepoPath {
        RepoPath::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn the_config_follows_the_project_settings() {
        let config = ProjectConfig {
            debounce_ms: 5_000,
            rescan_seconds: 30,
            max_file_bytes: 99,
            ..ProjectConfig::default()
        };
        let live = LiveConfig::from_project(&config);
        assert_eq!(live.watch.debounce, Duration::from_secs(5));
        assert_eq!(live.watch.max_wait, Duration::from_secs(5));
        assert_eq!(live.rescan, Duration::from_secs(30));
        assert_eq!(live.max_file_bytes, 99);
        let defaults = LiveConfig::from_project(&ProjectConfig::default());
        assert_eq!(defaults.watch, WatchConfig::default());
        assert_eq!(WatchMode::Watching.as_str(), "watching");
        assert_eq!(WatchMode::Degraded.as_str(), "degraded");
    }

    #[test]
    fn a_full_queue_marks_the_route_for_a_full_scan_and_keeps_a_degraded_watch() {
        let (sender, mut commands) = mpsc::channel(1);
        let overflow = Arc::new(Overflow::default());
        let workspace: WorkspaceId = "ws-1".parse().unwrap();
        let routes: Routes = Mutex::new(HashMap::from([(
            workspace,
            Route {
                sender,
                overflow: Arc::clone(&overflow),
            },
        )]));
        route(
            &routes,
            WatchEvent::Changed {
                workspace,
                paths: vec![path("a.txt")],
            },
        );
        let mut round = Round::default();
        overflow.take_into(&mut round);
        assert!(!round.full && round.degraded.is_none());
        route(&routes, WatchEvent::RescanRequested { workspace });
        route(
            &routes,
            WatchEvent::Degraded {
                workspace,
                reason: DegradedReason::WatchLimit,
            },
        );
        assert!(matches!(commands.try_recv(), Ok(Command::Paths(_))));
        assert!(
            commands.try_recv().is_err(),
            "the overflowing commands were not queued"
        );
        let mut round = Round::default();
        overflow.take_into(&mut round);
        assert!(round.full, "the next round scans in full");
        assert_eq!(round.degraded, Some(DegradedReason::WatchLimit));
        let mut again = Round::default();
        overflow.take_into(&mut again);
        assert!(!again.full && again.degraded.is_none(), "taken once");
    }

    #[test]
    fn a_mode_change_shows_as_a_modification_on_the_wire() {
        let file = FileChange {
            path: path("run.sh"),
            old_path: None,
            change: FileChangeKind::ModeChanged,
            binary: false,
            old_hash: None,
            new_hash: None,
            added_lines: 0,
            removed_lines: 0,
        };
        assert_eq!(changed_file(&file).change, ChangeKind::Modified);
    }
}

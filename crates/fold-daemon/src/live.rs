use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::reviews::checks::Purpose;
use fold_core::stall::{self, Activity, CallKind, ProposalTurn, Slot, WorkspaceFact};
use fold_core::tree::{ChangeKind as FileChangeKind, ChangeTally, FlatTree};
use fold_core::{
    AgentName, CheckRunId, CheckpointId, Clock, EventActor, EventError, FoldError, LiveEvent,
    NewEvent, ProjectConfig, ProjectPaths, ProposalId, RevisionId, TaskId, Timestamp, WorkerName,
    WorkerState, WorkspaceId, codes,
};
use fold_store::workspaces::{self, Checkpoint, CheckpointKind, WorkspaceKind, WorkspaceState};
use fold_store::{Store, reviews};
use fold_vcs::compare::{self as vcs, FileChange, LineCache, State};
use fold_vcs::ignore::{IgnoreRules, TrackedSet};
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

use crate::services::agents::WorkerStreams;
use crate::services::events::{BusMessage, EventBuffer, EventBus};
use crate::services::workspaces::{WorkspaceError, capabilities};
use crate::support::{blocking, lock, unpoisoned};

/// How many `(old hash, new hash)` line counts one workspace keeps.
const LINE_CACHE_PAIRS: usize = 4096;

/// The persisted summary columns of a workspace change at most this often.
pub const TOUCH_INTERVAL: Duration = Duration::from_secs(10);

/// How many workspaces get their watch and first scan in one group when a project opens.
pub const DEFAULT_WATCH_GROUP: usize = 4;

/// Time on the project's clock between two groups of workspaces that get their watch when a project opens.
pub const DEFAULT_WATCH_GAP: Duration = Duration::from_secs(2);

/// How the live state of a project is fed. Built from the project's configuration; tests shorten the intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveConfig {
    pub watch: WatchConfig,
    /// Time on the project's clock between two full rescans of a workspace (`rescan_seconds`).
    pub rescan: Duration,
    /// A larger file keeps its base entry and gives a warning (`max_file_bytes`).
    pub max_file_bytes: u64,
    /// How many waiting workspaces one live tick gives their watch at project open; at least one.
    pub watch_group: usize,
    /// Time on the project's clock between two such groups.
    pub watch_gap: Duration,
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
                ..defaults
            },
            rescan: Duration::from_secs(config.rescan_seconds),
            max_file_bytes: config.max_file_bytes,
            watch_group: DEFAULT_WATCH_GROUP,
            watch_gap: DEFAULT_WATCH_GAP,
        }
    }
}

/// The function that starts a project's watch hub from its config and its sink.
type StartHub = dyn Fn(WatchConfig, Box<dyn Fn(WatchEvent) + Send>) -> WatchHub + Send + Sync;

/// How a project's live state starts its watcher. The default starts the hub on the platform's native notifications; another start can wrap that hub, such as to keep its notifier or to see the events it sends.
#[derive(Clone, Default)]
pub struct WatchStart(Option<Arc<StartHub>>);

impl WatchStart {
    pub fn custom(
        start: impl Fn(WatchConfig, Box<dyn Fn(WatchEvent) + Send>) -> WatchHub + Send + Sync + 'static,
    ) -> Self {
        Self(Some(Arc::new(start)))
    }

    fn start(&self, config: WatchConfig, sink: Box<dyn Fn(WatchEvent) + Send>) -> WatchHub {
        match &self.0 {
            Some(start) => start(config, sink),
            None => WatchHub::start(config, sink),
        }
    }
}

impl std::fmt::Debug for WatchStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "WatchStart::Custom"
        } else {
            "WatchStart::Native"
        })
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
    /// The workspace's task; null for the main workspace, which belongs to no task.
    #[schema(value_type = Option<String>, required = true, example = "T-12")]
    pub task: Option<TaskId>,
    /// The worker label, which is its agent's name.
    #[schema(example = "frontend")]
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
    /// The worker's stall needs a person: fold told the worker's lead, or else the main agent, and one more `stall_minutes` passed without a fix, or no agent can act on it. `activity` says what fold saw.
    pub stalled: bool,
    /// Whether the working files differ from the tree of the open proposal's newest version: false while every file is the same, true from the first file that differs. Null without an open proposal.
    #[schema(required = true)]
    pub edits_after_version: Option<bool>,
    /// The check run that goes in the workspace: the worker's `run_checks` or the gate of its `create_proposal` or `update_proposal` in a task workspace, or a reviewer's `review_fix` in a review checkout. The worker waits for its result, so it is not stalled while the run goes, and the stall time counts from the run's end at the earliest. Null while no run goes.
    #[schema(required = true)]
    pub check_run: Option<LiveCheckRun>,
    /// What fold saw of the worker that holds the implementation claim, from the output of the process fold supervises, or from the workspace alone for a session fold does not supervise. Null while it works in short steps, waits for others or for a person, or holds no claim.
    #[schema(required = true)]
    pub activity: Option<LiveActivity>,
}

/// What fold saw of a workspace's worker, as the stall rule decides it. fold keeps the times of the worker's output in memory, so after a daemon start it counts from that start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LiveActivity {
    pub state: ActivityState,
    /// The kind of the open call of `working` and `long_call`; null for the other states.
    #[schema(required = true)]
    pub call: Option<LiveCall>,
    /// The role whose free place a `waits_for_slot` worker waits for; null for the other states.
    #[schema(required = true)]
    pub slot: Option<LiveSlot>,
    /// UTC milliseconds: the start of the open call, the last activity of a silent worker, the end of a stopped or failed worker's process, the time the work came that fold owes a `waits_for_slot` worker, the end of the first turn of a wait for the network, a usage limit or a working login, or the time a `parked` worker was parked.
    #[schema(value_type = i64)]
    pub since: Timestamp,
    /// fold runs the worker's process, so `restart_worker` can start it again in a new session.
    pub supervised: bool,
    /// The agents fold told of the stall: the worker's lead, or else the main agents. Empty while the worker works, and for a stall no agent can act on, which needs a person at once.
    pub told: Vec<String>,
    /// The agent that parked a `parked` worker, whose go it waits for; null for the other states.
    #[schema(required = true)]
    pub parked_by: Option<String>,
    /// What a `parked` worker waits for, in the words of the agent that parked it; null for the other states.
    #[schema(required = true)]
    pub wait_for: Option<String>,
}

/// What the worker does: `working` while a call has been open for a minute or more, `silent` after `stall_minutes` with no call open and no output, `long_call` once a command has been open for twice Claude Code's ten-minute limit (or a subagent or another tool wrote nothing for that time), `stopped` once its process ended because a person, an agent or fold asked for the stop, `failed` once its process ended without anyone asking, by a crash or a failure, and without a report, `waits_for_slot` while fold owes it a resume that the worker limits hold, which fold starts once a place is free, `waits_for_network` while its last turn could not reach the model, which fold runs again once the model answers, `waits_for_usage_limit` while its last turn reached a usage limit, which fold runs again after the reset, `waits_for_login` while the API refused the login of its last turn, which fold runs again once the project's login changes or works, and `parked` while it waits for the go of the agent that stopped it with `stop_worker`'s `wait_for`. `working`, `stopped`, `waits_for_slot`, `parked` and the waits are never stalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Working,
    Silent,
    LongCall,
    Stopped,
    Failed,
    WaitsForSlot,
    WaitsForNetwork,
    WaitsForUsageLimit,
    WaitsForLogin,
    Parked,
}

/// The role of a place under the worker limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LiveSlot {
    TaskWorker,
    Reviewer,
}

impl From<Slot> for LiveSlot {
    fn from(slot: Slot) -> Self {
        match slot {
            Slot::TaskWorker => Self::TaskWorker,
            Slot::Reviewer => Self::Reviewer,
        }
    }
}

/// The kind of an open call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LiveCall {
    Command,
    Fold,
    Subagent,
    Question,
    Tool,
}

impl From<CallKind> for LiveCall {
    fn from(kind: CallKind) -> Self {
        match kind {
            CallKind::Command => Self::Command,
            CallKind::Fold => Self::Fold,
            CallKind::Subagent => Self::Subagent,
            CallKind::Question => Self::Question,
            CallKind::Tool => Self::Tool,
        }
    }
}

impl LiveActivity {
    /// The view of `activity`; `None` for [`Activity::Quiet`].
    pub fn of(activity: Activity, supervised: bool, told: &[AgentName]) -> Option<Self> {
        let mut slot = None;
        let (state, call, since) = match activity {
            Activity::Quiet => return None,
            Activity::Working { call, since } => (ActivityState::Working, Some(call), since),
            Activity::Silent { since } => (ActivityState::Silent, None, since),
            Activity::LongCall { call, since } => (ActivityState::LongCall, Some(call), since),
            Activity::Stopped { since } => (ActivityState::Stopped, None, since),
            Activity::Failed { since } => (ActivityState::Failed, None, since),
            Activity::WaitsForSlot { slot: waits, since } => {
                slot = Some(LiveSlot::from(waits));
                (ActivityState::WaitsForSlot, None, since)
            }
            Activity::WaitsForNetwork { since } => (ActivityState::WaitsForNetwork, None, since),
            Activity::WaitsForUsageLimit { since } => {
                (ActivityState::WaitsForUsageLimit, None, since)
            }
            Activity::WaitsForLogin { since } => (ActivityState::WaitsForLogin, None, since),
            Activity::Parked { since } => (ActivityState::Parked, None, since),
        };
        Some(Self {
            state,
            call: call.map(LiveCall::from),
            slot,
            since,
            supervised,
            told: told.iter().map(ToString::to_string).collect(),
            parked_by: None,
            wait_for: None,
        })
    }

    /// The view with the park of a `parked` worker: the agent that parked it and what it waits for. Any other state keeps both null.
    #[must_use]
    pub fn with_park(mut self, park: Option<(&AgentName, &str)>) -> Self {
        if self.state == ActivityState::Parked
            && let Some((by, wait_for)) = park
        {
            self.parked_by = Some(by.to_string());
            self.wait_for = Some(wait_for.to_owned());
        }
        self
    }

    /// What fold saw, in the words of every view; see [`fold_core::stall::activity_text_by`].
    pub fn text(&self, worker: &str, now: Timestamp) -> Option<String> {
        fold_core::stall::activity_text_by(self.activity(), worker, self.parked_by.as_deref(), now)
    }

    /// The rule's activity this view shows.
    pub fn activity(&self) -> Activity {
        let call = match self.call {
            Some(LiveCall::Command) => CallKind::Command,
            Some(LiveCall::Fold) => CallKind::Fold,
            Some(LiveCall::Subagent) => CallKind::Subagent,
            Some(LiveCall::Question) => CallKind::Question,
            Some(LiveCall::Tool) | None => CallKind::Tool,
        };
        let since = self.since;
        match self.state {
            ActivityState::Working => Activity::Working { call, since },
            ActivityState::Silent => Activity::Silent { since },
            ActivityState::LongCall => Activity::LongCall { call, since },
            ActivityState::Stopped => Activity::Stopped { since },
            ActivityState::Failed => Activity::Failed { since },
            ActivityState::WaitsForSlot => Activity::WaitsForSlot {
                slot: match self.slot {
                    Some(LiveSlot::Reviewer) => Slot::Reviewer,
                    Some(LiveSlot::TaskWorker) | None => Slot::TaskWorker,
                },
                since,
            },
            ActivityState::WaitsForNetwork => Activity::WaitsForNetwork { since },
            ActivityState::WaitsForUsageLimit => Activity::WaitsForUsageLimit { since },
            ActivityState::WaitsForLogin => Activity::WaitsForLogin { since },
            ActivityState::Parked => Activity::Parked { since },
        }
    }

    /// A stall rather than work, a stop someone asked for, a wait for a free place or a wait for a lead's go.
    pub fn is_stall(&self) -> bool {
        self.activity().is_stall()
    }
}

/// The agents fold told of a workspace's stall, and when; see [`LiveState::told`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// The stall's reason and start, so a new stall of the same workspace is told again.
    pub reason: &'static str,
    pub since: Timestamp,
    pub at: Timestamp,
    pub agents: Vec<AgentName>,
}

/// What asked for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckPurpose {
    Check,
    CreateProposal,
    UpdateProposal,
    Integrate,
    ReviewFix,
}

impl From<Purpose> for CheckPurpose {
    fn from(purpose: Purpose) -> Self {
        match purpose {
            Purpose::Check => Self::Check,
            Purpose::CreateProposal => Self::CreateProposal,
            Purpose::UpdateProposal => Self::UpdateProposal,
            Purpose::Integrate => Self::Integrate,
            Purpose::ReviewFix => Self::ReviewFix,
        }
    }
}

/// A check run that goes in a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LiveCheckRun {
    #[schema(value_type = String, example = "cr-4")]
    pub run: CheckRunId,
    pub purpose: CheckPurpose,
    /// The proposal whose next version the run makes when its checks pass; null for `check` and `create_proposal`.
    #[schema(value_type = Option<String>, required = true, example = "P-3")]
    pub proposal: Option<ProposalId>,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub started_at: Timestamp,
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

/// What the watcher could not queue for a pipeline whose queue was full. Changed paths, rescans and a missing root are covered by a full scan; a change of the watch mode is not, so it is kept for the next round.
#[derive(Default)]
struct Overflow {
    full_scan: AtomicBool,
    watch: Mutex<WatchChanges>,
}

impl Overflow {
    /// Queues `command`, or keeps it when the queue is full. While a watch change is kept, a later one is kept behind it and never queued, so every watch change in the queue is older than every kept one and a round applies the kept ones last. Only the watcher thread sends, so the kept changes stay in send order.
    fn send(&self, sender: &mpsc::Sender<Command>, command: Command) {
        let mut watch = lock(&self.watch);
        if *watch != WatchChanges::default() {
            match command {
                Command::Degraded(reason) => return watch.degraded(reason),
                Command::Restored => return watch.restored(),
                _ => {}
            }
        }
        if let Err(TrySendError::Full(command)) = sender.try_send(command) {
            Self::keep_watch(&mut watch, command);
            self.full_scan.store(true, Ordering::Relaxed);
        }
    }

    fn keep_watch(watch: &mut WatchChanges, command: Command) {
        match command {
            Command::Degraded(reason) => watch.degraded(reason),
            Command::Restored => watch.restored(),
            _ => {}
        }
    }

    #[cfg(test)]
    fn keep(&self, command: Command) {
        Self::keep_watch(&mut lock(&self.watch), command);
        self.full_scan.store(true, Ordering::Relaxed);
    }

    /// Adds what was kept to `round` and forgets it.
    fn take_into(&self, round: &mut Round) {
        let kept = std::mem::take(&mut *lock(&self.watch));
        if let Some(reason) = kept.degraded {
            round.watch.degraded(reason);
        }
        if kept.restored {
            round.watch.restored();
        }
        if self.full_scan.swap(false, Ordering::Relaxed) {
            round.full = true;
        }
    }
}

/// The watch mode changes of one round, applied as the degrade first and the restore after it. A restore that comes before a degrade is lost to it, so the round ends in the mode of its last change.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct WatchChanges {
    degraded: Option<DegradedReason>,
    restored: bool,
}

impl WatchChanges {
    fn degraded(&mut self, reason: DegradedReason) {
        self.degraded = Some(reason);
        self.restored = false;
    }

    fn restored(&mut self) {
        self.restored = true;
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
    /// How the hub starts when no backend is given.
    watch_start: OnceLock<WatchStart>,
    /// The workspaces whose pipeline runs but whose watch and first scan wait for their group.
    waiting: Mutex<Waiting>,
    /// The project's capabilities probe (`ProjectHandle::capabilities`), shared with its workspace services.
    caps: Arc<OnceLock<Capabilities>>,
    /// Attach and detach of the project run one at a time.
    lifecycle: tokio::sync::Mutex<()>,
    /// Stall decisions run one at a time, so a decision from older store facts never lands after a newer one.
    stalls: tokio::sync::Mutex<()>,
    /// The output of the project's supervised worker processes, which the supervisor sets once it runs.
    streams: OnceLock<Arc<WorkerStreams>>,
    /// When the live state was made, which is when the daemon opened the project: fold counts the silence of a process that wrote nothing since from here at the earliest.
    opened_at: Timestamp,
    /// Who was told of each workspace's stall, and when.
    told: Mutex<HashMap<WorkspaceId, Told>>,
    /// A stall decision ran since the daemon opened the project, so the stalled flags say what the stall rule decides; before it, they say nothing yet.
    stalls_decided: AtomicBool,
    /// The check runs that stop at the first edit of a tracked file of their workspace.
    edits: Arc<EditWatches>,
}

/// The senders of the runs that watch their workspace's edits, by workspace. A workspace has at most one running check run.
type EditWatches = Mutex<HashMap<WorkspaceId, mpsc::Sender<String>>>;

/// A check run's watch on the edits of its workspace: it receives the path of each scan that found a tracked file whose content changed, and stops watching when dropped.
#[derive(Debug)]
pub struct EditWatch {
    workspace: WorkspaceId,
    edits: Arc<EditWatches>,
    sender: mpsc::Sender<String>,
    receiver: mpsc::Receiver<String>,
}

impl EditWatch {
    /// The path of the next scan that found an edit. A scan that finds one while an earlier one waits gives nothing more, so the run reads the newest state when it reads the files.
    pub async fn next(&mut self) -> Option<String> {
        self.receiver.recv().await
    }
}

impl Drop for EditWatch {
    fn drop(&mut self) {
        let mut edits = lock(&self.edits);
        if edits
            .get(&self.workspace)
            .is_some_and(|sender| sender.same_channel(&self.sender))
        {
            edits.remove(&self.workspace);
        }
    }
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
                None => self
                    .watch_start
                    .get()
                    .cloned()
                    .unwrap_or_default()
                    .start(self.config.watch, Box::new(sink)),
            })
        }))
    }

    fn sender(&self, workspace: WorkspaceId) -> Option<mpsc::Sender<Command>> {
        lock(&self.routes)
            .get(&workspace)
            .map(|route| route.sender.clone())
    }
}

/// What the hub needs to watch a workspace, kept while the workspace waits for its group, holds its watch or rests on the timer scan, so its watch can come back.
#[derive(Clone)]
struct Queued {
    workspace: WorkspaceId,
    root: PathBuf,
    rules: Arc<IgnoreRules>,
    tracked: Arc<TrackedSet>,
    worker: Option<WorkerName>,
}

/// Where each attached workspace stands with the hub: waiting for its group in the order they were attached, watched, or resting on the timer scan because nobody works in it. A workspace is in one of the three at most.
#[derive(Default)]
struct Waiting {
    queue: VecDeque<Queued>,
    next_at: Option<Timestamp>,
    watched: HashMap<WorkspaceId, Queued>,
    resting: HashMap<WorkspaceId, Queued>,
}

impl Waiting {
    /// The next group when it is due at `now`; the group after it is due `gap` later.
    fn due(&mut self, now: Timestamp, group: usize, gap: Duration) -> Vec<Queued> {
        if self.queue.is_empty() || self.next_at.is_some_and(|at| now < at) {
            return Vec::new();
        }
        self.next_at = now.checked_add(gap);
        let size = group.clamp(1, self.queue.len());
        self.queue.drain(..size).collect()
    }

    fn take(&mut self, workspace: WorkspaceId) -> Option<Queued> {
        let at = self
            .queue
            .iter()
            .position(|queued| queued.workspace == workspace)?;
        self.queue.remove(at)
    }

    /// Takes the workspaces of `worker` that wait for their group or rest, the waiting ones first.
    fn of_worker(&mut self, worker: &WorkerName) -> Vec<Queued> {
        let (theirs, others): (VecDeque<Queued>, VecDeque<Queued>) =
            std::mem::take(&mut self.queue)
                .into_iter()
                .partition(|queued| queued.worker.as_ref() == Some(worker));
        self.queue = others;
        let mut theirs: Vec<Queued> = theirs.into();
        let resting: Vec<WorkspaceId> = self
            .resting
            .iter()
            .filter(|(_, queued)| queued.worker.as_ref() == Some(worker))
            .map(|(workspace, _)| *workspace)
            .collect();
        theirs.extend(
            resting
                .into_iter()
                .filter_map(|workspace| self.resting.remove(&workspace)),
        );
        theirs
    }

    fn has_worker(&self, worker: &WorkerName) -> bool {
        self.queue
            .iter()
            .chain(self.resting.values())
            .any(|queued| queued.worker.as_ref() == Some(worker))
    }

    fn forget(&mut self, workspace: WorkspaceId) {
        self.take(workspace);
        self.watched.remove(&workspace);
        self.resting.remove(&workspace);
    }

    fn set_worker(&mut self, workspace: WorkspaceId, worker: &WorkerName) {
        let found = self
            .queue
            .iter_mut()
            .find(|queued| queued.workspace == workspace)
            .or_else(|| self.watched.get_mut(&workspace))
            .or_else(|| self.resting.get_mut(&workspace));
        if let Some(queued) = found {
            queued.worker = Some(worker.clone());
        }
    }

    /// What [`LiveState::follow_owners`] changes: the watched workspaces whose owner is not active go to rest, and the resting ones whose owner is active get their watch back. A waiting workspace keeps its place in the queue, so the gradual start holds; its group decides its watch.
    fn moves(&self, active: &HashSet<AgentName>) -> (Vec<WorkspaceId>, Vec<WorkspaceId>) {
        let wanted = |queued: &Queued| {
            queued
                .worker
                .as_ref()
                .is_some_and(|worker| active.contains(&worker.agent()))
        };
        let rest = self
            .watched
            .values()
            .filter(|queued| !wanted(queued))
            .map(|queued| queued.workspace)
            .collect();
        let wake = self
            .resting
            .values()
            .filter(|queued| wanted(queued))
            .map(|queued| queued.workspace)
            .collect();
        (rest, wake)
    }
}

/// Whether a workspace holds a live watch, from what fold knows of its owner: the state of the owner's supervised worker process, `None` for an agent fold does not supervise, and whether the owner has an open session. A process that starts or works always gets the watch; an idle one, or an agent without a process, only while a session of it is open. A process that stops or fails ends its session with it, so its workspace rests on the timer scan from then on, as a parked worker's does.
pub fn watch_wanted(worker: Option<WorkerState>, session_open: bool) -> bool {
    session_open || matches!(worker, Some(WorkerState::Starting | WorkerState::Working))
}

/// The agents whose workspaces hold a watch now, as [`watch_wanted`] decides it from the store.
async fn active_owners(store: &Store) -> Result<HashSet<AgentName>, fold_store::StoreError> {
    let mut conn = store.read().acquire().await?;
    // An ended process never gives the watch by itself, so the running ones decide it with the sessions.
    let processes = fold_store::agents::running_worker_states(&mut *conn).await?;
    let sessions: HashSet<AgentName> = fold_store::agents::open_session_calls_by_agent(&mut *conn)
        .await?
        .into_iter()
        .map(|(agent, _)| agent)
        .collect();
    let states: HashMap<AgentName, WorkerState> = processes.into_iter().collect();
    Ok(states
        .keys()
        .chain(sessions.iter())
        .filter(|agent| watch_wanted(states.get(*agent).copied(), sessions.contains(*agent)))
        .cloned()
        .collect())
}

/// Whether the owner of a workspace that leaves the queue is in `active`; a failed read of the owners (`None`) gives the watch, and the next worker or session event looks again.
fn owner_active(active: Option<&HashSet<AgentName>>, queued: &Queued) -> bool {
    active.is_none_or(|active| {
        queued
            .worker
            .as_ref()
            .is_some_and(|worker| active.contains(&worker.agent()))
    })
}

/// The stored events after which an owner's worker process or session may have started or ended.
fn moves_owners(event: &fold_core::Event) -> bool {
    [
        types::WORKER_STARTED,
        types::WORKER_RESUMED,
        types::WORKER_REFRESHED,
        types::WORKER_STOPPED,
        types::WORKER_FAILED,
        types::WORKER_PARKED,
        types::SESSION_STARTED,
        types::SESSION_ENDED,
    ]
    .contains(&event.event_type)
}

/// Gives the project's workspaces their watch or their rest again after each commit that starts, resumes, stops, parks or ends a worker process or a session, through [`LiveState::follow_owners`]. It ends when the project closes.
pub async fn follow_watch_owners(
    live: LiveState,
    mut events: tokio::sync::broadcast::Receiver<BusMessage>,
) {
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};
    let mut closing = live.shared.bus.closing();
    loop {
        let due = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            received = events.recv() => match received {
                Ok(BusMessage::Durable(event)) => moves_owners(&event),
                Ok(BusMessage::Live(_)) => false,
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => return,
            },
        };
        if !due {
            continue;
        }
        // Everything queued so far was committed before it was sent, so one look at the store covers it.
        while !matches!(
            events.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        live.follow_owners().await;
    }
}

/// Whether a workspace that starts its pipeline gets its watch at once or waits for its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    Now,
    InGroup,
}

/// Runs on the watcher thread, so it only hands the event on.
fn route(routes: &Routes, event: WatchEvent) {
    let (workspace, command) = match event {
        WatchEvent::Changed { workspace, paths } => (workspace, Command::Paths(paths)),
        WatchEvent::RescanRequested { workspace } => (workspace, Command::Rescan),
        WatchEvent::Degraded { workspace, reason } => (workspace, Command::Degraded(reason)),
        WatchEvent::RootMissing { workspace } => (workspace, Command::RootMissing),
        WatchEvent::Restored { workspace } => (workspace, Command::Restored),
    };
    let routes = lock(routes);
    if let Some(route) = routes.get(&workspace) {
        route.overflow.send(&route.sender, command);
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
        let opened_at = clock.now();
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
                watch_start: OnceLock::new(),
                waiting: Mutex::default(),
                caps,
                lifecycle: tokio::sync::Mutex::new(()),
                stalls: tokio::sync::Mutex::new(()),
                streams: OnceLock::new(),
                opened_at,
                told: Mutex::default(),
                stalls_decided: AtomicBool::new(false),
                edits: Arc::default(),
            }),
        }
    }

    /// Starts the watcher and the pipeline of an active task workspace and returns the state of its first scan. Called after the commit that created the workspace. Attaching an attached workspace only rescans it. A review checkout is refused: nothing in it is ever recorded, so it has no live state and the live job never reaches it.
    pub async fn attach(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Arc<WorkspaceLive>, WorkspaceError> {
        {
            let _serial = self.shared.lifecycle.lock().await;
            if self.shared.sender(workspace).is_none() {
                self.start(workspace, Turn::Now).await?;
            }
        }
        self.rescan(workspace).await
    }

    /// Starts the hub with `start` instead of the platform's native notifications. Only a call before the first attach counts.
    pub fn start_watch_with(&self, start: WatchStart) {
        let _ = self.shared.watch_start.set(start);
    }

    /// Starts the pipeline of an active task workspace, or of the main workspace, as a project opens. Its watch and its first scan wait for their group in [`LiveState::tick`], so the workspaces of a project do not all walk their trees at the same moment; until then the rescan tick covers it. A read of the workspace, a command on it or a tool call of its worker gives it its watch at once. An attached workspace is left as it is.
    pub async fn attach_in_group(&self, workspace: WorkspaceId) -> Result<(), WorkspaceError> {
        let _serial = self.shared.lifecycle.lock().await;
        if self.shared.sender(workspace).is_none() {
            self.start(workspace, Turn::InGroup).await?;
        }
        Ok(())
    }

    /// Takes a workspace that waits for its group out of the queue now: it gets its watch when its owner is active, and rests on the timer scan otherwise. True when it waited.
    async fn watch_now(&self, workspace: WorkspaceId) -> bool {
        if !lock(&self.shared.waiting)
            .queue
            .iter()
            .any(|queued| queued.workspace == workspace)
        {
            return false;
        }
        let _serial = self.shared.lifecycle.lock().await;
        let Some(queued) = lock(&self.shared.waiting).take(workspace) else {
            return false;
        };
        // Read under the lifecycle lock, so a worker or session event committed before the read is in it, and one committed after it reaches the follower, which waits for this lock.
        let active = active_owners(&self.shared.store).await.ok();
        if owner_active(active.as_ref(), &queued) {
            self.watch(queued).await;
        } else {
            lock(&self.shared.waiting).resting.insert(workspace, queued);
        }
        true
    }

    /// Adds the watch of a workspace that left the queue, unless it was detached since. The caller holds the lifecycle lock.
    async fn watch(&self, queued: Queued) {
        if self.shared.sender(queued.workspace).is_none() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let workspace = queued.workspace;
        let kept = queued.clone();
        let added = tokio::task::spawn_blocking(move || {
            shared.hub().add_workspace(
                queued.workspace,
                &queued.root,
                queued.rules,
                queued.tracked,
            );
        })
        .await;
        match added {
            Ok(()) => {
                lock(&self.shared.waiting).watched.insert(workspace, kept);
            }
            Err(_) => {
                tracing::warn!(workspace = %workspace, "the watcher did not take the workspace; the rescan tick still covers it");
            }
        }
    }

    /// Takes the watch of a watched workspace away, so only the rescan tick and reads scan it, and asks for a full scan, which covers the changes whose notifications the hub dropped with the watch. The caller holds the lifecycle lock.
    async fn rest(&self, workspace: WorkspaceId) {
        let Some(queued) = lock(&self.shared.waiting).watched.remove(&workspace) else {
            return;
        };
        self.rest_queued(queued, true).await;
    }

    async fn rest_queued(&self, queued: Queued, watched: bool) {
        let workspace = queued.workspace;
        if self.shared.sender(workspace).is_none() {
            return;
        }
        if watched && let Some(hub) = self.shared.hub.get() {
            let hub = Arc::clone(hub);
            let _ = tokio::task::spawn_blocking(move || hub.remove_workspace(workspace)).await;
        }
        lock(&self.shared.waiting).resting.insert(workspace, queued);
        self.scan_soon(workspace).await;
    }

    /// Gives a resting workspace its watch back, ends a timer mode that an overflow left, since the watch is new, and asks for a full scan of what changed while it rested. The caller holds the lifecycle lock.
    async fn wake(&self, queued: Queued) {
        let workspace = queued.workspace;
        self.watch(queued).await;
        if let Some(sender) = self.shared.sender(workspace) {
            let _ = sender.send(Command::Restored).await;
            let _ = sender.send(Command::Rescan).await;
        }
    }

    /// Gives each workspace the watch or the rest that [`watch_wanted`] decides for its owner now: one look at the store, then the moves of [`Waiting::moves`]. A failed read changes nothing; the next worker or session event looks again.
    pub async fn follow_owners(&self) {
        let _serial = self.shared.lifecycle.lock().await;
        let active = match active_owners(&self.shared.store).await {
            Ok(active) => active,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "the owners of the workspaces were not read; their watches stay as they are"
                );
                return;
            }
        };
        let (rest, wake) = lock(&self.shared.waiting).moves(&active);
        for workspace in rest {
            self.rest(workspace).await;
        }
        for workspace in wake {
            let queued = lock(&self.shared.waiting).resting.remove(&workspace);
            if let Some(queued) = queued {
                self.wake(queued).await;
            }
        }
    }

    /// Whether the hub watches a directory of `workspace` now.
    pub async fn has_watch(&self, workspace: WorkspaceId) -> bool {
        let Some(hub) = self.shared.hub.get() else {
            return false;
        };
        let hub = Arc::clone(hub);
        tokio::task::spawn_blocking(move || !hub.watched_dirs(workspace).is_empty())
            .await
            .unwrap_or(false)
    }

    /// How many workspaces rest on the timer scan without a watch.
    pub fn resting(&self) -> usize {
        lock(&self.shared.waiting).resting.len()
    }

    /// Gives the waiting and resting workspaces of `worker` their watch and a scan now, because their worker acts, and returns after the scan.
    pub async fn watch_worker(&self, worker: &WorkerName) {
        if !lock(&self.shared.waiting).has_worker(worker) {
            return;
        }
        let watched = {
            let _serial = self.shared.lifecycle.lock().await;
            let theirs = lock(&self.shared.waiting).of_worker(worker);
            let mut watched = Vec::with_capacity(theirs.len());
            for queued in theirs {
                watched.push(queued.workspace);
                self.wake(queued).await;
            }
            watched
        };
        // The first scan finishes before the call returns, so what the worker reads next has the workspace's state.
        for workspace in watched {
            if let Err(error) = self.rescan(workspace).await {
                tracing::warn!(workspace = %workspace, code = error.log_code(), "the first scan after the watch failed; the rescan tick covers it");
            }
        }
    }

    /// Asks for a full scan without waiting for it.
    async fn scan_soon(&self, workspace: WorkspaceId) {
        if let Some(sender) = self.shared.sender(workspace) {
            let _ = sender.send(Command::Rescan).await;
        }
    }

    /// How many workspaces wait for their watch.
    pub fn waiting(&self) -> usize {
        lock(&self.shared.waiting).queue.len()
    }

    /// Takes the group that is due and gives it its watch and first scan in a task of its own, so the tick returns at once.
    fn watch_due_group(&self) {
        let config = self.shared.config;
        let group = lock(&self.shared.waiting).due(
            self.shared.clock.now(),
            config.watch_group,
            config.watch_gap,
        );
        if group.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            lock(&self.shared.waiting).queue.extend(group);
            return;
        };
        let live = self.clone();
        runtime.spawn(async move {
            let _serial = live.shared.lifecycle.lock().await;
            // Read under the lifecycle lock, so no owner event falls between the read and the moves.
            let active = active_owners(&live.shared.store).await.ok();
            for queued in group {
                let workspace = queued.workspace;
                if owner_active(active.as_ref(), &queued) {
                    live.watch(queued).await;
                    live.scan_soon(workspace).await;
                } else {
                    live.rest_queued(queued, false).await;
                }
            }
        });
    }

    async fn start(&self, workspace: WorkspaceId, turn: Turn) -> Result<(), WorkspaceError> {
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
        let task = row.task;
        if row.state != WorkspaceState::Active {
            return Err(FoldError::new(
                codes::WORKSPACE_NOT_ACTIVE,
                format!("{workspace} is {}", row.state.as_str()),
            )
            .into());
        }
        let base = workspaces::get_by_number(read, row.base).await?;
        let checkpoints = workspaces::list_checkpoints(read, workspace, false).await?;
        let proposal = match task {
            Some(task) => reviews::open_for_task(read, task).await?.or(row.proposal),
            None => row.proposal,
        };
        let version = match proposal {
            Some(proposal) => reviews::list_versions(read, proposal)
                .await?
                .pop()
                .map(|version| version.tree),
            None => None,
        };
        let root = shared.paths.root().join(&row.path);
        let last_activity = row.last_activity_at.unwrap_or(row.created_at);
        let seed = Seed {
            workspace,
            root: root.clone(),
            base_tree: base.tree_hash,
            version,
            max_file_bytes: shared.config.max_file_bytes,
            tracked: Arc::default(),
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
        let tracked = Arc::clone(&seed.tracked);
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
            let (root, rules, tracked) = (root.clone(), Arc::clone(&rules), Arc::clone(&tracked));
            tokio::task::spawn_blocking(move || {
                let hub = shared.hub();
                if turn == Turn::Now {
                    hub.add_workspace(workspace, &root, rules, tracked);
                }
                hub
            })
            .await
        };
        let queued = Queued {
            workspace,
            root,
            rules,
            tracked,
            worker: row.worker.clone(),
        };
        match turn {
            Turn::InGroup => lock(&shared.waiting).queue.push_back(queued),
            Turn::Now if watching.is_ok() => {
                lock(&shared.waiting).watched.insert(workspace, queued);
            }
            Turn::Now => {}
        }
        if watching.is_err() {
            tracing::warn!(workspace = %workspace, "the watcher did not take the workspace; the rescan tick still covers it");
        }
        let hub = watching.ok();
        let pipeline = Pipeline {
            workspace,
            task,
            store: shared.store.clone(),
            bus: shared.bus.clone(),
            clock: Arc::clone(&shared.clock),
            config: shared.config,
            states: Arc::clone(&shared.states),
            edits: Arc::clone(&shared.edits),
            paths: shared.paths.clone(),
            caps,
            seed,
            hub,
            scanner: Some(opened),
            rebased: None,
            reversioned: None,
            new_version: false,
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
                proposal,
                stalled: false,
                edits_after_version: None,
                check_run: None,
                activity: None,
            },
            last_activity,
            last_full: shared.clock.now(),
            persisted: (row.changed_files, row.last_activity_at),
            touched_at: None,
            watch: WatchMode::Watching,
            degraded: None,
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
        lock(&shared.waiting).forget(workspace);
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
        self.watch_now(workspace).await;
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
        lock(&self.shared.waiting).set_worker(workspace, worker);
        self.update(workspace, |view| view.worker = worker.to_string());
    }

    /// Sets the open proposal after the commit of a proposal command, and has the pipeline compare the working files against `tree`, the tree of the proposal's newest version, in a full scan it starts without the caller waiting for it; `None` stops the comparison. Until that scan `edits_after_version` is null, so no state pairs the proposal with a comparison against another tree.
    pub async fn set_proposal(
        &self,
        workspace: WorkspaceId,
        proposal: Option<ProposalId>,
        tree: Option<ObjectHash>,
    ) {
        self.update(workspace, |view| {
            view.proposal = proposal;
            view.edits_after_version = None;
        });
        self.watch_now(workspace).await;
        if let Some(sender) = self.shared.sender(workspace) {
            let (answer, _unread) = oneshot::channel();
            let _ = sender.send(Command::Version { tree, answer }).await;
        }
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

    /// Sets what fold saw of the workspace's worker and whether its stall needs a person, as the stall check decided them for the state whose last activity was `last_activity_at`; a scan that moved it since keeps its own. Returns whether either changed.
    pub fn set_activity(
        &self,
        workspace: WorkspaceId,
        last_activity_at: Timestamp,
        activity: Option<LiveActivity>,
        stalled: bool,
    ) -> bool {
        let shared = &self.shared;
        let map = shared.states.write();
        let Some(current) = map.get(&workspace) else {
            return false;
        };
        let stalled = stalled && !current.missing;
        if current.view.last_activity_at != last_activity_at
            || (current.view.activity == activity && current.view.stalled == stalled)
        {
            return false;
        }
        let mut next = WorkspaceLive::clone(current);
        next.view.activity = activity;
        next.view.stalled = stalled;
        next.updated_at = shared.clock.now();
        shared
            .states
            .store(map, &shared.bus, workspace, Arc::new(next), true);
        true
    }

    /// Lets the stall check read the output of the supervisor's worker processes.
    pub fn read_streams(&self, streams: Arc<WorkerStreams>) {
        let _ = self.shared.streams.set(streams);
    }

    /// The output of the supervised worker processes; `None` without a supervisor.
    pub fn streams(&self) -> Option<&WorkerStreams> {
        self.shared.streams.get().map(Arc::as_ref)
    }

    pub(crate) fn bus(&self) -> &EventBus {
        &self.shared.bus
    }

    /// The project's time now.
    pub fn now(&self) -> Timestamp {
        self.shared.clock.now()
    }

    /// When the daemon opened the project.
    pub fn opened_at(&self) -> Timestamp {
        self.shared.opened_at
    }

    /// Who was told of the stall of `workspace`, and when.
    pub fn told(&self, workspace: WorkspaceId) -> Option<Told> {
        lock(&self.shared.told).get(&workspace).cloned()
    }

    /// Records who was told of the stall of `workspace`, or forgets it with `None` once the stall ended. A workspace that left the live state keeps no record.
    pub fn set_told(&self, workspace: WorkspaceId, told: Option<Told>) {
        let present = told.is_some() && self.get(workspace).is_some();
        let mut all = lock(&self.shared.told);
        match told {
            Some(told) if present => {
                all.insert(workspace, told);
            }
            _ => {
                all.remove(&workspace);
            }
        }
    }

    /// Forgets who was told of the stalls of the workspaces that left the live state: every one but `live`.
    pub fn keep_told(&self, live: &HashSet<WorkspaceId>) {
        lock(&self.shared.told).retain(|workspace, _| live.contains(workspace));
    }

    /// Whether a stall decision ran since the daemon opened the project.
    pub fn stalls_decided(&self) -> bool {
        self.shared.stalls_decided.load(Ordering::SeqCst)
    }

    /// Records that a stall decision ran.
    pub fn mark_stalls_decided(&self) {
        self.shared.stalls_decided.store(true, Ordering::SeqCst);
    }

    /// Holds the project's turn for a stall decision until the guard drops.
    pub async fn stall_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.shared.stalls.lock().await
    }

    /// A watch on the edits of `workspace` from its next scan on, which replaces an earlier watch of it.
    pub fn watch_edits(&self, workspace: WorkspaceId) -> EditWatch {
        let (sender, receiver) = mpsc::channel(1);
        lock(&self.shared.edits).insert(workspace, sender.clone());
        EditWatch {
            workspace,
            edits: Arc::clone(&self.shared.edits),
            sender,
            receiver,
        }
    }

    /// Sets the check run that goes in the workspace, as the stall check read it from the store. A run that goes clears the stalled flag at once; returns whether it cleared one.
    pub fn set_check_run(&self, workspace: WorkspaceId, run: Option<LiveCheckRun>) -> bool {
        let shared = &self.shared;
        let map = shared.states.write();
        let Some(current) = map.get(&workspace) else {
            return false;
        };
        if current.view.check_run == run {
            return false;
        }
        let cleared = run.is_some() && current.view.stalled;
        let mut next = WorkspaceLive::clone(current);
        next.view.check_run = run;
        if run.is_some() {
            next.view.stalled = false;
            next.view.activity = next.view.activity.filter(|activity| !activity.is_stall());
        }
        next.updated_at = shared.clock.now();
        shared
            .states
            .store(map, &shared.bus, workspace, Arc::new(next), true);
        cleared
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

    /// Lets every pipeline look at the clock: a workspace whose full rescan is due (`rescan_seconds` after the last) scans, and a held summary write goes out once its interval has passed. The next group of the workspaces that wait for their watch gets it once `watch_gap` has passed since the last group. The live job calls it every [`crate::jobs::LIVE_TICK`]. Returns how many workspaces it reached.
    pub fn tick(&self) -> usize {
        self.watch_due_group();
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
    Restored,
    RootMissing,
    Now(Answer),
    Rebase {
        base: RevisionId,
        tree: ObjectHash,
        answer: Answer,
    },
    Version {
        tree: Option<ObjectHash>,
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
    watch: WatchChanges,
    missing: bool,
    /// The newest base a sync or restore moved the workspace to.
    rebase: Option<(RevisionId, ObjectHash)>,
    /// The newest version tree a proposal command gave; `Some(None)` stops the comparison.
    version: Option<Option<ObjectHash>>,
    answers: Vec<Answer>,
    stop: bool,
}

impl Round {
    fn take(&mut self, command: Command) {
        match command {
            Command::Paths(paths) => self.paths.extend(paths),
            Command::Rescan => self.full = true,
            Command::Degraded(reason) => self.watch.degraded(reason),
            Command::Restored => self.watch.restored(),
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
            Command::Version { tree, answer } => {
                self.version = Some(tree);
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
    /// The tree of the open proposal's newest version.
    version: Option<ObjectHash>,
    max_file_bytes: u64,
    /// The base tree's paths, which the scanner and the watcher keep although an ignore rule matches them.
    tracked: Arc<TrackedSet>,
}

struct Pipeline {
    workspace: WorkspaceId,
    task: Option<TaskId>,
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    config: LiveConfig,
    states: Arc<States>,
    edits: Arc<EditWatches>,
    paths: ProjectPaths,
    caps: Capabilities,
    seed: Seed,
    /// The watcher that took the workspace, which picks its watches again when the scanner's tracked set moves.
    hub: Option<Arc<WatchHub>>,
    /// Moved into each scan's blocking work and back.
    scanner: Option<Scanner>,
    /// A base tree the open scanner has not loaded yet.
    rebased: Option<ObjectHash>,
    /// A version tree the open scanner has not loaded yet; `Some(None)` stops the comparison.
    reversioned: Option<Option<ObjectHash>>,
    /// A proposal command gave a new version that the next applied scan has not seen yet.
    new_version: bool,
    /// The state before the first scan.
    initial: LiveWorkspace,
    last_activity: Timestamp,
    last_full: Timestamp,
    /// What the row's `changed_files` and `last_activity_at` hold.
    persisted: (u32, Option<Timestamp>),
    touched_at: Option<Timestamp>,
    watch: WatchMode,
    /// Why the watch is degraded; only [`DegradedReason::Overflow`] comes back with `Restored`.
    degraded: Option<DegradedReason>,
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
    /// The first path whose `(path, content hash)` pair differs from the previous scan's.
    moved: Option<String>,
    warnings: Vec<Warning>,
    edits_after_version: Option<bool>,
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
        if let Some(reason) = round.watch.degraded {
            self.degrade(reason, now).await;
        }
        if round.watch.restored {
            self.restore(now).await;
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
        if let Some(tree) = round.version {
            self.seed.version = tree;
            self.reversioned = Some(tree);
            self.new_version = tree.is_some();
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
                if let Some(path) = &scanned.moved
                    && let Some(sender) = lock(&self.edits).get(&self.workspace)
                {
                    let _ = sender.try_send(path.clone());
                }
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
        let reversioned = self.reversioned.take();
        let (paths, caps, seed) = (self.paths.clone(), self.caps, self.seed.clone());
        let hub = self.hub.clone();
        let done = tokio::task::spawn_blocking(move || {
            let tracked = seed.tracked.get();
            let mut scanner = match scanner {
                Some(mut scanner) => {
                    if let Some(tree) = rebased {
                        scanner.rebase(&tree)?;
                    }
                    if let Some(tree) = reversioned {
                        scanner.set_version(tree.as_ref());
                    }
                    scanner
                }
                None => Scanner::open(&paths, caps, &seed)?,
            };
            if let Some(hub) = &hub
                && !Arc::ptr_eq(&tracked, &seed.tracked.get())
            {
                hub.retrack(seed.workspace, tracked);
            }
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
        if scanned.moved.is_some() {
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
        view.edits_after_version = scanned.edits_after_version;
        // The scan after a new version clears the flag as the stall rule would. A proposal that came back to its author is the stall check's to decide, so only an edit clears the flag then.
        let turn = std::mem::take(&mut self.new_version).then_some(ProposalTurn::Others);
        // A failed process does no more work, so a file the watcher sees after its end, such as its last write, leaves the stall as it is. A version that others made still clears it, since the stall rule says nothing of a sent workspace.
        let ended = view
            .activity
            .as_ref()
            .is_some_and(|activity| activity.state == ActivityState::Failed);
        if (scanned.moved.is_some() && !ended)
            || stall::present_workspace(view.edits_after_version, turn) == WorkspaceFact::Sent
        {
            view.stalled = false;
            view.activity = view.activity.filter(|activity| !activity.is_stall());
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
        next.view.activity = None;
        let changed = next.view != current.view;
        if changed {
            next.updated_at = now;
        }
        self.states
            .store(map, &self.bus, self.workspace, Arc::new(next), changed);
    }

    async fn degrade(&mut self, reason: DegradedReason, now: Timestamp) {
        // A watch refused while the hub brings an overflowed workspace back keeps it in timer mode for the new reason, so only the newest reason decides a later restore.
        self.degraded = Some(reason);
        if self.watch == WatchMode::Degraded {
            return;
        }
        self.set_watch(WatchMode::Degraded);
        tracing::warn!(workspace = %self.workspace, reason = reason.as_str(), "the watch fell back to timer scans");
        let event = NewEvent::new(types::WORKSPACE_WATCH_DEGRADED, EventActor::fold())
            .map(|event| {
                event
                    .task(self.task)
                    .workspace(self.workspace)
                    .reason(reason.as_str())
            })
            .and_then(|event| event.data(json!({(DegradedReason::DATA_KEY): reason.as_str()})));
        self.record_watch(event, now).await;
    }

    /// The hub watches a workspace again only after the timer mode of [`DegradedReason::Overflow`]; the other reasons are limits of the platform that stay for the life of the watch.
    async fn restore(&mut self, now: Timestamp) {
        if self.watch != WatchMode::Degraded || self.degraded != Some(DegradedReason::Overflow) {
            return;
        }
        self.degraded = None;
        self.set_watch(WatchMode::Watching);
        tracing::info!(workspace = %self.workspace, "the watch is back after timer scans");
        let event = NewEvent::new(types::WORKSPACE_WATCH_RESTORED, EventActor::fold())
            .map(|event| event.task(self.task).workspace(self.workspace));
        self.record_watch(event, now).await;
    }

    fn set_watch(&mut self, watch: WatchMode) {
        self.watch = watch;
        let mut map = self.states.write();
        if let Some(current) = map.get(&self.workspace) {
            let mut next = WorkspaceLive::clone(current);
            next.watch = watch;
            map.insert(self.workspace, Arc::new(next));
        }
    }

    async fn record_watch(&self, event: Result<NewEvent, EventError>, now: Timestamp) {
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
                tracing::warn!(workspace = %self.workspace, code = error.log_code(), "the change of the watch mode was not recorded");
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
    /// Shared with the watcher; it holds the base's entries.
    tracked: Arc<TrackedSet>,
    index: StatIndex,
    base: State,
    current: FlatTree,
    version: Version,
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
        seed.tracked.set(base.clone());
        let version = Version::load(&objects, seed.version.as_ref());
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
            tracked: Arc::clone(&seed.tracked),
            index,
            base: State::recorded(base),
            current: FlatTree::new(),
            version,
            scanned: false,
            warnings: BTreeMap::new(),
            cache: LineCache::new(LINE_CACHE_PAIRS),
            max_file_bytes: seed.max_file_bytes,
        })
    }

    /// Compares against `tree` from the next scan on. The stat index and the line counts stay: both are keyed by content.
    fn rebase(&mut self, tree: &ObjectHash) -> Result<(), FoldError> {
        let base = vcs::load_flat(&self.objects, tree)?;
        self.tracked.set(base.clone());
        self.base = State::recorded(base);
        Ok(())
    }

    fn set_version(&mut self, tree: Option<&ObjectHash>) {
        self.version = Version::load(&self.objects, tree);
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
        let moved = before
            .and_then(|before| first_difference(&before, &self.current))
            .map(|path| String::from_utf8_lossy(&path).into_owned());
        self.scanned = true;
        let working = State::working(self.root.clone(), self.current.clone());
        let comparison = vcs::compare_cached(&self.objects, &self.base, &working, &mut self.cache)?;
        Ok(Scan::Done(Scanned {
            files: comparison.files,
            moved,
            edits_after_version: self.version.differs_from(&self.current),
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

/// The tree of the open proposal's newest version, as the scanner compares the working files against it.
#[derive(Debug)]
enum Version {
    None,
    Tree(FlatTree),
    /// The tree could not be read, so every scan counts as an edit after the version.
    Unreadable,
}

impl Version {
    fn load(objects: &ObjectStore, tree: Option<&ObjectHash>) -> Self {
        let Some(tree) = tree else {
            return Self::None;
        };
        match vcs::load_flat(objects, tree) {
            Ok(flat) => Self::Tree(flat),
            Err(error) => {
                tracing::warn!(
                    code = error.code(),
                    "the version tree was not read; the working files count as edited"
                );
                Self::Unreadable
            }
        }
    }

    /// Exact entries, kinds included: the files a recording of the workspace would put in the version.
    fn differs_from(&self, current: &FlatTree) -> Option<bool> {
        match self {
            Self::None => None,
            Self::Tree(flat) => Some(flat != current),
            Self::Unreadable => Some(true),
        }
    }
}

/// The first path, in path order, that one tree has and the other has not, or that has another content hash; `None` for the same paths with the same content hashes. A kind change alone, such as a new executable bit, is not activity.
fn first_difference(before: &FlatTree, after: &FlatTree) -> Option<Vec<u8>> {
    let mut before = before.iter().peekable();
    let mut after = after.iter().peekable();
    loop {
        match (before.peek(), after.peek()) {
            (None, None) => return None,
            (Some((path, _)), None) | (None, Some((path, _))) => return Some(path.to_vec()),
            (Some((a, a_entry)), Some((b, b_entry))) => match a.cmp(b) {
                std::cmp::Ordering::Less => return Some(a.to_vec()),
                std::cmp::Ordering::Greater => return Some(b.to_vec()),
                std::cmp::Ordering::Equal if a_entry.hash != b_entry.hash => {
                    return Some(a.to_vec());
                }
                std::cmp::Ordering::Equal => {
                    before.next();
                    after.next();
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::events::BusMessage;
    use fold_store::agents;
    use fold_store::testing::{TestProject, eventually};
    use fold_store::workspaces::NewMainWorkspace;
    use fold_vcs::revision_store::{write_initial_tree, write_revision};
    use tokio::sync::broadcast::Receiver;

    fn path(text: &str) -> RepoPath {
        RepoPath::parse(text.as_bytes()).unwrap()
    }

    fn flat(entries: &[(&str, u8, fold_core::tree::EntryKind)]) -> FlatTree {
        entries
            .iter()
            .map(|(path, seed, kind)| {
                let entry = fold_core::tree::FlatEntry {
                    kind: *kind,
                    hash: ObjectHash::from_bytes([*seed; 32]),
                };
                (path.as_bytes().to_vec(), entry)
            })
            .collect()
    }

    #[test]
    fn the_first_difference_is_the_first_path_in_order_that_moved() {
        use fold_core::tree::EntryKind::{Executable, File};
        let before = flat(&[("a", 1, File), ("c", 2, File), ("e", 3, File)]);
        assert_eq!(first_difference(&before, &before), None);
        let mode = flat(&[("a", 1, Executable), ("c", 2, File), ("e", 3, File)]);
        assert_eq!(first_difference(&before, &mode), None);
        let edited = flat(&[("a", 1, File), ("c", 9, File), ("e", 4, File)]);
        assert_eq!(first_difference(&before, &edited), Some(b"c".to_vec()));
        let added = flat(&[
            ("a", 1, File),
            ("b", 5, File),
            ("c", 2, File),
            ("e", 3, File),
        ]);
        assert_eq!(first_difference(&before, &added), Some(b"b".to_vec()));
        let removed = flat(&[("a", 1, File), ("e", 3, File)]);
        assert_eq!(first_difference(&before, &removed), Some(b"c".to_vec()));
        let last = flat(&[("a", 1, File), ("c", 2, File)]);
        assert_eq!(first_difference(&before, &last), Some(b"e".to_vec()));
        assert_eq!(first_difference(&last, &before), Some(b"e".to_vec()));
    }

    #[test]
    fn a_workspace_is_watched_while_its_owner_works_or_has_an_open_session() {
        let states = [
            None,
            Some(WorkerState::Starting),
            Some(WorkerState::Working),
            Some(WorkerState::Idle),
            Some(WorkerState::Stopping),
            Some(WorkerState::Stopped),
            Some(WorkerState::Failed),
        ];
        for state in states {
            assert!(watch_wanted(state, true), "{state:?} in an open session");
        }
        let alone: Vec<Option<WorkerState>> = states
            .into_iter()
            .filter(|state| watch_wanted(*state, false))
            .collect();
        assert_eq!(
            alone,
            [Some(WorkerState::Starting), Some(WorkerState::Working)]
        );
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
        assert!(!round.full && round.watch == WatchChanges::default());
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
        assert_eq!(round.watch.degraded, Some(DegradedReason::WatchLimit));
        let mut again = Round::default();
        overflow.take_into(&mut again);
        assert!(
            !again.full && again.watch == WatchChanges::default(),
            "taken once"
        );
    }

    const WAIT: Duration = Duration::from_secs(10);

    /// A watcher whose notifications never arrive, so the tests give the pipeline each watch event through [`route`].
    struct Silent;

    impl WatchBackend for Silent {
        fn watch_dir(&mut self, _dir: &Path) -> Result<(), WatchRefused> {
            Ok(())
        }

        fn unwatch_dir(&mut self, _dir: &Path) {}
    }

    /// A project whose live state has the attached main workspace `ws-1`, on an empty `r1`.
    async fn attached() -> (TestProject, LiveState, WorkspaceId) {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let tree = write_initial_tree(&objects, FlatTree::new()).unwrap();
        let revision = workspaces::initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        workspaces::create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let workspace: WorkspaceId = "ws-1".parse().unwrap();
        let now = project.now();
        project
            .store()
            .write(async |tx| {
                let name = AgentName::parse("main").unwrap();
                let agent = agents::insert_agent(tx, &name, now).await?;
                let worker =
                    agents::insert_worker(tx, agent, &WorkerName::main(&name), now).await?;
                let new = NewMainWorkspace {
                    id: workspace,
                    worker_id: worker,
                    path: "main-ws",
                    base: RevisionId::from_number(1).unwrap(),
                    now,
                };
                workspaces::insert_main_workspace(tx, &new).await
            })
            .await
            .unwrap();
        std::fs::create_dir_all(project.paths().root().join("main-ws")).unwrap();
        let live = LiveState::with_backend(
            project.store().clone(),
            EventBus::new(),
            Arc::new(project.clock().clone()),
            project.paths().clone(),
            LiveConfig::from_project(&ProjectConfig::default()),
            Arc::default(),
            Silent,
        );
        live.attach(workspace).await.unwrap();
        (project, live, workspace)
    }

    fn send(live: &LiveState, event: WatchEvent) {
        route(&live.shared.routes, event);
    }

    async fn wait_for_watch(live: &LiveState, workspace: WorkspaceId, watch: WatchMode) {
        eventually(WAIT, async || match live.get(workspace) {
            Some(state) if state.watch == watch => Ok(()),
            other => Err(other.map(|state| state.watch)),
        })
        .await;
    }

    /// The watch events of the bus so far, as their types and reasons.
    fn watch_events(bus: &mut Receiver<BusMessage>) -> Vec<(String, Option<String>)> {
        let mut seen = Vec::new();
        while let Ok(message) = bus.try_recv() {
            if let BusMessage::Durable(event) = message
                && event.event_type.as_str().starts_with("workspace.watch_")
            {
                assert_eq!(event.workspace, Some("ws-1".parse().unwrap()));
                seen.push((
                    event.event_type.as_str().to_owned(),
                    event.data[DegradedReason::DATA_KEY]
                        .as_str()
                        .map(str::to_owned),
                ));
            }
        }
        seen
    }

    /// A rescan queues behind the watch events sent before it, so once it answers the pipeline has handled them.
    async fn settle(live: &LiveState, workspace: WorkspaceId) {
        live.rescan(workspace).await.unwrap();
    }

    #[tokio::test]
    async fn restored_after_an_overflow_watches_the_workspace_again() {
        let (_project, live, workspace) = attached().await;
        let mut bus = live.bus().subscribe();
        send(
            &live,
            WatchEvent::Degraded {
                workspace,
                reason: DegradedReason::Overflow,
            },
        );
        wait_for_watch(&live, workspace, WatchMode::Degraded).await;
        send(&live, WatchEvent::Restored { workspace });
        wait_for_watch(&live, workspace, WatchMode::Watching).await;
        settle(&live, workspace).await;
        assert_eq!(
            watch_events(&mut bus),
            [
                (
                    "workspace.watch_degraded".to_owned(),
                    Some("overflow".to_owned())
                ),
                ("workspace.watch_restored".to_owned(), None),
            ]
        );
    }

    #[tokio::test]
    async fn restored_leaves_a_watch_degraded_for_another_reason() {
        let (_project, live, workspace) = attached().await;
        let mut bus = live.bus().subscribe();
        send(
            &live,
            WatchEvent::Degraded {
                workspace,
                reason: DegradedReason::WatchLimit,
            },
        );
        wait_for_watch(&live, workspace, WatchMode::Degraded).await;
        send(&live, WatchEvent::Restored { workspace });
        settle(&live, workspace).await;
        assert_eq!(live.get(workspace).unwrap().watch, WatchMode::Degraded);
        assert_eq!(
            watch_events(&mut bus),
            [(
                "workspace.watch_degraded".to_owned(),
                Some("watch_limit".to_owned())
            )]
        );
    }

    #[tokio::test]
    async fn a_newer_reason_keeps_an_overflowed_watch_degraded_after_restored() {
        let (_project, live, workspace) = attached().await;
        let mut bus = live.bus().subscribe();
        // Each event in its own round, as the hub sends them seconds apart.
        for reason in [DegradedReason::Overflow, DegradedReason::WatchLimit] {
            send(&live, WatchEvent::Degraded { workspace, reason });
            settle(&live, workspace).await;
        }
        send(&live, WatchEvent::Restored { workspace });
        settle(&live, workspace).await;
        assert_eq!(live.get(workspace).unwrap().watch, WatchMode::Degraded);
        assert_eq!(
            watch_events(&mut bus),
            [(
                "workspace.watch_degraded".to_owned(),
                Some("overflow".to_owned())
            )]
        );
    }

    #[tokio::test]
    async fn restored_for_a_watched_or_unknown_workspace_changes_nothing() {
        let (_project, live, workspace) = attached().await;
        let mut bus = live.bus().subscribe();
        send(
            &live,
            WatchEvent::Restored {
                workspace: "ws-99".parse().unwrap(),
            },
        );
        send(&live, WatchEvent::Restored { workspace });
        settle(&live, workspace).await;
        assert_eq!(live.get(workspace).unwrap().watch, WatchMode::Watching);
        assert!(live.get("ws-99".parse().unwrap()).is_none());
        assert_eq!(watch_events(&mut bus), []);
    }

    #[test]
    fn a_watch_change_after_a_kept_one_is_kept_behind_it() {
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
        route(&routes, WatchEvent::RescanRequested { workspace });
        route(
            &routes,
            WatchEvent::Degraded {
                workspace,
                reason: DegradedReason::Overflow,
            },
        );
        // The pipeline takes the queued command and frees the slot, as a round starts.
        let mut round = Round::default();
        round.take(commands.try_recv().unwrap());
        route(&routes, WatchEvent::Restored { workspace });
        route(&routes, WatchEvent::RescanRequested { workspace });
        assert!(matches!(commands.try_recv(), Ok(Command::Rescan)));
        assert!(
            commands.try_recv().is_err(),
            "the restore waits behind the kept degrade"
        );
        overflow.take_into(&mut round);
        assert_eq!(
            round.watch,
            WatchChanges {
                degraded: Some(DegradedReason::Overflow),
                restored: true
            }
        );
        route(&routes, WatchEvent::Restored { workspace });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Restored)),
            "with nothing kept a restore queues"
        );
    }

    #[test]
    fn a_restore_before_a_degrade_in_one_round_is_lost_to_it() {
        let mut round = Round::default();
        round.take(Command::Restored);
        round.take(Command::Degraded(DegradedReason::Overflow));
        assert_eq!(
            round.watch,
            WatchChanges {
                degraded: Some(DegradedReason::Overflow),
                restored: false
            }
        );
        round.take(Command::Restored);
        assert!(round.watch.restored, "a restore after the degrade stays");
        let overflow = Overflow::default();
        overflow.keep(Command::Degraded(DegradedReason::Overflow));
        overflow.keep(Command::Restored);
        let mut kept = Round::default();
        overflow.take_into(&mut kept);
        assert_eq!(
            kept.watch,
            WatchChanges {
                degraded: Some(DegradedReason::Overflow),
                restored: true
            }
        );
        assert!(kept.full);
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

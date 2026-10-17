use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fold_core::{AgentName, Clock, ProjectPaths};
use fold_store::Store;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::push::PushTargets;
use crate::services::events::EventBus;
use crate::support::lock;
use questions::Waiter;
use watch::watch_done;

mod launch;
mod lines;
mod questions;
mod refresh;
mod restart;
mod resume;
mod send_back;
mod stop;
mod watch;

pub use restart::{PAUSE_TURN, RESTART_TURN};
pub(super) use resume::StartWatch;

/// The programs a worker process runs. The daemon reads no environment variable, so a test points `claude` at a stand-in here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    /// Claude Code; `claude` is looked up on `PATH`.
    pub claude: PathBuf,
    /// The fld binary that the worker's MCP configuration starts with the argument `mcp`; `None` is the running daemon's own binary.
    pub fold: Option<PathBuf>,
    /// Extra environment for every worker process.
    pub env: Vec<(String, String)>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            claude: PathBuf::from("claude"),
            fold: None,
            env: Vec::new(),
        }
    }
}

/// How long each step of a stop waits for the process to exit: after the interrupt and the closed stdin, after SIGINT, and after SIGTERM, before SIGKILL. The deadlines are read from the project's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopTimeouts {
    pub interrupt: Duration,
    pub sigint: Duration,
    pub sigterm: Duration,
}

impl Default for StopTimeouts {
    fn default() -> Self {
        Self {
            interrupt: Duration::from_secs(10),
            sigint: Duration::from_secs(5),
            sigterm: Duration::from_secs(5),
        }
    }
}

enum Input {
    Line(String),
    Turn(String),
    Close,
}

#[derive(Clone)]
struct Running {
    generation: u64,
    input: mpsc::UnboundedSender<Input>,
    exited: tokio::sync::watch::Receiver<bool>,
    pid: Option<u32>,
    /// Set when the daemon stops or a lead is refreshed: the process ends but the row keeps its state, so a restarted daemon resumes the worker and a refresh starts it in a new session.
    detached: Arc<AtomicBool>,
    /// Set once the process is reaped, after which its pid may belong to another process and is never signalled.
    reaped: Arc<AtomicBool>,
}

/// The worker processes of one project, in memory, with the task that stops a short-lived worker once its work is done. Every process is spawned with `kill_on_drop`, so none outlives the handle.
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Supervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supervisor").finish_non_exhaustive()
    }
}

struct Inner {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    paths: ProjectPaths,
    push: PushTargets,
    launcher: Mutex<Launcher>,
    timeouts: Mutex<StopTimeouts>,
    running: Mutex<HashMap<AgentName, Running>>,
    generation: AtomicU64,
    requests: AtomicU64,
    watch: Mutex<Option<JoinHandle<()>>>,
    /// The leads whose fresh start is under way, so a note and a turn end that both ask for it start one session.
    refreshing: Mutex<HashSet<AgentName>>,
    /// Held while a coder is resumed, so two events that resume the same coder start it once.
    resuming: tokio::sync::Mutex<()>,
    /// The live `can_use_tool` requests that wait for each open question, by the question's row id.
    pending: Mutex<HashMap<i64, Vec<Waiter>>>,
    /// The answered questions whose request ended with its process, by agent: each answers the resumed worker's first repeat of the same call once.
    replays: Mutex<HashMap<AgentName, Vec<i64>>>,
    /// The host registry, whose humans receive a worker's `AskUserQuestion` call as a question message.
    registry: Mutex<Option<Store>>,
    /// The workers whose session was gone and whose fresh start a pause refused, with the starter's instructions; the resume of the pause starts each fresh.
    held_lost: Mutex<HashMap<AgentName, Option<String>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.watch).take() {
            task.abort();
        }
    }
}

/// What a row id and its agent are, for the tasks of one process.
#[derive(Clone)]
struct Process {
    id: i64,
    agent: AgentName,
    /// The row's cost before this process, to which the process's running total adds.
    base_cost: u64,
    generation: u64,
    /// The process runs with `--resume`.
    resumed: bool,
    /// Whether the process's session went on (`true`) or was not found (`false`); `None` until its first output tells.
    session: Arc<tokio::sync::watch::Sender<Option<bool>>>,
    /// The worker row's Claude Code session id.
    session_id: String,
}

impl Supervisor {
    /// Starts the task that watches the bus for finished work. It must run inside a tokio runtime.
    pub fn new(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        paths: ProjectPaths,
        push: PushTargets,
        launcher: Launcher,
    ) -> Self {
        let supervisor = Self {
            inner: Arc::new(Inner {
                store,
                bus,
                clock,
                paths,
                push,
                launcher: Mutex::new(launcher),
                timeouts: Mutex::new(StopTimeouts::default()),
                running: Mutex::new(HashMap::new()),
                generation: AtomicU64::new(0),
                requests: AtomicU64::new(0),
                watch: Mutex::new(None),
                refreshing: Mutex::new(HashSet::new()),
                resuming: tokio::sync::Mutex::new(()),
                pending: Mutex::new(HashMap::new()),
                replays: Mutex::new(HashMap::new()),
                registry: Mutex::new(None),
                held_lost: Mutex::new(HashMap::new()),
            }),
        };
        let receiver = supervisor.inner.bus.subscribe();
        let task = tokio::spawn(watch_done(Arc::downgrade(&supervisor.inner), receiver));
        *lock(&supervisor.inner.watch) = Some(task);
        supervisor
    }

    pub fn set_launcher(&self, launcher: Launcher) {
        *lock(&self.inner.launcher) = launcher;
    }

    pub fn set_registry(&self, registry: Store) {
        *lock(&self.inner.registry) = Some(registry);
    }

    pub fn set_stop_timeouts(&self, timeouts: StopTimeouts) {
        *lock(&self.inner.timeouts) = timeouts;
    }

    /// The process id of the agent's running process.
    pub fn pid(&self, agent: &AgentName) -> Option<u32> {
        lock(&self.inner.running).get(agent).and_then(|run| run.pid)
    }

    pub fn is_running(&self, agent: &AgentName) -> bool {
        lock(&self.inner.running).contains_key(agent)
    }

    pub(super) fn paths(&self) -> &ProjectPaths {
        &self.inner.paths
    }

    /// Drops the agent's entry in `running` when it is still the one of `process`.
    fn forget(&self, process: &Process) {
        let mut running = lock(&self.inner.running);
        if running
            .get(&process.agent)
            .is_some_and(|run| run.generation == process.generation)
        {
            running.remove(&process.agent);
            self.inner.push.clear_stdin(&process.agent);
        }
    }

    fn request_id(&self) -> String {
        format!(
            "fold-{}",
            self.inner.requests.fetch_add(1, Ordering::SeqCst) + 1
        )
    }
}

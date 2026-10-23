use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fold_core::transcript::claude_config_dir;
use fold_core::{AgentName, Clock, ProjectPaths, Role};
use fold_store::Store;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::push::PushTargets;
use crate::services::agents::claude::{LOCATION_ENV, env_value, transcript_path};
use crate::services::events::EventBus;
use crate::support::lock;
use questions::Waiter;
use watch::watch_done;

mod fresh;
mod gate;
mod launch;
mod lines;
mod main;
mod message;
mod questions;
mod refresh;
mod restart;
mod resume;
mod send_back;
mod stderr;
mod stop;
mod watch;

pub use restart::{MCP_RETRY_TURN, PAUSE_TURN, RESTART_TURN};

/// The folder below a project's `.fold/` that, in a build for development, makes every worker of the project run fld's scripted stand-in for Claude Code instead of `claude`. It holds one script per agent, `<agent>.json`; the development seed writes it.
#[cfg(feature = "dev-launcher")]
pub const DEV_STUB_DIR: &str = "dev-stub";

/// The fld subcommand of the stand-in, put before Claude Code's own arguments.
#[cfg(feature = "dev-launcher")]
pub const DEV_STUB_COMMAND: [&str; 2] = ["dev", "stub"];
pub(super) use resume::StartWatch;

/// The programs a worker process runs. The daemon reads no environment variable, so a test points `claude` at a stand-in here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    /// Claude Code: the absolute path that `fld serve` found on the `PATH` of the workers' environment, else `claude`, looked up on `PATH`.
    pub claude: PathBuf,
    /// The fld binary that the worker's MCP configuration starts with the argument `mcp`; `None` is the running daemon's own binary, which [`Launcher::with_own_binary`] reads once at the daemon's start.
    pub fold: Option<PathBuf>,
    /// Extra environment for every worker process.
    pub env: Vec<(String, String)>,
    /// The fingerprint of the role text that the fld which runs the daemon gives a session of each role (see [`fold_core::roles::session_fingerprint`]): the role's prompt ships in this crate, its skill only in the binary that writes the skill files. A worker records it with its session, and a resume under another one tells the worker to load its role skill again. A role without one is never told.
    pub fingerprints: BTreeMap<Role, String>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            claude: PathBuf::from("claude"),
            fold: None,
            env: Vec::new(),
            fingerprints: BTreeMap::new(),
        }
    }
}

impl Launcher {
    /// The launcher with `fold` set to the daemon's own binary when it names none. The path is read once, at the daemon's start: an installer that replaces fld while the daemon runs leaves the running binary's link as `<path> (deleted)`, and the path without that suffix names the newly installed fld.
    #[must_use]
    pub fn with_own_binary(mut self) -> Self {
        if self.fold.is_none() {
            self.fold = own_binary();
        }
        self
    }

    /// The launcher with the variables of [`LOCATION_ENV`] that `var` sets and its own environment leaves out. A worker inherits them from the daemon anyway, with the same values; the launcher holds them so that the daemon, which reads no environment variable, knows where its workers' Claude Code keeps their transcripts. A value that is not UTF-8 is left out.
    #[must_use]
    pub fn with_claude_location(mut self, var: impl Fn(&str) -> Option<OsString>) -> Self {
        for name in LOCATION_ENV {
            if self.env.iter().any(|(key, _)| key == name) {
                continue;
            }
            if let Some(value) = var(name).and_then(|value| value.into_string().ok()) {
                self.env.push((name.to_owned(), value));
            }
        }
        self
    }
}

/// The path of the running binary, without the suffix Linux gives the link of a binary whose file was replaced. Blocking.
pub(super) fn own_binary() -> Option<PathBuf> {
    std::env::current_exe().ok().map(installed_path)
}

/// `path` without a trailing ` (deleted)`, the suffix of the link to a running binary whose file was removed or replaced. A file whose name really ends that way keeps it. Blocking.
fn installed_path(path: PathBuf) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    const DELETED: &[u8] = b" (deleted)";
    if path.is_file() {
        return path;
    }
    match path.as_os_str().as_bytes().strip_suffix(DELETED) {
        Some(kept) => PathBuf::from(std::ffi::OsStr::from_bytes(kept)),
        None => path,
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
    /// Set when the daemon stops or a lead is refreshed: the process ends but the row keeps its state, so a restarted daemon resumes the worker and a refresh starts it in a new session. Also set when a worker's fold MCP server did not connect, just before its row ends `failed`, so the rest of its process changes no row and a resume waits for its exit.
    detached: Arc<AtomicBool>,
    /// Set once the process is reaped, after which its pid may belong to another process and is never signalled.
    reaped: Arc<AtomicBool>,
    /// The user turns written to the process's stdin that no result has ended yet. Claude Code queues a turn written during another one, and a worker whose next turn waits in that queue has not answered it.
    open_turns: Arc<AtomicU64>,
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
    /// The workers whose new session is under way, from a lead's hand-off refresh or a restart that a human or the main agent asked for, so two requests start one session.
    refreshing: Mutex<HashSet<AgentName>>,
    /// Held while a task worker is resumed, so two events that resume the same task worker start it once.
    resuming: tokio::sync::Mutex<()>,
    /// The live `can_use_tool` requests that wait for each open question, by the question's row id.
    pending: Mutex<HashMap<i64, Vec<Waiter>>>,
    /// The answered questions whose request ended with its process, by agent: each answers the resumed worker's first repeat of the same call once.
    replays: Mutex<HashMap<AgentName, Vec<i64>>>,
    /// The host registry, whose humans receive a worker's `AskUserQuestion` call as a question message.
    registry: Mutex<Option<Store>>,
    /// The workers whose session was gone and whose fresh start a pause refused, with the starter's instructions; the resume of the pause starts each fresh.
    held_lost: Mutex<HashMap<AgentName, Option<String>>>,
    /// Set when the daemon started while the project was paused, so the resume of the pause resumes the workers that could not reach fold, as the daemon start would have.
    unreachable_owed: AtomicBool,
    /// The workers that fold started again to answer a message, which stop once they have answered.
    answering: Mutex<HashSet<AgentName>>,
    /// The workers whose start for a message is under way, so two events start one.
    message_starts: Mutex<HashSet<AgentName>>,
    /// Who runs the main agent now: the interactive main sessions and a start of the supervised main.
    main_gate: Mutex<main::MainGate>,
    /// While a start of the supervised main is under way, the turns that came meanwhile, for its process.
    main_waking: Mutex<Option<Vec<String>>>,
    /// Wakes the project's removal of closed workspace directories, which follows only stored events: the supervised main's turn ends with none, and its closed workspaces become due once it is idle.
    cleanup_due: Mutex<Option<Arc<tokio::sync::Notify>>>,
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
    generation: u64,
    /// The process id that the row records while this process runs.
    pid: Option<u32>,
    /// The process runs with `--resume`.
    resumed: bool,
    /// Whether the process's session went on (`true`) or was not found (`false`); `None` until its first output tells.
    session: Arc<tokio::sync::watch::Sender<Option<bool>>>,
    /// The worker row's Claude Code session id.
    session_id: String,
    stderr: stderr::StderrTail,
    /// Shared with [`Running::open_turns`].
    open_turns: Arc<AtomicU64>,
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
                unreachable_owed: AtomicBool::new(false),
                answering: Mutex::new(HashSet::new()),
                message_starts: Mutex::new(HashSet::new()),
                main_gate: Mutex::new(main::MainGate::default()),
                main_waking: Mutex::new(None),
                cleanup_due: Mutex::new(None),
            }),
        };
        let receiver = supervisor.inner.bus.subscribe();
        let task = tokio::spawn(watch_done(Arc::downgrade(&supervisor.inner), receiver));
        *lock(&supervisor.inner.watch) = Some(task);
        supervisor
    }

    /// The fingerprint of the role text the installed fld gives a session of `role`; see [`Launcher::fingerprints`].
    pub fn fingerprint(&self, role: Role) -> Option<String> {
        lock(&self.inner.launcher).fingerprints.get(&role).cloned()
    }

    pub fn set_launcher(&self, launcher: Launcher) {
        *lock(&self.inner.launcher) = launcher;
    }

    /// The notifier of the project's removal of closed workspace directories, which the end of each turn of the supervised main wakes.
    pub fn set_cleanup_due(&self, due: Arc<tokio::sync::Notify>) {
        *lock(&self.inner.cleanup_due) = Some(due);
    }

    /// Where the workers' Claude Code keeps the transcript of each session of `ids`, in the order of `ids`: the folder of the launcher's environment, a relative one in the project folder, where every worker runs, which is followed to its real path as Claude Code names it. Each is `None` when fold cannot tell; see [`transcript_path`].
    pub async fn transcripts(&self, ids: Vec<String>) -> Vec<Option<PathBuf>> {
        let env = lock(&self.inner.launcher).env.clone();
        let root = self.inner.paths.root().to_owned();
        let count = ids.len();
        tokio::task::spawn_blocking(move || {
            let root = root.canonicalize().unwrap_or(root);
            let config = claude_config_dir(|name| env_value(&env, name)).map(|dir| root.join(dir));
            ids.iter()
                .map(|id| {
                    config
                        .as_deref()
                        .and_then(|dir| transcript_path(dir, &root, id))
                })
                .collect()
        })
        .await
        .unwrap_or_else(|_| vec![None; count])
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

    /// The agents whose worker process runs now, ordered by name.
    pub fn running_agents(&self) -> Vec<AgentName> {
        let mut agents: Vec<AgentName> = lock(&self.inner.running).keys().cloned().collect();
        agents.sort();
        agents
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

/// The open batch that the reviewer of `row` holds, which fold made of its two or more proposals at its start, for a first turn built again. It is found by its holder, not by a member, because the reviewer may have taken any member out of it.
pub(super) async fn reviewer_batch(
    store: &Store,
    row: &fold_store::agents::WorkerProcessRow,
) -> Result<Option<fold_core::ReviewBatchId>, fold_store::StoreError> {
    if row.role != fold_core::Role::Reviewer {
        return Ok(None);
    }
    Ok(
        fold_store::reviews::batches::list_batches(store.read(), true)
            .await?
            .into_iter()
            .rev()
            .find(|batch| batch.holder.belongs_to(&row.agent))
            .map(|batch| batch.id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_binary_names_the_file_installed_at_its_path() {
        assert_eq!(
            installed_path(PathBuf::from("/home/ana/.local/bin/fld (deleted)")),
            PathBuf::from("/home/ana/.local/bin/fld")
        );
        assert_eq!(
            installed_path(PathBuf::from("/usr/bin/fld")),
            PathBuf::from("/usr/bin/fld")
        );
        assert_eq!(
            installed_path(PathBuf::from("/opt/fld (deleted)/fld")),
            PathBuf::from("/opt/fld (deleted)/fld")
        );
        let dir = tempfile::TempDir::new().unwrap();
        let named = dir.path().join("fld (deleted)");
        std::fs::write(&named, "").unwrap();
        assert_eq!(installed_path(named.clone()), named);
    }

    #[test]
    fn a_launcher_keeps_the_fld_it_names_and_reads_its_own_otherwise() {
        let named = Launcher {
            fold: Some(PathBuf::from("/bin/fld")),
            ..Launcher::default()
        };
        assert_eq!(
            named.with_own_binary().fold,
            Some(PathBuf::from("/bin/fld"))
        );
        let own = Launcher::default().with_own_binary().fold.unwrap();
        assert!(own.is_file(), "{}", own.display());
    }

    #[test]
    fn a_launcher_takes_the_claude_location_it_does_not_set_itself() {
        use std::os::unix::ffi::OsStringExt;
        let set = Launcher {
            env: vec![("CLAUDE_CONFIG_DIR".to_owned(), "/etc/cc".to_owned())],
            ..Launcher::default()
        };
        let launcher = set.with_claude_location(|name| match name {
            "CLAUDE_CONFIG_DIR" => Some(OsString::from("/home/ana/cc")),
            "HOME" => Some(OsString::from("/home/ana")),
            "CLAUDE_CODE_PROJECT_DIR_NAME" => Some(OsString::from_vec(vec![0xff])),
            _ => None,
        });
        assert_eq!(
            launcher.env,
            [
                ("CLAUDE_CONFIG_DIR".to_owned(), "/etc/cc".to_owned()),
                ("HOME".to_owned(), "/home/ana".to_owned()),
            ]
        );
    }
}

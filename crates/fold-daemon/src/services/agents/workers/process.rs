use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use fold_core::config::RoleSettings;
use fold_core::events::types;
use fold_core::{
    AgentName, Clock, EventActor, LiveEvent, NewEvent, ProjectPaths, WorkerState, codes,
};
use fold_store::agents::{self, TurnTotals, WorkerProcessRow};
use fold_store::{Store, StoreError};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::output::{LineEffect, RowChange, effect_of, micro_usd};
use crate::push::{PushTargets, StdinSink};
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeInput, ClaudeMessage, OutgoingRequest};
use crate::services::events::{BusMessage, EventBuffer, EventBus};
use crate::services::projects::load_config;
use questions::Waiter;

mod questions;
mod refresh;
mod resume;

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

/// How long the reader waits for one more line after the process ended.
const LAST_LINES: Duration = Duration::from_millis(200);

/// How long a stop waits for the exit after SIGKILL.
const KILL_WAIT: Duration = Duration::from_secs(10);

/// How often a stop looks at the clock while it waits for an exit.
const STOP_POLL: Duration = Duration::from_millis(50);

/// Session-specific variables of a Claude Code session that started the daemon; a worker is a session of its own.
const SESSION_ENV: [&str; 5] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
];

enum Input {
    Line(String),
    Turn(String),
    Close,
}

#[derive(Clone)]
struct Running {
    generation: u64,
    input: mpsc::UnboundedSender<Input>,
    exited: watch::Receiver<bool>,
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
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.watch).take() {
            task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
    session: Arc<watch::Sender<Option<bool>>>,
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

    /// Runs Claude Code for the row, which a committed command left in state `starting`: a new session, or `--resume` of the row's session. `turn` is the first user turn; without one a resumed worker waits idle for its next notice. A process that cannot start, for an unreadable configuration, files that cannot be written, a failed spawn or a process that cannot be recorded, leaves the row `failed` with `worker.failed` and gives no [`StartWatch`], so no `starting` row holds a place under the limits.
    pub(super) async fn launch(
        &self,
        row: &WorkerProcessRow,
        resume: bool,
        turn: Option<String>,
    ) -> Result<Option<StartWatch>, AgentError> {
        let config = match load_config(&self.inner.paths).await {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(agent = %row.agent, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "could not read the configuration for a worker");
                self.fail(row.id, &row.agent, "config").await?;
                return Ok(None);
            }
        };
        let settings = config.role(row.role);
        let launcher = lock(&self.inner.launcher).clone();
        let files = write_files(&self.inner.paths, &launcher, row, &settings).await;
        let (mcp, prompt) = match files {
            Ok(files) => files,
            Err(_) => {
                tracing::warn!(agent = %row.agent, "could not write the worker's files");
                self.fail(row.id, &row.agent, "files").await?;
                return Ok(None);
            }
        };
        let mut command = Command::new(&launcher.claude);
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ]);
        if resume {
            command.arg("--resume").arg(&row.session_id);
        } else {
            command.arg("--session-id").arg(&row.session_id);
        }
        if let Some(model) = &row.model {
            command.arg("--model").arg(model);
        }
        command
            .args([
                "--permission-mode",
                "auto",
                "--permission-prompt-tool",
                "stdio",
            ])
            .arg("--mcp-config")
            .arg(&mcp);
        if settings.mcp.is_some() {
            command.arg("--strict-mcp-config");
        }
        command.arg("--append-system-prompt-file").arg(&prompt);
        for name in SESSION_ENV {
            command.env_remove(name);
        }
        let ttl = settings.cache_ttl.as_str();
        command
            .env("FOLD_AGENT", row.agent.as_str())
            .env("CLAUDE_CODE_PROMPT_CACHE_TTL", ttl)
            .env("CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL", ttl)
            .envs(launcher.env.iter().map(|(key, value)| (key, value)))
            // A daemon that `fld claude`'s session started inherits its `main` tag request, which a worker never makes.
            .env_remove("FOLD_AGENT_TAGS")
            .current_dir(self.inner.paths.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(feature = "failpoints")]
        fail::fail_point!("worker_launch_before_spawn");
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                tracing::warn!(agent = %row.agent, "could not start Claude Code for a worker");
                self.fail(row.id, &row.agent, "spawn").await?;
                return Ok(None);
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            self.fail(row.id, &row.agent, "spawn").await?;
            return Ok(None);
        };
        let pid = child.id();
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (input, inputs) = mpsc::unbounded_channel();
        let (exit, exited) = watch::channel(false);
        let detached = Arc::new(AtomicBool::new(false));
        let reaped = Arc::new(AtomicBool::new(false));
        let (session, sessions) = watch::channel(None);
        let process = Process {
            id: row.id,
            agent: row.agent.clone(),
            base_cost: row.cost_micro_usd,
            generation,
            resumed: resume,
            session_id: row.session_id.clone(),
            session: Arc::new(session),
        };
        let start = StartWatch {
            session: sessions,
            exited: exited.clone(),
        };
        let run = Running {
            generation,
            input: input.clone(),
            exited,
            pid,
            detached: Arc::clone(&detached),
            reaped: Arc::clone(&reaped),
        };
        lock(&self.inner.running).insert(row.agent.clone(), run.clone());
        // A stop that came after the start committed but before the process was in `running` found no process to end. The row is read only now that the process is there, so either that stop sees the process or this check sees the stop.
        let idle = turn.is_none();
        let now = self.inner.clock.now();
        let current = self
            .inner
            .store
            .write(async |tx| {
                let starting = agents::find_worker_process(tx.conn(), &row.agent)
                    .await?
                    .is_some_and(|current| {
                        current.id == row.id && current.state == WorkerState::Starting
                    });
                if starting {
                    agents::set_worker_pid(tx.conn(), row.id, pid).await?;
                    if idle {
                        agents::set_worker_state(tx.conn(), row.id, WorkerState::Idle, now).await?;
                    }
                }
                Ok::<_, StoreError>(starting)
            })
            .await;
        let starting = match current {
            Ok(starting) => starting,
            Err(error) => {
                self.forget(&process);
                tracing::warn!(agent = %row.agent, code = error.code().unwrap_or("store_error"), "could not record a worker's process");
                // Dropping the child on return kills the process.
                self.fail(row.id, &row.agent, "record").await?;
                return Ok(None);
            }
        };
        tokio::spawn(drain(stderr));
        if !starting {
            tokio::spawn(self.clone().feed(stdin, inputs, process.clone()));
            tokio::spawn(
                self.clone()
                    .drive(child, stdout, process, exit, detached, reaped),
            );
            let supervisor = self.clone();
            tokio::spawn(async move { supervisor.terminate(&run, false).await });
            return Ok(None);
        }
        let initialize = ClaudeInput::ControlRequest {
            request_id: self.request_id(),
            request: OutgoingRequest::Initialize { hooks: None },
        };
        if let Ok(line) = initialize.to_line() {
            let _ = input.send(Input::Line(line));
        }
        if let Some(turn) = turn {
            let _ = input.send(Input::Turn(turn));
        }
        let sink_input = input.clone();
        let sink: StdinSink =
            Arc::new(move |text: &str| sink_input.send(Input::Turn(text.to_owned())).is_ok());
        self.inner.push.register_stdin(&row.agent, sink);
        tokio::spawn(self.clone().feed(stdin, inputs, process.clone()));
        tokio::spawn(
            self.clone()
                .drive(child, stdout, process, exit, detached, reaped),
        );
        Ok(Some(start))
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

    /// Records a worker whose process could not start. A row that is no longer the agent's current one, or that has ended already, is left as it is.
    async fn fail(&self, id: i64, agent: &AgentName, reason: &str) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                let current = agents::find_worker_process(tx.conn(), agent).await?;
                if !current.is_some_and(|current| current.id == id && current.state.is_running()) {
                    return Ok(());
                }
                agents::record_exit(tx.conn(), id, WorkerState::Failed, None, now).await?;
                agents::close_open_questions(tx.conn(), id, now).await?;
                let event = NewEvent::new(types::WORKER_FAILED, EventActor::fold())?
                    .data(json!({"agent": agent.as_str(), "reason": reason}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Writes the lines of the input channel to the process's stdin until the channel closes or a `Close` arrives, which closes stdin so Claude Code exits after its turn. Each user turn makes the worker `working`.
    async fn feed(
        self,
        mut stdin: ChildStdin,
        mut inputs: mpsc::UnboundedReceiver<Input>,
        process: Process,
    ) {
        while let Some(input) = inputs.recv().await {
            let (line, turn) = match input {
                Input::Line(line) => (line, false),
                Input::Turn(text) => match ClaudeInput::user(text).to_line() {
                    Ok(line) => (line, true),
                    Err(_) => continue,
                },
                Input::Close => break,
            };
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
            if turn {
                let marked = self
                    .inner
                    .store
                    .write(async |tx| agents::mark_worker_working(tx.conn(), process.id).await)
                    .await;
                if let Err(error) = marked {
                    tracing::warn!(agent = %process.agent, code = error.code().unwrap_or("store_error"), "could not mark a worker working");
                }
            }
        }
    }

    /// Reads the process's stdout line by line until it closes, then records the exit.
    async fn drive(
        self,
        mut child: Child,
        stdout: ChildStdout,
        process: Process,
        exit: watch::Sender<bool>,
        detached: Arc<AtomicBool>,
        reaped: Arc<AtomicBool>,
    ) {
        let mut lines = BufReader::new(stdout).lines();
        let status = loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        self.on_line(&process, &line, detached.load(Ordering::SeqCst)).await;
                    }
                    Ok(None) | Err(_) => {
                        let status = child.wait().await;
                        reaped.store(true, Ordering::SeqCst);
                        break status;
                    }
                },
                status = child.wait() => {
                    reaped.store(true, Ordering::SeqCst);
                    // A child of the process can keep the pipe open after the process ended, so the lines it wrote last are read only while they keep coming.
                    while let Ok(Ok(Some(line))) =
                        tokio::time::timeout(LAST_LINES, lines.next_line()).await
                    {
                        self.on_line(&process, &line, detached.load(Ordering::SeqCst)).await;
                    }
                    break status;
                }
            }
        };
        let code = status.ok().and_then(exit_code);
        if let Err(error) = self
            .on_exit(&process, code, detached.load(Ordering::SeqCst))
            .await
        {
            tracing::warn!(agent = %process.agent, code = super::code_of(&error), "could not record a worker's exit");
        }
        lock(&self.inner.replays).remove(&process.agent);
        self.forget(&process);
        self.forget_waiters(process.generation);
        let _ = exit.send(true);
    }

    /// A process the daemon lets go of at its stop changes no row: the interrupted turn's result would make a working worker look idle, and the next start resumes the worker by the state it had.
    async fn on_line(&self, process: &Process, line: &str, detached: bool) {
        let Ok(message) = ClaudeMessage::parse(line) else {
            tracing::debug!(agent = %process.agent, "a worker wrote a line that is not JSON");
            return;
        };
        let LineEffect {
            step,
            mut change,
            reply,
            permission,
        } = effect_of(&message);
        if let Some(went) = resume::session_signal(&message, process.resumed) {
            process.session.send_if_modified(|seen| {
                let first = seen.is_none();
                if first {
                    *seen = Some(went);
                }
                first
            });
            // A lost session ran no turn, so its error result counts none.
            if !went {
                change = RowChange::None;
            }
        }
        if let Some(reply) = reply
            && let Ok(line) = reply.to_line()
        {
            let input = lock(&self.inner.running)
                .get(&process.agent)
                .map(|run| run.input.clone());
            if let Some(input) = input {
                let _ = input.send(Input::Line(line));
            }
        }
        if let Some(request) = permission
            && !detached
        {
            self.on_tool_request(process, request).await;
        }
        let now = self.inner.clock.now();
        let id = process.id;
        let mut ended = None;
        let written = match change {
            _ if detached => Ok(()),
            RowChange::None => Ok(()),
            RowChange::TurnStarted { model } => {
                self.inner
                    .store
                    .write(async |tx| {
                        let model = (!model.is_empty()).then_some(model.as_str());
                        agents::set_worker_context(tx.conn(), id, model, None).await?;
                        agents::mark_worker_working(tx.conn(), id).await?;
                        Ok::<_, StoreError>(())
                    })
                    .await
            }
            RowChange::Context { tokens, model } => {
                self.inner
                    .store
                    .write(async |tx| {
                        agents::set_worker_context(tx.conn(), id, model.as_deref(), Some(tokens))
                            .await
                    })
                    .await
            }
            RowChange::TurnEnded {
                total_cost_usd,
                usage,
                windows,
            } => {
                ended = Some(windows);
                let totals = TurnTotals {
                    cost_micro_usd: process.base_cost.saturating_add(micro_usd(total_cost_usd)),
                    cache_read_tokens: usage.cache_read_input_tokens,
                    cache_write_tokens: usage.cache_creation_input_tokens,
                };
                self.inner
                    .store
                    .write(async |tx| agents::record_turn(tx.conn(), id, totals, now).await)
                    .await
            }
        };
        if let Err(error) = &written {
            tracing::warn!(agent = %process.agent, code = error.code().unwrap_or("store_error"), "could not record a worker's output");
        }
        if let Ok(event) = LiveEvent::new(
            types::WORKER_OUTPUT,
            None,
            step.data(process.agent.as_str()),
        ) {
            self.inner.bus.publish_live(event);
        }
        if let Some(windows) = ended
            && written.is_ok()
        {
            self.after_turn(&process.agent, &windows).await;
        }
    }

    /// A stop that fold asked for ends `stopped` with `worker.stopped`; any other exit ends `failed` with `worker.failed`. A process the daemon let go of at its own stop leaves the row as it was.
    async fn on_exit(
        &self,
        process: &Process,
        code: Option<i32>,
        detached: bool,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let id = process.id;
        if detached {
            self.inner
                .store
                .write(async |tx| agents::set_worker_pid(tx.conn(), id, None).await)
                .await?;
            return Ok(());
        }
        let mut buffer = EventBuffer::new();
        let agent = &process.agent;
        self.inner
            .store
            .write(async |tx| {
                let Some(row) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, AgentError>(());
                };
                if row.id != id || !row.state.is_running() {
                    return Ok(());
                }
                agents::close_open_questions(tx.conn(), id, now).await?;
                let event = if row.state == WorkerState::Stopping {
                    agents::record_exit(tx.conn(), id, WorkerState::Stopped, code, now).await?;
                    stopped_event(&row, code)?
                } else {
                    agents::record_exit(tx.conn(), id, WorkerState::Failed, code, now).await?;
                    NewEvent::new(types::WORKER_FAILED, EventActor::fold())?.data(json!({
                        "agent": agent.as_str(),
                        "reason": "exited",
                        "exit_code": code,
                    }))?
                };
                buffer.append(tx, &event, now).await?;
                Ok(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Marks the agent's running worker `stopping`, then ends its process: the interrupt of a working turn and a closed stdin, then SIGINT, SIGTERM and SIGKILL, each after its timeout. It returns once the process has exited and its exit is recorded. A worker that is not running is left as it is; one whose row runs without a process here ends `stopped` at once.
    pub(super) async fn stop(&self, agent: &AgentName) -> Result<(), AgentError> {
        self.stop_when(agent, |_| true).await.map(|_| ())
    }

    /// [`Supervisor::stop`] of a worker that `still` holds for, read in the transaction that marks it `stopping`. Returns whether it stopped the worker.
    async fn stop_when(
        &self,
        agent: &AgentName,
        still: impl Fn(&WorkerProcessRow) -> bool,
    ) -> Result<bool, AgentError> {
        let now = self.inner.clock.now();
        let started = self
            .inner
            .store
            .write(async |tx| {
                let Some(row) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, StoreError>(None);
                };
                if !row.state.is_running() || !still(&row) {
                    return Ok(None);
                }
                let working = row.state == WorkerState::Working;
                agents::set_worker_state(tx.conn(), row.id, WorkerState::Stopping, now).await?;
                Ok(Some((row, working)))
            })
            .await?;
        let Some((row, working)) = started else {
            return Ok(false);
        };
        let running = lock(&self.inner.running).get(agent).cloned();
        match running {
            Some(run) => self.terminate(&run, working).await,
            None => self.stopped_without_process(&row).await?,
        }
        Ok(true)
    }

    /// Stops `agent` in a task of its own, as a worker whose work is done.
    fn spawn_stop(&self, agent: AgentName) {
        let supervisor = self.clone();
        tokio::spawn(async move {
            if let Err(error) = supervisor.stop(&agent).await {
                tracing::warn!(%agent, code = super::code_of(&error), "could not stop a finished worker");
            }
        });
    }

    /// Ends a `stopping` row that has no process here. A process that a racing launch started and ended itself may have recorded the stop already, so a row that is no longer `stopping` is left as it is.
    async fn stopped_without_process(&self, row: &WorkerProcessRow) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                let stopping = agents::find_worker_process(tx.conn(), &row.agent)
                    .await?
                    .is_some_and(|current| {
                        current.id == row.id && current.state == WorkerState::Stopping
                    });
                if !stopping {
                    return Ok(());
                }
                agents::record_exit(tx.conn(), row.id, WorkerState::Stopped, None, now).await?;
                agents::close_open_questions(tx.conn(), row.id, now).await?;
                buffer.append(tx, &stopped_event(row, None)?, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    async fn terminate(&self, run: &Running, working: bool) {
        let timeouts = *lock(&self.inner.timeouts);
        if working {
            let interrupt = ClaudeInput::ControlRequest {
                request_id: self.request_id(),
                request: OutgoingRequest::Interrupt,
            };
            if let Ok(line) = interrupt.to_line() {
                let _ = run.input.send(Input::Line(line));
            }
        }
        let _ = run.input.send(Input::Close);
        let mut exited = run.exited.clone();
        for (signal, wait) in [
            (rustix::process::Signal::INT, timeouts.interrupt),
            (rustix::process::Signal::TERM, timeouts.sigint),
            (rustix::process::Signal::KILL, timeouts.sigterm),
        ] {
            if self.wait_exit(&mut exited, wait).await {
                return;
            }
            if run.reaped.load(Ordering::SeqCst) {
                continue;
            }
            if let Some(pid) = run
                .pid
                .and_then(|pid| i32::try_from(pid).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process(pid, signal);
            }
        }
        // SIGKILL cannot be ignored, so only the reader's last lines and the recorded exit remain; the timeout guards against a pid that was never known.
        let killed = tokio::time::timeout(KILL_WAIT, exited.wait_for(|done| *done)).await;
        if !matches!(killed, Ok(Ok(_))) {
            tracing::warn!("a worker process did not exit after SIGKILL");
        }
    }

    /// Waits until the process has exited or `wait` has passed on the project's clock.
    async fn wait_exit(&self, exited: &mut watch::Receiver<bool>, wait: Duration) -> bool {
        let Some(deadline) = self.inner.clock.now().checked_add(wait) else {
            return *exited.borrow();
        };
        loop {
            if *exited.borrow() {
                return true;
            }
            if self.inner.clock.now() >= deadline {
                return false;
            }
            tokio::select! {
                changed = exited.changed() => {
                    if changed.is_err() {
                        return true;
                    }
                }
                () = tokio::time::sleep(STOP_POLL) => {}
            }
        }
    }

    /// Ends every worker process the way a stop does, at the daemon's stop, but keeps each row's state, so the next start resumes the workers that were working or idle.
    pub async fn shutdown(&self) {
        let working: Vec<AgentName> = match self.inner.store.read().acquire().await {
            Ok(mut conn) => agents::list_worker_processes(&mut conn)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|row| row.state == WorkerState::Working)
                .map(|row| row.agent)
                .collect(),
            Err(_) => Vec::new(),
        };
        let running: Vec<(AgentName, Running)> = lock(&self.inner.running)
            .iter()
            .map(|(agent, run)| (agent.clone(), run.clone()))
            .collect();
        for (agent, run) in running {
            run.detached.store(true, Ordering::SeqCst);
            self.terminate(&run, working.contains(&agent)).await;
        }
    }

    /// The short-lived workers whose work `event` finished: a task coder once no bound task awaits its work (see [`agents::tasks_awaiting_proposal`]), checked when it opens a proposal or sends a new version and when a bound task is archived; a batch reviewer once every bound proposal has a verdict or is no longer open.
    async fn done_workers(&self, event: &fold_core::Event) -> Result<Vec<AgentName>, StoreError> {
        let kind = event.event_type;
        let mut done = Vec::new();
        if (kind == types::PROPOSAL_OPENED
            || kind == types::PROPOSAL_VERSION_ADDED
            || kind == types::TASK_ARCHIVED)
            && let Some(task) = event.task
        {
            for coder in agents::coders_of_task(self.inner.store.read(), task).await? {
                let mut conn = self
                    .inner
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                if let Some(row) = agents::find_worker_process(&mut conn, &coder).await?
                    && agents::tasks_awaiting_proposal(&mut *conn, row.id).await? == 0
                {
                    done.push(coder);
                }
            }
        }
        if (kind == types::REVIEW_SUBMITTED
            || kind == types::PROPOSAL_CLOSED
            || kind == types::PROPOSAL_INTEGRATED)
            && let Some(proposal) = event.proposal
        {
            for reviewer in agents::reviewers_of_proposal(self.inner.store.read(), proposal).await?
            {
                let mut conn = self
                    .inner
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                if let Some(row) = agents::find_worker_process(&mut conn, &reviewer).await?
                    && agents::proposals_without_verdict(&mut *conn, row.id).await? == 0
                {
                    done.push(reviewer);
                }
            }
        }
        // A batch review names its batch, not each member, so every running batch reviewer is checked.
        if kind == types::REVIEW_BATCH_SUBMITTED {
            let mut conn = self
                .inner
                .store
                .read()
                .acquire()
                .await
                .map_err(StoreError::from)?;
            let reviewers: Vec<WorkerProcessRow> = agents::list_worker_processes(&mut conn)
                .await?
                .into_iter()
                .filter(|row| {
                    row.role == fold_core::Role::BatchReviewer
                        && row.state.is_running()
                        && row.state != WorkerState::Stopping
                })
                .collect();
            for row in reviewers {
                if !done.contains(&row.agent)
                    && agents::proposals_without_verdict(&mut *conn, row.id).await? == 0
                {
                    done.push(row.agent);
                }
            }
        }
        Ok(done)
    }

    /// Stops every short-lived worker that has been idle for its role's `idle_minutes` at `now`. A worker is stopped only while it is still idle since the same moment, so one that took a turn after the rows were read keeps running. A worker that cannot be stopped is logged and the sweep goes on. Returns the agents it stopped.
    pub async fn sweep_idle(
        &self,
        now: fold_core::Timestamp,
    ) -> Result<Vec<AgentName>, AgentError> {
        let config = load_config(&self.inner.paths).await?;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        let mut stopped = Vec::new();
        for row in rows {
            let Some(minutes) = config.role(row.role).idle_minutes else {
                continue;
            };
            let Some(since) = row.idle_since.filter(|_| row.state == WorkerState::Idle) else {
                continue;
            };
            let due = since
                .checked_add(Duration::from_secs(minutes.saturating_mul(60)))
                .is_some_and(|due| due <= now);
            if !due {
                continue;
            }
            #[cfg(feature = "failpoints")]
            fail::fail_point!("worker_sweep_before_stop");
            let id = row.id;
            let still_idle = |current: &WorkerProcessRow| {
                current.id == id
                    && current.state == WorkerState::Idle
                    && current.idle_since == Some(since)
            };
            match self.stop_when(&row.agent, still_idle).await {
                Ok(true) => stopped.push(row.agent),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(agent = %row.agent, code = super::code_of(&error), "could not stop an idle worker");
                }
            }
        }
        Ok(stopped)
    }

    /// Runs again, after a daemon start, every worker that was working or idle when the daemon stopped: `--resume` of its session, with a turn that tells a working one to go on. A worker whose session is gone starts again in a new one, a lead with its latest hand-off note. A worker with an open question waits without a process until a human answers it, and then resumes with the answer as its first turn. A worker left `starting` ends `failed` and one left `stopping` ends `stopped`. A row that cannot be served is logged and the next row is served. Requested changes and conflicts that the worker limits held back are then served.
    pub async fn resume_after_restart(&self) -> Result<(), AgentError> {
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        for row in rows {
            if let Err(error) = self.resume_row(&row).await {
                tracing::warn!(agent = %row.agent, code = super::code_of(&error), "could not serve a worker after the daemon start");
            }
        }
        tokio::spawn(self.clone().resume_pending());
        Ok(())
    }

    /// What a daemon start does with one row; see [`Supervisor::resume_after_restart`].
    async fn resume_row(&self, row: &WorkerProcessRow) -> Result<(), AgentError> {
        match row.state {
            WorkerState::Working | WorkerState::Idle => {
                if self.waits_for_answer(row).await? {
                    return Ok(());
                }
                let turn = (row.state == WorkerState::Working).then(|| RESTART_TURN.to_owned());
                let now = self.inner.clock.now();
                self.inner
                    .store
                    .write(async |tx| agents::restart_worker_process(tx.conn(), row.id, now).await)
                    .await?;
                if let Some(watch) = self.launch(row, true, turn).await? {
                    tokio::spawn(self.clone().restart_lost(row.agent.clone(), watch, None));
                }
            }
            WorkerState::Starting => self.fail(row.id, &row.agent, "daemon_restart").await?,
            WorkerState::Stopping => self.stopped_without_process(row).await?,
            WorkerState::Stopped | WorkerState::Failed => {}
        }
        Ok(())
    }

    /// Does from the stored state what the events the watcher missed would have done: answers each `ask` question whose messages all have their replies, stops each short-lived worker whose work is done, and serves the resumes the limits held back.
    async fn recover(self) {
        self.answer_open_asks().await;
        self.stop_done_workers().await;
        self.resume_pending().await;
    }

    async fn stop_done_workers(&self) {
        let rows = async {
            let mut conn = self
                .inner
                .store
                .read()
                .acquire()
                .await
                .map_err(StoreError::from)?;
            let mut done = Vec::new();
            for row in agents::list_worker_processes(&mut conn).await? {
                if !matches!(row.state, WorkerState::Working | WorkerState::Idle) {
                    continue;
                }
                let finished = match row.role {
                    fold_core::Role::TaskCoder => {
                        agents::tasks_awaiting_proposal(&mut *conn, row.id).await? == 0
                    }
                    fold_core::Role::BatchReviewer => {
                        agents::proposals_without_verdict(&mut *conn, row.id).await? == 0
                    }
                    fold_core::Role::AreaLead | fold_core::Role::ReviewLead => false,
                };
                if finished {
                    done.push(row.agent);
                }
            }
            Ok::<_, StoreError>(done)
        }
        .await;
        match rows {
            Ok(done) => {
                for agent in done {
                    self.spawn_stop(agent);
                }
            }
            Err(error) => tracing::warn!(
                code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                "could not check for finished workers"
            ),
        }
    }
}

/// The turn a worker that was working gets after a daemon restart resumed its session.
pub const RESTART_TURN: &str = "fold restarted while you were working. Check your claims and your workspace, then continue your work.";

fn stopped_event(
    row: &WorkerProcessRow,
    code: Option<i32>,
) -> Result<NewEvent, fold_core::EventError> {
    NewEvent::new(types::WORKER_STOPPED, EventActor::fold())?.data(json!({
        "agent": row.agent.as_str(),
        "exit_code": code,
        "turns": row.turns,
        "cost_micro_usd": row.cost_micro_usd,
        "cache_read_tokens": row.cache_read_tokens,
        "cache_write_tokens": row.cache_write_tokens,
    }))
}

/// A signal's exit is reported as 128 plus its number, as a shell does.
fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
}

/// Reads and drops the process's stderr, so a full pipe never blocks it. Its text never reaches a log.
async fn drain(mut stderr: ChildStderr) {
    let mut buffer = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
            break;
        }
    }
}

/// Stops each short-lived worker whose work an event finished. A watcher that falls behind the bus checks the stored state instead of the events it missed. It holds the supervisor weakly, so the project's handle can drop.
async fn watch_done(
    inner: Weak<Inner>,
    mut receiver: tokio::sync::broadcast::Receiver<BusMessage>,
) {
    loop {
        let event = match receiver.recv().await {
            Ok(BusMessage::Durable(event)) => event,
            Ok(BusMessage::Live(_)) => continue,
            Err(RecvError::Lagged(missed)) => {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                tracing::warn!(
                    missed,
                    "the worker supervisor missed events; it checks the stored state"
                );
                tokio::spawn(Supervisor { inner }.recover());
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let supervisor = Supervisor { inner };
        supervisor.on_event(&event).await;
        let done = match supervisor.done_workers(&event).await {
            Ok(done) => done,
            Err(error) => {
                tracing::warn!(
                    seq = event.seq,
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                    "could not check for finished workers"
                );
                continue;
            }
        };
        for agent in done {
            supervisor.spawn_stop(agent);
        }
        let items = resume::resume_items(&event);
        if !items.is_empty() {
            tokio::spawn(supervisor.clone().resume_authors(items));
        }
        // A freed worker slot or a raised limit can serve a resume that the limits refused earlier.
        if event.event_type == types::WORKER_STOPPED
            || event.event_type == types::WORKER_FAILED
            || event.event_type == types::WORKERS_LIMITS_CHANGED
        {
            tokio::spawn(supervisor.resume_pending());
        }
    }
}

/// Writes the worker's MCP configuration and, unless the project overrides the role's prompt, the shipped prompt, under the worker's folder. Returns the two paths Claude Code reads.
async fn write_files(
    paths: &ProjectPaths,
    launcher: &Launcher,
    row: &WorkerProcessRow,
    settings: &RoleSettings,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = paths.worker_dir(&row.agent);
    let override_prompt = paths.role_prompt(row.role);
    let project_mcp = paths.mcp_json();
    let fold = match &launcher.fold {
        Some(fold) => fold.clone(),
        None => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fld")),
    };
    let agent = row.agent.clone();
    let prompt_text = row.role.prompt();
    let limit = settings.mcp.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        let config = mcp_config(&fold, &agent, limit.as_deref(), &project_mcp);
        let mcp = dir.join("mcp.json");
        std::fs::write(&mcp, config.to_string())?;
        let prompt = if override_prompt.is_file() {
            override_prompt
        } else {
            let prompt = dir.join("prompt.md");
            std::fs::write(&prompt, prompt_text)?;
            prompt
        };
        Ok((mcp, prompt))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// The `fold` server with the worker's agent name, plus, when the role limits its servers, each named server of the project's `.mcp.json`. Servers of the user's own configuration are never copied, because their entries can hold credentials; without a limit Claude Code loads them itself.
fn mcp_config(
    fold: &Path,
    agent: &AgentName,
    limit: Option<&[String]>,
    project_mcp: &Path,
) -> Value {
    let mut servers = Map::new();
    servers.insert(
        crate::services::agents::workers::FOLD_SERVER.to_owned(),
        json!({
            "command": fold,
            "args": ["mcp"],
            "env": {"FOLD_AGENT": agent.as_str()},
        }),
    );
    if let Some(names) = limit {
        let project: Value = std::fs::read_to_string(project_mcp)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        for name in names {
            if servers.contains_key(name) {
                continue;
            }
            if let Some(entry) = project
                .get("mcpServers")
                .and_then(|servers| servers.get(name))
            {
                servers.insert(name.clone(), entry.clone());
            }
        }
    }
    json!({ "mcpServers": servers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limited_config_copies_only_named_project_servers() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();
        let project = dir.join(".mcp.json");
        std::fs::write(
            &project,
            r#"{"mcpServers":{"pencil":{"command":"pencil"},"other":{"command":"x"},"fold":{"command":"old"}}}"#,
        )
        .unwrap();
        let agent = AgentName::parse("web-lead").unwrap();
        let fold = Path::new("/bin/fld");
        let open = mcp_config(fold, &agent, None, &project);
        assert_eq!(
            open,
            json!({"mcpServers": {"fold": {"command": "/bin/fld", "args": ["mcp"], "env": {"FOLD_AGENT": "web-lead"}}}})
        );
        let names = ["fold".to_owned(), "pencil".to_owned(), "missing".to_owned()];
        let limited = mcp_config(fold, &agent, Some(&names), &project);
        let servers = limited["mcpServers"].as_object().unwrap();
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["fold", "pencil"]);
        assert_eq!(servers["fold"]["command"], "/bin/fld");
        let none = mcp_config(fold, &agent, Some(&names), &dir.join("absent.json"));
        assert_eq!(none["mcpServers"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn a_signal_exit_counts_as_128_plus_the_signal() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(0)), Some(0));
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(2)), Some(130));
        assert_eq!(
            exit_code(std::process::ExitStatus::from_raw(1 << 8)),
            Some(1)
        );
    }
}

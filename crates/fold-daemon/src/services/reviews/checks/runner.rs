use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fold_core::layout::{AGENT_ENV, AGENT_TAGS_ENV};
use fold_core::reviews::checks::{CHECK_TIMEOUT, ChecksFile, Outcome, Planned};
use fold_core::{AgentName, CheckRunId, Clock, Timestamp, WorkspaceId};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{OwnedRwLockReadGuard, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

use super::memory::{self, MemorySource};
use crate::services::agents::peers::{self, PeerWorker};
use crate::support::{confine_git, lock, remove_daemon_context};

/// The end of a command's output that a result keeps. The end is where a failure shows.
pub const LOG_LIMIT: usize = 64 * 1024;

/// The variables that name who calls fold: the agent and tags of a session, and the human of the CLI. A check command acts for no agent and no human, so it never gets the daemon's.
const IDENTITY_ENV: [&str; 3] = [AGENT_ENV, AGENT_TAGS_ENV, "FOLD_HUMAN"];

/// How long a command that ran out of time has after SIGTERM before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(5);

/// How long the output readers may run on after the command exited, for a child that keeps the pipes open.
const DRAIN_WAIT: Duration = Duration::from_secs(5);

/// The limits of one command. Tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    pub timeout: Duration,
    pub kill_grace: Duration,
    pub log_limit: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            timeout: CHECK_TIMEOUT,
            kill_grace: KILL_GRACE,
            log_limit: LOG_LIMIT,
        }
    }
}

/// The host's settings of check runs, the same for every project the daemon opens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckSettings {
    /// The most runs at once on the machine; `None` is half the processors the daemon may use.
    pub max_runs: Option<NonZeroUsize>,
    /// The free memory in bytes a run waits for before its commands start; `None` is the larger of 4 GiB and 10% of the total.
    pub memory_floor: Option<u64>,
}

/// How often a run that waits for free memory reads the machine's memory again.
pub const MEMORY_TICK: Duration = Duration::from_secs(10);

/// The memory a run with its permit waits for before its commands start.
#[derive(Clone)]
struct MemoryGate {
    /// `None` skips the gate.
    source: Option<MemorySource>,
    floor: Option<u64>,
    tick: Duration,
}

impl std::fmt::Debug for MemoryGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryGate")
            .field("source", &self.source.is_some())
            .field("floor", &self.floor)
            .field("tick", &self.tick)
            .finish()
    }
}

/// How many runs of checks may go at the same time on this machine, and the free memory each waits for. Every project the daemon opens shares one set, so a machine with many projects never runs more test suites at once than the host allows.
#[derive(Debug, Clone)]
pub struct CheckPermits {
    runs: Arc<Semaphore>,
    memory: MemoryGate,
}

impl CheckPermits {
    /// At least one run, with no memory gate.
    pub fn new(runs: usize) -> Self {
        Self {
            runs: Arc::new(Semaphore::new(runs.max(1))),
            memory: MemoryGate {
                source: None,
                floor: None,
                tick: MEMORY_TICK,
            },
        }
    }

    /// The permits of the host's settings, with the memory gate of this machine's `/proc/meminfo` on Linux.
    pub fn from_settings(settings: CheckSettings) -> Self {
        let runs = settings
            .max_runs
            .map_or_else(half_the_processors, NonZeroUsize::get);
        let mut permits = Self::new(runs);
        permits.memory.source = memory::system_source();
        permits.memory.floor = settings.memory_floor;
        permits
    }

    /// The permits with the memory gate reading `source`, waiting for `floor` and reading again every `tick`.
    #[must_use]
    pub fn with_memory_gate(
        mut self,
        source: MemorySource,
        floor: Option<u64>,
        tick: Duration,
    ) -> Self {
        self.memory = MemoryGate {
            source: Some(source),
            floor,
            tick,
        };
        self
    }

    /// How many runs may go at once while none goes.
    pub fn available(&self) -> usize {
        self.runs.available_permits()
    }

    pub(crate) async fn acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.runs).acquire_owned().await.ok()
    }
}

/// Half the processors the daemon may use, at least one.
fn half_the_processors() -> usize {
    let processors = std::thread::available_parallelism().map_or(2, usize::from);
    (processors / 2).max(1)
}

/// Half the processors with no memory gate; the daemon builds its permits with [`CheckPermits::from_settings`].
impl Default for CheckPermits {
    fn default() -> Self {
        Self::new(half_the_processors())
    }
}

/// The background runs of one project: each holds a machine-wide permit while its commands run. Dropping the project's runner, or [`CheckRunner::shutdown`], aborts the runs, and each command's process is killed with its task.
#[derive(Debug, Clone, Default)]
pub struct CheckRunner {
    /// The slug of the project, which the peer checks compare with the project of a socket session.
    project: String,
    permits: CheckPermits,
    limits: RunLimits,
    tasks: Arc<Mutex<JoinSet<()>>>,
    /// Held while fold's check directory is brought to a tree and while checks run in it.
    check_dir: Arc<tokio::sync::Mutex<()>>,
    /// Shared by every blocking task in the check directory while it runs. An abort ends a run's future at once but not the blocking task it waits for, so [`CheckRunner::shutdown`] takes it whole to wait for those tasks.
    dir_work: Arc<tokio::sync::RwLock<()>>,
    queue: Arc<Queue>,
    held: Arc<Mutex<Vec<HeldRequest>>>,
    /// The runs of the project that wait, with what each waits for and the time it began to wait.
    memory_waits: Waits,
    /// The permits of each check with a `max_parallel`, by check name, with the limit they were made for.
    check_limits: CheckLimits,
    /// The runs of the project that a cancel can reach, by run, each with the sender its run listens on.
    cancels: Cancels,
    /// The folders whose `[setup]` runs, each with the receiver that turns true at its end.
    setups: Arc<Mutex<HashMap<WorkspaceId, tokio::sync::watch::Receiver<bool>>>>,
    /// The project's places for runs under `[runs] max_parallel_runs`.
    slots: Arc<RunSlots>,
}

type Cancels = Arc<Mutex<HashMap<CheckRunId, tokio::sync::oneshot::Sender<CancelRequest>>>>;

/// A cancel of one run: who cancelled it, and the sender the run answers on once it has ended.
#[derive(Debug)]
pub(crate) struct CancelRequest {
    pub by: fold_core::EventActor,
    pub ended: tokio::sync::oneshot::Sender<()>,
}

/// The place of a run among the runs a cancel can reach, left when the run ends or is aborted.
#[derive(Debug)]
pub(crate) struct CancelWatch {
    run: CheckRunId,
    cancels: Cancels,
    request: Option<tokio::sync::oneshot::Receiver<CancelRequest>>,
}

impl CancelWatch {
    /// The next cancel of the run; it never resolves once the run's place is gone.
    pub(crate) async fn next(&mut self) -> CancelRequest {
        match self.request.as_mut() {
            Some(request) => match request.await {
                Ok(cancel) => {
                    self.request = None;
                    cancel
                }
                Err(_) => {
                    self.request = None;
                    std::future::pending().await
                }
            },
            None => std::future::pending().await,
        }
    }
}

impl Drop for CancelWatch {
    fn drop(&mut self) {
        lock(&self.cancels).remove(&self.run);
    }
}

/// A running setup of a folder: dropping it ends the wait of every check run of the folder.
#[derive(Debug)]
pub(crate) struct SetupRunning {
    workspace: WorkspaceId,
    done: tokio::sync::watch::Sender<bool>,
    setups: Arc<Mutex<HashMap<WorkspaceId, tokio::sync::watch::Receiver<bool>>>>,
}

impl Drop for SetupRunning {
    fn drop(&mut self) {
        let _ = self.done.send(true);
        let mut setups = lock(&self.setups);
        if setups
            .get(&self.workspace)
            .is_some_and(|receiver| receiver.same_channel(&self.done.subscribe()))
        {
            setups.remove(&self.workspace);
        }
    }
}

/// The places of a project's runs under `[runs] max_parallel_runs`. Every run that holds a place counts, with or without a limit of its own; a run with a limit waits in the order it asked until fewer runs than its limit hold one, and a run without one takes its place at once, so a file without `[runs]` changes nothing.
#[derive(Debug, Default)]
pub struct RunSlots {
    state: Mutex<SlotState>,
    changed: tokio::sync::watch::Sender<u64>,
}

#[derive(Debug, Default)]
struct SlotState {
    next: u64,
    /// How many runs hold a place.
    holding: usize,
    /// The tickets that hold a place.
    granted: std::collections::HashSet<u64>,
    /// The tickets that wait, in the order they asked, each with its limit.
    queue: std::collections::VecDeque<(u64, NonZeroUsize)>,
    /// The ticket of each run that has one.
    runs: HashMap<CheckRunId, u64>,
}

impl SlotState {
    /// Gives a place to each ticket at the front of the queue while fewer runs than its limit hold one.
    fn grant(&mut self) -> bool {
        let mut moved = false;
        while let Some((number, limit)) = self.queue.front().copied() {
            if self.holding >= limit.get() {
                break;
            }
            self.queue.pop_front();
            self.holding += 1;
            self.granted.insert(number);
            moved = true;
        }
        moved
    }

    fn position(&self, number: u64) -> Option<usize> {
        self.queue
            .iter()
            .position(|(waiting, _)| *waiting == number)
            .map(|index| index + 1)
    }
}

impl RunSlots {
    /// A ticket for a run under `limit`, which holds a place at once when the run has no limit or fewer runs than its limit hold one and none waits before it.
    pub fn ticket(self: &Arc<Self>, limit: Option<NonZeroUsize>) -> SlotTicket {
        let mut state = lock(&self.state);
        let number = state.next;
        state.next += 1;
        match limit {
            None => {
                state.holding += 1;
                state.granted.insert(number);
            }
            Some(limit) => {
                state.queue.push_back((number, limit));
                state.grant();
            }
        }
        SlotTicket {
            slots: Arc::clone(self),
            number,
        }
    }

    /// The place in the queue of `run`, from 1 at the front; `None` when it holds a place or has no ticket.
    pub fn waiting(&self, run: CheckRunId) -> Option<usize> {
        let state = lock(&self.state);
        state
            .runs
            .get(&run)
            .and_then(|number| state.position(*number))
    }

    fn notify(&self) {
        self.changed
            .send_modify(|count| *count = count.wrapping_add(1));
    }
}

/// A run's place, or its wait for one, under `[runs] max_parallel_runs`. Dropping it gives the place, or the place in the queue, to the next run.
#[derive(Debug)]
pub struct SlotTicket {
    slots: Arc<RunSlots>,
    number: u64,
}

impl SlotTicket {
    /// The place in the queue, from 1 at the front; `None` once the ticket holds a place.
    pub fn position(&self) -> Option<usize> {
        lock(&self.slots.state).position(self.number)
    }

    /// Names the run the ticket is for, so the run shows its place in the queue.
    pub fn bind(&self, run: CheckRunId) {
        lock(&self.slots.state).runs.insert(run, self.number);
    }

    /// Waits until the ticket holds a place.
    pub async fn ready(&self) {
        let mut changed = self.slots.changed.subscribe();
        loop {
            if lock(&self.slots.state).granted.contains(&self.number) {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Drop for SlotTicket {
    fn drop(&mut self) {
        let mut state = lock(&self.slots.state);
        if state.granted.remove(&self.number) {
            state.holding = state.holding.saturating_sub(1);
        } else {
            state.queue.retain(|(waiting, _)| *waiting != self.number);
        }
        let number = self.number;
        state.runs.retain(|_, ticket| *ticket != number);
        state.grant();
        drop(state);
        self.slots.notify();
    }
}

/// The permits of each check with a `max_parallel`, by check name, with the limit they were made for.
type CheckLimits = Arc<Mutex<HashMap<String, (NonZeroUsize, Arc<Semaphore>)>>>;

type Waits = Arc<Mutex<HashMap<CheckRunId, (Waiting, Timestamp)>>>;

/// What a running run waits for. A run waits for memory before its first command and for a check's limit only before that check, so it never waits for both.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Waiting {
    Memory,
    Check(String),
}

/// A queued integration that waits for a valid checks file on main. Its stored request keeps no end, so a daemon start serves it again; while the daemon runs, the next revision of main does.
#[derive(Debug, Clone)]
pub(crate) struct HeldRequest {
    pub id: i64,
    pub actor: fold_core::Actor,
    pub proposal: fold_core::ProposalId,
    pub version: u32,
    pub expected_version: Option<u32>,
}

/// The queued integrations of a project in the order they were asked for: each takes a ticket when it is queued and runs when the queue serves that ticket.
#[derive(Debug)]
struct Queue {
    next: std::sync::atomic::AtomicU64,
    serving: tokio::sync::watch::Sender<u64>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(0),
            serving: tokio::sync::watch::Sender::new(0),
        }
    }
}

/// A place in a project's integration queue. Dropping it, whether its integration ended or was aborted, serves the next ticket.
#[derive(Debug)]
pub(crate) struct Ticket {
    number: u64,
    queue: Arc<Queue>,
}

impl Ticket {
    /// Waits until every integration queued before this one has ended.
    pub(crate) async fn turn(&self) {
        let mut serving = self.queue.serving.subscribe();
        let _ = serving.wait_for(|now| *now >= self.number).await;
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let next = self.number.saturating_add(1);
        self.queue.serving.send_if_modified(|now| {
            if *now < next {
                *now = next;
                true
            } else {
                false
            }
        });
    }
}

impl CheckRunner {
    pub fn new(permits: CheckPermits) -> Self {
        Self {
            project: String::new(),
            permits,
            limits: RunLimits::default(),
            tasks: Arc::default(),
            check_dir: Arc::default(),
            dir_work: Arc::default(),
            queue: Arc::default(),
            held: Arc::default(),
            memory_waits: Arc::default(),
            check_limits: Arc::default(),
            cancels: Arc::default(),
            setups: Arc::default(),
            slots: Arc::default(),
        }
    }

    /// Makes `run` one that a cancel reaches until the watch is dropped.
    pub(crate) fn watch_cancel(&self, run: CheckRunId) -> CancelWatch {
        let (send, request) = tokio::sync::oneshot::channel();
        lock(&self.cancels).insert(run, send);
        CancelWatch {
            run,
            cancels: Arc::clone(&self.cancels),
            request: Some(request),
        }
    }

    /// Sends `request` to `run`; the request comes back when no running run of the project listens for it.
    pub(crate) fn cancel(
        &self,
        run: CheckRunId,
        request: CancelRequest,
    ) -> Result<(), CancelRequest> {
        let Some(send) = lock(&self.cancels).remove(&run) else {
            return Err(request);
        };
        send.send(request)
    }

    /// Marks the setup of `workspace` as running until the returned guard drops.
    pub(crate) fn setup_started(&self, workspace: WorkspaceId) -> SetupRunning {
        let (done, receiver) = tokio::sync::watch::channel(false);
        lock(&self.setups).insert(workspace, receiver);
        SetupRunning {
            workspace,
            done,
            setups: Arc::clone(&self.setups),
        }
    }

    /// Waits until no setup runs in `workspace`.
    pub(crate) async fn setup_finished(&self, workspace: WorkspaceId) {
        let receiver = lock(&self.setups).get(&workspace).cloned();
        if let Some(mut receiver) = receiver {
            let _ = receiver.wait_for(|done| *done).await;
        }
    }

    /// A ticket of the project's run places under `limit`, the `[runs] max_parallel_runs` of the run's checks file.
    pub fn slot_ticket(&self, limit: Option<NonZeroUsize>) -> SlotTicket {
        self.slots.ticket(limit)
    }

    /// The place of `run` in the queue for a run place, from 1 at the front; `None` when it does not wait for one.
    pub fn waiting_for_run_slot(&self, run: CheckRunId) -> Option<usize> {
        self.slots.waiting(run)
    }

    /// The runner of the project `slug`. A runner without one runs checks that act as no agent of any project.
    #[must_use]
    pub fn with_project(mut self, slug: &str) -> Self {
        slug.clone_into(&mut self.project);
        self
    }

    /// Who the commands of a run act for when they call fold: the agent of the worker that asked for the run, in this project, or no agent.
    pub fn owner(&self, agent: Option<AgentName>) -> PeerWorker {
        PeerWorker {
            project: self.project.clone(),
            agent,
        }
    }

    /// The runner with other command limits, for tests.
    #[must_use]
    pub fn with_limits(mut self, limits: RunLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> RunLimits {
        self.limits
    }

    pub fn permits(&self) -> &CheckPermits {
        &self.permits
    }

    /// The machine-wide permit of `run`, once the machine has the free memory for it: while it has not, the run keeps its permit, shows as waiting for memory, logs that once and reads the memory again on every tick. A failed read starts the run, so a broken `/proc` never stops the checks.
    pub(crate) async fn start_permit(
        &self,
        run: CheckRunId,
        clock: &dyn Clock,
    ) -> Option<OwnedSemaphorePermit> {
        let permit = self.permits.acquire().await;
        let gate = &self.permits.memory;
        let Some(source) = gate.source.clone() else {
            return permit;
        };
        let mut wait = None;
        loop {
            let read = Arc::clone(&source);
            let info = tokio::task::spawn_blocking(move || read())
                .await
                .ok()
                .flatten();
            let Some(info) = info else {
                return permit;
            };
            if memory::enough_memory(info, gate.floor) {
                return permit;
            }
            if wait.is_none() {
                tracing::info!(
                    run = %run,
                    available_mib = info.available >> 20,
                    floor_mib = memory::memory_floor(info.total, gate.floor) >> 20,
                    "the check run waits for free memory before its commands start"
                );
                lock(&self.memory_waits).insert(run, (Waiting::Memory, clock.now()));
                wait = Some(MemoryWait {
                    waits: Arc::clone(&self.memory_waits),
                    run,
                });
            }
            tokio::time::sleep(gate.tick).await;
        }
    }

    /// Since when `run` holds its permit and waits for free memory; `None` when it does not wait.
    pub fn waiting_for_memory(&self, run: CheckRunId) -> Option<Timestamp> {
        match lock(&self.memory_waits).get(&run) {
            Some((Waiting::Memory, since)) => Some(*since),
            _ => None,
        }
    }

    /// The check whose `max_parallel` `run` waits for before its command starts, and since when; `None` when it does not wait.
    pub fn waiting_for_check(&self, run: CheckRunId) -> Option<(String, Timestamp)> {
        match lock(&self.memory_waits).get(&run) {
            Some((Waiting::Check(name), since)) => Some((name.clone(), *since)),
            _ => None,
        }
    }

    /// The per-check limits of the project as `run` meets them.
    pub(crate) fn gate(&self, run: CheckRunId) -> RunGate {
        RunGate {
            run,
            limits: Arc::clone(&self.check_limits),
            waits: Arc::clone(&self.memory_waits),
        }
    }

    /// A place at the end of the project's integration queue, taken at once so the order is the order of the calls.
    pub(crate) fn ticket(&self) -> Ticket {
        let number = self
            .queue
            .next
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ticket {
            number,
            queue: Arc::clone(&self.queue),
        }
    }

    /// Keeps a queued integration until main's checks file is valid again.
    pub(crate) fn hold(&self, request: HeldRequest) {
        lock(&self.held).push(request);
    }

    /// Every held integration, in the order it was held; the list is left empty.
    pub(crate) fn take_held(&self) -> Vec<HeldRequest> {
        std::mem::take(&mut *lock(&self.held))
    }

    /// The lock of the project's check directory.
    pub(crate) fn check_dir(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.check_dir)
    }

    /// A share of the check directory for one blocking task in it, which the task holds until it ends.
    pub(crate) async fn dir_work(&self) -> OwnedRwLockReadGuard<()> {
        Arc::clone(&self.dir_work).read_owned().await
    }

    /// Runs `work` in the background. Finished runs are dropped from the set here, so it holds only those still going.
    pub(crate) fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = lock(&self.tasks);
        while tasks.try_join_next().is_some() {}
        tasks.spawn(work);
    }

    /// Waits until every run started so far has ended, whether it ended its row or failed to.
    pub async fn wait_idle(&self) {
        loop {
            let mut tasks = std::mem::take(&mut *lock(&self.tasks));
            if tasks.is_empty() {
                return;
            }
            while tasks.join_next().await.is_some() {}
        }
    }

    /// Aborts every run still going, then waits until no blocking task writes or reads the check directory any more, so the next open of the project finds the directory at rest. A run cut short leaves its row running, which the next open of the project ends as stopped.
    pub async fn shutdown(&self) {
        let mut tasks = std::mem::take(&mut *lock(&self.tasks));
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        drop(self.dir_work.write().await);
    }
}

/// A run's place among the runs that wait, left when the wait ends or the run is aborted.
struct MemoryWait {
    waits: Waits,
    run: CheckRunId,
}

/// The per-check limits of a project as one run meets them: before the command of a check with `max_parallel`, the run takes one of the check's permits and holds it until the command ends.
#[derive(Debug, Clone)]
pub struct RunGate {
    run: CheckRunId,
    limits: CheckLimits,
    waits: Waits,
}

impl RunGate {
    /// The permit of `check` under `max`, once fewer than `max` runs of the project run its command. While it waits, the run shows as waiting for the check. A changed `max` starts a new set of permits for the check; the runs that hold the old ones end as they are.
    async fn enter(
        &self,
        check: &str,
        max: NonZeroUsize,
        clock: &dyn Clock,
    ) -> Option<OwnedSemaphorePermit> {
        let permits = {
            let mut limits = lock(&self.limits);
            let entry = limits
                .entry(check.to_owned())
                .or_insert_with(|| (max, Arc::new(Semaphore::new(max.get()))));
            if entry.0 != max {
                *entry = (max, Arc::new(Semaphore::new(max.get())));
            }
            Arc::clone(&entry.1)
        };
        if let Ok(permit) = Arc::clone(&permits).try_acquire_owned() {
            return Some(permit);
        }
        tracing::info!(run = %self.run, check, max = max.get(), "the check run waits for the max_parallel of a check");
        lock(&self.waits).insert(self.run, (Waiting::Check(check.to_owned()), clock.now()));
        let _wait = MemoryWait {
            waits: Arc::clone(&self.waits),
            run: self.run,
        };
        permits.acquire_owned().await.ok()
    }
}

impl Drop for MemoryWait {
    fn drop(&mut self) {
        lock(&self.waits).remove(&self.run);
    }
}

/// One command of a run: the setup (`check` is `None`) or a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub check: Option<String>,
    pub run: String,
    pub required: bool,
    /// False for a check whose `files` pattern matched no changed path.
    pub runs: bool,
    /// The check's own `timeout`; `None` takes the runner's.
    pub timeout: Option<Duration>,
    /// The most runs of the project that run this check's command at once; `None` is no limit.
    pub max_parallel: Option<NonZeroUsize>,
}

/// What one command did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub check: Option<String>,
    pub required: bool,
    pub outcome: Outcome,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// The end of the combined output, at most the log limit.
    pub log: Vec<u8>,
    /// The size of the whole output.
    pub log_bytes: u64,
    /// fold could not start the command or wait for it, so its failure is fold's and says nothing of the tree.
    pub fault: bool,
    /// The signal that ended the command when fold did not send it, such as the SIGKILL of the kernel's OOM killer, so its failure says nothing of the tree either.
    pub killed: Option<i32>,
}

impl CommandResult {
    /// A required command that failed or ran out of time fails its run.
    pub fn blocks(&self) -> bool {
        self.required && self.outcome.is_failure()
    }
}

/// The commands of one run in order: the setup first when the file has one, then each check of the plan.
pub fn commands(file: &ChecksFile, plan: &[Planned<'_>]) -> Vec<CommandSpec> {
    let setup = file.setup.iter().map(|run| CommandSpec {
        check: None,
        run: run.clone(),
        required: true,
        runs: true,
        timeout: None,
        max_parallel: None,
    });
    let checks = plan.iter().map(|planned| CommandSpec {
        check: Some(planned.check.name.clone()),
        run: planned.check.run.clone(),
        required: planned.check.required,
        runs: planned.runs,
        timeout: planned.check.timeout,
        max_parallel: planned.check.max_parallel,
    });
    setup.chain(checks).collect()
}

/// [`run_commands_for`] of a run that no worker asked for: a process of its commands acts as no agent and never as a human.
pub async fn run_commands(
    dir: &Path,
    specs: Vec<CommandSpec>,
    limits: RunLimits,
    clock: &dyn Clock,
) -> Vec<CommandResult> {
    let nobody = PeerWorker {
        project: String::new(),
        agent: None,
    };
    run_commands_for(&nobody, dir, specs, limits, clock).await
}

/// [`run_commands`] of `gate`'s run: a check with `max_parallel` waits for its limit before its command.
pub async fn run_gated_commands(
    gate: &RunGate,
    dir: &Path,
    specs: Vec<CommandSpec>,
    limits: RunLimits,
    clock: &dyn Clock,
) -> Vec<CommandResult> {
    let nobody = PeerWorker {
        project: String::new(),
        agent: None,
    };
    run_gated_commands_for(Some(gate), &nobody, dir, specs, limits, clock).await
}

/// Runs `specs` one after another in `dir`. A skipped check is not started; after a failed setup, and after a command that a signal fold did not send ended, no command is started. Every command is a `sh -c` of its own in a process group of its own, registered for the peer checks while it runs: a process under it that calls fold acts as `owner` and never as a human.
pub async fn run_commands_for(
    owner: &PeerWorker,
    dir: &Path,
    specs: Vec<CommandSpec>,
    limits: RunLimits,
    clock: &dyn Clock,
) -> Vec<CommandResult> {
    run_gated_commands_for(None, owner, dir, specs, limits, clock).await
}

/// [`run_commands_for`] under `gate`, when the run has one: a check with `max_parallel` waits for its limit before its command, and the wait counts in neither its duration nor its timeout.
pub async fn run_gated_commands_for(
    gate: Option<&RunGate>,
    owner: &PeerWorker,
    dir: &Path,
    specs: Vec<CommandSpec>,
    limits: RunLimits,
    clock: &dyn Clock,
) -> Vec<CommandResult> {
    let mut results = Vec::with_capacity(specs.len());
    let mut stopped = false;
    for spec in specs {
        let skipped = |outcome| CommandResult {
            check: spec.check.clone(),
            required: spec.required,
            outcome,
            exit_code: None,
            duration_ms: 0,
            log: Vec::new(),
            log_bytes: 0,
            fault: false,
            killed: None,
        };
        if stopped {
            results.push(skipped(Outcome::NotRun));
            continue;
        }
        if !spec.runs {
            results.push(skipped(Outcome::Skipped));
            continue;
        }
        let limit = match (gate, &spec.check, spec.max_parallel) {
            (Some(gate), Some(check), Some(max)) => gate.enter(check, max, clock).await,
            _ => None,
        };
        let started = clock.now();
        let own = RunLimits {
            timeout: spec.timeout.unwrap_or(limits.timeout),
            ..limits
        };
        let ran = run_command(dir.to_path_buf(), &spec.run, own, owner).await;
        drop(limit);
        let duration_ms = clock
            .now()
            .as_millis()
            .saturating_sub(started.as_millis())
            .try_into()
            .unwrap_or(0);
        let result = CommandResult {
            check: spec.check.clone(),
            required: spec.required,
            outcome: ran.outcome,
            exit_code: ran.exit_code,
            duration_ms,
            log: ran.log,
            log_bytes: ran.log_bytes,
            fault: ran.fault,
            killed: ran.killed,
        };
        stopped = (spec.check.is_none() && result.outcome.is_failure()) || result.killed.is_some();
        results.push(result);
    }
    results
}

struct Ran {
    outcome: Outcome,
    exit_code: Option<i32>,
    log: Vec<u8>,
    log_bytes: u64,
    fault: bool,
    killed: Option<i32>,
}

/// The end of a stream of output, at most `limit` bytes, and the size of the whole.
#[derive(Debug)]
struct Tail {
    bytes: Vec<u8>,
    total: u64,
    limit: usize,
}

impl Tail {
    fn push(&mut self, chunk: &[u8]) {
        self.total = self.total.saturating_add(chunk.len() as u64);
        self.bytes.extend_from_slice(chunk);
        // Trimmed only once the buffer holds twice the limit, so a long output is not moved on every chunk.
        if self.bytes.len() > self.limit.saturating_mul(2) {
            let cut = self.bytes.len() - self.limit;
            self.bytes.drain(..cut);
        }
    }

    fn finish(mut self) -> (Vec<u8>, u64) {
        if self.bytes.len() > self.limit {
            let cut = self.bytes.len() - self.limit;
            self.bytes.drain(..cut);
        }
        (self.bytes, self.total)
    }
}

async fn read_into(mut stream: impl AsyncRead + Unpin, tail: Arc<Mutex<Tail>>) {
    let mut chunk = vec![0_u8; 8192];
    while let Ok(read) = stream.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        lock(&tail).push(&chunk[..read]);
    }
}

fn note(tail: &Arc<Mutex<Tail>>, text: &str) {
    lock(tail).push(text.as_bytes());
}

async fn run_command(dir: PathBuf, run: &str, limits: RunLimits, owner: &PeerWorker) -> Ran {
    let tail = Arc::new(Mutex::new(Tail {
        bytes: Vec::new(),
        total: 0,
        limit: limits.log_limit,
    }));
    let mut command = shell(&dir, run);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            note(
                &tail,
                &format!("fold could not start the command: {}\n", error.kind()),
            );
            let (log, log_bytes) = finish(tail);
            return Ran {
                outcome: Outcome::Failed,
                exit_code: None,
                log,
                log_bytes,
                fault: true,
                killed: None,
            };
        }
    };
    let _peer = child
        .id()
        .map(|pid| peers::register_check(pid, owner.clone()));
    let group = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw);
    let guard = GroupKill(group);
    let mut readers = JoinSet::new();
    if let Some(stdout) = child.stdout.take() {
        readers.spawn(read_into(stdout, Arc::clone(&tail)));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.spawn(read_into(stderr, Arc::clone(&tail)));
    }
    let (outcome, exit_code, fault, killed) =
        match tokio::time::timeout(limits.timeout, child.wait()).await {
            Ok(Ok(status)) => {
                let outcome = if status.success() {
                    Outcome::Passed
                } else {
                    Outcome::Failed
                };
                let killed = killed_by_signal(status);
                if let Some(signal) = killed {
                    note(&tail, &killed_note(signal));
                }
                (outcome, status.code(), false, killed)
            }
            Ok(Err(error)) => {
                note(
                    &tail,
                    &format!("\nfold could not wait for the command: {}\n", error.kind()),
                );
                (Outcome::Failed, None, true, None)
            }
            Err(_) => {
                signal(group, rustix::process::Signal::TERM);
                if tokio::time::timeout(limits.kill_grace, child.wait())
                    .await
                    .is_err()
                {
                    signal(group, rustix::process::Signal::KILL);
                    let _ = child.wait().await;
                }
                note(
                    &tail,
                    &format!(
                        "\nfold stopped the command after {} s\n",
                        limits.timeout.as_secs()
                    ),
                );
                (Outcome::TimedOut, None, false, None)
            }
        };
    drop(guard);
    let drained = tokio::time::timeout(DRAIN_WAIT, async {
        while readers.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        readers.abort_all();
    }
    let (log, log_bytes) = finish(tail);
    Ran {
        outcome,
        exit_code,
        log,
        log_bytes,
        fault,
        killed,
    }
}

/// The line a command's log ends with when a signal that fold did not send ended it.
fn killed_note(signal: i32) -> String {
    format!("\nkilled: out of memory or by a signal (signal {signal})\n")
}

/// The `oom_score_adj` of a check command's shell, which every process it starts inherits: the kernel's OOM killer ends a check before the daemon or a worker, whose score stays near 0.
pub const CHECK_OOM_SCORE_ADJ: &str = "800";

/// The signal that ended the command: any signal that ended its shell, or a SIGKILL or SIGTERM that ended the process the shell waited for last, which the shell reports as exit 137 or 143. fold signals a command only after a timeout or at a cancel, whose status this never reads, so this one came from the kernel's OOM killer or another process.
fn killed_by_signal(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;
    if let Some(signal) = status.signal() {
        return Some(signal);
    }
    let code = status.code()?;
    [rustix::process::Signal::KILL, rustix::process::Signal::TERM]
        .into_iter()
        .map(rustix::process::Signal::as_raw)
        .find(|signal| code == 128 + signal)
}

/// Kills the command's process group when it is dropped: at the command's end, so what it left running, such as a server a test started, ends with it, and when a stopping daemon aborts the run, which `kill_on_drop` alone does only for the shell.
struct GroupKill(Option<rustix::process::Pid>);

impl Drop for GroupKill {
    fn drop(&mut self) {
        signal(self.0, rustix::process::Signal::KILL);
    }
}

fn signal(group: Option<rustix::process::Pid>, signal: rustix::process::Signal) {
    if let Some(pid) = group {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

/// The shell on Linux raises its own OOM score, then `exec`s the `sh -c` of the command in the same process, so the score is set before the command's first instruction and every child inherits it. The command keeps its text, its `$0`, its empty arguments, its pid and its exit code; a failed write is silent and the command runs as it is.
#[cfg(target_os = "linux")]
fn shell_args(run: &str) -> [String; 4] {
    [
        "-c".to_owned(),
        format!(
            "{{ echo {CHECK_OOM_SCORE_ADJ} >/proc/self/oom_score_adj; }} 2>/dev/null; exec /bin/sh -c \"$1\""
        ),
        "/bin/sh".to_owned(),
        run.to_owned(),
    ]
}

#[cfg(not(target_os = "linux"))]
fn shell_args(run: &str) -> [String; 2] {
    ["-c".to_owned(), run.to_owned()]
}

/// The `sh -c` of one command in `dir`, with stdin closed and without the daemon's session and identity variables.
fn shell(dir: &Path, run: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(shell_args(run))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The command leads a process group of its own, so a timeout or the end of the command reaches every process it started.
        .process_group(0)
        .kill_on_drop(true);
    remove_daemon_context(&mut command);
    for name in IDENTITY_ENV {
        command.env_remove(name);
    }
    // Every check folder is a folder of fold's without a `.git`, below the project's repository; its parent as the ceiling keeps `git` out of that repository wherever fold puts the folder.
    confine_git(&mut command, dir.parent().unwrap_or(dir));
    command
}

fn finish(tail: Arc<Mutex<Tail>>) -> (Vec<u8>, u64) {
    let tail = std::mem::replace(
        &mut *lock(&tail),
        Tail {
            bytes: Vec::new(),
            total: 0,
            limit: 0,
        },
    );
    tail.finish()
}

#[cfg(test)]
mod tests {
    use fold_core::ManualClock;

    use crate::support::{SERVICE_ENV, SESSION_ENV};

    use super::*;

    fn spec(check: &str, run: &str) -> CommandSpec {
        CommandSpec {
            check: Some(check.to_owned()),
            run: run.to_owned(),
            required: true,
            runs: true,
            timeout: None,
            max_parallel: None,
        }
    }

    fn run(number: u32) -> CheckRunId {
        CheckRunId::from_number(number).unwrap()
    }

    #[tokio::test]
    async fn runs_over_the_limit_wait_in_order_and_start_when_a_place_frees() {
        let runner = CheckRunner::new(CheckPermits::new(4));
        let two = NonZeroUsize::new(2);
        let tickets: Vec<SlotTicket> = (1..=4)
            .map(|number| {
                let ticket = runner.slot_ticket(two);
                ticket.bind(run(number));
                ticket
            })
            .collect();
        let positions: Vec<_> = tickets.iter().map(SlotTicket::position).collect();
        assert_eq!(positions, [None, None, Some(1), Some(2)]);
        assert_eq!(runner.waiting_for_run_slot(run(3)), Some(1));
        assert_eq!(runner.waiting_for_run_slot(run(4)), Some(2));
        assert_eq!(runner.waiting_for_run_slot(run(1)), None);
        tokio::time::timeout(Duration::from_secs(1), tickets[0].ready())
            .await
            .unwrap();
        let mut tickets = tickets.into_iter();
        let first = tickets.next().unwrap();
        let second = tickets.next().unwrap();
        let third = tickets.next().unwrap();
        let fourth = tickets.next().unwrap();
        let waiter = tokio::spawn(async move {
            third.ready().await;
            third
        });
        drop(first);
        let third = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(third.position(), None);
        assert_eq!(fourth.position(), Some(1));
        drop(fourth);
        assert_eq!(runner.waiting_for_run_slot(run(4)), None);
        drop((second, third));
        let after = runner.slot_ticket(two);
        assert_eq!(after.position(), None);
    }

    #[test]
    fn a_run_without_a_limit_never_waits_and_counts_for_the_others() {
        let runner = CheckRunner::new(CheckPermits::new(4));
        let mut free: Vec<SlotTicket> = (0..3).map(|_| runner.slot_ticket(None)).collect();
        assert!(free.iter().all(|ticket| ticket.position().is_none()));
        let limited = runner.slot_ticket(NonZeroUsize::new(3));
        assert_eq!(limited.position(), Some(1));
        let behind = runner.slot_ticket(None);
        assert_eq!(behind.position(), None);
        drop(free.pop());
        assert_eq!(limited.position(), Some(1), "three runs still hold a place");
        drop(behind);
        assert_eq!(limited.position(), None);
    }

    fn clock() -> ManualClock {
        ManualClock::new()
    }

    fn quick() -> RunLimits {
        RunLimits {
            timeout: Duration::from_secs(20),
            kill_grace: Duration::from_millis(200),
            log_limit: 1024,
        }
    }

    #[tokio::test]
    async fn a_passing_and_a_failing_command_report_their_exit() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_commands(
            dir.path(),
            vec![
                spec("ok", "echo fine"),
                spec("bad", "echo broken >&2; exit 3"),
            ],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].outcome, Outcome::Passed);
        assert_eq!(results[0].exit_code, Some(0));
        assert_eq!(results[0].log, b"fine\n");
        assert_eq!(results[1].outcome, Outcome::Failed);
        assert_eq!(results[1].exit_code, Some(3));
        assert_eq!(results[1].log, b"broken\n");
        assert!(results[1].blocks());
    }

    #[tokio::test]
    async fn git_in_a_check_folder_fails_instead_of_reaching_the_project_repository() {
        let root = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        assert!(init.success());
        let dir = root.path().join(".fold/checks/main");
        std::fs::create_dir_all(&dir).unwrap();
        let results = run_commands(
            &dir,
            vec![
                spec("git", "git rev-parse --show-toplevel"),
                spec("plain", "echo fine"),
            ],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(String::from_utf8_lossy(&results[0].log).contains("not a git repository"));
        assert_eq!(results[1].outcome, Outcome::Passed);
        assert_eq!(results[1].log, b"fine\n");
    }

    #[test]
    fn a_command_gets_no_session_or_identity_variable_of_the_daemon() {
        let command = shell(Path::new("/"), "true");
        let removed: Vec<_> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in SESSION_ENV.iter().chain(&SERVICE_ENV).chain(&IDENTITY_ENV) {
            assert!(removed.iter().any(|gone| gone == name), "{name}");
        }
        assert!(removed.iter().any(|gone| gone == "FOLD_AGENT_TAGS"));
        for name in crate::support::GIT_REPO_ENV {
            assert!(removed.iter().any(|gone| gone == name), "{name}");
        }
        assert!(
            removed
                .iter()
                .any(|gone| gone == "CLAUDE_CODE_MESSAGING_TOKEN")
        );
    }

    #[tokio::test]
    async fn a_command_that_cannot_start_is_a_fault_of_folds_and_a_red_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let results = run_commands(&gone, vec![spec("ok", "true")], quick(), &clock()).await;
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(results[0].fault);
        let results =
            run_commands(dir.path(), vec![spec("bad", "exit 1")], quick(), &clock()).await;
        assert!(!results[0].fault);
    }

    #[tokio::test]
    async fn a_command_runs_in_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "here").unwrap();
        let results = run_commands(
            dir.path(),
            vec![spec("cat", "cat marker")],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].log, b"here");
    }

    #[tokio::test]
    async fn a_command_past_its_timeout_is_stopped_with_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let limits = RunLimits {
            timeout: Duration::from_millis(300),
            ..quick()
        };
        let started = std::time::Instant::now();
        let results = run_commands(
            dir.path(),
            vec![spec("slow", "sleep 30 & sleep 30; echo never")],
            limits,
            &clock(),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(results[0].outcome, Outcome::TimedOut);
        assert_eq!(results[0].exit_code, None);
        assert!(String::from_utf8_lossy(&results[0].log).contains("fold stopped the command"));
    }

    #[tokio::test]
    async fn a_check_with_its_own_timeout_stops_at_it() {
        let dir = tempfile::tempdir().unwrap();
        let own = CommandSpec {
            timeout: Some(Duration::from_millis(300)),
            ..spec("slow", "sleep 30")
        };
        let started = std::time::Instant::now();
        let results = run_commands(dir.path(), vec![own], quick(), &clock()).await;
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(results[0].outcome, Outcome::TimedOut);
    }

    #[tokio::test]
    async fn a_large_output_keeps_its_end_and_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_commands(
            dir.path(),
            vec![spec("loud", "seq 1 20000; echo the-end")],
            quick(),
            &clock(),
        )
        .await;
        let log = &results[0].log;
        assert_eq!(log.len(), 1024);
        assert!(log.ends_with(b"20000\nthe-end\n"));
        assert!(results[0].log_bytes > 100_000);
    }

    #[tokio::test]
    async fn a_failed_setup_runs_no_check_and_a_skipped_check_never_starts() {
        let dir = tempfile::tempdir().unwrap();
        let setup = CommandSpec {
            check: None,
            ..spec("", "exit 1")
        };
        let results = run_commands(
            dir.path(),
            vec![setup, spec("test", "touch ran")],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert_eq!(results[1].outcome, Outcome::NotRun);
        assert!(!dir.path().join("ran").exists());

        let skipped = CommandSpec {
            runs: false,
            ..spec("docs", "touch ran")
        };
        let optional = CommandSpec {
            required: false,
            ..spec("lint", "exit 1")
        };
        let results = run_commands(dir.path(), vec![skipped, optional], quick(), &clock()).await;
        assert_eq!(results[0].outcome, Outcome::Skipped);
        assert!(!dir.path().join("ran").exists());
        assert_eq!(results[1].outcome, Outcome::Failed);
        assert!(
            !results[1].blocks(),
            "a check that is not required never blocks"
        );
    }

    #[tokio::test]
    async fn a_missing_directory_fails_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_commands(
            &dir.path().join("gone"),
            vec![spec("test", "true")],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].outcome, Outcome::Failed);
        assert!(String::from_utf8_lossy(&results[0].log).contains("could not start"));
    }

    #[test]
    fn the_commands_of_a_file_start_with_its_setup() {
        let file = ChecksFile::parse(
            "[setup]\nrun = \"make deps\"\n[[check]]\nname = \"t\"\nrun = \"make test\"\nfiles = \"\\\\.rs$\"\n",
        )
        .unwrap();
        let plan = file.plan(
            fold_core::reviews::checks::Stage::Proposal,
            &["a.md".to_owned()],
        );
        let specs = commands(&file, &plan);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].check, None);
        assert_eq!(specs[1].check.as_deref(), Some("t"));
        assert!(!specs[1].runs);
    }

    #[tokio::test]
    async fn queued_tickets_take_their_turn_in_order() {
        let runner = CheckRunner::default();
        let first = runner.ticket();
        let second = runner.ticket();
        first.turn().await;
        let waiting = tokio::time::timeout(Duration::from_millis(50), second.turn()).await;
        assert!(waiting.is_err(), "the second waits for the first");
        drop(first);
        tokio::time::timeout(Duration::from_secs(5), second.turn())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_shutdown_waits_for_the_blocking_work_of_a_run_it_aborts() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let runner = CheckRunner::default();
        let (started, on_start) = tokio::sync::oneshot::channel();
        let (release, on_release) = std::sync::mpsc::channel::<()>();
        let ended = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&ended);
        let inner = runner.clone();
        runner.spawn(async move {
            let work = inner.dir_work().await;
            let _ = tokio::task::spawn_blocking(move || {
                let _work = work;
                let _ = started.send(());
                let _ = on_release.recv();
                seen.store(true, Ordering::SeqCst);
            })
            .await;
        });
        on_start.await.unwrap();
        let shutdown = runner.shutdown();
        tokio::pin!(shutdown);
        let early = tokio::time::timeout(Duration::from_millis(50), &mut shutdown).await;
        assert!(early.is_err(), "the blocking work still uses the directory");
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap();
        assert!(ended.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn permits_limit_the_runs_that_go_at_once() {
        let permits = CheckPermits::new(1);
        let first = permits.acquire().await.unwrap();
        let second = tokio::time::timeout(Duration::from_millis(50), permits.acquire()).await;
        assert!(second.is_err(), "the only permit is taken");
        drop(first);
        assert!(permits.acquire().await.is_some());
        assert!(
            CheckPermits::new(0).acquire().await.is_some(),
            "at least one run"
        );
    }

    #[tokio::test]
    async fn a_check_run_waits_for_the_setup_of_its_folder_only() {
        let runner = CheckRunner::default();
        let folder = WorkspaceId::from_number(1).unwrap();
        let other = WorkspaceId::from_number(2).unwrap();
        tokio::time::timeout(Duration::from_secs(5), runner.setup_finished(folder))
            .await
            .expect("no setup runs in the folder");

        let first = runner.setup_started(folder);
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), runner.setup_finished(folder)).await;
        assert!(blocked.is_err(), "the folder's setup runs");
        tokio::time::timeout(Duration::from_secs(5), runner.setup_finished(other))
            .await
            .expect("another folder's setup does not hold this folder");

        let waiting = runner.setup_finished(folder);
        tokio::pin!(waiting);
        let started = tokio::time::timeout(Duration::from_millis(10), &mut waiting).await;
        assert!(started.is_err(), "the wait found the first setup");
        let second = runner.setup_started(folder);
        drop(first);
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("a wait ends when the setup it found ends");
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), runner.setup_finished(folder)).await;
        assert!(
            blocked.is_err(),
            "the end of the first setup keeps the entry of the second"
        );
        drop(second);
        tokio::time::timeout(Duration::from_secs(5), runner.setup_finished(folder))
            .await
            .expect("no setup runs after both ended");
        assert!(lock(&runner.setups).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_check_command_runs_with_a_high_oom_score() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_commands(
            dir.path(),
            vec![spec("oom", "cat /proc/self/oom_score_adj")],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(
            String::from_utf8_lossy(&results[0].log).trim(),
            CHECK_OOM_SCORE_ADJ
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_child_that_a_check_command_starts_at_once_has_the_high_oom_score() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_commands(
            dir.path(),
            vec![spec(
                "child",
                "cat /proc/self/oom_score_adj & wait; sh -c 'cat /proc/self/oom_score_adj'",
            )],
            quick(),
            &clock(),
        )
        .await;
        let log = String::from_utf8_lossy(&results[0].log).into_owned();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines, [CHECK_OOM_SCORE_ADJ, CHECK_OOM_SCORE_ADJ], "{log}");
    }

    #[tokio::test]
    async fn a_command_keeps_its_text_arguments_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_commands(
            dir.path(),
            vec![spec(
                "args",
                "echo \"$0|$#|$1\"; echo 'it'\\''s \"q\"'; exit 7",
            )],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].exit_code, Some(7));
        assert_eq!(results[0].log, b"/bin/sh|0|\nit's \"q\"\n");
    }

    #[tokio::test]
    async fn a_command_that_an_outside_signal_ended_is_killed_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        for (run, signal) in [
            ("kill -KILL $$", 9),
            ("sh -c 'kill -KILL $$'", 9),
            ("kill -TERM $$", 15),
            ("sh -c 'kill -TERM $$'", 15),
            ("kill -INT $$", 2),
        ] {
            let results =
                run_commands(dir.path(), vec![spec("killed", run)], quick(), &clock()).await;
            let killed = &results[0];
            assert_eq!(killed.outcome, Outcome::Failed, "{run}");
            assert_eq!(killed.killed, Some(signal), "{run}");
            assert!(!killed.fault);
            assert!(
                String::from_utf8_lossy(&killed.log).contains(&format!(
                    "killed: out of memory or by a signal (signal {signal})"
                )),
                "{run}"
            );
        }
        let results =
            run_commands(dir.path(), vec![spec("red", "exit 1")], quick(), &clock()).await;
        assert_eq!(results[0].killed, None);
    }

    #[tokio::test]
    async fn no_command_starts_after_an_outside_signal_ended_one() {
        let dir = tempfile::tempdir().unwrap();
        let optional = CommandSpec {
            required: false,
            ..spec("first", "kill -KILL $$")
        };
        let results = run_commands(
            dir.path(),
            vec![optional, spec("later", "touch ran")],
            quick(),
            &clock(),
        )
        .await;
        assert_eq!(results[0].killed, Some(9));
        assert_eq!(results[1].outcome, Outcome::NotRun);
        assert!(!dir.path().join("ran").exists());
    }

    #[tokio::test]
    async fn a_cancel_reaches_a_watched_run_once_and_none_after_its_watch() {
        let runner = CheckRunner::default();
        let run = CheckRunId::from_number(3).unwrap();
        let request = |ended| CancelRequest {
            by: fold_core::EventActor::fold(),
            ended,
        };
        let (ended, _) = tokio::sync::oneshot::channel();
        assert!(
            runner.cancel(run, request(ended)).is_err(),
            "nothing watches"
        );
        let mut watch = runner.watch_cancel(run);
        let (ended, answer) = tokio::sync::oneshot::channel();
        runner.cancel(run, request(ended)).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), watch.next())
            .await
            .unwrap();
        got.ended.send(()).unwrap();
        answer.await.unwrap();
        let (ended, _) = tokio::sync::oneshot::channel();
        assert!(
            runner.cancel(run, request(ended)).is_err(),
            "a run is cancelled once"
        );
        let again = tokio::time::timeout(Duration::from_millis(50), watch.next()).await;
        assert!(again.is_err(), "a used watch never resolves again");
        drop(watch);
        let _other = runner.watch_cancel(CheckRunId::from_number(4).unwrap());
        let (ended, _) = tokio::sync::oneshot::channel();
        assert!(runner.cancel(run, request(ended)).is_err());
    }

    #[tokio::test]
    async fn a_timeout_is_not_a_kill() {
        let dir = tempfile::tempdir().unwrap();
        let limits = RunLimits {
            timeout: Duration::from_millis(200),
            kill_grace: Duration::from_millis(100),
            ..quick()
        };
        let results = run_commands(
            dir.path(),
            vec![spec("stubborn", "trap '' TERM; sleep 30")],
            limits,
            &clock(),
        )
        .await;
        assert_eq!(results[0].outcome, Outcome::TimedOut);
        assert_eq!(results[0].killed, None);
    }

    #[test]
    fn the_host_setting_sets_the_number_of_permits() {
        let three = CheckPermits::from_settings(CheckSettings {
            max_runs: NonZeroUsize::new(3),
            memory_floor: None,
        });
        assert_eq!(three.available(), 3);
        assert_eq!(
            CheckPermits::from_settings(CheckSettings::default()).available(),
            half_the_processors()
        );
    }

    #[tokio::test]
    async fn a_run_waits_for_memory_with_its_permit_and_starts_when_it_frees() {
        use std::sync::atomic::{AtomicU64, Ordering};
        const GIB: u64 = 1024 * 1024 * 1024;
        let free = Arc::new(AtomicU64::new(GIB));
        let read = Arc::clone(&free);
        let source: MemorySource = Arc::new(move || {
            Some(memory::MemInfo {
                total: 16 * GIB,
                available: read.load(Ordering::SeqCst),
            })
        });
        let permits =
            CheckPermits::new(1).with_memory_gate(source, Some(2 * GIB), Duration::from_millis(10));
        let runner = CheckRunner::new(permits);
        let run = CheckRunId::from_number(7).unwrap();
        let clock = clock();
        let waiting = runner.clone();
        let start = tokio::spawn(async move { waiting.start_permit(run, &clock).await.is_some() });
        tokio::time::timeout(Duration::from_secs(5), async {
            while runner.waiting_for_memory(run).is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            runner.permits().available(),
            0,
            "the waiting run holds its permit"
        );
        free.store(2 * GIB, Ordering::SeqCst);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), start)
                .await
                .unwrap()
                .unwrap()
        );
        assert_eq!(runner.waiting_for_memory(run), None);
    }

    #[tokio::test]
    async fn a_check_with_max_parallel_runs_in_one_run_at_a_time_and_others_in_both() {
        let dir = tempfile::tempdir().unwrap();
        let runner = CheckRunner::new(CheckPermits::new(2));
        let heavy = CommandSpec {
            max_parallel: NonZeroUsize::new(1),
            ..spec(
                "heavy",
                "mkdir lock || echo overlap >> overlaps; until [ -e go ]; do sleep 0.02; done; rmdir lock",
            )
        };
        let runs = [("a", "b", 1), ("b", "a", 2)].map(|(own, other, number)| {
            let light = spec(
                "light",
                &format!("touch light-{own}; until [ -e light-{other} ]; do sleep 0.02; done"),
            );
            let gate = runner.gate(CheckRunId::from_number(number).unwrap());
            let specs = vec![light, heavy.clone()];
            let work = dir.path().to_path_buf();
            tokio::spawn(
                async move { run_gated_commands(&gate, &work, specs, quick(), &clock()).await },
            )
        });
        let ids = [1, 2].map(|number| CheckRunId::from_number(number).unwrap());
        let waiting = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(found) = ids
                    .iter()
                    .find_map(|run| runner.waiting_for_check(*run).map(|wait| (*run, wait.0)))
                {
                    return found;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(waiting.1, "heavy");
        assert_eq!(runner.waiting_for_memory(waiting.0), None);
        std::fs::write(dir.path().join("go"), "").unwrap();
        for run in runs {
            let results = tokio::time::timeout(Duration::from_secs(20), run)
                .await
                .unwrap()
                .unwrap();
            let outcomes: Vec<Outcome> = results.iter().map(|result| result.outcome).collect();
            assert_eq!(outcomes, [Outcome::Passed, Outcome::Passed]);
        }
        assert!(
            !dir.path().join("overlaps").exists(),
            "heavy ran twice at once"
        );
        assert!(
            ids.iter()
                .all(|run| runner.waiting_for_check(*run).is_none())
        );
    }

    #[tokio::test]
    async fn a_changed_max_parallel_takes_new_permits() {
        let runner = CheckRunner::new(CheckPermits::new(1));
        let gate = runner.gate(CheckRunId::from_number(1).unwrap());
        let one = NonZeroUsize::new(1).unwrap();
        let held = gate.enter("heavy", one, &clock()).await;
        assert!(held.is_some());
        let two = NonZeroUsize::new(2).unwrap();
        let other =
            tokio::time::timeout(Duration::from_secs(5), gate.enter("heavy", two, &clock()))
                .await
                .unwrap();
        assert!(other.is_some());
    }

    #[tokio::test]
    async fn a_failed_memory_read_starts_the_run() {
        let permits = CheckPermits::new(1).with_memory_gate(
            Arc::new(|| None),
            Some(u64::MAX),
            Duration::from_millis(10),
        );
        let runner = CheckRunner::new(permits);
        let started = tokio::time::timeout(
            Duration::from_secs(5),
            runner.start_permit(CheckRunId::from_number(1).unwrap(), &clock()),
        )
        .await
        .unwrap();
        assert!(started.is_some());
    }
}

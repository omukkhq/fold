use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fold_core::layout::{AGENT_ENV, AGENT_TAGS_ENV};
use fold_core::reviews::checks::{CHECK_TIMEOUT, ChecksFile, Outcome, Planned};
use fold_core::{AgentName, Clock};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

use crate::services::agents::peers::{self, PeerWorker};
use crate::support::{SESSION_ENV, lock};

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

/// How many runs of checks may go at the same time on this machine. Every project the daemon opens shares one set, so a machine with many projects never runs more test suites at once than half its processors.
#[derive(Debug, Clone)]
pub struct CheckPermits(Arc<Semaphore>);

impl CheckPermits {
    /// At least one run.
    pub fn new(runs: usize) -> Self {
        Self(Arc::new(Semaphore::new(runs.max(1))))
    }

    /// Half the processors the daemon may use, at least one.
    pub fn for_machine() -> Self {
        let processors = std::thread::available_parallelism().map_or(2, usize::from);
        Self::new(processors / 2)
    }

    pub(crate) async fn acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.0).acquire_owned().await.ok()
    }
}

impl Default for CheckPermits {
    fn default() -> Self {
        Self::for_machine()
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
}

impl CheckRunner {
    pub fn new(permits: CheckPermits) -> Self {
        Self {
            project: String::new(),
            permits,
            limits: RunLimits::default(),
            tasks: Arc::default(),
        }
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

    pub(crate) fn permits(&self) -> &CheckPermits {
        &self.permits
    }

    /// Runs `work` in the background. Finished runs are dropped from the set here, so it holds only those still going.
    pub(crate) fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = lock(&self.tasks);
        while tasks.try_join_next().is_some() {}
        tasks.spawn(work);
    }

    /// Aborts every run still going. A run cut short leaves its row running, which the next open of the project ends as stopped.
    pub async fn shutdown(&self) {
        let mut tasks = std::mem::take(&mut *lock(&self.tasks));
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
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
    });
    let checks = plan.iter().map(|planned| CommandSpec {
        check: Some(planned.check.name.clone()),
        run: planned.check.run.clone(),
        required: planned.check.required,
        runs: planned.runs,
        timeout: planned.check.timeout,
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

/// Runs `specs` one after another in `dir`. A skipped check is not started; after a failed setup no check is started. Every command is a `sh -c` of its own in a process group of its own, registered for the peer checks while it runs: a process under it that calls fold acts as `owner` and never as a human.
pub async fn run_commands_for(
    owner: &PeerWorker,
    dir: &Path,
    specs: Vec<CommandSpec>,
    limits: RunLimits,
    clock: &dyn Clock,
) -> Vec<CommandResult> {
    let mut results = Vec::with_capacity(specs.len());
    let mut setup_failed = false;
    for spec in specs {
        let skipped = |outcome| CommandResult {
            check: spec.check.clone(),
            required: spec.required,
            outcome,
            exit_code: None,
            duration_ms: 0,
            log: Vec::new(),
            log_bytes: 0,
        };
        if setup_failed {
            results.push(skipped(Outcome::NotRun));
            continue;
        }
        if !spec.runs {
            results.push(skipped(Outcome::Skipped));
            continue;
        }
        let started = clock.now();
        let own = RunLimits {
            timeout: spec.timeout.unwrap_or(limits.timeout),
            ..limits
        };
        let ran = run_command(dir.to_path_buf(), &spec.run, own, owner).await;
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
        };
        setup_failed = spec.check.is_none() && result.outcome.is_failure();
        results.push(result);
    }
    results
}

struct Ran {
    outcome: Outcome,
    exit_code: Option<i32>,
    log: Vec<u8>,
    log_bytes: u64,
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
    let mut readers = JoinSet::new();
    if let Some(stdout) = child.stdout.take() {
        readers.spawn(read_into(stdout, Arc::clone(&tail)));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.spawn(read_into(stderr, Arc::clone(&tail)));
    }
    let (outcome, exit_code) = match tokio::time::timeout(limits.timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let outcome = if status.success() {
                Outcome::Passed
            } else {
                Outcome::Failed
            };
            (outcome, status.code())
        }
        Ok(Err(error)) => {
            note(
                &tail,
                &format!("\nfold could not wait for the command: {}\n", error.kind()),
            );
            (Outcome::Failed, None)
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
            (Outcome::TimedOut, None)
        }
    };
    // What the command left running in its group, such as a server a test started, ends with it.
    signal(group, rustix::process::Signal::KILL);
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
    }
}

fn signal(group: Option<rustix::process::Pid>, signal: rustix::process::Signal) {
    if let Some(pid) = group {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

/// The `sh -c` of one command in `dir`, with stdin closed and without the daemon's session and identity variables.
fn shell(dir: &Path, run: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(run)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The command leads a process group of its own, so a timeout or the end of the command reaches every process it started.
        .process_group(0)
        .kill_on_drop(true);
    for name in SESSION_ENV.iter().chain(&IDENTITY_ENV) {
        command.env_remove(name);
    }
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

    use super::*;

    fn spec(check: &str, run: &str) -> CommandSpec {
        CommandSpec {
            check: Some(check.to_owned()),
            run: run.to_owned(),
            required: true,
            runs: true,
            timeout: None,
        }
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

    #[test]
    fn a_command_gets_no_session_or_identity_variable_of_the_daemon() {
        let command = shell(Path::new("/"), "true");
        let removed: Vec<_> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in SESSION_ENV.iter().chain(&IDENTITY_ENV) {
            assert!(removed.iter().any(|gone| gone == name), "{name}");
        }
        assert!(removed.iter().any(|gone| gone == "FOLD_AGENT_TAGS"));
        assert!(
            removed
                .iter()
                .any(|gone| gone == "CLAUDE_CODE_MESSAGING_TOKEN")
        );
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
}

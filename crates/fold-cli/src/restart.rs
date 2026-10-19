use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::Args;
use fold_cli::daemon_client::{self, ClientConfig, ClientError};
use fold_daemon::config::LockOwner;
use thiserror::Error;

use crate::doctor::{LockState, inspect_lock};
use crate::output;
use crate::platform::{Host, LAUNCH_AGENT_LABEL};
use crate::serve::{self, ServeError};
use crate::service::{self, ServiceError};

#[derive(Args)]
pub struct RestartArgs {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Stop the daemon and start none; the uninstaller runs it before it removes fld
    #[arg(long, hide = true)]
    stop: bool,
}

/// How long the old daemon gets to stop. Its shutdown waits for each job, the workers and the open connections in turn, each up to the daemon's drain timeout.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the new daemon gets to hold the lock and answer on the socket once it is started.
const START_TIMEOUT: Duration = Duration::from_secs(15);
const FIRST_POLL: Duration = Duration::from_millis(20);
const MAX_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Error)]
pub enum RestartError {
    #[error(transparent)]
    Serve(#[from] ServeError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error("a daemon holds {}, but fld cannot find its process", path.display())]
    NoProcess { path: PathBuf },
    #[error("{} names pid {recorded}, but pid {peer} listens on {}", lock.display(), socket.display())]
    OwnerMismatch {
        lock: PathBuf,
        recorded: i32,
        peer: i32,
        socket: PathBuf,
    },
    #[error("cannot stop the daemon with pid {pid}: {source}")]
    Signal { pid: i32, source: io::Error },
    #[error("the daemon with pid {pid} did not stop within {} seconds", timeout.as_secs())]
    StopTimeout { pid: i32, timeout: Duration },
    #[error("no daemon answered on {} within {} seconds of the start", socket.display(), timeout.as_secs())]
    StartTimeout { socket: PathBuf, timeout: Duration },
    #[error("stopped the fold daemon {old}, but the new daemon did not start: {error}")]
    NotStarted {
        old: String,
        error: Box<RestartError>,
    },
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl RestartError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Serve(error) => error.code(),
            Self::Service(error) => error.code(),
            Self::NotStarted { error, .. } => error.code(),
            _ => None,
        }
    }

    /// The hint that names the next step, when the error has one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Service(error) => error.hint(),
            Self::NoProcess { .. } => {
                Some("stop the `fld serve` process yourself, then run `fld restart`")
            }
            Self::OwnerMismatch { .. } => Some(
                "fld cannot tell which process is the daemon, so it signals neither: stop the `fld serve` process yourself, then run `fld restart`",
            ),
            Self::NotStarted { .. } => Some(
                "the old daemon is stopped: run `fld serve` to see why the new one does not start, then `fld restart`",
            ),
            Self::StopTimeout { .. } => {
                Some("the daemon still shuts down: run `fld restart` again once it has stopped")
            }
            Self::StartTimeout { .. } => {
                Some("run `fld doctor`, or `fld serve` to see why the daemon does not start")
            }
            _ => None,
        }
    }
}

/// The daemon that holds the lock: its pid as the kernel reports the socket's peer, else as the daemon recorded it, and its version as it recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) pid: Option<i32>,
    pub(crate) version: Option<String>,
}

/// What `fld restart` does to daemon processes, so the tests can count the steps without starting one.
pub(crate) trait Daemons {
    /// The daemon that runs now, if any.
    fn running(&self) -> Result<Option<Found>, RestartError>;
    /// Ends `found` with SIGTERM, which shuts `fld serve` down as Ctrl-C does: each supervised worker stops and keeps its session, and the next daemon resumes it. Returns once the daemon lock is free.
    fn stop(&self, found: &Found) -> Result<(), RestartError>;
    /// Starts `fld serve` in the background the way `fld claude` does when no daemon runs.
    fn start(&self) -> Result<(), RestartError>;
    /// Waits until a daemon holds the lock and answers on the socket.
    fn wait_started(&self) -> Result<Found, RestartError>;
}

/// The service that `fld setup` installed, when its file is there and its manager's command is on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Manager {
    Systemd {
        systemctl: PathBuf,
    },
    Launchd {
        launchctl: PathBuf,
        domain: String,
        agent: PathBuf,
    },
}

impl Manager {
    /// The installed service, when its manager answers. A unit file without a user instance of systemd, as in a session without a user bus, or an agent without a launchd domain for this user, starts nothing, so fld starts the daemon itself.
    fn of(host: &Host) -> Option<Self> {
        Self::installed(host).filter(Self::answers)
    }

    fn installed(host: &Host) -> Option<Self> {
        if host.platform.linux {
            let unit = host.unit_path()?;
            if !unit.is_file() {
                return None;
            }
            return Some(Self::Systemd {
                systemctl: host.systemctl()?,
            });
        }
        let agent = host.agent_path()?;
        if !agent.is_file() {
            return None;
        }
        Some(Self::Launchd {
            launchctl: host.launchctl()?,
            domain: format!("gui/{}", host.uid),
            agent,
        })
    }

    fn answers(&self) -> bool {
        match self {
            Self::Systemd { systemctl } => succeeds(systemctl, &["--user", "cat", "fold.service"]),
            Self::Launchd {
                launchctl, domain, ..
            } => succeeds(launchctl, &["print", domain]),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Systemd { .. } => "fold.service",
            Self::Launchd { .. } => LAUNCH_AGENT_LABEL,
        }
    }

    /// `restart` rather than `start`, and `kickstart -k`, because the manager may not have seen the old daemon's exit yet; an agent that is not loaded is bootstrapped, which starts it.
    fn start(&self) -> Result<(), ServiceError> {
        match self {
            Self::Systemd { systemctl } => {
                service::systemctl_user(systemctl, &["restart", "fold.service"])
            }
            Self::Launchd {
                launchctl,
                domain,
                agent,
            } => {
                let target = format!("{domain}/{LAUNCH_AGENT_LABEL}");
                if service::launchctl_succeeds(launchctl, &["kickstart", "-k", &target])? {
                    return Ok(());
                }
                service::launchctl_run(launchctl, &["bootstrap", domain, &agent.to_string_lossy()])
            }
        }
    }
}

pub fn run(args: &RestartArgs, out: &mut impl Write) -> Result<(), RestartError> {
    let host = Host::from_env();
    let config = serve::client_config(args.socket.as_deref(), &host.dirs)?;
    let daemons = Processes {
        lock: serve::lock_path(&host.dirs),
        config,
        stop_timeout: STOP_TIMEOUT,
        start_timeout: START_TIMEOUT,
    };
    if args.stop {
        return stop(&daemons, out);
    }
    restart(&host, &daemons, &daemons.config.socket, out)
}

/// The same clean stop as a restart's, and nothing after it. A daemon under a service manager stays stopped: its SIGTERM shutdown exits 0, and neither fold's unit nor its agent restarts a daemon that exited 0.
fn stop(daemons: &impl Daemons, out: &mut impl Write) -> Result<(), RestartError> {
    let line = match daemons.running()? {
        Some(old) => {
            daemons.stop(&old)?;
            format!("stopped the fold daemon {}", version(&old))
        }
        None => "no fold daemon was running".to_owned(),
    };
    output::line(out, &line)?;
    Ok(())
}

/// The old daemon always stops through its own SIGTERM, also under a service manager: a manager's stop signals every process of the service at once, and a worker that dies before the daemon lets go of it counts as failed instead of resuming. The manager then starts the new daemon, so it keeps supervising it.
fn restart(
    host: &Host,
    daemons: &impl Daemons,
    socket: &Path,
    out: &mut impl Write,
) -> Result<(), RestartError> {
    let manager = Manager::of(host);
    let old = daemons.running()?;
    if let Some(old) = &old {
        daemons.stop(old)?;
    }
    let started = match &manager {
        Some(manager) => manager.start().map_err(RestartError::from),
        None => daemons.start(),
    }
    .and_then(|()| daemons.wait_started());
    let new = match (started, &old) {
        (Ok(new), _) => new,
        (Err(error), Some(old)) => {
            return Err(RestartError::NotStarted {
                old: version(old).to_owned(),
                error: Box::new(error),
            });
        }
        (Err(error), None) => return Err(error),
    };
    let through = manager
        .map(|manager| format!(" through {}", manager.name()))
        .unwrap_or_default();
    let line = match old {
        Some(old) => format!(
            "restarted the fold daemon{through}: {} -> {}, socket {}",
            version(&old),
            version(&new),
            socket.display()
        ),
        None => format!(
            "no fold daemon was running; started {}{through}, socket {}",
            version(&new),
            socket.display()
        ),
    };
    output::line(out, &line)?;
    Ok(())
}

fn version(found: &Found) -> &str {
    found.version.as_deref().unwrap_or("unknown version")
}

/// The real daemons of this host, found through the daemon lock and the socket.
struct Processes {
    lock: Option<PathBuf>,
    config: ClientConfig,
    stop_timeout: Duration,
    start_timeout: Duration,
}

impl Processes {
    fn held(&self) -> bool {
        self.lock
            .as_deref()
            .is_some_and(|lock| inspect_lock(lock) == LockState::Held)
    }

    /// The lock's record, read only while the lock is held, so a killed daemon's lines never count. The pid comes from the kernel's report of the socket's peer whenever the socket answers, because a held lock does not prove the record its own: a daemon older than the record leaves the lines of the daemon before it in place. A record whose pid differs from the peer's is refused, since fld cannot tell which of the two is right. Only while the socket does not answer, as during the daemon's start, does the recorded pid count.
    fn found(&self) -> Result<Option<Found>, RestartError> {
        let Some(lock) = &self.lock else {
            return Ok(None);
        };
        if !self.held() {
            return Ok(None);
        }
        let owner = std::fs::read_to_string(lock)
            .ok()
            .and_then(|text| LockOwner::parse(&text));
        let recorded = owner
            .as_ref()
            .and_then(|owner| i32::try_from(owner.pid).ok());
        let pid = match (recorded, peer_pid(&self.config.socket)?) {
            (Some(recorded), Some(peer)) if recorded != peer => {
                return Err(RestartError::OwnerMismatch {
                    lock: lock.clone(),
                    recorded,
                    peer,
                    socket: self.config.socket.clone(),
                });
            }
            (_, Some(peer)) => Some(peer),
            (recorded, None) => recorded,
        };
        Ok(Some(Found {
            pid,
            version: owner.map(|owner| owner.version),
        }))
    }
}

impl Daemons for Processes {
    fn running(&self) -> Result<Option<Found>, RestartError> {
        self.found()
    }

    fn stop(&self, found: &Found) -> Result<(), RestartError> {
        let path = self.lock.clone().unwrap_or_default();
        let pid = found
            .pid
            .ok_or(RestartError::NoProcess { path: path.clone() })?;
        let process =
            rustix::process::Pid::from_raw(pid).ok_or(RestartError::NoProcess { path })?;
        match rustix::process::kill_process(process, rustix::process::Signal::TERM) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => {
                return Err(RestartError::Signal {
                    pid,
                    source: error.into(),
                });
            }
        }
        // The lock, not the pid, tells that the daemon is gone: a daemon that `fld claude` started stays a zombie of that session after it exits.
        poll(self.stop_timeout, || !self.held())
            .then_some(())
            .ok_or(RestartError::StopTimeout {
                pid,
                timeout: self.stop_timeout,
            })
    }

    fn start(&self) -> Result<(), RestartError> {
        let runtime = serve::current_thread_runtime().map_err(RestartError::Runtime)?;
        runtime.block_on(daemon_client::connect(&self.config))?;
        Ok(())
    }

    /// Waits on the socket alone: the daemon takes the lock before it binds the socket, so the lock is read only once the socket answers, and a lock test can never meet the new daemon's own `try_lock` and make it fail as a second daemon.
    fn wait_started(&self) -> Result<Found, RestartError> {
        let answers = || std::os::unix::net::UnixStream::connect(&self.config.socket).is_ok();
        if !poll(self.start_timeout, answers) {
            return Err(RestartError::StartTimeout {
                socket: self.config.socket.clone(),
                timeout: self.start_timeout,
            });
        }
        self.found()?.ok_or(RestartError::StartTimeout {
            socket: self.config.socket.clone(),
            timeout: self.start_timeout,
        })
    }
}

/// Whether `done` held before `timeout` ran out, checked with a growing pause.
fn poll(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    let mut pause = FIRST_POLL;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(MAX_POLL);
    }
}

/// True when `program` exits 0; its output goes nowhere, because only the answer counts.
fn succeeds(program: &Path, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The pid of the process that listens on `socket`, when it runs as this user.
fn peer_pid(socket: &Path) -> Result<Option<i32>, RestartError> {
    let runtime = serve::current_thread_runtime().map_err(RestartError::Runtime)?;
    Ok(runtime.block_on(async {
        let stream = tokio::net::UnixStream::connect(socket).await.ok()?;
        let cred = stream.peer_cred().ok()?;
        (cred.uid() == rustix::process::getuid().as_raw())
            .then(|| cred.pid())
            .flatten()
    }))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::platform::{Platform, Probe};
    use crate::serve::Dirs;

    /// Records each step and answers with the daemons it was given.
    struct Fake {
        running: Option<Found>,
        started: Found,
        stop_fails: bool,
        start_fails: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn new(running: Option<&str>, started: &str) -> Self {
            let found = |version: &str| Found {
                pid: Some(4242),
                version: (!version.is_empty()).then(|| version.to_owned()),
            };
            Self {
                running: running.map(found),
                started: found(started),
                stop_fails: false,
                start_fails: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl Daemons for Fake {
        fn running(&self) -> Result<Option<Found>, RestartError> {
            self.calls.borrow_mut().push("running");
            Ok(self.running.clone())
        }

        fn stop(&self, found: &Found) -> Result<(), RestartError> {
            self.calls.borrow_mut().push("stop");
            if self.stop_fails {
                return Err(RestartError::StopTimeout {
                    pid: found.pid.unwrap_or_default(),
                    timeout: STOP_TIMEOUT,
                });
            }
            Ok(())
        }

        fn start(&self) -> Result<(), RestartError> {
            self.calls.borrow_mut().push("start");
            if self.start_fails {
                return Err(RestartError::StartTimeout {
                    socket: "/run/fold/fold.sock".into(),
                    timeout: START_TIMEOUT,
                });
            }
            Ok(())
        }

        fn wait_started(&self) -> Result<Found, RestartError> {
            self.calls.borrow_mut().push("wait");
            Ok(self.started.clone())
        }
    }

    /// A host below `dir`: home `dir/home`, config `dir/cfg`, and a `PATH` of `dir/bin` only.
    fn host(dir: &Path, linux: bool) -> Host {
        Host {
            dirs: Dirs {
                home: Some(dir.join("home")),
                config_home: Some(dir.join("cfg")),
                ..Dirs::default()
            },
            user: Some("ana".to_owned()),
            uid: 501,
            path: Some(std::env::join_paths([dir.join("bin")]).unwrap()),
            platform: Platform {
                linux,
                macos: !linux,
                file_modes: true,
            },
            probe: Probe::new(dir.join("root")),
            names: crate::fold_name::Environment::default(),
            claude: None,
        }
    }

    /// A script `name` in `dir/bin` that logs its arguments to `dir/calls.log` and then runs `body`.
    fn tool(dir: &Path, name: &str, body: &str) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.join("calls.log");
        let script = bin.join(name);
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        log
    }

    fn installed(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn restarted(host: &Host, fake: &Fake) -> Result<String, RestartError> {
        let mut out = Vec::new();
        restart(host, fake, Path::new("/run/fold/fold.sock"), &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn without_a_service_the_old_daemon_stops_and_fld_starts_the_new_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let fake = Fake::new(Some("0.2.0"), "0.3.0");
        let shown = restarted(&host(dir.path(), true), &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "stop", "start", "wait"]);
        assert_eq!(
            shown,
            "restarted the fold daemon: 0.2.0 -> 0.3.0, socket /run/fold/fold.sock\n"
        );
    }

    #[test]
    fn with_no_daemon_running_one_starts_and_the_line_says_so() {
        let dir = tempfile::TempDir::new().unwrap();
        let fake = Fake::new(None, "0.3.0");
        let shown = restarted(&host(dir.path(), true), &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "start", "wait"]);
        assert_eq!(
            shown,
            "no fold daemon was running; started 0.3.0, socket /run/fold/fold.sock\n"
        );
    }

    #[test]
    fn a_daemon_without_a_recorded_version_shows_an_unknown_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let fake = Fake::new(Some(""), "");
        let shown = restarted(&host(dir.path(), true), &fake).unwrap();
        assert!(
            shown.contains("unknown version -> unknown version"),
            "{shown}"
        );
    }

    #[test]
    fn an_installed_unit_restarts_through_systemd_after_the_old_daemon_stopped() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = tool(dir.path(), "systemctl", "exit 0");
        let host = host(dir.path(), true);
        installed(&host.unit_path().unwrap());
        let fake = Fake::new(Some("0.2.0"), "0.3.0");
        let shown = restarted(&host, &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "stop", "wait"]);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user cat fold.service\n--user restart fold.service\n"
        );
        assert_eq!(
            shown,
            "restarted the fold daemon through fold.service: 0.2.0 -> 0.3.0, socket /run/fold/fold.sock\n"
        );

        let fake = Fake::new(None, "0.3.0");
        let shown = restarted(&host, &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "wait"]);
        assert!(
            shown.starts_with("no fold daemon was running; started 0.3.0 through fold.service")
        );
    }

    #[test]
    fn a_failed_systemctl_is_reported_and_a_failed_stop_starts_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = tool(
            dir.path(),
            "systemctl",
            "[ \"$2\" = cat ] && exit 0\nexit 4",
        );
        let host = host(dir.path(), true);
        installed(&host.unit_path().unwrap());
        let error = restarted(&host, &Fake::new(None, "0.3.0")).unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert_eq!(
            error.to_string(),
            "`systemctl --user restart fold.service` failed with exit status: 4"
        );

        std::fs::remove_file(&log).unwrap();
        let mut fake = Fake::new(Some("0.2.0"), "0.3.0");
        fake.stop_fails = true;
        let error = restarted(&host, &fake).unwrap_err();
        assert!(matches!(error, RestartError::StopTimeout { pid: 4242, .. }));
        assert_eq!(fake.calls(), ["running", "stop"]);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user cat fold.service\n",
            "only the probe, and no restart after a failed stop"
        );
    }

    #[test]
    fn a_start_that_fails_after_the_stop_says_the_old_daemon_is_stopped() {
        let dir = tempfile::TempDir::new().unwrap();
        tool(
            dir.path(),
            "systemctl",
            "[ \"$2\" = cat ] && exit 0\nexit 4",
        );
        let unit = host(dir.path(), true);
        installed(&unit.unit_path().unwrap());
        let fake = Fake::new(Some("0.2.0"), "0.3.0");
        let error = restarted(&unit, &fake).unwrap_err();
        assert_eq!(fake.calls(), ["running", "stop"]);
        assert_eq!(error.code(), Some("service_install_failed"));
        assert_eq!(
            error.to_string(),
            "stopped the fold daemon 0.2.0, but the new daemon did not start: `systemctl --user restart fold.service` failed with exit status: 4"
        );
        let hint = error.hint().unwrap();
        assert!(
            hint.contains("old daemon is stopped") && hint.contains("`fld serve`"),
            "{hint}"
        );

        let bare = tempfile::TempDir::new().unwrap();
        let mut fake = Fake::new(Some("0.2.0"), "0.3.0");
        fake.start_fails = true;
        let error = restarted(&host(bare.path(), true), &fake).unwrap_err();
        assert_eq!(fake.calls(), ["running", "stop", "start"]);
        assert!(matches!(error, RestartError::NotStarted { .. }), "{error}");
        assert_eq!(error.code(), None);
        assert!(error.to_string().contains("no daemon answered"), "{error}");

        let mut fake = Fake::new(None, "0.3.0");
        fake.start_fails = true;
        let error = restarted(&host(bare.path(), true), &fake).unwrap_err();
        assert!(
            matches!(error, RestartError::StartTimeout { .. }),
            "with no old daemon there is nothing stopped to name: {error}"
        );
    }

    #[test]
    fn a_unit_without_systemctl_or_systemctl_without_a_unit_is_no_service() {
        let dir = tempfile::TempDir::new().unwrap();
        let bare = host(dir.path(), true);
        installed(&bare.unit_path().unwrap());
        assert_eq!(Manager::of(&bare), None);
        let fake = Fake::new(None, "0.3.0");
        restarted(&bare, &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "start", "wait"]);

        let other = tempfile::TempDir::new().unwrap();
        tool(other.path(), "systemctl", "exit 0");
        assert_eq!(Manager::of(&host(other.path(), true)), None);
    }

    #[test]
    fn a_service_whose_manager_does_not_answer_falls_back_to_the_background_start() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = tool(dir.path(), "systemctl", "exit 1");
        let linux = host(dir.path(), true);
        installed(&linux.unit_path().unwrap());
        let fake = Fake::new(Some("0.2.0"), "0.3.0");
        let shown = restarted(&linux, &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "stop", "start", "wait"]);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user cat fold.service\n"
        );
        assert!(!shown.contains("through"), "{shown}");

        let dir = tempfile::TempDir::new().unwrap();
        let log = tool(dir.path(), "launchctl", "exit 113");
        let macos = host(dir.path(), false);
        installed(&macos.agent_path().unwrap());
        let fake = Fake::new(None, "0.3.0");
        restarted(&macos, &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "start", "wait"]);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "print gui/501\n");
    }

    #[test]
    fn a_loaded_launch_agent_is_kickstarted_and_one_that_is_not_loaded_is_bootstrapped() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = tool(dir.path(), "launchctl", "exit 0");
        let host = host(dir.path(), false);
        let agent = host.agent_path().unwrap();
        installed(&agent);
        let fake = Fake::new(Some("0.2.0"), "0.3.0");
        let shown = restarted(&host, &fake).unwrap();
        assert_eq!(fake.calls(), ["running", "stop", "wait"]);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "print gui/501\nkickstart -k gui/501/dev.omukk.fold\n"
        );
        assert!(shown.contains("through dev.omukk.fold: 0.2.0 -> 0.3.0"));

        std::fs::remove_file(&log).unwrap();
        tool(
            dir.path(),
            "launchctl",
            "[ \"$1\" = kickstart ] && exit 113\nexit 0",
        );
        restarted(&host, &Fake::new(None, "0.3.0")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!(
                "print gui/501\nkickstart -k gui/501/dev.omukk.fold\nbootstrap gui/501 {}\n",
                agent.display()
            )
        );

        tool(
            dir.path(),
            "launchctl",
            "[ \"$1\" = print ] && exit 0\nexit 5",
        );
        let error = restarted(&host, &Fake::new(None, "0.3.0")).unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert!(error.to_string().contains("bootstrap gui/501"));
    }

    #[test]
    fn stop_ends_the_running_daemon_and_starts_none() {
        let fake = Fake::new(Some("0.2.0"), "0.3.0");
        let mut out = Vec::new();
        stop(&fake, &mut out).unwrap();
        assert_eq!(fake.calls(), ["running", "stop"]);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "stopped the fold daemon 0.2.0\n"
        );

        let fake = Fake::new(None, "0.3.0");
        let mut out = Vec::new();
        stop(&fake, &mut out).unwrap();
        assert_eq!(fake.calls(), ["running"]);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "no fold daemon was running\n"
        );

        let mut fake = Fake::new(Some("0.2.0"), "0.3.0");
        fake.stop_fails = true;
        let error = stop(&fake, &mut Vec::new()).unwrap_err();
        assert!(matches!(error, RestartError::StopTimeout { .. }));
    }

    #[test]
    fn errors_carry_their_codes_and_hints() {
        let stop = RestartError::StopTimeout {
            pid: 7,
            timeout: STOP_TIMEOUT,
        };
        assert_eq!(
            stop.to_string(),
            "the daemon with pid 7 did not stop within 60 seconds"
        );
        assert_eq!(stop.code(), None);
        assert!(stop.hint().unwrap().contains("fld restart"));
        let start = RestartError::StartTimeout {
            socket: "/s".into(),
            timeout: START_TIMEOUT,
        };
        assert_eq!(
            start.to_string(),
            "no daemon answered on /s within 15 seconds of the start"
        );
        assert!(start.hint().unwrap().contains("fld doctor"));
        let missing = RestartError::NoProcess {
            path: "/run/fold/daemon.lock".into(),
        };
        assert!(missing.to_string().contains("/run/fold/daemon.lock"));
        assert!(missing.hint().unwrap().contains("fld serve"));
        let serve = RestartError::Serve(ServeError::Daemon(
            fold_daemon::DaemonError::RuntimeDirMissing,
        ));
        assert_eq!(serve.code(), Some("runtime_dir_missing"));
        assert_eq!(serve.hint(), None);
        let refused = RestartError::Service(ServiceError::NoHome);
        assert_eq!(refused.code(), None);
        assert_eq!(refused.hint(), None);
    }

    #[test]
    fn the_real_daemons_find_none_without_a_held_lock_and_time_out_waiting() {
        let dir = tempfile::TempDir::new().unwrap();
        let processes = Processes {
            lock: Some(dir.path().join("daemon.lock")),
            config: ClientConfig::running(dir.path().join("fold.sock")),
            stop_timeout: Duration::from_millis(50),
            start_timeout: Duration::from_millis(50),
        };
        assert_eq!(processes.running().unwrap(), None);
        std::fs::write(dir.path().join("daemon.lock"), "1\n0.1.0\n").unwrap();
        assert_eq!(
            processes.running().unwrap(),
            None,
            "a free lock's lines are stale"
        );
        let error = processes.wait_started().unwrap_err();
        assert!(
            matches!(error, RestartError::StartTimeout { .. }),
            "{error}"
        );
        let error = processes
            .stop(&Found {
                pid: None,
                version: None,
            })
            .unwrap_err();
        assert!(matches!(error, RestartError::NoProcess { .. }), "{error}");

        let held = std::fs::File::open(dir.path().join("daemon.lock")).unwrap();
        held.lock().unwrap();
        let found = processes.running().unwrap().unwrap();
        assert_eq!(
            found,
            Found {
                pid: Some(1),
                version: Some("0.1.0".to_owned())
            }
        );
        std::fs::write(dir.path().join("daemon.lock"), "").unwrap();
        let found = processes.running().unwrap().unwrap();
        assert_eq!(
            found,
            Found {
                pid: None,
                version: None
            },
            "no socket, so no peer"
        );

        let gone = processes
            .stop(&Found {
                pid: Some(i32::MAX),
                version: None,
            })
            .unwrap_err();
        assert!(matches!(gone, RestartError::StopTimeout { .. }), "{gone}");
        drop(held);
        assert_eq!(processes.running().unwrap(), None);
        processes
            .stop(&Found {
                pid: Some(i32::MAX),
                version: None,
            })
            .unwrap();
        let no_lock = Processes {
            lock: None,
            ..processes
        };
        assert_eq!(no_lock.running().unwrap(), None);
    }

    #[test]
    fn the_peer_pid_is_this_process_for_a_socket_it_listens_on() {
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("fold.sock");
        assert_eq!(peer_pid(&socket).unwrap(), None);
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(
            peer_pid(&socket).unwrap(),
            i32::try_from(std::process::id()).ok()
        );
    }

    /// Only reads: no test here signals a process.
    #[test]
    fn the_socket_peer_decides_the_pid_and_a_record_that_disagrees_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let lock = dir.path().join("daemon.lock");
        let socket = dir.path().join("fold.sock");
        let processes = Processes {
            lock: Some(lock.clone()),
            config: ClientConfig::running(socket.clone()),
            stop_timeout: Duration::from_millis(50),
            start_timeout: Duration::from_millis(50),
        };
        let me = i32::try_from(std::process::id()).unwrap();
        std::fs::write(&lock, format!("{me}\n0.3.0\n")).unwrap();
        let held = std::fs::File::open(&lock).unwrap();
        held.lock().unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let mine = Found {
            pid: Some(me),
            version: Some("0.3.0".to_owned()),
        };
        assert_eq!(processes.running().unwrap(), Some(mine.clone()));
        assert_eq!(processes.wait_started().unwrap(), mine);

        std::fs::write(&lock, "1\n0.1.0\n").unwrap();
        let error = processes.running().unwrap_err();
        assert!(
            matches!(error, RestartError::OwnerMismatch { recorded: 1, peer, .. } if peer == me),
            "{error}"
        );
        assert!(
            error.to_string().contains(&format!("pid {me} listens on")),
            "{error}"
        );
        assert!(error.hint().unwrap().contains("`fld restart`"));

        std::fs::write(&lock, "").unwrap();
        assert_eq!(
            processes.running().unwrap(),
            Some(Found {
                pid: Some(me),
                version: None
            }),
            "a daemon older than the record is found through the peer"
        );
    }
}

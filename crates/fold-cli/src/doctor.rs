use std::fs::File;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use fold_core::events::types;
use fold_core::{Clock, FoldError, HumanName, ProjectConfig, ProjectPaths, Role, Timestamp, codes};
use fold_daemon::SystemClock;
use fold_daemon::projects::database_missing_hint;
use fold_daemon::services::agents::claude::{AuthStatus, ClaudeVersion, MIN_CLAUDE_VERSION};
use fold_daemon::services::projects::remove_deleted_on_host;
use fold_vcs::git::Git;
use fold_vcs::git_remote::{EXCLUDE_LINK, folder_has_changes};
use fold_vcs::git_sync_map::{SYNC_MAP_FILE, SyncMap};
use fold_vcs::watch::DegradedReason;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row, SqliteConnection};
use thiserror::Error;

use crate::claude_setup;
use crate::init::host_paths;
use crate::output;
use crate::platform::{Host, Platform, on_path};
use crate::serve::{self, DEFAULT_LISTEN, Dirs, FileConfig, read_file_config};
use crate::{shell_env, status};
use fold_cli::claude_files::{self, shadowing_files};
use fold_cli::claude_user::{self, UserSetup};
use fold_daemon::config::{LOCK_FILE, SOCKET_FILE};

const NOT_APPLICABLE: &str = "not applicable on this platform";
const MIN_WATCHES: u64 = 65_536;
const RECOMMENDED_WATCHES: u64 = 524_288;
const MIN_FREE_BYTES: u64 = 1 << 30;
const DEGRADED_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// The first 16 bytes of every SQLite file.
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";
/// How long `claude --version` and `claude auth status` may run before doctor stops them.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `run_briefly` still reads the output of a command that ended at its deadline.
const OUTPUT_GRACE: Duration = Duration::from_millis(200);
const CLAUDE_INSTALL: &str =
    "install Claude Code: `curl -fsSL https://claude.ai/install.sh | bash`";

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Check only the project in this directory; default is every project in the registry
    #[arg(long)]
    project: Option<PathBuf>,
    /// Unix socket of the daemon to check; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum DoctorError {
    #[error(transparent)]
    HostPaths(FoldError),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    /// `names` lists the failed checks, as the summary names them.
    #[error("checks failed: {failed} ({names})")]
    ChecksFailed { failed: usize, names: String },
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl DoctorError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::HostPaths(error) => Some(error.code()),
            Self::ChecksFailed { .. } => Some(codes::DOCTOR_CHECKS_FAILED),
            Self::Runtime(_) | Self::Write(_) => None,
        }
    }

    /// The hint that names the next step, when a refusal carries one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::HostPaths(error) => error.hint(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub(crate) name: &'static str,
    pub(crate) status: Status,
    pub(crate) message: String,
    pub(crate) fix: Option<String>,
}

impl Check {
    fn ok(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            message: message.into(),
            fix: None,
        }
    }

    fn not_applicable(name: &'static str) -> Self {
        Self::ok(name, NOT_APPLICABLE)
    }

    fn warn(name: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            message: message.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(name: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            status: Status::Fail,
            ..Self::warn(name, message, fix)
        }
    }
}

#[derive(Debug)]
pub(crate) struct Section {
    pub(crate) title: String,
    pub(crate) checks: Vec<Check>,
}

/// What doctor reports: the lines about this fld and the daemon, then the checks.
#[derive(Debug)]
pub(crate) struct Report {
    pub(crate) header: Vec<String>,
    pub(crate) sections: Vec<Section>,
}

pub(crate) fn failed(sections: &[Section]) -> usize {
    sections
        .iter()
        .flat_map(|section| &section.checks)
        .filter(|check| check.status == Status::Fail)
        .count()
}

fn label(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Fail => "fail",
    }
}

fn write_report(sections: &[Section], out: &mut impl Write) -> io::Result<()> {
    for section in sections {
        output::line(out, &section.title)?;
        for check in &section.checks {
            output::line(
                out,
                &format!(
                    "  {:<4} {}: {}",
                    label(check.status),
                    check.name,
                    check.message
                ),
            )?;
            if let Some(fix) = &check.fix {
                output::line(out, &format!("       fix: {fix}"))?;
            }
        }
    }
    Ok(())
}

/// The name of `check` in the summary: a project's checks carry the project, since every project has checks of the same names.
fn summary_name(section: &Section, check: &Check) -> String {
    match section.title.as_str() {
        "host" => check.name.to_owned(),
        title => format!("{} ({title})", check.name),
    }
}

/// The failed and warned checks, in report order, each with its summary name.
fn problems(sections: &[Section]) -> Vec<(String, &Check)> {
    sections
        .iter()
        .flat_map(|section| {
            section
                .checks
                .iter()
                .filter(|check| check.status != Status::Ok)
                .map(move |check| (summary_name(section, check), check))
        })
        .collect()
}

/// The summary names of the failed checks, comma separated.
pub(crate) fn failed_names(sections: &[Section]) -> String {
    problems(sections)
        .into_iter()
        .filter(|(_, check)| check.status == Status::Fail)
        .map(|(name, _)| name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The end of the report, which names every failed and warned check again with its fix, so a paste of only the end still shows them.
fn write_summary(sections: &[Section], out: &mut impl Write) -> io::Result<()> {
    let problems = problems(sections);
    if problems.is_empty() {
        return output::line(out, "summary: every check passed");
    }
    let warned = problems
        .iter()
        .filter(|(_, check)| check.status == Status::Warn)
        .count();
    output::line(
        out,
        &format!(
            "summary: {} failed, {warned} warned",
            problems.len() - warned
        ),
    )?;
    for (name, check) in problems {
        output::line(
            out,
            &format!(
                "  {:<4} {name}: fix: {}",
                label(check.status),
                check.fix.as_deref().unwrap_or_default()
            ),
        )?;
    }
    Ok(())
}

/// Exit code 1 comes from the `ChecksFailed` error; warnings alone end with success.
pub fn run(args: &DoctorArgs, out: &mut impl Write) -> Result<(), DoctorError> {
    let host = Host::from_env();
    let now = SystemClock.now();
    let runtime = serve::current_thread_runtime().map_err(DoctorError::Runtime)?;
    let report = collect(args, &host, now, &runtime)?;
    for line in &report.header {
        output::line(out, line)?;
    }
    write_report(&report.sections, out)?;
    write_summary(&report.sections, out)?;
    match failed(&report.sections) {
        0 => Ok(()),
        failed => Err(DoctorError::ChecksFailed {
            failed,
            names: failed_names(&report.sections),
        }),
    }
}

pub(crate) fn collect(
    args: &DoctorArgs,
    host: &Host,
    now: Timestamp,
    runtime: &tokio::runtime::Runtime,
) -> Result<Report, DoctorError> {
    let mut sections = vec![Section {
        title: "host".to_owned(),
        checks: Vec::new(),
    }];
    let socket = daemon_socket(args.socket.as_deref(), &host.dirs);
    let daemon = socket.as_deref().map_or_else(
        || status::Daemon::NoSocket("XDG_RUNTIME_DIR is not set".to_owned()),
        status::probe,
    );
    let fld = env!("CARGO_PKG_VERSION");
    let header = daemon::header_lines(
        fld,
        std::env::current_exe().ok().as_deref(),
        socket.as_deref(),
        &daemon,
        host.platform,
    );
    let (file, config_check) = daemon_file(&host.dirs);
    sections[0].checks.extend(config_check);
    sections[0]
        .checks
        .extend(host_checks(args.socket.as_deref(), &file, host));
    sections[0]
        .checks
        .extend(daemon::version_verdict(&daemon, fld));
    let report = socket
        .as_deref()
        .map_or(environment::DaemonReport::NotRunning, |socket| {
            environment::ask_daemon(socket, host.user.clone(), runtime)
        });
    let names: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .collect();
    let terminal = environment::sign_in_names(names.iter().map(String::as_str));
    sections[0]
        .checks
        .push(environment::daemon_claude_verdict(&report, &terminal));
    sections[0].checks.extend(environment::daemon_env_verdict(
        &report,
        &terminal,
        host.platform.linux,
    ));
    let start = match &daemon {
        status::Daemon::Running { start, .. } => start.as_ref(),
        _ => None,
    };
    let files = match (shell_of(start), host.dirs.home.as_deref()) {
        (Some(shell), Some(home)) => daemon::startup_files(&shell, home),
        _ => Vec::new(),
    };
    sections[0]
        .checks
        .extend(daemon::shell_verdict(start, &files));
    let projects = match &args.project {
        Some(dir) => {
            let root = absolute_dir(dir);
            vec![(root.display().to_string(), root)]
        }
        None => {
            remove_deleted_projects(args.socket.as_deref(), host, runtime);
            let (projects, registry_check) = registered_projects(&host.dirs, runtime)?;
            sections[0].checks.extend(registry_check);
            projects
        }
    };
    let registered = if args.project.is_some() {
        registered_projects(&host.dirs, runtime)
            .map(|(projects, _)| projects)
            .unwrap_or_default()
    } else {
        projects.clone()
    };
    let answering = socket
        .as_deref()
        .filter(|_| matches!(daemon, status::Daemon::Running { .. }));
    for (label, root) in projects {
        let mut checks = project_checks(&root, host, now, runtime);
        let slug = push::slug_of(&registered, &root);
        checks.push(push::push_check(socket.as_deref(), slug.as_deref()));
        checks.push(workers::workers_check(
            answering,
            slug.as_deref(),
            start.map(|start| start.started_at),
        ));
        sections.push(Section {
            title: format!("project {label}"),
            checks,
        });
    }
    Ok(Report { header, sections })
}

/// The login shell whose startup files count: the one the daemon names, else the one this terminal's user has.
fn shell_of(start: Option<&status::Start>) -> Option<String> {
    if let Some(shell) = start.and_then(|start| start.shell.clone()) {
        return Some(shell);
    }
    let passwd = std::fs::read_to_string("/etc/passwd").ok();
    let uid = rustix::process::getuid().as_raw();
    shell_env::login_shell(std::env::var_os("SHELL"), passwd.as_deref(), uid)
        .as_deref()
        .and_then(shell_env::shell_name)
}

/// The shadowing walk climbs the parents of the root, so a relative `--project` is resolved first.
fn absolute_dir(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir)
        .or_else(|_| std::path::absolute(dir))
        .unwrap_or_else(|_| dir.to_owned())
}

/// An unreadable `daemon.toml` is a failed check and the defaults stand in, so the rest still reports.
fn daemon_file(dirs: &Dirs) -> (FileConfig, Option<Check>) {
    let Ok(path) = dirs.config_file() else {
        return (FileConfig::default(), None);
    };
    match read_file_config(&path) {
        Ok(file) => (file, None),
        Err(_) => (
            FileConfig::default(),
            Some(Check::fail(
                "daemon.toml",
                format!("{} cannot be read or parsed", path.display()),
                format!("correct or delete {}", path.display()),
            )),
        ),
    }
}

/// An unreadable daemon.toml is already a failed check, so the runtime directory stands in.
fn daemon_socket(socket_flag: Option<&Path>, dirs: &Dirs) -> Option<PathBuf> {
    serve::socket_path(socket_flag, dirs)
        .ok()
        .flatten()
        .or_else(|| {
            serve::runtime_dir(dirs)
                .ok()
                .map(|dir| dir.join(SOCKET_FILE))
        })
}

fn host_checks(socket_flag: Option<&Path>, file: &FileConfig, host: &Host) -> Vec<Check> {
    let socket = daemon_socket(socket_flag, &host.dirs);
    let state = socket
        .as_deref()
        .map(|path| inspect_socket(path, host.platform));
    let answers = matches!(
        state,
        Some(SocketState::Answers | SocketState::WrongMode { answers: true, .. })
    );
    let lock = serve::lock_path(&host.dirs);
    let listen = file.listen.unwrap_or(DEFAULT_LISTEN);
    vec![
        socket_check(socket.as_deref(), state),
        lock_check(lock.as_deref(), answers),
        http_check(listen),
        git_check(host),
        fold_name_check(
            crate::fold_name::resolve(
                &host.names,
                crate::fold_name::file(&host.dirs).as_deref(),
                false,
                &mut io::empty(),
                &mut io::sink(),
            )
            .ok()
            .as_ref(),
        ),
        claude_check(host),
        claude_setup_check(host),
        service_manager_check(host),
        linger_check(host),
        inotify_limit_check(host),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SocketState {
    Missing,
    WrongMode { mode: u32, answers: bool },
    Answers,
    Dead,
}

/// The daemon answers when it replies to a plain HTTP request with an HTTP status line.
pub(crate) fn inspect_socket(path: &Path, platform: Platform) -> SocketState {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return SocketState::Missing;
    };
    let mode = meta.permissions().mode() & 0o777;
    let answers = answers_http(path);
    if platform.file_modes && !mode_is_private(mode) {
        SocketState::WrongMode { mode, answers }
    } else if answers {
        SocketState::Answers
    } else {
        SocketState::Dead
    }
}

fn answers_http(path: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    let request = b"GET /api/v1/projects HTTP/1.1\r\nHost: fold\r\nConnection: close\r\n\r\n";
    let mut head = [0_u8; 5];
    stream.write_all(request).is_ok() && stream.read_exact(&mut head).is_ok() && head == *b"HTTP/"
}

pub(crate) fn mode_is_private(mode: u32) -> bool {
    mode & 0o777 == 0o600
}

fn socket_check(path: Option<&Path>, state: Option<SocketState>) -> Check {
    let (Some(path), Some(state)) = (path, state) else {
        return Check::warn(
            "socket",
            "no socket path: XDG_RUNTIME_DIR is not set",
            "`export XDG_RUNTIME_DIR=/run/user/$(id -u)`, or pass `--socket <path>` to `fld serve` and `fld doctor`",
        );
    };
    let shown = path.display();
    match state {
        SocketState::Answers => Check::ok("socket", format!("the daemon answers on {shown}")),
        SocketState::WrongMode { mode, .. } => Check::fail(
            "socket",
            format!("{shown} has mode {mode:04o}, not 0600"),
            format!("`chmod 600 {shown}`"),
        ),
        SocketState::Missing => Check::warn(
            "socket",
            format!("no daemon socket at {shown}"),
            "`fld serve`",
        ),
        SocketState::Dead => Check::warn(
            "socket",
            format!("the daemon does not answer on {shown}"),
            "`fld serve`",
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockState {
    Absent,
    Free,
    Held,
}

/// A shared try-lock only reads the lock state; it never changes the file.
pub(crate) fn inspect_lock(path: &Path) -> LockState {
    let Ok(file) = File::open(path) else {
        return LockState::Absent;
    };
    match file.try_lock_shared() {
        Err(std::fs::TryLockError::WouldBlock) => LockState::Held,
        _ => LockState::Free,
    }
}

fn lock_check(path: Option<&Path>, socket_answers: bool) -> Check {
    let Some(path) = path else {
        return Check::ok("locks", format!("no runtime directory, so no {LOCK_FILE}"));
    };
    match (inspect_lock(path), socket_answers) {
        (LockState::Held, false) => Check::fail(
            "locks",
            format!("{} is held and the socket is dead", path.display()),
            format!(
                "stop the process that holds it (`fuser {}`), then `rm {}`",
                path.display(),
                path.display()
            ),
        ),
        (LockState::Held, true) => Check::ok(
            "locks",
            format!("{LOCK_FILE} is held by the running daemon"),
        ),
        (LockState::Free | LockState::Absent, _) => {
            Check::ok("locks", format!("{LOCK_FILE} is free"))
        }
    }
}

fn http_check(listen: std::net::SocketAddr) -> Check {
    match TcpStream::connect_timeout(&listen, CONNECT_TIMEOUT) {
        Ok(_) => Check::ok("http", format!("{listen} accepts connections")),
        Err(_) => Check::warn(
            "http",
            format!("{listen} does not accept connections"),
            "`fld serve`",
        ),
    }
}

fn git_check(host: &Host) -> Check {
    if on_path("git", host.path.as_deref()).is_some() {
        Check::ok("git", "git is on PATH")
    } else {
        Check::warn(
            "git",
            "git is not on PATH; fold needs it only in a project whose Git sync is on: for fld init --with-git, the Git commit of every revision, fld remote, fld push and fld pull",
            "install git",
        )
    }
}

/// The name fld sends as the human, and where it comes from. The daemon refuses a name outside the fold name rules, so such a name is a warning.
pub(crate) fn fold_name_check(found: Option<&crate::fold_name::Found>) -> Check {
    const FIX: &str = "run `fld claude` on a terminal to choose a fold name, or set FOLD_HUMAN to 1 to 32 characters of a-z, 0-9, - and _";
    match found {
        Some(found) if HumanName::parse(&found.name).is_ok() => {
            Check::ok("fold name", format!("{} from {}", found.name, found.source))
        }
        Some(found) => Check::warn(
            "fold name",
            format!(
                "{} from {} is not a name fold accepts",
                found.name, found.source
            ),
            FIX,
        ),
        None => Check::warn("fold name", "fld cannot tell who you are", FIX),
    }
}

/// What doctor learned from the installed Claude Code. `version` and `signed_in` are `None` when the command failed or gave no answer it could read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeFacts {
    Missing,
    Found {
        version: Option<ClaudeVersion>,
        signed_in: Option<bool>,
    },
}

pub(crate) fn claude_verdict(facts: ClaudeFacts) -> Check {
    let ClaudeFacts::Found { version, signed_in } = facts else {
        return Check::warn(
            "claude",
            "claude is not on PATH; fold cannot start Claude Code workers",
            CLAUDE_INSTALL,
        );
    };
    let version = match version {
        None => {
            return Check::warn(
                "claude",
                "claude --version gave no version fold can read",
                "claude update",
            );
        }
        Some(version) if version < MIN_CLAUDE_VERSION => {
            return Check::warn(
                "claude",
                format!(
                    "claude {version} is older than {MIN_CLAUDE_VERSION}, the oldest version fold drives"
                ),
                "claude update",
            );
        }
        Some(version) => version,
    };
    match signed_in {
        Some(true) => Check::ok("claude", format!("claude {version} is signed in")),
        Some(false) => Check::warn(
            "claude",
            format!("claude {version} is not signed in"),
            "claude auth login",
        ),
        None => Check::warn(
            "claude",
            format!("claude {version}: claude auth status gave no answer fold can read"),
            "claude auth login",
        ),
    }
}

/// Whether the user's Claude Code configuration holds fold's server, permission and hooks. A user who declined them gets no warning: `fld claude` gives its sessions the same entries by flags.
pub(crate) fn claude_setup_verdict(setup: Option<UserSetup>, declined: bool) -> Check {
    const NAME: &str = "claude setup";
    match setup {
        None => Check::not_applicable(NAME),
        Some(UserSetup::Complete) => Check::ok(NAME, "fold is in your Claude Code config"),
        Some(UserSetup::Incomplete(_)) if declined => Check::ok(
            NAME,
            format!(
                "you chose to keep fold out of your Claude Code config; {} adds it",
                claude_setup::ADD_COMMAND
            ),
        ),
        Some(UserSetup::Incomplete(problems)) => Check::warn(
            NAME,
            problems.join("; "),
            format!("`{}`", claude_setup::ADD_COMMAND),
        ),
    }
}

/// Doctor reads only the two files that hold the user-scope MCP servers and the user settings.
fn claude_setup_check(host: &Host) -> Check {
    claude_setup_verdict(
        host.claude.as_ref().map(claude_user::state),
        claude_setup::declined(&host.dirs),
    )
}

/// Doctor asks Claude Code itself; it never reads Claude Code's credential files.
fn claude_check(host: &Host) -> Check {
    let facts = match on_path("claude", host.path.as_deref()) {
        None => ClaudeFacts::Missing,
        Some(claude) => ClaudeFacts::Found {
            version: run_briefly(&claude, &["--version"], CLAUDE_TIMEOUT)
                .as_deref()
                .and_then(ClaudeVersion::parse),
            signed_in: run_briefly(&claude, &["auth", "status", "--json"], CLAUDE_TIMEOUT)
                .as_deref()
                .and_then(AuthStatus::parse)
                .map(|status| status.logged_in),
        },
    };
    claude_verdict(facts)
}

/// The stdout of a command that ends within `timeout`, whatever its exit code, because the answer is read from stdout alone. A command that runs longer is killed. The wait for its output ends at the deadline too, or [`OUTPUT_GRACE`] after the command ends when that is later, and gives what came by then: a descendant that keeps stdout open after the command ends cannot hold doctor.
pub(crate) fn run_briefly(program: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        while let Ok(read @ 1..) = stdout.read(&mut chunk) {
            if sender.send(chunk[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let until = deadline.max(std::time::Instant::now() + OUTPUT_GRACE);
    let mut bytes = Vec::new();
    // The reader drops its sender at the end of stdout, which ends the wait early.
    while let Ok(chunk) =
        receiver.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
    {
        bytes.extend(chunk);
    }
    String::from_utf8(bytes).ok()
}

fn systemd_available(host: &Host) -> bool {
    host.systemctl().is_some() && host.probe.systemd_running()
}

fn service_manager_check(host: &Host) -> Check {
    if systemd_available(host) {
        Check::ok("service manager", "systemd user services are available")
    } else if host.launchctl().is_some() {
        let installed = host.agent_path().filter(|path| path.is_file());
        Check::ok(
            "service manager",
            format!(
                "launchd user agents are available; fold's agent is {}",
                installed.map_or_else(
                    || "not installed".to_owned(),
                    |path| path.display().to_string()
                )
            ),
        )
    } else {
        Check::warn(
            "service manager",
            "no service manager support",
            "`fld serve`, started by the host's own supervisor",
        )
    }
}

pub(crate) enum LingerVerdict {
    NotApplicable,
    NotInstalled,
    Enabled,
    Off,
}

pub(crate) fn linger_verdict(
    systemd: bool,
    installed: bool,
    user_known: bool,
    linger: bool,
) -> LingerVerdict {
    if !systemd || !user_known {
        LingerVerdict::NotApplicable
    } else if !installed {
        LingerVerdict::NotInstalled
    } else if linger {
        LingerVerdict::Enabled
    } else {
        LingerVerdict::Off
    }
}

fn linger_check(host: &Host) -> Check {
    let user = host.user.as_deref();
    let installed = host.unit_path().is_some_and(|path| path.is_file());
    let linger = user.is_some_and(|user| host.probe.linger_enabled(user));
    match linger_verdict(systemd_available(host), installed, user.is_some(), linger) {
        LingerVerdict::NotApplicable => Check::not_applicable("linger"),
        LingerVerdict::NotInstalled => Check::ok("linger", "the fold service is not installed"),
        LingerVerdict::Enabled => {
            Check::ok("linger", "linger is on, so the daemon survives logout")
        }
        LingerVerdict::Off => Check::warn(
            "linger",
            "linger is off, so the daemon stops at logout",
            format!("`loginctl enable-linger {}`", user.unwrap_or_default()),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WatchLimit {
    NotApplicable,
    Enough,
    Low(u64),
}

pub(crate) fn watch_limit_verdict(linux: bool, limit: Option<u64>) -> WatchLimit {
    match limit {
        Some(limit) if linux && limit < MIN_WATCHES => WatchLimit::Low(limit),
        Some(_) if linux => WatchLimit::Enough,
        _ => WatchLimit::NotApplicable,
    }
}

fn sysctl_fix() -> String {
    format!(
        "sudo sysctl fs.inotify.max_user_watches={RECOMMENDED_WATCHES}, and to keep it: echo fs.inotify.max_user_watches={RECOMMENDED_WATCHES} | sudo tee /etc/sysctl.d/99-fold.conf"
    )
}

fn inotify_limit_check(host: &Host) -> Check {
    let limit = host.probe.inotify_max_watches();
    match watch_limit_verdict(host.platform.linux, limit) {
        WatchLimit::NotApplicable => Check::not_applicable("inotify limit"),
        WatchLimit::Enough => Check::ok(
            "inotify limit",
            format!("max_user_watches is {}", limit.unwrap_or_default()),
        ),
        WatchLimit::Low(limit) => Check::warn(
            "inotify limit",
            format!("max_user_watches is {limit}, below {MIN_WATCHES}"),
            sysctl_fix(),
        ),
    }
}

/// A project whose folder is there without `.fold/` leaves the registry before the project checks, so doctor reports nothing for it: a running daemon removes it, and closes its workers, when it lists its projects, and without a daemon doctor removes it from the host registry itself. A failure leaves the row, and its project checks report the missing data.
fn remove_deleted_projects(
    socket_flag: Option<&Path>,
    host: &Host,
    runtime: &tokio::runtime::Runtime,
) {
    let listed =
        daemon_socket(socket_flag, &host.dirs).is_some_and(|socket| lists_projects(&socket));
    if listed {
        return;
    }
    if let Ok(paths) = host_paths(&host.dirs) {
        let _ = runtime.block_on(remove_deleted_on_host(&paths));
    }
}

/// Reads the daemon's whole answer to the project list, since the daemon removes deleted projects while it makes that answer.
fn lists_projects(socket: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    let request = b"GET /api/v1/projects HTTP/1.1\r\nHost: fold\r\nConnection: close\r\n\r\n";
    let mut reply = Vec::new();
    stream.write_all(request).is_ok()
        && stream.read_to_end(&mut reply).is_ok()
        && reply.starts_with(b"HTTP/1.1 200")
}

/// A project's slug and root.
type Registered = (String, PathBuf);

/// A registry that cannot be read is a failed check; a missing registry means no projects yet.
fn registered_projects(
    dirs: &Dirs,
    runtime: &tokio::runtime::Runtime,
) -> Result<(Vec<Registered>, Option<Check>), DoctorError> {
    let host = host_paths(dirs).map_err(DoctorError::HostPaths)?;
    let registry = host.registry_db();
    if !registry.is_file() {
        return Ok((Vec::new(), None));
    }
    match runtime.block_on(read_registry(&registry)) {
        Ok(projects) => Ok((projects, None)),
        Err(_) => Ok((
            Vec::new(),
            Some(Check::fail(
                "registry",
                format!("{} cannot be read", registry.display()),
                format!("restore {} from a backup", registry.display()),
            )),
        )),
    }
}

async fn read_registry(path: &Path) -> Result<Vec<Registered>, sqlx::Error> {
    let mut conn = connect_read_only(path).await?;
    let rows = sqlx::query!("SELECT slug, root FROM projects ORDER BY slug")
        .fetch_all(&mut conn)
        .await?;
    let _ = conn.close().await;
    Ok(rows
        .into_iter()
        .map(|row| (row.slug, PathBuf::from(row.root)))
        .collect())
}

/// A WAL database is normally read through `-wal` and `-shm` files, and a reader creates them when they are absent. With no `-wal` file no process has the database open, so it opens immutable and doctor leaves no file behind. An immutable reader takes no lock: a daemon that opens the database during the check writes to a new `-wal` and the check reads the state the file had when it started, unless that daemon checkpoints within the same moment.
async fn connect_read_only(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(!Path::new(&wal).exists())
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
}

fn project_checks(
    root: &Path,
    host: &Host,
    now: Timestamp,
    runtime: &tokio::runtime::Runtime,
) -> Vec<Check> {
    if !root.is_dir() {
        return vec![Check::fail(
            "directory",
            format!(
                "{} does not exist; fold keeps the project and opens it again when the folder is back",
                root.display()
            ),
            format!(
                "mount the disk that holds {} or restore the folder from a backup",
                root.display()
            ),
        )];
    }
    let paths = ProjectPaths::new(root);
    if std::fs::symlink_metadata(paths.fold_dir()).is_err() {
        return vec![fold_data_check(&paths)];
    }
    let watch_applicable =
        watch_limit_verdict(host.platform.linux, host.probe.inotify_max_watches())
            != WatchLimit::NotApplicable;
    let events_now = watch_applicable.then_some(now);
    let facts = runtime.block_on(inspect_database(&paths.db(), events_now));
    let mut checks = vec![
        database_check(&paths, &facts),
        objects_check(&paths.objects()),
    ];
    if facts.git_sync == Some(false) {
        // A plain folder has no Git repository, so fold reads no Git state of it.
        checks.push(Check::ok(
            "git sync",
            "Git sync is off; fld init --with-git turns Git on",
        ));
    } else {
        checks.extend([
            git_history_check(&paths, host),
            folder_check(&paths, host),
            exclude_check(&paths),
        ]);
    }
    checks.push(if watch_applicable {
        degraded_check(&facts, now)
    } else {
        Check::not_applicable("inotify watches")
    });
    checks.push(claude_files_check(root));
    checks.push(role_prompts_check(&paths));
    checks.push(models_check(&paths, &facts));
    let home = host.dirs.home.as_deref().map(absolute_dir);
    checks.extend(shadowing_checks(root, home.as_deref()));
    checks
}

/// A project folder without `.fold/` holds no project, and `fld init` there makes a new one in place of the registration. A folder that keeps its Git repository takes `--with-git`, as init refuses a plain init of a repository.
fn fold_data_check(paths: &ProjectPaths) -> Check {
    let init = if paths.git_dir().is_dir() {
        "fld init --with-git"
    } else {
        "fld init"
    };
    Check::fail(
        "fold data",
        format!(
            "{} is missing: the fold data of the project was deleted",
            paths.fold_dir().display()
        ),
        format!("`cd {} && {init}`", output::shell_quote(paths.root())),
    )
}

#[derive(Debug, Default)]
pub(crate) struct DatabaseFacts {
    pub(crate) exists: bool,
    pub(crate) wal: Option<bool>,
    /// `None` when the file cannot be opened as a SQLite database.
    pub(crate) quick_check_problems: Option<usize>,
    /// The pragma itself returned an error, which SQLite does for a damaged page it cannot read.
    pub(crate) quick_check_errored: bool,
    pub(crate) degraded: Vec<Timestamp>,
    /// Whether the project's Git sync is on; `None` when the database cannot tell.
    pub(crate) git_sync: Option<bool>,
    /// The model of each area profile that names one, as `(area, model)`.
    pub(crate) area_models: Vec<(String, String)>,
}

/// SQLite sets bytes 18 and 19 of the header to 2 in WAL mode. Reading the header avoids the journal-mode pragma, which a connection may try to change.
pub(crate) fn header_is_wal(header: &[u8]) -> Option<bool> {
    if header.len() < 20 || !header.starts_with(SQLITE_MAGIC) {
        return None;
    }
    Some(header[18] == 2 && header[19] == 2)
}

/// `events_now` is the time the degraded-watch window ends at; `None` skips reading the events.
async fn inspect_database(path: &Path, events_now: Option<Timestamp>) -> DatabaseFacts {
    let mut facts = DatabaseFacts::default();
    let Ok(mut file) = File::open(path) else {
        return facts;
    };
    facts.exists = true;
    let mut header = [0_u8; 100];
    let read = file.read(&mut header).unwrap_or(0);
    facts.wal = header_is_wal(&header[..read]);
    let Ok(mut conn) = connect_read_only(path).await else {
        return facts;
    };
    match sqlx::query("PRAGMA quick_check").fetch_all(&mut conn).await {
        Ok(rows) => {
            let problems = rows
                .iter()
                .filter(|row| {
                    row.try_get::<String, _>(0)
                        .map_or(true, |line| line != "ok")
                })
                .count();
            facts.quick_check_problems = Some(problems);
        }
        Err(_) => facts.quick_check_errored = true,
    }
    if let Some(now) = events_now
        && facts.quick_check_problems == Some(0)
    {
        facts.degraded = degraded_events(&mut conn, now).await.unwrap_or_default();
    }
    facts.git_sync = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM git_sync"#)
        .fetch_one(&mut conn)
        .await
        .ok()
        .map(|rows| rows > 0);
    facts.area_models =
        sqlx::query_as("SELECT name, model FROM areas WHERE model IS NOT NULL ORDER BY name")
            .fetch_all(&mut conn)
            .await
            .unwrap_or_default();
    let _ = conn.close().await;
    facts
}

/// The `workspace.watch_degraded` events with reason `watch_limit` inside the window that ends at `now`.
async fn degraded_events(
    conn: &mut SqliteConnection,
    now: Timestamp,
) -> Result<Vec<Timestamp>, sqlx::Error> {
    let rows = sqlx::query_scalar!(
        "SELECT created_at FROM events WHERE type = ? AND reason = ? AND created_at > ?",
        types::WORKSPACE_WATCH_DEGRADED.as_str(),
        DegradedReason::WatchLimit.as_str(),
        degraded_window_start(now)
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|created_at| Timestamp::from_millis(created_at).ok())
        .collect())
}

/// The window holds the events created after this many milliseconds.
pub(crate) fn degraded_window_start(now: Timestamp) -> i64 {
    now.as_millis().saturating_sub(DEGRADED_WINDOW_MS)
}

/// The issues the database check reports; empty means healthy.
pub(crate) fn database_issues(facts: &DatabaseFacts) -> Vec<String> {
    if !facts.exists {
        return vec!["the file is missing".to_owned()];
    }
    let mut issues = Vec::new();
    match facts.quick_check_problems {
        None if facts.quick_check_errored => issues.push("quick_check failed".to_owned()),
        None => issues.push("it cannot be opened as a SQLite database".to_owned()),
        Some(0) => {}
        Some(count) => issues.push(format!("quick_check reported {count} problems")),
    }
    if facts.wal == Some(false) {
        issues.push("journal_mode is not wal".to_owned());
    }
    issues
}

fn database_check(paths: &ProjectPaths, facts: &DatabaseFacts) -> Check {
    let issues = database_issues(facts);
    if issues.is_empty() {
        return Check::ok("database", "quick_check is ok and journal_mode is wal");
    }
    let fix = if facts.exists {
        format!("restore {} from a backup", paths.db().display())
    } else {
        database_missing_hint(paths)
    };
    Check::fail(
        "database",
        format!("{}: {}", paths.db().display(), issues.join("; ")),
        fix,
    )
}

pub(crate) fn within_degraded_window(events: &[Timestamp], now: Timestamp) -> usize {
    events
        .iter()
        .filter(|at| now.as_millis().saturating_sub(at.as_millis()) < DEGRADED_WINDOW_MS)
        .count()
}

fn degraded_check(facts: &DatabaseFacts, now: Timestamp) -> Check {
    match within_degraded_window(&facts.degraded, now) {
        0 => Check::ok(
            "inotify watches",
            "no workspace fell back to scanning in the last 24 hours",
        ),
        count => Check::warn(
            "inotify watches",
            format!(
                "{count} watch_degraded events with reason watch_limit in the last 24 hours; a degraded workspace scans every 10 s and keeps working"
            ),
            sysctl_fix(),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjectsFacts {
    pub(crate) exists: bool,
    pub(crate) writable: bool,
    pub(crate) free_bytes: Option<u64>,
}

/// The directory is created on the first object write, so a missing one counts as writable when its parent is.
pub(crate) fn inspect_objects(path: &Path) -> ObjectsFacts {
    use rustix::fs::{Access, access, statvfs};
    let exists = path.is_dir();
    let target = if exists { Some(path) } else { path.parent() };
    ObjectsFacts {
        exists,
        writable: target.is_some_and(|dir| access(dir, Access::WRITE_OK | Access::EXEC_OK).is_ok()),
        free_bytes: target.and_then(|dir| {
            statvfs(dir)
                .ok()
                .map(|stat| stat.f_bavail.saturating_mul(stat.f_frsize))
        }),
    }
}

pub(crate) fn objects_verdict(path: &Path, facts: ObjectsFacts) -> Check {
    let shown = path.display();
    if !facts.writable {
        return Check::fail(
            "objects",
            format!("{shown} is not writable"),
            format!("`chmod -R u+rwX {shown}`"),
        );
    }
    match facts.free_bytes {
        Some(free) if free < MIN_FREE_BYTES => Check::warn(
            "objects",
            format!("{} MiB free, below 1 GiB", free >> 20),
            format!("free space on the filesystem of {shown} (df -h {shown} shows it)"),
        ),
        _ => Check::ok(
            "objects",
            format!("{shown} is writable with enough free space"),
        ),
    }
}

fn objects_check(path: &Path) -> Check {
    objects_verdict(path, inspect_objects(path))
}

/// The problems with fold's Claude Code files in the project root: the marked section of the instructions file, which `instructions` names with its text, the files of the fold skill that are `missing`, and the project files in `old` that still hold entries an earlier fold wrote there.
pub(crate) fn claude_file_problems(
    instructions: (&str, Option<&str>),
    missing: &[&str],
    old: &[&str],
) -> Vec<String> {
    use claude_files::{AGENTS_BEGIN, AGENTS_END};
    let mut problems = Vec::new();
    let (name, text) = instructions;
    match text {
        None => problems.push(format!("{name} is missing")),
        Some(text) if !text.contains(AGENTS_BEGIN) || !text.contains(AGENTS_END) => {
            problems.push(format!("{name} lacks the fold section"));
        }
        Some(_) => {}
    }
    for file in missing {
        problems.push(format!("{file} is missing"));
    }
    for file in old {
        problems.push(format!(
            "{file} holds fold entries, which belong in your Claude Code config"
        ));
    }
    problems
}

/// Every integration continues the commit `.fold/git-sync.map` names last, so a project repository without that commit leaves each new revision without its Git commit.
fn git_history_check(paths: &ProjectPaths, host: &Host) -> Check {
    const NAME: &str = "git history";
    if !paths.git_dir().is_dir() {
        return Check::warn(
            NAME,
            "the project folder has no Git repository in .git, so new revisions get no Git commit",
            "re-create the project with `fld init`",
        );
    }
    let fold_dir = paths.fold_dir();
    let last = match SyncMap::load(&fold_dir) {
        Ok(map) => map.last().cloned(),
        Err(_) => {
            return Check::fail(
                NAME,
                format!("{SYNC_MAP_FILE} cannot be read"),
                format!("restore .fold/{SYNC_MAP_FILE} from a backup"),
            );
        }
    };
    let Some((revision, commit)) = last else {
        return Check::ok(NAME, "no revision has a Git commit yet");
    };
    let Some(program) = on_path("git", host.path.as_deref()) else {
        return Check::not_applicable(NAME);
    };
    match Git::new(program).has_commit(&paths.git_dir(), &commit) {
        Ok(true) => Check::ok(NAME, format!("{revision} has its Git commit in .git")),
        _ => Check::warn(
            NAME,
            format!(
                ".git does not hold {commit}, the commit of {revision}, so new revisions get no Git commit"
            ),
            "run `fld remote add <name> <url>` with a repository that holds that commit, then `fld pull <name>`",
        ),
    }
}

/// While Git sync is on, the project folder and its branch follow main together, so a change of the human on a file main changes holds both back until `fld record` records it.
fn folder_check(paths: &ProjectPaths, host: &Host) -> Check {
    const NAME: &str = "project folder";
    let Some(program) = on_path("git", host.path.as_deref()) else {
        return Check::not_applicable(NAME);
    };
    if !paths.git_dir().is_dir() {
        return Check::warn(
            NAME,
            "the project folder has no Git repository in .git, so it cannot follow main",
            "re-create the project with `fld init`",
        );
    }
    match folder_has_changes(&Git::new(program), paths) {
        Ok(false) => Check::ok(
            NAME,
            "the project folder has no changes that are not committed",
        ),
        Ok(true) => Check::warn(
            NAME,
            "the project folder has changes that are not committed; on a file main changes they hold the folder and its branch back",
            "run `fld record` in the project folder to record them on main, or undo what you staged with `git restore --staged`; fold brings the folder to main at the next integration or daemon start",
        ),
        Err(_) => Check::warn(
            NAME,
            "git cannot read the state of the project folder",
            "run `git status` in the project folder",
        ),
    }
}

/// Git reads `.git/info/exclude` and fold reads `.fold/exclude`; only while the first is a link to the second do they leave out the same paths.
fn exclude_check(paths: &ProjectPaths) -> Check {
    const NAME: &str = "exclude file";
    let link = paths.git_dir().join("info").join("exclude");
    if !paths.git_dir().is_dir() {
        return Check::not_applicable(NAME);
    }
    match std::fs::read_link(&link) {
        Ok(target) if target == Path::new(EXCLUDE_LINK) => Check::ok(
            NAME,
            ".git/info/exclude links to .fold/exclude, so Git and fold leave out the same paths",
        ),
        _ => Check::warn(
            NAME,
            ".git/info/exclude is not the link to .fold/exclude, so Git and fold can leave out different paths",
            "`fld init --refresh`, which moves its lines into .fold/exclude and makes the link",
        ),
    }
}

fn claude_files_check(root: &Path) -> Check {
    let read = |relative: &str| std::fs::read_to_string(root.join(relative)).ok();
    let instructions = claude_files::instructions_file(root);
    let mut problems = claude_file_problems(
        (instructions, read(instructions).as_deref()),
        &claude_files::SKILL_FILES
            .into_iter()
            .filter(|file| !root.join(file).is_file())
            .collect::<Vec<_>>(),
        &claude_files::old_entries(root),
    );
    if let Some(stale) = claude_files::stale_section(root) {
        problems.push(format!(
            "{stale} holds a second fold section, which belongs in {instructions}"
        ));
    }
    if problems.is_empty() {
        Check::ok("claude files", "the fold entries are in place")
    } else {
        Check::warn("claude files", problems.join("; "), "`fld init --refresh`")
    }
}

/// fold looks up a project's prompt by the role's file name and starts a worker with the shipped prompt when that file is absent, so a prompt under any other name is never read and nothing else says so.
fn role_prompts_check(paths: &ProjectPaths) -> Check {
    const NAME: &str = "role prompts";
    let main = paths.main_role_prompt();
    let known: Vec<PathBuf> = Role::ALL
        .iter()
        .map(|role| paths.role_prompt(*role))
        .chain([main.clone()])
        .collect();
    let mut unknown: Vec<String> = main
        .parent()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md") && !known.contains(path))
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    if unknown.is_empty() {
        return Check::ok(NAME, "every prompt in .fold/roles names a role");
    }
    unknown.sort();
    let roles: Vec<&str> = Role::ALL.iter().map(|role| role.as_str()).collect();
    Check::warn(
        NAME,
        format!(
            "fold never reads {} in .fold/roles, because no role has that name",
            unknown.join(", ")
        ),
        format!(
            "rename each file to <role>.md, where the role is {} or main",
            roles.join(", ")
        ),
    )
}

/// The models that `.fold/config.toml` gives the roles and that the area profiles name. A configuration that cannot be read adds none: the daemon refuses it with its own error.
fn models_check(paths: &ProjectPaths, facts: &DatabaseFacts) -> Check {
    let config = std::fs::read_to_string(paths.config())
        .ok()
        .and_then(|text| ProjectConfig::from_toml(&text).ok())
        .unwrap_or_default();
    let models: Vec<(String, String)> = config
        .roles
        .iter()
        .filter_map(|(role, settings)| Some((format!("roles.{role}"), settings.model.clone()?)))
        .chain(
            facts
                .area_models
                .iter()
                .map(|(area, model)| (format!("area {area}"), model.clone())),
        )
        .collect();
    environment::model_names_verdict(&models)
}

fn shadowing_checks(root: &Path, home: Option<&Path>) -> Vec<Check> {
    let files = shadowing_files(root, home);
    if files.is_empty() {
        return vec![Check::ok("claude md", "no CLAUDE.md hides AGENTS.md")];
    }
    files
        .iter()
        .map(|path| {
            let shown = path.display();
            Check::warn(
                "claude md",
                format!("{shown} makes Claude Code skip AGENTS.md"),
                format!("move the text of {shown} into AGENTS.md and delete {shown}"),
            )
        })
        .collect()
}

mod daemon;
mod environment;
mod push;
mod workers;

#[cfg(test)]
mod tests;

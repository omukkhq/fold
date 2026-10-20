use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use clap::Args;
use fold_cli::claude_files::ClaudeFileError;
use fold_core::{FoldError, codes};
use thiserror::Error;

use crate::claude_setup::{self, Offer, SetupError};
use crate::output;
use crate::platform::{Host, LAUNCH_AGENT_LABEL};

#[derive(Args)]
pub struct SetupArgs {
    /// Write the systemd unit or the launchd agent file and start nothing
    #[arg(long)]
    no_start: bool,
    /// Add fold to Claude Code for your user without asking, and install no service
    #[arg(long, conflicts_with_all = ["no_start", "remove"])]
    claude: bool,
    /// Remove fold's entries from Claude Code's config of your user; the service stays
    #[arg(long, conflicts_with = "no_start")]
    remove: bool,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Refused(FoldError),
    #[error("neither XDG_CONFIG_HOME nor HOME is set to an absolute path")]
    NoHome,
    #[error("HOME is not set to an absolute path")]
    NoHomeDir,
    #[error("cannot find the path of the running binary: {0}")]
    Exe(io::Error),
    #[error("the path of the running binary is not valid UTF-8")]
    ExeNotUtf8,
    #[error("cannot write {path}: {source}")]
    WriteUnit { path: PathBuf, source: io::Error },
    #[error("cannot run {program}: {source}")]
    Spawn { program: PathBuf, source: io::Error },
    #[error("`systemctl --user {args}` failed with {status}")]
    Systemctl { args: String, status: String },
    #[error("`launchctl {args}` failed with {status}")]
    Launchctl { args: String, status: String },
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
    #[error(transparent)]
    ClaudeFiles(#[from] ClaudeFileError),
}

impl ServiceError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refused(error) => Some(error.code()),
            Self::WriteUnit { .. }
            | Self::Spawn { .. }
            | Self::Systemctl { .. }
            | Self::Launchctl { .. } => Some(codes::SERVICE_INSTALL_FAILED),
            Self::ClaudeFiles(error) => error.code(),
            Self::NoHome | Self::NoHomeDir | Self::Exe(_) | Self::ExeNotUtf8 | Self::Write(_) => {
                None
            }
        }
    }

    /// The hint that names the next step, when the error has one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Refused(error) => error.hint(),
            Self::ClaudeFiles(error) => error.hint(),
            _ => None,
        }
    }
}

fn no_service_manager() -> ServiceError {
    ServiceError::Refused(
        FoldError::new(codes::INVALID_REQUEST, "no service manager support")
            .with_hint("run `fld serve` under the host's own supervisor")
            .with_details([("reason", serde_json::json!("no_service_manager"))]),
    )
}

const START_ARGS: [&[&str]; 2] = [&["daemon-reload"], &["enable", "--now", "fold.service"]];

/// `--claude` and `--remove` change only Claude Code's user configuration. A plain setup installs the service and then offers to add fold to that configuration.
pub fn run(args: &SetupArgs, out: &mut impl Write) -> Result<(), ServiceError> {
    let host = Host::from_env();
    if args.claude || args.remove {
        let files = host.claude.as_ref().ok_or(ServiceError::NoHomeDir)?;
        let changed = if args.remove {
            claude_setup::remove(&host.dirs, files, out)
        } else {
            claude_setup::add(&host.dirs, files, out)
        };
        return changed.map_err(|error| match error {
            SetupError::Files(error) => ServiceError::ClaudeFiles(error),
            SetupError::Write(error) => ServiceError::Write(error),
        });
    }
    let exe = std::env::current_exe().map_err(ServiceError::Exe)?;
    install(&host, &exe, args.no_start, out)?;
    claude_setup::offer_on_terminal(&host.dirs, host.claude.as_ref(), Offer::Setup, out);
    Ok(())
}

/// `--no-start` is the way to write the unit on a host where `systemctl` is not on `PATH`; only a host that is neither Linux nor macOS is refused whatever the flags say.
fn install(
    host: &Host,
    exe: &Path,
    no_start: bool,
    out: &mut impl Write,
) -> Result<(), ServiceError> {
    if host.platform.macos {
        return install_launch_agent(host, exe, no_start, out);
    }
    if !host.platform.linux {
        return Err(no_service_manager());
    }
    let systemctl = if no_start {
        None
    } else {
        Some(host.systemctl().ok_or_else(no_service_manager)?)
    };
    let exe = exe.to_str().ok_or(ServiceError::ExeNotUtf8)?;
    let unit = host.unit_path().ok_or(ServiceError::NoHome)?;
    write_unit(&unit, &render_unit(Path::new(exe), &agent_env(host))).map_err(|source| {
        ServiceError::WriteUnit {
            path: unit.clone(),
            source,
        }
    })?;
    output::line(out, &format!("wrote {}", unit.display()))?;
    if let Some(systemctl) = systemctl {
        for args in START_ARGS {
            systemctl_user(&systemctl, args)?;
        }
        output::line(out, "enabled and started fold.service")?;
    }
    if let Some(user) = &host.user
        && !host.probe.linger_enabled(user)
    {
        output::line(
            out,
            &format!(
                "linger is off, so the daemon stops at logout: run `loginctl enable-linger {user}`"
            ),
        )?;
    }
    Ok(())
}

pub(crate) fn systemctl_user(program: &Path, args: &[&str]) -> Result<(), ServiceError> {
    let status = Command::new(program)
        .arg("--user")
        .args(args)
        .status()
        .map_err(|source| ServiceError::Spawn {
            program: program.to_owned(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::Systemctl {
            args: args.join(" "),
            status: status.to_string(),
        })
    }
}

/// How often `launchctl bootstrap` is tried after a `bootout` that stopped a loaded agent, and the wait between two tries: launchd can refuse the label for a moment while it removes the old agent.
const BOOTSTRAP_TRIES: u32 = 5;
const BOOTSTRAP_RETRY: Duration = Duration::from_millis(200);

/// The variable that the service of `fld setup` sets for its daemon, with the service manager's name, so the daemon can tell that its own environment is the one `fld setup` saved.
pub(crate) const SERVICE_ENV: &str = "FOLD_SERVICE";

/// The variables the service's daemon takes from the environment of `fld setup`. launchd starts an agent with a bare `PATH` and no XDG variable, and a systemd user manager gives a `PATH` without the user's own folders, so without them the daemon would not find `claude` or `git` where the user's shell does when it cannot read that shell, and on macOS it would open other folders and another socket than the commands of that shell.
const AGENT_ENV: [&str; 4] = [
    "PATH",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
];

/// `--no-start` writes the agent file and runs no `launchctl`. A start boots out an agent of the same label first, so a second `fld setup` loads the new file.
fn install_launch_agent(
    host: &Host,
    exe: &Path,
    no_start: bool,
    out: &mut impl Write,
) -> Result<(), ServiceError> {
    let launchctl = if no_start {
        None
    } else {
        Some(host.launchctl().ok_or_else(no_service_manager)?)
    };
    let exe = exe.to_str().ok_or(ServiceError::ExeNotUtf8)?;
    let (Some(agent), Some(home)) = (host.agent_path(), host.dirs.home.as_deref()) else {
        return Err(ServiceError::NoHomeDir);
    };
    let log = home.join("Library/Logs/fold.log");
    let text = render_launch_agent(Path::new(exe), &log, &agent_env(host));
    write_unit(&agent, &text).map_err(|source| ServiceError::WriteUnit {
        path: agent.clone(),
        source,
    })?;
    output::line(out, &format!("wrote {}", agent.display()))?;
    if let Some(launchctl) = launchctl {
        let domain = format!("gui/{}", host.uid);
        let service = format!("{domain}/{LAUNCH_AGENT_LABEL}");
        let agent = agent.to_string_lossy();
        let was_loaded = launchctl_succeeds(&launchctl, &["bootout", &service])?;
        launchctl_run(&launchctl, &["enable", &service])?;
        let tries = if was_loaded { BOOTSTRAP_TRIES } else { 1 };
        let mut attempt = 1;
        while let Err(error) = launchctl_run(&launchctl, &["bootstrap", &domain, &agent]) {
            if attempt >= tries {
                return Err(error);
            }
            attempt += 1;
            std::thread::sleep(BOOTSTRAP_RETRY);
        }
        output::line(
            out,
            &format!(
                "loaded {LAUNCH_AGENT_LABEL}: the daemon runs now and at every login, and logs to {}",
                log.display()
            ),
        )?;
    }
    Ok(())
}

pub(crate) fn agent_env(host: &Host) -> Vec<(&'static str, String)> {
    let dirs = &host.dirs;
    let values = [
        host.path.as_deref(),
        dirs.config_home.as_deref().map(Path::as_os_str),
        dirs.data_home.as_deref().map(Path::as_os_str),
        dirs.runtime_dir.as_deref().map(Path::as_os_str),
    ];
    AGENT_ENV
        .into_iter()
        .zip(values)
        .filter_map(|(name, value)| Some((name, value?.to_str()?.to_owned())))
        .filter(|(_, value)| !value.is_empty())
        .collect()
}

fn launchctl_command(program: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    command
}

/// True when `launchctl` exits 0. Its error text goes nowhere: a `bootout` of an agent that is not loaded fails, and that is no error here.
pub(crate) fn launchctl_succeeds(program: &Path, args: &[&str]) -> Result<bool, ServiceError> {
    launchctl_command(program, args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|source| ServiceError::Spawn {
            program: program.to_owned(),
            source,
        })
}

pub(crate) fn launchctl_run(program: &Path, args: &[&str]) -> Result<(), ServiceError> {
    let status = launchctl_command(program, args)
        .status()
        .map_err(|source| ServiceError::Spawn {
            program: program.to_owned(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::Launchctl {
            args: args.join(" "),
            status: status.to_string(),
        })
    }
}

/// The launchd property list for a daemon started from `exe`. `KeepAlive` with `SuccessfulExit` false restarts the daemon only when it fails, as `Restart=on-failure` does for the systemd unit, and `ThrottleInterval` matches its `RestartSec`.
pub fn render_launch_agent(exe: &Path, log: &Path, env: &[(&str, String)]) -> String {
    let mut text = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    let string = |value: &str| format!("<string>{}</string>", xml_text(value));
    text.push_str(&format!(
        "\t<key>Label</key>\n\t{}\n",
        string(LAUNCH_AGENT_LABEL)
    ));
    text.push_str(&format!(
        "\t<key>ProgramArguments</key>\n\t<array>\n\t\t{}\n\t\t{}\n\t</array>\n",
        string(&exe.to_string_lossy()),
        string("serve")
    ));
    text.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
    let marker = (SERVICE_ENV, "launchd".to_owned());
    for (name, value) in std::iter::once(&marker).chain(env) {
        text.push_str(&format!(
            "\t\t<key>{}</key>\n\t\t{}\n",
            xml_text(name),
            string(value)
        ));
    }
    text.push_str("\t</dict>\n");
    text.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\t<key>ThrottleInterval</key>\n\t<integer>2</integer>\n");
    text.push_str(&format!(
        "\t<key>StandardOutPath</key>\n\t{}\n\t<key>StandardErrorPath</key>\n\t{}\n",
        string(&log.to_string_lossy()),
        string(&log.to_string_lossy())
    ));
    text.push_str("</dict>\n</plist>\n");
    text
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// How long systemd waits for the daemon's own stop before it sends SIGKILL: longer than the 60 seconds `fld restart` gives that stop, which ends every worker in turn.
pub(crate) const UNIT_STOP_TIMEOUT_SECS: u64 = 90;

/// The unit file for a daemon started from `exe`, with `env` from the environment of `fld setup` and the [`SERVICE_ENV`] marker as its `Environment=` lines; a variable whose value holds a control character is left out. `KillMode=mixed` sends the stop's SIGTERM to the daemon alone, so a stop, a logout or a reboot ends the workers through the daemon's clean shutdown, which keeps each one resumable; the default would signal every worker at the same time, and a worker that exits before the daemon lets go of it counts as failed. What still runs after `TimeoutStopSec` gets SIGKILL.
pub fn render_unit(exe: &Path, env: &[(&str, String)]) -> String {
    let marker = (SERVICE_ENV, "systemd".to_owned());
    let environment: String = std::iter::once(&marker)
        .chain(env)
        .filter_map(|(name, value)| environment_line(name, value))
        .collect();
    format!(
        "[Unit]\nDescription=fold daemon\nAfter=default.target\n\n[Service]\nType=simple\n{environment}ExecStart={} serve\nRestart=on-failure\nRestartSec=2\nKillMode=mixed\nTimeoutStopSec={UNIT_STOP_TIMEOUT_SECS}\n\n[Install]\nWantedBy=default.target\n",
        exec_word(&exe.to_string_lossy())
    )
}

/// One `Environment=` line. systemd expands `%` specifiers there, so each `%` is doubled, but it expands no `$` variable, so a `$` stays as it is; an assignment whose value holds whitespace, a quote, a backslash or an equals sign is quoted, with C escapes for the backslash and the double quote.
fn environment_line(name: &str, value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let assignment = format!("{name}={}", value.replace('%', "%%"));
    if value.contains(|c: char| c.is_whitespace() || "\"'\\=".contains(c)) {
        Some(format!(
            "Environment=\"{}\"\n",
            assignment.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    } else {
        Some(format!("Environment={assignment}\n"))
    }
}

/// What [`refresh_unit`] did with an installed unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refreshed {
    /// The unit already had the text `fld setup` writes for this fld.
    Current,
    /// The unit started this fld with older lines and now has the current text.
    Wrote,
    /// The unit starts another fld, named when its `ExecStart` has the form `fld setup` writes, so it was left as it is.
    Kept(Option<PathBuf>),
}

/// Writes the unit for `exe` at `unit` when the installed unit starts that same fld with other text, then has systemd read it again. A unit that an older fld installed thus gets the current lines without a new `fld setup`, while a unit that starts another fld, such as the installed one when a build of fld runs the restart, keeps pointing at it. The reload runs also when nothing was written, because a reload that failed after an earlier write left systemd with the old unit while the file already holds the new text.
pub(crate) fn refresh_unit(
    unit: &Path,
    exe: &Path,
    env: &[(&str, String)],
    systemctl: &Path,
) -> Result<Refreshed, ServiceError> {
    let exe = exe.to_str().ok_or(ServiceError::ExeNotUtf8)?;
    let text = render_unit(Path::new(exe), env);
    let installed = std::fs::read_to_string(unit).ok();
    let refreshed = match installed.as_deref() {
        Some(installed) if installed == text => Refreshed::Current,
        installed => match installed.and_then(unit_exe) {
            Some(started) if same_file(&started, Path::new(exe)) => {
                write_unit(unit, &text).map_err(|source| ServiceError::WriteUnit {
                    path: unit.to_owned(),
                    source,
                })?;
                Refreshed::Wrote
            }
            started => Refreshed::Kept(started),
        },
    };
    systemctl_user(systemctl, &["daemon-reload"])?;
    Ok(refreshed)
}

/// The fld that a unit's `ExecStart` starts, read back from the form [`render_unit`] writes; `None` for a unit without such a line.
pub(crate) fn unit_exe(text: &str) -> Option<PathBuf> {
    let word = text
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))?
        .strip_suffix(" serve")?;
    let path = match word.strip_prefix('"') {
        Some(quoted) => {
            let mut path = String::new();
            let mut chars = quoted.strip_suffix('"')?.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => path.push(chars.next()?),
                    '"' => return None,
                    c => path.push(c),
                }
            }
            path
        }
        None => word.to_owned(),
    };
    Some(PathBuf::from(path.replace("%%", "%").replace("$$", "$")))
}

/// Whether `a` and `b` name one file: the same path, or the same device and inode once symlinks are followed.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    a == b
        || matches!(
            (std::fs::metadata(a), std::fs::metadata(b)),
            (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino()
        )
}

/// systemd expands `%` specifiers and `$` variables in `ExecStart`, so each `%` and `$` is doubled; a word with whitespace, quotes or a backslash is quoted.
fn exec_word(path: &str) -> String {
    let path = path.replace('%', "%%").replace('$', "$$");
    if path.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\\') {
        format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        path
    }
}

/// The unit goes through `write_atomic`, so a reader never sees half a unit.
fn write_unit(path: &Path, text: &str) -> io::Result<()> {
    fold_vcs::atomic::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::platform::{Platform, Probe};
    use crate::serve::Dirs;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: SetupArgs,
    }

    const GOLDEN: &str = "[Unit]\nDescription=fold daemon\nAfter=default.target\n\n[Service]\nType=simple\nEnvironment=FOLD_SERVICE=systemd\nEnvironment=PATH=/home/ana/.local/bin:/usr/bin\nEnvironment=XDG_RUNTIME_DIR=/run/user/1000\nExecStart=/usr/bin/fld serve\nRestart=on-failure\nRestartSec=2\nKillMode=mixed\nTimeoutStopSec=90\n\n[Install]\nWantedBy=default.target\n";

    /// The environment of the `fld setup` that [`GOLDEN`] was written by.
    fn golden_env() -> Vec<(&'static str, String)> {
        vec![
            ("PATH", "/home/ana/.local/bin:/usr/bin".to_owned()),
            ("XDG_RUNTIME_DIR", "/run/user/1000".to_owned()),
        ]
    }

    #[test]
    fn the_unit_matches_the_golden_text() {
        assert_eq!(
            render_unit(Path::new("/usr/bin/fld"), &golden_env()),
            GOLDEN
        );
    }

    #[test]
    fn an_environment_line_doubles_percent_signs_keeps_dollars_and_quotes_what_systemd_would_split()
    {
        let unit =
            |value: &str| render_unit(Path::new("/usr/bin/fld"), &[("PATH", value.to_owned())]);
        assert!(
            unit("/opt/100%/bin:/usr/bin").contains("\nEnvironment=PATH=/opt/100%%/bin:/usr/bin\n")
        );
        assert!(unit("/opt/$HOME/bin").contains("\nEnvironment=PATH=/opt/$HOME/bin\n"));
        assert!(unit("/opt/my apps/bin").contains("\nEnvironment=\"PATH=/opt/my apps/bin\"\n"));
        assert!(unit("/opt/a\"b\\c/bin").contains("\nEnvironment=\"PATH=/opt/a\\\"b\\\\c/bin\"\n"));
        assert!(unit("/opt/it's/bin").contains("\nEnvironment=\"PATH=/opt/it's/bin\"\n"));
        assert!(unit("/opt/a=b").contains("\nEnvironment=\"PATH=/opt/a=b\"\n"));
        let broken = unit("/bin\nExecStartPre=/bin/evil");
        assert!(!broken.contains("PATH"), "{broken}");
        assert!(!broken.contains("evil"), "{broken}");
        assert!(broken.contains("Environment=FOLD_SERVICE=systemd\n"));
    }

    #[test]
    fn systemd_waits_longer_for_the_stop_than_fld_restart_does() {
        assert!(
            std::time::Duration::from_secs(UNIT_STOP_TIMEOUT_SECS) > crate::restart::STOP_TIMEOUT
        );
    }

    /// A `systemctl` in `dir/bin` that logs its arguments to `dir/calls.log` and exits with `status`.
    fn fake_systemctl(dir: &Path, status: u8) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.join("calls.log");
        let script = bin.join("systemctl");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\nexit {status}\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    /// The unit an older fld wrote for `/usr/bin/fld`, without the stop lines.
    fn older() -> String {
        GOLDEN.replace("KillMode=mixed\nTimeoutStopSec=90\n", "")
    }

    #[test]
    fn a_refresh_keeps_the_drop_ins_of_the_unit() {
        let dir = tempfile::TempDir::new().unwrap();
        let (systemctl, _) = fake_systemctl(dir.path(), 0);
        let unit = dir.path().join("systemd/user/fold.service");
        let drop_in = dir.path().join("systemd/user/fold.service.d/path.conf");
        std::fs::create_dir_all(drop_in.parent().unwrap()).unwrap();
        std::fs::write(&unit, older()).unwrap();
        let own = "[Service]\nEnvironment=PATH=%h/.local/bin:/usr/bin\n";
        std::fs::write(&drop_in, own).unwrap();
        assert_eq!(
            refresh_unit(&unit, Path::new("/usr/bin/fld"), &golden_env(), &systemctl).unwrap(),
            Refreshed::Wrote
        );
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), GOLDEN);
        assert_eq!(std::fs::read_to_string(&drop_in).unwrap(), own);
    }

    #[test]
    fn a_unit_that_starts_this_fld_is_written_again_and_reloaded_and_a_current_one_is_left() {
        let dir = tempfile::TempDir::new().unwrap();
        let (systemctl, log) = fake_systemctl(dir.path(), 0);
        let unit = dir.path().join("systemd/user/fold.service");
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, older()).unwrap();
        let exe = Path::new("/usr/bin/fld");
        assert_eq!(
            refresh_unit(&unit, exe, &golden_env(), &systemctl).unwrap(),
            Refreshed::Wrote
        );
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), GOLDEN);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user daemon-reload\n"
        );
        assert_eq!(
            refresh_unit(&unit, exe, &golden_env(), &systemctl).unwrap(),
            Refreshed::Current
        );
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), GOLDEN);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user daemon-reload\n--user daemon-reload\n",
            "a current unit is not written, but systemd reads it again"
        );

        let (failing, _) = fake_systemctl(&dir.path().join("broken"), 1);
        std::fs::write(&unit, older()).unwrap();
        let error = refresh_unit(&unit, exe, &golden_env(), &failing).unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), GOLDEN);
        std::fs::remove_file(&log).unwrap();
        assert_eq!(
            refresh_unit(&unit, exe, &golden_env(), &systemctl).unwrap(),
            Refreshed::Current
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user daemon-reload\n",
            "the reload that failed after the write runs at the next refresh"
        );
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/fold-\xff"));
        assert!(matches!(
            refresh_unit(&unit, bad, &[], &systemctl),
            Err(ServiceError::ExeNotUtf8)
        ));
    }

    #[test]
    fn a_unit_that_starts_another_fld_is_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let (systemctl, log) = fake_systemctl(dir.path(), 0);
        let unit = dir.path().join("fold.service");
        std::fs::write(&unit, older()).unwrap();
        assert_eq!(
            refresh_unit(
                &unit,
                Path::new("/home/ana/fold/target/debug/fld"),
                &[],
                &systemctl
            )
            .unwrap(),
            Refreshed::Kept(Some(PathBuf::from("/usr/bin/fld")))
        );
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), older());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user daemon-reload\n"
        );
        for text in [
            "",
            "[Service]\nExecStart=/usr/bin/fld serve --socket /run/a.sock\n",
        ] {
            std::fs::write(&unit, text).unwrap();
            assert_eq!(
                refresh_unit(&unit, Path::new("/usr/bin/fld"), &[], &systemctl).unwrap(),
                Refreshed::Kept(None),
                "{text:?}"
            );
            assert_eq!(std::fs::read_to_string(&unit).unwrap(), text);
        }
        assert_eq!(
            refresh_unit(
                &dir.path().join("missing"),
                Path::new("/usr/bin/fld"),
                &[],
                &systemctl
            )
            .unwrap(),
            Refreshed::Kept(None)
        );
    }

    #[test]
    fn a_unit_that_starts_this_fld_through_a_symlink_is_written_again() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let (systemctl, _) = fake_systemctl(dir.path(), 0);
        let exe = dir.path().join("versions/fld");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        let link = dir.path().join("fld");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let units = dir.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let unit = units.join("fold.service");
        std::fs::write(
            &unit,
            render_unit(&link, &[]).replace("KillMode=mixed\n", ""),
        )
        .unwrap();
        assert_eq!(
            refresh_unit(&unit, &exe, &[], &systemctl).unwrap(),
            Refreshed::Wrote
        );
        assert_eq!(
            std::fs::read_to_string(&unit).unwrap(),
            render_unit(&exe, &[])
        );

        std::fs::write(
            &unit,
            render_unit(&link, &[]).replace("KillMode=mixed\n", ""),
        )
        .unwrap();
        std::fs::set_permissions(&units, std::fs::Permissions::from_mode(0o555)).unwrap();
        let error = refresh_unit(&unit, &exe, &[], &systemctl).unwrap_err();
        std::fs::set_permissions(&units, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(error, ServiceError::WriteUnit { .. }), "{error}");
    }

    #[test]
    fn the_exe_of_a_unit_is_read_back_from_every_form_render_unit_writes() {
        for path in [
            "/usr/bin/fld",
            "/opt/my apps/fld",
            "/opt/100%/fld",
            "/opt/$HOME/fld",
            "/opt/a b/50%/fo\"ld",
            "/opt/back\\slash/fld",
        ] {
            assert_eq!(
                unit_exe(&render_unit(Path::new(path), &[])),
                Some(PathBuf::from(path)),
                "{path}"
            );
        }
        assert_eq!(unit_exe("ExecStart=\"/opt/a\"b\" serve\n"), None);
        assert_eq!(unit_exe("ExecStart=\"/opt/a\\\" serve\n"), None);
    }

    #[test]
    fn a_path_with_spaces_is_quoted_and_percent_signs_are_doubled() {
        let unit = render_unit(Path::new("/opt/my apps/fld"), &[]);
        assert!(unit.contains("ExecStart=\"/opt/my apps/fld\" serve\n"));
        let unit = render_unit(Path::new("/opt/100%/fld"), &[]);
        assert!(unit.contains("ExecStart=/opt/100%%/fld serve\n"));
        let unit = render_unit(Path::new("/opt/$HOME/fld"), &[]);
        assert!(unit.contains("ExecStart=/opt/$$HOME/fld serve\n"));
        let unit = render_unit(Path::new("/opt/a b/50%/fo\"ld"), &[]);
        assert!(unit.contains("ExecStart=\"/opt/a b/50%%/fo\\\"ld\" serve\n"));
    }

    #[test]
    fn setup_parses_its_flag() {
        let bare = Wrapper::try_parse_from(["fld"]).unwrap();
        assert!(!bare.args.no_start);
        let flagged = Wrapper::try_parse_from(["fld", "--no-start"]).unwrap();
        assert!(flagged.args.no_start);
        assert!(Wrapper::try_parse_from(["fld", "install"]).is_err());
    }

    fn host(config_home: &Path, linux: bool) -> Host {
        Host {
            dirs: Dirs {
                config_home: Some(config_home.to_owned()),
                ..Dirs::default()
            },
            user: Some("ana".to_owned()),
            uid: 501,
            path: None,
            platform: Platform {
                linux,
                macos: false,
                file_modes: true,
            },
            probe: Probe::new(config_home.join("root")),
            names: crate::fold_name::Environment::default(),
            claude: None,
        }
    }

    /// A macOS host whose home is `dir/home` and whose `PATH` holds `dir/bin` only.
    fn mac_host(dir: &Path) -> Host {
        let mut host = host(&dir.join("cfg"), false);
        host.platform.macos = true;
        host.dirs.home = Some(dir.join("home"));
        host.dirs.data_home = Some(dir.join("data"));
        host.path = Some(std::env::join_paths([dir.join("bin")]).unwrap());
        host
    }

    /// A `launchctl` on `dir/bin` that logs its arguments to `dir/calls.log` and exits with what `body` decides.
    fn fake_launchctl(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.join("calls.log");
        let script = bin.join("launchctl");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        log
    }

    const AGENT_GOLDEN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key>\n\t<string>dev.omukk.fold</string>\n\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/usr/local/bin/fld</string>\n\t\t<string>serve</string>\n\t</array>\n\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>FOLD_SERVICE</key>\n\t\t<string>launchd</string>\n\t\t<key>PATH</key>\n\t\t<string>/opt/homebrew/bin:/usr/bin</string>\n\t</dict>\n\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\t<key>ThrottleInterval</key>\n\t<integer>2</integer>\n\t<key>StandardOutPath</key>\n\t<string>/Users/ana/Library/Logs/fold.log</string>\n\t<key>StandardErrorPath</key>\n\t<string>/Users/ana/Library/Logs/fold.log</string>\n</dict>\n</plist>\n";

    #[test]
    fn the_launch_agent_matches_the_golden_text_and_escapes_xml() {
        let env = [("PATH", "/opt/homebrew/bin:/usr/bin".to_owned())];
        assert_eq!(
            render_launch_agent(
                Path::new("/usr/local/bin/fld"),
                Path::new("/Users/ana/Library/Logs/fold.log"),
                &env
            ),
            AGENT_GOLDEN
        );
        let odd = render_launch_agent(Path::new("/opt/a & <b>/fld"), Path::new("/l"), &[]);
        assert!(odd.contains("<string>/opt/a &amp; &lt;b&gt;/fld</string>"));
        assert!(odd.contains(
            "<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>FOLD_SERVICE</key>\n\t\t<string>launchd</string>\n\t</dict>"
        ));
    }

    #[test]
    fn no_start_on_macos_writes_the_agent_with_the_shell_environment_and_runs_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = fake_launchctl(dir.path(), "exit 0");
        let mut host = mac_host(dir.path());
        host.dirs.runtime_dir = Some("/run/me".into());
        let mut out = Vec::new();
        install(&host, Path::new("/usr/local/bin/fld"), true, &mut out).unwrap();
        let agent = dir
            .path()
            .join("home/Library/LaunchAgents/dev.omukk.fold.plist");
        let text = std::fs::read_to_string(&agent).unwrap();
        assert!(text.contains("<string>/usr/local/bin/fld</string>"));
        for (name, value) in [
            ("PATH", dir.path().join("bin")),
            ("XDG_CONFIG_HOME", dir.path().join("cfg")),
            ("XDG_DATA_HOME", dir.path().join("data")),
            ("XDG_RUNTIME_DIR", PathBuf::from("/run/me")),
        ] {
            assert!(
                text.contains(&format!(
                    "<key>{name}</key>\n\t\t<string>{}</string>",
                    value.display()
                )),
                "{name} in {text}"
            );
        }
        assert!(text.contains(&format!(
            "{}",
            dir.path().join("home/Library/Logs/fold.log").display()
        )));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("wrote {}\n", agent.display())
        );
        assert!(!log.exists(), "no launchctl call");
        assert!(!dir.path().join("cfg/systemd").exists());
    }

    #[test]
    fn setup_on_macos_boots_out_enables_and_bootstraps_the_agent() {
        let dir = tempfile::TempDir::new().unwrap();
        // Not loaded yet: the bootout fails and the rest succeeds.
        let log = fake_launchctl(dir.path(), "[ \"$1\" = bootout ] && exit 3\nexit 0");
        let host = mac_host(dir.path());
        let mut out = Vec::new();
        install(&host, Path::new("/usr/local/bin/fld"), false, &mut out).unwrap();
        let agent = dir
            .path()
            .join("home/Library/LaunchAgents/dev.omukk.fold.plist");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!(
                "bootout gui/501/dev.omukk.fold\nenable gui/501/dev.omukk.fold\nbootstrap gui/501 {}\n",
                agent.display()
            )
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("loaded dev.omukk.fold: the daemon runs now and at every login"));
        assert!(!text.contains("linger"));
    }

    #[test]
    fn a_bootstrap_after_a_bootout_is_tried_again_and_a_failed_one_is_reported() {
        let dir = tempfile::TempDir::new().unwrap();
        // Loaded before: the bootout succeeds and the first bootstrap fails once.
        let flag = dir.path().join("failed-once");
        let log = fake_launchctl(
            dir.path(),
            &format!(
                "if [ \"$1\" = bootstrap ] && [ ! -e '{0}' ]; then : > '{0}'; exit 5; fi\nexit 0",
                flag.display()
            ),
        );
        let host = mac_host(dir.path());
        install(
            &host,
            Path::new("/usr/local/bin/fld"),
            false,
            &mut Vec::new(),
        )
        .unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(calls.matches("bootstrap gui/501 ").count(), 2, "{calls}");

        let broken = tempfile::TempDir::new().unwrap();
        let log = fake_launchctl(broken.path(), "exit 1");
        let error = install(
            &mac_host(broken.path()),
            Path::new("/usr/local/bin/fld"),
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert_eq!(
            error.to_string(),
            "`launchctl enable gui/501/dev.omukk.fold` failed with exit status: 1"
        );
        assert!(!std::fs::read_to_string(&log).unwrap().contains("bootstrap"));
    }

    #[test]
    fn setup_on_macos_needs_launchctl_a_home_and_a_runnable_launchctl() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = mac_host(dir.path());
        let error = install(
            &host,
            Path::new("/usr/local/bin/fld"),
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some("invalid_request"));
        assert!(!dir.path().join("home").exists());

        let mut homeless = mac_host(dir.path());
        homeless.dirs.home = None;
        let error = install(
            &homeless,
            Path::new("/usr/local/bin/fld"),
            true,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(error, ServiceError::NoHomeDir));
        assert_eq!(error.code(), None);

        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/fold-\xff"));
        let error = install(&host, bad, true, &mut Vec::new()).unwrap_err();
        assert!(matches!(error, ServiceError::ExeNotUtf8));

        let error =
            launchctl_succeeds(Path::new("/nonexistent/launchctl"), &["bootout"]).unwrap_err();
        assert!(matches!(error, ServiceError::Spawn { .. }));
        let error = launchctl_run(Path::new("/nonexistent/launchctl"), &["enable"]).unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));

        let blocked = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(blocked.path().join("home/Library")).unwrap();
        std::fs::write(blocked.path().join("home/Library/LaunchAgents"), "").unwrap();
        let error = install(
            &mac_host(blocked.path()),
            Path::new("/usr/local/bin/fld"),
            true,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert!(error.to_string().contains("dev.omukk.fold.plist"));
    }

    #[test]
    fn an_environment_value_that_is_empty_or_not_utf8_stays_out_of_the_agent() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::TempDir::new().unwrap();
        let mut host = mac_host(dir.path());
        host.path = Some(std::ffi::OsStr::from_bytes(b"/bin:\xff").to_owned());
        host.dirs.config_home = Some(PathBuf::new());
        let env = agent_env(&host);
        let names: Vec<&str> = env.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["XDG_DATA_HOME"]);
    }

    #[test]
    fn no_start_writes_the_unit_and_prints_the_linger_hint() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = host(dir.path(), true);
        let mut out = Vec::new();
        install(&host, Path::new("/usr/bin/fld"), true, &mut out).unwrap();
        let unit = dir.path().join("systemd/user/fold.service");
        let written = std::fs::read_to_string(&unit).unwrap();
        assert_eq!(
            written,
            render_unit(Path::new("/usr/bin/fld"), &agent_env(&host))
        );
        assert!(
            written.contains(&format!(
                "Environment=FOLD_SERVICE=systemd\nEnvironment=XDG_CONFIG_HOME={}\nExecStart=",
                dir.path().display()
            )),
            "{written}"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with(&format!("wrote {}\n", unit.display())));
        assert!(text.contains("loginctl enable-linger ana"));

        let before = std::fs::read(&unit).unwrap();
        install(&host, Path::new("/usr/bin/fld"), true, &mut Vec::new()).unwrap();
        assert_eq!(std::fs::read(&unit).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(unit.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn enabled_linger_prints_no_hint() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = host(dir.path(), true);
        let linger = dir.path().join("root/var/lib/systemd/linger");
        std::fs::create_dir_all(&linger).unwrap();
        std::fs::write(linger.join("ana"), "").unwrap();
        let mut out = Vec::new();
        install(&host, Path::new("/usr/bin/fld"), true, &mut out).unwrap();
        assert!(!String::from_utf8(out).unwrap().contains("linger"));
    }

    #[test]
    fn a_host_without_a_service_manager_is_refused_before_anything_is_written() {
        let dir = tempfile::TempDir::new().unwrap();
        for (linux, no_start) in [(true, false), (false, false), (false, true)] {
            let error = install(
                &host(dir.path(), linux),
                Path::new("/usr/bin/fld"),
                no_start,
                &mut Vec::new(),
            )
            .unwrap_err();
            assert_eq!(error.code(), Some("invalid_request"));
            assert_eq!(error.to_string(), "no service manager support");
            assert_eq!(
                error.hint(),
                Some("run `fld serve` under the host's own supervisor")
            );
            let ServiceError::Refused(inner) = &error else {
                panic!("expected a refusal, got {error:?}");
            };
            assert_eq!(
                inner.details().unwrap()["reason"],
                serde_json::json!("no_service_manager")
            );
        }
        assert!(!dir.path().join("systemd").exists());
    }

    #[test]
    fn the_start_runs_daemon_reload_then_enable_now() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = dir.path().join("calls.log");
        let script = bin.join("systemctl");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut host = host(dir.path(), true);
        host.path = Some(std::env::join_paths([&bin]).unwrap());
        let mut out = Vec::new();
        install(&host, Path::new("/usr/bin/fld"), false, &mut out).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--user daemon-reload\n--user enable --now fold.service\n"
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("enabled and started fold.service")
        );

        std::fs::write(&script, "#!/bin/sh\nexit 3\n").unwrap();
        let error = install(&host, Path::new("/usr/bin/fld"), false, &mut Vec::new()).unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert!(error.to_string().contains("daemon-reload"));
    }

    #[test]
    fn a_systemctl_that_cannot_run_is_reported() {
        let error =
            systemctl_user(Path::new("/nonexistent/systemctl"), &["daemon-reload"]).unwrap_err();
        assert!(matches!(error, ServiceError::Spawn { .. }));
        assert_eq!(error.code(), Some("service_install_failed"));
    }

    #[test]
    fn setup_errors_say_what_is_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut no_home = host(dir.path(), true);
        no_home.dirs.config_home = None;
        let error =
            install(&no_home, Path::new("/usr/bin/fld"), true, &mut Vec::new()).unwrap_err();
        assert!(matches!(error, ServiceError::NoHome));
        assert_eq!(error.code(), None);
        assert_eq!(error.hint(), None);

        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/fold-\xff"));
        let error = install(&host(dir.path(), true), bad, true, &mut Vec::new()).unwrap_err();
        assert!(matches!(error, ServiceError::ExeNotUtf8));

        // A file where the unit directory belongs makes the write fail with the path named.
        let blocked = tempfile::TempDir::new().unwrap();
        std::fs::write(blocked.path().join("systemd"), "").unwrap();
        let error = install(
            &host(blocked.path(), true),
            Path::new("/usr/bin/fld"),
            true,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some("service_install_failed"));
        assert!(error.to_string().contains("fold.service"));
    }

    #[test]
    fn a_failed_write_is_an_output_error_without_a_code() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut full: &mut [u8] = &mut [];
        let error = install(
            &host(dir.path(), true),
            Path::new("/usr/bin/fld"),
            true,
            &mut full,
        )
        .unwrap_err();
        assert!(matches!(error, ServiceError::Write(_)));
        assert_eq!(error.code(), None);
    }
}

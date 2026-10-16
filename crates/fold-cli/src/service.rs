use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;
use fold_core::{FoldError, codes};
use thiserror::Error;

use crate::output;
use crate::platform::Host;

#[derive(Args)]
pub struct SetupArgs {
    /// Write the unit and skip every systemctl call
    #[arg(long)]
    no_start: bool,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Refused(FoldError),
    #[error("neither XDG_CONFIG_HOME nor HOME is set to an absolute path")]
    NoHome,
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
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl ServiceError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refused(error) => Some(error.code()),
            Self::WriteUnit { .. } | Self::Spawn { .. } | Self::Systemctl { .. } => {
                Some(codes::SERVICE_INSTALL_FAILED)
            }
            Self::NoHome | Self::Exe(_) | Self::ExeNotUtf8 | Self::Write(_) => None,
        }
    }

    /// The hint that names the next step, when the error has one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Refused(error) => error.hint(),
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

pub fn run(args: &SetupArgs, out: &mut impl Write) -> Result<(), ServiceError> {
    let no_start = &args.no_start;
    let exe = std::env::current_exe().map_err(ServiceError::Exe)?;
    install(&Host::from_env(), &exe, *no_start, out)
}

/// `--no-start` is the way to write the unit on a host where `systemctl` is not on `PATH`; only a non-Linux host is refused whatever the flags say.
fn install(
    host: &Host,
    exe: &Path,
    no_start: bool,
    out: &mut impl Write,
) -> Result<(), ServiceError> {
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
    write_unit(&unit, &render_unit(Path::new(exe))).map_err(|source| ServiceError::WriteUnit {
        path: unit.clone(),
        source,
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

fn systemctl_user(program: &Path, args: &[&str]) -> Result<(), ServiceError> {
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

/// The unit file for a daemon started from `exe`.
pub fn render_unit(exe: &Path) -> String {
    format!(
        "[Unit]\nDescription=fold daemon\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={} serve\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
        exec_word(&exe.to_string_lossy())
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

    const GOLDEN: &str = "[Unit]\nDescription=fold daemon\nAfter=default.target\n\n[Service]\nType=simple\nExecStart=/usr/bin/fld serve\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n";

    #[test]
    fn the_unit_matches_the_golden_text() {
        assert_eq!(render_unit(Path::new("/usr/bin/fld")), GOLDEN);
    }

    #[test]
    fn a_path_with_spaces_is_quoted_and_percent_signs_are_doubled() {
        let unit = render_unit(Path::new("/opt/my apps/fld"));
        assert!(unit.contains("ExecStart=\"/opt/my apps/fld\" serve\n"));
        let unit = render_unit(Path::new("/opt/100%/fld"));
        assert!(unit.contains("ExecStart=/opt/100%%/fld serve\n"));
        let unit = render_unit(Path::new("/opt/$HOME/fld"));
        assert!(unit.contains("ExecStart=/opt/$$HOME/fld serve\n"));
        let unit = render_unit(Path::new("/opt/a b/50%/fo\"ld"));
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
            path: None,
            platform: Platform {
                linux,
                file_modes: true,
            },
            probe: Probe::new(config_home.join("root")),
        }
    }

    #[test]
    fn no_start_writes_the_unit_and_prints_the_linger_hint() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = host(dir.path(), true);
        let mut out = Vec::new();
        install(&host, Path::new("/usr/bin/fld"), true, &mut out).unwrap();
        let unit = dir.path().join("systemd/user/fold.service");
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), GOLDEN);
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

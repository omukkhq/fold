use std::path::{Path, PathBuf};

use fold_core::Timestamp;

use super::Check;
use crate::output;
use crate::platform::Platform;
use crate::status::{Daemon, Manager, Start, service_name};

const VERSION_NAME: &str = "daemon version";
const SHELL_NAME: &str = "daemon shell";
const RESTART_FIX: &str = "`fld restart`, which reads your login shell again";
const BASHRC: &[u8] = b".bashrc";
const FUTURE_FIX: &str = "`touch` each file whose time lies in the future, which gives it the current time, then `fld restart`, which reads your login shell again";

/// The lines before the checks: this fld and the daemon that answers on `socket`, or why none does. A missing daemon is stated here, never a check.
pub(crate) fn header_lines(
    fld: &str,
    exe: Option<&Path>,
    socket: Option<&Path>,
    daemon: &Daemon,
    platform: Platform,
) -> Vec<String> {
    let mut lines = vec![match exe {
        Some(exe) => format!(
            "fld {fld} at {}",
            output::escape_control(&exe.to_string_lossy())
        ),
        None => format!("fld {fld}"),
    }];
    let shown = socket.map(|socket| output::escape_control(&socket.to_string_lossy()));
    match daemon {
        Daemon::NoSocket(reason) => {
            lines.push(format!(
                "daemon: fld cannot tell where its socket is: {reason}"
            ));
            return lines;
        }
        Daemon::NotRunning => {
            lines.push(format!(
                "daemon: not running; no daemon answers on {}, and fld claude, fld serve or fld setup starts one",
                shown.unwrap_or_default()
            ));
            return lines;
        }
        Daemon::Silent(reason) => lines.push(format!(
            "daemon: the socket accepts connections, but fld cannot read the answer: {reason}"
        )),
        Daemon::Running {
            version: None,
            start: None,
            ..
        } => lines.push(
            "daemon: a fold older than this fld, which does not report its version or process"
                .to_owned(),
        ),
        Daemon::Running { version, start, .. } => {
            let mut line = format!(
                "daemon: fold {}",
                output::escape_control(version.as_deref().unwrap_or("of an unknown version"))
            );
            if let Some(start) = start {
                line.push_str(&format!(
                    ", pid {}, started {} {}",
                    start.pid,
                    start.started_at,
                    started_by(&start.started_by, platform)
                ));
            }
            lines.push(line);
        }
    }
    if let Some(shown) = shown {
        lines.push(format!("daemon socket: {shown}"));
    }
    if let Daemon::Running {
        listen: Some(listen),
        ..
    } = daemon
    {
        lines.push(format!(
            "daemon web interface: http://{}",
            output::escape_control(listen)
        ));
    }
    lines
}

/// The words for what started the daemon. A service of an unknown name runs under the manager of this platform.
fn started_by(by: &str, platform: Platform) -> String {
    let manager = match by {
        "hand" => return "by hand, outside the service".to_owned(),
        "systemd" => Manager::Systemd,
        "launchd" => Manager::Launchd,
        _ if platform.macos => Manager::Launchd,
        _ => Manager::Systemd,
    };
    format!("by {}", service_name(manager))
}

/// Whether the running daemon runs this fld's version; `None` when no daemon answers.
pub(crate) fn version_verdict(daemon: &Daemon, fld: &str) -> Option<Check> {
    let Daemon::Running { version, .. } = daemon else {
        return None;
    };
    Some(match version {
        Some(version) if version == fld => Check::ok(
            VERSION_NAME,
            format!("the daemon runs fold {fld}, as this fld does"),
        ),
        Some(version) => Check::warn(
            VERSION_NAME,
            format!(
                "the daemon runs fold {} and this fld is {fld}",
                output::escape_control(version)
            ),
            "`fld restart`",
        ),
        None => Check::warn(
            VERSION_NAME,
            format!("the daemon is older than this fld {fld} and does not report its version"),
            "`fld restart`",
        ),
    })
}

/// The startup files that `shell` reads as a login and interactive shell, in `home`, each as a group of which the shell reads the first that exists:
/// - bash: the first of `~/.bash_profile`, `~/.bash_login` and `~/.profile`, and `~/.bashrc` when that file holds the text `.bashrc`, since a login shell reads `~/.bashrc` only when its profile file calls it;
/// - zsh: `.zshenv`, `.zprofile`, `.zshrc` and `.zlogin` in `zdotdir`, else `home`, and `~/.zshenv`, where `ZDOTDIR` is often set;
/// - fish: `config.fish` and each `conf.d/*.fish` in the `fish` folder of `config_home`, else `~/.config`;
/// - tcsh: the first of `~/.tcshrc` and `~/.cshrc`, and `~/.login`; csh: `~/.cshrc` and `~/.login`;
/// - any other shell: `~/.profile`.
///
/// Each file that exists comes as its path, with `~` for `home`, and its modification time. Of the contents, only bash's profile file is searched, for that text; nothing of it is shown.
pub(crate) fn startup_files(
    shell: &str,
    home: &Path,
    zdotdir: Option<&Path>,
    config_home: Option<&Path>,
) -> Vec<(String, Option<Timestamp>)> {
    let in_home =
        |names: &[&str]| -> Vec<PathBuf> { names.iter().map(|name| home.join(name)).collect() };
    let groups: Vec<Vec<PathBuf>> = match shell {
        "bash" => {
            let profile = in_home(&[".bash_profile", ".bash_login", ".profile"])
                .into_iter()
                .find(|path| path.exists());
            let calls_bashrc = profile
                .as_deref()
                .and_then(|path| std::fs::read(path).ok())
                .is_some_and(|text| text.windows(BASHRC.len()).any(|window| window == BASHRC));
            let mut groups = vec![profile.into_iter().collect()];
            if calls_bashrc {
                groups.push(in_home(&[".bashrc"]));
            }
            groups
        }
        "zsh" => {
            let dir = zdotdir.unwrap_or(home);
            let mut groups = vec![in_home(&[".zshenv"])];
            if dir != home {
                groups.push(vec![dir.join(".zshenv")]);
            }
            groups.extend([".zprofile", ".zshrc", ".zlogin"].map(|name| vec![dir.join(name)]));
            groups
        }
        "fish" => {
            let dir = config_home
                .map_or_else(|| home.join(".config"), Path::to_path_buf)
                .join("fish");
            let mut groups = vec![vec![dir.join("config.fish")]];
            groups.extend(
                fish_snippets(&dir.join("conf.d"))
                    .into_iter()
                    .map(|path| vec![path]),
            );
            groups
        }
        "tcsh" => vec![in_home(&[".tcshrc", ".cshrc"]), in_home(&[".login"])],
        "csh" => vec![in_home(&[".cshrc"]), in_home(&[".login"])],
        _ => vec![in_home(&[".profile"])],
    };
    groups
        .iter()
        .filter_map(|group| {
            group.iter().find_map(|path| {
                let meta = std::fs::metadata(path).ok()?;
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| Timestamp::from_system_time(time).ok());
                Some((shown(path, home), modified))
            })
        })
        .collect()
}

/// The `*.fish` files of `dir`, sorted, which fish reads before `config.fish`.
fn fish_snippets(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "fish")
        })
        .collect();
    files.sort();
    files
}

/// `path` with `~` for `home`.
fn shown(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", output::escape_control(&rest.to_string_lossy())),
        Err(_) => output::escape_control(&path.to_string_lossy()),
    }
}

/// When the daemon read the login shell, and whether a startup file of `files` changed after that, so the workers lack the change. A file whose modification time is later than `now` cannot be compared with the read, and a restart does not change that, so it is named on its own. `None` while no daemon answers or the daemon does not report its read; the version check names that daemon.
pub(crate) fn shell_verdict(
    start: Option<&Start>,
    files: &[(String, Option<Timestamp>)],
    now: Timestamp,
) -> Option<Check> {
    let start = start?;
    let Some(read_at) = start.shell_read_at else {
        return Some(Check::ok(
            SHELL_NAME,
            "the daemon ran no login shell at its start",
        ));
    };
    let shell = start
        .shell
        .as_deref()
        .map(|shell| format!(" ({})", output::escape_control(shell)))
        .unwrap_or_default();
    let named = |keep: &dyn Fn(Timestamp) -> bool| -> Vec<&str> {
        files
            .iter()
            .filter(|(_, modified)| modified.is_some_and(keep))
            .map(|(name, _)| name.as_str())
            .collect()
    };
    let future = named(&|modified| modified > now);
    let changed = named(&|modified| modified > read_at && modified <= now);
    let mut parts = Vec::new();
    if !changed.is_empty() {
        parts.push(format!(
            "{} changed after the daemon read your login shell{shell} at {read_at}, so the workers lack the change",
            changed.join(", ")
        ));
    }
    if !future.is_empty() {
        parts.push(format!(
            "the modification time of {} lies in the future, so doctor cannot tell whether the workers have its content",
            future.join(", ")
        ));
    }
    if !parts.is_empty() {
        let fix = if future.is_empty() {
            RESTART_FIX
        } else {
            FUTURE_FIX
        };
        return Some(Check::warn(SHELL_NAME, parts.join("; "), fix));
    }
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    Some(Check::ok(
        SHELL_NAME,
        match names.as_slice() {
            [] => format!("the daemon read your login shell{shell} at {read_at}"),
            names => format!(
                "the daemon read your login shell{shell} at {read_at}, and {} did not change after that",
                names.join(", ")
            ),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use super::super::Status;
    use super::*;

    const SOCKET: &str = "/run/user/1000/fold/fold.sock";

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn start(by: &str, read_at: Option<i64>) -> Start {
        Start {
            pid: 4242,
            started_at: at(1_790_000_000_000),
            started_by: by.to_owned(),
            shell: Some("bash".to_owned()),
            shell_read_at: read_at.map(at),
        }
    }

    fn running(version: Option<&str>, start: Option<Start>) -> Daemon {
        Daemon::Running {
            version: version.map(str::to_owned),
            listen: version.map(|_| "127.0.0.1:7420".to_owned()),
            start,
        }
    }

    fn linux() -> Platform {
        Platform {
            linux: true,
            macos: false,
            file_modes: true,
        }
    }

    fn header(daemon: &Daemon) -> Vec<String> {
        header_lines(
            "0.3.1",
            Some(Path::new("/home/ana/.local/bin/fld")),
            Some(Path::new(SOCKET)),
            daemon,
            linux(),
        )
    }

    #[test]
    fn the_header_names_this_fld_and_the_running_daemons_process() {
        assert_eq!(
            header(&running(Some("0.3.0"), Some(start("systemd", Some(0))))),
            [
                "fld 0.3.1 at /home/ana/.local/bin/fld",
                "daemon: fold 0.3.0, pid 4242, started 2026-09-21T14:13:20.000Z by the systemd user service fold.service",
                "daemon socket: /run/user/1000/fold/fold.sock",
                "daemon web interface: http://127.0.0.1:7420",
            ]
        );
        assert!(
            header(&running(Some("0.3.1"), Some(start("hand", None))))[1]
                .ends_with("by hand, outside the service")
        );
        let mac = Platform {
            linux: false,
            macos: true,
            file_modes: true,
        };
        assert_eq!(started_by("launchd", linux()), started_by("other", mac));
        assert_eq!(
            started_by("other", linux()),
            "by the systemd user service fold.service"
        );
        assert_eq!(
            header(&running(Some("0.3.1"), None))[1],
            "daemon: fold 0.3.1"
        );
        assert_eq!(
            header_lines("0.3.1", None, None, &running(None, None), linux()),
            [
                "fld 0.3.1",
                "daemon: a fold older than this fld, which does not report its version or process",
            ]
        );
    }

    #[test]
    fn the_header_states_a_missing_or_silent_daemon() {
        assert_eq!(
            header(&Daemon::NotRunning),
            [
                "fld 0.3.1 at /home/ana/.local/bin/fld",
                "daemon: not running; no daemon answers on /run/user/1000/fold/fold.sock, and fld claude, fld serve or fld setup starts one",
            ]
        );
        assert_eq!(
            header(&Daemon::NoSocket("XDG_RUNTIME_DIR is not set".into()))[1],
            "daemon: fld cannot tell where its socket is: XDG_RUNTIME_DIR is not set"
        );
        assert_eq!(
            header(&Daemon::Silent("no answer within 2 seconds".into()))[1..],
            [
                "daemon: the socket accepts connections, but fld cannot read the answer: no answer within 2 seconds",
                "daemon socket: /run/user/1000/fold/fold.sock",
            ]
        );
    }

    #[test]
    fn a_daemon_of_another_or_unknown_version_warns_with_the_restart() {
        assert_eq!(version_verdict(&Daemon::NotRunning, "0.3.1"), None);
        let same = version_verdict(&running(Some("0.3.1"), None), "0.3.1").unwrap();
        assert_eq!(same.status, Status::Ok);
        let other = version_verdict(&running(Some("0.3.0"), None), "0.3.1").unwrap();
        assert_eq!(other.status, Status::Warn);
        assert_eq!(
            other.message,
            "the daemon runs fold 0.3.0 and this fld is 0.3.1"
        );
        assert_eq!(other.fix.as_deref(), Some("`fld restart`"));
        let old = version_verdict(&running(None, None), "0.3.1").unwrap();
        assert_eq!(old.status, Status::Warn);
        assert!(old.message.contains("does not report its version"));
    }

    #[test]
    fn the_startup_files_follow_the_shell_and_bash_reads_its_first_profile() {
        let home = tempfile::TempDir::new().unwrap();
        let names = |shell: &str| -> Vec<String> {
            startup_files(shell, home.path(), None, None)
                .into_iter()
                .map(|(name, modified)| {
                    assert!(modified.is_some());
                    name
                })
                .collect()
        };
        assert!(names("bash").is_empty());
        for name in [
            ".profile",
            ".bashrc",
            ".bash_login",
            ".zshrc",
            ".zlogin",
            ".cshrc",
            ".login",
        ] {
            std::fs::write(home.path().join(name), "").unwrap();
        }
        assert_eq!(names("bash"), ["~/.bash_login"]);
        std::fs::write(
            home.path().join(".bash_login"),
            "[ -f ~/.bashrc ] && . ~/.bashrc\n",
        )
        .unwrap();
        assert_eq!(names("bash"), ["~/.bash_login", "~/.bashrc"]);
        assert_eq!(names("zsh"), ["~/.zshrc", "~/.zlogin"]);
        assert_eq!(names("sh"), ["~/.profile"]);
        assert_eq!(names("csh"), ["~/.cshrc", "~/.login"]);
        assert_eq!(names("tcsh"), ["~/.cshrc", "~/.login"]);
        std::fs::write(home.path().join(".tcshrc"), "").unwrap();
        assert_eq!(names("tcsh"), ["~/.tcshrc", "~/.login"]);
        assert!(names("fish").is_empty());
    }

    #[test]
    fn zsh_reads_its_files_in_zdotdir_and_fish_in_its_config_folder() {
        let home = tempfile::TempDir::new().unwrap();
        let names = |shell: &str, zdotdir: Option<&Path>, config: Option<&Path>| -> Vec<String> {
            startup_files(shell, home.path(), zdotdir, config)
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        };
        let zdotdir = home.path().join(".config/zsh");
        std::fs::create_dir_all(&zdotdir).unwrap();
        for name in [".zshenv", ".zshrc"] {
            std::fs::write(zdotdir.join(name), "").unwrap();
        }
        std::fs::write(home.path().join(".zshenv"), "").unwrap();
        assert_eq!(
            names("zsh", Some(&zdotdir), None),
            ["~/.zshenv", "~/.config/zsh/.zshenv", "~/.config/zsh/.zshrc"]
        );
        assert_eq!(names("zsh", None, None), ["~/.zshenv"]);

        let fish = home.path().join(".config/fish");
        std::fs::create_dir_all(fish.join("conf.d")).unwrap();
        for name in [
            "config.fish",
            "conf.d/b.fish",
            "conf.d/a.fish",
            "conf.d/notes.txt",
        ] {
            std::fs::write(fish.join(name), "").unwrap();
        }
        std::fs::write(home.path().join(".profile"), "").unwrap();
        assert_eq!(
            names("fish", None, None),
            [
                "~/.config/fish/config.fish",
                "~/.config/fish/conf.d/a.fish",
                "~/.config/fish/conf.d/b.fish",
            ]
        );
        let elsewhere = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(elsewhere.path().join("fish")).unwrap();
        std::fs::write(elsewhere.path().join("fish/config.fish"), "").unwrap();
        assert_eq!(
            names("fish", None, Some(elsewhere.path())),
            [format!("{}/fish/config.fish", elsewhere.path().display())]
        );
    }

    #[test]
    fn a_startup_file_newer_than_the_shell_read_warns_with_the_restart() {
        let home = tempfile::TempDir::new().unwrap();
        let read_at = SystemTime::now();
        std::fs::write(home.path().join(".profile"), ". \"$HOME/.bashrc\"\n").unwrap();
        std::fs::write(home.path().join(".bashrc"), "").unwrap();
        for (name, offset) in [(".profile", -60), (".bashrc", 60)] {
            let file = File::options()
                .write(true)
                .open(home.path().join(name))
                .unwrap();
            let time = if offset < 0 {
                read_at - Duration::from_secs(60)
            } else {
                read_at + Duration::from_secs(60)
            };
            file.set_modified(time).unwrap();
        }
        let read = Timestamp::from_system_time(read_at).unwrap();
        let now = Timestamp::from_system_time(read_at + Duration::from_secs(120)).unwrap();
        let files = startup_files("bash", home.path(), None, None);
        let check =
            shell_verdict(Some(&start("hand", Some(read.as_millis()))), &files, now).unwrap();
        assert_eq!(check.status, Status::Warn);
        assert_eq!(
            check.message,
            format!(
                "~/.bashrc changed after the daemon read your login shell (bash) at {read}, so the workers lack the change"
            )
        );
        assert_eq!(check.fix.as_deref(), Some(RESTART_FIX));

        let older = &files[..1];
        let check =
            shell_verdict(Some(&start("hand", Some(read.as_millis()))), older, now).unwrap();
        assert_eq!(check.status, Status::Ok);
        assert_eq!(
            check.message,
            format!(
                "the daemon read your login shell (bash) at {read}, and ~/.profile did not change after that"
            )
        );
        assert_eq!(
            shell_verdict(Some(&start("hand", Some(0))), &[], now)
                .unwrap()
                .message,
            "the daemon read your login shell (bash) at 1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            shell_verdict(Some(&start("hand", None)), &files, now)
                .unwrap()
                .status,
            Status::Ok
        );
        assert_eq!(shell_verdict(None, &files, now), None);

        let file = File::options()
            .write(true)
            .truncate(true)
            .open(home.path().join(".profile"))
            .unwrap();
        file.set_modified(read_at - Duration::from_secs(60))
            .unwrap();
        let files = startup_files("bash", home.path(), None, None);
        assert_eq!(files.len(), 1, "{files:?}");
        let uncalled =
            shell_verdict(Some(&start("hand", Some(read.as_millis()))), &files, now).unwrap();
        assert_eq!(uncalled.status, Status::Ok);
        assert_eq!(uncalled.fix, None);
        assert!(
            !uncalled.message.contains(".bashrc"),
            "{}",
            uncalled.message
        );
    }

    #[test]
    fn a_startup_file_whose_time_lies_in_the_future_is_named_as_such() {
        let read = at(1_790_000_000_000);
        let now = at(1_790_000_100_000);
        let files = [
            ("~/.profile".to_owned(), Some(at(1_790_000_050_000))),
            ("~/.bashrc".to_owned(), Some(at(1_790_000_200_000))),
        ];
        let check =
            shell_verdict(Some(&start("hand", Some(read.as_millis()))), &files, now).unwrap();
        assert_eq!(check.status, Status::Warn);
        assert_eq!(
            check.message,
            format!(
                "~/.profile changed after the daemon read your login shell (bash) at {read}, so the workers lack the change; the modification time of ~/.bashrc lies in the future, so doctor cannot tell whether the workers have its content"
            )
        );
        assert_eq!(check.fix.as_deref(), Some(FUTURE_FIX));
        let only_future = shell_verdict(
            Some(&start("hand", Some(read.as_millis()))),
            &files[1..],
            now,
        )
        .unwrap();
        assert_eq!(only_future.status, Status::Warn);
        assert!(
            only_future
                .message
                .starts_with("the modification time of ~/.bashrc lies in the future"),
            "{}",
            only_future.message
        );
    }
}

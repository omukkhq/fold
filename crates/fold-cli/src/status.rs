use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fold_cli::daemon_client::{self, Response};
use fold_core::{Timestamp, codes};
use fold_daemon::http::host::web_origin;
use fold_daemon::services::projects::row_for_dir_on_host;

use crate::doctor::run_briefly;
use crate::platform::{Host, LAUNCH_AGENT_LABEL};
use crate::serve::{self, Dirs};
use crate::{init, output};
use serde::Deserialize;

/// How long the probe of the daemon and each query of the service manager may take, so a bare `fld` answers at once even when the daemon or the manager hangs.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the read of the host registry waits for a lock, so a registry that another process holds costs a bare `fld` its project line and not its answer.
const REGISTRY_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// The socket-only route that names the daemon's version and listen address.
const DAEMON_ROUTE: &str = "/api/v1/daemon";

/// What the probe of the socket found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Daemon {
    /// fld knows no socket to probe; the text says why.
    NoSocket(String),
    NotRunning,
    /// Something accepts connections on the socket but gave no answer fld can read; the text says what went wrong.
    Silent(String),
    /// `version` and `listen` are `None` for a daemon older than the route that reports them, and `start` for one older than the fields of its process.
    Running {
        version: Option<String>,
        listen: Option<String>,
        start: Option<Start>,
    },
}

/// The daemon's process and its read of the login shell, as the daemon route reports them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Start {
    pub(crate) pid: u32,
    pub(crate) started_at: Timestamp,
    /// `hand`, `systemd` or `launchd`.
    pub(crate) started_by: String,
    pub(crate) shell: Option<String>,
    pub(crate) shell_read_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Manager {
    Systemd,
    Launchd,
}

/// The service that `fld setup` installed, and whether its manager runs it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) manager: Manager,
    pub(crate) active: bool,
}

/// The fold project that holds the current folder, with the name and slug the host registry gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) registered: Registered,
}

/// What the host registry says of a project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Registered {
    /// The name and the slug of the registered project.
    Listed(String, String),
    /// The name and the slug of a project a person removed from fold; its data stays in the folder.
    Removed(String, String),
    Unlisted,
    /// The registry is missing, locked, of another schema version or unreadable, so the status names no project.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) socket: Option<PathBuf>,
    pub(crate) daemon: Daemon,
    /// `None` when no service is installed.
    pub(crate) service: Option<Service>,
    pub(crate) project: Option<Project>,
}

/// Prints fold's status: the version line first, so a failed write ends the command before anything is probed, then the daemon, its service and the current project. A missing daemon is a status, never an error, and this never starts one.
pub fn run(out: &mut impl Write) -> io::Result<()> {
    output::line(out, &output::version_line())?;
    let host = Host::from_env();
    let status = gather(&host, std::env::current_dir().ok().as_deref());
    for line in lines(&status, env!("CARGO_PKG_VERSION")) {
        output::line(out, &line)?;
    }
    Ok(())
}

fn gather(host: &Host, cwd: Option<&Path>) -> Status {
    let (socket, daemon) = match serve::socket_path(None, &host.dirs) {
        Ok(Some(path)) => {
            let path = std::path::absolute(&path).unwrap_or(path);
            let daemon = probe(&path);
            (Some(path), daemon)
        }
        Ok(None) => (
            None,
            Daemon::NoSocket("XDG_RUNTIME_DIR is not set and daemon.toml names no socket".into()),
        ),
        Err(error) => (None, Daemon::NoSocket(first_line(&error.to_string()))),
    };
    Status {
        socket,
        daemon,
        service: service(host),
        project: cwd.and_then(|cwd| project(&host.dirs, cwd)),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

/// Asks the daemon on `socket` what it is, without a human and without starting one.
pub(crate) fn probe(socket: &Path) -> Daemon {
    let runtime = match serve::current_thread_runtime() {
        Ok(runtime) => runtime,
        Err(error) => return Daemon::Silent(format!("cannot start the async runtime: {error}")),
    };
    let answer = runtime.block_on(async {
        tokio::time::timeout(PROBE_TIMEOUT, daemon_client::probe(socket, DAEMON_ROUTE)).await
    });
    match answer {
        Ok(Ok(None)) => Daemon::NotRunning,
        Ok(Ok(Some(response))) => running(&response),
        Ok(Err(error)) => Daemon::Silent(error.to_string()),
        Err(_) => Daemon::Silent(format!(
            "no answer within {} seconds",
            PROBE_TIMEOUT.as_secs()
        )),
    }
}

/// A daemon that refuses the route, or answers without the fields, is older than the route.
fn running(response: &Response) -> Daemon {
    let text = |key: &str| {
        response
            .is_success()
            .then(|| response.body[key].as_str().map(str::to_owned))
            .flatten()
    };
    Daemon::Running {
        version: text("version"),
        listen: text("listen"),
        start: response
            .is_success()
            .then(|| Start::deserialize(&response.body).ok())
            .flatten(),
    }
}

/// The installed service and whether it runs now, through the same files and managers as `fld setup` and `fld restart`, each query cut off after [`PROBE_TIMEOUT`].
fn service(host: &Host) -> Option<Service> {
    if host.platform.linux {
        host.unit_path().filter(|unit| unit.is_file())?;
        let active = host
            .systemctl()
            .and_then(|systemctl| {
                run_briefly(
                    &systemctl,
                    &["--user", "is-active", "fold.service"],
                    PROBE_TIMEOUT,
                )
            })
            .is_some_and(|answer| answer.trim() == "active");
        return Some(Service {
            manager: Manager::Systemd,
            active,
        });
    }
    if host.platform.macos {
        host.agent_path().filter(|agent| agent.is_file())?;
        let target = format!("gui/{}/{LAUNCH_AGENT_LABEL}", host.uid);
        let active = host
            .launchctl()
            .and_then(|launchctl| run_briefly(&launchctl, &["print", &target], PROBE_TIMEOUT))
            .is_some_and(|answer| launchd_running(&answer));
        return Some(Service {
            manager: Manager::Launchd,
            active,
        });
    }
    None
}

/// `launchctl print` names a running agent's state as `state = running`.
fn launchd_running(answer: &str) -> bool {
    answer.lines().any(|line| line.trim() == "state = running")
}

/// The project at or above `cwd`, named as the host registry names it. The registry is only read: a missing one is not created, one of another schema version is not migrated, and a lock is waited for briefly.
fn project(dirs: &Dirs, cwd: &Path) -> Option<Project> {
    let cwd = std::fs::canonicalize(cwd).ok()?;
    let root = init::project_root_at_or_above(&cwd)?;
    let registered = init::host_paths(dirs)
        .ok()
        .filter(|host| host.registry_db().is_file())
        .and_then(|host| {
            let runtime = serve::current_thread_runtime().ok()?;
            runtime.block_on(async {
                tokio::time::timeout(
                    PROBE_TIMEOUT,
                    row_for_dir_on_host(&host, &root, REGISTRY_BUSY_TIMEOUT),
                )
                .await
                .ok()
            })
        })
        .map_or(Registered::Unknown, |row| match row {
            Ok(found) if found.removed_at.is_some() => {
                Registered::Removed(found.row.name, found.row.slug)
            }
            Ok(found) => Registered::Listed(found.row.name, found.row.slug),
            Err(error) if error.code() == Some(codes::PROJECT_NOT_FOUND) => Registered::Unlisted,
            Err(_) => Registered::Unknown,
        });
    Some(Project { root, registered })
}

pub(crate) fn service_name(manager: Manager) -> String {
    match manager {
        Manager::Systemd => "the systemd user service fold.service".to_owned(),
        Manager::Launchd => format!("the launchd agent {LAUNCH_AGENT_LABEL}"),
    }
}

/// The address a browser on this machine opens for the listener the daemon reports.
fn web_url(listen: &str) -> String {
    listen
        .parse::<SocketAddr>()
        .map_or_else(|_| format!("http://{listen}"), web_origin)
}

/// The lines after the version line; `fld` is the version of this fld.
pub(crate) fn lines(status: &Status, fld: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let running = matches!(status.daemon, Daemon::Running { .. });
    match &status.daemon {
        Daemon::NoSocket(reason) => {
            lines.push(format!(
                "daemon: fld cannot tell where its socket is: {reason}"
            ));
        }
        Daemon::NotRunning => {
            lines.push("daemon: not running; fld claude, fld serve or fld setup starts it".into());
        }
        Daemon::Silent(reason) => {
            lines.push(format!(
                "daemon: the socket accepts connections, but fld cannot read the answer: {reason}"
            ));
            lines.push("hint: run fld doctor".into());
        }
        Daemon::Running {
            version: Some(version),
            listen: Some(listen),
            ..
        } => {
            lines.push(format!(
                "daemon: running fold {version}, web interface {}; fld web prints a sign-in link",
                web_url(listen)
            ));
            if version != fld {
                lines.push(format!(
                    "hint: the daemon runs fold {version} and this fld is {fld}: run fld restart"
                ));
            }
        }
        Daemon::Running { .. } => {
            lines.push(
                "daemon: running a fold older than this fld, which does not report its version or address"
                    .into(),
            );
            lines.push(format!(
                "hint: run fld restart, so the daemon runs this fld {fld}"
            ));
        }
    }
    if let Some(socket) = &status.socket {
        let note = if running || matches!(status.daemon, Daemon::Silent(_)) {
            ""
        } else {
            ", where fld looks for the daemon"
        };
        lines.push(format!("socket: {}{note}", socket.display()));
    }
    lines.push(match status.service {
        Some(Service {
            manager,
            active: true,
        }) if running => format!("service: the daemon runs through {}", service_name(manager)),
        Some(Service {
            manager,
            active: true,
        }) => format!(
            "service: {} is active, but no daemon answers on the socket",
            service_name(manager)
        ),
        Some(Service {
            manager,
            active: false,
        }) if running => format!(
            "service: started by hand; {} is installed but not active",
            service_name(manager)
        ),
        Some(Service {
            manager,
            active: false,
        }) => format!(
            "service: {} is installed but not active",
            service_name(manager)
        ),
        None if running => {
            "service: started by hand; fld setup installs a service that starts the daemon at login"
                .into()
        }
        None => {
            "service: none; fld setup installs a service that starts the daemon at login".into()
        }
    });
    match &status.project {
        Some(Project {
            root,
            registered: Registered::Listed(name, slug),
        }) => lines.push(format!(
            "project: {} ({slug}) in {}",
            output::escape_control(name),
            output::escape_control(&root.to_string_lossy())
        )),
        Some(Project {
            root,
            registered: Registered::Removed(name, slug),
        }) => lines.push(format!(
            "project: {} ({slug}) in {}, removed from fold; fld claude or fld init there brings it back",
            output::escape_control(name),
            output::escape_control(&root.to_string_lossy())
        )),
        Some(Project {
            root,
            registered: Registered::Unlisted,
        }) => lines.push(format!(
            "project: {}, which this host's registry does not list",
            output::escape_control(&root.to_string_lossy())
        )),
        Some(Project {
            registered: Registered::Unknown,
            ..
        }) => {}
        None => lines.push(
            "project: none; this folder is in no fold project, and fld init makes one".into(),
        ),
    }
    lines.push("Run fld --help for commands".into());
    lines
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;
    use crate::platform::{Platform, Probe};

    const SOCKET: &str = "/run/user/1000/fold/fold.sock";

    fn status(daemon: Daemon, service: Option<Service>, project: Option<Project>) -> Status {
        Status {
            socket: Some(PathBuf::from(SOCKET)),
            daemon,
            service,
            project,
        }
    }

    fn running(version: &str) -> Daemon {
        Daemon::Running {
            version: Some(version.to_owned()),
            listen: Some("127.0.0.1:7420".to_owned()),
            start: None,
        }
    }

    fn app() -> Option<Project> {
        Some(Project {
            root: PathBuf::from("/home/ana/app"),
            registered: Registered::Listed("App".to_owned(), "app".to_owned()),
        })
    }

    #[test]
    fn a_running_daemon_names_its_address_socket_service_and_the_project() {
        let systemd = Service {
            manager: Manager::Systemd,
            active: true,
        };
        assert_eq!(
            lines(&status(running("0.3.0"), Some(systemd), app()), "0.3.0"),
            [
                "daemon: running fold 0.3.0, web interface http://127.0.0.1:7420; fld web prints a sign-in link",
                "socket: /run/user/1000/fold/fold.sock",
                "service: the daemon runs through the systemd user service fold.service",
                "project: App (app) in /home/ana/app",
                "Run fld --help for commands",
            ]
        );
    }

    #[test]
    fn a_daemon_of_another_version_gets_the_restart_hint() {
        let shown = lines(&status(running("0.2.0"), None, None), "0.3.0");
        assert_eq!(
            shown[..4],
            [
                "daemon: running fold 0.2.0, web interface http://127.0.0.1:7420; fld web prints a sign-in link",
                "hint: the daemon runs fold 0.2.0 and this fld is 0.3.0: run fld restart",
                "socket: /run/user/1000/fold/fold.sock",
                "service: started by hand; fld setup installs a service that starts the daemon at login",
            ]
        );
        let old = Daemon::Running {
            version: None,
            listen: None,
            start: None,
        };
        let shown = lines(&status(old, None, None), "0.3.0");
        assert!(shown[0].contains("older than this fld"), "{shown:?}");
        assert_eq!(
            shown[1],
            "hint: run fld restart, so the daemon runs this fld 0.3.0"
        );
    }

    #[test]
    fn a_missing_daemon_is_a_status_with_the_socket_fld_looks_at() {
        let launchd = Service {
            manager: Manager::Launchd,
            active: false,
        };
        assert_eq!(
            lines(&status(Daemon::NotRunning, Some(launchd), None), "0.3.0"),
            [
                "daemon: not running; fld claude, fld serve or fld setup starts it",
                "socket: /run/user/1000/fold/fold.sock, where fld looks for the daemon",
                "service: the launchd agent dev.omukk.fold is installed but not active",
                "project: none; this folder is in no fold project, and fld init makes one",
                "Run fld --help for commands",
            ]
        );
        let active = Service {
            manager: Manager::Systemd,
            active: true,
        };
        assert_eq!(
            lines(&status(Daemon::NotRunning, Some(active), None), "0.3.0")[2],
            "service: the systemd user service fold.service is active, but no daemon answers on the socket"
        );
        let inactive = Service {
            manager: Manager::Systemd,
            active: false,
        };
        assert_eq!(
            lines(&status(running("0.3.0"), Some(inactive), None), "0.3.0")[2],
            "service: started by hand; the systemd user service fold.service is installed but not active"
        );
        let none = Status {
            socket: None,
            ..status(
                Daemon::NoSocket("XDG_RUNTIME_DIR is not set".into()),
                None,
                None,
            )
        };
        assert_eq!(
            lines(&none, "0.3.0")[..2],
            [
                "daemon: fld cannot tell where its socket is: XDG_RUNTIME_DIR is not set",
                "service: none; fld setup installs a service that starts the daemon at login",
            ]
        );
    }

    #[test]
    fn a_silent_daemon_names_the_reason_and_doctor() {
        let shown = lines(
            &status(
                Daemon::Silent("no answer within 2 seconds".into()),
                None,
                None,
            ),
            "0.3.0",
        );
        assert_eq!(
            shown[..3],
            [
                "daemon: the socket accepts connections, but fld cannot read the answer: no answer within 2 seconds",
                "hint: run fld doctor",
                "socket: /run/user/1000/fold/fold.sock",
            ]
        );
    }

    #[test]
    fn an_unregistered_project_and_control_characters_in_its_name_are_shown_safely() {
        let odd = Some(Project {
            root: PathBuf::from("/home/ana/app"),
            registered: Registered::Listed("A\u{1b}[2Jpp".to_owned(), "app".to_owned()),
        });
        assert_eq!(
            lines(&status(Daemon::NotRunning, None, odd), "0.3.0")[3],
            "project: A\\u{1b}[2Jpp (app) in /home/ana/app"
        );
        let removed = Some(Project {
            root: PathBuf::from("/home/ana/app"),
            registered: Registered::Removed("App".to_owned(), "app".to_owned()),
        });
        assert_eq!(
            lines(&status(Daemon::NotRunning, None, removed), "0.3.0")[3],
            "project: App (app) in /home/ana/app, removed from fold; fld claude or fld init there brings it back"
        );
        let unlisted = Some(Project {
            root: PathBuf::from("/home/ana/app"),
            registered: Registered::Unlisted,
        });
        assert_eq!(
            lines(&status(Daemon::NotRunning, None, unlisted), "0.3.0")[3],
            "project: /home/ana/app, which this host's registry does not list"
        );
        let unknown = Some(Project {
            root: PathBuf::from("/home/ana/app"),
            registered: Registered::Unknown,
        });
        assert_eq!(
            lines(&status(Daemon::NotRunning, None, unknown), "0.3.0")[3],
            "Run fld --help for commands"
        );
    }

    #[test]
    fn the_answer_of_the_daemon_route_or_its_refusal_reads_as_a_running_daemon() {
        let answer = Response {
            status: 200,
            body: json!({"version": "0.3.0", "listen": "127.0.0.1:7420"}),
        };
        assert_eq!(super::running(&answer), running("0.3.0"));
        let full = Response {
            status: 200,
            body: json!({
                "version": "0.3.0",
                "listen": "127.0.0.1:7420",
                "pid": 4242,
                "started_at": 1_000,
                "started_by": "systemd",
                "shell": "bash",
                "shell_read_at": 1_250,
            }),
        };
        let Daemon::Running {
            start: Some(start), ..
        } = super::running(&full)
        else {
            panic!("no start");
        };
        assert_eq!(start.pid, 4242);
        assert_eq!(start.started_by, "systemd");
        assert_eq!(start.shell.as_deref(), Some("bash"));
        assert_eq!(start.shell_read_at.map(Timestamp::as_millis), Some(1_250));
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "not_found", "message": "no route"}}),
        };
        assert_eq!(
            super::running(&refused),
            Daemon::Running {
                version: None,
                listen: None,
                start: None,
            }
        );
    }

    #[test]
    fn a_daemon_on_every_address_is_named_by_its_loopback_address() {
        assert_eq!(web_url("0.0.0.0:7420"), "http://127.0.0.1:7420");
        assert_eq!(web_url("[::]:7420"), "http://[::1]:7420");
        assert_eq!(web_url("127.0.0.1:7420"), "http://127.0.0.1:7420");
        assert_eq!(web_url("192.168.1.5:7420"), "http://192.168.1.5:7420");
        assert_eq!(web_url("[::1]:9000"), "http://[::1]:9000");
        let everywhere = Daemon::Running {
            version: Some("0.3.0".to_owned()),
            listen: Some("0.0.0.0:7420".to_owned()),
            start: None,
        };
        assert_eq!(
            lines(&status(everywhere, None, None), "0.3.0")[0],
            "daemon: running fold 0.3.0, web interface http://127.0.0.1:7420; fld web prints a sign-in link"
        );
    }

    #[test]
    fn launchd_counts_an_agent_whose_state_is_running() {
        assert!(launchd_running(
            "gui/501/dev.omukk.fold = {\n\tactive count = 1\n\tstate = running\n\tpid = 4242\n}\n"
        ));
        assert!(!launchd_running(
            "gui/501/dev.omukk.fold = {\n\tstate = not running\n}\n"
        ));
    }

    fn host(dir: &Path, linux: bool) -> Host {
        Host {
            dirs: Dirs {
                home: Some(dir.to_owned()),
                config_home: Some(dir.join("config")),
                ..Dirs::default()
            },
            user: None,
            uid: 501,
            path: Some(dir.join("bin").into_os_string()),
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

    fn tool(dir: &Path, name: &str, script: &str) {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(name), script).unwrap();
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_service_is_installed_when_its_file_is_there_and_active_when_its_manager_says_so() {
        let dir = tempfile::TempDir::new().unwrap();
        let linux = host(dir.path(), true);
        assert_eq!(service(&linux), None);
        let unit = dir.path().join("config/systemd/user/fold.service");
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, "[Service]\n").unwrap();
        let inactive = Service {
            manager: Manager::Systemd,
            active: false,
        };
        assert_eq!(service(&linux), Some(inactive));
        tool(
            dir.path(),
            "systemctl",
            "#!/bin/sh\n[ \"$*\" = \"--user is-active fold.service\" ] && echo active\n",
        );
        assert_eq!(
            service(&linux),
            Some(Service {
                active: true,
                ..inactive
            })
        );

        let macos = host(dir.path(), false);
        assert_eq!(service(&macos), None);
        let agent = dir.path().join("Library/LaunchAgents/dev.omukk.fold.plist");
        std::fs::create_dir_all(agent.parent().unwrap()).unwrap();
        std::fs::write(&agent, "<plist/>\n").unwrap();
        tool(
            dir.path(),
            "launchctl",
            "#!/bin/sh\n[ \"$*\" = \"print gui/501/dev.omukk.fold\" ] && printf '\\tstate = running\\n'\n",
        );
        assert_eq!(
            service(&macos),
            Some(Service {
                manager: Manager::Launchd,
                active: true,
            })
        );
    }
}

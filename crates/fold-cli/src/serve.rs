use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use fold_cli::claude_files;
use fold_cli::daemon_client::{ClientConfig, ClientError};
use fold_core::Clock;
use fold_daemon::config::{LOCK_FILE, SOCKET_FILE};
use fold_daemon::projects::FolderFiles;
use fold_daemon::services::agents::Launcher;
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_daemon::{Daemon, DaemonConfig, DaemonError, SystemClock, shutdown_signal};
use serde::Deserialize;
use thiserror::Error;

/// The variable through which `fld claude --socket` hands its socket to the session's `fld mcp` and hooks.
pub(crate) const SOCKET_ENV: &str = "FOLD_SOCKET";

pub(crate) const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 7420);

#[derive(Args)]
pub struct ServeArgs {
    /// HTTP listen address; overrides daemon.toml (default 127.0.0.1:7420)
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// Unix socket path; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Serve the web files from this folder instead of the ones built into the binary
    #[arg(long)]
    web_dir: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ServeError {
    #[error(transparent)]
    Daemon(#[from] DaemonError),
    #[error("neither the XDG directory nor HOME is set")]
    NoHome,
    #[error("cannot read {path}: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },
    #[error("invalid configuration file {path}: {source}")]
    InvalidConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot listen for signals: {0}")]
    Signals(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl ServeError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Daemon(error) => error.code(),
            _ => None,
        }
    }
}

/// `$XDG_CONFIG_HOME/fold/daemon.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileConfig {
    pub(crate) listen: Option<SocketAddr>,
    pub(crate) socket: Option<PathBuf>,
    /// How many days a web sign-in lasts off loopback.
    pub(crate) session_days: Option<NonZeroU16>,
}

/// The directories the daemon uses, read from the environment once so the rules below stay pure.
#[derive(Debug, Default, Clone)]
pub(crate) struct Dirs {
    pub(crate) home: Option<PathBuf>,
    pub(crate) config_home: Option<PathBuf>,
    pub(crate) data_home: Option<PathBuf>,
    pub(crate) runtime_dir: Option<PathBuf>,
    /// True on macOS, which sets no `XDG_RUNTIME_DIR`: the runtime directory is then [`LIBRARY_RUNTIME_DIR`] below the home directory.
    pub(crate) library_runtime: bool,
}

/// The macOS runtime directory below the home directory. It is per user and outside `$TMPDIR`, which macOS empties of files left untouched for days, a running daemon's socket included; the path stays well inside the 104 bytes macOS allows a socket path.
pub(crate) const LIBRARY_RUNTIME_DIR: &str = "Library/Application Support/fold/run";

impl Dirs {
    pub(crate) fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        Self {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME"),
            data_home: var("XDG_DATA_HOME"),
            runtime_dir: var("XDG_RUNTIME_DIR"),
            library_runtime: cfg!(target_os = "macos"),
        }
    }

    pub(crate) fn config_file(&self) -> Result<PathBuf, ServeError> {
        Ok(self.config_base()?.join("fold").join("daemon.toml"))
    }

    /// `$XDG_CONFIG_HOME`, else `~/.config`.
    pub(crate) fn config_base(&self) -> Result<PathBuf, ServeError> {
        self.base(&self.config_home, ".config")
    }

    fn base(&self, xdg: &Option<PathBuf>, fallback: &str) -> Result<PathBuf, ServeError> {
        match (xdg, &self.home) {
            (Some(path), _) => Ok(path.clone()),
            (None, Some(home)) => Ok(home.join(fallback)),
            (None, None) => Err(ServeError::NoHome),
        }
    }
}

/// `$XDG_RUNTIME_DIR/fold`, which holds the daemon lock, the start lock and the default socket. On macOS without `XDG_RUNTIME_DIR` it is [`LIBRARY_RUNTIME_DIR`] below the home directory.
pub(crate) fn runtime_dir(dirs: &Dirs) -> Result<PathBuf, ServeError> {
    match (&dirs.runtime_dir, &dirs.home) {
        (Some(base), _) => Ok(base.join("fold")),
        (None, Some(home)) if dirs.library_runtime => Ok(home.join(LIBRARY_RUNTIME_DIR)),
        (None, _) => Err(DaemonError::RuntimeDirMissing.into()),
    }
}

/// The current-thread runtime that runs the one async exchange of a command.
pub(crate) fn current_thread_runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// The daemon lock in the runtime directory.
pub(crate) fn lock_path(dirs: &Dirs) -> Option<PathBuf> {
    runtime_dir(dirs).ok().map(|dir| dir.join(LOCK_FILE))
}

/// Where a command finds the daemon, and how it starts one: the socket of [`socket_path`] made absolute, the runtime directory for the start lock, and this binary as the daemon to start.
pub(crate) fn client_config(flag: Option<&Path>, dirs: &Dirs) -> Result<ClientConfig, ServeError> {
    let socket = socket_path(flag, dirs)?.ok_or(ClientError::RuntimeDirMissing)?;
    Ok(ClientConfig {
        socket: std::path::absolute(&socket).map_err(ClientError::io("resolve the socket path"))?,
        runtime_dir: runtime_dir(dirs).ok(),
        fold_exe: Some(std::env::current_exe().map_err(ClientError::io("find the fld binary"))?),
    })
}

/// The socket of every command: the `--socket` flag, else `socket` from `daemon.toml`, else `fold.sock` in the runtime directory. `None` when none of the three is known, so no daemon can be listening.
pub(crate) fn socket_path(flag: Option<&Path>, dirs: &Dirs) -> Result<Option<PathBuf>, ServeError> {
    if let Some(flag) = flag {
        return Ok(Some(flag.to_owned()));
    }
    let file = match dirs.config_file() {
        Ok(path) => read_file_config(&path)?,
        Err(_) => FileConfig::default(),
    };
    Ok(resolve_socket(
        None,
        &file,
        runtime_dir(dirs).ok().as_deref(),
    ))
}

fn resolve_socket(
    flag: Option<&Path>,
    file: &FileConfig,
    runtime_dir: Option<&Path>,
) -> Option<PathBuf> {
    flag.map(Path::to_path_buf)
        .or_else(|| file.socket.clone())
        .or_else(|| runtime_dir.map(|dir| dir.join(SOCKET_FILE)))
}

/// Flags win over the file, the file wins over the defaults.
fn build_config(
    args: &ServeArgs,
    dirs: &Dirs,
    file: &FileConfig,
) -> Result<DaemonConfig, ServeError> {
    let runtime_dir = runtime_dir(dirs)?;
    let data_dir = dirs.base(&dirs.data_home, ".local/share")?.join("fold");
    let listen = args.listen.or(file.listen).unwrap_or(DEFAULT_LISTEN);
    let socket = resolve_socket(args.socket.as_deref(), file, Some(&runtime_dir))
        .unwrap_or_else(|| runtime_dir.join(SOCKET_FILE));
    Ok(DaemonConfig {
        listen,
        socket,
        data_dir,
        runtime_dir,
        web_dir: args.web_dir.clone(),
        session_lifetime: file.session_days.map_or(DEFAULT_SESSION_LIFETIME, |days| {
            Duration::from_secs(u64::from(days.get()) * 24 * 60 * 60)
        }),
    })
}

pub(crate) fn read_file_config(path: &Path) -> Result<FileConfig, ServeError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_file_config(&text).map_err(|source| ServeError::InvalidConfig {
            path: path.to_owned(),
            source,
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(source) => Err(ServeError::ReadConfig {
            path: path.to_owned(),
            source,
        }),
    }
}

fn parse_file_config(text: &str) -> Result<FileConfig, toml::de::Error> {
    toml::from_str(text)
}

/// `RUST_LOG`'s filter, else `info`, with rmcp held at `warn` whatever `RUST_LOG` asks: rmcp logs the requests and notifications it handles with their arguments, and those can carry an agent's text.
fn log_filter(from_env: Option<tracing_subscriber::EnvFilter>) -> tracing_subscriber::EnvFilter {
    let filter = from_env.unwrap_or_else(|| tracing_subscriber::EnvFilter::new("info"));
    match "rmcp=warn".parse::<tracing_subscriber::filter::Directive>() {
        Ok(directive) => filter.add_directive(directive),
        Err(_) => filter,
    }
}

/// A project whose skills cannot be written still opens: its workers then find the skills a refresh or an earlier fold wrote.
fn write_skills(root: &Path) {
    if let Err(error) = claude_files::write_skills(root) {
        tracing::warn!(
            root = %root.display(),
            code = error.code(),
            "could not write fold's skills into the project folder"
        );
    }
}

pub fn run(args: &ServeArgs) -> Result<(), ServeError> {
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(log_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().ok(),
        ))
        .init();
    #[cfg(feature = "failpoints")]
    let _failpoints = fold_daemon::failpoints_from_env();
    let dirs = Dirs::from_env();
    let file = read_file_config(&dirs.config_file()?)?;
    let config = build_config(args, &dirs, &file)?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (chosen, started_by) = crate::shell_env::from_host(clock.as_ref());
    crate::shell_env::log(&chosen.environment);
    if let Some(path) = &chosen.path {
        // SAFETY: no other thread runs yet: the tokio runtime is built below, and neither the log subscriber nor the failpoints start one, so nothing reads the environment while it changes.
        unsafe { std::env::set_var("PATH", path) };
    }
    let launcher = Launcher {
        claude: chosen
            .environment
            .claude
            .clone()
            .unwrap_or_else(|| PathBuf::from("claude")),
        fold: None,
        env: chosen.workers,
    }
    .with_claude_location(|name| std::env::var_os(name));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    runtime.block_on(async {
        let signals = shutdown_signal().map_err(ServeError::Signals)?;
        let daemon = Daemon::start_on_host(
            config,
            clock,
            launcher,
            chosen.environment,
            started_by,
            FolderFiles::new(write_skills),
        )
        .await?;
        signals.await;
        tracing::info!("shutting down");
        daemon.shutdown().await;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rmcp_stays_at_warn_whatever_rust_log_asks() {
        use tracing_subscriber::EnvFilter;
        let default = log_filter(None).to_string();
        assert!(
            default.contains("rmcp=warn") && default.contains("info"),
            "{default}"
        );
        for asked in ["trace", "rmcp=trace,fold_daemon=debug", "rmcp=info"] {
            let filter = log_filter(Some(EnvFilter::new(asked))).to_string();
            assert!(filter.contains("rmcp=warn"), "{asked}: {filter}");
            assert!(
                !filter.contains("rmcp=trace") && !filter.contains("rmcp=info"),
                "{asked}: {filter}"
            );
        }
    }

    fn dirs() -> Dirs {
        Dirs {
            home: Some("/home/ana".into()),
            config_home: None,
            data_home: None,
            runtime_dir: Some("/run/user/1000".into()),
            library_runtime: false,
        }
    }

    fn args(listen: Option<&str>, socket: Option<&str>) -> ServeArgs {
        ServeArgs {
            listen: listen.map(|text| text.parse().unwrap()),
            socket: socket.map(PathBuf::from),
            web_dir: None,
        }
    }

    #[test]
    fn defaults_follow_the_xdg_directories() {
        let config = build_config(&args(None, None), &dirs(), &FileConfig::default()).unwrap();
        assert_eq!(config.listen, "127.0.0.1:7420".parse().unwrap());
        assert_eq!(config.socket, Path::new("/run/user/1000/fold/fold.sock"));
        assert_eq!(config.runtime_dir, Path::new("/run/user/1000/fold"));
        assert_eq!(config.data_dir, Path::new("/home/ana/.local/share/fold"));
        assert_eq!(
            dirs().config_file().unwrap(),
            Path::new("/home/ana/.config/fold/daemon.toml")
        );
    }

    #[test]
    fn xdg_variables_beat_the_home_fallbacks() {
        let dirs = Dirs {
            config_home: Some("/cfg".into()),
            data_home: Some("/data".into()),
            ..dirs()
        };
        let config = build_config(&args(None, None), &dirs, &FileConfig::default()).unwrap();
        assert_eq!(config.data_dir, Path::new("/data/fold"));
        assert_eq!(
            dirs.config_file().unwrap(),
            Path::new("/cfg/fold/daemon.toml")
        );
    }

    #[test]
    fn flags_beat_the_file_and_the_file_beats_the_defaults() {
        let file = parse_file_config("listen = \"127.0.0.1:9000\"\nsocket = \"/tmp/file.sock\"\n")
            .unwrap();
        let from_file = build_config(&args(None, None), &dirs(), &file).unwrap();
        assert_eq!(from_file.listen, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(from_file.socket, Path::new("/tmp/file.sock"));
        let from_flags = build_config(
            &args(Some("127.0.0.1:9100"), Some("/tmp/flag.sock")),
            &dirs(),
            &file,
        )
        .unwrap();
        assert_eq!(from_flags.listen, "127.0.0.1:9100".parse().unwrap());
        assert_eq!(from_flags.socket, Path::new("/tmp/flag.sock"));
    }

    #[test]
    fn the_session_lifetime_is_thirty_days_unless_the_file_sets_it() {
        let default = build_config(&args(None, None), &dirs(), &FileConfig::default()).unwrap();
        assert_eq!(
            default.session_lifetime,
            Duration::from_secs(30 * 24 * 60 * 60)
        );
        let file = parse_file_config("session_days = 7\n").unwrap();
        let week = build_config(&args(None, None), &dirs(), &file).unwrap();
        assert_eq!(week.session_lifetime, Duration::from_secs(7 * 24 * 60 * 60));
        assert!(parse_file_config("session_days = 0").is_err());
        assert!(parse_file_config("session_days = -1").is_err());
    }

    #[test]
    fn a_missing_runtime_directory_stops_the_start() {
        let dirs = Dirs {
            runtime_dir: None,
            ..dirs()
        };
        let error = build_config(&args(None, None), &dirs, &FileConfig::default()).unwrap_err();
        assert!(matches!(
            error,
            ServeError::Daemon(DaemonError::RuntimeDirMissing)
        ));
        assert_eq!(error.code(), Some("runtime_dir_missing"));
    }

    #[test]
    fn missing_home_and_data_home_is_an_error() {
        let dirs = Dirs {
            home: None,
            ..dirs()
        };
        let error = build_config(&args(None, None), &dirs, &FileConfig::default()).unwrap_err();
        assert!(matches!(error, ServeError::NoHome));
        assert_eq!(error.code(), None);
        assert!(dirs.config_file().is_err());
    }

    #[test]
    fn unknown_file_keys_are_refused() {
        assert!(parse_file_config("listn = \"x\"").is_err());
        assert!(parse_file_config("listen = \"not an address\"").is_err());
    }

    #[test]
    fn a_missing_file_means_defaults_and_a_bad_file_names_its_path() {
        let dir = std::env::temp_dir().join(format!("fold-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = read_file_config(&dir.join("absent.toml")).unwrap();
        assert!(missing.listen.is_none() && missing.socket.is_none());
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "listen = 5").unwrap();
        let error = read_file_config(&bad).unwrap_err();
        assert!(matches!(error, ServeError::InvalidConfig { .. }));
        assert!(error.to_string().contains("bad.toml"));
        let unreadable = read_file_config(&dir).unwrap_err();
        assert!(matches!(unreadable, ServeError::ReadConfig { .. }));
        let good = dir.join("good.toml");
        std::fs::write(&good, "listen = \"127.0.0.1:1\"").unwrap();
        assert_eq!(
            read_file_config(&good).unwrap().listen,
            Some("127.0.0.1:1".parse().unwrap())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_runtime_directory_is_the_fold_directory_under_xdg_runtime_dir() {
        assert_eq!(
            runtime_dir(&dirs()).unwrap(),
            Path::new("/run/user/1000/fold")
        );
        let no_runtime = Dirs {
            runtime_dir: None,
            ..dirs()
        };
        assert!(runtime_dir(&no_runtime).is_err());
    }

    #[test]
    fn on_macos_the_runtime_directory_falls_back_to_the_library_folder() {
        let library = Dirs {
            runtime_dir: None,
            library_runtime: true,
            ..dirs()
        };
        assert_eq!(
            runtime_dir(&library).unwrap(),
            Path::new("/home/ana/Library/Application Support/fold/run")
        );
        let config = build_config(&args(None, None), &library, &FileConfig::default()).unwrap();
        assert_eq!(
            config.socket,
            Path::new("/home/ana/Library/Application Support/fold/run/fold.sock")
        );
        let set = Dirs {
            library_runtime: true,
            ..dirs()
        };
        assert_eq!(runtime_dir(&set).unwrap(), Path::new("/run/user/1000/fold"));
        let homeless = Dirs {
            home: None,
            ..library
        };
        assert!(runtime_dir(&homeless).is_err());
        assert_eq!(Dirs::from_env().library_runtime, cfg!(target_os = "macos"));
    }
}

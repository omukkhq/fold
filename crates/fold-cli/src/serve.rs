use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use fold_daemon::{Daemon, DaemonConfig, DaemonError, SystemClock, shutdown_signal};
use serde::Deserialize;
use thiserror::Error;

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
}

/// The directories the daemon uses, read from the environment once so the rules below stay pure.
#[derive(Debug, Default, Clone)]
pub(crate) struct Dirs {
    pub(crate) home: Option<PathBuf>,
    pub(crate) config_home: Option<PathBuf>,
    pub(crate) data_home: Option<PathBuf>,
    pub(crate) runtime_dir: Option<PathBuf>,
}

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
        }
    }

    pub(crate) fn config_file(&self) -> Result<PathBuf, ServeError> {
        let base = self.base(&self.config_home, ".config")?;
        Ok(base.join("fold").join("daemon.toml"))
    }

    fn base(&self, xdg: &Option<PathBuf>, fallback: &str) -> Result<PathBuf, ServeError> {
        match (xdg, &self.home) {
            (Some(path), _) => Ok(path.clone()),
            (None, Some(home)) => Ok(home.join(fallback)),
            (None, None) => Err(ServeError::NoHome),
        }
    }
}

/// `$XDG_RUNTIME_DIR/fold`, which holds the daemon lock, the start lock and the default socket.
pub(crate) fn runtime_dir(dirs: &Dirs) -> Result<PathBuf, ServeError> {
    let base = dirs
        .runtime_dir
        .clone()
        .ok_or(DaemonError::RuntimeDirMissing)?;
    Ok(base.join("fold"))
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
        .or_else(|| runtime_dir.map(|dir| dir.join("fold.sock")))
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
        .unwrap_or_else(|| runtime_dir.join("fold.sock"));
    Ok(DaemonConfig {
        listen,
        socket,
        data_dir,
        runtime_dir,
        web_dir: args.web_dir.clone(),
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

pub fn run(args: &ServeArgs) -> Result<(), ServeError> {
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(
            // rmcp logs the notifications it receives at info, and a notification can carry an agent's text.
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rmcp=warn".into()),
        )
        .init();
    let dirs = Dirs::from_env();
    let file = read_file_config(&dirs.config_file()?)?;
    let config = build_config(args, &dirs, &file)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    runtime.block_on(async {
        let daemon = Daemon::start(config, Arc::new(SystemClock)).await?;
        shutdown_signal().await.map_err(ServeError::Signals)?;
        tracing::info!("shutting down");
        daemon.shutdown().await;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Dirs {
        Dirs {
            home: Some("/home/ana".into()),
            config_home: None,
            data_home: None,
            runtime_dir: Some("/run/user/1000".into()),
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
}

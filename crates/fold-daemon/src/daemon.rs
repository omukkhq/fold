use std::fs::{File, TryLockError};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use fold_core::{Clock, FoldError, codes};
use fold_store::{Store, StoreError};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::DaemonConfig;
use crate::http::{self, assets::AssetSource};
use crate::jobs::{self, Job};
use crate::mcp;
use crate::projects::Projects;
use crate::socket::SocketServer;

/// How long open HTTP connections get to finish after shutdown starts.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// What every request handler can reach.
pub struct DaemonState {
    pub registry: Store,
    pub projects: Projects,
    pub clock: Arc<dyn Clock>,
    pub assets: AssetSource,
    /// The IP address the HTTP listener is bound to. It fixes the auth mode at start and names the hosts the `/api` routes answer.
    pub listen: IpAddr,
    /// How long a session that `POST /api/v1/session` opens lasts.
    pub session_lifetime: Duration,
}

impl DaemonState {
    /// True off loopback, where every guarded `/api` route needs the `fold_session` cookie of a live session. `0.0.0.0` and `::` are off loopback, so they need it even for a request from `127.0.0.1`.
    pub fn auth_required(&self) -> bool {
        !self.listen.is_loopback()
    }
}

impl std::fmt::Debug for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonState").finish_non_exhaustive()
    }
}

pub type AppState = Arc<DaemonState>;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("the XDG runtime directory is not set")]
    RuntimeDirMissing,
    #[error("another daemon holds {path}")]
    AlreadyRunning { path: PathBuf },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        source: io::Error,
    },
}

impl DaemonError {
    fn io(context: &'static str) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Io { context, source }
    }

    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::RuntimeDirMissing => Some(codes::RUNTIME_DIR_MISSING),
            Self::AlreadyRunning { .. } => Some(codes::DAEMON_ALREADY_RUNNING),
            Self::Store(error) => error.code(),
            Self::Io { .. } => None,
        }
    }
}

impl From<DaemonError> for FoldError {
    fn from(error: DaemonError) -> Self {
        FoldError::new(
            error.code().unwrap_or(codes::INTERNAL_ERROR),
            error.to_string(),
        )
    }
}

pub struct Daemon;

impl Daemon {
    /// Takes the host lock, opens the registry and every registered project, ends the sessions a previous run left open, then binds the socket and the HTTP listener. `clock` goes to every project handle.
    pub async fn start(
        config: DaemonConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<RunningDaemon, DaemonError> {
        let lock = {
            let config = config.clone();
            tokio::task::spawn_blocking(move || prepare_runtime_dir(&config))
                .await
                .map_err(|error| DaemonError::Io {
                    context: "prepare the runtime directory",
                    source: io::Error::other(error),
                })??
        };

        let registry = Store::open_registry(&config.registry_path()).await?;
        let state: AppState = Arc::new(DaemonState {
            registry,
            projects: Projects::default(),
            clock: clock.clone(),
            assets: config
                .web_dir
                .clone()
                .map_or(AssetSource::Embedded, AssetSource::Dir),
            listen: config.listen.ip(),
            session_lifetime: config.session_lifetime,
        });
        for row in fold_store::projects::list_projects(state.registry.read()).await? {
            // A project that fails to open is logged and marked unavailable; the others start.
            let _ = state.projects.open(&row, clock.clone()).await;
        }
        for handle in state.projects.handles() {
            if let Err(error) = handle.agents().close_orphan_sessions().await {
                tracing::error!(project = %handle.slug, %error, "could not end orphan sessions");
            }
        }

        let (stop, stopped) = watch::channel(false);
        remove_stale_socket(&config.socket).await?;
        let socket = SocketServer::bind(
            &config.socket,
            http::socket_router(state.clone()),
            mcp::handshake::handler(state.clone()),
            stopped.clone(),
        )
        .await
        .map_err(DaemonError::io("bind the socket"))?;
        let listener = TcpListener::bind(config.listen)
            .await
            .map_err(DaemonError::io("bind the HTTP listener"))?;
        let http_addr = listener
            .local_addr()
            .map_err(DaemonError::io("read the HTTP address"))?;
        let gc_daily = jobs::spawn_gc_daily(state.clone(), jobs::JOB_TICK, stopped.clone());
        let presence = jobs::spawn_presence(state.clone(), jobs::PRESENCE_TICK, stopped.clone());
        let live = jobs::spawn_live(state.clone(), jobs::LIVE_TICK, stopped.clone());
        let http = tokio::spawn(serve_http(listener, http::router(state.clone()), stopped));
        tracing::info!(%http_addr, "daemon started");
        Ok(RunningDaemon {
            config,
            state,
            http_addr,
            socket,
            http,
            gc_daily,
            presence,
            live,
            stop,
            _lock: lock,
        })
    }
}

/// A started daemon. Dropping it without [`RunningDaemon::shutdown`] leaves the sockets to the runtime that is dropped with it.
#[derive(Debug)]
pub struct RunningDaemon {
    config: DaemonConfig,
    state: AppState,
    http_addr: SocketAddr,
    socket: SocketServer,
    http: JoinHandle<()>,
    gc_daily: Job,
    presence: Job,
    live: Job,
    stop: watch::Sender<bool>,
    /// The advisory lock lives as long as this file is open.
    _lock: File,
}

impl RunningDaemon {
    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn socket_path(&self) -> &Path {
        &self.config.socket
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn socket(&self) -> &SocketServer {
        &self.socket
    }

    /// Stops accepting, ends open sessions, checkpoints every WAL and removes the socket file.
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        self.socket.stopped().await;
        let abort = self.http.abort_handle();
        if tokio::time::timeout(DRAIN_TIMEOUT, self.http)
            .await
            .is_err()
        {
            abort.abort();
        }
        self.gc_daily.stopped(DRAIN_TIMEOUT).await;
        self.presence.stopped(DRAIN_TIMEOUT).await;
        self.live.stopped(DRAIN_TIMEOUT).await;
        for handle in self.state.projects.handles() {
            if let Err(error) = handle.agents().close_sessions_at_shutdown().await {
                tracing::error!(project = %handle.slug, %error, "could not end sessions");
            }
            if let Err(error) = handle.store.close().await {
                tracing::error!(project = %handle.slug, %error, "could not close the project database");
            }
        }
        if let Err(error) = self.state.registry.close().await {
            tracing::error!(%error, "could not close the registry database");
        }
        if let Err(error) = tokio::fs::remove_file(&self.config.socket).await {
            tracing::warn!(%error, "could not remove the socket file");
        }
        tracing::info!("daemon stopped");
    }
}

/// Resolves when the process receives SIGINT or SIGTERM.
pub async fn shutdown_signal() -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    Ok(())
}

async fn serve_http(listener: TcpListener, router: Router, mut stopped: watch::Receiver<bool>) {
    let served = axum::serve(listener, router).with_graceful_shutdown(async move {
        let _ = stopped.changed().await;
    });
    if let Err(error) = served.await {
        tracing::error!(%error, "HTTP listener failed");
    }
}

async fn remove_stale_socket(path: &Path) -> Result<(), DaemonError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DaemonError::io("remove the stale socket")(error)),
    }
}

/// Creates the private directories and takes the exclusive lock that makes this the only daemon on the host. The lock, not the socket, decides.
fn prepare_runtime_dir(config: &DaemonConfig) -> Result<File, DaemonError> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let make = |path: &Path| {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    };
    make(&config.runtime_dir).map_err(DaemonError::io("create the runtime directory"))?;
    std::fs::set_permissions(&config.runtime_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(DaemonError::io("restrict the runtime directory"))?;
    if let Some(dir) = config.socket_dir() {
        make(dir).map_err(DaemonError::io("create the socket directory"))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(config.lock_path())
        .map_err(DaemonError::io("open the lock file"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(DaemonError::AlreadyRunning {
            path: config.lock_path(),
        }),
        Err(TryLockError::Error(source)) => Err(DaemonError::Io {
            context: "lock the daemon lock file",
            source,
        }),
    }
}

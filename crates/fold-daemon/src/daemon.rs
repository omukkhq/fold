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
use crate::projects::{ProjectHandle, Projects, resume_workers};
use crate::services::agents::Launcher;
use crate::socket::SocketServer;

/// How long the open connections of the HTTP listener and of the socket get to finish after shutdown starts, and the jobs to stop.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// [`Daemon::start_with`] with the default programs for the worker processes.
    pub async fn start(
        config: DaemonConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<RunningDaemon, DaemonError> {
        Self::start_with(config, clock, Launcher::default()).await
    }

    /// Takes the host lock, opens the registry, binds the HTTP listener, opens every registered project through its start-up sequence, binds the socket, starts the jobs and the HTTP server, and only then resumes the workers of the projects, so a resumed worker's adapter finds the socket. A failed bind leaves no socket file, no worker and no open project database behind. `clock` goes to every project handle; `launcher` names the programs every worker process runs.
    pub async fn start_with(
        config: DaemonConfig,
        clock: Arc<dyn Clock>,
        launcher: Launcher,
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
        let bound = async {
            let listener = TcpListener::bind(config.listen)
                .await
                .map_err(DaemonError::io("bind the HTTP listener"))?;
            let http_addr = listener
                .local_addr()
                .map_err(DaemonError::io("read the HTTP address"))?;
            Ok::<_, DaemonError>((listener, http_addr))
        };
        let (listener, http_addr) = match bound.await {
            Ok(bound) => bound,
            Err(error) => {
                let _ = registry.close().await;
                return Err(error);
            }
        };
        let state: AppState = Arc::new(DaemonState {
            projects: Projects::default()
                .with_launcher(launcher)
                .with_registry(registry.clone()),
            registry,
            clock: clock.clone(),
            assets: config
                .web_dir
                .clone()
                .map_or(AssetSource::Embedded, AssetSource::Dir),
            listen: config.listen.ip(),
            session_lifetime: config.session_lifetime,
        });
        let rows = match fold_store::projects::list_projects(state.registry.read()).await {
            Ok(rows) => rows,
            Err(error) => {
                let _ = state.registry.close().await;
                return Err(error.into());
            }
        };
        let mut opened = Vec::new();
        for row in rows {
            // A project that fails to open is logged and marked unavailable; the others start.
            if let Ok(handle) = state.projects.open_at_start(&row, clock.clone()).await {
                opened.push(handle);
            }
        }

        let (stop, stopped) = watch::channel(false);
        let socket = match bind_socket(&config, &state, stopped.clone()).await {
            Ok(socket) => socket,
            Err(error) => {
                for handle in state.projects.handles() {
                    handle.bus.close();
                    stop_project(&handle).await;
                }
                let _ = state.registry.close().await;
                return Err(error);
            }
        };
        let gc_daily = jobs::spawn_gc_daily(state.clone(), jobs::JOB_TICK, stopped.clone());
        let presence = jobs::spawn_presence(state.clone(), jobs::PRESENCE_TICK, stopped.clone());
        let live = jobs::spawn_live(state.clone(), jobs::LIVE_TICK, stopped.clone());
        let auto_checkpoints = jobs::spawn_auto_checkpoints(
            state.clone(),
            jobs::AUTO_CHECKPOINT_TICK,
            stopped.clone(),
        );
        let leases = jobs::spawn_lease_sweeper(state.clone(), jobs::LEASE_TICK, stopped.clone());
        let stalls = jobs::spawn_stalls(state.clone(), jobs::STALL_TICK, stopped.clone());
        let auto_integrate =
            jobs::spawn_auto_integrate(state.clone(), jobs::AUTO_INTEGRATE_TICK, stopped.clone());
        let worker_idle =
            jobs::spawn_worker_idle(state.clone(), jobs::WORKER_IDLE_TICK, stopped.clone());
        let http = tokio::spawn(serve_http(listener, http::router(state.clone()), stopped));
        for handle in &opened {
            resume_workers(handle).await;
        }
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
            auto_checkpoints,
            leases,
            stalls,
            auto_integrate,
            worker_idle,
            stop,
            _lock: lock,
        })
    }
}

/// Binds the socket at the configured path, after removing a stale file there.
async fn bind_socket(
    config: &DaemonConfig,
    state: &AppState,
    stopped: watch::Receiver<bool>,
) -> Result<SocketServer, DaemonError> {
    remove_stale_socket(&config.socket).await?;
    SocketServer::bind(
        &config.socket,
        http::socket_router(state.clone()),
        mcp::handshake::handler(state.clone()),
        stopped,
    )
    .await
    .map_err(DaemonError::io("bind the socket"))
}

/// Closes one project after its workers stopped and its bus closed: the live state of its workspaces is written and let go, its commit writer and push delivery end, every open session ends and its database is checkpointed and closed.
async fn stop_project(handle: &ProjectHandle) {
    for live in handle.live.all() {
        handle.live.detach(live.view.workspace).await;
    }
    handle.stop_tasks(DRAIN_TIMEOUT).await;
    if let Err(error) = handle.agents().close_sessions_at_shutdown().await {
        tracing::error!(project = %handle.slug, %error, "could not end sessions");
    }
    if let Err(error) = handle.store.close().await {
        tracing::error!(project = %handle.slug, %error, "could not close the project database");
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
    auto_checkpoints: Job,
    leases: Job,
    stalls: Job,
    auto_integrate: Job,
    worker_idle: Job,
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

    /// Stops accepting and stops the jobs, ends the worker processes while their sessions are still served (their rows keep their state, so the next start resumes them), closes every project's bus, which ends the event streams, the waits and the commit writers, drains the socket and HTTP connections, then ends open sessions, checkpoints every WAL and removes the socket file.
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        self.gc_daily.stopped(DRAIN_TIMEOUT).await;
        self.presence.stopped(DRAIN_TIMEOUT).await;
        self.live.stopped(DRAIN_TIMEOUT).await;
        self.auto_checkpoints.stopped(DRAIN_TIMEOUT).await;
        self.leases.stopped(DRAIN_TIMEOUT).await;
        self.stalls.stopped(DRAIN_TIMEOUT).await;
        self.auto_integrate.stopped(DRAIN_TIMEOUT).await;
        self.worker_idle.stopped(DRAIN_TIMEOUT).await;
        let handles = self.state.projects.handles();
        for handle in &handles {
            handle.workers.shutdown().await;
        }
        for handle in &handles {
            handle.bus.close();
        }
        self.socket.stopped().await;
        let abort = self.http.abort_handle();
        if tokio::time::timeout(DRAIN_TIMEOUT, self.http)
            .await
            .is_err()
        {
            abort.abort();
        }
        for handle in &handles {
            stop_project(handle).await;
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

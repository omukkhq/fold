use std::future::Future;
use std::io;
use std::os::fd::AsFd;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use axum::{Extension, Router};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rustix::net::RecvFlags;
use tokio::io::Interest;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

use crate::daemon::DRAIN_TIMEOUT;

/// Takes over a connection whose first byte is `{`. The stream is untouched: the handler reads the handshake line itself.
pub type HandshakeHandler =
    Arc<dyn Fn(UnixStream, SocketPeer) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The process at the other end of a socket connection, as the kernel reports it when the connection is accepted. A pid the peer writes itself is never trusted. Every HTTP request of the connection carries it as an extension.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SocketPeer {
    /// `None` when the platform reports no pid; such a peer counts as outside every worker.
    pub pid: Option<u32>,
}

impl SocketPeer {
    fn of(stream: &UnixStream) -> Self {
        let pid = stream
            .peer_cred()
            .ok()
            .and_then(|cred| cred.pid())
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid != 0);
        Self { pid }
    }
}

const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// The Unix socket listener. One socket carries two protocols, told apart by the first byte of a connection: `{` starts the JSON handshake, anything else is HTTP/1.1.
pub struct SocketServer {
    handler: Arc<RwLock<HandshakeHandler>>,
    task: JoinHandle<()>,
    drain: watch::Sender<bool>,
}

impl std::fmt::Debug for SocketServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketServer").finish_non_exhaustive()
    }
}

impl SocketServer {
    /// Binds `path`, restricts it to its owner and starts accepting until `shutdown` changes. A stale file at `path` must already be gone. The handshake handler comes with the bind, so no connection is ever accepted without one.
    pub async fn bind(
        path: &Path,
        router: Router,
        handshake: HandshakeHandler,
        shutdown: watch::Receiver<bool>,
    ) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        restrict(path).await?;
        let handler = Arc::new(RwLock::new(handshake));
        let (drain, draining) = watch::channel(false);
        let task = tokio::spawn(accept_loop(
            listener,
            router,
            handler.clone(),
            shutdown,
            draining,
        ));
        Ok(Self {
            handler,
            task,
            drain,
        })
    }

    /// Replaces the handshake handler; connections that arrive later use it.
    pub fn on_handshake(&self, handler: HandshakeHandler) {
        *self.handler.write().unwrap_or_else(PoisonError::into_inner) = handler;
    }

    /// Ends the open connections and waits until they are closed; the accept loop stops at the `shutdown` of [`SocketServer::bind`]. Each connection finishes what it is doing: an HTTP connection its request, an MCP session the calls in flight, whose adapter then reads the end of the stream. A connection still open after [`DRAIN_TIMEOUT`] is cut. Until this is called, the connections that were open at the shutdown keep being served, so the daemon can stop its worker processes while their sessions still answer.
    pub async fn stopped(self) {
        self.drain.send_replace(true);
        let _ = self.task.await;
    }
}

async fn restrict(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await
}

async fn accept_loop(
    listener: UnixListener,
    router: Router,
    handler: Arc<RwLock<HandshakeHandler>>,
    mut shutdown: watch::Receiver<bool>,
    mut draining: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    loop {
        // Finished connections leave the set so it does not grow for the daemon's lifetime.
        while connections.try_join_next().is_some() {}
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let handler = handler.read().unwrap_or_else(PoisonError::into_inner).clone();
                    connections.spawn(serve_connection(
                        stream,
                        router.clone(),
                        handler,
                        draining.clone(),
                    ));
                }
                Err(error) => {
                    tracing::warn!(%error, "socket accept failed");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
        }
    }
    drop(listener);
    drained(&mut draining).await;
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        connections.shutdown().await;
    }
}

/// Resolves once [`SocketServer::stopped`] starts the drain, or at once when the server was dropped without it.
async fn drained(draining: &mut watch::Receiver<bool>) {
    let _ = draining.wait_for(|drain| *drain).await;
}

async fn serve_connection(
    stream: UnixStream,
    router: Router,
    handler: HandshakeHandler,
    draining: watch::Receiver<bool>,
) {
    let peer = SocketPeer::of(&stream);
    match first_byte(&stream).await {
        Some(b'{') => serve_handshake(stream, peer, handler, draining).await,
        Some(_) => serve_http(stream, router.layer(Extension(peer)), draining).await,
        None => {}
    }
}

/// At the drain the connection stops reading: the MCP session reads the end of its input, sends the answers of the calls still in flight and ends, as it does when an adapter goes away.
async fn serve_handshake(
    stream: UnixStream,
    peer: SocketPeer,
    handler: HandshakeHandler,
    mut draining: watch::Receiver<bool>,
) {
    let reader = stream.as_fd().try_clone_to_owned();
    let session = handler(stream, peer);
    tokio::pin!(session);
    tokio::select! {
        () = &mut session => return,
        () = drained(&mut draining) => {}
    }
    if let Ok(reader) = reader {
        let _ = rustix::net::shutdown(&reader, rustix::net::Shutdown::Read);
    }
    session.await;
}

/// Looks at the first byte without consuming it; `None` when the peer closed first. The handshake handler and hyper both read the connection from its first byte.
async fn first_byte(stream: &UnixStream) -> Option<u8> {
    let mut first = [0_u8; 1];
    loop {
        stream.readable().await.ok()?;
        let peeked = stream.try_io(Interest::READABLE, || {
            rustix::net::recv(stream, &mut first, RecvFlags::PEEK)
                .map(|(received, _)| received)
                .map_err(io::Error::from)
        });
        match peeked {
            Ok(0) => return None,
            Ok(_) => return Some(first[0]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return None,
        }
    }
}

async fn serve_http(stream: UnixStream, router: Router, mut draining: watch::Receiver<bool>) {
    let service = TowerToHyperService::new(router);
    let connection = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(connection);
    let served = tokio::select! {
        served = &mut connection => served,
        () = drained(&mut draining) => {
            connection.as_mut().graceful_shutdown();
            connection.await
        }
    };
    if let Err(error) = served {
        tracing::debug!(%error, "socket connection ended with an error");
    }
}

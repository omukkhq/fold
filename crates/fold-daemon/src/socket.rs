use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::Router;
use fold_core::codes;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rustix::net::RecvFlags;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, Interest};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

/// Takes over a connection whose first byte is `{`. The stream is untouched: the handler reads the handshake line itself.
pub type HandshakeHandler =
    Arc<dyn Fn(UnixStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// The Unix socket listener. One socket carries two protocols, told apart by the first byte of a connection: `{` starts the JSON handshake, anything else is HTTP/1.1.
pub struct SocketServer {
    handler: Arc<RwLock<Option<HandshakeHandler>>>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for SocketServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketServer").finish_non_exhaustive()
    }
}

impl SocketServer {
    /// Binds `path`, restricts it to its owner and starts accepting. A stale file at `path` must already be gone.
    pub async fn bind(
        path: &Path,
        router: Router,
        shutdown: watch::Receiver<bool>,
    ) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        restrict(path).await?;
        let handler = Arc::new(RwLock::new(None));
        let task = tokio::spawn(accept_loop(listener, router, handler.clone(), shutdown));
        Ok(Self { handler, task })
    }

    /// Installs the handshake handler; connections that arrive later use it.
    pub fn on_handshake(&self, handler: HandshakeHandler) {
        if let Ok(mut slot) = self.handler.write() {
            *slot = Some(handler);
        }
    }

    /// Waits until the accept loop has stopped and open connections are closed.
    pub async fn stopped(self) {
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
    handler: Arc<RwLock<Option<HandshakeHandler>>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    loop {
        // Finished connections leave the set so it does not grow for the daemon's lifetime.
        while connections.try_join_next().is_some() {}
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let handler = handler.read().ok().and_then(|slot| slot.clone());
                    connections.spawn(serve_connection(stream, router.clone(), handler));
                }
                Err(error) => {
                    tracing::warn!(%error, "socket accept failed");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
        }
    }
    drop(listener);
    connections.shutdown().await;
}

async fn serve_connection(stream: UnixStream, router: Router, handler: Option<HandshakeHandler>) {
    match first_byte(&stream).await {
        Some(b'{') => match handler {
            Some(handler) => handler(stream).await,
            None => refuse_handshake(stream).await,
        },
        Some(_) => serve_http(stream, router).await,
        None => {}
    }
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

async fn serve_http(stream: UnixStream, router: Router) {
    let service = TowerToHyperService::new(router);
    let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
    if let Err(error) = connection.with_upgrades().await {
        tracing::debug!(%error, "socket connection ended with an error");
    }
}

async fn refuse_handshake(mut stream: UnixStream) {
    // Unread bytes at close would turn the reply into a connection reset for the client.
    let mut sent = [0_u8; 4096];
    let _ = stream.read(&mut sent).await;
    let line = json!({
        "error": {
            "code": codes::HANDSHAKE_UNAVAILABLE,
            "message": "this daemon does not accept the handshake yet",
            "details": {},
        }
    });
    let mut bytes = line.to_string().into_bytes();
    bytes.push(b'\n');
    if stream.write_all(&bytes).await.is_ok() {
        let _ = stream.shutdown().await;
    }
}

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST, HeaderValue};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use thiserror::Error;
use tokio::net::UnixStream;

pub use fold_daemon::http::actor::HUMAN_HEADER;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_BACKOFF: Duration = Duration::from_millis(10);
const MAX_BACKOFF: Duration = Duration::from_millis(200);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot tell who you are: set FOLD_HUMAN")]
    HumanUnknown,
    #[error("no daemon listens on {}", path.display())]
    NoDaemon { path: PathBuf },
    #[error("the name in FOLD_HUMAN or USER cannot go in a header")]
    HumanInvalid,
    #[error("the daemon did not open {} within 10 seconds", path.display())]
    StartTimeout { path: PathBuf },
    #[error("the daemon stopped while it started; run fold serve to see why")]
    StartFailed,
    #[error(
        "XDG_RUNTIME_DIR is not set, so fold cannot start the daemon; set it, or pass --socket of a running daemon"
    )]
    RuntimeDirMissing,
    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("the HTTP exchange with the daemon failed: {0}")]
    Http(#[from] hyper::Error),
    #[error("the daemon answered with a body that is not JSON")]
    Body,
}

fn io_error(context: &'static str) -> impl FnOnce(io::Error) -> ClientError {
    move |source| ClientError::Io { context, source }
}

/// Where the daemon is, and how to start it when it is not running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub socket: PathBuf,
    /// `$XDG_RUNTIME_DIR/fold`, which holds `start.lock`. Without it a client reaches a running daemon but cannot start one.
    pub runtime_dir: Option<PathBuf>,
    /// The binary started as `fold serve --socket <socket>`. `None` makes a client that never starts a daemon: an absent socket is [`ClientError::NoDaemon`].
    pub fold_exe: Option<PathBuf>,
}

/// The first of `FOLD_HUMAN`, `USER` and `LOGNAME` that is set and not empty.
pub fn human_from(
    fold_human: Option<OsString>,
    user: Option<OsString>,
    logname: Option<OsString>,
) -> Result<String, ClientError> {
    [fold_human, user, logname]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or(ClientError::HumanUnknown)
}

/// The human from the environment, else the account name of this process's user in `/etc/passwd`.
pub fn human_from_env() -> Result<String, ClientError> {
    human_from(
        std::env::var_os("FOLD_HUMAN"),
        std::env::var_os("USER"),
        std::env::var_os("LOGNAME"),
    )
    .or_else(|error| os_account().ok_or(error))
}

fn os_account() -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let uid = std::fs::metadata("/proc/self").ok()?.uid();
    account_name(&std::fs::read_to_string("/etc/passwd").ok()?, uid)
}

/// The name of the `passwd` entry for `uid`: the first field of the line whose third field is `uid`.
pub fn account_name(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let id = fields.nth(1)?;
        (!name.is_empty() && id.parse() == Ok(uid)).then(|| name.to_owned())
    })
}

/// Connects to the daemon socket, and starts the daemon first when the socket is absent. The start lock makes clients that find no socket at the same time start one daemon: the first one starts it, and the others find its socket once they hold the lock.
pub async fn connect(config: &ClientConfig) -> Result<UnixStream, ClientError> {
    match UnixStream::connect(&config.socket).await {
        Ok(stream) => return Ok(stream),
        Err(error) if absent(&error) => {}
        Err(error) => return Err(io_error("connect to the daemon socket")(error)),
    }
    let Some(fold_exe) = config.fold_exe.clone() else {
        return Err(ClientError::NoDaemon {
            path: config.socket.clone(),
        });
    };
    let config = config.clone();
    let stream = tokio::task::spawn_blocking(move || start_and_connect(&config, &fold_exe))
        .await
        .map_err(|error| io_error("start the daemon")(io::Error::other(error)))??;
    stream
        .set_nonblocking(true)
        .map_err(io_error("connect to the daemon socket"))?;
    UnixStream::from_std(stream).map_err(io_error("connect to the daemon socket"))
}

/// A socket file without a listener is a daemon that did not stop cleanly; the next daemon removes it.
fn absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

fn start_and_connect(
    config: &ClientConfig,
    fold_exe: &Path,
) -> Result<std::os::unix::net::UnixStream, ClientError> {
    let runtime_dir = config
        .runtime_dir
        .as_ref()
        .ok_or(ClientError::RuntimeDirMissing)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(runtime_dir)
        .map_err(io_error("create the runtime directory"))?;
    std::fs::set_permissions(runtime_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(io_error("restrict the runtime directory"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(runtime_dir.join("start.lock"))
        .map_err(io_error("open the start lock"))?;
    lock.lock().map_err(io_error("take the start lock"))?;
    if let Some(stream) = try_connect(config)? {
        return Ok(stream);
    }
    let mut daemon = Command::new(fold_exe)
        .arg("serve")
        .arg("--socket")
        .arg(&config.socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // The daemon outlives this session, so it must not keep a task workspace as its working directory.
        .current_dir("/")
        .process_group(0)
        .spawn()
        .map_err(io_error("start the daemon"))?;
    let deadline = Instant::now() + START_TIMEOUT;
    let mut backoff = FIRST_BACKOFF;
    loop {
        if let Some(stream) = try_connect(config)? {
            reap(daemon);
            return Ok(stream);
        }
        if daemon
            .try_wait()
            .map_err(io_error("watch the daemon"))?
            .is_some()
        {
            return Err(ClientError::StartFailed);
        }
        if Instant::now() >= deadline {
            reap(daemon);
            return Err(ClientError::StartTimeout {
                path: config.socket.clone(),
            });
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn try_connect(
    config: &ClientConfig,
) -> Result<Option<std::os::unix::net::UnixStream>, ClientError> {
    match std::os::unix::net::UnixStream::connect(&config.socket) {
        Ok(stream) => Ok(Some(stream)),
        Err(error) if absent(&error) => Ok(None),
        Err(error) => Err(io_error("connect to the daemon socket")(error)),
    }
}

/// The daemon outlives this process; a thread waits for it so it never stays a zombie while this process runs.
fn reap(mut daemon: Child) {
    std::thread::spawn(move || daemon.wait());
}

/// An answer from the daemon's HTTP API.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    /// `Value::Null` for an empty body.
    pub body: Value,
}

/// Sends the CLI's HTTP/1.1 requests over the socket, one connection per request.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    config: ClientConfig,
    human: Option<String>,
}

impl DaemonClient {
    /// The human is checked only once a daemon answers the connection, so a command that finds no daemon never needs one.
    pub fn new(config: ClientConfig, human: Option<String>) -> Self {
        Self { config, human }
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Response, ClientError> {
        let stream = connect(&self.config).await?;
        let human = self.human.as_deref().ok_or(ClientError::HumanUnknown)?;
        let human =
            HeaderValue::from_bytes(human.as_bytes()).map_err(|_| ClientError::HumanInvalid)?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        let driver = tokio::spawn(connection);
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, "fold")
            .header(HUMAN_HEADER, human);
        let bytes = match body {
            Some(body) => {
                request = request.header(CONTENT_TYPE, "application/json");
                Bytes::from(body.to_string())
            }
            None => Bytes::new(),
        };
        let request = request
            .body(Full::new(bytes))
            .map_err(|error| io_error("build the request")(io::Error::other(error)))?;
        let response = sender.send_request(request).await?;
        let status = response.status().as_u16();
        let bytes = response.into_body().collect().await?.to_bytes();
        driver.abort();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| ClientError::Body)?
        };
        Ok(Response { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(text: &str) -> Option<OsString> {
        Some(OsString::from(text))
    }

    #[test]
    fn fold_human_wins_then_user_then_logname() {
        assert_eq!(
            human_from(os("lead"), os("ana"), os("ana2")).unwrap(),
            "lead"
        );
        assert_eq!(human_from(None, os("ana"), os("ana2")).unwrap(), "ana");
        assert_eq!(human_from(os(""), os("ana"), None).unwrap(), "ana");
        assert_eq!(human_from(None, os(""), os("ana2")).unwrap(), "ana2");
        assert!(matches!(
            human_from(None, None, os("")),
            Err(ClientError::HumanUnknown)
        ));
    }

    #[test]
    fn the_account_name_comes_from_the_passwd_line_of_the_uid() {
        let passwd = "root:x:0:0:root:/root:/bin/sh\n:x:7:7::/:/bin/sh\nbroken\nana:x:1000:1000:Ana:/home/ana:/bin/zsh\n";
        assert_eq!(account_name(passwd, 0).as_deref(), Some("root"));
        assert_eq!(account_name(passwd, 1000).as_deref(), Some("ana"));
        assert_eq!(account_name(passwd, 7), None);
        assert_eq!(account_name(passwd, 42), None);
    }

    #[tokio::test]
    async fn the_human_is_checked_only_when_a_daemon_answers() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = |socket: &str| ClientConfig {
            socket: dir.path().join(socket),
            runtime_dir: None,
            fold_exe: None,
        };
        let request = |client: DaemonClient| async move {
            client.request(Method::GET, "/", None).await.unwrap_err()
        };
        let error = request(DaemonClient::new(config("none.sock"), None)).await;
        assert!(matches!(error, ClientError::NoDaemon { .. }), "{error}");
        assert!(error.to_string().ends_with("none.sock"));

        let _listener =
            std::os::unix::net::UnixListener::bind(dir.path().join("live.sock")).unwrap();
        let error = request(DaemonClient::new(config("live.sock"), None)).await;
        assert!(matches!(error, ClientError::HumanUnknown), "{error}");
        let error = request(DaemonClient::new(config("live.sock"), Some("a\nb".into()))).await;
        assert!(matches!(error, ClientError::HumanInvalid), "{error}");
    }
}

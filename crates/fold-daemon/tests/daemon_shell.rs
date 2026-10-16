use fold_daemon::http::host::CLIENT_HEADER;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{Clock, ManualClock, Timestamp};
use fold_daemon::http::{router, socket_router};
use fold_daemon::{Daemon, DaemonConfig, DaemonError, RunningDaemon};
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::{TestProject, eventually};
use fold_store::{Store, StoreError};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::Row;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use tower::ServiceExt;

#[cfg(test)]
const WAIT: Duration = Duration::from_secs(10);

#[cfg(test)]
struct Host {
    _dir: TempDir,
    config: DaemonConfig,
    clock: ManualClock,
}

#[cfg(test)]
impl Host {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: dir.path().join("run").join("fold.sock"),
            data_dir: dir.path().join("data"),
            runtime_dir: dir.path().join("run"),
            web_dir: None,
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        };
        Self {
            _dir: dir,
            config,
            clock: ManualClock::new(),
        }
    }

    async fn registry(&self) -> Store {
        Store::open_registry(&self.config.registry_path())
            .await
            .unwrap()
    }

    async fn register(&self, slug: &str, project: &TestProject) {
        let registry = self.registry().await;
        add(&registry, slug, project.paths().root(), self.clock.now()).await;
        registry.close().await.unwrap();
    }

    async fn start(&self) -> RunningDaemon {
        Daemon::start(self.config.clone(), Arc::new(self.clock.clone()))
            .await
            .unwrap()
    }
}

#[cfg(test)]
async fn add(registry: &Store, slug: &str, root: &Path, now: Timestamp) {
    let row = ProjectRow {
        slug: slug.to_owned(),
        name: format!("Project {slug}"),
        root: root.to_string_lossy().into_owned(),
    };
    registry
        .write(async |tx| {
            projects::insert_project(tx, &row, now).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

#[cfg(test)]
async fn call(
    router: axum::Router,
    method: Method,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(path);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |text| Body::from(text.to_owned())))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[cfg(test)]
/// Sends one HTTP/1.1 request on a fresh connection and returns the status and JSON body.
async fn raw_http<S>(mut stream: S, method: &str, path: &str, body: &str) -> (u16, Value)
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n{CLIENT_HEADER}: test\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8(response).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split_once("\r\n\r\n").unwrap().1;
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

#[cfg(test)]
fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

#[cfg(test)]
async fn count(store: &Store, sql: &'static str) -> i64 {
    sqlx::query(sql)
        .fetch_one(store.read())
        .await
        .unwrap()
        .get::<i64, _>(0)
}

#[tokio::test]
async fn the_socket_is_private_and_a_second_daemon_is_refused() {
    let host = Host::new();
    let daemon = host.start().await;
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(daemon.socket_path()), 0o600);
    assert_eq!(mode(daemon.socket_path().parent().unwrap()), 0o700);
    assert_eq!(mode(&host.config.lock_path()), 0o600);

    let second = Daemon::start(host.config.clone(), Arc::new(host.clock.clone())).await;
    let error = second.unwrap_err();
    assert!(matches!(error, DaemonError::AlreadyRunning { .. }));
    assert_eq!(error.code(), Some("daemon_already_running"));
    assert!(daemon.socket_path().exists());

    daemon.shutdown().await;
    host.start().await.shutdown().await;
}

#[tokio::test]
async fn a_non_loopback_daemon_starts_and_its_api_needs_the_session_cookie() {
    let mut host = Host::new();
    host.config.listen = "0.0.0.0:0".parse().unwrap();
    let daemon = host.start().await;
    assert!(daemon.state().auth_required());
    let tcp = TcpStream::connect(daemon.http_addr()).await.unwrap();
    let (status, body) = raw_http(tcp, "GET", "/api/v1/projects", "").await;
    assert_eq!((status, code(&body)), (401, "auth_required"));
    let unix = UnixStream::connect(daemon.socket_path()).await.unwrap();
    let (status, body) = raw_http(unix, "GET", "/api/v1/projects", "").await;
    assert_eq!((status, body), (200, json!({"projects": []})));
    daemon.shutdown().await;
}

#[tokio::test]
async fn projects_are_listed_and_unknown_routes_and_slugs_use_the_envelope() {
    let host = Host::new();
    let alpha = TestProject::new().await;
    host.register("alpha", &alpha).await;
    let daemon = host.start().await;
    let state = daemon.state().clone();

    let (status, body) = call(router(state.clone()), Method::GET, "/api/v1/projects", None).await;
    assert_eq!(status, StatusCode::OK);
    let project = &body["projects"][0];
    assert_eq!(
        (
            project["slug"].as_str(),
            project["name"].as_str(),
            project["available"].as_bool()
        ),
        (Some("alpha"), Some("Project alpha"), Some(true))
    );
    assert_eq!(project["overview"]["counts"]["backlog"], 0);

    let (status, body) = call(
        router(state.clone()),
        Method::GET,
        "/api/v1/projects/alpha",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "alpha");

    let (status, body) = call(router(state.clone()), Method::GET, "/api/v1/nothing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not_found");
    assert_eq!(body["error"]["details"], json!({}));

    let (status, body) = call(
        router(state.clone()),
        Method::DELETE,
        "/api/v1/projects",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not_found");

    let (status, body) = call(router(state), Method::GET, "/api/v1/projects/ghost", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "project_not_found");
    daemon.shutdown().await;
}

#[tokio::test]
async fn opening_a_project_works_over_the_socket_only_and_is_idempotent() {
    let host = Host::new();
    let daemon = host.start().await;
    let state = daemon.state().clone();
    let late = TestProject::new().await;
    let registry = host.registry().await;
    add(&registry, "late", late.paths().root(), host.clock.now()).await;

    let (_, body) = call(router(state.clone()), Method::GET, "/api/v1/projects", None).await;
    assert_eq!(body["projects"][0]["available"], false);
    let (status, body) = call(
        router(state.clone()),
        Method::GET,
        "/api/v1/projects/late",
        None,
    )
    .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::NOT_FOUND, "project_not_found")
    );

    let open = r#"{"slug":"late"}"#;
    let tcp = TcpStream::connect(daemon.http_addr()).await.unwrap();
    let (status, body) = raw_http(tcp, "POST", "/api/v1/projects", open).await;
    assert_eq!(status, 404);
    assert_eq!(code(&body), "not_found");
    let (status, body) = call(
        router(state.clone()),
        Method::POST,
        "/api/v1/projects",
        Some(open),
    )
    .await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "not_found"));

    for _ in 0..2 {
        let socket = UnixStream::connect(daemon.socket_path()).await.unwrap();
        let (status, body) = raw_http(socket, "POST", "/api/v1/projects", open).await;
        assert_eq!(status, 200);
        assert_eq!(body, json!({"slug": "late", "name": "Project late"}));
    }
    assert_eq!(state.projects.handles().len(), 1);
    let (_, body) = call(router(state.clone()), Method::GET, "/api/v1/projects", None).await;
    assert_eq!(body["projects"][0]["available"], true);
    let (status, _) = call(
        router(state.clone()),
        Method::GET,
        "/api/v1/projects/late",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let socket = || socket_router(state.clone());
    let (status, body) = call(
        socket(),
        Method::POST,
        "/api/v1/projects",
        Some(r#"{"slug":"ghost"}"#),
    )
    .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::NOT_FOUND, "project_not_found")
    );
    let (status, body) = call(socket(), Method::POST, "/api/v1/projects", Some("{")).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    let (status, body) = call(
        socket(),
        Method::POST,
        "/api/v1/projects",
        Some(r#"{"name":"x"}"#),
    )
    .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri("/api/v1/projects")
        .body(Body::from(open))
        .unwrap();
    let response = socket().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    registry.close().await.unwrap();
    daemon.shutdown().await;
}

#[tokio::test]
async fn orphan_sessions_end_at_the_start_time_of_the_injected_clock() {
    let host = Host::new();
    let alpha = TestProject::new().await;
    host.register("alpha", &alpha).await;
    for sql in [
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 1)",
        "INSERT INTO sessions (agent_id, pid, started_at) VALUES (1, 10, 100)",
        "INSERT INTO sessions (pid, started_at) VALUES (11, 101)",
        "INSERT INTO sessions (pid, started_at, ended_at) VALUES (12, 102, 103)",
    ] {
        alpha
            .store()
            .write(async |tx| {
                sqlx::query(sql).execute(tx.conn()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }
    let at = Timestamp::from_millis(1_800_000_000_123).unwrap();
    host.clock.set(at);
    let daemon = host.start().await;

    let open = count(
        alpha.store(),
        "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL",
    )
    .await;
    assert_eq!(open, 0);
    let ended_at = count(
        alpha.store(),
        "SELECT COUNT(*) FROM sessions WHERE ended_at = 1800000000123",
    )
    .await;
    assert_eq!(ended_at, 2);
    let rows = sqlx::query("SELECT type, actor, data, created_at FROM events ORDER BY seq")
        .fetch_all(alpha.store().read())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    for row in &rows {
        assert_eq!(row.get::<String, _>("type"), "session.ended");
        assert_eq!(row.get::<String, _>("actor"), "fold");
        assert_eq!(row.get::<i64, _>("created_at"), at.as_millis());
    }
    let agents: Vec<Value> = rows
        .iter()
        .map(|row| {
            serde_json::from_str::<Value>(&row.get::<String, _>("data")).unwrap()["agent"].clone()
        })
        .collect();
    assert_eq!(agents, [json!("frontend")]);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_corrupt_project_is_unavailable_while_another_project_works() {
    let host = Host::new();
    let good = TestProject::new().await;
    let bad = TestProject::new().await;
    let missing = TestProject::new().await;
    bad.store().close().await.unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let mut name = bad.paths().db().into_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(&name);
    }
    std::fs::write(bad.paths().db(), vec![0xAB_u8; 4096]).unwrap();
    std::fs::remove_file(missing.paths().db()).unwrap();
    host.register("good", &good).await;
    host.register("bad", &bad).await;
    host.register("missing", &missing).await;
    let daemon = host.start().await;
    let state = daemon.state().clone();

    for slug in ["bad", "missing"] {
        let (status, body) = call(
            router(state.clone()),
            Method::GET,
            &format!("/api/v1/projects/{slug}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{slug}");
        assert_eq!(code(&body), "project_unavailable");
    }
    let (status, _) = call(
        router(state.clone()),
        Method::GET,
        "/api/v1/projects/good",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call(router(state.clone()), Method::GET, "/api/v1/projects", None).await;
    let availability: Vec<(&str, bool)> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["slug"].as_str().unwrap(),
                p["available"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        availability,
        [("bad", false), ("good", true), ("missing", false)]
    );

    let (status, body) = call(
        socket_router(state),
        Method::POST,
        "/api/v1/projects",
        Some(r#"{"slug":"bad"}"#),
    )
    .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::SERVICE_UNAVAILABLE, "project_unavailable")
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn the_socket_tells_a_handshake_from_http_by_the_first_byte() {
    let host = Host::new();
    let daemon = host.start().await;

    let mut handshake = UnixStream::connect(daemon.socket_path()).await.unwrap();
    handshake.write_all(b"{\"hello\":1}\n").await.unwrap();
    let mut reply = String::new();
    handshake.read_to_string(&mut reply).await.unwrap();
    assert!(reply.ends_with('\n'));
    let line: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(line["ok"], json!(false));
    assert_eq!(code(&line), "invalid_handshake");

    let http = UnixStream::connect(daemon.socket_path()).await.unwrap();
    let (status, body) = raw_http(http, "GET", "/api/v1/projects", "").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"projects": []}));

    daemon.socket().on_handshake(Arc::new(|mut stream, _peer| {
        Box::pin(async move {
            let mut first = [0_u8; 1];
            // The handler sees the connection from its first byte.
            if stream.read_exact(&mut first).await.is_ok() {
                let _ = stream.write_all(&first).await;
            }
        })
    }));
    let mut handshake = UnixStream::connect(daemon.socket_path()).await.unwrap();
    handshake.write_all(b"{").await.unwrap();
    let mut echoed = [0_u8; 1];
    handshake.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"{");

    // A client that connects and leaves without a byte costs nothing.
    drop(UnixStream::connect(daemon.socket_path()).await.unwrap());
    let http = UnixStream::connect(daemon.socket_path()).await.unwrap();
    assert_eq!(raw_http(http, "GET", "/api/v1/projects", "").await.0, 200);
    daemon.shutdown().await;
}

#[tokio::test]
async fn the_tcp_listener_serves_the_api() {
    let host = Host::new();
    let daemon = host.start().await;
    let tcp = TcpStream::connect(daemon.http_addr()).await.unwrap();
    let (status, body) = raw_http(tcp, "GET", "/api/v1/projects", "").await;
    assert_eq!((status, body), (200, json!({"projects": []})));
    let tcp = TcpStream::connect(daemon.http_addr()).await.unwrap();
    let (status, body) = raw_http(tcp, "GET", "/api/elsewhere?token=secret", "").await;
    assert_eq!((status, code(&body)), (404, "not_found"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn shutdown_ends_an_open_event_stream_before_the_drain_timeout() {
    let host = Host::new();
    let alpha = TestProject::new().await;
    host.register("alpha", &alpha).await;
    let daemon = host.start().await;
    let mut tcp = TcpStream::connect(daemon.http_addr()).await.unwrap();
    tcp.write_all(
        b"GET /api/v1/projects/alpha/events HTTP/1.1\r\nhost: 127.0.0.1\r\naccept: text/event-stream\r\n\r\n",
    )
    .await
    .unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(WAIT, async {
        let mut chunk = [0_u8; 4096];
        while !String::from_utf8_lossy(&received).contains(r#""kind":"snapshot""#) {
            let read = tcp.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "the stream ended before its snapshot");
            received.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .expect("no snapshot arrived in time");

    // Shorter than the drain timeout, so the open stream must end because the daemon closed it.
    tokio::time::timeout(Duration::from_secs(4), daemon.shutdown())
        .await
        .expect("shutdown waited for the open event stream");
    tokio::time::timeout(WAIT, tcp.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&received);
    assert!(
        text.ends_with("0\r\n\r\n"),
        "the stream ended with its last chunk: {text:?}"
    );
}

#[tokio::test]
async fn shutdown_ends_sessions_checkpoints_and_removes_the_socket() {
    let host = Host::new();
    let alpha = TestProject::new().await;
    host.register("alpha", &alpha).await;
    let daemon = host.start().await;
    let socket = daemon.socket_path().to_path_buf();
    assert!(socket.exists());
    alpha
        .store()
        .write(async |tx| {
            sqlx::query("INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 1)")
                .execute(tx.conn())
                .await?;
            sqlx::query("INSERT INTO sessions (agent_id, pid, started_at) VALUES (1, 5, 50)")
                .execute(tx.conn())
                .await?;
            sqlx::query("INSERT INTO sessions (pid, started_at) VALUES (6, 51)")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let later = Timestamp::from_millis(1_900_000_000_000).unwrap();
    host.clock.set(later);

    daemon.shutdown().await;

    assert!(!socket.exists());
    let ended = eventually(WAIT, async || {
        let ended = count(
            alpha.store(),
            "SELECT COUNT(*) FROM sessions WHERE ended_at = 1900000000000",
        )
        .await;
        if ended == 2 { Ok(ended) } else { Err(ended) }
    })
    .await;
    assert_eq!(ended, 2);
    assert_eq!(
        count(
            alpha.store(),
            "SELECT COUNT(*) FROM events WHERE type = 'session.ended'"
        )
        .await,
        1
    );
    // The daemon released the lock, so a new one starts.
    let again = host.start().await;
    again.shutdown().await;
}

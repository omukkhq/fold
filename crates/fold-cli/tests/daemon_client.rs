mod common;

use std::path::Path;
use std::sync::Arc;

use fold_cli::daemon_client::{
    ClientConfig, ClientError, DaemonClient, HUMAN_HEADER, connect, human_from,
};
use fold_core::ManualClock;
use fold_daemon::{Daemon, DaemonConfig};
use hyper::Method;
use serde_json::json;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

use common::{DaemonGuard, Host, adapter, daemons, list_agents};

#[cfg(test)]
fn config(socket: &Path, runtime_dir: &Path, fold_exe: &str) -> ClientConfig {
    ClientConfig {
        socket: socket.to_path_buf(),
        runtime_dir: Some(runtime_dir.to_path_buf()),
        fold_exe: Some(fold_exe.into()),
    }
}

/// Answers one request with `response` and returns the request as it arrived.
#[cfg(test)]
async fn serve_once(listener: &UnixListener, response: &'static str) -> String {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        request.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&request);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map_or(0, |value| value.parse::<usize>().unwrap());
            if body.len() >= length {
                break;
            }
        }
        assert!(read > 0, "the client closed before its request ended");
    }
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    String::from_utf8(request).unwrap()
}

#[tokio::test]
async fn a_request_carries_the_human_from_fold_human_or_the_user_name() {
    let dir = TempDir::new().unwrap();
    let socket = dir.path().join("fake.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let config = config(&socket, dir.path(), "/bin/false");
    const OK: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}";

    let lead = human_from(Some("lead".into()), Some("ana".into()), None).unwrap();
    let client = DaemonClient::new(config.clone(), Some(lead));
    let (response, request) = tokio::join!(
        client.request(Method::GET, "/api/v1/projects", None),
        serve_once(&listener, OK)
    );
    let response = response.unwrap();
    assert_eq!((response.status, response.body), (200, json!({"ok": true})));
    assert!(
        request.starts_with("GET /api/v1/projects HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        request.contains(&format!("{HUMAN_HEADER}: lead\r\n")),
        "{request}"
    );

    let user = human_from(None, Some("ana".into()), Some("other".into())).unwrap();
    let client = DaemonClient::new(config.clone(), Some(user));
    let body = json!({"slug": "app"});
    const EMPTY: &str = "HTTP/1.1 204 No Content\r\n\r\n";
    let (response, request) = tokio::join!(
        client.request(Method::POST, "/api/v1/projects", Some(&body)),
        serve_once(&listener, EMPTY)
    );
    assert_eq!(response.unwrap().body, serde_json::Value::Null);
    assert!(
        request.contains(&format!("{HUMAN_HEADER}: ana\r\n")),
        "{request}"
    );
    assert!(
        request.contains("content-type: application/json\r\n"),
        "{request}"
    );
    assert!(request.ends_with("{\"slug\":\"app\"}"), "{request}");

    const TEXT: &str = "HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello";
    let (response, _) = tokio::join!(
        client.request(Method::GET, "/", None),
        serve_once(&listener, TEXT)
    );
    assert!(matches!(response, Err(ClientError::Body)));
}

#[tokio::test]
async fn a_daemon_that_stops_while_it_starts_is_reported() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("none.sock");
    let error = connect(&config(&missing, &dir.path().join("run"), "/bin/true"))
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::StartFailed), "{error}");
    assert!(dir.path().join("run").join("start.lock").exists());

    // A file that no daemon listens on counts as absent too.
    let stale = dir.path().join("stale.sock");
    std::fs::write(&stale, "").unwrap();
    let error = connect(&config(&stale, dir.path(), "/bin/true"))
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::StartFailed), "{error}");

    let error = connect(&config(&missing, dir.path(), "/nonexistent/fold"))
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::Io { .. }), "{error}");
    assert!(error.to_string().starts_with("start the daemon: "));
}

#[tokio::test]
async fn without_a_runtime_directory_a_client_reaches_a_running_daemon_but_starts_none() {
    let dir = TempDir::new().unwrap();
    let socket = dir.path().join("live.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let no_runtime = |socket: &Path| ClientConfig {
        runtime_dir: None,
        ..config(socket, dir.path(), "/bin/true")
    };
    assert!(connect(&no_runtime(&socket)).await.is_ok());

    let error = connect(&no_runtime(&dir.path().join("none.sock")))
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::RuntimeDirMissing), "{error}");
    assert!(!dir.path().join("start.lock").exists());
}

#[tokio::test]
async fn the_start_lock_directory_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let runtime = dir.path().join("run");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let missing = dir.path().join("none.sock");
    let error = connect(&config(&missing, &runtime, "/bin/true"))
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::StartFailed), "{error}");
    let mode = std::fs::metadata(&runtime).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    let lock = std::fs::metadata(runtime.join("start.lock")).unwrap();
    assert_eq!(lock.permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn the_socket_flag_reaches_a_daemon_on_a_non_default_path() {
    let host = Host::new().await;
    let socket = host.dir.path().join("elsewhere").join("custom.sock");
    let daemon = Daemon::start(
        DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: socket.clone(),
            data_dir: host.data_home().join("fold"),
            runtime_dir: host.dir.path().join("elsewhere"),
        },
        Arc::new(ManualClock::new()),
    )
    .await
    .unwrap();

    let client = DaemonClient::new(
        config(&socket, &host.runtime_base(), "/bin/false"),
        Some("ana".into()),
    );
    let response = client
        .request(Method::GET, "/api/v1/projects", None)
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body["projects"][0]["slug"], "app");

    let socket_flag = socket.to_string_lossy().into_owned();
    let (mcp, mut child) = adapter(host.fold(&["mcp", "--socket", &socket_flag], &host.root)).await;
    assert_eq!(list_agents(&mcp).await, json!({"agents": []}));
    mcp.cancel().await.unwrap();
    assert!(child.wait().await.unwrap().success());
    let started = daemons(&host.default_socket());
    assert!(
        started.is_empty(),
        "the adapter started a daemon at the default path"
    );
    DaemonGuard(started).stop().await;
    daemon.shutdown().await;
}

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use fold_core::{HumanName, ManualClock};
use fold_daemon::{Daemon, DaemonConfig};
use fold_store::Store;
use fold_store::projects;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;

/// HOME and the XDG directories in one temporary directory, with no daemon and no project.
#[cfg(test)]
struct Host(TempDir);

#[cfg(test)]
impl Host {
    fn new() -> Self {
        Self(TempDir::new().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn data_dir(&self) -> PathBuf {
        self.path().join("data").join("fold")
    }

    async fn fold(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_fold"))
            .args(args)
            .current_dir(self.path())
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("XDG_DATA_HOME", self.path().join("data"))
            .env("XDG_RUNTIME_DIR", self.path().join("run"))
            .env_remove("FOLD_HUMAN")
            .output()
            .await
            .unwrap()
    }
}

#[cfg(test)]
async fn status_of(addr: SocketAddr, path: &str, cookie: Option<&str>) -> u16 {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let cookie = cookie.map_or_else(String::new, |c| format!("cookie: {c}\r\n"));
    let request =
        format!("GET {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n{cookie}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

/// Posts `body` to the session route and returns the status and the `fold_session` pair, if the answer sets one.
#[cfg(test)]
async fn sign_in(addr: SocketAddr, token: &str) -> (u16, Option<String>) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = format!("{{\"token\":\"{token}\"}}");
    let request = format!(
        "POST /api/v1/session HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8(response).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let pair = text.lines().find_map(|line| {
        line.split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .and_then(|(_, value)| value.trim().split(';').next())
            .map(str::to_owned)
    });
    (status, pair)
}

#[tokio::test]
async fn a_token_created_without_a_daemon_signs_in_to_a_daemon_started_afterwards() {
    let host = Host::new();
    let output = host
        .fold(&["token", "create", "ana", "--label", "laptop"])
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let token = stdout.trim_end();
    assert_eq!(stdout, format!("{token}\n"));
    assert_eq!(token.len(), 43);
    assert!(
        !host
            .path()
            .join("run")
            .join("fold")
            .join("fold.sock")
            .exists()
    );

    let registry_db = host.data_dir().join("registry.db");
    let registry = Store::open_registry(&registry_db).await.unwrap();
    let human = projects::find_human(registry.read(), &HumanName::parse("ana").unwrap())
        .await
        .unwrap();
    assert!(human.is_some());
    registry.close().await.unwrap();

    let daemon = Daemon::start(
        DaemonConfig {
            listen: "0.0.0.0:0".parse().unwrap(),
            socket: host.path().join("run").join("fold").join("fold.sock"),
            data_dir: host.data_dir(),
            runtime_dir: host.path().join("run").join("fold"),
            web_dir: None,
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        },
        Arc::new(ManualClock::new()),
    )
    .await
    .unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], daemon.http_addr().port()));
    assert_eq!(status_of(addr, "/api/v1/projects", None).await, 401);
    let (status, cookie) = sign_in(addr, token).await;
    assert_eq!(status, 204);
    let cookie = cookie.unwrap();
    assert!(!cookie.contains(token));
    assert_eq!(
        status_of(addr, "/api/v1/projects", Some(&cookie)).await,
        200
    );

    let output = host.fold(&["token", "list"]).await;
    assert!(output.status.success());
    let listed = String::from_utf8(output.stdout).unwrap();
    assert!(
        listed.starts_with("wt-1 ana created ") && listed.ends_with(" (laptop)\n"),
        "{listed}"
    );
    assert!(!listed.contains("last used never"), "{listed}");
    assert!(!listed.contains(token));

    let output = host.fold(&["token", "revoke", "wt-1"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "revoked wt-1\n");
    assert_eq!(
        status_of(addr, "/api/v1/projects", Some(&cookie)).await,
        401
    );
    assert_eq!(sign_in(addr, token).await, (401, None));
    daemon.shutdown().await;

    let output = host.fold(&["token", "list"]).await;
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "no web tokens\n");
}

#[tokio::test]
async fn revoking_an_unknown_or_malformed_id_is_refused_with_its_code() {
    let host = Host::new();
    for (id, code) in [("wt-7", "token_not_found"), ("7", "invalid_id")] {
        let output = host.fold(&["token", "revoke", id]).await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(code), "{stderr}");
    }
    let output = host.fold(&["token", "list"]).await;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "no web tokens\n");
}

#[tokio::test]
async fn a_malformed_human_name_is_refused_with_its_code() {
    let host = Host::new();
    let output = host.fold(&["token", "create", "Not A Name"]).await;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid_name"));
}

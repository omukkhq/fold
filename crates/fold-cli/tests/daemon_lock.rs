#![cfg(test)]

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use fold_cli::daemon_client::{ClientConfig, DaemonClient};
use fold_store::testing::eventually;
use hyper::Method;
use tokio::process::Child;

use crate::common::{Host, daemons};

const WAIT: Duration = Duration::from_secs(20);

/// `fld serve` on the private socket `socket`, once it accepts connections.
async fn serve(host: &Host, socket: &Path) -> Child {
    let mut command = host.fold(&["serve", "--socket", socket.to_str().unwrap()], &host.root);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().unwrap();
    eventually(WAIT, async || {
        tokio::net::UnixStream::connect(socket)
            .await
            .map(drop)
            .map_err(|error| error.kind())
    })
    .await;
    child
}

async fn kill(mut daemon: Child) {
    daemon.start_kill().unwrap();
    daemon.wait().await.unwrap();
}

/// The OS lock on `daemon.lock` decides: a second `fld serve` on the same host is refused with `daemon_already_running` while the first runs, even on another socket. SIGKILL releases the lock and leaves the socket file behind, and the next `fld serve` replaces that file and serves.
#[tokio::test]
async fn a_second_daemon_is_refused_until_the_first_is_killed() {
    let host = Host::new().await;
    let socket = host.dir.path().join("first.sock");
    let other = host.dir.path().join("second.sock");
    let first = serve(&host, &socket).await;

    let second = tokio::time::timeout(
        WAIT,
        host.fold(&["serve", "--socket", other.to_str().unwrap()], &host.root)
            .output(),
    )
    .await
    .expect("the second daemon exits within the wait")
    .unwrap();
    assert!(!second.status.success(), "{second:?}");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("daemon_already_running"), "{stderr}");
    assert!(!other.exists(), "the refused daemon bound no socket");

    kill(first).await;
    assert!(socket.exists(), "a killed daemon leaves its socket file");
    let again = serve(&host, &socket).await;
    let answer = DaemonClient::new(
        ClientConfig {
            socket: socket.clone(),
            runtime_dir: None,
            fold_exe: None,
        },
        Some("tanim".to_owned()),
    )
    .request(Method::GET, "/api/v1/projects", None)
    .await
    .unwrap();
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.body["projects"][0]["slug"], "app");

    kill(again).await;
    assert!(
        daemons(&socket).is_empty() && daemons(&other).is_empty(),
        "a fld serve outlived the test"
    );
}

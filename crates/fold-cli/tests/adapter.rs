use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::common::{DaemonGuard, Host, adapter, daemons, list_agents};

#[tokio::test]
async fn three_adapters_started_together_leave_one_daemon_and_read_the_same_state() {
    let host = Host::new().await;
    assert!(!host.default_socket().exists());
    let workspace = host.root.join("ws").join("t-1");
    std::fs::create_dir_all(&workspace).unwrap();

    let ((first, mut first_child), (second, mut second_child), (third, mut third_child)) = tokio::join!(
        adapter(host.fold(&["mcp"], &host.root)),
        adapter(host.fold(&["mcp"], &host.root)),
        adapter(host.fold(&["mcp"], &workspace)),
    );
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert_eq!(guard.0.len(), 1, "one daemon serves every adapter");

    let empty = json!({"agents": []});
    assert_eq!(list_agents(&first).await, empty);
    assert_eq!(list_agents(&second).await, empty);
    assert_eq!(list_agents(&third).await, empty);

    for (client, child) in [
        (first, &mut first_child),
        (second, &mut second_child),
        (third, &mut third_child),
    ] {
        client.cancel().await.unwrap();
        assert!(
            child.wait().await.unwrap().success(),
            "closing stdin ends the adapter"
        );
    }
    assert_eq!(
        daemons(&host.default_socket()),
        guard.0,
        "the daemon outlives its adapters"
    );
    guard.stop().await;
}

#[tokio::test]
async fn an_adapter_connects_to_the_socket_in_fold_socket() {
    let host = Host::new().await;
    let socket = host.dir.path().join("by-hand.sock");
    let mut command = host.fold(&["mcp"], &host.root);
    command.env("FOLD_SOCKET", &socket);
    let (client, mut child) = adapter(command).await;
    let guard = DaemonGuard(daemons(&socket));
    assert_eq!(
        guard.0.len(),
        1,
        "the adapter started the daemon of FOLD_SOCKET"
    );
    assert!(daemons(&host.default_socket()).is_empty());
    client.cancel().await.unwrap();
    assert!(child.wait().await.unwrap().success());
    guard.stop().await;
}

#[tokio::test]
async fn an_adapter_outside_every_project_serves_no_tools_quietly_and_starts_no_daemon() {
    let host = Host::new().await;
    let outside = host.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let mut command = host.fold(&["mcp"], &outside);
    command.env("FOLD_AGENT", "frontend");
    let (client, mut child) = adapter(command).await;
    let info = client.peer_info().unwrap();
    assert!(info.capabilities.tools.is_none(), "no tools are offered");
    assert_eq!(
        info.instructions.as_deref(),
        Some(fold_daemon::mcp::OUTSIDE_INSTRUCTIONS)
    );
    assert!(daemons(&host.default_socket()).is_empty());
    client.cancel().await.unwrap();
    assert!(child.wait().await.unwrap().success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    assert_eq!(stderr, "", "the user's other sessions show no error");
}

#[tokio::test]
async fn an_adapter_whose_daemon_stops_writes_one_line_and_exits_1() {
    let host = Host::new().await;
    let (client, mut child) = adapter(host.fold(&["mcp"], &host.root)).await;
    assert_eq!(list_agents(&client).await, json!({"agents": []}));
    DaemonGuard(daemons(&host.default_socket())).stop().await;
    let status = child.wait().await.unwrap();
    assert_eq!(status.code(), Some(1));
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    assert_eq!(
        stderr,
        "fld mcp: the daemon closed the session; start a new session to reconnect\n"
    );
    drop(client);
}

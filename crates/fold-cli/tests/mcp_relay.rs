use std::sync::Arc;
use std::time::Duration;

use fold_cli::daemon_client::{self, ClientConfig, HelloLine};
use fold_cli::mcp_relay::{ReconnectPolicy, RelayError, relay};
use fold_core::{FoldError, ManualClock, codes};
use fold_daemon::mcp::handshake::{Answer, Hello, HelloOptions, PROTOCOL_VERSION, parse_line};
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

use crate::common::{Client, Host};

const WAIT: Duration = Duration::from_secs(20);

#[cfg(test)]
async fn start_daemon(host: &Host) -> RunningDaemon {
    Daemon::start(
        DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: host.default_socket(),
            data_dir: host.data_home().join("fold"),
            runtime_dir: host.runtime_base().join("fold"),
            web_dir: None,
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        },
        Arc::new(ManualClock::new()),
    )
    .await
    .unwrap()
}

fn hello(host: &Host) -> HelloLine {
    Hello {
        fold: PROTOCOL_VERSION,
        cwd: host.root.clone(),
        pid: std::process::id(),
        agent: Some("frontend".to_owned()),
        tags: Vec::new(),
        inbox: None,
    }
    .into()
}

#[cfg(test)]
/// A relay between an rmcp client and the daemon of `host`, as `fld mcp` runs it over stdio.
async fn connected(
    host: &Host,
    policy: ReconnectPolicy,
) -> (Client, JoinHandle<Result<(), RelayError>>) {
    let config = ClientConfig::running(host.default_socket());
    let hello = hello(host);
    let stream = daemon_client::connect(&config).await.unwrap();
    let connection = daemon_client::handshake(stream, &hello).await.unwrap();
    let (client_side, relay_side) = tokio::io::duplex(64 * 1024);
    let (stdin, stdout) = tokio::io::split(relay_side);
    let relayed =
        tokio::spawn(
            async move { relay(stdin, stdout, connection, &config, &hello, &policy).await },
        );
    let client = ().serve(client_side).await.unwrap();
    (client, relayed)
}

fn short_policy(give_up_after: Duration) -> ReconnectPolicy {
    ReconnectPolicy {
        first_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
        give_up_after,
    }
}

#[cfg(test)]
async fn call(client: &Client, tool: &'static str) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new(tool))
        .await
        .unwrap()
}

#[cfg(test)]
fn error_body(result: &CallToolResult) -> Option<Value> {
    if result.is_error != Some(true) {
        return None;
    }
    let text = &result.content[0].as_text().unwrap().text;
    let body: Value = serde_json::from_str(text).unwrap();
    Some(body["error"].clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_serves_the_next_call_as_the_same_agent() {
    let host = Host::new().await;
    let first = start_daemon(&host).await;
    let (client, relayed) = connected(&host, short_policy(WAIT)).await;
    assert_eq!(error_body(&call(&client, "get_inbox").await), None);

    first.shutdown().await;
    let gap = error_body(&call(&client, "get_inbox").await).expect("the call in the gap fails");
    assert_eq!(gap["code"], "daemon_restarting");
    assert!(
        gap["hint"]
            .as_str()
            .unwrap()
            .starts_with("call the tool again in a few seconds"),
        "{gap}"
    );
    assert!(!relayed.is_finished(), "the stdio side stays open");

    let second = start_daemon(&host).await;
    // get_inbox refuses with agent_not_registered unless the new session is bound to frontend.
    fold_store::testing::eventually(WAIT, async || {
        match error_body(&call(&client, "get_inbox").await) {
            None => Ok(()),
            Some(error) => Err(error),
        }
    })
    .await;
    let result = call(&client, "list_agents").await;
    let agents: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(agents["agents"][0]["name"], "frontend", "{agents}");

    client.cancel().await.unwrap();
    let ended = tokio::time::timeout(WAIT, relayed).await.unwrap().unwrap();
    assert!(ended.is_ok(), "closing stdin ends the relay cleanly");
    second.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_relay_gives_up_with_the_closed_session_message() {
    let host = Host::new().await;
    let daemon = start_daemon(&host).await;
    let (client, relayed) = connected(&host, short_policy(Duration::from_millis(200))).await;
    assert_eq!(error_body(&call(&client, "list_agents").await), None);
    daemon.shutdown().await;
    let ended = tokio::time::timeout(WAIT, relayed).await.unwrap().unwrap();
    let error = ended.expect_err("no daemon came back");
    assert!(matches!(error, RelayError::DaemonClosed));
    assert_eq!(
        error.to_string(),
        "the daemon closed the session; start a new session to reconnect"
    );
    drop(client);
}

#[cfg(test)]
/// Accepts the next connection of a stand-in daemon, reads its handshake line and answers it.
async fn handshake_line(listener: &UnixListener, answer: &Answer) -> (Vec<u8>, UnixStream) {
    let (stream, _) = tokio::time::timeout(WAIT, listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).await.unwrap();
    let mut stream = reader.into_inner();
    stream.write_all(&answer.line()).await.unwrap();
    (line, stream)
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_reopen_marks_its_handshake_and_a_refused_replay_stops_the_relay() {
    let dir = tempfile::TempDir::new().unwrap();
    let socket = dir.path().join("fold.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let config = ClientConfig::running(socket);
    let hello = HelloLine {
        hello: Hello {
            fold: PROTOCOL_VERSION,
            cwd: dir.path().to_path_buf(),
            pid: std::process::id(),
            agent: Some("main".to_owned()),
            tags: vec!["main".to_owned()],
            inbox: None,
        },
        options: HelloOptions {
            keep_others: true,
            reconnect: false,
        },
    };

    let accepted = Answer::accepted("app");
    let stream = daemon_client::connect(&config).await.unwrap();
    let (connection, (first, first_stream)) = tokio::join!(
        daemon_client::handshake(stream, &hello),
        handshake_line(&listener, &accepted),
    );
    let connection = connection.unwrap();
    let (first_hello, first_options) = parse_line(&first).unwrap();
    assert_eq!(first_hello, hello.hello);
    assert!(!first_options.reconnect, "the first connect has no mark");
    assert!(!String::from_utf8_lossy(&first).contains("reconnect"));

    let (client_side, relay_side) = tokio::io::duplex(64 * 1024);
    let (stdin, stdout) = tokio::io::split(relay_side);
    let policy = short_policy(WAIT);
    let relay_hello = hello.clone();
    let relayed = tokio::spawn(async move {
        relay(stdin, stdout, connection, &config, &relay_hello, &policy).await
    });
    drop(first_stream);

    let taken_over = FoldError::new(
        codes::MAIN_SESSION_TAKEN_OVER,
        "a newer session took this one over",
    );
    let (replayed, _replayed_stream) =
        handshake_line(&listener, &Answer::refused(&taken_over)).await;
    let (replayed_hello, replayed_options) = parse_line(&replayed).unwrap();
    assert_eq!(replayed_hello, hello.hello);
    assert_eq!(
        replayed_options,
        HelloOptions {
            keep_others: true,
            reconnect: true,
        },
        "the reopen after a closed stream has the mark and keeps keep_others"
    );

    let ended = tokio::time::timeout(WAIT, relayed).await.unwrap().unwrap();
    let error = ended.expect_err("the refused replay stops the relay");
    assert!(
        matches!(
            &error,
            RelayError::Refused(daemon_client::HandshakeError::Refused { code, .. })
                if code == "main_session_taken_over"
        ),
        "{error:?}"
    );
    drop(client_side);
}

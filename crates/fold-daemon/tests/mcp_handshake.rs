use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fold_core::{Clock, ManualClock};
use fold_daemon::daemon::AppState;
use fold_daemon::mcp::handshake::{self, Answer, MAX_LINE, PROTOCOL_VERSION, parse_hello};
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::{TestProject, eventually};
use fold_store::{Store, StoreError};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use sqlx::Row;
use tempfile::TempDir;
use tokio::io::{
    AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
};

#[cfg(test)]
const WAIT: Duration = Duration::from_secs(10);

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

#[cfg(test)]
type Rest = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);

/// A started daemon with one registered project, `app`.
#[cfg(test)]
struct Host {
    _dir: TempDir,
    _project: TestProject,
    root: PathBuf,
    daemon: RunningDaemon,
}

#[cfg(test)]
impl Host {
    async fn start() -> Self {
        let dir = TempDir::new().unwrap();
        let config = DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: dir.path().join("run").join("fold.sock"),
            data_dir: dir.path().join("data"),
            runtime_dir: dir.path().join("run"),
            web_dir: None,
        };
        let clock = ManualClock::new();
        let project = TestProject::new().await;
        let root = project.paths().root().canonicalize().unwrap();
        let registry = Store::open_registry(&config.registry_path()).await.unwrap();
        let row = ProjectRow {
            slug: "app".to_owned(),
            name: "App".to_owned(),
            root: root.to_string_lossy().into_owned(),
        };
        registry
            .write(async |tx| {
                projects::insert_project(tx, &row, clock.now()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        registry.close().await.unwrap();
        let daemon = Daemon::start(config, Arc::new(clock)).await.unwrap();
        Self {
            _dir: dir,
            _project: project,
            root,
            daemon,
        }
    }

    fn state(&self) -> AppState {
        self.daemon.state().clone()
    }

    fn hello(&self, cwd: &Path, agent: Option<&str>) -> Value {
        json!({"fold": PROTOCOL_VERSION, "cwd": cwd, "pid": 4242, "agent": agent})
    }

    async fn count(&self, sql: &'static str) -> i64 {
        let handle = match self.state().projects.slot("app") {
            Some(fold_daemon::projects::Slot::Open(handle)) => handle,
            other => panic!("project app is not open: {other:?}"),
        };
        sqlx::query(sql)
            .fetch_one(handle.store.read())
            .await
            .unwrap()
            .get(0)
    }
}

/// Sends `bytes` as the first thing on a fresh in-memory connection and reads the answer line.
#[cfg(test)]
async fn handshake_raw(state: AppState, bytes: &[u8]) -> (Answer, Rest) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(handshake::accept(state, server));
    let (read, mut write) = tokio::io::split(client);
    write.write_all(bytes).await.unwrap();
    let mut read = BufReader::new(read);
    let mut line = String::new();
    read.read_line(&mut line).await.unwrap();
    assert!(line.ends_with('\n'), "the answer is one line: {line:?}");
    (serde_json::from_str(&line).unwrap(), (read, write))
}

#[cfg(test)]
async fn handshake(state: AppState, hello: &Value) -> (Answer, Rest) {
    handshake_raw(state, format!("{hello}\n").as_bytes()).await
}

#[cfg(test)]
async fn refused(state: AppState, bytes: &[u8]) -> (String, Option<String>) {
    let (answer, (mut read, _write)) = handshake_raw(state, bytes).await;
    assert!(!answer.ok);
    assert_eq!(answer.project, None);
    let mut after = Vec::new();
    read.read_to_end(&mut after).await.unwrap();
    assert!(
        after.is_empty(),
        "a refused connection closes after the answer"
    );
    let error = answer.error.unwrap();
    (error.code, error.hint)
}

#[cfg(test)]
async fn mcp(rest: Rest) -> Client {
    ().serve(rest).await.unwrap()
}

#[cfg(test)]
async fn list_agents(client: &Client) -> Value {
    let result = client
        .call_tool(CallToolRequestParams::new("list_agents"))
        .await
        .unwrap();
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn an_rmcp_client_completes_the_handshake_and_the_session_ends_with_the_stream() {
    let host = Host::start().await;
    let (answer, rest) = handshake(host.state(), &host.hello(&host.root, None)).await;
    assert_eq!(answer, Answer::accepted("app"));
    let client = mcp(rest).await;
    assert_eq!(list_agents(&client).await, json!({"agents": []}));
    assert_eq!(
        host.count("SELECT count(*) FROM sessions WHERE ended_at IS NULL AND pid = 4242")
            .await,
        1
    );

    client.cancel().await.unwrap();
    eventually(WAIT, async || {
        match host
            .count("SELECT count(*) FROM sessions WHERE ended_at IS NULL")
            .await
        {
            0 => Ok(()),
            open => Err(open),
        }
    })
    .await;
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn bytes_sent_right_after_the_line_reach_the_mcp_server() {
    let host = Host::start().await;
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"},
        },
    });
    let bytes = format!("{}\n{initialize}\n", host.hello(&host.root, None));
    let (answer, (mut read, _write)) = handshake_raw(host.state(), bytes.as_bytes()).await;
    assert!(answer.ok);
    let mut reply = String::new();
    read.read_line(&mut reply).await.unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["id"], 1);
    assert!(reply["result"]["instructions"].is_string(), "{reply}");
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_client_that_does_not_initialise_first_loses_its_session() {
    let host = Host::start().await;
    let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let bytes = format!("{}\n{notification}\n", host.hello(&host.root, None));
    let (answer, (mut read, _write)) = handshake_raw(host.state(), bytes.as_bytes()).await;
    assert!(answer.ok);
    let mut after = Vec::new();
    read.read_to_end(&mut after).await.unwrap();
    assert!(after.is_empty(), "the daemon closes the stream");
    eventually(WAIT, async || {
        match host
            .count("SELECT count(*) FROM sessions WHERE ended_at IS NULL")
            .await
        {
            0 => Ok(()),
            open => Err(open),
        }
    })
    .await;
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_cwd_inside_a_task_workspace_resolves_its_project() {
    let host = Host::start().await;
    let cwd = host.root.join("ws").join("t-1").join("web");
    let (answer, rest) = handshake(host.state(), &host.hello(&cwd, None)).await;
    assert_eq!(answer.project.as_deref(), Some("app"));
    assert_eq!(list_agents(&mcp(rest).await).await, json!({"agents": []}));
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_cwd_outside_every_project_gets_project_not_found() {
    let host = Host::start().await;
    let mut sibling = host.root.clone().into_os_string();
    sibling.push("-2");
    for cwd in [PathBuf::from("/"), PathBuf::from(sibling)] {
        let (code, hint) = refused(
            host.state(),
            format!("{}\n", host.hello(&cwd, None)).as_bytes(),
        )
        .await;
        assert_eq!(code, "project_not_found");
        assert_eq!(hint.as_deref(), Some("run fold init in the project root"));
    }
    assert_eq!(host.count("SELECT count(*) FROM sessions").await, 0);
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn another_protocol_version_is_unsupported() {
    let host = Host::start().await;
    for version in [0, 2, 99] {
        let hello = json!({"fold": version, "cwd": host.root, "pid": 1, "agent": null});
        let (code, hint) = refused(host.state(), format!("{hello}\n").as_bytes()).await;
        assert_eq!(code, "protocol_version_unsupported");
        assert!(hint.is_some());
    }
    let only_version = json!({"fold": 2});
    let (code, _) = refused(host.state(), format!("{only_version}\n").as_bytes()).await;
    assert_eq!(code, "protocol_version_unsupported");
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_line_longer_than_4_kib_is_refused_and_a_line_of_4_kib_is_accepted() {
    let host = Host::start().await;
    let hello = host.hello(&host.root, None).to_string();
    // The padding goes into a field the daemon ignores, so only the length decides.
    let padded = |total: usize| {
        let without = format!("{},\"pad\":\"\"}}\n", &hello[..hello.len() - 1]);
        let pad = "x".repeat(total - without.len());
        format!("{},\"pad\":\"{pad}\"}}\n", &hello[..hello.len() - 1])
    };
    let exact = padded(MAX_LINE);
    assert_eq!(exact.len(), MAX_LINE);
    let (answer, _rest) = handshake_raw(host.state(), exact.as_bytes()).await;
    assert!(answer.ok, "{answer:?}");

    let over = padded(MAX_LINE + 1);
    let (code, _) = refused(host.state(), over.as_bytes()).await;
    assert_eq!(code, "invalid_handshake");
    let endless = format!("{{\"fold\":1,\"cwd\":\"{}\"", "y".repeat(3 * MAX_LINE));
    let (code, _) = refused(host.state(), endless.as_bytes()).await;
    assert_eq!(code, "invalid_handshake");
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn malformed_lines_are_invalid_handshakes() {
    let host = Host::start().await;
    let cases = [
        "{not json}\n".to_owned(),
        "{\"fold\":\"1\"}\n".to_owned(),
        "{\"fold\":1}\n".to_owned(),
        json!({"fold": 1, "cwd": "relative/dir", "pid": 1, "agent": null}).to_string() + "\n",
        json!({"fold": 1, "cwd": format!("{}/../..", host.root.display()), "pid": 1, "agent": null})
            .to_string() + "\n",
        json!({"fold": 1, "cwd": host.root, "pid": -1, "agent": null}).to_string() + "\n",
    ];
    for case in &cases {
        let (code, _) = refused(host.state(), case.as_bytes()).await;
        assert_eq!(code, "invalid_handshake", "{case}");
    }
    let (code, _) = refused(
        host.state(),
        format!("{}\n", host.hello(&host.root, Some("Not A Name"))).as_bytes(),
    )
    .await;
    assert_eq!(code, "invalid_name");
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn fold_agent_binds_the_agent_and_a_second_connection_reuses_it() {
    let host = Host::start().await;
    let (first, rest) = handshake(host.state(), &host.hello(&host.root, Some("frontend"))).await;
    assert!(first.ok);
    let client = mcp(rest).await;
    let (second, _rest) = handshake(host.state(), &host.hello(&host.root, Some("frontend"))).await;
    assert!(second.ok);
    assert_eq!(host.count("SELECT count(*) FROM agents").await, 1);
    assert_eq!(
        host.count(
            "SELECT count(*) FROM sessions s JOIN agents a ON a.id = s.agent_id WHERE a.name = 'frontend'"
        )
        .await,
        2
    );
    assert_eq!(
        list_agents(&client).await,
        json!({"agents": [{"agent": "frontend", "tags": [], "workers": ["frontend"]}]})
    );

    let (unset, _rest) = handshake(host.state(), &host.hello(&host.root, Some(""))).await;
    assert!(unset.ok);
    assert_eq!(
        host.count("SELECT count(*) FROM sessions WHERE agent_id IS NULL")
            .await,
        1
    );
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_silent_client_is_refused_after_five_seconds() {
    let host = Host::start().await;
    let (client, server) = tokio::io::duplex(1024);
    tokio::time::pause();
    let accept = tokio::spawn(handshake::accept(host.state(), server));
    let (read, _write) = tokio::io::split(client);
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await.unwrap();
    tokio::time::resume();
    let answer: Answer = serde_json::from_str(&line).unwrap();
    assert_eq!(answer.error.unwrap().code, "invalid_handshake");
    accept.await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn the_daemon_socket_serves_the_handshake() {
    let host = Host::start().await;
    let stream = tokio::net::UnixStream::connect(host.daemon.socket_path())
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    let hello = host.hello(&host.root, Some("review"));
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let mut read = BufReader::new(read);
    let mut line = String::new();
    read.read_line(&mut line).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Answer>(&line).unwrap(),
        Answer::accepted("app")
    );
    let client: Client = ().serve((read, write)).await.unwrap();
    assert_eq!(
        list_agents(&client).await,
        json!({"agents": [{"agent": "review", "tags": [], "workers": ["review"]}]})
    );
    drop(client);
    host.daemon.shutdown().await;
}

#[test]
fn parse_hello_treats_an_empty_agent_as_unset() {
    let hello = parse_hello(br#"{"fold":1,"cwd":"/src/app","pid":3,"agent":""}"#).unwrap();
    assert_eq!(hello.agent, None);
    let hello = parse_hello(br#"{"fold":1,"cwd":"/src/app","pid":3,"agent":"web"}"#).unwrap();
    assert_eq!(hello.agent.as_deref(), Some("web"));
    assert_eq!(hello.pid, 3);
}

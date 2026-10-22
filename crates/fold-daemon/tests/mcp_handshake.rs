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
use tokio::net::UnixListener;
use tower::ServiceExt as _;

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
    config: DaemonConfig,
    clock: ManualClock,
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
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
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
        let daemon = Daemon::start(config.clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        Self {
            _dir: dir,
            _project: project,
            root,
            daemon,
            config,
            clock,
        }
    }

    /// Stops the daemon and starts a new one on the same folders, as `fld restart` does.
    async fn restart(self) -> Self {
        let Self {
            _dir,
            _project,
            root,
            daemon,
            config,
            clock,
        } = self;
        daemon.shutdown().await;
        let daemon = Daemon::start(config.clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        Self {
            _dir,
            _project,
            root,
            daemon,
            config,
            clock,
        }
    }

    fn state(&self) -> AppState {
        self.daemon.state().clone()
    }

    fn hello(&self, cwd: &Path, agent: Option<&str>) -> Value {
        json!({"fold": PROTOCOL_VERSION, "cwd": cwd, "pid": 4242, "agent": agent})
    }

    async fn text(&self, sql: &'static str) -> String {
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
    tokio::spawn(handshake::accept(state, server, Default::default()));
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
/// The `list_agents` result reduced to each agent's name, tags, presence and worker names.
async fn list_agents(client: &Client) -> Value {
    let result = client
        .call_tool(CallToolRequestParams::new("list_agents"))
        .await
        .unwrap();
    let listed: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    let agents: Vec<Value> = listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| {
            let workers: Vec<&Value> = agent["workers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|worker| &worker["name"])
                .collect();
            json!({
                "name": agent["name"],
                "tags": agent["tags"],
                "presence": agent["presence"],
                "workers": workers,
            })
        })
        .collect();
    json!({ "agents": agents })
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
    assert_eq!(
        host.text("SELECT cwd FROM sessions WHERE pid = 4242").await,
        host.root.to_string_lossy(),
        "the session keeps the working directory of the handshake"
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
        assert_eq!(
            hint.as_deref(),
            Some("run the command inside a project directory, or `fld init` one")
        );
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
        json!({"agents": [{"name": "frontend", "tags": [], "presence": "active", "workers": ["frontend"]}]})
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

/// The `error` object of a tool call that must fail.
#[cfg(test)]
async fn tool_error(client: &Client, tool: &str, arguments: Value) -> Value {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    let body: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    body["error"].clone()
}

#[tokio::test]
async fn an_invalid_config_refuses_no_session_and_fails_only_the_commands_that_need_the_lease() {
    let host = Host::start().await;
    std::fs::write(host._project.paths().config(), "lease_minutes = 0\n").unwrap();
    let (answer, rest) = handshake(host.state(), &host.hello(&host.root, Some("frontend"))).await;
    assert_eq!(answer, Answer::accepted("app"));
    let client = mcp(rest).await;
    let created = client
        .call_tool(
            CallToolRequestParams::new("create_task")
                .with_arguments(json!({"title": "Leased"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(created.is_error, Some(false), "{created:?}");
    for (tool, arguments) in [
        ("claim_task", json!({"id": "T-1"})),
        ("create_workspace", json!({"task": "T-1"})),
    ] {
        let error = tool_error(&client, tool, arguments).await;
        assert_eq!(error["code"], "invalid_config", "{tool}");
        assert!(
            error["hint"].as_str().unwrap().contains("config.toml"),
            "{tool}: {error}"
        );
    }
    let response = crate::common::signed_in_router(&host.state(), "ana")
        .await
        .oneshot(
            axum::http::Request::get("/api/v1/projects/app/repository")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_silent_client_is_refused_after_five_seconds() {
    let host = Host::start().await;
    let (client, server) = tokio::io::duplex(1024);
    tokio::time::pause();
    let accept = tokio::spawn(handshake::accept(host.state(), server, Default::default()));
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
        json!({"agents": [{"name": "review", "tags": [], "presence": "active", "workers": ["review"]}]})
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

#[tokio::test]
async fn the_tags_of_the_handshake_register_the_main_agent_once() {
    let host = Host::start().await;
    let mut hello = host.hello(&host.root, Some("main"));
    hello["tags"] = json!(["main"]);
    let (answer, (read, write)) = handshake(host.state(), &hello).await;
    assert!(answer.ok, "{answer:?}");
    let client: Client = ().serve((read, write)).await.unwrap();
    assert_eq!(
        list_agents(&client).await,
        json!({"agents": [{"name": "main", "tags": ["main"], "presence": "active", "workers": ["main"]}]})
    );

    let mut other = host.hello(&host.root, Some("lead"));
    other["tags"] = json!(["main"]);
    let (code, _) = refused(host.state(), format!("{other}\n").as_bytes()).await;
    assert_eq!(code, "main_tag_denied");
    let mut bad = host.hello(&host.root, Some("lead"));
    bad["tags"] = json!(["Not a tag"]);
    let (code, _) = refused(host.state(), format!("{bad}\n").as_bytes()).await;
    assert_eq!(code, "invalid_name");
    drop(client);
    host.daemon.shutdown().await;
}

#[test]
fn parse_hello_reads_the_tags_and_an_adapter_without_them_sends_none() {
    let hello =
        parse_hello(br#"{"fold":1,"cwd":"/src/app","pid":3,"agent":"main","tags":["main"]}"#)
            .unwrap();
    assert_eq!(hello.tags, ["main"]);
    let hello = parse_hello(br#"{"fold":1,"cwd":"/src/app","pid":3,"agent":"web"}"#).unwrap();
    assert!(hello.tags.is_empty());
    assert!(!serde_json::to_string(&hello).unwrap().contains("tags"));
}

#[tokio::test]
async fn shutdown_answers_an_open_wait_for_events_and_ends_before_the_drain_timeout() {
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
    let Some(fold_daemon::projects::Slot::Open(handle)) = host.state().projects.slot("app") else {
        panic!("project app is not open");
    };
    let waiting = tokio::spawn(async move {
        let mut arguments = serde_json::Map::new();
        arguments.insert("types".to_owned(), json!(["main.advanced"]));
        arguments.insert("timeout_s".to_owned(), json!(1500));
        client
            .call_tool(CallToolRequestParams::new("wait_for_events").with_arguments(arguments))
            .await
    });
    eventually(WAIT, async || {
        let waiters = handle.bus.waiters();
        if waiters == 1 { Ok(()) } else { Err(waiters) }
    })
    .await;
    drop(handle);

    // Shorter than the drain timeout, so the session must end because the daemon ended it.
    tokio::time::timeout(Duration::from_secs(4), host.daemon.shutdown())
        .await
        .expect("shutdown waited for the open wait");
    let result = tokio::time::timeout(WAIT, waiting)
        .await
        .unwrap()
        .unwrap()
        .expect("the wait got its answer before the session ended");
    let body: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(body, json!({"timed_out": true}));
}

/// A stand-in for a Claude Code session's inbox at `path`: the bytes of each connection, read to its end. A connection that sends nothing, as the check of a kept socket, delivers nothing.
#[cfg(test)]
fn session_inbox(path: &Path) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let listener = UnixListener::bind(path).unwrap();
    let (sender, received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            if !bytes.is_empty() {
                let _ = sender.send(String::from_utf8(bytes).unwrap());
            }
        }
    });
    received
}

#[cfg(test)]
async fn next_notice(inbox: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(WAIT, inbox.recv())
        .await
        .expect("a notice within the wait")
        .unwrap()
}

#[cfg(test)]
const INBOX_TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// The session-start hook of `agent` with the session's inbox, through the daemon's socket router.
#[cfg(test)]
async fn session_start_hook(host: &Host, agent: &str, socket: &Path) -> Value {
    let body = json!({"cwd": host.root, "agent": agent, "socket": socket, "token": INBOX_TOKEN});
    let request = axum::http::Request::builder()
        .header(fold_daemon::http::host::CLIENT_HEADER, "test")
        .method(axum::http::Method::POST)
        .uri("/api/v1/hooks/session-start")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = fold_daemon::http::socket_router(host.state())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// Sends one message from the human `ana` to `main` through the project's message service, which opens the project first.
#[cfg(test)]
async fn message_main(host: &Host) {
    let state = host.state();
    let ana = fold_core::HumanName::parse("ana").unwrap();
    let now = host.clock.now();
    state
        .registry
        .write(async |tx| projects::ensure_human(tx, &ana, now).await)
        .await
        .unwrap();
    let handle = fold_daemon::services::projects::open_for_dir(&state, &host.root)
        .await
        .unwrap();
    handle
        .messages(&state.registry)
        .send(
            &fold_core::Actor::human(ana),
            fold_daemon::services::messages::SendMessage {
                to: fold_core::Address::parse_all(&["main"]).unwrap(),
                body: fold_core::MessageBody::parse("are you there").unwrap(),
                task: None,
                proposal: None,
                reply_to: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn the_first_session_of_a_new_project_gets_notices_once_its_adapter_registers_the_agent() {
    let host = Host::start().await;
    let socket = host.root.parent().unwrap().join("main-inbox.sock");
    let mut inbox = session_inbox(&socket);
    let answer = session_start_hook(&host, "main", &socket).await;
    assert!(
        answer["context"]
            .as_str()
            .unwrap()
            .contains("not registered in this project yet"),
        "{answer}"
    );
    let (accepted, rest) = handshake(host.state(), &host.hello(&host.root, Some("main"))).await;
    assert!(accepted.ok);
    let client = mcp(rest).await;

    message_main(&host).await;
    assert!(
        next_notice(&mut inbox)
            .await
            .contains("fold: message m-1 arrived. Call get_inbox.")
    );
    client.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_session_gets_the_next_notice_after_the_daemon_restarts() {
    let host = Host::start().await;
    let socket = host.root.parent().unwrap().join("main-inbox.sock");
    let mut inbox = session_inbox(&socket);
    let (accepted, rest) = handshake(host.state(), &host.hello(&host.root, Some("main"))).await;
    assert!(accepted.ok);
    let client = mcp(rest).await;
    session_start_hook(&host, "main", &socket).await;
    message_main(&host).await;
    assert!(next_notice(&mut inbox).await.contains("m-1 arrived"));

    let host = host.restart().await;
    drop(client);
    message_main(&host).await;
    let notice = next_notice(&mut inbox).await;
    assert!(notice.contains("m-2 arrived"), "{notice}");
    assert!(notice.contains(INBOX_TOKEN));
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn the_adapter_reports_the_inbox_that_claude_code_gave_it() {
    let host = Host::start().await;
    let socket = host.root.parent().unwrap().join("main-inbox.sock");
    let mut inbox = session_inbox(&socket);
    let mut hello = host.hello(&host.root, Some("main"));
    hello["inbox"] = json!({"socket": socket, "token": INBOX_TOKEN});
    let (accepted, rest) = handshake(host.state(), &hello).await;
    assert!(accepted.ok);
    let client = mcp(rest).await;
    message_main(&host).await;
    assert!(next_notice(&mut inbox).await.contains("m-1 arrived"));
    let kept = host.config.push_dir().join("app").join("main.json");
    assert!(!host.root.join(".fold").join("push").exists());
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&kept).unwrap().permissions())
            & 0o777,
        0o600
    );

    // A relative socket is refused without refusing the session, and the kept file stays the one above.
    let mut bad = host.hello(&host.root, Some("main"));
    bad["inbox"] = json!({"socket": "relative.sock", "token": INBOX_TOKEN});
    let (accepted, _rest) = handshake(host.state(), &bad).await;
    assert!(accepted.ok, "a bad inbox never refuses the session");
    assert!(
        std::fs::read_to_string(&kept)
            .unwrap()
            .contains("main-inbox.sock")
    );
    client.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

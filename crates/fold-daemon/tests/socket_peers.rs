use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use fold_core::events::types;
use fold_core::reviews::checks::Outcome;
use fold_core::{AgentName, Clock, EventFilter, ManualClock, Role, WorkerState};
use fold_daemon::http::socket_router;
use fold_daemon::mcp::handshake::{self, Answer, PROTOCOL_VERSION};
use fold_daemon::projects::{ProjectHandle, Slot};
use fold_daemon::services::agents::peers::{self, PeerWorker};
use fold_daemon::services::reviews::checks::{
    CheckRunner, CommandResult, CommandSpec, RunLimits, run_commands, run_commands_for,
};
use fold_daemon::socket::SocketPeer;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use fold_store::agents::{self, NewWorkerProcess};
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError};
use http_body_util::BodyExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tower::ServiceExt as _;

#[cfg(test)]
const WAIT: Duration = Duration::from_secs(10);

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
        let daemon = Daemon::start(config, Arc::new(clock)).await.unwrap();
        Self {
            _dir: dir,
            _project: project,
            root,
            daemon,
        }
    }

    fn handle(&self) -> Arc<ProjectHandle> {
        match self.daemon.state().projects.slot("app") {
            Some(Slot::Open(handle)) => handle,
            other => panic!("project app is not open: {other:?}"),
        }
    }

    fn hello(&self, agent: Option<&str>) -> String {
        format!(
            "{}\n",
            json!({"fold": PROTOCOL_VERSION, "cwd": self.root, "pid": 4242, "agent": agent})
        )
    }

    /// Records `pid` as the process of the worker agent `agent`, as the supervisor does once it has started Claude Code.
    async fn record_worker(&self, agent: &str, pid: u32) {
        let handle = self.handle();
        let service = handle.agents();
        let mut ctx = service.start_session(1).await.unwrap();
        let name = AgentName::parse(agent).unwrap();
        service.register_agent(&mut ctx, &name, &[]).await.unwrap();
        service.end_session(&ctx).await.unwrap();
        let now = handle.clock.now();
        let session_id = format!("00000000-0000-4000-8000-{pid:012}");
        handle
            .store
            .write(async |tx| {
                let agent = agents::find_agent(tx.conn(), &name).await?.unwrap();
                let id = agents::insert_worker_process(
                    tx,
                    &NewWorkerProcess {
                        agent_id: agent.id,
                        role: Role::Reviewer,
                        area_id: None,
                        model: None,
                        session_id: &session_id,
                        started_by: "ana",
                        now,
                    },
                )
                .await?;
                agents::set_worker_pid(tx, id, Some(pid)).await?;
                agents::set_worker_state(tx, id, WorkerState::Working, now).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }

    /// The `data` of every stored `session.bind_refused` event.
    async fn refusals(&self) -> Vec<Value> {
        fold_store::events::read_after(self.handle().store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == types::SESSION_BIND_REFUSED)
            .map(|event| event.data)
            .collect()
    }
}

/// A fake worker process tree: a shell that stands for the worker's Claude Code process and a `sleep` it started, which stands for a command in the worker's Bash.
#[cfg(test)]
struct FakeWorker {
    shell: Child,
    worker: u32,
    descendant: u32,
}

#[cfg(test)]
impl FakeWorker {
    async fn spawn() -> Self {
        let mut shell = Command::new("sh")
            .args(["-c", "sleep 60 & echo $!; wait"])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let worker = shell.id().unwrap();
        let mut line = String::new();
        BufReader::new(shell.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .unwrap();
        let descendant = line.trim().parse().unwrap();
        Self {
            shell,
            worker,
            descendant,
        }
    }

    async fn stop(mut self) {
        let _ = std::process::Command::new("kill")
            .arg(self.descendant.to_string())
            .status();
        self.shell.kill().await.unwrap();
    }
}

/// Sends the handshake line on an in-process connection whose peer is `peer`, and returns the answer and the stream for MCP.
#[cfg(test)]
async fn handshake_as(
    host: &Host,
    peer: SocketPeer,
    agent: Option<&str>,
) -> (
    Answer,
    BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(handshake::accept(host.daemon.state().clone(), server, peer));
    let (read, mut write) = tokio::io::split(client);
    write.write_all(host.hello(agent).as_bytes()).await.unwrap();
    let mut read = BufReader::new(read);
    let mut line = String::new();
    read.read_line(&mut line).await.unwrap();
    (serde_json::from_str(&line).unwrap(), read, write)
}

/// Sends the handshake line over the daemon's real socket, so the kernel reports this test process as the peer.
#[cfg(test)]
async fn socket_handshake(host: &Host, agent: Option<&str>) -> Answer {
    let stream = UnixStream::connect(host.daemon.socket_path())
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    write.write_all(host.hello(agent).as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

/// A CLI request over the daemon's real socket: status and error code of the answer.
#[cfg(test)]
async fn socket_http(
    host: &Host,
    method: &str,
    path: &str,
    human: &str,
    body: &str,
) -> (u16, String) {
    let mut stream = UnixStream::connect(host.daemon.socket_path())
        .await
        .unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: fold\r\nconnection: close\r\nx-fold-human: {human}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8(response).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body: Value =
        serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap_or(Value::Null);
    (
        status,
        body["error"]["code"].as_str().unwrap_or("").to_owned(),
    )
}

#[tokio::test]
async fn a_command_in_a_workers_bash_binds_only_to_the_workers_own_agent() {
    let host = Host::start().await;
    let fake = FakeWorker::spawn().await;
    let bash = SocketPeer {
        pid: Some(fake.descendant),
    };

    let (answer, _, _) = handshake_as(&host, bash, Some("main")).await;
    assert_eq!(answer, Answer::accepted("app"), "no worker runs yet");

    host.record_worker("web-worker", fake.worker).await;
    let terminal = SocketPeer {
        pid: Some(std::process::id()),
    };
    let (answer, _, _) = handshake_as(&host, terminal, Some("main")).await;
    assert_eq!(
        answer,
        Answer::accepted("app"),
        "a process outside the worker's tree binds as main while the worker runs"
    );
    let (answer, _, _) = handshake_as(&host, bash, Some("review-lead")).await;
    let error = answer.error.unwrap();
    assert_eq!(error.code, "worker_peer_refused");
    assert_eq!(
        host.refusals().await,
        [json!({"agent": "review-lead", "worker": "web-worker", "pid": fake.descendant})]
    );

    let (answer, _, _) = handshake_as(&host, bash, Some("web-worker")).await;
    assert_eq!(
        answer,
        Answer::accepted("app"),
        "the worker's own adapter binds"
    );

    let (answer, read, write) = handshake_as(&host, bash, None).await;
    assert_eq!(answer, Answer::accepted("app"));
    let client: RunningService<RoleClient, ()> = ().serve((read, write)).await.unwrap();
    let Value::Object(arguments) = json!({"name": "main"}) else {
        unreachable!()
    };
    let result = client
        .call_tool(CallToolRequestParams::new("register_agent").with_arguments(arguments))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let body: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(body["error"]["code"], "worker_peer_refused");
    assert_eq!(host.refusals().await.len(), 2);
    client.cancel().await.unwrap();

    let (answer, _, _) = handshake_as(&host, SocketPeer::default(), Some("main")).await;
    assert_eq!(
        answer,
        Answer::accepted("app"),
        "a peer outside every worker binds as before"
    );
    fake.stop().await;
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn the_kernel_pid_decides_and_is_the_pid_the_session_records() {
    let host = Host::start().await;
    assert_eq!(
        socket_handshake(&host, Some("main")).await,
        Answer::accepted("app"),
        "the user's own terminal binds as main"
    );
    let me = std::process::id();
    let recorded: i64 = sqlx::query_scalar("SELECT pid FROM sessions ORDER BY id DESC LIMIT 1")
        .fetch_one(host.handle().store.read())
        .await
        .unwrap();
    assert_eq!(
        recorded,
        i64::from(me),
        "not the pid the handshake line claims"
    );

    host.record_worker("web-worker", me).await;
    let refused = socket_handshake(&host, Some("main")).await;
    assert_eq!(refused.error.unwrap().code, "worker_peer_refused");
    assert_eq!(
        socket_handshake(&host, Some("web-worker")).await,
        Answer::accepted("app")
    );
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_human_name_from_a_workers_process_tree_is_refused_over_the_socket() {
    let host = Host::start().await;
    let feedback = "/api/v1/projects/app/feedback";
    let (status, code) = socket_http(&host, "POST", feedback, "x", "{}").await;
    assert_eq!(
        (status, code.as_str()),
        (400, "invalid_request"),
        "the user's terminal gets past the human check"
    );

    host.record_worker("web-worker", std::process::id()).await;
    let (status, code) = socket_http(&host, "POST", feedback, "x", "{}").await;
    assert_eq!((status, code.as_str()), (403, "worker_peer_refused"));
    let (status, _) = socket_http(&host, "GET", "/api/v1/projects", "x", "").await;
    assert_eq!(status, 200, "a route that acts as no human still answers");
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_hook_from_a_workers_process_tree_names_only_the_workers_own_agent() {
    let host = Host::start().await;
    host.record_worker("web-worker", std::process::id()).await;
    let body = |agent: &str| {
        json!({"cwd": host.root, "agent": agent, "workspace": ".fold/ws/review-p-1"}).to_string()
    };
    for hook in [
        "/api/v1/hooks/stop",
        "/api/v1/hooks/session-start",
        "/api/v1/hooks/edit",
    ] {
        let (status, code) = socket_http(&host, "POST", hook, "x", &body("main")).await;
        assert_eq!(
            (status, code.as_str()),
            (403, "worker_peer_refused"),
            "{hook}"
        );
        let (status, _) = socket_http(&host, "POST", hook, "x", &body("web-worker")).await;
        assert_eq!(status, 200, "{hook}");
    }
    host.daemon.shutdown().await;
}

/// A check command running under fold's runner for `owner`: the pids of its shell and of a `sleep` the shell started, which stands for a `fld` command in the check. The command ends when `go` exists.
#[cfg(test)]
struct RunningCheck {
    shell: u32,
    descendant: u32,
    go: PathBuf,
    run: tokio::task::JoinHandle<Vec<CommandResult>>,
    _dir: TempDir,
}

#[cfg(test)]
impl RunningCheck {
    async fn start(owner: Option<PeerWorker>) -> Self {
        let dir = TempDir::new().unwrap();
        let pids = dir.path().join("pids");
        let go = dir.path().join("go");
        let run = format!(
            "sleep 60 & echo \"$$ $!\" > '{}'; until [ -e '{}' ]; do sleep 0.05; done",
            pids.display(),
            go.display()
        );
        let spec = CommandSpec {
            check: Some("fld".to_owned()),
            run,
            required: true,
            runs: true,
            timeout: None,
        };
        let work = dir.path().to_path_buf();
        let run = tokio::spawn(async move {
            let clock = ManualClock::new();
            match owner {
                Some(owner) => {
                    run_commands_for(&owner, &work, vec![spec], RunLimits::default(), &clock).await
                }
                None => run_commands(&work, vec![spec], RunLimits::default(), &clock).await,
            }
        });
        let line =
            fold_store::testing::eventually(WAIT, async || match std::fs::read_to_string(&pids) {
                Ok(line) if line.ends_with('\n') => Ok(line),
                other => Err(format!("{other:?}")),
            })
            .await;
        let mut pids = line.split_whitespace().map(|pid| pid.parse().unwrap());
        Self {
            shell: pids.next().unwrap(),
            descendant: pids.next().unwrap(),
            go,
            run,
            _dir: dir,
        }
    }

    fn peer(&self) -> SocketPeer {
        SocketPeer {
            pid: Some(self.descendant),
        }
    }

    async fn stop(self) {
        std::fs::write(&self.go, "").unwrap();
        let results = tokio::time::timeout(WAIT, self.run).await.unwrap().unwrap();
        assert_eq!(results[0].outcome, Outcome::Passed);
        assert_eq!(
            peers::check_peer(&[self.shell, std::process::id()]),
            None,
            "an ended check leaves the registry"
        );
    }
}

/// A request through the socket's router from `peer`, as the CLI sends it: status and error code of the answer.
#[cfg(test)]
async fn peer_http(
    host: &Host,
    peer: SocketPeer,
    path: &str,
    human: &str,
    body: String,
) -> (u16, String) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("host", "fold")
        .header("x-fold-human", human)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    request.extensions_mut().insert(peer);
    let response = socket_router(host.daemon.state().clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (
        status,
        body["error"]["code"].as_str().unwrap_or("").to_owned(),
    )
}

#[tokio::test]
async fn a_check_acts_as_the_worker_that_asked_for_it_and_never_as_a_human() {
    let host = Host::start().await;
    let owner = CheckRunner::default()
        .with_project("app")
        .owner(Some(AgentName::parse("web-worker").unwrap()));
    let check = RunningCheck::start(Some(owner)).await;
    let feedback = "/api/v1/projects/app/feedback";

    let (status, code) = peer_http(&host, check.peer(), feedback, "x", "{}".to_owned()).await;
    assert_eq!((status, code.as_str()), (403, "worker_peer_refused"));
    let (answer, _, _) = handshake_as(&host, check.peer(), Some("main")).await;
    assert_eq!(answer.error.unwrap().code, "worker_peer_refused");
    assert_eq!(
        host.refusals().await,
        [json!({"agent": "main", "worker": "web-worker", "pid": check.descendant})]
    );
    let (answer, _, _) = handshake_as(&host, check.peer(), Some("web-worker")).await;
    assert_eq!(
        answer,
        Answer::accepted("app"),
        "a check acts as the worker that asked for its run"
    );

    let terminal = SocketPeer {
        pid: Some(std::process::id()),
    };
    let (status, code) = peer_http(&host, terminal, feedback, "x", "{}".to_owned()).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "invalid_request"),
        "the user's terminal still names a human while a check runs"
    );
    let (answer, _, _) = handshake_as(&host, terminal, Some("main")).await;
    assert_eq!(answer, Answer::accepted("app"));
    check.stop().await;
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn a_check_that_no_worker_asked_for_acts_as_no_agent_and_no_human() {
    let host = Host::start().await;
    let check = RunningCheck::start(None).await;
    let (status, code) = peer_http(
        &host,
        check.peer(),
        "/api/v1/projects/app/feedback",
        "x",
        "{}".to_owned(),
    )
    .await;
    assert_eq!((status, code.as_str()), (403, "worker_peer_refused"));
    for agent in ["main", "web-worker"] {
        let (answer, _, _) = handshake_as(&host, check.peer(), Some(agent)).await;
        assert_eq!(answer.error.unwrap().code, "worker_peer_refused", "{agent}");
        let body = json!({"cwd": host.root, "agent": agent, "workspace": ".fold/ws/review-p-1"});
        let (status, code) = peer_http(
            &host,
            check.peer(),
            "/api/v1/hooks/stop",
            "x",
            body.to_string(),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (403, "worker_peer_refused"),
            "{agent}"
        );
    }
    check.stop().await;
    host.daemon.shutdown().await;
}

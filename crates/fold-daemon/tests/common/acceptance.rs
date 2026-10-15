use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::socket_router;
use fold_daemon::mcp::handshake::{self, Answer, PROTOCOL_VERSION};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::{TestProject, eventually};
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::ServiceExt as _;

/// The longest any single effect may take to appear.
pub const WAIT: Duration = Duration::from_secs(10);
pub const SLUG: &str = "demo";
/// The three sessions of the acceptance scenarios, in registration order.
pub const AGENTS: [&str; 3] = ["frontend", "backend", "review"];
/// The human behind the CLI requests: `FOLD_HUMAN`.
pub const HUMAN: &str = "ana";
/// `CLAUDE_CODE_MESSAGING_TOKEN` of every fake inbox.
pub const TOKEN: &str = "0123456789abcdef0123456789abcdef";

pub type Client = RunningService<RoleClient, ()>;

/// One adapter session: the MCP client and the daemon task that serves it. The task ends the fold session once the client's stream closes, so awaiting it is how a test knows the session is over.
pub struct Session {
    pub client: Client,
    server: JoinHandle<()>,
}

/// Project `demo` open in daemon state, with `r1` holding `README.md`, and one adapter session for each of [`AGENTS`], connected over in-memory streams through the same handshake `fold mcp` performs with `FOLD_AGENT` set. `review` holds `integrator`, granted by the human [`HUMAN`] over the socket.
pub struct Harness {
    pub project: TestProject,
    /// Holds the fake inbox sockets.
    dir: TempDir,
    row: ProjectRow,
    pub state: AppState,
    pub handle: Arc<ProjectHandle>,
    sessions: BTreeMap<String, Session>,
    next_pid: u32,
}

impl Harness {
    pub async fn start() -> Self {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        tree.insert(
            b"README.md".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, b"# demo\n").unwrap(),
            },
        );
        let revision =
            initial_revision(write_initial_tree(&objects, tree).unwrap(), project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let row = ProjectRow {
            slug: SLUG.to_owned(),
            name: "Demo".to_owned(),
            root: project
                .paths()
                .root()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        };
        project
            .registry()
            .write(async |tx| projects::insert_project(tx, &row, project.now()).await)
            .await
            .unwrap();
        let (state, handle) = open(&project, &row).await;
        let mut harness = Self {
            project,
            dir: TempDir::new().unwrap(),
            row,
            state,
            handle,
            sessions: BTreeMap::new(),
            next_pid: 4000,
        };
        for agent in AGENTS {
            harness.connect(agent).await;
        }
        let (status, answer) = harness
            .socket(
                Method::PUT,
                &format!("/api/v1/projects/{SLUG}/agents/review/tags/integrator"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        harness
    }

    /// The canonical project root: the `cwd` of every session and hook.
    pub fn root(&self) -> &str {
        &self.row.root
    }

    /// Opens a new adapter session of `agent`: the handshake line, the daemon's answer, then MCP on the rest of the stream.
    pub async fn connect(&mut self, agent: &str) {
        assert!(
            !self.sessions.contains_key(agent),
            "{agent} is already connected"
        );
        self.next_pid += 1;
        let (client_end, server_end) = tokio::io::duplex(256 * 1024);
        let server = tokio::spawn(handshake::accept(self.state.clone(), server_end));
        let (read, mut write) = tokio::io::split(client_end);
        let hello = json!({"fold": PROTOCOL_VERSION, "cwd": self.root(), "pid": self.next_pid, "agent": agent});
        write
            .write_all(format!("{hello}\n").as_bytes())
            .await
            .unwrap();
        let mut read = BufReader::new(read);
        let mut line = String::new();
        tokio::time::timeout(WAIT, read.read_line(&mut line))
            .await
            .expect("the handshake answer within the wait")
            .unwrap();
        let answer: Answer = serde_json::from_str(&line).unwrap();
        assert!(answer.ok, "the handshake of {agent} was refused: {line}");
        assert_eq!(answer.project.as_deref(), Some(SLUG));
        let client = ().serve((read, write)).await.unwrap();
        self.sessions
            .insert(agent.to_owned(), Session { client, server });
    }

    /// Closes the session's stream and returns once the daemon has ended the fold session.
    pub async fn disconnect(&mut self, agent: &str) {
        let session = self
            .sessions
            .remove(agent)
            .unwrap_or_else(|| panic!("{agent} is not connected"));
        session.client.cancel().await.unwrap();
        tokio::time::timeout(WAIT, session.server)
            .await
            .expect("the session ends within the wait")
            .unwrap();
    }

    /// Ends the session of `agent` and opens a new one for the same agent, as a restarted Claude Code session does.
    pub async fn reconnect(&mut self, agent: &str) {
        self.disconnect(agent).await;
        self.connect(agent).await;
    }

    /// Stops the project the way a stopping daemon does, opens the same files again the way a starting daemon does, and reconnects the sessions that were connected. Push targets live in memory, so none survive.
    pub async fn restart(&mut self) {
        let connected: Vec<String> = self.sessions.keys().cloned().collect();
        for agent in &connected {
            self.disconnect(agent).await;
        }
        self.handle.bus.close();
        for live in self.handle.live.all() {
            self.handle.live.detach(live.view.workspace).await;
        }
        self.handle
            .agents()
            .close_sessions_at_shutdown()
            .await
            .unwrap();
        self.handle.store.close().await.unwrap();
        let (state, handle) = open(&self.project, &self.row).await;
        handle.agents().close_orphan_sessions().await.unwrap();
        self.state = state;
        self.handle = handle;
        for agent in AGENTS {
            if connected.iter().any(|name| name == agent) {
                self.connect(agent).await;
            }
        }
    }

    pub fn client(&self, agent: &str) -> &Client {
        &self
            .sessions
            .get(agent)
            .unwrap_or_else(|| panic!("{agent} is not connected"))
            .client
    }

    pub async fn call_raw(&self, agent: &str, tool: &str, arguments: Value) -> CallToolResult {
        let Value::Object(arguments) = arguments else {
            panic!("arguments must be an object");
        };
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        self.client(agent).call_tool(params).await.unwrap()
    }

    /// The result body of a call that must succeed.
    pub async fn call(&self, agent: &str, tool: &str, arguments: Value) -> Value {
        let result = self.call_raw(agent, tool, arguments).await;
        assert_eq!(
            result.is_error,
            Some(false),
            "{agent} {tool} failed: {result:?}"
        );
        body(&result)
    }

    /// `send_message` from `from` to `to`; returns the new message id.
    pub async fn send(&self, from: &str, to: &[&str], text: &str) -> String {
        self.send_with(from, json!({"to": to, "body": text})).await
    }

    /// `send_message` with these arguments; returns the new message id.
    pub async fn send_with(&self, from: &str, arguments: Value) -> String {
        let sent = self.call(from, "send_message", arguments).await;
        sent["id"].as_str().unwrap().to_owned()
    }

    /// The ids `get_inbox` returns to `agent`, oldest first.
    pub async fn inbox(&self, agent: &str) -> Vec<String> {
        ids(&self.call(agent, "get_inbox", json!({})).await)
    }

    /// Reads every event after the agent's cursor, so its next wait starts from now.
    pub async fn drain_events(&self, agent: &str) {
        tokio::time::timeout(WAIT, async {
            while !self.call(agent, "get_events", json!({"limit": 500})).await["events"]
                .as_array()
                .unwrap()
                .is_empty()
            {}
        })
        .await
        .expect("the backlog drains within the wait");
    }

    /// Waits until `count` calls of `wait_for_events` have read their backlog and listen on the bus alone.
    pub async fn until_waiting(&self, count: usize) {
        let bus = &self.handle.bus;
        eventually(WAIT, async || {
            let waiters = bus.waiters();
            if waiters >= count {
                Ok(())
            } else {
                Err(waiters)
            }
        })
        .await;
    }

    /// Binds a fake inbox for `agent` and reports it through the session-start hook route, as `fold hook session-start` does when Claude Code sets `CLAUDE_CODE_MESSAGING_SOCKET` and `CLAUDE_CODE_MESSAGING_TOKEN`.
    pub async fn register_inbox(&self, agent: &str) -> FakeInbox {
        let inbox = FakeInbox::bind(self.dir.path().join(format!("{agent}.sock")));
        let hook = json!({
            "cwd": self.root(),
            "agent": agent,
            "socket": inbox.path,
            "token": TOKEN,
        });
        let (status, answer) = self
            .socket(Method::POST, "/api/v1/hooks/session-start", Some(&hook))
            .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(
            answer["context"]
                .as_str()
                .unwrap()
                .contains(&format!("You are agent {agent}")),
            "{answer}"
        );
        let name = fold_core::AgentName::parse(agent).unwrap();
        assert_eq!(self.handle.push.sockets(&name), vec![inbox.path.clone()]);
        inbox
    }

    /// A request over the Unix socket router as the human [`HUMAN`], the way the CLI sends it.
    pub async fn socket(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("x-fold-human", HUMAN);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = socket_router(self.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// How many rows the `messages` table holds.
    pub async fn message_rows(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(self.handle.store.read())
            .await
            .unwrap()
    }
}

async fn open(project: &TestProject, row: &ProjectRow) -> (AppState, Arc<ProjectHandle>) {
    let clock = Arc::new(project.clock().clone());
    let state: AppState = Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default(),
        clock: clock.clone(),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1".parse().unwrap(),
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    });
    let handle = state.projects.open(row, clock).await.unwrap();
    (state, handle)
}

/// The JSON body of a tool result, without the footer.
pub fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// The footer line of a tool result. It counts the caller's own unread messages and new events, so it differs between callers.
pub fn footer(result: &CallToolResult) -> Option<String> {
    result
        .content
        .get(1)
        .map(|block| block.as_text().unwrap().text.clone())
}

/// The `id` of each entry of a `messages` list.
pub fn ids(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap().to_owned())
        .collect()
}

/// A stand-in for a Claude Code session's inbox socket: it accepts each connection, reads it to the end and never writes back.
pub struct FakeInbox {
    pub path: PathBuf,
    received: mpsc::UnboundedReceiver<String>,
}

impl FakeInbox {
    pub fn bind(path: PathBuf) -> Self {
        let listener = UnixListener::bind(&path).unwrap();
        let (sender, received) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let sender = sender.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    stream.read_to_end(&mut bytes).await.unwrap();
                    let _ = sender.send(String::from_utf8(bytes).unwrap());
                });
            }
        });
        Self { path, received }
    }

    /// The bytes of the next connection.
    pub async fn next(&mut self) -> String {
        tokio::time::timeout(WAIT, self.received.recv())
            .await
            .expect("a post within the wait")
            .unwrap()
    }
}

/// The bytes a session's inbox receives for one notice: the auth line, then the notice in fold's cross-session envelope.
pub fn wire(token: &str, text: &str) -> String {
    format!(
        "{{\"type\":\"auth\",\"token\":\"{token}\"}}\n{{\"type\":\"user\",\"priority\":\"next\",\"message\":{{\"role\":\"user\",\"content\":\"<cross-session-message from-name=\\\"fold\\\">\\n{text}\\n</cross-session-message>\"}}}}\n"
    )
}

/// The notice text for a new message.
pub fn message_notice(message: &str) -> String {
    format!("fold: message {message} arrived. Call get_inbox.")
}

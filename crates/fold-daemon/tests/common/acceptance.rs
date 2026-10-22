use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::object::ObjectKind;
use fold_core::roles::Role;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{router, socket_router};
use fold_daemon::mcp::handshake::{self, Answer, PROTOCOL_VERSION};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::Launcher;
use fold_daemon::services::projects::{DEFAULT_SESSION_LIFETIME, InitializedProject, init_project};
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::{TestProject, eventually};
use fold_store::workspaces::{self, RevisionRow, create_initial_revision, initial_revision};
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

/// The pid that the extra handshake of [`Harness::make_main`] sends; no other session uses it.
const MAIN_HANDSHAKE_PID: u32 = 900_000;
pub const SLUG: &str = "demo";
/// The three sessions of the acceptance scenarios, in registration order.
pub const AGENTS: [&str; 3] = ["frontend", "backend", "review"];
/// A launcher whose Claude Code is a file that cannot run, so a scenario without a stand-in never runs the real `claude`, and a worker it starts fails with the plain `spawn` reason.
fn no_claude() -> Launcher {
    Launcher {
        claude: std::path::PathBuf::from("/dev/null"),
        ..Launcher::default()
    }
}

pub use crate::common::roles::WORK_AREAS as WORK;
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

/// Project `demo` open in daemon state, with `r1` holding `README.md` and the project folder holding it as `fld init` leaves it, and one adapter session for each of [`AGENTS`], connected over in-memory streams through the same handshake `fld mcp` performs with `FOLD_AGENT` set. `review` is a reviewer that fold started and `frontend` and `backend` lead the areas of [`WORK`], as if fold had started them, and [`HUMAN`] is a known human.
pub struct Harness {
    pub project: TestProject,
    /// Holds the fake inbox sockets.
    dir: TempDir,
    row: ProjectRow,
    pub state: AppState,
    pub handle: Arc<ProjectHandle>,
    sessions: BTreeMap<String, Session>,
    next_pid: u32,
    /// The labels of the areas of [`WORK`] that have their lead; [`Harness::create_task`] gives a task one of them.
    work_labels: Vec<&'static str>,
    /// The programs the project's worker processes run, kept for [`Harness::start_again`].
    launcher: Launcher,
}

impl Harness {
    pub async fn start() -> Self {
        let mut harness = Self::start_without_leads().await;
        harness.lead_the_work_area().await;
        harness
    }

    /// [`Harness::start`] without the area leads: `frontend` and `backend` have no fold role, so neither claims a task, and either may become the main agent.
    pub async fn start_without_leads() -> Self {
        let harness = Self::start_without_roles().await;
        crate::common::roles::give_role(&harness.handle.store, "review", Role::Reviewer, None)
            .await;
        fold_daemon::services::projects::ensure_human(
            harness.project.registry(),
            harness.project.clock(),
            &fold_core::HumanName::parse(HUMAN).unwrap(),
        )
        .await
        .unwrap();
        harness
    }

    /// The same project and sessions with no worker role and no main agent, so no session may approve, reject or integrate.
    pub async fn start_without_roles() -> Self {
        Self::start_with_launcher(no_claude()).await
    }

    /// [`Harness::start_without_roles`] with the programs the project's worker processes run, such as a stand-in for Claude Code.
    pub async fn start_with_launcher(launcher: Launcher) -> Self {
        Self::start_on(TestProject::new().await, launcher).await
    }

    /// [`Harness::start`] in a plain project: no Git repository, and Git sync off.
    pub async fn start_plain() -> Self {
        let mut harness = Self::start_on(TestProject::plain().await, no_claude()).await;
        crate::common::roles::give_role(&harness.handle.store, "review", Role::Reviewer, None)
            .await;
        harness.lead_the_work_area().await;
        fold_daemon::services::projects::ensure_human(
            harness.project.registry(),
            harness.project.clock(),
            &fold_core::HumanName::parse(HUMAN).unwrap(),
        )
        .await
        .unwrap();
        harness
    }

    async fn start_on(project: TestProject, launcher: Launcher) -> Self {
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        tree.insert(
            b"README.md".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, b"# demo\n").unwrap(),
            },
        );
        let tree = write_initial_tree(&objects, tree).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        std::fs::write(project.paths().root().join("README.md"), b"# demo\n").unwrap();
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
        let (state, handle) = open(&project, &row, &launcher).await;
        let mut harness = Self {
            project,
            dir: TempDir::new().unwrap(),
            row,
            state,
            handle,
            sessions: BTreeMap::new(),
            next_pid: 4000,
            work_labels: Vec::new(),
            launcher,
        };
        for agent in AGENTS {
            harness.connect(agent).await;
        }
        harness
    }

    /// Project `demo` made by `fld init` in a folder that holds the files of `source`, so `r1` holds them, with the three sessions connected and no tag granted. Returns what init reported.
    pub async fn start_initialized(source: &Path) -> (Self, InitializedProject) {
        let mut initialized = None;
        let project = TestProject::initialized(async |root, host, clock| {
            copy_dir(source, root);
            let made = init_project(host, root, Some("Demo"), &crate::common::ana(), clock)
                .await
                .unwrap();
            initialized = Some(made);
        })
        .await;
        let initialized = initialized.unwrap();
        assert_eq!(initialized.slug, SLUG);
        let row = ProjectRow {
            slug: initialized.slug.clone(),
            name: initialized.name.clone(),
            root: initialized.root.to_string_lossy().into_owned(),
        };
        let launcher = no_claude();
        let (state, handle) = open(&project, &row, &launcher).await;
        let mut harness = Self {
            project,
            dir: TempDir::new().unwrap(),
            row,
            state,
            handle,
            sessions: BTreeMap::new(),
            next_pid: 4000,
            work_labels: Vec::new(),
            launcher,
        };
        for agent in AGENTS {
            harness.connect(agent).await;
        }
        (harness, initialized)
    }

    /// Makes `frontend` and `backend` the leads of their areas of [`WORK`], as if fold had started them, so each claims the tasks that [`Harness::create_task`] labels: only a lead claims a task.
    pub async fn lead_the_work_area(&mut self) {
        crate::common::roles::lead_work_areas(&self.handle.store).await;
        self.work_labels = WORK.map(|(_, label)| label).to_vec();
    }

    /// Makes `agent`, `frontend` or `backend`, the lead of its area of [`WORK`] alone, for a scenario where the other one has no fold role, such as one that becomes the main agent.
    pub async fn lead_area(&mut self, agent: &str) {
        let (_, area) = WORK
            .into_iter()
            .find(|(name, _)| *name == agent)
            .expect("an agent of WORK");
        crate::common::roles::give_role(&self.handle.store, agent, Role::AreaLead, Some(area))
            .await;
        self.work_labels.push(area);
    }

    /// The label of `frontend`'s area of [`WORK`], for a test that creates a task with its own arguments: a task has at most one area, so only that lead claims it.
    pub fn work_labels() -> Value {
        Self::work_labels_of("frontend")
    }

    /// The label of the area of [`WORK`] that `agent` leads, for a task that `agent` claims.
    pub fn work_labels_of(agent: &str) -> Value {
        json!(
            crate::common::roles::work_labels_of(agent)
                .iter()
                .map(|label| label.as_str().to_owned())
                .collect::<Vec<_>>()
        )
    }

    /// The canonical project root: the `cwd` of every session and hook.
    pub fn root(&self) -> &str {
        &self.row.root
    }

    /// Opens a new adapter session of `agent`: the handshake line, the daemon's answer, then MCP on the rest of the stream.
    pub async fn connect(&mut self, agent: &str) {
        self.connect_tagged(agent, &[]).await;
    }

    /// [`Harness::connect`] with the tags of `FOLD_AGENT_TAGS` in the handshake line, as `fld claude` gives its session the `main` tag.
    pub async fn connect_tagged(&mut self, agent: &str, tags: &[&str]) {
        assert!(
            !self.sessions.contains_key(agent),
            "{agent} is already connected"
        );
        self.next_pid += 1;
        let (client_end, server_end) = tokio::io::duplex(256 * 1024);
        let server = tokio::spawn(handshake::accept(
            self.state.clone(),
            server_end,
            Default::default(),
        ));
        let (read, mut write) = tokio::io::split(client_end);
        let hello = json!({"fold": PROTOCOL_VERSION, "cwd": self.root(), "pid": self.next_pid, "agent": agent, "tags": tags});
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
        self.stop().await;
        self.start_again().await;
        for agent in AGENTS {
            if connected.iter().any(|name| name == agent) {
                self.connect(agent).await;
            }
        }
    }

    /// Stops the project the way a stopping daemon does: the sessions end, the stream closes, the live state lets go of the workspaces, the worker processes end with their rows unchanged, the commit writer and the push delivery end and the database closes. [`Harness::start_again`] opens it again.
    pub async fn stop(&mut self) {
        let connected: Vec<String> = self.sessions.keys().cloned().collect();
        for agent in &connected {
            self.disconnect(agent).await;
        }
        self.handle.bus.close();
        for live in self.handle.live.all() {
            self.handle.live.detach(live.view.workspace).await;
        }
        self.handle.workers.shutdown().await;
        self.handle.stop_tasks(WAIT).await;
        self.handle
            .agents()
            .close_sessions_at_shutdown()
            .await
            .unwrap();
        self.handle.store.close().await.unwrap();
    }

    /// Stops the project the way a killed daemon stops: the session tasks are aborted, so no session ends, and the workspace watchers stop without a last write, so nothing the test changes while the daemon is down reaches the old handle. Nothing is closed or checkpointed. [`Harness::start_again`] opens it again.
    pub async fn drop_daemon(&mut self) {
        for (_, session) in std::mem::take(&mut self.sessions) {
            session.server.abort();
            let _ = session.server.await;
            drop(session.client);
        }
        for live in self.handle.live.all() {
            self.handle.live.detach(live.view.workspace).await;
        }
    }

    /// Opens the same files again the way a starting daemon does, through the project's start-up sequence, with no session connected.
    pub async fn start_again(&mut self) {
        let (state, handle) = open(&self.project, &self.row, &self.launcher).await;
        self.state = state;
        self.handle = handle;
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

    /// Gives `agent` the `main` tag, as `fld claude` gives it to the session it starts, so that agent does the main agent's work. Only a handshake carries the tag, so this opens one more session for `agent` with the tag in its handshake line and closes it again; the agent's own session stays.
    pub async fn make_main(&self, agent: &str) {
        let (client_end, server_end) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(handshake::accept(
            self.state.clone(),
            server_end,
            Default::default(),
        ));
        let (read, mut write) = tokio::io::split(client_end);
        let hello = json!({"fold": PROTOCOL_VERSION, "cwd": self.root(), "pid": MAIN_HANDSHAKE_PID, "agent": agent, "tags": ["main"]});
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
        assert!(
            answer.ok,
            "the main handshake of {agent} was refused: {line}"
        );
        drop((read, write));
        tokio::time::timeout(WAIT, server)
            .await
            .expect("the main handshake's session ends within the wait")
            .unwrap();
    }

    /// Gives the reviewer `agent` the review claim on `proposal`, such as `P-1`, as the review lead's `start_worker` gives it to a reviewer that fold starts; a reviewer never claims itself.
    pub async fn give_review(&self, agent: &str, proposal: &str) {
        use fold_core::Clock as _;
        crate::common::roles::give_review_claim(
            &self.handle.store,
            agent,
            proposal.parse().unwrap(),
            self.project.clock().now(),
        )
        .await;
    }

    /// Makes a review batch of `proposals` for the reviewer `agent`, as the review lead's `start_worker` makes it for a reviewer of two or more proposals; returns its id, such as `rb-1`.
    pub async fn give_batch(&self, agent: &str, proposals: &[&str]) -> String {
        use fold_core::Clock as _;
        let proposals: Vec<fold_core::ProposalId> =
            proposals.iter().map(|id| id.parse().unwrap()).collect();
        crate::common::roles::give_batch(
            &self.handle.store,
            agent,
            &proposals,
            self.project.clock().now(),
        )
        .await
        .to_string()
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

    /// Reads every event after the main worker's cursor, so its next wait starts from now.
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

    /// Binds a fake inbox for `agent` and reports it through the session-start hook route, as `fld hook session-start` does when Claude Code sets `CLAUDE_CODE_MESSAGING_SOCKET` and `CLAUDE_CODE_MESSAGING_TOKEN`.
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
            .header(CLIENT_HEADER, "test")
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

    /// `POST /requests` with the multipart form of `parts`, as the human `human` when one is named, the way the web's New request dialog sends it.
    pub async fn create_request_form(
        &self,
        parts: &[FormPart<'_>],
        human: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(Method::POST)
            .uri(format!("/api/v1/projects/{SLUG}/requests"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={FORM_BOUNDARY}"),
            );
        if let Some(human) = human {
            request = request.header("cookie", format!("fold_human={human}"));
        }
        let request = request.body(Body::from(multipart_form(parts))).unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// A request through the HTTP router as the human `human`, named by the `fold_human` cookie the way the browser sends it.
    pub async fn http(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        human: &str,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(method)
            .uri(path)
            .header("cookie", format!("fold_human={human}"));
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// Every stored event in commit order, read from the project's read pool.
    pub async fn events(&self) -> Vec<StoredEvent> {
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT e.type, e.actor, t.number, p.number, e.data FROM events e \
             LEFT JOIN tasks t ON t.id = e.task_id \
             LEFT JOIN proposals p ON p.id = e.proposal_id ORDER BY e.seq",
        )
        .fetch_all(self.handle.store.read())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(kind, actor, task, proposal, data)| StoredEvent {
                kind,
                actor,
                task: task.map(|number| format!("T-{number}")),
                proposal: proposal.map(|number| format!("P-{number}")),
                data: serde_json::from_str(&data).unwrap(),
            })
            .collect()
    }

    /// The head of main, read from the project's read pool.
    pub async fn head(&self) -> RevisionRow {
        workspaces::head(self.handle.store.read())
            .await
            .unwrap()
            .unwrap()
    }

    /// How many completion rows `task` has: one once it is Done, else none.
    pub async fn completions(&self, task: &str) -> i64 {
        let number: i64 = task.strip_prefix("T-").unwrap().parse().unwrap();
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM completions c JOIN tasks t ON t.id = c.task_id WHERE t.number = ?",
        )
        .bind(number)
        .fetch_one(self.handle.store.read())
        .await
        .unwrap()
    }

    /// `create_task` by `agent`; returns the new task id. The task gets the label of `agent`'s area of [`WORK`] when `agent` leads one, else the label of the first led area, so the lead of that one area claims it.
    pub async fn create_task(&self, agent: &str, title: &str) -> String {
        let own = WORK
            .into_iter()
            .find(|(name, label)| *name == agent && self.work_labels.contains(label))
            .map(|(_, label)| label);
        let arguments = match own.or_else(|| self.work_labels.first().copied()) {
            Some(label) => json!({"title": title, "labels": [label]}),
            None => json!({"title": title}),
        };
        let created = self.call(agent, "create_task", arguments).await;
        created["id"].as_str().unwrap().to_owned()
    }

    /// `create_task` by `agent` for a task in the area of [`WORK`] that `lead` leads; returns the new task id.
    pub async fn create_task_for(&self, agent: &str, lead: &str, title: &str) -> String {
        let arguments = json!({"title": title, "labels": Self::work_labels_of(lead)});
        let created = self.call(agent, "create_task", arguments).await;
        created["id"].as_str().unwrap().to_owned()
    }

    /// `agent` claims `task` and creates its workspace. A worker that fold started holds the claim from its lead's `start_worker` and never claims, so it only creates the workspace.
    pub async fn start_work(&self, agent: &str, task: &str) -> Work {
        let view = self.call(agent, "get_task", json!({"id": task})).await;
        if view["claim"]["holder"] != agent {
            let claimed = self.call(agent, "claim_task", json!({"id": task})).await;
            assert_eq!(claimed["holder"], agent);
        }
        let created = self
            .call(agent, "create_workspace", json!({"task": task}))
            .await;
        Work {
            task: task.to_owned(),
            workspace: created["workspace"].as_str().unwrap().to_owned(),
            dir: Path::new(self.root()).join(created["path"].as_str().unwrap()),
        }
    }

    /// `agent` records the files of `work` and sends them for review; returns the new proposal id.
    pub async fn propose(&self, agent: &str, work: &Work, title: &str) -> String {
        let recorded = self
            .call(
                agent,
                "record_changes",
                json!({"ws": work.workspace, "message": title}),
            )
            .await;
        assert!(recorded["changeset"].as_str().is_some(), "{recorded}");
        let created = self
            .call(
                agent,
                "create_proposal",
                json!({"task": work.task, "title": title, "evidence": "tests pass"}),
            )
            .await;
        assert_eq!(created["status"], "review", "{created}");
        created["id"].as_str().unwrap().to_owned()
    }

    /// The current version of `proposal` as `agent` reads it with `get_proposal`.
    pub async fn version_of(&self, agent: &str, proposal: &str) -> u32 {
        let read = self
            .call(agent, "get_proposal", json!({"id": proposal}))
            .await;
        let current = read["current_version"].as_str().unwrap();
        current.strip_prefix('v').unwrap().parse().unwrap()
    }

    /// `review` reads `proposal` and approves the version it read.
    pub async fn approve(&self, proposal: &str) {
        let version = self.version_of("review", proposal).await;
        let submitted = self
            .call(
                "review",
                "submit_review",
                json!({"id": proposal, "version": version, "verdict": "approve", "body": "Looks right."}),
            )
            .await;
        assert_eq!(submitted["verdict"], "approve");
    }

    /// The `error` object of a tool call that must fail.
    pub async fn refused(&self, agent: &str, tool: &str, arguments: Value) -> Value {
        let result = self.call_raw(agent, tool, arguments).await;
        assert_eq!(
            result.is_error,
            Some(true),
            "{agent} {tool} succeeded: {result:?}"
        );
        body(&result)["error"].clone()
    }
}

/// One task an agent works on: its id, its workspace and the workspace directory.
pub struct Work {
    pub task: String,
    pub workspace: String,
    pub dir: PathBuf,
}

impl Work {
    /// Writes each `(path, content)` into the workspace directory.
    pub fn write(&self, files: &[(&str, &str)]) {
        for (path, content) in files {
            let target = self.dir.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, content).unwrap();
        }
    }
}

/// `type`, `actor`, task number, proposal number and `data` of one event.
type EventRow = (String, String, Option<i64>, Option<i64>, String);

/// One row of the `events` table with public ids.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub kind: String,
    pub actor: String,
    pub task: Option<String>,
    pub proposal: Option<String>,
    pub data: Value,
}

/// Panics unless the event types `wanted` appear in `events` in this order; other events may come between them.
pub fn assert_in_order(events: &[StoredEvent], wanted: &[&str]) {
    let mut rest = events.iter();
    for kind in wanted {
        assert!(
            rest.any(|event| event.kind == *kind),
            "{kind} is missing or out of order in {:?}",
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>()
        );
    }
}

/// Every file under `dir` by relative path, with its bytes.
pub fn read_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            // Git's and fold's data are never part of a tree.
            if path
                .file_name()
                .is_some_and(|name| name == ".git" || name == ".fold")
            {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, into);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                into.insert(relative.into_owned(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

async fn open(
    project: &TestProject,
    row: &ProjectRow,
    launcher: &Launcher,
) -> (AppState, Arc<ProjectHandle>) {
    let clock = Arc::new(project.clock().clone());
    let state: AppState = Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default()
            .with_launcher(launcher.clone())
            .with_registry(project.registry().clone()),
        clock: clock.clone(),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1:0".parse().unwrap(),
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

    /// The bytes of a connection that has already arrived, if any.
    pub fn try_next(&mut self) -> Option<String> {
        self.received.try_recv().ok()
    }
}

/// The boundary of every multipart form a test sends.
pub const FORM_BOUNDARY: &str = "fold-test-boundary";

/// One part of a multipart form: its name, its file name, its content type and its bytes.
pub type FormPart<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a [u8]);

/// The body of a multipart form of `parts`, delimited by [`FORM_BOUNDARY`].
pub fn multipart_form(parts: &[FormPart<'_>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, file, media_type, content) in parts {
        bytes.extend_from_slice(format!("--{FORM_BOUNDARY}\r\n").as_bytes());
        let mut disposition = format!("Content-Disposition: form-data; name=\"{name}\"");
        if let Some(file) = file {
            disposition.push_str(&format!("; filename=\"{file}\""));
        }
        bytes.extend_from_slice(format!("{disposition}\r\n").as_bytes());
        if let Some(media_type) = media_type {
            bytes.extend_from_slice(format!("Content-Type: {media_type}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(format!("--{FORM_BOUNDARY}--\r\n").as_bytes());
    bytes
}

/// The bytes a session's inbox receives for one notice: the auth line, then the notice in fold's cross-session envelope.
pub fn wire(token: &str, text: &str) -> String {
    format!(
        "{{\"type\":\"auth\",\"token\":\"{token}\"}}\n{{\"type\":\"user\",\"priority\":\"next\",\"message\":{{\"role\":\"user\",\"content\":\"<cross-session-message from-name=\\\"fold\\\">\\n{text}\\n</cross-session-message>\"}}}}\n"
    )
}

/// The notice text for a newly opened proposal.
pub fn proposal_notice(proposal: &str, task: &str) -> String {
    format!("fold: {proposal} opened for {task}. Call start_worker.")
}

/// The notice text a lead gets when a task of its area is ready and nobody holds it.
pub fn ready_notice(task: &str) -> String {
    format!("fold: {task} is ready in your area. Call start_worker.")
}

/// The notice text for a new message.
pub fn message_notice(message: &str) -> String {
    format!("fold: message {message} arrived. Call get_inbox.")
}

/// Copies every file and folder of `from` into `to`, which it creates.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

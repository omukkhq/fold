#![cfg(test)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::events::types;
use fold_core::projects::notify::{OFF_AFTER_FAILURES, sign};
use fold_core::{
    Actor, Address, AgentActor, AgentName, EventActor, EventType, HumanName, MessageBody, NewEvent,
    NotifyTargetId, TaskDescription, TaskId, TaskTitle, WorkerName,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::jobs::spawn_notify;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::projects::notify::{
    NotifySettings, PassState, Scope, TryOutcome, notify_pass,
};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;

const WAIT: Duration = Duration::from_secs(10);
const BODY: &str = "BODY-TEXT-that-never-leaves-fold";
const WEB: &str = "https://fold.example.com";
const HUMAN: &str = "tanim";

fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

/// One post a receiver got.
#[derive(Debug)]
pub struct Post {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Post {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    /// The types of the notices of a generic send.
    pub fn types(&self) -> Vec<String> {
        self.json()["data"]["notices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|notice| notice["type"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// A receiver on a local port that answers each post with the next status of its script, then 200.
pub struct Receiver {
    pub url: String,
    posts: mpsc::UnboundedReceiver<Post>,
}

impl Receiver {
    pub async fn start(statuses: &[u16]) -> Self {
        Self::start_at("/hooks/fold-token", statuses).await
    }

    pub async fn start_at(path: &str, statuses: &[u16]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{path}", listener.local_addr().unwrap());
        let (sender, posts) = mpsc::unbounded_channel();
        let script = Arc::new(Mutex::new(
            statuses.iter().copied().collect::<VecDeque<_>>(),
        ));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let sender = sender.clone();
                let status = script.lock().unwrap().pop_front().unwrap_or(200);
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
                    let mut headers = Vec::new();
                    loop {
                        line.clear();
                        reader.read_line(&mut line).await.unwrap();
                        let header = line.trim_end();
                        if header.is_empty() {
                            break;
                        }
                        let (key, value) = header.split_once(':').unwrap();
                        headers.push((key.trim().to_owned(), value.trim().to_owned()));
                    }
                    let length = headers
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map_or(0, |(_, value)| value.parse().unwrap());
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).await.unwrap();
                    let _ = sender.send(Post {
                        path,
                        headers,
                        body,
                    });
                    let extra = if status == 503 {
                        "retry-after: 30\r\n"
                    } else {
                        ""
                    };
                    let answer = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: 0\r\n{extra}connection: close\r\n\r\n"
                    );
                    reader.get_mut().write_all(answer.as_bytes()).await.unwrap();
                });
            }
        });
        Self { url, posts }
    }

    pub async fn next(&mut self) -> Post {
        tokio::time::timeout(WAIT, self.posts.recv())
            .await
            .expect("a post in time")
            .unwrap()
    }

    pub fn none_waiting(&mut self) -> bool {
        self.posts.try_recv().is_err()
    }
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Project `demo` with agent `frontend`, human `tanim`, task `T-1` and notice settings in a folder of its own.
pub struct Fixture {
    pub project: TestProject,
    pub state: AppState,
    pub handle: Arc<ProjectHandle>,
    pub settings: NotifySettings,
    pub pass_state: PassState,
    data: TempDir,
}

impl Fixture {
    async fn open(project: &TestProject, data: &TempDir) -> (AppState, Arc<ProjectHandle>) {
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default().with_notify_dir(data.path().join("notices")),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1:7420".parse().unwrap(),
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        (state, handle)
    }

    /// Closes the project as a stopping daemon does and opens it again in a new daemon state.
    pub async fn restart(&mut self) {
        self.handle.bus.close();
        self.handle.store.close().await.unwrap();
        let (state, handle) = Self::open(&self.project, &self.data).await;
        self.settings = state.projects.notify().unwrap().clone();
        self.state = state;
        self.handle = handle;
        self.pass_state = PassState::default();
    }

    pub async fn new() -> Self {
        let project = TestProject::new().await;
        let data = tempfile::tempdir().unwrap();
        let (state, handle) = Self::open(&project, &data).await;
        fold_daemon::services::projects::ensure_human(
            project.registry(),
            project.clock(),
            &HumanName::parse(HUMAN).unwrap(),
        )
        .await
        .unwrap();
        let agents = AgentService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        let mut ctx = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut ctx, &AgentName::parse("frontend").unwrap(), &[])
            .await
            .unwrap();
        TaskService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
            5,
        )
        .create_task(
            &agent("frontend"),
            CreateTask {
                title: TaskTitle::parse("Add PIN sign-in").unwrap(),
                description: TaskDescription::new(""),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
        let settings = state.projects.notify().unwrap().clone();
        Self {
            project,
            state,
            handle,
            settings,
            pass_state: PassState::default(),
            data,
        }
    }

    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let response = crate::common::signed_in_router(&self.state, HUMAN)
            .await
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    /// Adds a target of `kind` to the project, or to the host with `host`, with the web address of links set, and returns its id and secret.
    pub async fn target(
        &self,
        kind: &str,
        url: &str,
        host: bool,
        extra: Value,
    ) -> (String, Option<String>) {
        let (status, body) = self
            .call(Method::PUT, "/api/v1/notify", Some(json!({"web_url": WEB})))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let path = if host {
            "/api/v1/notify/targets"
        } else {
            "/api/v1/projects/demo/notify/targets"
        };
        let mut request = json!({"kind": kind, "name": format!("{kind} target"), "url": url});
        for (key, value) in extra.as_object().unwrap() {
            request[key] = value.clone();
        }
        let (status, body) = self.call(Method::POST, path, Some(request)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        (
            body["target"]["id"].as_str().unwrap().to_owned(),
            body["secret"].as_str().map(str::to_owned),
        )
    }

    /// Appends a stored event about `T-1`, as a service's write does.
    pub async fn emit(&self, kind: EventType, data: Value) {
        let event = NewEvent::new(kind, EventActor::fold())
            .unwrap()
            .task(TaskId::from_number(1))
            .data(data)
            .unwrap();
        let now = self.handle.clock.now();
        self.handle
            .store
            .write(async |tx| fold_store::events::append(tx, &event, now).await)
            .await
            .unwrap();
    }

    pub async fn moved(&self, from: &str, to: &str) {
        self.emit(types::TASK_STATUS_CHANGED, json!({"from": from, "to": to}))
            .await;
    }

    pub async fn pass(&mut self) -> u64 {
        self.try_pass().await.unwrap()
    }

    pub async fn try_pass(&mut self) -> Result<u64, fold_core::FoldError> {
        notify_pass(&self.handle, &self.settings, &mut self.pass_state)
            .await
            .map(|pass| pass.sent)
    }

    pub fn advance(&self, by: Duration) {
        self.project.clock().advance(by).unwrap();
    }

    /// Sends an ask from `frontend` to the humans and returns its id.
    pub async fn ask(&self) -> String {
        MessageService::new(
            self.handle.store.clone(),
            self.state.registry.clone(),
            self.handle.bus.clone(),
            self.handle.clock.clone(),
        )
        .send_marked(
            &agent("frontend"),
            SendMessage {
                to: Address::parse_all(&["humans"]).unwrap(),
                body: MessageBody::parse(BODY).unwrap(),
                task: None,
                proposal: None,
                reply_to: None,
            },
            None,
            None,
            true,
        )
        .await
        .unwrap()
        .id
        .to_string()
    }

    pub async fn reply(&self, to: &str) {
        let (status, body) = self
            .call(
                Method::POST,
                "/api/v1/projects/demo/feedback",
                Some(json!({"body": "the first", "reply_to": to})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
}

/// The signature a receiver computes from what it got.
fn signed(secret: &str, post: &Post) -> String {
    sign(
        secret,
        post.header("webhook-id").unwrap(),
        post.header("webhook-timestamp").unwrap().parse().unwrap(),
        &post.body,
    )
    .unwrap()
}

#[tokio::test]
async fn a_card_move_to_review_sends_one_signed_generic_notice() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    let (target, secret) = fx.target("webhook", &receiver.url, false, json!({})).await;
    let secret = secret.unwrap();
    fx.moved("in_progress", "review").await;
    assert_eq!(fx.pass().await, 0, "the window holds the notice");
    fx.advance(Duration::from_secs(15));
    assert_eq!(fx.pass().await, 1);
    let post = receiver.next().await;
    assert_eq!(post.path, "/hooks/fold-token");
    assert_eq!(post.header("content-type"), Some("application/json"));
    assert!(post.header("user-agent").unwrap().starts_with("fold/"));
    let webhook_id = post.header("webhook-id").unwrap();
    assert!(webhook_id.starts_with("demo-"), "{webhook_id}");
    assert_eq!(
        post.header("webhook-signature").unwrap(),
        signed(&secret, &post)
    );
    let timestamp: i64 = post.header("webhook-timestamp").unwrap().parse().unwrap();
    assert_eq!(timestamp, fx.handle.clock.now().as_millis() / 1000);
    let body = post.json();
    assert_eq!(body["type"], "fold.notices");
    assert_eq!(body["data"]["project"]["slug"], "demo");
    assert_eq!(
        body["data"]["project"]["url"],
        format!("{WEB}/p/demo/board")
    );
    let notice = &body["data"]["notices"][0];
    assert_eq!(notice["type"], "task.moved");
    assert_eq!(
        (notice["from"].as_str(), notice["to"].as_str()),
        (Some("in_progress"), Some("review"))
    );
    assert_eq!(notice["task"]["id"], "T-1");
    assert_eq!(notice["task"]["title"], "Add PIN sign-in");
    assert_eq!(notice["task"]["url"], format!("{WEB}/p/demo/t/T-1"));
    assert!(notice["seq"].as_u64().unwrap() > 0);
    assert!(receiver.none_waiting());
    assert_eq!(fx.pass().await, 0, "a delivered send never goes again");
    let (status, log) = fx
        .call(
            Method::GET,
            &format!("/api/v1/projects/demo/notify/targets/{target}/sends"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{log}");
    assert_eq!(log["sends"][0]["state"], "delivered");
    assert_eq!(log["sends"][0]["status"], 200);
    assert_eq!(log["sends"][0]["notices"], json!(["task.moved T-1"]));
    let (_, shown) = fx
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    let text = shown.to_string();
    assert!(
        !text.contains(&secret) && !text.contains("fold-token"),
        "{text}"
    );
    assert_eq!(shown["targets"][0]["state"], "on");
    assert_eq!(shown["targets"][0]["preset"], "recommended");
    let logs = captured.text();
    assert!(logs.contains(&target), "{logs}");
    assert!(
        !logs.contains("fold-token") && !logs.contains(&secret),
        "{logs}"
    );
}

#[tokio::test]
async fn a_burst_in_the_window_is_one_send_and_a_window_of_zero_sends_each_alone() {
    let mut fx = Fixture::new().await;
    let mut grouped = Receiver::start(&[]).await;
    let mut alone = Receiver::start(&[]).await;
    fx.target(
        "webhook",
        &grouped.url,
        false,
        json!({"moves_to": ["review", "done", "in_progress"]}),
    )
    .await;
    fx.target(
        "webhook",
        &alone.url,
        false,
        json!({"window_seconds": 0, "moves_to": ["review", "done", "in_progress"]}),
    )
    .await;
    fx.moved("todo", "in_progress").await;
    fx.advance(Duration::from_secs(5));
    fx.moved("in_progress", "review").await;
    fx.advance(Duration::from_secs(5));
    fx.moved("review", "done").await;
    assert_eq!(fx.pass().await, 3, "the window-0 target sends each alone");
    for to in ["in_progress", "review", "done"] {
        assert_eq!(alone.next().await.json()["data"]["notices"][0]["to"], to);
    }
    assert!(grouped.none_waiting());
    fx.advance(Duration::from_secs(5));
    assert_eq!(fx.pass().await, 1);
    let post = grouped.next().await;
    assert_eq!(post.types(), ["task.moved", "task.moved", "task.moved"]);
    let seqs: Vec<u64> = post.json()["data"]["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|notice| notice["seq"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    assert!(grouped.none_waiting() && alone.none_waiting());
}

#[tokio::test]
async fn a_target_hears_only_the_columns_and_events_it_chose() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    fx.target(
        "webhook",
        &receiver.url,
        false,
        json!({"window_seconds": 0}),
    )
    .await;
    fx.moved("todo", "in_progress").await;
    fx.emit(
        types::CHECKS_FINISHED,
        json!({"state": "passed", "stage": "test"}),
    )
    .await;
    fx.emit(types::TASK_CREATED, json!({})).await;
    fx.moved("in_progress", "done").await;
    assert_eq!(fx.pass().await, 1);
    let post = receiver.next().await;
    assert_eq!(post.json()["data"]["notices"][0]["to"], "done");
    assert!(receiver.none_waiting());
}

#[tokio::test]
async fn slack_and_discord_get_one_message_of_lines() {
    let mut fx = Fixture::new().await;
    let mut slack = Receiver::start_at("/services/T0/B0/token", &[]).await;
    let mut discord = Receiver::start_at("/api/webhooks/12/token", &[]).await;
    let (_, none) = fx.target("slack", &slack.url, false, json!({})).await;
    assert!(none.is_none());
    fx.target("discord", &discord.url, true, json!({})).await;
    fx.moved("in_progress", "review").await;
    fx.emit(
        types::CHECKS_FINISHED,
        json!({"state": "failed", "stage": "test", "version": 2}),
    )
    .await;
    fx.advance(Duration::from_secs(15));
    assert_eq!(fx.pass().await, 2);
    let post = slack.next().await;
    assert!(post.header("webhook-signature").is_none());
    let text = post.json()["text"].as_str().unwrap().to_owned();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            format!(
                "<{WEB}/p/demo/board|Demo> · <{WEB}/p/demo/t/T-1|T-1> Add PIN sign-in moved In progress → Review"
            ),
            format!(
                "<{WEB}/p/demo/board|Demo> · Check failed: <{WEB}/p/demo/t/T-1|T-1> Add PIN sign-in v2, stage test"
            ),
        ]
    );
    let post = discord.next().await;
    assert_eq!(post.path, "/api/webhooks/12/token?wait=true");
    let body = post.json();
    assert_eq!(body["allowed_mentions"]["parse"], json!([]));
    assert_eq!(body["content"].as_str().unwrap().lines().count(), 2);
    assert!(
        body["content"]
            .as_str()
            .unwrap()
            .contains(&format!("[T-1](<{WEB}/p/demo/t/T-1>)"))
    );
}

#[tokio::test]
async fn a_failed_send_tries_again_with_the_same_webhook_id() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[503, 500]).await;
    let (target, secret) = fx
        .target(
            "webhook",
            &receiver.url,
            false,
            json!({"window_seconds": 0}),
        )
        .await;
    fx.moved("in_progress", "review").await;
    assert_eq!(fx.pass().await, 0);
    let first = receiver.next().await;
    let id = first.header("webhook-id").unwrap().to_owned();
    fx.advance(Duration::from_secs(10));
    assert_eq!(
        fx.pass().await,
        0,
        "Retry-After: 30 holds the try past the schedule's 10 s"
    );
    assert!(receiver.none_waiting());
    fx.advance(Duration::from_secs(20));
    assert_eq!(fx.pass().await, 0);
    let second = receiver.next().await;
    assert_eq!(second.header("webhook-id").unwrap(), id);
    assert_eq!(second.body, first.body, "a retry sends the same body");
    assert_eq!(
        second.header("webhook-signature").unwrap(),
        signed(secret.as_deref().unwrap(), &second)
    );
    let (_, shown) = fx
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert_eq!(shown["targets"][0]["failures"], 2);
    assert!(shown["targets"][0]["next_try_at"].is_i64());
    fx.advance(Duration::from_secs(60));
    assert_eq!(fx.pass().await, 1);
    assert_eq!(receiver.next().await.header("webhook-id").unwrap(), id);
    let (_, log) = fx
        .call(
            Method::GET,
            &format!("/api/v1/projects/demo/notify/targets/{target}/sends"),
            None,
        )
        .await;
    assert_eq!(
        (
            log["sends"][0]["tries"].as_u64(),
            log["sends"][0]["status"].as_u64()
        ),
        (Some(3), Some(200))
    );
    let send = log["sends"][0]["id"].as_str().unwrap().to_owned();
    let (status, again) = fx
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/sends/{send}/again"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(fx.pass().await, 1);
    assert_eq!(
        receiver.next().await.header("webhook-id").unwrap(),
        id,
        "Send again keeps the webhook id"
    );
    let (status, refused) = fx
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/sends/ns-99/again",
            None,
        )
        .await;
    assert_eq!(
        (status, refused["error"]["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("notify_send_not_found"))
    );
}

#[tokio::test]
async fn a_failed_test_is_one_try_that_never_goes_again() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[503]).await;
    let (target, _) = fx.target("webhook", &receiver.url, false, json!({})).await;
    let (status, body) = fx
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/targets/{target}/test"),
            None,
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::BAD_GATEWAY, Some("notify_send_failed"))
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("HTTP 503")
    );
    receiver.next().await;
    let (_, log) = fx
        .call(
            Method::GET,
            &format!("/api/v1/projects/demo/notify/targets/{target}/sends"),
            None,
        )
        .await;
    assert_eq!(log["sends"][0]["state"], "given_up");
    assert_eq!(log["sends"][0]["tries"], 1);
    assert_eq!(log["sends"][0]["test"], true);
    fx.advance(Duration::from_secs(60));
    assert_eq!(fx.pass().await, 0);
    assert!(receiver.none_waiting());
}

#[tokio::test]
async fn a_gone_receiver_turns_its_target_off_at_once() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[410]).await;
    fx.target(
        "webhook",
        &receiver.url,
        false,
        json!({"window_seconds": 0}),
    )
    .await;
    fx.moved("in_progress", "review").await;
    fx.moved("review", "done").await;
    fx.pass().await;
    receiver.next().await;
    let (_, shown) = fx
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert_eq!(shown["targets"][0]["state"], "gone");
    fx.moved("in_progress", "review").await;
    fx.advance(Duration::from_secs(3600));
    assert_eq!(fx.pass().await, 0);
    assert!(receiver.none_waiting(), "an off target gets nothing more");
    let (_, needs) = fx
        .call(Method::GET, "/api/v1/projects/demo/needs-you", None)
        .await;
    assert!(needs["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn twenty_failures_in_a_row_turn_a_target_off_and_into_needs_you() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[500]).await;
    let mut other = Receiver::start(&[]).await;
    let (target, _) = fx
        .target("webhook", &receiver.url, true, json!({"window_seconds": 0}))
        .await;
    fx.target(
        "slack",
        &other.url,
        false,
        json!({"window_seconds": 0, "preset": "needs_you_only"}),
    )
    .await;
    let id: NotifyTargetId = target.parse().unwrap();
    let now = fx.handle.clock.now();
    for _ in 1..OFF_AFTER_FAILURES {
        fx.settings
            .record_try("demo", id, true, TryOutcome::Failed, now)
            .await
            .unwrap();
    }
    fx.moved("in_progress", "review").await;
    fx.pass().await;
    receiver.next().await;
    let (_, shown) = fx
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert_eq!(shown["host_targets"][0]["state"], "failing");
    let (_, needs) = fx
        .call(Method::GET, "/api/v1/projects/demo/needs-you", None)
        .await;
    let item = &needs["items"][0];
    assert_eq!(
        (item["kind"].as_str(), item["screen"].as_str()),
        (Some("notify_target_off"), Some("settings"))
    );
    assert_eq!(item["target"], target);
    fx.pass().await;
    let notice = other.next().await.json();
    assert!(
        notice["text"]
            .as_str()
            .unwrap()
            .contains("fold turned the notice target webhook target off"),
        "{notice}"
    );
    let (status, tested) = fx
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/targets/{target}/test"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tested}");
    assert_eq!(receiver.next().await.types(), ["webhook.test"]);
    let (_, shown) = fx
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert_eq!(shown["host_targets"][0]["state"], "on");
    let (_, needs) = fx
        .call(Method::GET, "/api/v1/projects/demo/needs-you", None)
        .await;
    assert!(needs["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_restart_sends_nothing_twice_and_loses_nothing() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    fx.target("webhook", &receiver.url, false, json!({})).await;
    fx.moved("in_progress", "review").await;
    fx.pass().await;
    fx.advance(Duration::from_secs(15));
    assert_eq!(fx.pass().await, 1);
    receiver.next().await;
    fx.moved("review", "done").await;
    fx.pass().await;
    fx.restart().await;
    fx.moved("in_progress", "review").await;
    assert_eq!(
        fx.pass().await,
        0,
        "the window of the waiting notice still runs"
    );
    fx.advance(Duration::from_secs(15));
    assert_eq!(fx.pass().await, 1);
    let post = receiver.next().await;
    let tos: Vec<String> = post.json()["data"]["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|notice| notice["to"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tos, ["done", "review"]);
    fx.restart().await;
    fx.advance(Duration::from_secs(3600));
    assert_eq!(fx.pass().await, 0);
    assert!(receiver.none_waiting());
}

#[tokio::test]
async fn needs_you_items_open_and_an_item_closed_inside_the_window_sends_nothing() {
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    let (_, secret) = fx
        .target(
            "webhook",
            &receiver.url,
            false,
            json!({"events": ["needs_you.opened", "needs_you.closed"]}),
        )
        .await;
    let quick = fx.ask().await;
    fx.pass().await;
    fx.reply(&quick).await;
    fx.advance(Duration::from_secs(15));
    assert_eq!(
        fx.pass().await,
        0,
        "an item that came and went inside one window sends nothing"
    );
    assert!(receiver.none_waiting());
    let asked = fx.ask().await;
    fx.pass().await;
    fx.advance(Duration::from_secs(15));
    assert_eq!(fx.pass().await, 1);
    let post = receiver.next().await;
    let notice = &post.json()["data"]["notices"][0];
    assert_eq!(notice["type"], "needs_you.opened");
    assert_eq!(
        (notice["kind"].as_str(), notice["item"].as_str()),
        (Some("question"), Some(asked.as_str()))
    );
    assert_eq!(notice["title"], "@frontend asks a question");
    assert!(
        notice["url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{WEB}/p/demo/messages?thread="))
    );
    let raw = String::from_utf8(post.body.clone()).unwrap();
    assert!(
        !raw.contains(BODY) && !raw.contains(secret.as_deref().unwrap()),
        "{raw}"
    );
    fx.reply(&asked).await;
    fx.pass().await;
    fx.advance(Duration::from_secs(15));
    assert_eq!(fx.pass().await, 1);
    let closed = receiver.next().await.json();
    assert_eq!(closed["data"]["notices"][0]["type"], "needs_you.closed");
    assert_eq!(closed["data"]["notices"][0]["item"], asked);
}

#[tokio::test]
async fn plain_text_targets_off_this_machine_and_agents_are_refused() {
    let fx = Fixture::new().await;
    let (status, body) = fx
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(json!({"kind": "webhook", "name": "x", "url": "http://hooks.example.com/x"})),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("plaintext_notify_target"))
    );
    assert!(!body.to_string().contains("hooks.example.com/x"));
    let (status, body) = fx
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(json!({"kind": "webhook", "name": "x", "url": "https://h/x", "preset": "all", "events": ["task.moved"]})),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_notify_target"))
    );
    let error = fold_daemon::services::projects::notify::add_target(
        &fx.settings,
        &agent("frontend"),
        &Scope::Project("demo".to_owned()),
        fold_core::projects::notify::TargetKind::Webhook,
        Default::default(),
        fx.handle.clock.now(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), fold_core::codes::HUMAN_REQUIRED);
    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri("/api/v1/projects/demo/notify/targets")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"kind": "slack", "name": "x", "url": "https://h/x"}).to_string(),
        ))
        .unwrap();
    let response = fold_daemon::http::router(fx.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a request without a session changes nothing"
    );
    let (_, shown) = fx
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert!(shown["targets"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_project_that_turns_the_host_targets_off_sends_only_to_its_own() {
    let mut fx = Fixture::new().await;
    let mut host = Receiver::start(&[]).await;
    let mut own = Receiver::start(&[]).await;
    fx.target("webhook", &host.url, true, json!({"window_seconds": 0}))
        .await;
    fx.target("webhook", &own.url, false, json!({"window_seconds": 0}))
        .await;
    let (status, body) = fx
        .call(
            Method::PUT,
            "/api/v1/projects/demo/notify",
            Some(json!({"use_host_targets": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["use_host_targets"], false);
    fx.moved("in_progress", "review").await;
    assert_eq!(fx.pass().await, 1);
    own.next().await;
    assert!(host.none_waiting());
}

#[tokio::test]
async fn the_job_sends_after_the_window_by_the_project_clock() {
    let fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    fx.target("webhook", &receiver.url, false, json!({})).await;
    let (_stop, stopped) = watch::channel(false);
    let job = spawn_notify(fx.state.clone(), Duration::from_millis(10), stopped);
    let mut progress = job.progress();
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks >= 1))
        .await
        .unwrap()
        .unwrap();
    fx.moved("in_progress", "review").await;
    let checks = progress.borrow().checks;
    fx.settings.wake(Some("demo"));
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks > checks))
        .await
        .unwrap()
        .unwrap();
    assert!(receiver.none_waiting());
    fx.advance(Duration::from_secs(15));
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.cycles >= 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receiver.next().await.types(), ["task.moved"]);
}

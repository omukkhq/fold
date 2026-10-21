#![cfg(test)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::projects::notify::{SETTLE, sign};
use fold_core::{
    Actor, Address, AgentActor, AgentName, HumanName, MessageBody, TaskDescription, TaskTitle,
    WorkerName,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::router;
use fold_daemon::jobs::spawn_notify;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::projects::notify::{
    NewChannel, NewTarget, NotifySettings, Scope, notify_pass,
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
const SECRET: &str = "s3cret-signing-key-0123456789";
const BODY: &str = "BODY-TEXT-that-never-leaves-fold";
const WEB: &str = "https://fold.example.com";

fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

/// One post a webhook receiver got.
#[derive(Debug)]
struct Post {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Post {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A webhook receiver on a local port that answers every post with `status`.
struct Receiver {
    url: String,
    posts: mpsc::UnboundedReceiver<Post>,
}

impl Receiver {
    async fn start(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hooks/fold-token", listener.local_addr().unwrap());
        let (sender, posts) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let sender = sender.clone();
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
                    let answer = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    reader.get_mut().write_all(answer.as_bytes()).await.unwrap();
                });
            }
        });
        Self { url, posts }
    }

    async fn next(&mut self) -> Post {
        tokio::time::timeout(WAIT, self.posts.recv())
            .await
            .expect("a post in time")
            .unwrap()
    }

    fn none_waiting(&mut self) -> bool {
        self.posts.try_recv().is_err()
    }
}

/// An SMTP server on a local port that takes every message without TLS and hands back the text of each.
async fn smtp_server() -> (u16, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, messages) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let sender = sender.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                reader.get_mut().write_all(b"220 fake\r\n").await.unwrap();
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    let command = line.trim_end().to_ascii_uppercase();
                    let answer: &[u8] = if command.starts_with("DATA") {
                        reader.get_mut().write_all(b"354 go\r\n").await.unwrap();
                        let mut text = String::new();
                        loop {
                            line.clear();
                            reader.read_line(&mut line).await.unwrap();
                            if line == ".\r\n" {
                                break;
                            }
                            text.push_str(&line);
                        }
                        let _ = sender.send(text);
                        b"250 queued\r\n"
                    } else if command.starts_with("QUIT") {
                        reader.get_mut().write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        b"250 fine\r\n"
                    };
                    reader.get_mut().write_all(answer).await.unwrap();
                }
            });
        }
    });
    (port, messages)
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

/// Project `demo` with agent `frontend`, human `tanim`, task `T-1` and notification settings in a folder of its own.
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
    settings: NotifySettings,
    messages: MessageService,
    _data: TempDir,
}

impl Fixture {
    /// A daemon state that opens the project with the notification settings in `data`, as a start of the daemon does.
    async fn open(project: &TestProject, data: &TempDir) -> (AppState, Arc<ProjectHandle>) {
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default().with_notify_dir(data.path().join("notify")),
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
    async fn restart(&mut self) {
        self.handle.bus.close();
        self.handle.store.close().await.unwrap();
        let (state, handle) = Self::open(&self.project, &self._data).await;
        self.messages = MessageService::new(
            handle.store.clone(),
            state.registry.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        self.settings = state.projects.notify().unwrap().clone();
        self.state = state;
        self.handle = handle;
    }

    async fn new() -> Self {
        let project = TestProject::new().await;
        let data = tempfile::tempdir().unwrap();
        let (state, handle) = Self::open(&project, &data).await;
        let now = project.now();
        let tanim = HumanName::parse("tanim").unwrap();
        project
            .registry()
            .write(async |tx| fold_store::projects::insert_human(tx, &tanim, now).await)
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
                title: TaskTitle::parse("task").unwrap(),
                description: TaskDescription::new(""),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
        let messages = MessageService::new(
            handle.store.clone(),
            state.registry.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        let settings = state.projects.notify().unwrap().clone();
        Self {
            project,
            state,
            handle,
            settings,
            messages,
            _data: data,
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(method)
            .uri(uri)
            .header("cookie", "fold_human=tanim")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    /// Adds a webhook target to the project, or to the host with `host`, and sets the web address of links.
    async fn webhook(&self, url: &str, host: bool, interval_minutes: u32) -> String {
        let (status, body) = self
            .call(Method::PUT, "/api/v1/notify", Some(json!({"web_url": WEB})))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let path = if host {
            "/api/v1/notify/targets"
        } else {
            "/api/v1/projects/demo/notify/targets"
        };
        let (status, body) = self
            .call(
                Method::POST,
                path,
                Some(json!({"kind": "webhook", "url": url, "secret": SECRET, "interval_minutes": interval_minutes})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_str().unwrap().to_owned()
    }

    /// Sends an ask from `frontend` to the humans and returns its id.
    async fn ask(&self) -> String {
        self.messages
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

    async fn reply(&self, to: &str) {
        let (status, body) = self
            .call(
                Method::POST,
                "/api/v1/projects/demo/feedback",
                Some(json!({"body": "the first", "reply_to": to})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    async fn pass(&self) -> u64 {
        notify_pass(&self.handle, &self.settings, self.state.listen)
            .await
            .unwrap()
            .sent
    }

    fn advance(&self, by: Duration) {
        self.project.clock().advance(by).unwrap();
    }
}

#[tokio::test]
async fn an_item_into_needs_you_sends_one_signed_webhook_without_its_body() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let fixture = Fixture::new().await;
    let mut receiver = Receiver::start(200).await;
    let target = fixture.webhook(&receiver.url, false, 10).await;
    let (stop, stopped) = watch::channel(false);
    let job = spawn_notify(fixture.state.clone(), Duration::from_millis(10), stopped);
    let mut progress = job.progress();
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks >= 1))
        .await
        .unwrap()
        .unwrap();
    let asked = fixture.ask().await;
    let checks = progress.borrow().checks;
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks > checks))
        .await
        .unwrap()
        .unwrap();
    assert!(receiver.none_waiting(), "the item waits the settle time");
    fixture.advance(SETTLE);
    let post = receiver.next().await;
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.cycles >= 1))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(post.path, "/hooks/fold-token");
    assert_eq!(post.header("x-fold-event"), Some("needs_you"));
    assert_eq!(post.header("content-type"), Some("application/json"));
    assert_eq!(
        post.header("x-fold-signature-256").unwrap(),
        sign(SECRET.as_bytes(), &post.body).unwrap()
    );
    let body = post.json();
    assert_eq!(body["event"], "needs_you");
    assert_eq!(
        body["project"],
        json!({"slug": "demo", "name": "Demo", "link": "https://fold.example.com/p/demo/board"})
    );
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "question");
    assert_eq!(items[0]["id"], asked);
    assert_eq!(items[0]["title"], "frontend asks a question");
    assert_eq!(
        items[0]["link"],
        format!("https://fold.example.com/p/demo/messages?thread={asked}")
    );
    let text = String::from_utf8(post.body.clone()).unwrap();
    assert!(!text.contains(BODY) && !text.contains(SECRET), "{text}");

    fixture.advance(Duration::from_secs(3600));
    let checks = progress.borrow().checks;
    fixture.ask().await;
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks > checks))
        .await
        .unwrap()
        .unwrap();
    fixture.advance(SETTLE);
    let second = receiver.next().await;
    assert_eq!(
        second.json()["items"].as_array().unwrap().len(),
        1,
        "an open item is never sent twice"
    );

    stop.send(true).unwrap();
    job.stopped(WAIT).await;
    let logs = captured.text();
    assert!(logs.contains("a notice went out"), "{logs}");
    assert!(logs.contains(&target), "{logs}");
    for hidden in [BODY, SECRET, "fold-token"] {
        assert!(!logs.contains(hidden), "{hidden} in the logs: {logs}");
    }
}

#[tokio::test]
async fn a_burst_becomes_one_digest_and_the_interval_holds_the_next_send() {
    let fixture = Fixture::new().await;
    let mut receiver = Receiver::start(200).await;
    fixture.webhook(&receiver.url, false, 10).await;
    assert_eq!(fixture.pass().await, 0);
    let asked = [
        fixture.ask().await,
        fixture.ask().await,
        fixture.ask().await,
    ];
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 1);
    let digest = receiver.next().await.json();
    let ids: Vec<&str> = digest["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, asked.iter().map(String::as_str).collect::<Vec<_>>());

    let later = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(
        fixture.pass().await,
        0,
        "the interval after the digest holds it"
    );
    fixture.advance(Duration::from_secs(600));
    assert_eq!(fixture.pass().await, 1);
    let next = receiver.next().await.json();
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["items"][0]["id"], later);
    assert!(receiver.none_waiting());
}

#[tokio::test]
async fn an_item_closed_before_the_send_sends_nothing() {
    let fixture = Fixture::new().await;
    let mut receiver = Receiver::start(200).await;
    fixture.webhook(&receiver.url, false, 10).await;
    let asked = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.reply(&asked).await;
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(Duration::from_secs(3600));
    assert_eq!(fixture.pass().await, 0);
    assert!(receiver.none_waiting());
}

#[tokio::test]
async fn the_host_targets_serve_a_project_without_its_own_and_a_new_target_hears_only_new_items() {
    let fixture = Fixture::new().await;
    let mut host = Receiver::start(200).await;
    let mut own = Receiver::start(200).await;
    fixture.webhook(&host.url, true, 1).await;
    let (_, settings) = fixture
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert_eq!(settings["uses_host"], true);
    fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 1);
    assert_eq!(
        host.next().await.json()["items"].as_array().unwrap().len(),
        1
    );

    fixture.webhook(&own.url, false, 1).await;
    assert_eq!(
        fixture.pass().await,
        0,
        "the open item is not news to the new target"
    );
    let later = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 1);
    let post = own.next().await.json();
    assert_eq!(post["items"].as_array().unwrap().len(), 1);
    assert_eq!(post["items"][0]["id"], later);
    assert!(
        host.none_waiting(),
        "the project's own target replaces the host's"
    );
}

#[tokio::test]
async fn a_pass_without_a_target_writes_nothing_and_a_new_target_hears_only_later_items() {
    let fixture = Fixture::new().await;
    let state = async || {
        let mut conn = fixture.handle.store.read().acquire().await.unwrap();
        fold_store::projects::notify::notify_state(&mut conn)
            .await
            .unwrap()
    };
    let before = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    assert_eq!(
        state().await,
        fold_store::projects::notify::NotifyState::default(),
        "no target applies, so the pass records nothing"
    );

    fixture.advance(Duration::from_secs(1));
    let mut receiver = Receiver::start(200).await;
    let id = fixture.webhook(&receiver.url, false, 10).await;
    assert_eq!(fixture.pass().await, 0);
    let recorded = state().await;
    let target: fold_core::NotifyTargetId = id.parse().unwrap();
    assert!(
        recorded
            .sent
            .contains(&(target, format!("question:{before}"))),
        "the item open before the target counts as sent: {recorded:?}"
    );
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 0);

    let later = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 1);
    let post = receiver.next().await.json();
    assert_eq!(post["items"].as_array().unwrap().len(), 1);
    assert_eq!(post["items"][0]["id"], later);
    assert!(receiver.none_waiting());
}

#[tokio::test]
async fn a_failed_send_keeps_its_items_for_the_next_interval() {
    let fixture = Fixture::new().await;
    let mut broken = Receiver::start(500).await;
    let id = fixture.webhook(&broken.url, false, 5).await;
    fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 0);
    broken.next().await;
    assert_eq!(fixture.pass().await, 0, "the interval holds the retry");
    fixture.advance(Duration::from_secs(300));
    assert_eq!(fixture.pass().await, 0);
    assert_eq!(
        broken.next().await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (status, body) = fixture
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/targets/{id}/test"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["code"], "notify_send_failed");
    assert!(!body.to_string().contains("fold-token"), "{body}");
    let test = broken.next().await;
    assert_eq!(test.header("x-fold-event"), Some("test"));
}

#[tokio::test]
async fn a_redirect_is_a_failed_send_that_keeps_its_items() {
    let fixture = Fixture::new().await;
    let mut moved = Receiver::start(301).await;
    let id = fixture.webhook(&moved.url, false, 5).await;
    let asked = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 0, "a 301 is no delivery");
    moved.next().await;
    fixture.advance(Duration::from_secs(300));
    assert_eq!(fixture.pass().await, 0);
    assert_eq!(
        moved.next().await.json()["items"][0]["id"],
        asked,
        "the item waits for the next try"
    );
    let (status, body) = fixture
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/targets/{id}/test"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["code"], "notify_send_failed");
    assert_eq!(
        body["error"]["message"],
        "the webhook receiver answered HTTP 301"
    );
    moved.next().await;
}

#[tokio::test]
async fn an_email_target_sends_the_titles_and_links_without_the_body() {
    let fixture = Fixture::new().await;
    let (port, mut mail) = smtp_server().await;
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(json!({
                "kind": "email",
                "to": "team@example.com",
                "from": "fold@example.com",
                "smtp_host": "127.0.0.1",
                "smtp_port": port,
                "security": "none",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "email");
    assert_eq!(body["secret_set"], false);
    let asked = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 1);
    let text = tokio::time::timeout(WAIT, mail.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        text.contains("Subject: fold in Demo: frontend asks a question"),
        "{text}"
    );
    assert!(
        text.contains(&format!("Question {asked}: frontend asks a question")),
        "{text}"
    );
    assert!(
        text.contains("http://127.0.0.1:7420/p/demo/messages?thread="),
        "{text}"
    );
    assert!(!text.contains(BODY), "{text}");
}

#[tokio::test]
async fn the_routes_never_show_a_secret_and_only_a_human_changes_targets() {
    let fixture = Fixture::new().await;
    let mut receiver = Receiver::start(200).await;
    let id = fixture.webhook(&receiver.url, false, 10).await;
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(json!({
                "kind": "email",
                "to": "team@example.com",
                "from": "fold@example.com",
                "smtp_host": "smtp.example.com",
                "security": "starttls",
                "username": "fold",
                "password": "smtp-password-value",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["smtp_port"], 587);
    let (status, settings) = fixture
        .call(Method::GET, "/api/v1/projects/demo/notify", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["uses_host"], false);
    assert_eq!(settings["link_base"], WEB);
    let webhook = &settings["targets"][0];
    assert_eq!(webhook["id"], id);
    assert_eq!(webhook["secret_set"], true);
    assert_eq!(
        webhook["url"].as_str().unwrap(),
        format!("{}/…", receiver.url.trim_end_matches("/hooks/fold-token"))
    );
    let shown = settings.to_string();
    for hidden in [SECRET, "smtp-password-value", "fold-token"] {
        assert!(!shown.contains(hidden), "{hidden}: {shown}");
    }

    let (status, body) = fixture
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/targets/{id}/test"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let test = receiver.next().await;
    assert_eq!(test.header("x-fold-event"), Some("test"));
    assert_eq!(test.json()["project"]["slug"], "demo");
    assert_eq!(test.json()["items"], json!([]));

    for plain in [
        json!({"kind": "webhook", "url": "http://hooks.example.com/x", "secret": SECRET}),
        json!({
            "kind": "email",
            "to": "team@example.com",
            "from": "fold@example.com",
            "smtp_host": "smtp.example.com",
            "security": "none",
        }),
    ] {
        let (status, body) = fixture
            .call(Method::POST, "/api/v1/notify/targets", Some(plain))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], "plaintext_notify_target");
        assert!(!body.to_string().contains("hooks.example.com"), "{body}");
    }
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(json!({"kind": "webhook", "url": "ftp://x/y", "secret": SECRET})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_notify_target");
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(json!({"kind": "webhook", "url": "https://x/y", "secret": SECRET, "to": "a@b.c"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri("/api/v1/notify/targets")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"kind": "webhook", "url": "https://x/y", "secret": SECRET}).to_string(),
        ))
        .unwrap();
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a write names a human"
    );

    let (status, _) = fixture
        .call(
            Method::DELETE,
            &format!("/api/v1/projects/demo/notify/targets/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = fixture
        .call(
            Method::DELETE,
            &format!("/api/v1/projects/demo/notify/targets/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "notify_target_not_found");
    let (status, host) = fixture.call(Method::GET, "/api/v1/notify", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(host["web_url"], WEB);
    assert_eq!(host["targets"], json!([]));
    let (status, _) = fixture
        .call(
            Method::PUT,
            "/api/v1/notify",
            Some(json!({"web_url": null})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, host) = fixture.call(Method::GET, "/api/v1/notify", None).await;
    assert_eq!(host["link_base"], "http://127.0.0.1:7420");
}

#[tokio::test]
async fn a_restart_repeats_no_notice_and_keeps_what_waits() {
    let mut fixture = Fixture::new().await;
    let mut receiver = Receiver::start(200).await;
    fixture.webhook(&receiver.url, false, 10).await;
    let first = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);
    fixture.advance(SETTLE);
    assert_eq!(fixture.pass().await, 1);
    assert_eq!(receiver.next().await.json()["items"][0]["id"], first);
    let second = fixture.ask().await;
    assert_eq!(fixture.pass().await, 0);

    fixture.restart().await;
    fixture.advance(SETTLE);
    assert_eq!(
        fixture.pass().await,
        0,
        "the interval after the first send still holds"
    );
    fixture.advance(Duration::from_secs(600));
    assert_eq!(fixture.pass().await, 1);
    let post = receiver.next().await.json();
    let ids: Vec<&str> = post["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [second.as_str()], "the sent item is not sent again");
    assert!(receiver.none_waiting());
}

/// Adds a webhook target to the project `slug` straight in the settings files.
async fn project_target(settings: &NotifySettings, slug: &str) {
    settings
        .add(&Scope::Project(slug.to_owned()), |id| {
            NewTarget {
                interval_minutes: None,
                channel: NewChannel::Webhook {
                    url: "https://hooks.example.com/x".to_owned(),
                    secret: SECRET.to_owned(),
                },
            }
            .checked(id, &crate::common::ana(), fold_core::Timestamp::EPOCH)
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_project_that_leaves_fold_takes_its_targets_and_their_secrets_with_it() {
    use fold_core::{HostPaths, ManualClock};
    use fold_daemon::services::projects::{self as project_service, init_project};

    let dir = TempDir::new().unwrap();
    let host = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
    let clock = ManualClock::new();
    let settings = NotifySettings::new(host.notify_dir());
    let file = host.notify_dir().join("projects/app.json");
    let targets = async || {
        settings
            .targets(&Scope::Project("app".to_owned()))
            .await
            .unwrap()
    };
    let init = async |root: &std::path::Path| {
        std::fs::create_dir_all(root).unwrap();
        init_project(&host, root, None, &crate::common::ana(), &clock)
            .await
            .unwrap()
    };

    // A registration whose `.fold/` was deleted goes, and a new project under its slug has no targets.
    let first = dir.path().join("app");
    assert_eq!(init(&first).await.slug, "app");
    project_target(&settings, "app").await;
    project_target(&settings, "kept").await;
    assert!(file.exists());
    std::fs::remove_dir_all(first.join(".fold")).unwrap();
    assert_eq!(
        project_service::remove_deleted_on_host(&host)
            .await
            .unwrap(),
        ["app"]
    );
    assert!(
        !file.exists(),
        "the secrets file went with the registration"
    );
    let second = dir.path().join("again/app");
    assert_eq!(init(&second).await.slug, "app");
    assert!(targets().await.is_empty());
    assert_eq!(
        settings
            .targets(&Scope::Project("kept".to_owned()))
            .await
            .unwrap()
            .len(),
        1,
        "another project keeps its targets"
    );

    // An init in a folder whose `.fold/` was deleted replaces the registration and starts with no targets.
    project_target(&settings, "app").await;
    std::fs::remove_dir_all(second.join(".fold")).unwrap();
    let replaced = init(&second).await;
    assert_eq!(replaced.slug, "app");
    assert!(targets().await.is_empty());

    // A person's removal deletes the targets, and the restored project starts with none.
    let registry = fold_store::Store::open_registry(&host.registry_db())
        .await
        .unwrap();
    let state: AppState = Arc::new(DaemonState {
        projects: Projects::default()
            .with_registry(registry.clone())
            .with_notify_dir(host.notify_dir()),
        registry,
        clock: Arc::new(clock.clone()),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1:0".parse().unwrap(),
        session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
    });
    project_service::open(&state, "app").await.unwrap();
    project_target(&settings, "app").await;
    let ana = Actor::human(crate::common::ana());
    project_service::remove(&state, &ana, "app").await.unwrap();
    assert!(!file.exists());
    project_service::restore(&state, &ana, "app").await.unwrap();
    assert!(targets().await.is_empty());

    // The daemon's removal of a project whose `.fold/` was deleted deletes them too.
    project_target(&settings, "app").await;
    let row = fold_store::projects::find_project(state.registry.read(), "app")
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_dir_all(second.join(".fold")).unwrap();
    assert!(state.projects.remove_if_deleted(&row).await);
    assert!(!file.exists());

    for handle in state.projects.handles() {
        handle.bus.close();
        handle.stop_tasks(Duration::from_secs(5)).await;
        handle.store.close().await.unwrap();
    }
    state.registry.close().await.unwrap();
}

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{AgentName, EventFilter, HumanName, Tag, TaskDescription, TaskTitle, Timestamp};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{router, socket_router};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::tasks::CreateTask;
use fold_store::events;
use fold_store::projects::{self as registry, ProjectRow};
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// A bodiless write that only a named human makes; it succeeds once, because `T-1` is then archived.
const PATH: &str = "/api/v1/projects/demo/tasks/T-1/archive";

#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        Self::with_config(None).await
    }

    /// The project with `config` as its `.fold/config.toml`, written before the daemon opens it.
    async fn with_config(config: Option<&str>) -> Self {
        let project = TestProject::new().await;
        if let Some(config) = config {
            std::fs::create_dir_all(project.paths().fold_dir()).unwrap();
            std::fs::write(project.paths().config(), config).unwrap();
        }
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1".parse().unwrap(),
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        let agents = AgentService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        let mut ctx = agents.start_session(1).await.unwrap();
        agents
            .register_agent(
                &mut ctx,
                &AgentName::parse("review").unwrap(),
                &[Tag::parse("review").unwrap()],
            )
            .await
            .unwrap();
        let review = agents.actor(&ctx, None).await.unwrap();
        handle
            .tasks
            .create_task(
                &review,
                CreateTask {
                    title: TaskTitle::parse("Write the docs").unwrap(),
                    description: TaskDescription::new("what to write"),
                    labels: Vec::new(),
                },
            )
            .await
            .unwrap();
        Self {
            project,
            state,
            handle,
        }
    }

    fn http(&self) -> Router {
        router(self.state.clone())
    }

    fn socket(&self) -> Router {
        socket_router(self.state.clone())
    }

    /// The actor of every stored `task.archived` event.
    async fn archive_actors(&self) -> Vec<String> {
        events::read_after(self.handle.store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.event_type.as_str() == "task.archived")
            .map(|e| e.actor.to_string())
            .collect()
    }
}

#[cfg(test)]
async fn call(
    router: Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[cfg(test)]
fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

#[tokio::test]
async fn a_write_with_the_cookie_acts_as_that_human() {
    let fixture = Fixture::new().await;
    let (status, body) = call(
        fixture.http(),
        Method::POST,
        PATH,
        &[("cookie", "theme=dark; fold_human=tanim")],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(fixture.archive_actors().await, ["human:tanim"]);
    let human = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(human.created_at, fixture.project.now());
}

#[tokio::test]
async fn a_repeated_write_keeps_the_human() {
    let fixture = Fixture::new().await;
    let cookie = [("cookie", "fold_human=tanim")];
    call(fixture.http(), Method::POST, PATH, &cookie).await;
    let first = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap();
    fixture
        .project
        .clock()
        .set(Timestamp::from_millis(fixture.project.now().as_millis() + 5000).unwrap());
    let (status, body) = call(fixture.http(), Method::POST, PATH, &cookie).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&body), "task_closed");
    assert_eq!(fixture.archive_actors().await.len(), 1);
    let second = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(second, first);
}

#[tokio::test]
async fn the_socket_takes_the_name_from_the_header() {
    let fixture = Fixture::new().await;
    let (status, body) = call(
        fixture.socket(),
        Method::POST,
        PATH,
        &[("x-fold-human", "tanim")],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(fixture.archive_actors().await, ["human:tanim"]);
}

#[tokio::test]
async fn a_request_without_a_name_gets_human_name_required() {
    let fixture = Fixture::new().await;
    // Each transport reads only its own source: the header is ignored over HTTP and the cookie over the socket.
    for (router, headers) in [
        (fixture.http(), vec![]),
        (fixture.socket(), vec![]),
        (fixture.http(), vec![("x-fold-human", "tanim")]),
        (fixture.socket(), vec![("cookie", "fold_human=tanim")]),
        (fixture.http(), vec![("cookie", "fold_human=; other=1")]),
        (fixture.socket(), vec![("x-fold-human", "")]),
    ] {
        let (status, body) = call(router, Method::POST, PATH, &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{headers:?}");
        assert_eq!(code(&body), "human_name_required");
    }
    assert!(fixture.archive_actors().await.is_empty());
}

#[tokio::test]
async fn an_invalid_name_gets_invalid_name() {
    let fixture = Fixture::new().await;
    for (router, headers) in [
        (fixture.http(), vec![("cookie", "fold_human=Tanim")]),
        (fixture.socket(), vec![("x-fold-human", "a b")]),
    ] {
        let (status, body) = call(router, Method::POST, PATH, &headers).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{headers:?}");
        assert_eq!(code(&body), "invalid_name");
    }
    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri(PATH)
        .header("x-fold-human", &b"t\xffm"[..])
        .body(Body::empty())
        .unwrap();
    let response = fixture.socket().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(fixture.archive_actors().await.is_empty());
}

#[tokio::test]
async fn an_unknown_task_or_project_is_not_found() {
    let fixture = Fixture::new().await;
    let cookie = [("cookie", "fold_human=tanim")];
    let (status, body) = call(
        fixture.http(),
        Method::POST,
        "/api/v1/projects/demo/tasks/T-9/archive",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "task_not_found");
    let (status, body) = call(
        fixture.http(),
        Method::POST,
        "/api/v1/projects/other/tasks/T-1/archive",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "project_not_found");
}

const AGENTS: &str = "/api/v1/projects/demo/agents";

#[cfg(test)]
impl Fixture {
    fn agents(&self) -> AgentService {
        AgentService::new(
            self.handle.store.clone(),
            self.handle.bus.clone(),
            self.handle.clock.clone(),
        )
    }

    /// Registers `name` in a new session and returns that session.
    async fn register(&self, name: &str) -> fold_daemon::services::agents::SessionContext {
        let agents = self.agents();
        let mut ctx = agents.start_session(2).await.unwrap();
        agents
            .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
            .await
            .unwrap();
        ctx
    }

    fn advance(&self, millis: i64) {
        let now = self.project.now().as_millis();
        self.project
            .clock()
            .set(Timestamp::from_millis(now + millis).unwrap());
    }
}

#[tokio::test]
async fn the_agents_route_returns_the_overview_in_registration_order() {
    let fixture = Fixture::new().await;
    let registered = fixture.project.now().as_millis();
    let backend = fixture.register("backend").await;
    fixture
        .agents()
        .register_worker(&backend, "api", None)
        .await
        .unwrap();
    let (status, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["review", "backend"],
        "registration order, not names"
    );
    assert_eq!(
        body["agents"][1],
        json!({
            "name": "backend",
            "tags": [],
            "presence": "active",
            "last_call_at": registered,
            "sessions": 1,
            "open_sessions": [{"started_at": registered, "pid": 2, "cwd": null}],
            "attention": [],
            "workers": [
                {"name": "backend", "parent": null, "last_call_at": null, "activity": null,
                 "claims": [], "workspaces": [], "proposals": []},
                {"name": "backend/api", "parent": "backend", "last_call_at": null, "activity": null,
                 "claims": [], "workspaces": [], "proposals": []},
            ],
            "events": [],
        })
    );
    assert_eq!(body["agents"][0]["tags"], json!(["review"]));

    fixture.advance(120_000);
    let (_, body) = call(fixture.socket(), Method::GET, AGENTS, &[]).await;
    assert_eq!(body["agents"][1]["presence"], "idle");
    fixture.agents().end_session(&backend).await.unwrap();
    let (_, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(body["agents"][1]["presence"], "offline");
    assert_eq!(body["agents"][1]["sessions"], 0);
    assert_eq!(body["agents"][1]["open_sessions"], json!([]));
}

#[tokio::test]
async fn open_sessions_carry_the_working_directory_of_the_handshake() {
    let fixture = Fixture::new().await;
    let agents = fixture.agents();
    let first = fixture.project.now().as_millis();
    let mut one = agents
        .start_session_in(81231, std::path::Path::new("/home/tanim/ledger"))
        .await
        .unwrap();
    let name = AgentName::parse("backend").unwrap();
    agents.register_agent(&mut one, &name, &[]).await.unwrap();
    fixture.advance(1_000);
    let mut two = agents
        .start_session_in(81232, std::path::Path::new("/home/tanim/ledger/web"))
        .await
        .unwrap();
    agents.register_agent(&mut two, &name, &[]).await.unwrap();

    let (status, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["agents"][1]["open_sessions"],
        json!([
            {"started_at": first, "pid": 81231, "cwd": "/home/tanim/ledger"},
            {"started_at": first + 1_000, "pid": 81232, "cwd": "/home/tanim/ledger/web"},
        ])
    );
    agents.end_session(&one).await.unwrap();
    let (_, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(body["agents"][1]["sessions"], 1);
    assert_eq!(body["agents"][1]["open_sessions"][0]["pid"], 81232);
    assert_eq!(
        body["agents"][1]["open_sessions"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn events_returns_at_most_the_requested_number_newest_first() {
    let fixture = Fixture::new().await;
    let backend = fixture.register("backend").await;
    let agents = fixture.agents();
    for index in 0..60 {
        agents
            .register_worker(&backend, &format!("w{index}"), None)
            .await
            .unwrap();
    }
    let (status, body) = call(
        fixture.http(),
        Method::GET,
        &format!("{AGENTS}?events=3"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let events = body["agents"][1]["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] > pair[1]),
        "newest first: {seqs:?}"
    );
    assert_eq!(events[0]["type"], "worker.registered");
    assert_eq!(events[0]["actor"], "backend");
    assert_eq!(events[0]["data"]["worker"], "backend/w59");
    let review = body["agents"][0]["events"].as_array().unwrap();
    assert!(review.len() <= 3);
    assert!(
        review.iter().all(|e| e["actor"] == "review"),
        "only the agent's own events"
    );

    let (_, body) = call(
        fixture.http(),
        Method::GET,
        &format!("{AGENTS}?events=100"),
        &[],
    )
    .await;
    assert_eq!(
        body["agents"][1]["events"].as_array().unwrap().len(),
        50,
        "clamped to 50"
    );
    let (_, body) = call(
        fixture.http(),
        Method::GET,
        &format!("{AGENTS}?events=0"),
        &[],
    )
    .await;
    assert_eq!(body["agents"][1]["events"], json!([]));

    for query in ["events=abc", "events=-1", "events=3.5"] {
        let (status, body) = call(
            fixture.http(),
            Method::GET,
            &format!("{AGENTS}?{query}"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(code(&body), "invalid_request", "{query}");
    }
}

#[tokio::test]
async fn the_route_judges_stale_claims_by_the_configured_lease() {
    use fold_core::{Actor, AgentActor, TaskDescription, TaskTitle, WorkerName};
    use fold_daemon::services::tasks::{CreateTask, TaskService};

    let fixture = Fixture::with_config(Some("lease_minutes = 1\n")).await;
    fixture.register("backend").await;
    let tasks = TaskService::new(
        fixture.handle.store.clone(),
        fixture.handle.bus.clone(),
        fixture.handle.clock.clone(),
        1,
    );
    let backend = Actor::Agent(AgentActor {
        agent: AgentName::parse("backend").unwrap(),
        worker: WorkerName::parse("backend").unwrap(),
        tags: Vec::new(),
    });
    let task = tasks
        .create_task(
            &backend,
            CreateTask {
                title: TaskTitle::parse("task").unwrap(),
                description: TaskDescription::new("body"),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    tasks.claim_task(&backend, task.id).await.unwrap();
    fixture.advance(59_999);
    let (_, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(body["agents"][1]["workers"][0]["claims"][0]["stale"], false);
    fixture.advance(1);
    let (_, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    let claim = &body["agents"][1]["workers"][0]["claims"][0];
    assert_eq!(claim["task"], task.id.to_string());
    assert_eq!(claim["kind"], "implementation");
    assert_eq!(claim["stale"], true);
}

#[tokio::test]
async fn an_invalid_configuration_fails_the_overview_and_leaves_the_project_open() {
    let fixture = Fixture::new().await;
    std::fs::create_dir_all(fixture.project.paths().fold_dir()).unwrap();
    std::fs::write(
        fixture.project.paths().config(),
        "lease_minutes = \"soon\"\n",
    )
    .unwrap();
    let (status, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(code(&body), "invalid_config");
    let (status, body) = call(
        fixture.socket(),
        Method::POST,
        PATH,
        &[("x-fold-human", "ana")],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    std::fs::write(fixture.project.paths().config(), "lease_minutes = 3\n").unwrap();
    let (status, body) = call(fixture.http(), Method::GET, AGENTS, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

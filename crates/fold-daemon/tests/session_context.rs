use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{
    Actor, Address, AgentActor, AgentName, HumanName, MessageBody, NoteId, TaskDescription, TaskId,
    TaskTitle, WorkerName, codes,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{router, socket_router};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::{
    ContextClaim, MAX_SESSION_CONTEXT, SessionFacts, render_session_context,
};
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::tasks::CreateTask;
use fold_store::agents::ClaimKind;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn name(text: &str) -> AgentName {
    AgentName::parse(text).unwrap()
}

#[cfg(test)]
fn agent(worker: &str) -> Actor {
    let worker = WorkerName::parse(worker).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

/// Project `demo` with agents `frontend` (and its worker `frontend/ui`) and `backend`, and the tasks `T-1` to `T-8`, created at one instant so they are ready in number order.
#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
    root: String,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1".parse().unwrap(),
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        });
        let root = project
            .paths()
            .root()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: root.clone(),
        };
        project
            .registry()
            .write(async |tx| fold_store::projects::insert_project(tx, &row, project.now()).await)
            .await
            .unwrap();
        let handle = state.projects.open(&row, clock).await.unwrap();
        let agents = handle.agents();
        for agent in ["frontend", "backend"] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &name(agent), &[])
                .await
                .unwrap();
            if agent == "frontend" {
                agents.register_worker(&ctx, "ui", None).await.unwrap();
            }
        }
        let ana = HumanName::parse("ana").unwrap();
        project
            .registry()
            .write(async |tx| fold_store::projects::insert_human(tx, &ana, project.now()).await)
            .await
            .unwrap();
        for number in 1..=8 {
            crate::common::roles::lead_work_areas(&handle.store).await;
            handle
                .tasks
                .create_task(
                    &Actor::human(ana.clone()),
                    CreateTask {
                        title: TaskTitle::parse(&format!("Task {number}")).unwrap(),
                        description: TaskDescription::new(""),
                        labels: crate::common::roles::work_labels(),
                    },
                )
                .await
                .unwrap();
        }
        Self {
            project,
            state,
            handle,
            root,
        }
    }

    async fn send_to_frontend(&self) {
        let messages = MessageService::new(
            self.handle.store.clone(),
            self.project.registry().clone(),
            self.handle.bus.clone(),
            self.handle.clock.clone(),
        );
        messages
            .send(
                &agent("backend"),
                SendMessage {
                    to: Address::parse_all(&["frontend"]).unwrap(),
                    body: MessageBody::parse("text").unwrap(),
                    task: None,
                    proposal: None,
                    reply_to: None,
                },
            )
            .await
            .unwrap();
    }

    fn advance_minutes(&self, minutes: f64) {
        self.project
            .clock()
            .advance(Duration::from_secs_f64(minutes * 60.0))
            .unwrap();
    }
}

#[cfg(test)]
async fn post(router: Router, path: &str, body: &Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn the_context_names_the_agent_its_unread_messages_claims_with_minutes_left_and_ready_tasks()
{
    let fixture = Fixture::new().await;
    fixture.send_to_frontend().await;
    fixture.send_to_frontend().await;
    fixture
        .handle
        .tasks
        .claim_task(&agent("frontend"), t(1))
        .await
        .unwrap();
    fixture.advance_minutes(10.0);
    fixture
        .handle
        .tasks
        .claim_task(&agent("frontend/ui"), t(2))
        .await
        .unwrap();
    fixture.advance_minutes(2.5);

    let context = fixture
        .handle
        .agents()
        .session_context("demo", &name("frontend"), &fixture.handle.tasks)
        .await
        .unwrap();
    assert_eq!(
        context,
        "fold project demo.\n\
         You are agent frontend, main worker frontend.\n\
         Unread messages: 2. Call get_inbox.\n\
         Claims:\n\
         - T-1 implementation: 17 minutes left\n\
         - T-2 implementation by frontend/ui: 27 minutes left\n\
         Ready tasks:\n\
         - T-3 Task 3\n\
         - T-4 Task 4\n\
         - T-5 Task 5\n\
         - T-6 Task 6\n\
         - T-7 Task 7"
    );

    let backend = fixture
        .handle
        .agents()
        .session_context("demo", &name("backend"), &fixture.handle.tasks)
        .await
        .unwrap();
    assert!(
        backend.starts_with(
            "fold project demo.\nYou are agent backend, main worker backend.\nUnread messages: 0.\nClaims: none.\nReady tasks:\n- T-3 Task 3\n"
        ),
        "{backend}"
    );
}

#[tokio::test]
async fn an_agent_the_project_does_not_know_gets_its_context_and_no_push_target() {
    let fixture = Fixture::new().await;
    let body = json!({
        "cwd": format!("{}/ws/t-1", fixture.root),
        "agent": "newcomer",
        "socket": "/run/user/1000/claude/inbox.sock",
        "token": "secret",
    });
    let (status, answer) = post(
        socket_router(fixture.state.clone()),
        "/api/v1/hooks/session-start",
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let context = answer["context"].as_str().unwrap();
    assert!(
        context.starts_with(
            "fold project demo.\nYou are agent newcomer, not registered in this project yet; the fold MCP server registers it when it connects.\nClaims: none.\nReady tasks:\n- T-1 Task 1\n"
        ),
        "{context}"
    );
    assert!(fixture.handle.push.sockets(&name("newcomer")).is_empty());
}

#[tokio::test]
async fn the_session_start_route_stores_the_push_target_only_with_socket_and_token() {
    let fixture = Fixture::new().await;
    let socket = socket_router(fixture.state.clone());
    let path = "/api/v1/hooks/session-start";
    let frontend = name("frontend");
    let (status, _) = post(
        socket.clone(),
        path,
        &json!({"cwd": fixture.root, "agent": "frontend", "socket": "/run/a.sock"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(fixture.handle.push.sockets(&frontend).is_empty());
    let (status, answer) = post(
        socket.clone(),
        path,
        &json!({"cwd": fixture.root, "agent": "frontend", "socket": "/run/a.sock", "token": "t0k"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        answer["context"]
            .as_str()
            .unwrap()
            .contains("agent frontend")
    );
    assert_eq!(
        fixture.handle.push.sockets(&frontend),
        [std::path::PathBuf::from("/run/a.sock")]
    );
    let (status, answer) = post(
        socket,
        path,
        &json!({"cwd": fixture.root, "agent": "frontend", "socket": "relative.sock", "token": "t"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(answer["error"]["code"], codes::INVALID_PUSH_TARGET);
}

#[tokio::test]
async fn the_stop_route_counts_unread_messages_and_refuses_a_directory_outside_the_project() {
    let fixture = Fixture::new().await;
    let socket = socket_router(fixture.state.clone());
    let path = "/api/v1/hooks/stop";
    let body = json!({"cwd": fixture.root, "agent": "frontend"});
    assert_eq!(
        post(socket.clone(), path, &body).await,
        (StatusCode::OK, json!({"unread": 0}))
    );
    fixture.send_to_frontend().await;
    assert_eq!(
        post(socket.clone(), path, &body).await,
        (StatusCode::OK, json!({"unread": 1}))
    );
    let (status, answer) = post(
        socket.clone(),
        path,
        &json!({"cwd": fixture.root, "agent": "ghost"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(answer["error"]["code"], codes::AGENT_NOT_REGISTERED);
    let (status, answer) = post(
        socket.clone(),
        path,
        &json!({"cwd": "/", "agent": "frontend"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(answer["error"]["code"], codes::PROJECT_NOT_FOUND);
    let (status, answer) = post(
        socket,
        path,
        &json!({"cwd": fixture.root, "agent": "Not A Name"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(answer["error"]["code"], codes::INVALID_NAME);
}

#[tokio::test]
async fn every_hook_route_is_not_found_on_the_tcp_router() {
    let fixture = Fixture::new().await;
    let body = json!({
        "cwd": fixture.root,
        "agent": "frontend",
        "socket": "/run/a.sock",
        "token": "t0k",
        "workspace": ".fold/ws/review-p-1",
    });
    for path in [
        "/api/v1/hooks/session-start",
        "/api/v1/hooks/stop",
        "/api/v1/hooks/edit",
    ] {
        let (status, answer) = post(router(fixture.state.clone()), path, &body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(answer["error"]["code"], codes::NOT_FOUND, "{path}");
    }
    assert!(fixture.handle.push.sockets(&name("frontend")).is_empty());
}

#[test]
fn the_context_is_capped_at_1500_characters_and_counts_the_claims_it_leaves_out() {
    let worker = WorkerName::parse("frontend/a-worker-with-a-long-name").unwrap();
    let claims = (1..=14)
        .map(|number| ContextClaim {
            task: t(number),
            kind: if number % 2 == 0 {
                ClaimKind::Review
            } else {
                ClaimKind::Implementation
            },
            worker: worker.clone(),
            minutes_left: u64::from(number % 3),
        })
        .collect();
    let facts = SessionFacts {
        project: "p".repeat(200),
        agent: name("frontend"),
        registered: true,
        unread: 3,
        claims,
        ready: (20..25)
            .map(|number| (t(number), "é".repeat(400)))
            .collect(),
        notes: vec![(NoteId::from_number(3).unwrap(), "n".repeat(300))],
    };
    let text = render_session_context(&facts);
    assert_eq!(text.chars().count(), MAX_SESSION_CONTEXT);
    assert!(text.ends_with('…'));
    assert!(
        text.contains(
            "- T-1 implementation by frontend/a-worker-with-a-long-name: 1 minute left\n"
        )
    );
    assert!(text.contains("- T-2 review by frontend/a-worker-with-a-long-name: 2 minutes left\n"));
    assert!(
        text.contains(
            "- T-3 implementation by frontend/a-worker-with-a-long-name: lease expired\n"
        )
    );
    assert!(text.contains("- and 4 more\n"));
    let title = format!("- T-20 {}…\n", "é".repeat(99));
    assert!(text.contains(&title), "a title is cut to 100 characters");
    assert!(
        !text.contains("n-3"),
        "the notes are cut before the ready tasks"
    );

    let short = SessionFacts {
        project: "demo".to_owned(),
        claims: Vec::new(),
        ready: Vec::new(),
        unread: 0,
        ..facts
    };
    assert_eq!(
        render_session_context(&short),
        format!(
            "fold project demo.\nYou are agent frontend, main worker frontend.\nUnread messages: 0.\nClaims: none.\nReady tasks: none.\nRecent decision notes, which you keep to (list_notes reads them):\n- n-3 {}…",
            "n".repeat(79)
        )
    );
    let none = SessionFacts {
        notes: Vec::new(),
        ..short
    };
    assert!(render_session_context(&none).ends_with("Ready tasks: none."));
}

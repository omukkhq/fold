use fold_daemon::http::host::CLIENT_HEADER;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{
    Actor, Address, AgentActor, AgentName, HumanName, MessageBody, MessageId, TaskDescription,
    TaskId, TaskTitle, WorkerName,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::router;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::messages::{MessageService, SendMessage, tasks_awaiting_human};
use fold_daemon::services::tasks::{CreateTask, TaskService};
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
fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
struct Fixture {
    _project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
    messages: MessageService,
}

#[cfg(test)]
impl Fixture {
    /// Human `tanim`; agents `frontend` and `backend`; tasks `T-1` and `T-2`.
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
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
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
        for name in ["frontend", "backend"] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
                .await
                .unwrap();
        }
        let tasks = TaskService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
            5,
        );
        for _ in 0..2 {
            tasks
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
        }
        let messages = MessageService::new(
            handle.store.clone(),
            state.registry.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        Self {
            _project: project,
            state,
            handle,
            messages,
        }
    }

    async fn ask(&self, from: &str, to: &[&str], task: u32, reply_to: Option<MessageId>) {
        self.messages
            .send(
                &agent(from),
                SendMessage {
                    to: Address::parse_all(to).unwrap(),
                    body: MessageBody::parse("which one?").unwrap(),
                    task: Some(t(task)),
                    proposal: None,
                    reply_to,
                },
            )
            .await
            .unwrap();
    }

    async fn questions(&self) -> BTreeSet<TaskId> {
        tasks_awaiting_human(&self.handle.store).await.unwrap()
    }

    /// The attention kinds of each task card on `GET /board`, by task id.
    async fn board(&self) -> BTreeMap<String, Vec<String>> {
        let request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(Method::GET)
            .uri("/api/v1/projects/demo/board")
            .body(Body::empty())
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let board: Value = serde_json::from_slice(&bytes).unwrap();
        board["columns"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|column| column["cards"].as_array().unwrap().iter())
            .map(|card| {
                let kinds = card["attention"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["kind"].as_str().unwrap().to_owned())
                    .collect();
                (card["task"].as_str().unwrap().to_owned(), kinds)
            })
            .collect()
    }

    /// The tasks whose board card shows `question`.
    async fn board_questions(&self) -> Vec<String> {
        self.board()
            .await
            .into_iter()
            .filter(|(_, kinds)| kinds.iter().any(|kind| kind == "question"))
            .map(|(task, _)| task)
            .collect()
    }

    async fn reply(&self, to: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(Method::POST)
            .uri("/api/v1/projects/demo/feedback")
            .header("cookie", "fold_human=tanim")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"body": "the first", "reply_to": to}).to_string(),
            ))
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
}

#[tokio::test]
async fn a_question_to_humans_marks_its_task_until_a_human_replies() {
    let fixture = Fixture::new().await;
    assert!(fixture.questions().await.is_empty());
    assert!(fixture.board_questions().await.is_empty());
    fixture.ask("frontend", &["humans"], 1, None).await;
    assert_eq!(fixture.questions().await, BTreeSet::from([t(1)]));
    let board = fixture.board().await;
    assert_eq!(board["T-1"], ["question"]);
    assert!(board["T-2"].is_empty());
    let (status, body) = fixture.reply("m-1").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(fixture.questions().await.is_empty());
    assert!(fixture.board().await["T-1"].is_empty());
}

#[tokio::test]
async fn a_question_to_one_human_counts_and_an_agent_reply_does_not_answer_it() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["human:tanim"], 2, None).await;
    assert_eq!(fixture.questions().await, BTreeSet::from([t(2)]));
    assert_eq!(fixture.board_questions().await, ["T-2"]);
    fixture
        .ask(
            "backend",
            &["frontend"],
            2,
            Some(MessageId::from_number(1).unwrap()),
        )
        .await;
    assert_eq!(fixture.questions().await, BTreeSet::from([t(2)]));
    let (status, _) = fixture.reply("m-2").await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        fixture.questions().await.is_empty(),
        "a human reply anywhere later in the thread answers it"
    );
}

#[tokio::test]
async fn a_message_to_an_agent_adds_nothing() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["backend"], 1, None).await;
    assert!(fixture.questions().await.is_empty());
    assert!(fixture.board_questions().await.is_empty());
    let (status, _) = fixture.reply("m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    fixture.ask("frontend", &["humans"], 2, None).await;
    fixture.ask("backend", &["humans"], 1, None).await;
    assert_eq!(fixture.questions().await, BTreeSet::from([t(1), t(2)]));
    assert_eq!(fixture.board_questions().await, ["T-1", "T-2"]);
}

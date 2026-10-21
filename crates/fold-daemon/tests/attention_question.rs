use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{
    Actor, Address, AgentActor, AgentName, Choice, HumanName, MessageBody, MessageChoices,
    MessageId, Tag, TaskDescription, TaskId, TaskTitle, WorkerName, codes,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::router;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::{AgentService, NeedsYouKind, needs_you};
use fold_daemon::services::messages::{MessageError, MessageService, SendMessage};
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
fn m(number: u32) -> MessageId {
    MessageId::from_number(number).unwrap()
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
    /// Humans `tanim` and `ana`; agents `frontend`, `backend` and `main` with the `main` tag; tasks `T-1` and `T-2`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1:0".parse().unwrap(),
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        let now = project.now();
        for name in ["tanim", "ana"] {
            let name = HumanName::parse(name).unwrap();
            project
                .registry()
                .write(async |tx| fold_store::projects::insert_human(tx, &name, now).await)
                .await
                .unwrap();
        }
        let agents = AgentService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        for (name, tags) in [
            ("frontend", vec![]),
            ("backend", vec![]),
            ("main", vec![Tag::parse("main").unwrap()]),
        ] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &tags)
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

    /// Sends a message marked as asking a human for an answer.
    async fn ask(&self, from: &str, to: &[&str], task: u32, reply_to: Option<MessageId>) {
        self.send(from, to, task, reply_to, true).await.unwrap();
    }

    /// Sends a plain message: a report that waits for no one.
    async fn report(&self, from: &str, to: &[&str], task: u32, reply_to: Option<MessageId>) {
        self.send(from, to, task, reply_to, false).await.unwrap();
    }

    async fn send(
        &self,
        from: &str,
        to: &[&str],
        task: u32,
        reply_to: Option<MessageId>,
        asks_human: bool,
    ) -> Result<MessageId, MessageError> {
        self.messages
            .send_marked(
                &agent(from),
                SendMessage {
                    to: Address::parse_all(to).unwrap(),
                    body: MessageBody::parse("which one?").unwrap(),
                    task: Some(t(task)),
                    proposal: None,
                    reply_to,
                },
                None,
                None,
                asks_human,
            )
            .await
            .map(|sent| sent.id)
    }

    /// The tasks of the `question` items of Needs you.
    async fn needs_you_questions(&self) -> BTreeSet<TaskId> {
        needs_you(&self.handle.store)
            .await
            .unwrap()
            .into_iter()
            .filter(|item| item.kind == NeedsYouKind::Question)
            .filter_map(|item| item.task)
            .collect()
    }

    /// Sends `request` to the router as human `tanim`.
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
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// `tanim`'s Messages listing: the threads marked `awaiting_human`, and the count the tab badge shows.
    async fn messages_count(&self) -> (Vec<String>, u64) {
        let (status, list) = self
            .call(
                Method::GET,
                "/api/v1/projects/demo/messages?mine=true",
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let marked = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|thread| thread["awaiting_human"] == true)
            .map(|thread| thread["id"].as_str().unwrap().to_owned())
            .collect();
        (marked, list["total_awaiting_human"].as_u64().unwrap())
    }

    /// Checks that the board's question mark, Needs you and the Messages count name the same asks, and returns their tasks.
    async fn agreed(&self) -> BTreeSet<TaskId> {
        let store = self.questions().await;
        let board: BTreeSet<TaskId> = self
            .board_questions()
            .await
            .iter()
            .map(|task| task.parse().unwrap())
            .collect();
        assert_eq!(board, store, "the board and the store");
        assert_eq!(
            self.needs_you_questions().await,
            store,
            "Needs you and the store"
        );
        let (marked, count) = self.messages_count().await;
        assert_eq!(
            marked.len() as u64,
            count,
            "the badge counts the marked threads"
        );
        store
    }

    async fn mark_answered(&self, by: &str, message: u32) -> Result<MessageId, MessageError> {
        self.messages.mark_answered(&agent(by), m(message)).await
    }

    async fn questions(&self) -> BTreeSet<TaskId> {
        fold_store::messages::tasks_awaiting_human(self.handle.store.read())
            .await
            .unwrap()
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
async fn an_ask_to_humans_marks_its_task_until_a_human_replies() {
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
async fn an_ask_to_one_human_counts_and_an_agent_reply_does_not_answer_it() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["human:tanim"], 2, None).await;
    assert_eq!(fixture.questions().await, BTreeSet::from([t(2)]));
    assert_eq!(fixture.board_questions().await, ["T-2"]);
    fixture
        .report(
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
    fixture.report("frontend", &["backend"], 1, None).await;
    assert!(fixture.questions().await.is_empty());
    assert!(fixture.board_questions().await.is_empty());
    let (status, _) = fixture.reply("m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    fixture.ask("frontend", &["humans"], 2, None).await;
    fixture.ask("backend", &["humans"], 1, None).await;
    assert_eq!(fixture.questions().await, BTreeSet::from([t(1), t(2)]));
    assert_eq!(fixture.board_questions().await, ["T-1", "T-2"]);
}

#[tokio::test]
async fn a_report_a_thank_you_and_an_answer_to_the_humans_own_message_wait_for_no_one() {
    let fixture = Fixture::new().await;
    fixture
        .report("frontend", &["humans", "main"], 1, None)
        .await;
    fixture.report("frontend", &["human:tanim"], 2, None).await;
    let (status, _) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(json!({"body": "All good now?", "task": "T-1", "to": ["frontend"]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    fixture
        .report("frontend", &["human:tanim"], 1, Some(m(3)))
        .await;
    assert!(fixture.agreed().await.is_empty());
    assert_eq!(fixture.messages_count().await, (Vec::new(), 0));
}

#[tokio::test]
async fn a_question_with_choices_asks_without_the_mark() {
    let fixture = Fixture::new().await;
    let choices =
        MessageChoices::parse(&[Choice::label("Red"), Choice::label("Blue")], None).unwrap();
    fixture
        .messages
        .send_with_choices(
            &agent("frontend"),
            SendMessage {
                to: Address::parse_all(&["humans"]).unwrap(),
                body: MessageBody::parse("which colour?").unwrap(),
                task: Some(t(2)),
                proposal: None,
                reply_to: None,
            },
            None,
            Some(choices),
        )
        .await
        .unwrap();
    assert_eq!(fixture.agreed().await, BTreeSet::from([t(2)]));
    assert_eq!(fixture.messages_count().await, (vec!["m-1".to_owned()], 1));
}

#[tokio::test]
async fn the_mark_needs_a_human_recipient() {
    let fixture = Fixture::new().await;
    let error = fixture
        .send("frontend", &["backend"], 1, None, true)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
}

#[tokio::test]
async fn the_asking_agent_records_that_the_user_answered_elsewhere() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["humans", "main"], 1, None).await;
    fixture.ask("frontend", &["human:tanim"], 2, None).await;
    assert_eq!(fixture.agreed().await, BTreeSet::from([t(1), t(2)]));
    let refused = fixture.mark_answered("backend", 1).await.unwrap_err();
    assert_eq!(refused.code(), Some(codes::ASK_CLOSE_DENIED));
    assert_eq!(fixture.mark_answered("frontend", 1).await.unwrap(), m(1));
    assert_eq!(fixture.agreed().await, BTreeSet::from([t(2)]));
    assert_eq!(fixture.messages_count().await, (vec!["m-2".to_owned()], 1));
    let again = fixture.mark_answered("frontend", 1).await.unwrap_err();
    assert_eq!(again.code(), Some(codes::ASK_NOT_OPEN));
    let missing = fixture.mark_answered("frontend", 9).await.unwrap_err();
    assert_eq!(missing.code(), Some(codes::MESSAGE_NOT_FOUND));
    let closed = fold_store::events::read_after(
        fixture.handle.store.read(),
        0,
        &fold_core::EventFilter {
            types: vec![fold_core::events::types::MESSAGE_ASK_CLOSED],
            ..fold_core::EventFilter::default()
        },
        100,
    )
    .await
    .unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].actor.as_str(), "frontend");
    assert_eq!(closed[0].task, Some(t(1)));
    assert_eq!(
        closed[0].data,
        json!({"message": "m-1", "thread": "m-1", "reason": "answered_elsewhere"})
    );
}

#[tokio::test]
async fn the_main_agent_records_an_answer_that_the_user_gave_in_its_session() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["humans", "main"], 1, None).await;
    fixture.report("main", &["frontend"], 1, Some(m(1))).await;
    assert_eq!(
        fixture.agreed().await,
        BTreeSet::from([t(1)]),
        "the main agent's reply is no human's answer"
    );
    fixture.mark_answered("main", 1).await.unwrap();
    assert!(fixture.agreed().await.is_empty());
    let not_an_ask = fixture.mark_answered("main", 2).await.unwrap_err();
    assert_eq!(not_an_ask.code(), Some(codes::ASK_NOT_OPEN));
}

#[tokio::test]
async fn a_human_marks_the_asks_of_a_thread_or_a_task_answered() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["humans"], 1, None).await;
    fixture
        .ask("backend", &["human:tanim"], 1, Some(m(1)))
        .await;
    fixture.ask("backend", &["humans"], 2, None).await;
    fixture.ask("frontend", &["human:tanim"], 2, None).await;
    assert_eq!(fixture.agreed().await, BTreeSet::from([t(1), t(2)]));
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/asks/answered",
            Some(json!({"thread": "m-2"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"answered": ["m-1", "m-2"]}));
    assert_eq!(fixture.agreed().await, BTreeSet::from([t(2)]));
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/asks/answered",
            Some(json!({"task": "T-2"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"answered": ["m-3", "m-4"]}));
    assert!(fixture.agreed().await.is_empty());
    let (status, body) = fixture
        .call(
            Method::POST,
            "/api/v1/projects/demo/asks/answered",
            Some(json!({"task": "T-2"})),
        )
        .await;
    assert_eq!((status, body), (StatusCode::OK, json!({"answered": []})));
    for (request, code) in [
        (json!({}), "invalid_request"),
        (json!({"thread": "m-1", "task": "T-1"}), "invalid_request"),
        (json!({"thread": "m-9"}), "message_not_found"),
        (json!({"task": "T-9"}), "task_not_found"),
    ] {
        let (_, body) = fixture
            .call(
                Method::POST,
                "/api/v1/projects/demo/asks/answered",
                Some(request.clone()),
            )
            .await;
        assert_eq!(body["error"]["code"], code, "{request}");
    }
}

#[tokio::test]
async fn the_messages_count_means_the_asks_that_wait_for_the_signed_in_human() {
    let fixture = Fixture::new().await;
    fixture.ask("frontend", &["human:ana"], 1, None).await;
    fixture.ask("backend", &["humans"], 2, None).await;
    assert_eq!(fixture.agreed().await, BTreeSet::from([t(1), t(2)]));
    assert_eq!(fixture.messages_count().await, (vec!["m-2".to_owned()], 1));
    let ana_ask = needs_you(&fixture.handle.store)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.message.map(|(id, _)| id) == Some(m(1)))
        .unwrap();
    assert_eq!(ana_ask.human, Some(HumanName::parse("ana").unwrap()));
}

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{
    Actor, Address, AgentActor, AgentName, HumanName, MessageBody, ProposalId, TaskDescription,
    TaskId, TaskTitle, WorkerName,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{router, socket_router};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_store::StoreError;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const BASE: &str = "/api/v1/projects/demo";
const COOKIE: (&str, &str) = ("cookie", "fold_human=tanim");

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
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    /// Humans `tanim` and `ana`; agents `frontend` with child worker `frontend/ui`, `backend` and `review`; tasks `T-1` and `T-2`; proposal `P-1` on `T-1`.
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
        for name in ["tanim", "ana"] {
            let now = project.now();
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
        for name in ["frontend", "backend", "review"] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
                .await
                .unwrap();
            if name == "frontend" {
                agents.register_worker(&ctx, "ui", None).await.unwrap();
            }
        }
        let tasks = TaskService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
            5,
        );
        for _ in 0..2 {
            crate::common::roles::lead_work_areas(&handle.store).await;
            tasks
                .create_task(
                    &agent("frontend"),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new(""),
                        labels: crate::common::roles::work_labels(),
                    },
                )
                .await
                .unwrap();
        }
        handle
            .store
            .write(async |tx| {
                sqlx::query(
                    "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
                     SELECT 1, t.id, w.id, 'proposal', 0, 0 FROM tasks t, workers w
                     WHERE t.number = 1 AND w.name = 'frontend'",
                )
                .execute(tx.conn())
                .await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
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
            tasks,
        }
    }

    fn http(&self) -> Router {
        router(self.state.clone())
    }

    async fn send(&self, from: &str, to: &[&str], task: Option<u32>, proposal: Option<u32>) {
        self.messages
            .send(
                &agent(from),
                SendMessage {
                    to: Address::parse_all(to).unwrap(),
                    body: MessageBody::parse("text").unwrap(),
                    task: task.map(|n| TaskId::from_number(n).unwrap()),
                    proposal: proposal.map(|n| ProposalId::from_number(n).unwrap()),
                    reply_to: None,
                },
            )
            .await
            .unwrap();
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        call(
            self.http(),
            Method::GET,
            &format!("{BASE}{path}"),
            &[],
            None,
        )
        .await
    }

    async fn feedback(&self, headers: &[(&str, &str)], body: Value) -> (StatusCode, Value) {
        call(
            self.http(),
            Method::POST,
            &format!("{BASE}/feedback"),
            headers,
            Some(body),
        )
        .await
    }
}

#[cfg(test)]
async fn call(
    router: Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let body = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[cfg(test)]
fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

#[cfg(test)]
fn thread_ids(body: &Value) -> Vec<&str> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items in {body}"))
        .iter()
        .map(|thread| thread["id"].as_str().unwrap())
        .collect()
}

#[cfg(test)]
fn recipients(message: &Value) -> Vec<&str> {
    message["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|recipient| recipient["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn the_list_filters_by_agent_task_proposal_and_awaiting_human() {
    let fixture = Fixture::new().await;
    fixture.send("frontend", &["humans"], Some(1), None).await;
    fixture.send("backend", &["review"], None, Some(1)).await;
    fixture
        .send("review", &["frontend/ui"], Some(2), None)
        .await;
    let (status, all) = fixture.get("/messages").await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(thread_ids(&all), ["m-3", "m-2", "m-1"]);
    assert_eq!(all["next"], Value::Null);
    assert_eq!(all["total_awaiting_human"], 1);
    let first = &all["items"][2];
    assert_eq!(first["task"], "T-1");
    assert_eq!(first["proposal"], Value::Null);
    assert_eq!(first["awaiting_human"], true);
    assert_eq!(
        first["participants"],
        json!(["frontend", "human:ana", "human:tanim"])
    );
    assert!(first["last_at"].is_i64());
    let message = &first["messages"][0];
    let mut keys: Vec<_> = message.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "body",
            "choices",
            "header",
            "id",
            "proposal",
            "recipients",
            "reply_to",
            "request",
            "sender",
            "sent_at",
            "task",
            "thread"
        ]
    );
    assert_eq!(recipients(message), ["human:ana", "human:tanim"]);
    for (query, expected) in [
        ("?agent=frontend", vec!["m-3", "m-1"]),
        ("?agent=frontend/ui", vec!["m-3"]),
        ("?agent=review", vec!["m-3", "m-2"]),
        ("?task=T-2", vec!["m-3"]),
        ("?proposal=P-1", vec!["m-2"]),
        ("?awaiting_human=true", vec!["m-1"]),
        ("?awaiting_human=false&task=T-1", vec!["m-1"]),
        ("?agent=review&task=T-1", vec![]),
    ] {
        let (status, body) = fixture.get(&format!("/messages{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        assert_eq!(thread_ids(&body), expected, "{query}");
        assert_eq!(body["total_awaiting_human"], 1, "{query}");
    }
    let (_, page) = fixture.get("/messages?limit=2").await;
    assert_eq!(thread_ids(&page), ["m-3", "m-2"]);
    assert_eq!(page["next"], "m-2");
    let (_, rest) = fixture.get("/messages?limit=2&before=m-2").await;
    assert_eq!(thread_ids(&rest), ["m-1"]);
    assert_eq!(rest["next"], Value::Null);
    for (query, expected) in [
        ("?task=12", "invalid_id"),
        ("?agent=Not%20Valid", "invalid_name"),
        ("?limit=0", "invalid_request"),
        ("?limit=many", "invalid_request"),
    ] {
        let (status, body) = fixture.get(&format!("/messages{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(code(&body), expected, "{query}");
    }
}

#[tokio::test]
async fn a_thread_is_read_by_any_of_its_messages() {
    let fixture = Fixture::new().await;
    fixture.send("frontend", &["backend"], Some(1), None).await;
    fixture
        .messages
        .send(
            &agent("backend"),
            SendMessage {
                to: Address::parse_all(&["frontend"]).unwrap(),
                body: MessageBody::parse("reply").unwrap(),
                task: None,
                proposal: None,
                reply_to: Some("m-1".parse().unwrap()),
            },
        )
        .await
        .unwrap();
    for id in ["m-1", "m-2"] {
        let (status, thread) = fixture.get(&format!("/threads/{id}")).await;
        assert_eq!(status, StatusCode::OK, "{thread}");
        assert_eq!(thread["id"], "m-1");
        assert_eq!(thread["participants"], json!(["frontend", "backend"]));
        assert_eq!(thread["awaiting_human"], false);
        let messages = thread["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["reply_to"], "m-1");
        assert_eq!(messages[1]["task"], "T-1");
        assert_eq!(messages[1]["body"], "reply");
        assert_eq!(
            messages[1]["recipients"],
            json!([{"name": "frontend", "addressed_as": "agent", "delivered_at": null, "acknowledged_at": null, "pushed_at": null, "reachable": false}])
        );
    }
    let (status, body) = fixture.get("/threads/m-9").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "message_not_found");
    let (status, body) = fixture.get("/threads/T-1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_id");
}

#[tokio::test]
async fn feedback_goes_to_the_claim_holder() {
    let fixture = Fixture::new().await;
    let (status, body) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "use the short copy", "task": "T-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(code(&body), "recipient_not_found");
    fixture
        .tasks
        .claim_task(&agent("frontend/ui"), "T-1".parse().unwrap())
        .await
        .unwrap();
    let (status, sent) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "use the short copy", "task": "T-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(sent["id"], "m-1");
    assert_eq!(sent["sender"], "human:tanim");
    assert_eq!(sent["task"], "T-1");
    assert_eq!(recipients(&sent), ["frontend/ui"]);
    let inbox = fixture
        .messages
        .inbox(&agent("frontend/ui"), false)
        .await
        .unwrap();
    assert_eq!(inbox[0].id.to_string(), "m-1");
    let (status, named) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "fyi", "task": "T-1", "to": ["backend", "human:tanim"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{named}");
    assert_eq!(recipients(&named), ["backend", "human:tanim"]);
    let (status, body) = fixture
        .feedback(&[COOKIE], json!({"body": "x", "task": "T-9"}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "task_not_found");
}

#[tokio::test]
async fn a_human_reply_joins_the_thread_and_goes_to_the_asking_worker() {
    let fixture = Fixture::new().await;
    fixture
        .send("frontend/ui", &["humans"], Some(2), None)
        .await;
    let (status, reply) = call(
        socket_router(fixture.state.clone()),
        Method::POST,
        &format!("{BASE}/feedback"),
        &[("x-fold-human", "ana")],
        Some(json!({"body": "the first one", "reply_to": "m-1"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    assert_eq!(reply["thread"], "m-1");
    assert_eq!(reply["reply_to"], "m-1");
    assert_eq!(reply["task"], "T-2");
    assert_eq!(reply["sender"], "human:ana");
    assert_eq!(recipients(&reply), ["frontend/ui"]);
    let (_, thread) = fixture.get("/threads/m-1").await;
    assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
    let (status, body) = fixture
        .feedback(&[COOKIE], json!({"body": "x", "reply_to": "m-7"}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "message_not_found");
}

#[tokio::test]
async fn feedback_needs_a_named_human_and_a_task_or_reply() {
    let fixture = Fixture::new().await;
    let (status, body) = fixture
        .feedback(&[], json!({"body": "x", "task": "T-1"}))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&body), "human_name_required");
    let (status, body) = call(
        socket_router(fixture.state.clone()),
        Method::POST,
        &format!("{BASE}/feedback"),
        &[COOKIE],
        Some(json!({"body": "x", "task": "T-1"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the socket reads only the header"
    );
    assert_eq!(code(&body), "human_name_required");
    for (request, expected) in [
        (json!({"body": "x"}), "invalid_request"),
        (json!({"body": " ", "task": "T-1"}), "invalid_request"),
        (json!({"task": "T-1"}), "invalid_request"),
        (json!({"body": "x", "task": "m-1"}), "invalid_id"),
        (
            json!({"body": "x", "task": "T-1", "to": ["Bad Name"]}),
            "invalid_name",
        ),
    ] {
        let (status, body) = fixture.feedback(&[COOKIE], request.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{request}");
        assert_eq!(code(&body), expected, "{request}");
    }
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(fixture.handle.store.read())
        .await
        .unwrap();
    assert_eq!(messages, 0);
}

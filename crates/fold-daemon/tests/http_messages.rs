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
    /// Humans `tanim` and `ana`; agents `frontend`, `design`, `backend` and `review`; tasks `T-1` and `T-2`; proposal `P-1` on `T-1`.
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
        for name in ["frontend", "design", "backend", "review"] {
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

    /// A message to a human is marked as asking for an answer.
    async fn send(&self, from: &str, to: &[&str], task: Option<u32>, proposal: Option<u32>) {
        self.send_reply(from, to, task, proposal, None).await;
    }

    async fn send_reply(
        &self,
        from: &str,
        to: &[&str],
        task: Option<u32>,
        proposal: Option<u32>,
        reply_to: Option<&str>,
    ) {
        let asks = to
            .iter()
            .any(|entry| *entry == "humans" || entry.starts_with("human:"));
        self.messages
            .send_marked(
                &agent(from),
                SendMessage {
                    to: Address::parse_all(to).unwrap(),
                    body: MessageBody::parse("text").unwrap(),
                    task: task.map(|n| TaskId::from_number(n).unwrap()),
                    proposal: proposal.map(|n| ProposalId::from_number(n).unwrap()),
                    reply_to: reply_to.map(|id| id.parse().unwrap()),
                },
                None,
                None,
                asks,
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
    fixture.send("review", &["design"], Some(2), None).await;
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
            "asks_human",
            "body",
            "choices",
            "header",
            "id",
            "lines",
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
        ("?agent=frontend", vec!["m-1"]),
        ("?agent=design", vec!["m-3"]),
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
async fn mine_lists_only_the_threads_that_concern_the_signed_in_human() {
    let fixture = Fixture::new().await;
    fixture
        .send("frontend", &["human:tanim"], Some(1), None)
        .await;
    fixture.send("backend", &["review"], None, None).await;
    fixture.send("review", &["humans"], None, None).await;
    fixture.send("frontend", &["human:ana"], None, None).await;
    fixture.send("backend", &["frontend"], Some(2), None).await;
    crate::common::roles::give_main_tag(&fixture.handle.store, "backend").await;
    let (status, sent) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "about T-2", "task": "T-2", "to": ["backend"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    fixture
        .send_reply("backend", &["frontend"], None, None, Some("m-2"))
        .await;
    fixture
        .send_reply(
            "frontend",
            &["backend", "human:tanim"],
            None,
            None,
            Some("m-7"),
        )
        .await;
    let mine = async |query: &str| {
        call(
            fixture.http(),
            Method::GET,
            &format!("{BASE}/messages?mine=true{query}"),
            &[COOKIE],
            None,
        )
        .await
    };
    let (status, body) = mine("").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        thread_ids(&body),
        ["m-2", "m-6", "m-3", "m-1"],
        "a thread the human joined later, a message to humans, their own feedback and a message to them by name"
    );
    let awaiting: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|thread| thread["awaiting_human"] == true)
        .map(|thread| thread["id"].as_str().unwrap())
        .collect();
    assert_eq!(awaiting, ["m-2", "m-3", "m-1"]);
    assert_eq!(
        body["total_awaiting_human"], 3,
        "the ask to ana is not tanim's"
    );
    let (_, waiting) = mine("&awaiting_human=true").await;
    assert_eq!(thread_ids(&waiting), ["m-2", "m-3", "m-1"]);
    let (_, page) = mine("&limit=2").await;
    assert_eq!(thread_ids(&page), ["m-2", "m-6"]);
    assert_eq!(page["next"], "m-6");
    assert_eq!(page["total_awaiting_human"], 3);
    let (_, rest) = mine("&limit=2&before=m-6").await;
    assert_eq!(thread_ids(&rest), ["m-3", "m-1"]);
    let (_, all) = fixture.get("/messages").await;
    assert_eq!(
        thread_ids(&all).len(),
        6,
        "without mine every thread is listed"
    );
    assert_eq!(all["total_awaiting_human"], 4);
    let (_, task) = fixture.get("/messages?task=T-2").await;
    assert_eq!(
        thread_ids(&task),
        ["m-6", "m-5"],
        "the task screen keeps the agents' threads"
    );
    let (status, refused) = fixture.get("/messages?mine=true").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    assert_eq!(code(&refused), "human_name_required");
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
            json!([{"name": "frontend", "addressed_as": "agent", "delivered_at": null, "acknowledged_at": null, "pushed_at": null, "reachable": false, "forwarded_to": null}])
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
async fn feedback_goes_to_the_first_reader_and_only_to_the_readers() {
    let fixture = Fixture::new().await;
    let (status, lead) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "use the short copy", "task": "T-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{lead}");
    assert_eq!(
        recipients(&lead),
        ["frontend"],
        "without a claim the area lead reads it"
    );
    crate::common::roles::give_claim(
        &fixture.handle.store,
        "design",
        "T-1".parse().unwrap(),
        fixture.handle.clock.now(),
    )
    .await;
    let (status, sent) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "use the short copy", "task": "T-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(sent["id"], "m-2");
    assert_eq!(sent["sender"], "human:tanim");
    assert_eq!(sent["task"], "T-1");
    assert_eq!(recipients(&sent), ["design"]);
    let inbox = fixture.messages.inbox(&agent("design")).await.unwrap();
    assert_eq!(inbox[0].id.to_string(), "m-2");
    let (status, named) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "fyi", "task": "T-1", "to": ["frontend", "design"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{named}");
    assert_eq!(recipients(&named), ["frontend", "design"]);
    for to in [
        json!(["backend"]),
        json!(["frontend", "tag:ui"]),
        json!(["human:tanim"]),
    ] {
        let (status, body) = fixture
            .feedback(&[COOKIE], json!({"body": "fyi", "task": "T-1", "to": to}))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{to}: {body}");
        assert_eq!(code(&body), "recipient_not_reader", "{to}");
    }
    fixture
        .tasks
        .create_task(
            &agent("review"),
            CreateTask {
                title: TaskTitle::parse("no area").unwrap(),
                description: TaskDescription::new(""),
                labels: vec![],
            },
        )
        .await
        .unwrap();
    let (status, body) = fixture
        .feedback(&[COOKIE], json!({"body": "x", "task": "T-3"}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        code(&body),
        "recipient_not_found",
        "a task in no area of a project without a main agent has no reader"
    );
    let (status, body) = fixture
        .feedback(&[COOKIE], json!({"body": "x", "task": "T-9"}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "task_not_found");
}

#[tokio::test]
async fn a_human_reply_joins_the_thread_and_goes_to_the_asking_worker() {
    let fixture = Fixture::new().await;
    fixture.send("design", &["humans"], Some(2), None).await;
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
    assert_eq!(recipients(&reply), ["design"]);
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

#[tokio::test]
async fn a_task_s_mine_listing_holds_the_human_s_threads_with_the_agents_replies() {
    let fixture = Fixture::new().await;
    fixture.send("frontend", &["backend"], Some(1), None).await;
    crate::common::roles::give_main_tag(&fixture.handle.store, "backend").await;
    let (status, sent) = fixture
        .feedback(
            &[COOKIE],
            json!({"body": "shorter copy", "task": "T-1", "to": ["backend"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    fixture
        .send_reply("backend", &["frontend"], None, None, Some("m-2"))
        .await;
    fixture
        .send("frontend", &["human:ana"], Some(1), None)
        .await;
    fixture.send("frontend", &["humans"], Some(1), None).await;
    fixture.send("frontend", &["humans"], Some(2), None).await;
    let (status, body) = call(
        fixture.http(),
        Method::GET,
        &format!("{BASE}/messages?mine=true&task=T-1"),
        &[COOKIE],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        thread_ids(&body),
        ["m-5", "m-2"],
        "an ask to humans and the human's own feedback; the agents' thread, ana's thread and T-2's ask are left out"
    );
    let feedback = &body["items"][1];
    let ids: Vec<&str> = feedback["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["m-2", "m-3"],
        "the agent's reply is part of the thread"
    );
    assert_eq!(body["items"][0]["awaiting_human"], true);
    assert_eq!(feedback["awaiting_human"], false);
}

#[cfg(test)]
async fn readers(fixture: &Fixture, task: &str) -> Vec<(String, String)> {
    let (status, body) = fixture.get(&format!("/tasks/{task}/readers")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["readers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reader| {
            (
                reader["name"].as_str().unwrap().to_owned(),
                reader["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
fn reader(name: &str, role: &str) -> (String, String) {
    (name.to_owned(), role.to_owned())
}

#[tokio::test]
async fn a_task_offers_its_claim_holder_its_area_lead_and_the_main_agent_as_readers() {
    use fold_core::LabelName;
    let fixture = Fixture::new().await;
    let store = &fixture.handle.store;
    assert_eq!(
        readers(&fixture, "T-1").await,
        [reader("frontend", "area_lead")],
        "the project has no main agent yet"
    );
    crate::common::roles::give_main_tag(store, "review").await;
    assert_eq!(
        readers(&fixture, "T-1").await,
        [
            reader("frontend", "area_lead"),
            reader("review", "main_agent"),
        ],
        "without a claim the area lead is the default"
    );
    crate::common::roles::give_claim(
        &fixture.handle.store,
        "design",
        "T-1".parse().unwrap(),
        fixture.handle.clock.now(),
    )
    .await;
    assert_eq!(
        readers(&fixture, "T-1").await,
        [
            reader("design", "claim_holder"),
            reader("frontend", "area_lead"),
            reader("review", "main_agent"),
        ],
        "the claim holder comes first"
    );
    fixture
        .tasks
        .release_task(&agent("design"), "T-1".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        readers(&fixture, "T-1").await,
        [
            reader("frontend", "area_lead"),
            reader("review", "main_agent"),
        ],
        "a released claim has no holder"
    );
    for (labels, title) in [(vec![], "no area"), (vec![LabelName::main()], "main")] {
        fixture
            .tasks
            .create_task(
                &agent("review"),
                CreateTask {
                    title: TaskTitle::parse(title).unwrap(),
                    description: TaskDescription::new(""),
                    labels,
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(
        readers(&fixture, "T-3").await,
        [reader("review", "main_agent")],
        "a task in no area has no lead"
    );
    assert_eq!(
        readers(&fixture, "T-4").await,
        [reader("review", "main_agent")],
        "a task of Main has the main agent alone"
    );
    crate::common::roles::give_claim(
        store,
        "review",
        "T-4".parse().unwrap(),
        fixture.handle.clock.now(),
    )
    .await;
    assert_eq!(
        readers(&fixture, "T-4").await,
        [reader("review", "claim_holder")],
        "the main agent that builds a task of Main is listed once"
    );
    let (status, body) = fixture.get("/tasks/T-9/readers").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "task_not_found");
    let (status, body) = fixture.get("/tasks/m-1/readers").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_id");
}

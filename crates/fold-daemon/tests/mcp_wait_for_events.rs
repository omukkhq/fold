use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::policy::SessionPeer;
use fold_core::{
    Actor, Address, AgentActor, AgentName, EventActor, EventType, HumanName, MessageBody,
    MessageId, NewEvent, TaskDescription, TaskId, TaskTitle, Timestamp,
};
use fold_daemon::mcp::FoldServer;
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBuffer, EventBus, EventService, WaitQuery, WaitStep};
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::agents::WorkerRow;
use fold_store::testing::{TestProject, eventually};
use fold_store::{Store, StoreError, events};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest,
    ProgressNotificationParam,
};
use rmcp::service::{NotificationContext, PeerRequestOptions, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(10);

/// Records every progress notification the server sends.
#[cfg(test)]
#[derive(Clone)]
struct Recorder(mpsc::UnboundedSender<ProgressNotificationParam>);

impl ClientHandler for Recorder {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self.0.send(params);
    }
}

#[cfg(test)]
type Client = RunningService<RoleClient, Recorder>;

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    agents: AgentService,
    tasks: TaskService,
}

#[cfg(test)]
struct Session {
    client: Arc<Client>,
    progress: mpsc::UnboundedReceiver<ProgressNotificationParam>,
    context: SessionContext,
    /// The agent's main worker, known once the session registered.
    main: Option<WorkerRow>,
}

#[cfg(test)]
impl Session {
    fn main(&self) -> &WorkerRow {
        self.main.as_ref().unwrap()
    }
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        Self::with_bus(EventBus::new()).await
    }

    /// A fixture whose agent service reads the project's `.fold/config.toml`, written with `config`.
    async fn with_config(config: &str) -> Self {
        let mut fx = Self::new().await;
        std::fs::write(fx.project.paths().config(), config).unwrap();
        fx.agents = fx
            .agents
            .clone()
            .with_project_config(fx.project.paths().clone());
        fx
    }

    async fn with_bus(bus: EventBus) -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock, 30);
        Self {
            project,
            bus,
            agents,
            tasks,
        }
    }

    fn store(&self) -> &Store {
        self.project.store()
    }

    fn workspaces(&self) -> WorkspaceService {
        WorkspaceService::new(
            self.store().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
            self.project.paths().clone(),
            30,
        )
    }

    fn messages(&self) -> MessageService {
        MessageService::new(
            self.store().clone(),
            self.project.registry().clone(),
            self.bus.clone(),
            Arc::new(self.project.clock().clone()),
        )
    }

    /// An adapter session of agent `name` over an in-memory stream, already registered and caught up with every stored event.
    async fn connect(&self, name: &str) -> Session {
        self.connect_with_peer(name, SessionPeer::Unsupervised)
            .await
    }

    /// [`Fixture::connect`] for a session whose socket peer runs under `peer`.
    async fn connect_with_peer(&self, name: &str, peer: SessionPeer) -> Session {
        let events = EventService::new(self.store().clone(), self.bus.clone());
        let mut context = self.agents.start_session(7).await.unwrap();
        context.peer = peer;
        let server = FoldServer::new(
            self.agents.clone(),
            events,
            self.workspaces(),
            self.tasks.clone(),
            self.messages(),
            context.clone(),
        );
        let (client_end, server_end) = tokio::io::duplex(256 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });
        let (sender, progress) = mpsc::unbounded_channel();
        let client = Recorder(sender).serve(client_end).await.unwrap();
        let mut session = Session {
            client: Arc::new(client),
            progress,
            context,
            main: None,
        };
        call(&session, "register_agent", json!({"name": name})).await;
        let agent =
            fold_store::agents::find_agent(self.store().read(), &AgentName::parse(name).unwrap())
                .await
                .unwrap()
                .unwrap();
        session.context.agent = Some(agent.name);
        session.main = Some(
            self.agents
                .resolve_worker(&session.context, None)
                .await
                .unwrap(),
        );
        while !call(&session, "get_events", json!({"limit": 500})).await["events"]
            .as_array()
            .unwrap()
            .is_empty()
        {}
        session
    }

    async fn create_task(&self) {
        crate::common::roles::lead_work_areas(self.store()).await;
        self.tasks
            .create_task(
                &Actor::human(HumanName::parse("ana").unwrap()),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new(""),
                    labels: crate::common::roles::work_labels(),
                },
            )
            .await
            .unwrap();
    }

    /// Appends `batch` in one write and publishes it after the commit, as a command does.
    async fn commit(&self, batch: Vec<NewEvent>) -> Vec<u64> {
        commit(self.store(), &self.bus, self.project.now(), batch).await
    }

    /// The main worker's cursor.
    async fn cursor(&self, session: &Session) -> u64 {
        events::cursor(self.store().read(), session.main().id)
            .await
            .unwrap()
    }

    async fn sql(&self, statement: &str) {
        self.store()
            .write(async |tx| {
                sqlx::query(sqlx::AssertSqlSafe(statement))
                    .execute(tx.conn())
                    .await
                    .map_err(StoreError::from)
            })
            .await
            .unwrap();
    }

    async fn millis(&self, sql: &str) -> Option<i64> {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(self.store().read())
            .await
            .unwrap()
    }
}

#[cfg(test)]
async fn commit(store: &Store, bus: &EventBus, now: Timestamp, batch: Vec<NewEvent>) -> Vec<u64> {
    let mut buffer = EventBuffer::new();
    store
        .write(async |tx| {
            for event in &batch {
                buffer.append(tx, event, now).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let seqs = buffer.events().iter().map(|event| event.seq).collect();
    bus.publish(buffer);
    seqs
}

#[cfg(test)]
fn new_event(event_type: EventType, by: &str) -> NewEvent {
    NewEvent::new(event_type, EventActor::parse(by).unwrap()).unwrap()
}

#[cfg(test)]
fn m(number: u32) -> MessageId {
    MessageId::from_number(number).unwrap()
}

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn params(tool: &str, arguments: Value) -> CallToolRequestParams {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments)
}

#[cfg(test)]
async fn call_raw(session: &Session, tool: &str, arguments: Value) -> CallToolResult {
    session
        .client
        .call_tool(params(tool, arguments))
        .await
        .unwrap()
}

#[cfg(test)]
fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[cfg(test)]
async fn call(session: &Session, tool: &str, arguments: Value) -> Value {
    let result = call_raw(session, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    body(&result)
}

#[cfg(test)]
fn returned_seqs(body: &Value) -> Vec<u64> {
    body["events"]
        .as_array()
        .unwrap_or_else(|| panic!("no events in {body}"))
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect()
}

#[cfg(test)]
async fn subscribed(bus: &EventBus, count: usize) {
    eventually(WAIT, async || {
        let now = bus.subscribers();
        if now == count { Ok(()) } else { Err(now) }
    })
    .await;
}

#[cfg(test)]
fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

#[tokio::test]
async fn a_matching_backlog_returns_at_once_and_moves_the_cursor() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let added = fx
        .commit(vec![
            new_event(types::TASK_UPDATED, "backend").task(t(1)),
            new_event(types::MAIN_ADVANCED, "fold"),
        ])
        .await;
    let result = tokio::time::timeout(
        WAIT,
        call(
            &session,
            "wait_for_events",
            json!({"types": ["main.advanced"], "timeout_s": 600}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(returned_seqs(&result), [added[1]]);
    assert_eq!(result["next"], json!(added[1]));
    assert_eq!(
        result["events"][0],
        json!({
            "seq": added[1],
            "type": "main.advanced",
            "actor": "fold",
            "created_at": fx.project.now().as_millis(),
            "data": {},
        })
    );
    assert_eq!(fx.cursor(&session).await, added[1]);
    assert_eq!(
        fx.bus.subscribers(),
        0,
        "the waiter dropped its subscription"
    );
}

#[tokio::test]
async fn a_wait_wakes_on_an_event_committed_while_it_waits() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let client = session.client.clone();
    let waiting = tokio::spawn(async move {
        client
            .call_tool(params(
                "wait_for_events",
                json!({"task": "T-1", "timeout_s": 600}),
            ))
            .await
            .unwrap()
    });
    subscribed(&fx.bus, 1).await;
    fx.commit(vec![new_event(types::TASK_UPDATED, "backend").task(t(2))])
        .await;
    let added = fx
        .commit(vec![
            new_event(types::TASK_UPDATED, "backend").task(t(1)),
            new_event(types::TASK_UPDATED, "human:ana").task(t(1)),
        ])
        .await;
    let result = tokio::time::timeout(WAIT, waiting).await.unwrap().unwrap();
    let body = body(&result);
    assert_eq!(
        returned_seqs(&body),
        added,
        "the match and every event committed with it"
    );
    assert_eq!(body["events"][0]["task"], json!("T-1"));
    assert_eq!(fx.cursor(&session).await, added[1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_event_is_lost_between_the_backlog_and_the_subscription() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let store = fx.store().clone();
    let bus = fx.bus.clone();
    let now = fx.project.now();
    let writers = tokio::spawn(async move {
        let mut handles = Vec::new();
        for _ in 0..200 {
            let store = store.clone();
            let bus = bus.clone();
            handles.push(tokio::spawn(async move {
                commit(
                    &store,
                    &bus,
                    now,
                    vec![new_event(types::MAIN_ADVANCED, "fold")],
                )
                .await
            }));
        }
        let mut seqs = BTreeSet::new();
        for handle in handles {
            seqs.extend(handle.await.unwrap());
        }
        seqs
    });
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), async {
        while seen.len() < 200 {
            let result = call(
                &session,
                "wait_for_events",
                json!({"types": ["main.advanced"], "timeout_s": 30}),
            )
            .await;
            assert_ne!(
                result.get("timed_out"),
                Some(&json!(true)),
                "an event was lost"
            );
            seen.extend(returned_seqs(&result));
        }
    })
    .await
    .unwrap();
    let committed = writers.await.unwrap();
    let unique: BTreeSet<u64> = seen.iter().copied().collect();
    assert_eq!(unique.len(), seen.len(), "an event was returned twice");
    assert_eq!(unique, committed);
}

#[tokio::test]
async fn a_lagged_waiter_recovers_from_the_table() {
    let fx = Fixture::with_bus(EventBus::with_capacity(4)).await;
    let session = fx.connect("frontend").await;
    let client = session.client.clone();
    let waiting = tokio::spawn(async move {
        client
            .call_tool(params(
                "wait_for_events",
                json!({"types": ["main.advanced"], "timeout_s": 600}),
            ))
            .await
            .unwrap()
    });
    subscribed(&fx.bus, 1).await;
    // One publish sends the whole batch while the waiter cannot run, so the match is pushed out of the four-slot channel.
    let mut batch = vec![new_event(types::MAIN_ADVANCED, "fold")];
    batch.extend((0..20).map(|_| new_event(types::TASK_UPDATED, "backend").task(t(1))));
    let added = fx.commit(batch).await;
    let result = tokio::time::timeout(WAIT, waiting).await.unwrap().unwrap();
    assert_eq!(returned_seqs(&body(&result)), [added[0]]);
}

#[tokio::test]
async fn a_message_event_wakes_only_an_agent_whose_worker_receives_it() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    let backend = fx.connect("backend").await;
    fx.sql(
        "INSERT INTO messages (number, thread_id, sender_kind, sender_human, body, created_at)
         VALUES (1, 1, 'human', 'ana', 'one', 0), (2, 2, 'human', 'ana', 'two', 0);
         INSERT INTO message_recipients (message_id, worker_id, addressed_as)
         SELECT m.id, w.id, 'agent' FROM messages m, workers w WHERE m.number = 1 AND w.name = 'backend';
         INSERT INTO message_recipients (message_id, human, addressed_as)
         SELECT id, 'bo', 'human' FROM messages WHERE number = 2;",
    )
    .await;
    let added = fx
        .commit(vec![
            new_event(types::MESSAGE_SENT, "human:ana").message(m(1)),
            new_event(types::MESSAGE_SENT, "human:ana").message(m(2)),
            new_event(types::MESSAGE_ACKNOWLEDGED, "backend").message(m(1)),
            new_event(types::MESSAGE_SENT, "human:ana"),
        ])
        .await;
    let types = json!(["message.sent", "message.acknowledged"]);
    let cursor = fx.cursor(&frontend).await;
    let reached = call(
        &backend,
        "wait_for_events",
        json!({"types": types, "timeout_s": 1}),
    )
    .await;
    assert_eq!(returned_seqs(&reached), [added[0], added[2]]);
    let missed = call(
        &frontend,
        "wait_for_events",
        json!({"types": types, "timeout_s": 1}),
    )
    .await;
    assert_eq!(missed, json!({"timed_out": true}));
    assert_eq!(
        fx.cursor(&frontend).await,
        cursor,
        "nothing returned, nothing moved"
    );
}

#[tokio::test]
async fn a_sent_message_wakes_its_recipient_and_no_other_agent() {
    let fx = Fixture::new().await;
    let frontend = fx.connect("frontend").await;
    let backend = fx.connect("backend").await;
    let messages = MessageService::new(
        fx.store().clone(),
        fx.project.registry().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
    );
    let wait = |session: &Session| {
        let client = session.client.clone();
        tokio::spawn(async move {
            client
                .call_tool(params(
                    "wait_for_events",
                    json!({"types": ["message.sent"], "timeout_s": 2}),
                ))
                .await
                .unwrap()
        })
    };
    let reached = wait(&backend);
    let missed = wait(&frontend);
    subscribed(&fx.bus, 2).await;
    let sender = Actor::human(HumanName::parse("ana").unwrap());
    let sent = messages
        .send(
            &sender,
            SendMessage {
                to: Address::parse_all(&["backend"]).unwrap(),
                body: MessageBody::parse("are you there?").unwrap(),
                task: None,
                proposal: None,
                reply_to: None,
            },
        )
        .await
        .unwrap();
    let reached = tokio::time::timeout(WAIT, reached).await.unwrap().unwrap();
    let reached = body(&reached);
    assert_eq!(reached["events"].as_array().unwrap().len(), 1, "{reached}");
    assert_eq!(
        reached["events"][0]["data"]["message"],
        json!(sent.id.to_string())
    );
    let missed = tokio::time::timeout(WAIT, missed).await.unwrap().unwrap();
    assert_eq!(body(&missed), json!({"timed_out": true}));
}

#[cfg(test)]
fn wait_in_background(
    session: &Session,
    arguments: Value,
) -> tokio::task::JoinHandle<CallToolResult> {
    let client = session.client.clone();
    tokio::spawn(async move {
        client
            .call_tool(params("wait_for_events", arguments))
            .await
            .unwrap()
    })
}

#[cfg(test)]
async fn send_as_human(fx: &Fixture, to: &str) -> MessageId {
    fx.messages()
        .send(
            &Actor::human(HumanName::parse("ana").unwrap()),
            SendMessage {
                to: Address::parse_all(&[to]).unwrap(),
                body: MessageBody::parse("are you there?").unwrap(),
                task: None,
                proposal: None,
                reply_to: None,
            },
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_filtered_wait_ends_for_a_new_message_to_the_waiter_only() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    let frontend = fx.connect("frontend").await;
    let filter = json!({"types": ["worker.stopped", "proposal.opened"], "timeout_s": 2});
    let reached = wait_in_background(&backend, filter.clone());
    let missed = wait_in_background(&frontend, filter);
    subscribed(&fx.bus, 2).await;
    let sent = send_as_human(&fx, "backend").await;
    let reached = tokio::time::timeout(WAIT, reached).await.unwrap().unwrap();
    assert_eq!(reached.is_error, Some(false));
    let woke = body(&reached);
    assert_eq!(woke["events"][0]["type"], json!("message.sent"));
    assert_eq!(woke["messages"], json!([sent.to_string()]));
    assert_eq!(
        woke["hint"],
        json!("a message waits for you; call get_inbox")
    );
    let footer = reached.content.last().unwrap().as_text().unwrap();
    assert!(footer.text.starts_with("fold: 1 unread"), "{}", footer.text);
    assert_eq!(fx.cursor(&backend).await, woke["next"].as_u64().unwrap());
    let missed = tokio::time::timeout(WAIT, missed).await.unwrap().unwrap();
    assert_eq!(body(&missed), json!({"timed_out": true}));
    let again = call(
        &backend,
        "wait_for_events",
        json!({"types": ["proposal.opened"], "timeout_s": 1}),
    )
    .await;
    assert_eq!(
        again,
        json!({"timed_out": true}),
        "the cursor passed the message"
    );
}

#[cfg(test)]
fn filtered(types: &[&str], worker: Option<&str>) -> Value {
    let mut arguments = json!({"types": types, "timeout_s": 2});
    if let Some(worker) = worker {
        arguments["as"] = json!(worker);
    }
    arguments
}

#[tokio::test]
async fn a_main_worker_wakes_for_its_workers_and_a_child_only_for_itself() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    call(&backend, "register_worker", json!({"name": "sub"})).await;
    let main = wait_in_background(&backend, filtered(&["proposal.opened"], None));
    subscribed(&fx.bus, 1).await;
    let to_sub = send_as_human(&fx, "backend/sub").await;
    let main = body(&tokio::time::timeout(WAIT, main).await.unwrap().unwrap());
    assert_eq!(main["messages"], json!([to_sub.to_string()]));
    assert_eq!(
        main["hint"],
        json!("a message waits for a worker of your agent; call get_inbox with include_workers")
    );
    let child = call(
        &backend,
        "wait_for_events",
        filtered(&["proposal.opened"], Some("backend/sub")),
    )
    .await;
    assert_eq!(
        child["messages"],
        json!([to_sub.to_string()]),
        "the main worker's wait leaves the child's cursor alone"
    );
    let child = wait_in_background(
        &backend,
        filtered(&["proposal.opened"], Some("backend/sub")),
    );
    subscribed(&fx.bus, 1).await;
    send_as_human(&fx, "backend").await;
    let child = body(&tokio::time::timeout(WAIT, child).await.unwrap().unwrap());
    assert_eq!(
        child,
        json!({"timed_out": true}),
        "a message to the main worker"
    );
}

#[tokio::test]
async fn a_main_worker_and_its_child_waiting_together_both_wake_for_the_childs_message() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    call(&backend, "register_worker", json!({"name": "sub"})).await;
    let main = wait_in_background(&backend, filtered(&["proposal.opened"], None));
    let child = wait_in_background(
        &backend,
        filtered(&["proposal.opened"], Some("backend/sub")),
    );
    subscribed(&fx.bus, 2).await;
    let to_sub = send_as_human(&fx, "backend/sub").await;
    let main = body(&tokio::time::timeout(WAIT, main).await.unwrap().unwrap());
    let child = body(&tokio::time::timeout(WAIT, child).await.unwrap().unwrap());
    assert_eq!(main["messages"], json!([to_sub.to_string()]), "{main}");
    assert_eq!(child["messages"], json!([to_sub.to_string()]), "{child}");
    assert_eq!(
        child["hint"],
        json!("a message waits for you; call get_inbox")
    );
}

#[tokio::test]
async fn concurrent_waits_of_a_main_worker_and_its_child_each_wake_for_their_own_message() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    call(&backend, "register_worker", json!({"name": "sub"})).await;
    let main = wait_in_background(&backend, filtered(&["worker.stopped"], None));
    let child = wait_in_background(
        &backend,
        filtered(&["proposal.opened"], Some("backend/sub")),
    );
    subscribed(&fx.bus, 2).await;
    let to_main = send_as_human(&fx, "backend").await;
    let main = body(&tokio::time::timeout(WAIT, main).await.unwrap().unwrap());
    assert_eq!(main["messages"], json!([to_main.to_string()]), "{main}");
    let to_sub = send_as_human(&fx, "backend/sub").await;
    let child = body(&tokio::time::timeout(WAIT, child).await.unwrap().unwrap());
    assert_eq!(child["messages"], json!([to_sub.to_string()]), "{child}");
    let again = call(
        &backend,
        "wait_for_events",
        filtered(&["worker.stopped"], None),
    )
    .await;
    assert_eq!(
        again["messages"],
        json!([to_sub.to_string()]),
        "the child's wait left the message to it for the main worker"
    );
}

#[tokio::test]
async fn the_footer_counts_new_events_after_the_calling_workers_cursor() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    call(&backend, "register_worker", json!({"name": "sub"})).await;
    let head = fx
        .commit(vec![new_event(types::MAIN_ADVANCED, "fold")])
        .await[0];
    let footer = |result: &CallToolResult| {
        result
            .content
            .get(1)
            .map(|line| line.as_text().unwrap().text.clone())
    };
    let main = call_raw(&backend, "get_events", json!({})).await;
    assert_eq!(footer(&main), None);
    let peek = call_raw(
        &backend,
        "get_events",
        json!({"after": head, "as": "backend/sub"}),
    )
    .await;
    assert_eq!(
        footer(&peek).as_deref(),
        Some("fold: 1 new events"),
        "the main worker's read leaves the child's count"
    );
    let read = call_raw(&backend, "get_events", json!({"as": "backend/sub"})).await;
    assert_eq!(returned_seqs(&body(&read)).last(), Some(&head));
    assert_eq!(footer(&read), None, "the child's read clears its count");
}

#[tokio::test]
async fn a_bad_filter_is_refused_before_the_wait_starts() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    for arguments in [
        json!({"types": ["task.exploded"]}),
        json!({"task": "12"}),
        json!({"proposal": "T-1"}),
    ] {
        let result = call_raw(&session, "wait_for_events", arguments).await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            body(&result)["error"]["code"],
            json!("invalid_event_filter")
        );
        assert_eq!(body(&result)["error"]["hint"], json!("call get_events"));
    }
    let result = call_raw(&session, "get_events", json!({"limit": 0})).await;
    assert_eq!(
        body(&result)["error"]["code"],
        json!("invalid_event_filter")
    );
    assert_eq!(fx.bus.subscribers(), 0);
}

#[tokio::test]
async fn the_event_tools_need_a_registered_agent() {
    let fx = Fixture::new().await;
    let events = EventService::new(fx.store().clone(), fx.bus.clone());
    let context = fx.agents.start_session(9).await.unwrap();
    let server = FoldServer::new(
        fx.agents.clone(),
        events,
        fx.workspaces(),
        fx.tasks.clone(),
        fx.messages(),
        context,
    );
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_end).await.unwrap();
    for tool in ["get_events", "wait_for_events"] {
        let result = client.call_tool(params(tool, json!({}))).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let error = body(&result)["error"].clone();
        assert_eq!(error["code"], json!("agent_not_registered"));
        assert_eq!(error["hint"], json!("call register_agent"));
    }
}

#[tokio::test]
async fn cancelling_the_request_drops_the_subscription() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(params(
        "wait_for_events",
        json!({"timeout_s": 1500}),
    )));
    let handle = session
        .client
        .peer()
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await
        .unwrap();
    subscribed(&fx.bus, 1).await;
    handle.cancel(Some("done waiting".into())).await.unwrap();
    subscribed(&fx.bus, 0).await;
    let still = call(&session, "list_agents", json!({})).await;
    assert_eq!(still["agents"].as_array().unwrap().len(), 1);
}

/// Keeps tokio from moving a paused clock by itself while it lives: a running blocking task holds auto-advance off, so the clock moves only through `tokio::time::advance` even while the runtime waits on SQLite's threads. Without auto-advance a timer due exactly now never fires, because tokio rounds a deadline up to the next millisecond, so every step goes a second past the deadline it targets.
#[cfg(test)]
struct ManualTime {
    release: std::sync::mpsc::Sender<()>,
}

#[cfg(test)]
impl ManualTime {
    fn start() -> Self {
        let (release, held) = std::sync::mpsc::channel::<()>();
        tokio::task::spawn_blocking(move || {
            let _ = held.recv_timeout(Duration::from_secs(60));
        });
        tokio::time::pause();
        Self { release }
    }

    /// Awaits `future` and fails after `WAIT` of wall time, which the paused clock cannot measure.
    async fn within<T>(&self, future: impl Future<Output = T>) -> T {
        let (fire, fired) = tokio::sync::oneshot::channel::<()>();
        let (done, finished) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if finished.recv_timeout(WAIT).is_err() {
                let _ = fire.send(());
            }
        });
        let output = tokio::select! {
            output = future => output,
            _ = fired => panic!("no result within {WAIT:?} of wall time"),
        };
        let _ = done.send(());
        output
    }

    /// Spins on the runtime until `check` holds, for at most `WAIT` of wall time.
    async fn until(&self, mut check: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + WAIT;
        while !check() {
            assert!(std::time::Instant::now() < deadline, "condition not met");
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(test)]
impl Drop for ManualTime {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

#[tokio::test]
async fn a_wait_ticks_every_minute_renews_the_worker_and_times_out() {
    let fx = Fixture::new().await;
    fx.create_task().await;
    let mut session = fx.connect("frontend").await;
    let main = fx
        .agents
        .resolve_worker(&session.context, None)
        .await
        .unwrap();
    fx.tasks.claim_task(&by(&main), t(1)).await.unwrap();
    call(&session, "get_events", json!({})).await;
    let session_id = session.context.session_id;
    let time = ManualTime::start();
    let client = session.client.clone();
    let started = tokio::time::Instant::now();
    let waiting = tokio::spawn(async move {
        client
            .call_tool(params("wait_for_events", json!({"timeout_s": 130})))
            .await
            .unwrap()
    });
    time.until(|| fx.bus.waiters() == 1).await;
    for tick in 1..=2_u32 {
        let renewed = fx.project.clock().advance(Duration::from_secs(60)).unwrap();
        tokio::time::advance(Duration::from_secs(61)).await;
        let progress = time.within(session.progress.recv()).await.unwrap();
        assert_eq!(progress.progress, f64::from(tick));
        assert!(started.elapsed() >= Duration::from_secs(60 * u64::from(tick)));
        let at = Some(renewed.as_millis());
        assert_eq!(
            fx.millis(&format!(
                "SELECT last_call_at FROM sessions WHERE id = {session_id}"
            ))
            .await,
            at
        );
        assert_eq!(
            fx.millis("SELECT last_call_at FROM workers WHERE name = 'frontend'")
                .await,
            at
        );
        assert_eq!(
            fx.millis("SELECT renewed_at FROM claims WHERE released_at IS NULL")
                .await,
            at,
            "the claim lease is renewed while the worker waits"
        );
    }
    assert!(!waiting.is_finished(), "the wait runs until its timeout");
    tokio::time::advance(Duration::from_secs(9)).await;
    let result = time.within(waiting).await.unwrap();
    assert_eq!(body(&result), json!({"timed_out": true}));
    assert!(started.elapsed() >= Duration::from_secs(130));
    assert!(
        session.progress.try_recv().is_err(),
        "two ticks, two notices"
    );
    drop(time);
    assert_eq!(fx.bus.subscribers(), 0);
}

#[tokio::test]
async fn a_timeout_above_the_limit_is_lowered_to_1500_seconds() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let time = ManualTime::start();
    let client = session.client.clone();
    let started = tokio::time::Instant::now();
    let waiting = tokio::spawn(async move {
        client
            .call_tool(params("wait_for_events", json!({"timeout_s": 99_999})))
            .await
            .unwrap()
    });
    time.until(|| fx.bus.waiters() == 1).await;
    tokio::time::advance(Duration::from_secs(1501)).await;
    let result = time.within(waiting).await.unwrap();
    assert_eq!(body(&result), json!({"timed_out": true}));
    assert!(started.elapsed() < Duration::from_secs(1600));
}

/// Starts a wait for 99 999 s as the agent `frontend`, which fold supervises with `role`, in a session whose peer is `peer`, and returns it with the paused clock that drives it.
#[cfg(test)]
async fn supervised_wait(
    fx: &Fixture,
    role: &str,
    peer: SessionPeer,
) -> (ManualTime, tokio::task::JoinHandle<CallToolResult>) {
    let session = fx.connect_with_peer("frontend", peer).await;
    fx.sql(&format!(
        "INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) \
         SELECT id, '{role}', 'session-1', 'working', 'main', 1 FROM agents WHERE name = 'frontend'"
    ))
    .await;
    let time = ManualTime::start();
    let client = session.client.clone();
    let waiting = tokio::spawn(async move {
        client
            .call_tool(params("wait_for_events", json!({"timeout_s": 99_999})))
            .await
            .unwrap()
    });
    time.until(|| fx.bus.waiters() == 1).await;
    (time, waiting)
}

#[cfg(test)]
fn frontend_process() -> SessionPeer {
    SessionPeer::Worker(AgentName::parse("frontend").unwrap())
}

/// Checks that the wait runs past 270 s and times out at 1500 s.
#[cfg(test)]
async fn assert_long_wait(time: ManualTime, waiting: tokio::task::JoinHandle<CallToolResult>) {
    tokio::time::advance(Duration::from_secs(271)).await;
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    assert!(!waiting.is_finished());
    tokio::time::advance(Duration::from_secs(1230)).await;
    let result = time.within(waiting).await.unwrap();
    assert_eq!(body(&result), json!({"timed_out": true}));
}

/// Checks that the wait times out at 270 s.
#[cfg(test)]
async fn assert_short_wait(time: ManualTime, waiting: tokio::task::JoinHandle<CallToolResult>) {
    tokio::time::advance(Duration::from_secs(271)).await;
    let result = time.within(waiting).await.unwrap();
    assert_eq!(body(&result), json!({"timed_out": true}));
}

#[tokio::test]
async fn a_reviewer_with_the_five_minute_cache_waits_at_most_270_seconds() {
    let fx = Fixture::new().await;
    let (time, waiting) = supervised_wait(&fx, "reviewer", frontend_process()).await;
    assert_short_wait(time, waiting).await;
}

#[tokio::test]
async fn a_lead_with_the_one_hour_cache_keeps_its_longer_wait() {
    let fx = Fixture::new().await;
    let (time, waiting) = supervised_wait(&fx, "review_lead", frontend_process()).await;
    assert_long_wait(time, waiting).await;
}

#[tokio::test]
async fn a_session_opened_by_hand_under_a_workers_name_keeps_its_longer_wait() {
    let fx = Fixture::new().await;
    let (time, waiting) = supervised_wait(&fx, "reviewer", SessionPeer::Unsupervised).await;
    assert_long_wait(time, waiting).await;
}

#[tokio::test]
async fn a_reviewer_that_the_project_gives_the_one_hour_cache_keeps_its_longer_wait() {
    let fx = Fixture::with_config("[roles.reviewer]\ncache_ttl = \"1h\"\n").await;
    let (time, waiting) = supervised_wait(&fx, "reviewer", frontend_process()).await;
    assert_long_wait(time, waiting).await;
}

#[tokio::test]
async fn a_lead_that_the_project_gives_the_five_minute_cache_waits_at_most_270_seconds() {
    let fx = Fixture::with_config("[roles.review_lead]\ncache_ttl = \"5m\"\n").await;
    let (time, waiting) = supervised_wait(&fx, "review_lead", frontend_process()).await;
    assert_short_wait(time, waiting).await;
}

#[tokio::test]
async fn a_ready_match_wins_over_a_deadline_that_has_passed() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let added = fx
        .commit(vec![new_event(types::MAIN_ADVANCED, "fold")])
        .await;
    let events = EventService::new(fx.store().clone(), fx.bus.clone());
    let time = ManualTime::start();
    let query = WaitQuery {
        timeout_s: Some(1),
        ..WaitQuery::default()
    };
    let mut waiter = time
        .within(events.wait(session.main(), &query))
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    let step = time.within(waiter.next()).await.unwrap();
    let WaitStep::Events(page) = step else {
        panic!("the deadline won over a ready match: {step:?}");
    };
    let returned: Vec<u64> = page.events.iter().map(|event| event.seq).collect();
    assert_eq!(returned, added);
    assert_eq!(fx.cursor(&session).await, added[0]);
}

#[tokio::test]
async fn the_footer_counts_new_events_that_concern_the_agent() {
    let fx = Fixture::new().await;
    fx.create_task().await;
    let session = fx.connect("frontend").await;
    let main = fx
        .agents
        .resolve_worker(&session.context, None)
        .await
        .unwrap();
    fx.tasks.claim_task(&by(&main), t(1)).await.unwrap();
    call(&session, "get_events", json!({})).await;
    let quiet = call_raw(&session, "list_agents", json!({})).await;
    assert_eq!(
        quiet.content.len(),
        1,
        "no footer line when every count is zero"
    );
    let cursor = fx.cursor(&session).await;
    fx.commit(vec![
        new_event(types::TASK_UPDATED, "backend").task(t(1)),
        new_event(types::MAIN_ADVANCED, "fold"),
    ])
    .await;
    let footer = |result: &CallToolResult| {
        result
            .content
            .get(1)
            .map(|line| line.as_text().unwrap().text.clone())
    };
    let result = call_raw(&session, "list_agents", json!({})).await;
    assert_eq!(footer(&result).as_deref(), Some("fold: 2 new events"));
    fx.commit(vec![new_event(types::TASK_UPDATED, "frontend").task(t(1))])
        .await;
    let result = call_raw(&session, "list_agents", json!({})).await;
    assert_eq!(
        footer(&result).as_deref(),
        Some("fold: 2 new events"),
        "the agent's own action is not news"
    );
    assert_eq!(
        fx.cursor(&session).await,
        cursor,
        "the footer never moves the cursor"
    );
    let after = call_raw(&session, "get_events", json!({"after": cursor})).await;
    assert_eq!(footer(&after).as_deref(), Some("fold: 2 new events"));
    let read = call_raw(&session, "get_events", json!({})).await;
    assert_eq!(returned_seqs(&body(&read)).len(), 3);
    assert_eq!(
        footer(&read),
        None,
        "reading after the cursor clears the count"
    );
}

#[tokio::test]
async fn a_wait_behind_a_large_backlog_reads_one_page() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let added = fx
        .commit(
            (0..1500)
                .map(|_| new_event(types::MAIN_ADVANCED, "fold"))
                .collect(),
        )
        .await;
    let events = EventService::new(fx.store().clone(), fx.bus.clone());
    let query = WaitQuery {
        types: vec!["main.advanced".to_owned()],
        timeout_s: Some(600),
        ..WaitQuery::default()
    };
    for page in 0..2 {
        let mut waiter = events.wait(session.main(), &query).await.unwrap();
        assert_eq!(waiter.held(), 100, "the waiter holds one page, not 1500");
        let step = tokio::time::timeout(WAIT, waiter.next())
            .await
            .unwrap()
            .unwrap();
        let WaitStep::Events(read) = step else {
            panic!("no page: {step:?}");
        };
        let seqs: Vec<u64> = read.events.iter().map(|event| event.seq).collect();
        assert_eq!(seqs, added[page * 100..(page + 1) * 100]);
        assert_eq!(fx.cursor(&session).await, added[page * 100 + 99]);
    }
}

#[tokio::test]
async fn a_wait_ends_once_the_bus_closes() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    let events = EventService::new(fx.store().clone(), fx.bus.clone());
    let query = WaitQuery {
        timeout_s: Some(1500),
        ..WaitQuery::default()
    };
    let mut waiter = events.wait(session.main(), &query).await.unwrap();
    let waiting = tokio::spawn(async move { waiter.next().await.unwrap() });
    fx.bus.close();
    let step = tokio::time::timeout(WAIT, waiting).await.unwrap().unwrap();
    assert_eq!(step, WaitStep::TimedOut);
    let mut late = events.wait(session.main(), &query).await.unwrap();
    let step = tokio::time::timeout(WAIT, late.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        step,
        WaitStep::TimedOut,
        "a wait that starts after the close ends at once"
    );
}

#[tokio::test]
async fn the_footer_shows_more_than_100_new_events_as_100_plus() {
    let fx = Fixture::new().await;
    let session = fx.connect("frontend").await;
    fx.commit(
        (0..150)
            .map(|_| new_event(types::MAIN_ADVANCED, "fold"))
            .collect(),
    )
    .await;
    let result = call_raw(&session, "list_agents", json!({})).await;
    let footer = result
        .content
        .get(1)
        .map(|line| line.as_text().unwrap().text.clone());
    assert_eq!(footer.as_deref(), Some("fold: 100+ new events"));
}

#[tokio::test]
async fn a_claim_in_the_last_quarter_of_its_lease_shows_in_the_next_footer() {
    let fx = Fixture::new().await;
    fx.create_task().await;
    let session = fx.connect("frontend").await;
    let main = fx
        .agents
        .resolve_worker(&session.context, None)
        .await
        .unwrap();
    fx.tasks.claim_task(&by(&main), t(1)).await.unwrap();
    let footer = |result: &CallToolResult| {
        result
            .content
            .get(1)
            .map(|line| line.as_text().unwrap().text.clone())
    };
    let fresh = call_raw(&session, "get_events", json!({})).await;
    assert_eq!(footer(&fresh), None);
    // The lease is 30 minutes, so its last quarter starts 22.5 minutes after the renewal.
    fx.project
        .clock()
        .advance(Duration::from_secs(22 * 60))
        .unwrap();
    let early = call_raw(&session, "list_agents", json!({})).await;
    assert_eq!(footer(&early), None);
    fx.project
        .clock()
        .advance(Duration::from_secs(23 * 60))
        .unwrap();
    let late = call_raw(&session, "list_agents", json!({})).await;
    assert_eq!(footer(&late).as_deref(), Some("fold: 1 claims near stale"));
    let renewed = call_raw(&session, "list_agents", json!({})).await;
    assert_eq!(
        footer(&renewed),
        None,
        "the call that showed the claim renewed it"
    );
}

#[tokio::test]
async fn a_message_the_agent_has_read_never_wakes_its_next_filtered_wait() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    for _ in 0..3 {
        send_as_human(&fx, "backend").await;
    }
    let inbox = call(&backend, "get_inbox", json!({})).await;
    assert_eq!(
        inbox["messages"].as_array().map(Vec::len),
        Some(3),
        "{inbox}"
    );
    let filter = json!({"types": ["proposal.opened"], "timeout_s": 1});
    assert_eq!(
        call(&backend, "wait_for_events", filter.clone()).await,
        json!({"timed_out": true}),
        "the first wait of an agent that never waited skips the messages it read"
    );
    let unread = send_as_human(&fx, "backend").await;
    let woke = call(&backend, "wait_for_events", filter).await;
    assert_eq!(woke["messages"], json!([unread.to_string()]));
}

#[tokio::test]
async fn a_worker_never_wakes_for_its_own_message_and_a_main_worker_wakes_for_its_childs() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    call(&backend, "register_worker", json!({"name": "sub"})).await;
    let filter = json!({"types": ["proposal.opened"], "timeout_s": 1});
    call(
        &backend,
        "send_message",
        json!({"to": ["backend/sub"], "body": "take the parser"}),
    )
    .await;
    assert_eq!(
        call(&backend, "wait_for_events", filter.clone()).await,
        json!({"timed_out": true}),
        "the main worker's message to its child"
    );
    let report = call(
        &backend,
        "send_message",
        json!({"to": ["backend"], "body": "the parser is done", "as": "backend/sub"}),
    )
    .await;
    let woke = call(&backend, "wait_for_events", filter).await;
    assert_eq!(woke["messages"], json!([report["id"]]), "{woke}");
}

#[tokio::test]
async fn a_wait_names_no_message_the_worker_has_read_or_sent() {
    let fx = Fixture::new().await;
    let backend = fx.connect("backend").await;
    call(&backend, "register_worker", json!({"name": "sub"})).await;
    let read = send_as_human(&fx, "backend").await;
    call(&backend, "get_inbox", json!({})).await;
    let own = call(
        &backend,
        "send_message",
        json!({"to": ["backend/sub"], "body": "take the parser"}),
    )
    .await;
    let unfiltered = json!({"timeout_s": 1});
    let woke = call(&backend, "wait_for_events", unfiltered.clone()).await;
    let named: Vec<&Value> = woke["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "message.sent")
        .map(|event| &event["data"]["message"])
        .collect();
    assert_eq!(named, [&json!(read.to_string()), &own["id"]], "{woke}");
    assert_eq!(woke.get("messages"), None, "{woke}");
    assert_eq!(woke.get("hint"), None, "{woke}");
    let unread = send_as_human(&fx, "backend").await;
    let woke = call(&backend, "wait_for_events", unfiltered).await;
    assert_eq!(woke["messages"], json!([unread.to_string()]));
    assert_eq!(
        woke["hint"],
        json!("a message waits for you; call get_inbox")
    );
}

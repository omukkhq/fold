use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    AgentName, EventActor, EventType, NewEvent, TaskDescription, TaskId, TaskTitle, Timestamp,
};
use fold_daemon::mcp::FoldServer;
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{EventBuffer, EventBus, EventService, WaitQuery, WaitStep};
use fold_daemon::services::tasks::{CreateTask, TaskService};
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
    agent_id: i64,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        Self::with_bus(EventBus::new()).await
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

    /// An adapter session of agent `name` over an in-memory stream, already registered and caught up with every stored event.
    async fn connect(&self, name: &str) -> Session {
        let events = EventService::new(self.store().clone(), self.bus.clone());
        let context = self.agents.start_session(7).await.unwrap();
        let server = FoldServer::new(self.agents.clone(), events, context.clone());
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
            agent_id: 0,
        };
        call(&session, "register_agent", json!({"name": name})).await;
        let agent =
            fold_store::agents::find_agent(self.store().read(), &AgentName::parse(name).unwrap())
                .await
                .unwrap()
                .unwrap();
        session.context.agent = Some(agent.name);
        session.agent_id = agent.id;
        while !call(&session, "get_events", json!({"limit": 500})).await["events"]
            .as_array()
            .unwrap()
            .is_empty()
        {}
        session
    }

    async fn create_task(&self) {
        self.tasks
            .create_task(
                &EventActor::parse("human:ana").unwrap(),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new(""),
                    labels: Vec::new(),
                },
            )
            .await
            .unwrap();
    }

    /// Appends `batch` in one write and publishes it after the commit, as a command does.
    async fn commit(&self, batch: Vec<NewEvent>) -> Vec<u64> {
        commit(self.store(), &self.bus, self.project.now(), batch).await
    }

    async fn cursor(&self, session: &Session) -> u64 {
        events::cursor(self.store().read(), session.agent_id)
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
            new_event(types::MESSAGE_SENT, "human:ana")
                .data(json!({"message": "m-1"}))
                .unwrap(),
            new_event(types::MESSAGE_SENT, "human:ana")
                .data(json!({"message": "m-2"}))
                .unwrap(),
            new_event(types::MESSAGE_ACKNOWLEDGED, "backend")
                .data(json!({"message": "m-1"}))
                .unwrap(),
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
    let server = FoldServer::new(fx.agents.clone(), events, context);
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
    fx.tasks.claim_task(&main, t(1)).await.unwrap();
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
        .within(events.wait(session.agent_id, &query))
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
    fx.tasks.claim_task(&main, t(1)).await.unwrap();
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

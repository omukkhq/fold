use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    Actor, Address, AddressedAs, AgentActor, AgentName, EventFilter, FoldError, HumanName,
    MessageBody, MessageId, Sender, Tag, TaskDescription, TaskId, TaskTitle, WorkerName, codes,
};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::messages::{
    Feedback, MessageError, MessageService, SendMessage, ThreadQuery, unread_count,
};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_store::testing::TestProject;
use fold_store::{events, projects};
use sqlx::Row;

#[cfg(test)]
fn m(number: u32) -> MessageId {
    MessageId::from_number(number).unwrap()
}

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
fn human(name: &str) -> Actor {
    Actor::human(HumanName::parse(name).unwrap())
}

#[cfg(test)]
fn body(text: &str) -> MessageBody {
    MessageBody::parse(text).unwrap()
}

#[cfg(test)]
fn message(to: &[&str], text: &str) -> SendMessage {
    SendMessage {
        to: Address::parse_all(to).unwrap(),
        body: body(text),
        task: None,
        proposal: None,
        reply_to: None,
    }
}

#[cfg(test)]
fn refusal(error: MessageError) -> FoldError {
    match error {
        MessageError::Fold(error) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
    messages: MessageService,
}

#[cfg(test)]
impl Fixture {
    /// Agents `backend`, `frontend` with child worker `frontend/ui`, and `review`; humans `ana` and `tanim`; tasks `T-1` and `T-2`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock.clone(), 5);
        let messages = MessageService::new(
            project.store().clone(),
            project.registry().clone(),
            bus,
            clock,
        );
        let frontend = register(&agents, "frontend").await;
        agents.register_worker(&frontend, "ui", None).await.unwrap();
        register(&agents, "backend").await;
        register(&agents, "review").await;
        for name in ["tanim", "ana"] {
            let now = project.now();
            let name = HumanName::parse(name).unwrap();
            project
                .registry()
                .write(async |tx| projects::insert_human(tx, &name, now).await)
                .await
                .unwrap();
        }
        for _ in 0..2 {
            crate::common::roles::lead_work_areas(project.store()).await;
            tasks
                .create_task(
                    &human("ana"),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new(""),
                        labels: crate::common::roles::work_labels(),
                    },
                )
                .await
                .unwrap();
        }
        Self {
            project,
            tasks,
            messages,
        }
    }

    async fn send(&self, from: &str, to: &[&str]) -> MessageId {
        self.messages
            .send(&agent(from), message(to, "text"))
            .await
            .unwrap()
            .id
    }

    async fn agent_id(&self, name: &str) -> i64 {
        fold_store::agents::find_agent(
            self.project.store().read(),
            &AgentName::parse(name).unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
        .id
    }

    /// `(delivered_at, acked_at)` of the row of `message` for the worker `worker`.
    async fn row(&self, message: MessageId, worker: &str) -> (Option<i64>, Option<i64>) {
        let row = sqlx::query(
            "SELECT r.delivered_at, r.acked_at FROM message_recipients r
             JOIN messages m ON m.id = r.message_id JOIN workers w ON w.id = r.worker_id
             WHERE m.number = ? AND w.name = ?",
        )
        .bind(message.to_db())
        .bind(worker)
        .fetch_one(self.project.store().read())
        .await
        .unwrap();
        (row.get(0), row.get(1))
    }
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> SessionContext {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(
            &mut ctx,
            &AgentName::parse(name).unwrap(),
            &[Tag::parse("team").unwrap()],
        )
        .await
        .unwrap();
    ctx
}

#[tokio::test]
async fn the_inbox_marks_exactly_the_rows_it_returns_and_keeps_the_first_delivery_time() {
    let fixture = Fixture::new().await;
    let first = fixture.send("frontend", &["backend"]).await;
    let other = fixture.send("frontend", &["review"]).await;
    let delivered = fixture.project.now().as_millis();
    let inbox = fixture
        .messages
        .inbox(&agent("backend"), false)
        .await
        .unwrap();
    assert_eq!(
        inbox.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![first]
    );
    assert_eq!(inbox[0].delivered_at.as_millis(), delivered);
    assert_eq!(inbox[0].addressed_as, AddressedAs::Agent);
    assert_eq!(
        inbox[0].sender,
        Sender::Worker(WorkerName::parse("frontend").unwrap())
    );
    assert_eq!(fixture.row(first, "backend").await, (Some(delivered), None));
    assert_eq!(fixture.row(other, "review").await, (None, None));
    fixture
        .project
        .clock()
        .advance(Duration::from_secs(60))
        .unwrap();
    let again = fixture
        .messages
        .inbox(&agent("backend"), false)
        .await
        .unwrap();
    assert_eq!(again.len(), 1, "reading never acknowledges");
    assert_eq!(again[0].delivered_at.as_millis(), delivered);
    assert_eq!(fixture.row(first, "backend").await, (Some(delivered), None));
}

#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_failed_inbox_read_leaves_no_row_marked_delivered() {
    let fixture = Fixture::new().await;
    let first = fixture.send("frontend", &["backend"]).await;
    let scenario = fail::FailScenario::setup();
    fail::cfg("inbox_after_delivery", "return").unwrap();
    let error = refusal(
        fixture
            .messages
            .inbox(&agent("backend"), false)
            .await
            .unwrap_err(),
    );
    fail::remove("inbox_after_delivery");
    scenario.teardown();
    assert_eq!(error.code(), codes::FAILPOINT);
    assert_eq!(
        fixture.row(first, "backend").await,
        (None, None),
        "the marking rolled back with the read"
    );
}

#[tokio::test]
async fn include_workers_reads_every_worker_of_the_agent_and_as_reads_one() {
    let fixture = Fixture::new().await;
    let main = fixture.send("backend", &["frontend"]).await;
    let child = fixture.send("backend", &["frontend/ui"]).await;
    let both = fixture.send("backend", &["frontend", "frontend/ui"]).await;
    let names = |items: &[fold_daemon::services::messages::InboxItem]| {
        items
            .iter()
            .map(|item| (item.id, item.to.to_string()))
            .collect::<Vec<_>>()
    };
    let own = fixture
        .messages
        .inbox(&agent("frontend"), false)
        .await
        .unwrap();
    assert_eq!(
        names(&own),
        vec![(main, "frontend".into()), (both, "frontend".into())]
    );
    let ui = fixture
        .messages
        .inbox(&agent("frontend/ui"), false)
        .await
        .unwrap();
    assert_eq!(
        names(&ui),
        vec![(child, "frontend/ui".into()), (both, "frontend/ui".into())]
    );
    let all = fixture
        .messages
        .inbox(&agent("frontend"), true)
        .await
        .unwrap();
    assert_eq!(
        names(&all),
        vec![
            (main, "frontend".into()),
            (child, "frontend/ui".into()),
            (both, "frontend".into()),
            (both, "frontend/ui".into()),
        ]
    );
}

#[tokio::test]
async fn acknowledge_sets_one_row_or_every_earlier_row_and_is_idempotent() {
    let fixture = Fixture::new().await;
    let a = fixture.send("frontend", &["backend"]).await;
    let b = fixture.send("frontend", &["backend"]).await;
    let c = fixture.send("frontend", &["backend", "review"]).await;
    let d = fixture.send("frontend", &["backend"]).await;
    let backend = agent("backend");
    let at = fixture.project.now().as_millis();
    assert_eq!(
        fixture
            .messages
            .acknowledge(&backend, b, false)
            .await
            .unwrap(),
        vec![b]
    );
    assert_eq!(fixture.row(b, "backend").await.1, Some(at));
    assert_eq!(fixture.row(a, "backend").await.1, None);
    fixture
        .project
        .clock()
        .advance(Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        fixture
            .messages
            .acknowledge(&backend, b, false)
            .await
            .unwrap(),
        Vec::<MessageId>::new()
    );
    assert_eq!(
        fixture.row(b, "backend").await.1,
        Some(at),
        "an acknowledged row keeps its time"
    );
    let later = fixture.project.now().as_millis();
    assert_eq!(
        fixture
            .messages
            .acknowledge(&backend, c, true)
            .await
            .unwrap(),
        vec![a, c]
    );
    assert_eq!(fixture.row(a, "backend").await.1, Some(later));
    assert_eq!(fixture.row(c, "backend").await.1, Some(later));
    assert_eq!(
        fixture.row(c, "review").await.1,
        None,
        "another recipient's row is untouched"
    );
    assert_eq!(fixture.row(d, "backend").await.1, None);
    let left = fixture.messages.inbox(&backend, false).await.unwrap();
    assert_eq!(left.iter().map(|item| item.id).collect::<Vec<_>>(), vec![d]);
}

#[tokio::test]
async fn a_recipient_cannot_acknowledge_another_recipients_row() {
    let fixture = Fixture::new().await;
    let id = fixture.send("frontend", &["backend"]).await;
    for (actor, up_to) in [
        (agent("review"), false),
        (agent("review"), true),
        (agent("frontend"), false),
    ] {
        let error = refusal(
            fixture
                .messages
                .acknowledge(&actor, id, up_to)
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), codes::MESSAGE_NOT_FOUND);
        assert_eq!(error.hint(), Some("call get_inbox to see your messages"));
    }
    let error = refusal(
        fixture
            .messages
            .acknowledge(&agent("backend"), m(99), false)
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::MESSAGE_NOT_FOUND);
    assert_eq!(fixture.row(id, "backend").await.1, None);
}

#[tokio::test]
async fn the_acknowledged_event_names_its_newest_message_so_it_reaches_the_recipient() {
    let fixture = Fixture::new().await;
    let a = fixture.send("frontend", &["backend"]).await;
    let b = fixture.send("frontend", &["backend"]).await;
    fixture
        .messages
        .acknowledge(&agent("backend"), b, true)
        .await
        .unwrap();
    fixture
        .messages
        .acknowledge(&agent("backend"), b, true)
        .await
        .unwrap();
    let read = fixture.project.store().read();
    let stored = events::read_after(
        read,
        0,
        &EventFilter {
            types: vec![types::MESSAGE_ACKNOWLEDGED],
            ..EventFilter::default()
        },
        100,
    )
    .await
    .unwrap();
    assert_eq!(stored.len(), 1, "a repeat appends nothing");
    assert_eq!(stored[0].actor.as_str(), "backend");
    assert_eq!(
        stored[0].data,
        serde_json::json!({"ids": [a.to_string(), b.to_string()]})
    );
    let column: i64 = sqlx::query_scalar("SELECT message_id FROM events WHERE seq = ?")
        .bind(i64::try_from(stored[0].seq).unwrap())
        .fetch_one(read)
        .await
        .unwrap();
    assert_eq!(column, b.to_db());
    let backend = fixture.agent_id("backend").await;
    assert!(
        events::is_recipient(read, stored[0].seq, backend)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn unread_count_counts_every_worker_of_the_agent() {
    let fixture = Fixture::new().await;
    let store = fixture.project.store();
    let frontend = fixture.agent_id("frontend").await;
    assert_eq!(unread_count(store, frontend).await.unwrap(), 0);
    fixture.send("backend", &["frontend"]).await;
    let child = fixture.send("backend", &["frontend/ui", "review"]).await;
    assert_eq!(unread_count(store, frontend).await.unwrap(), 2);
    fixture
        .messages
        .inbox(&agent("frontend"), true)
        .await
        .unwrap();
    assert_eq!(
        unread_count(store, frontend).await.unwrap(),
        2,
        "delivery is not acknowledgement"
    );
    fixture
        .messages
        .acknowledge(&agent("frontend/ui"), child, false)
        .await
        .unwrap();
    assert_eq!(fixture.messages.unread_count(frontend).await.unwrap(), 1);
    let review = fixture.agent_id("review").await;
    assert_eq!(unread_count(store, review).await.unwrap(), 1);
}

#[tokio::test]
async fn only_a_registered_worker_has_an_inbox() {
    let fixture = Fixture::new().await;
    let error = refusal(
        fixture
            .messages
            .inbox(&human("ana"), false)
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    let error = refusal(
        fixture
            .messages
            .acknowledge(&Actor::System, m(1), false)
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    for name in ["ghost", "frontend/ghost"] {
        let error = refusal(
            fixture
                .messages
                .inbox(&agent(name), false)
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), codes::WORKER_NOT_FOUND);
    }
}

#[tokio::test]
async fn the_inbox_query_uses_the_worker_and_acked_index() {
    let fixture = Fixture::new().await;
    let plan: Vec<String> = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT r.id FROM message_recipients r
         JOIN workers rw ON rw.id = r.worker_id
         JOIN messages m ON m.id = r.message_id
         WHERE r.worker_id IN (SELECT value FROM json_each('[1,2]')) AND r.acked_at IS NULL
         ORDER BY m.number, rw.id",
    )
    .fetch_all(fixture.project.store().read())
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>("detail"))
    .collect();
    assert!(
        plan.iter()
            .any(|line| line.contains("message_recipients_worker_acked")
                && line.contains("worker_id=? AND acked_at=?")),
        "{plan:?}"
    );
    let count: Vec<String> = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT COUNT(*) FROM workers w JOIN message_recipients r ON r.worker_id = w.id
         WHERE w.agent_id = 1 AND r.acked_at IS NULL",
    )
    .fetch_all(fixture.project.store().read())
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>("detail"))
    .collect();
    assert!(
        count
            .iter()
            .any(|line| line.contains("message_recipients_worker_acked")),
        "{count:?}"
    );
}

#[tokio::test]
async fn feedback_needs_a_task_or_a_reply_and_only_a_human_sends_it() {
    let fixture = Fixture::new().await;
    let feedback = |task, reply_to| Feedback {
        body: body("x"),
        task,
        request: None,
        reply_to,
        to: None,
    };
    let error = refusal(
        fixture
            .messages
            .feedback(&human("ana"), feedback(None, None))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert!(error.hint().is_some(), "{error:?}");
    let error = refusal(
        fixture
            .messages
            .feedback(&agent("backend"), feedback(Some(t(1)), None))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    let error = refusal(
        fixture
            .messages
            .feedback(&human("ana"), feedback(None, Some(m(9))))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::MESSAGE_NOT_FOUND);
    let error = refusal(
        fixture
            .messages
            .feedback(&human("ana"), feedback(Some(t(1)), None))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        error.code(),
        codes::RECIPIENT_NOT_FOUND,
        "T-1 has no claim holder"
    );
    fixture
        .tasks
        .claim_task(&agent("frontend/ui"), t(1))
        .await
        .unwrap();
    let sent = fixture
        .messages
        .feedback(&human("ana"), feedback(Some(t(1)), None))
        .await
        .unwrap();
    assert_eq!(sent.recipients[0].name.to_string(), "frontend/ui");
    assert_eq!(sent.task, Some(t(1)));
}

#[tokio::test]
async fn a_request_message_keeps_the_task_it_names_and_only_a_human_sends_it() {
    let fixture = Fixture::new().await;
    let request = fixture
        .tasks
        .create_request(&human("ana"), "Make the board faster.", Vec::new())
        .await
        .unwrap();
    let feedback = Feedback {
        body: body("It is the main board."),
        task: Some(t(2)),
        request: Some(request.id),
        reply_to: None,
        to: Some(Address::parse_all(&["backend"]).unwrap()),
    };
    let error = refusal(
        fixture
            .messages
            .feedback(&agent("frontend"), feedback.clone())
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    let sent = fixture
        .messages
        .feedback(&human("tanim"), feedback)
        .await
        .unwrap();
    assert_eq!(sent.request, Some(request.id));
    assert_eq!(sent.task, Some(t(2)));
    assert_eq!(sent.recipients[0].name.to_string(), "backend");
}

#[tokio::test]
async fn a_reply_goes_to_the_parent_sender_and_inherits_its_task() {
    let fixture = Fixture::new().await;
    let question = fixture
        .messages
        .send(
            &agent("frontend/ui"),
            SendMessage {
                task: Some(t(2)),
                ..message(&["humans"], "which one?")
            },
        )
        .await
        .unwrap();
    let reply = fixture
        .messages
        .feedback(
            &human("tanim"),
            Feedback {
                body: body("the first"),
                task: None,
                request: None,
                reply_to: Some(question.id),
                to: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(reply.thread, question.id);
    assert_eq!(reply.reply_to, Some(question.id));
    assert_eq!(reply.task, Some(t(2)));
    assert_eq!(
        reply
            .recipients
            .iter()
            .map(|r| r.name.to_string())
            .collect::<Vec<_>>(),
        vec!["frontend/ui"]
    );
    let follow_up = fixture
        .messages
        .feedback(
            &human("tanim"),
            Feedback {
                body: body("and also"),
                task: Some(t(1)),
                request: None,
                reply_to: Some(reply.id),
                to: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(follow_up.task, Some(t(1)));
    assert_eq!(
        follow_up
            .recipients
            .iter()
            .map(|r| r.name.to_string())
            .collect::<Vec<_>>(),
        vec!["frontend/ui"],
        "a reply to your own message goes to its other recipients"
    );
    let named = fixture
        .messages
        .feedback(
            &human("tanim"),
            Feedback {
                body: body("fyi"),
                task: None,
                request: None,
                reply_to: Some(question.id),
                to: Some(Address::parse_all(&["backend"]).unwrap()),
            },
        )
        .await
        .unwrap();
    assert_eq!(named.recipients[0].name.to_string(), "backend");
}

#[tokio::test]
async fn threads_list_newest_activity_first_with_filters_and_pages() {
    let fixture = Fixture::new().await;
    let first = fixture
        .messages
        .send(
            &agent("frontend"),
            SendMessage {
                task: Some(t(1)),
                ..message(&["humans"], "q")
            },
        )
        .await
        .unwrap()
        .id;
    let second = fixture.send("backend", &["review"]).await;
    let third = fixture.send("review", &["frontend/ui"]).await;
    let reply = fixture
        .messages
        .send(
            &agent("backend"),
            SendMessage {
                reply_to: Some(first),
                ..message(&["frontend"], "a")
            },
        )
        .await
        .unwrap()
        .id;
    let page = fixture
        .messages
        .threads(&ThreadQuery::default())
        .await
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|thread| thread.id)
            .collect::<Vec<_>>(),
        vec![first, third, second]
    );
    assert_eq!(page.next, None);
    let thread = &page.items[0];
    assert_eq!(thread.task, Some(t(1)));
    assert!(
        thread.awaiting_human,
        "an agent's reply does not answer a question to humans"
    );
    assert_eq!(
        thread.participants,
        vec!["frontend", "human:ana", "human:tanim", "backend"]
    );
    assert_eq!(
        thread.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![first, reply]
    );
    assert!(!page.items[1].awaiting_human);

    let query = |agent: Option<&str>, task: Option<&str>, awaiting: bool| ThreadQuery {
        agent: agent.map(str::to_owned),
        task: task.map(str::to_owned),
        awaiting_human: awaiting,
        ..ThreadQuery::default()
    };
    let ids = async |query: ThreadQuery| {
        fixture
            .messages
            .threads(&query)
            .await
            .unwrap()
            .items
            .iter()
            .map(|thread| thread.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(query(Some("frontend"), None, false)).await,
        vec![first, third]
    );
    assert_eq!(
        ids(query(Some("frontend/ui"), None, false)).await,
        vec![third]
    );
    assert_eq!(
        ids(query(Some("review"), None, false)).await,
        vec![third, second]
    );
    assert_eq!(ids(query(None, Some("T-1"), false)).await, vec![first]);
    assert_eq!(
        ids(query(None, Some("T-2"), false)).await,
        Vec::<MessageId>::new()
    );
    assert_eq!(ids(query(None, None, true)).await, vec![first]);

    let one = ThreadQuery {
        limit: Some(1),
        ..ThreadQuery::default()
    };
    let page = fixture.messages.threads(&one).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next, Some(reply));
    let rest = fixture
        .messages
        .threads(&ThreadQuery {
            before: page.next.map(|id| id.to_string()),
            ..ThreadQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        rest.items
            .iter()
            .map(|thread| thread.id)
            .collect::<Vec<_>>(),
        vec![third, second]
    );

    for (bad, code) in [
        (
            ThreadQuery {
                limit: Some(0),
                ..ThreadQuery::default()
            },
            codes::INVALID_REQUEST,
        ),
        (query(None, Some("12"), false), codes::INVALID_ID),
        (query(Some("Bad Name"), None, false), codes::INVALID_NAME),
        (
            ThreadQuery {
                before: Some("x".into()),
                ..ThreadQuery::default()
            },
            codes::INVALID_ID,
        ),
    ] {
        let error = refusal(fixture.messages.threads(&bad).await.unwrap_err());
        assert_eq!(error.code(), code);
        if code == codes::INVALID_REQUEST {
            assert!(error.hint().is_some(), "{error:?}");
        }
    }
    let view = fixture.messages.thread_view(reply).await.unwrap();
    assert_eq!(view.id, first);
    let error = refusal(fixture.messages.thread_view(m(99)).await.unwrap_err());
    assert_eq!(error.code(), codes::MESSAGE_NOT_FOUND);
}

#[tokio::test]
async fn messages_never_change_task_state() {
    let fixture = Fixture::new().await;
    let before: Vec<(i64, String)> =
        sqlx::query_as("SELECT number, status FROM tasks ORDER BY number")
            .fetch_all(fixture.project.store().read())
            .await
            .unwrap();
    let id = fixture.send("frontend", &["backend"]).await;
    fixture
        .messages
        .inbox(&agent("backend"), false)
        .await
        .unwrap();
    fixture
        .messages
        .acknowledge(&agent("backend"), id, true)
        .await
        .unwrap();
    let after: Vec<(i64, String)> =
        sqlx::query_as("SELECT number, status FROM tasks ORDER BY number")
            .fetch_all(fixture.project.store().read())
            .await
            .unwrap();
    assert_eq!(before, after);
}

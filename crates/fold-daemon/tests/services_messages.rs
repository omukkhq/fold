use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    Actor, Address, AddressedAs, AgentActor, AgentName, Event, EventFilter, FoldError, HumanName,
    MessageBody, MessageId, ProposalId, Sender, Tag, Target, TaskDescription, TaskId, TaskTitle,
    Timestamp, WorkerName, codes,
};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_daemon::services::messages::{MessageError, MessageService, MessageView, SendMessage};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_store::testing::TestProject;
use fold_store::{StoreError, events, projects};
use serde_json::json;
use sqlx::Row;

use crate::common::awaiting_human;

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn m(number: u32) -> MessageId {
    MessageId::from_number(number).unwrap()
}

#[cfg(test)]
fn worker(name: &str) -> WorkerName {
    WorkerName::parse(name).unwrap()
}

#[cfg(test)]
fn agent(name: &str) -> Actor {
    let worker = worker(name);
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
fn to(entries: &[&str]) -> Vec<Address> {
    Address::parse_all(entries).unwrap()
}

#[cfg(test)]
fn body(text: &str) -> MessageBody {
    MessageBody::parse(text).unwrap()
}

#[cfg(test)]
fn message(entries: &[&str], text: &str) -> SendMessage {
    SendMessage {
        to: to(entries),
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
fn details(error: &FoldError) -> serde_json::Value {
    error
        .details()
        .cloned()
        .map_or(serde_json::Value::Null, serde_json::Value::Object)
}

/// `(name, addressed_as)` of every recipient, in row order.
#[cfg(test)]
fn recipients(view: &MessageView) -> Vec<(String, AddressedAs)> {
    view.recipients
        .iter()
        .map(|recipient| (recipient.name.to_string(), recipient.addressed_as))
        .collect()
}

#[cfg(test)]
fn pairs(list: &[(&str, AddressedAs)]) -> Vec<(String, AddressedAs)> {
    list.iter()
        .map(|(name, addressed_as)| ((*name).to_owned(), *addressed_as))
        .collect()
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    agents: AgentService,
    tasks: TaskService,
    messages: MessageService,
    frontend: SessionContext,
}

#[cfg(test)]
impl Fixture {
    /// Agents `backend` (tag review), `frontend` (tag ui) with child worker `frontend/ui`, and `review` (tag review); humans `ana` and `tanim`; tasks `T-1` and `T-2`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock.clone(), 5);
        let messages = MessageService::new(
            project.store().clone(),
            project.registry().clone(),
            bus.clone(),
            clock,
        );
        let frontend = register(&agents, "frontend", &["ui"]).await;
        agents.register_worker(&frontend, "ui", None).await.unwrap();
        register(&agents, "backend", &["review"]).await;
        register(&agents, "review", &["review"]).await;
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
                    &agent("frontend"),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("work"),
                        labels: crate::common::roles::work_labels(),
                    },
                )
                .await
                .unwrap();
        }
        Self {
            project,
            bus,
            agents,
            tasks,
            messages,
            frontend,
        }
    }

    async fn message_events(&self) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            0,
            &EventFilter {
                types: vec![types::MESSAGE_SENT],
                ..EventFilter::default()
            },
            1000,
        )
        .await
        .unwrap()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
            .get::<i64, _>(0)
    }

    async fn insert_proposal(&self, number: u32, task: u32) {
        let sql = format!(
            "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
             SELECT {number}, t.id, w.id, 'proposal', 0, 0 FROM tasks t, workers w
             WHERE t.number = {task} AND w.name = 'frontend'"
        );
        self.project
            .store()
            .write(async |tx| {
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(tx.conn())
                    .await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str, tags: &[&str]) -> SessionContext {
    let mut ctx = agents.start_session(1).await.unwrap();
    let tags: Vec<Tag> = tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &tags)
        .await
        .unwrap();
    ctx
}

/// `(recipient name, addressed_as)` pairs in row order.
#[cfg(test)]
type Expected = &'static [(&'static str, AddressedAs)];

#[tokio::test]
async fn every_address_form_writes_its_recipient_rows() {
    let fixture = Fixture::new().await;
    let sender = agent("frontend");
    let table: [(&[&str], Expected); 6] = [
        (&["backend"], &[("backend", AddressedAs::Agent)]),
        (&["frontend/ui"], &[("frontend/ui", AddressedAs::Worker)]),
        (
            &["tag:review"],
            &[("backend", AddressedAs::Tag), ("review", AddressedAs::Tag)],
        ),
        (
            &["all"],
            &[("backend", AddressedAs::All), ("review", AddressedAs::All)],
        ),
        (&["human:tanim"], &[("human:tanim", AddressedAs::Human)]),
        (
            &["humans"],
            &[
                ("human:ana", AddressedAs::Humans),
                ("human:tanim", AddressedAs::Humans),
            ],
        ),
    ];
    for (number, (entries, expected)) in (1..).zip(table) {
        let view = fixture
            .messages
            .send(&sender, message(entries, "hi"))
            .await
            .unwrap();
        assert_eq!(view.id, m(number));
        assert_eq!(
            view.thread,
            m(number),
            "a message without reply_to starts a thread"
        );
        assert_eq!(view.reply_to, None);
        assert_eq!(view.sender, Sender::Worker(worker("frontend")));
        assert_eq!(view.sent_at, fixture.project.now());
        assert_eq!(recipients(&view), pairs(expected), "{entries:?}");
        let stored = fixture.messages.thread(view.id).await.unwrap();
        assert_eq!(stored, vec![view], "the returned view is what was stored");
    }
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM message_recipients WHERE human IS NOT NULL AND addressed_as = 'humans'")
            .await,
        2,
        "humans writes one row per registry human"
    );
}

#[tokio::test]
async fn a_tag_the_sender_carries_leaves_the_sender_out() {
    let fixture = Fixture::new().await;
    let view = fixture
        .messages
        .send(
            &agent("review"),
            message(&["tag:review", "frontend", "all"], "hi"),
        )
        .await
        .unwrap();
    assert_eq!(
        recipients(&view),
        pairs(&[
            ("backend", AddressedAs::Tag),
            ("frontend", AddressedAs::Agent),
        ])
    );
}

#[tokio::test]
async fn a_reply_joins_the_thread_and_inherits_task_and_proposal() {
    let fixture = Fixture::new().await;
    fixture.insert_proposal(4, 2).await;
    let first = fixture
        .messages
        .send(
            &agent("frontend"),
            SendMessage {
                task: Some(t(2)),
                proposal: Some(ProposalId::from_number(4).unwrap()),
                ..message(&["backend"], "question")
            },
        )
        .await
        .unwrap();
    fixture
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    let reply = fixture
        .messages
        .send(
            &agent("backend"),
            SendMessage {
                reply_to: Some(first.id),
                ..message(&["frontend"], "answer")
            },
        )
        .await
        .unwrap();
    assert_eq!(reply.thread, first.id);
    assert_eq!(reply.reply_to, Some(first.id));
    assert_eq!(reply.task, Some(t(2)));
    assert_eq!(reply.proposal, ProposalId::from_number(4));
    let nested = fixture
        .messages
        .send(
            &human("tanim"),
            SendMessage {
                reply_to: Some(reply.id),
                task: Some(t(1)),
                ..message(&["frontend", "backend"], "a human joins")
            },
        )
        .await
        .unwrap();
    assert_eq!(
        nested.thread, first.id,
        "a reply to a reply stays in the root thread"
    );
    assert_eq!(nested.reply_to, Some(reply.id));
    assert_eq!(nested.task, Some(t(1)), "a task given on a reply wins");
    assert_eq!(
        nested.sender,
        Sender::Human(HumanName::parse("tanim").unwrap())
    );
    let unrelated = fixture
        .messages
        .send(&agent("frontend"), message(&["review"], "elsewhere"))
        .await
        .unwrap();
    for member in [first.id, reply.id, nested.id] {
        let thread = fixture.messages.thread(member).await.unwrap();
        let ids: Vec<MessageId> = thread.iter().map(|view| view.id).collect();
        assert_eq!(ids, [first.id, reply.id, nested.id]);
        assert_eq!(thread[1].body, "answer");
    }
    assert_eq!(
        fixture.messages.thread(unrelated.id).await.unwrap().len(),
        1
    );
    let error = refusal(fixture.messages.thread(m(99)).await.unwrap_err());
    assert_eq!(error.code(), codes::MESSAGE_NOT_FOUND);
}

#[tokio::test]
async fn unknown_references_are_refused_before_anything_is_written() {
    let fixture = Fixture::new().await;
    let cases = [
        (
            SendMessage {
                reply_to: Some(m(7)),
                ..message(&["backend"], "x")
            },
            codes::MESSAGE_NOT_FOUND,
        ),
        (
            SendMessage {
                task: Some(t(9)),
                ..message(&["backend"], "x")
            },
            codes::TASK_NOT_FOUND,
        ),
        (
            SendMessage {
                proposal: ProposalId::from_number(3),
                ..message(&["backend"], "x")
            },
            codes::PROPOSAL_NOT_FOUND,
        ),
    ];
    for (input, code) in cases {
        let error = fixture
            .messages
            .send(&agent("frontend"), input)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some(code));
    }
    let error = fixture
        .messages
        .send(&agent("frontend/ghost"), message(&["backend"], "x"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::WORKER_NOT_FOUND));
    let error = fixture
        .messages
        .send(&agent("ghost"), message(&["backend"], "x"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::WORKER_NOT_FOUND));
    assert_eq!(fixture.count("SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM message_recipients")
            .await,
        0
    );
    assert!(fixture.message_events().await.is_empty());
}

#[tokio::test]
async fn recipient_not_found_names_the_unknown_entry_or_every_entry() {
    let fixture = Fixture::new().await;
    for entry in ["ghost", "frontend/ghost"] {
        let error = refusal(
            fixture
                .messages
                .send(&agent("frontend"), message(&["backend", entry], "x"))
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), codes::RECIPIENT_NOT_FOUND);
        assert_eq!(details(&error), json!({"to": [entry]}));
    }
    let error = refusal(
        fixture
            .messages
            .send(&agent("frontend"), message(&["tag:nobody", "tag:ui"], "x"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::RECIPIENT_NOT_FOUND);
    assert_eq!(details(&error), json!({"to": ["tag:nobody", "tag:ui"]}));
    let error = refusal(
        fixture
            .messages
            .send(&agent("frontend"), message(&["human:bob"], "x"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::UNKNOWN_HUMAN);
    assert_eq!(details(&error), json!({"to": ["human:bob"]}));
    assert_eq!(fixture.count("SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(
        fixture
            .count("SELECT next FROM counters WHERE kind = 'message' UNION ALL SELECT 0 LIMIT 1")
            .await,
        0
    );
}

#[tokio::test]
async fn humans_with_no_other_human_reaches_nobody() {
    let project = TestProject::new().await;
    let clock = Arc::new(project.clock().clone());
    let messages = MessageService::new(
        project.store().clone(),
        project.registry().clone(),
        EventBus::new(),
        clock,
    );
    let error = refusal(
        messages
            .send(&human("tanim"), message(&["humans"], "anyone?"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::RECIPIENT_NOT_FOUND);
    assert_eq!(details(&error), json!({"to": ["humans"]}));
}

#[tokio::test]
async fn the_system_sends_as_fold() {
    let fixture = Fixture::new().await;
    let view = fixture
        .messages
        .send(&Actor::System, message(&["all"], "main moved"))
        .await
        .unwrap();
    assert_eq!(view.sender, Sender::Fold);
    assert_eq!(view.recipients.len(), 3, "the system is in no expansion");
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM messages WHERE sender_kind = 'fold' AND sender_worker_id IS NULL AND sender_human IS NULL")
            .await,
        1
    );
    assert_eq!(fixture.message_events().await[0].actor.as_str(), "fold");
}

#[tokio::test]
async fn feedback_goes_to_the_claim_holder_by_default() {
    let fixture = Fixture::new().await;
    // `T-1` is in the area that `frontend` leads, so `backend` claims it as the main agent.
    crate::common::roles::give_main_tag(fixture.project.store(), "backend").await;
    let ui = agent("frontend/ui");
    fixture.tasks.claim_task(&ui, t(1)).await.unwrap();
    let view = fixture
        .messages
        .send_feedback(&human("tanim"), t(1), body("please add tests"), None)
        .await
        .unwrap();
    assert_eq!(view.task, Some(t(1)));
    assert_eq!(view.thread, view.id);
    assert_eq!(
        view.sender,
        Sender::Human(HumanName::parse("tanim").unwrap())
    );
    assert_eq!(
        recipients(&view),
        pairs(&[("frontend/ui", AddressedAs::Worker)])
    );
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM messages m JOIN tasks t ON t.id = m.task_id WHERE t.number = 1 AND m.sender_kind = 'human' AND m.sender_human = 'tanim'")
            .await,
        1
    );
    let events = fixture.message_events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].task, Some(t(1)));
    assert_eq!(events[0].actor.as_str(), "human:tanim");

    fixture.tasks.release_task(&ui, t(1)).await.unwrap();
    let view = fixture
        .messages
        .send_feedback(&human("ana"), t(1), body("ping"), None)
        .await
        .unwrap();
    assert_eq!(
        recipients(&view),
        pairs(&[("frontend", AddressedAs::Agent)]),
        "after the release the area lead reads it"
    );
    fixture
        .tasks
        .claim_task(&agent("backend"), t(1))
        .await
        .unwrap();
    let view = fixture
        .messages
        .send_feedback(&human("ana"), t(1), body("ping"), None)
        .await
        .unwrap();
    assert_eq!(recipients(&view), pairs(&[("backend", AddressedAs::Agent)]));
}

#[tokio::test]
async fn feedback_takes_a_chosen_reader_and_needs_one() {
    let fixture = Fixture::new().await;
    let view = fixture
        .messages
        .send_feedback(
            &human("tanim"),
            t(2),
            body("for the lead"),
            Some(to(&["frontend"])),
        )
        .await
        .unwrap();
    assert_eq!(
        recipients(&view),
        pairs(&[("frontend", AddressedAs::Agent)])
    );
    let error = refusal(
        fixture
            .messages
            .send_feedback(
                &human("tanim"),
                t(2),
                body("for reviewers"),
                Some(to(&["frontend", "tag:review"])),
            )
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::RECIPIENT_NOT_READER);
    assert_eq!(
        details(&error),
        json!({"task": "T-2", "to": ["tag:review"], "readers": ["frontend"]})
    );
    assert!(error.hint().is_some());
    fixture
        .tasks
        .create_task(
            &agent("frontend"),
            CreateTask {
                title: TaskTitle::parse("no area").unwrap(),
                description: TaskDescription::new(""),
                labels: vec![],
            },
        )
        .await
        .unwrap();
    let error = refusal(
        fixture
            .messages
            .send_feedback(&human("tanim"), t(3), body("anyone?"), None)
            .await
            .unwrap_err(),
    );
    assert_eq!(
        error.code(),
        codes::RECIPIENT_NOT_FOUND,
        "a task in no area of a project without a main agent has no reader"
    );
    assert_eq!(details(&error), json!({"task": "T-3"}));
    let error = fixture
        .messages
        .send_feedback(&human("tanim"), t(9), body("x"), None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::TASK_NOT_FOUND));
    for actor in [agent("frontend"), Actor::System] {
        let error = fixture
            .messages
            .send_feedback(&actor, t(2), body("x"), Some(to(&["backend"])))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some(codes::HUMAN_REQUIRED));
    }
}

#[tokio::test]
async fn awaiting_human_drops_an_ask_once_a_human_replies() {
    let fixture = Fixture::new().await;
    let question = fixture
        .messages
        .send_marked(
            &agent("frontend/ui"),
            SendMessage {
                task: Some(t(1)),
                ..message(&["human:tanim", "backend"], "which colour?")
            },
            None,
            None,
            true,
        )
        .await
        .unwrap();
    let broadcast = fixture
        .messages
        .send_marked(
            &agent("backend"),
            message(&["humans"], "anyone around?"),
            None,
            None,
            true,
        )
        .await
        .unwrap();
    fixture
        .messages
        .send(&agent("backend"), message(&["humans"], "the build passes"))
        .await
        .unwrap();
    fixture
        .messages
        .send(&agent("review"), message(&["frontend"], "not for humans"))
        .await
        .unwrap();
    fixture
        .messages
        .send(&human("ana"), message(&["human:tanim"], "human to human"))
        .await
        .unwrap();
    let waiting = awaiting_human(fixture.project.store()).await;
    let ids: Vec<MessageId> = waiting.iter().map(|view| view.id).collect();
    assert_eq!(ids, [question.id, broadcast.id]);
    assert_eq!(waiting[0].task, Some(t(1)));

    let follow_up = fixture
        .messages
        .send(
            &agent("backend"),
            SendMessage {
                reply_to: Some(question.id),
                ..message(&["frontend/ui"], "blue, I think")
            },
        )
        .await
        .unwrap();
    assert_eq!(follow_up.task, Some(t(1)));
    let ids: Vec<MessageId> = awaiting_human(fixture.project.store())
        .await
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(
        ids,
        [question.id, broadcast.id],
        "a worker reply answers nothing"
    );

    fixture
        .messages
        .send(
            &human("tanim"),
            SendMessage {
                reply_to: Some(follow_up.id),
                ..message(&["frontend/ui"], "green")
            },
        )
        .await
        .unwrap();
    let ids: Vec<MessageId> = awaiting_human(fixture.project.store())
        .await
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids, [broadcast.id]);
}

#[tokio::test]
async fn the_sent_event_names_recipients_and_never_holds_the_body() {
    let fixture = Fixture::new().await;
    let mut receiver = fixture.bus.subscribe();
    let first = fixture
        .messages
        .send(
            &agent("frontend/ui"),
            SendMessage {
                task: Some(t(2)),
                ..message(&["backend", "human:ana"], "SECRET-BODY-TEXT")
            },
        )
        .await
        .unwrap();
    let reply = fixture
        .messages
        .send(
            &agent("backend"),
            SendMessage {
                reply_to: Some(first.id),
                ..message(&["frontend/ui"], "SECRET-REPLY-TEXT")
            },
        )
        .await
        .unwrap();
    let stored = fixture.message_events().await;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].actor.as_str(), "frontend/ui");
    assert_eq!(stored[0].task, Some(t(2)));
    assert_eq!(stored[0].created_at, fixture.project.now());
    assert_eq!(
        stored[0].data,
        json!({"message": "m-1", "thread": "m-1", "reply_to": null, "recipients": ["backend", "human:ana"]})
    );
    assert_eq!(stored[1].task, Some(t(2)), "the reply inherits the task");
    assert_eq!(
        stored[1].data,
        json!({"message": reply.id.to_string(), "thread": "m-1", "reply_to": "m-1", "recipients": ["frontend/ui"]})
    );
    for event in &stored {
        let text = serde_json::to_string(event).unwrap();
        assert!(!text.contains("SECRET"), "{text}");
    }
    for expected in &stored {
        match tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap()
        {
            BusMessage::Durable(event) => assert_eq!(&*event, expected),
            BusMessage::Live(event) => panic!("unexpected live event {event:?}"),
        }
    }
    fixture
        .messages
        .send(&agent("frontend"), message(&["ghost"], "refused"))
        .await
        .unwrap_err();
    assert!(
        receiver.try_recv().is_err(),
        "a refused send publishes nothing"
    );
}

#[tokio::test]
async fn a_message_to_an_offline_agent_waits_undelivered() {
    let fixture = Fixture::new().await;
    fixture.agents.end_session(&fixture.frontend).await.unwrap();
    let view = fixture
        .messages
        .send(
            &agent("backend"),
            message(&["frontend", "frontend/ui"], "while you were out"),
        )
        .await
        .unwrap();
    assert!(
        view.recipients.iter().all(
            |recipient| recipient.delivered_at.is_none() && recipient.acknowledged_at.is_none()
        )
    );
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM message_recipients WHERE delivered_at IS NULL AND acked_at IS NULL")
            .await,
        2
    );
    let stored = fixture.messages.thread(view.id).await.unwrap();
    assert_eq!(stored[0].recipients, view.recipients);
    assert!(matches!(stored[0].recipients[0].name, Target::Worker(_)));
}

#[tokio::test]
async fn messages_change_no_task_claim_or_status() {
    let fixture = Fixture::new().await;
    // `T-1` is in the area that `frontend` leads, so `backend` claims it as the main agent.
    crate::common::roles::give_main_tag(fixture.project.store(), "backend").await;
    fixture
        .tasks
        .claim_task(&agent("backend"), t(1))
        .await
        .unwrap();
    let snapshot = async || {
        let tasks = fixture.tasks.list(true).await.unwrap();
        let claims = fixture
            .count("SELECT COUNT(*) FROM claims WHERE released_at IS NULL")
            .await;
        let other_events = fixture
            .count("SELECT COUNT(*) FROM events WHERE type <> 'message.sent'")
            .await;
        (tasks, claims, other_events)
    };
    let before = snapshot().await;
    fixture
        .project
        .clock()
        .advance(Duration::from_secs(60))
        .unwrap();
    fixture
        .messages
        .send(
            &agent("frontend"),
            SendMessage {
                task: Some(t(1)),
                ..message(&["backend"], "T-2 is done, start T-1")
            },
        )
        .await
        .unwrap();
    fixture
        .messages
        .send_feedback(&human("tanim"), t(1), body("ship it"), None)
        .await
        .unwrap();
    fixture
        .messages
        .send_feedback(
            &human("tanim"),
            t(2),
            body("claim this"),
            Some(to(&["frontend"])),
        )
        .await
        .unwrap();
    assert_eq!(snapshot().await, before);
    assert_eq!(fixture.message_events().await.len(), 3);
}

#[tokio::test]
async fn a_message_time_comes_from_the_project_clock() {
    let fixture = Fixture::new().await;
    let at = Timestamp::from_millis(1_900_000_000_000).unwrap();
    fixture.project.clock().set(at);
    let view = fixture
        .messages
        .send(&agent("frontend"), message(&["backend"], "x"))
        .await
        .unwrap();
    assert_eq!(view.sent_at, at);
    assert_eq!(
        fixture
            .count("SELECT created_at FROM messages WHERE number = 1")
            .await,
        at.as_millis()
    );
}

#[tokio::test]
async fn the_event_names_its_message_in_a_column_that_decides_recipients() {
    let fixture = Fixture::new().await;
    let view = fixture
        .messages
        .send(
            &agent("frontend"),
            message(&["backend", "frontend/ui"], "x"),
        )
        .await
        .unwrap();
    let stored = fixture.message_events().await;
    let seq = stored[0].seq;
    assert_eq!(
        fixture
            .count(&format!("SELECT message_id FROM events WHERE seq = {seq}"))
            .await,
        view.id.to_db()
    );
    let read = fixture.project.store().read();
    let agent_id = async |name: &str| {
        fold_store::agents::find_agent(read, &AgentName::parse(name).unwrap())
            .await
            .unwrap()
            .unwrap()
            .id
    };
    for (name, expected) in [("backend", true), ("frontend", true), ("review", false)] {
        let id = agent_id(name).await;
        assert_eq!(
            fold_store::events::is_recipient(read, seq, id)
                .await
                .unwrap(),
            expected,
            "{name}"
        );
    }
    let other = fixture
        .count("SELECT MIN(seq) FROM events WHERE message_id IS NULL")
        .await;
    let backend = agent_id("backend").await;
    assert!(
        !fold_store::events::is_recipient(read, u64::try_from(other).unwrap(), backend)
            .await
            .unwrap(),
        "an event about no message reaches nobody"
    );
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM events WHERE message_id IS NOT NULL AND type <> 'message.sent'")
            .await,
        0
    );
}

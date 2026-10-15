use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, AgentName, EventActor, EventType, FoldError, HumanName, NewEvent,
    ProposalId, TaskDescription, TaskId, TaskTitle, codes,
};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{
    EventBuffer, EventBus, EventPage, EventQuery, EventService, EventsError, MAX_LIMIT, query,
    read_since_cursor,
};
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_store::agents::WorkerRow;
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError, events};

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

#[cfg(test)]
fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

#[cfg(test)]
fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn actor(label: &str) -> EventActor {
    EventActor::parse(label).unwrap()
}

#[cfg(test)]
fn new_event(event_type: EventType, by: &str) -> NewEvent {
    NewEvent::new(event_type, actor(by)).unwrap()
}

#[cfg(test)]
struct Agent {
    id: i64,
    name: AgentName,
    main: WorkerRow,
    _ctx: SessionContext,
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    agents: AgentService,
    tasks: TaskService,
    events: EventService,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock, 30);
        let events = EventService::new(project.store().clone(), bus.clone());
        Self {
            project,
            bus,
            agents,
            tasks,
            events,
        }
    }

    fn store(&self) -> &Store {
        self.project.store()
    }

    async fn register(&self, name: &str) -> Agent {
        let mut ctx = self.agents.start_session(1).await.unwrap();
        let name = AgentName::parse(name).unwrap();
        self.agents
            .register_agent(&mut ctx, &name, &[])
            .await
            .unwrap();
        let main = self.agents.resolve_worker(&ctx, None).await.unwrap();
        Agent {
            id: main.agent_id,
            name,
            main,
            _ctx: ctx,
        }
    }

    async fn create_tasks(&self, count: u32) {
        for _ in 0..count {
            self.tasks
                .create_task(
                    &ana(),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new(""),
                        labels: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
    }

    /// Appends `batch` in one write and publishes it after the commit, as a command does.
    async fn commit(&self, batch: Vec<NewEvent>) -> Vec<u64> {
        let mut buffer = EventBuffer::new();
        self.store()
            .write(async |tx| {
                for event in &batch {
                    buffer.append(tx, event, self.project.now()).await?;
                }
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        let seqs = buffer.events().iter().map(|event| event.seq).collect();
        self.bus.publish(buffer);
        seqs
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.store().read()).await.unwrap()
    }

    async fn cursor(&self, agent: &Agent) -> u64 {
        events::cursor(self.store().read(), agent.id).await.unwrap()
    }

    /// Moves the agent's cursor to the head, so a test counts only what it appends next.
    async fn catch_up(&self, agent: &Agent) {
        loop {
            let page = self
                .events
                .get_events(
                    agent.id,
                    &EventQuery {
                        limit: Some(MAX_LIMIT),
                        ..EventQuery::default()
                    },
                )
                .await
                .unwrap();
            if page.events.is_empty() {
                break;
            }
        }
        assert_eq!(self.cursor(agent).await, self.head().await);
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
}

#[cfg(test)]
fn seqs(page: &EventPage) -> Vec<u64> {
    page.events.iter().map(|event| event.seq).collect()
}

#[cfg(test)]
fn refusal(error: EventsError) -> FoldError {
    match error {
        EventsError::Fold(error) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
fn by_type(name: &str) -> EventQuery {
    EventQuery {
        after: Some(0),
        types: vec![name.to_owned()],
        ..EventQuery::default()
    }
}

#[tokio::test]
async fn filters_select_by_type_task_and_proposal() {
    let fx = Fixture::new().await;
    let start = fx.head().await;
    let seqs_added = fx
        .commit(vec![
            new_event(types::TASK_UPDATED, "backend").task(t(1)),
            new_event(types::TASK_UPDATED, "backend").task(t(2)),
            new_event(types::REVIEW_SUBMITTED, "review").proposal(p(1)),
            new_event(types::MAIN_ADVANCED, "fold"),
        ])
        .await;
    let after = |types: Vec<&str>, task: Option<&str>, proposal: Option<&str>| EventQuery {
        after: Some(start),
        types: types.into_iter().map(str::to_owned).collect(),
        task: task.map(str::to_owned),
        proposal: proposal.map(str::to_owned),
        limit: None,
    };
    let all = query(fx.store(), &after(vec![], None, None)).await.unwrap();
    assert_eq!(seqs(&all), seqs_added);
    assert_eq!(all.next, seqs_added[3]);
    let updated = query(fx.store(), &after(vec!["task.updated"], None, None))
        .await
        .unwrap();
    assert_eq!(seqs(&updated), seqs_added[..2]);
    let two_types = query(
        fx.store(),
        &after(vec!["main.advanced", "review.submitted"], None, None),
    )
    .await
    .unwrap();
    assert_eq!(seqs(&two_types), seqs_added[2..]);
    let task = query(fx.store(), &after(vec![], Some("T-2"), None))
        .await
        .unwrap();
    assert_eq!(seqs(&task), [seqs_added[1]]);
    assert_eq!(task.events[0].task, Some(t(2)));
    let proposal = query(fx.store(), &after(vec![], None, Some("P-1")))
        .await
        .unwrap();
    assert_eq!(seqs(&proposal), [seqs_added[2]]);
    let none = query(fx.store(), &after(vec!["task.updated"], None, Some("P-1")))
        .await
        .unwrap();
    assert!(none.events.is_empty());
    assert_eq!(none.next, start, "an empty page points at its start");
}

#[tokio::test]
async fn the_live_type_is_a_valid_filter_that_never_returns_anything() {
    let fx = Fixture::new().await;
    fx.commit(vec![new_event(types::MAIN_ADVANCED, "fold")])
        .await;
    fx.bus.publish_live(
        fold_core::LiveEvent::new(types::WORKSPACE_CHANGED, None, serde_json::json!({})).unwrap(),
    );
    let page = query(fx.store(), &by_type("workspace.changed"))
        .await
        .unwrap();
    assert!(page.events.is_empty());
}

#[tokio::test]
async fn limit_defaults_to_100_and_is_lowered_to_500() {
    let fx = Fixture::new().await;
    let start = fx.head().await;
    fx.commit(
        (0..600)
            .map(|_| new_event(types::MAIN_ADVANCED, "fold"))
            .collect(),
    )
    .await;
    let from = |limit: Option<u32>| EventQuery {
        after: Some(start),
        limit,
        ..EventQuery::default()
    };
    assert_eq!(
        query(fx.store(), &from(None)).await.unwrap().events.len(),
        100
    );
    let two = query(fx.store(), &from(Some(2))).await.unwrap();
    assert_eq!(seqs(&two), [start + 1, start + 2]);
    assert_eq!(two.next, start + 2);
    assert_eq!(
        query(fx.store(), &from(Some(10_000)))
            .await
            .unwrap()
            .events
            .len(),
        500
    );
}

#[tokio::test]
async fn a_bad_filter_is_refused_with_invalid_event_filter() {
    let fx = Fixture::new().await;
    let agent = fx.register("frontend").await;
    let bad = [
        by_type("task.exploded"),
        by_type("Task.Created"),
        EventQuery {
            task: Some("12".into()),
            ..EventQuery::default()
        },
        EventQuery {
            task: Some("P-1".into()),
            ..EventQuery::default()
        },
        EventQuery {
            proposal: Some("P-0".into()),
            ..EventQuery::default()
        },
        EventQuery {
            limit: Some(0),
            ..EventQuery::default()
        },
    ];
    for request in bad {
        let error = refusal(query(fx.store(), &request).await.unwrap_err());
        assert_eq!(error.code(), codes::INVALID_EVENT_FILTER, "{request:?}");
        assert_eq!(error.hint(), Some("call get_events"));
        let error = refusal(
            read_since_cursor(fx.store(), agent.id, &request)
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), codes::INVALID_EVENT_FILTER);
    }
    let unknown = refusal(
        query(fx.store(), &by_type("task.exploded"))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        unknown.details().and_then(|details| details.get("type")),
        Some(&serde_json::json!("task.exploded"))
    );
    assert_eq!(fx.cursor(&agent).await, 0, "a refused read moves nothing");
}

#[tokio::test]
async fn the_cursor_resumes_and_advances_to_the_last_event_returned() {
    let fx = Fixture::new().await;
    let agent = fx.register("frontend").await;
    let head = fx.head().await;
    assert_eq!(fx.cursor(&agent).await, 0, "no cursor row reads as 0");
    let first = fx
        .events
        .get_events(
            agent.id,
            &EventQuery {
                limit: Some(2),
                ..EventQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(seqs(&first), [1, 2]);
    assert_eq!(fx.cursor(&agent).await, 2);
    let rest = fx
        .events
        .get_events(agent.id, &EventQuery::default())
        .await
        .unwrap();
    assert_eq!(rest.events.first().map(|event| event.seq), Some(3));
    assert_eq!(rest.next, head);
    assert_eq!(fx.cursor(&agent).await, head);
    let empty = fx
        .events
        .get_events(agent.id, &EventQuery::default())
        .await
        .unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.next, head);
    let added = fx
        .commit(vec![new_event(types::MAIN_ADVANCED, "fold")])
        .await;
    let news = fx
        .events
        .get_events(agent.id, &EventQuery::default())
        .await
        .unwrap();
    assert_eq!(seqs(&news), added);
    assert_eq!(fx.cursor(&agent).await, added[0]);
}

#[tokio::test]
async fn a_filtered_cursor_read_moves_the_cursor_to_the_last_match() {
    let fx = Fixture::new().await;
    let agent = fx.register("frontend").await;
    fx.catch_up(&agent).await;
    let added = fx
        .commit(vec![
            new_event(types::MAIN_ADVANCED, "fold"),
            new_event(types::TASK_UPDATED, "backend").task(t(1)),
            new_event(types::MAIN_ADVANCED, "fold"),
        ])
        .await;
    let page = fx
        .events
        .get_events(
            agent.id,
            &EventQuery {
                task: Some("T-1".into()),
                ..EventQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(seqs(&page), [added[1]]);
    assert_eq!(fx.cursor(&agent).await, added[1]);
}

#[tokio::test]
async fn an_explicit_after_leaves_the_cursor_alone() {
    let fx = Fixture::new().await;
    let agent = fx.register("frontend").await;
    fx.catch_up(&agent).await;
    let cursor = fx.cursor(&agent).await;
    fx.commit(vec![new_event(types::MAIN_ADVANCED, "fold")])
        .await;
    for after in [0, cursor] {
        let page = fx
            .events
            .get_events(
                agent.id,
                &EventQuery {
                    after: Some(after),
                    ..EventQuery::default()
                },
            )
            .await
            .unwrap();
        assert!(!page.events.is_empty());
        assert_eq!(fx.cursor(&agent).await, cursor);
    }
}

#[tokio::test]
async fn the_cursor_never_moves_back() {
    let fx = Fixture::new().await;
    let agent = fx.register("frontend").await;
    fx.catch_up(&agent).await;
    let head = fx.head().await;
    fx.store()
        .write(async |tx| events::advance_cursor(tx, agent.id, 1).await)
        .await
        .unwrap();
    assert_eq!(fx.cursor(&agent).await, head);
}

#[tokio::test]
async fn two_agents_keep_independent_cursors() {
    let fx = Fixture::new().await;
    let frontend = fx.register("frontend").await;
    let backend = fx.register("backend").await;
    fx.catch_up(&frontend).await;
    assert_eq!(fx.cursor(&backend).await, 0);
    let added = fx
        .commit(vec![new_event(types::MAIN_ADVANCED, "fold")])
        .await;
    let page = fx
        .events
        .get_events(backend.id, &EventQuery::default())
        .await
        .unwrap();
    assert_eq!(page.events.first().map(|event| event.seq), Some(1));
    assert_eq!(fx.cursor(&backend).await, added[0]);
    let page = fx
        .events
        .get_events(frontend.id, &EventQuery::default())
        .await
        .unwrap();
    assert_eq!(seqs(&page), added);
}

#[tokio::test]
async fn the_cursor_survives_a_reopened_project() {
    let mut fx = Fixture::new().await;
    let agent = fx.register("frontend").await;
    fx.catch_up(&agent).await;
    let cursor = fx.cursor(&agent).await;
    fx.project.reopen().await;
    let events = EventService::new(fx.project.store().clone(), EventBus::new());
    assert_eq!(fx.cursor(&agent).await, cursor);
    let empty = events
        .get_events(agent.id, &EventQuery::default())
        .await
        .unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.next, cursor);
}

#[tokio::test]
async fn new_events_count_only_what_concerns_the_agent() {
    let fx = Fixture::new().await;
    fx.create_tasks(4).await;
    let frontend = fx.register("frontend").await;
    let backend = fx.register("backend").await;
    let review = fx.register("review").await;
    fx.tasks
        .claim_task(&by(&frontend.main), t(1))
        .await
        .unwrap();
    fx.tasks.add_dependency(&ana(), t(1), t(2)).await.unwrap();
    fx.sql(&format!(
        "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
         SELECT 1, id, {}, 'one', 0, 0 FROM tasks WHERE number = 3;
         INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
         SELECT 2, id, {}, 'two', 0, 0 FROM tasks WHERE number = 4;
         INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at)
         SELECT 'review', id, {}, 0, 0 FROM proposals WHERE number = 2;",
        frontend.main.id, backend.main.id, frontend.main.id
    ))
    .await;
    fx.catch_up(&frontend).await;
    let count = || fx.events.new_event_count(frontend.id, &frontend.name);
    assert_eq!(count().await.unwrap(), 0);
    let cursor = fx.cursor(&frontend).await;
    fx.commit(vec![
        new_event(types::TASK_UPDATED, "backend").task(t(1)),
        new_event(types::TASK_UPDATED, "human:ana").task(t(2)),
        new_event(types::REVIEW_SUBMITTED, "review").proposal(p(1)),
        new_event(types::PROPOSAL_VERSION_ADDED, "backend").proposal(p(2)),
        new_event(types::MAIN_ADVANCED, "fold"),
        new_event(types::TASK_UPDATED, "backend").task(t(4)),
        new_event(types::TASK_UPDATED, "frontend").task(t(1)),
        new_event(types::MAIN_ADVANCED, "frontend"),
    ])
    .await;
    assert_eq!(count().await.unwrap(), 5);
    assert_eq!(fx.cursor(&frontend).await, cursor, "counting moves nothing");
    let facts_of_review = fx.events.new_event_count(review.id, &review.name).await;
    assert!(
        facts_of_review.unwrap() >= 2,
        "main.advanced reaches every agent"
    );
    fx.tasks
        .release_task(&by(&frontend.main), t(1))
        .await
        .unwrap();
    fx.catch_up(&frontend).await;
    fx.commit(vec![
        new_event(types::TASK_UPDATED, "backend").task(t(1)),
        new_event(types::TASK_UPDATED, "backend").task(t(2)),
    ])
    .await;
    assert_eq!(count().await.unwrap(), 0, "a released claim stops the news");
}

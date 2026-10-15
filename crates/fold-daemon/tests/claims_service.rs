use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    AgentName, Event, EventFilter, FoldError, TaskDescription, TaskId, TaskStatus, TaskTitle,
    Timestamp, WorkerName,
};
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_daemon::services::tasks::{self, ClaimView, CreateTask, TaskError, TaskService};
use fold_store::agents::{self, WorkerRow};
use fold_store::tasks::claims;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::{StoreError, events};
use serde_json::json;

const LEASE_MINUTES: u64 = 5;

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

#[cfg(test)]
struct Agent {
    ctx: SessionContext,
    main: WorkerRow,
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    bus: EventBus,
    agents: AgentService,
    tasks: TaskService,
    frontend: Agent,
    ui: WorkerRow,
    backend: Agent,
}

#[cfg(test)]
impl Fixture {
    /// Three tasks, the agent `frontend` with its child worker `frontend/ui`, and the agent `backend`. Claims use a lease of `lease_minutes`.
    async fn new(lease_minutes: u64) -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(project.store().clone(), bus.clone(), clock, lease_minutes);
        let creator = fold_core::EventActor::parse("frontend").unwrap();
        for _ in 0..3 {
            tasks
                .create_task(
                    &creator,
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        let frontend = register(&agents, "frontend").await;
        let ui = agents
            .register_worker(&frontend.ctx, "ui", None)
            .await
            .unwrap()
            .worker;
        let backend = register(&agents, "backend").await;
        Self {
            project,
            bus,
            agents,
            tasks,
            frontend,
            ui,
            backend,
        }
    }

    async fn claim_events(&self) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            0,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type.as_str().starts_with("claim."))
        .collect()
    }

    async fn renewed_at(&self, task: TaskId) -> Timestamp {
        claims::unreleased_for_task(self.project.store().read(), task)
            .await
            .unwrap()
            .unwrap()
            .renewed_at
    }

    fn advance(&self, by: Duration) -> Timestamp {
        self.project.clock().advance(by).unwrap()
    }
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> Agent {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    let main = agents.resolve_worker(&ctx, None).await.unwrap();
    Agent { ctx, main }
}

#[cfg(test)]
fn refusal(error: TaskError) -> FoldError {
    match error {
        TaskError::Fold(error) => error,
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

// Each round spawns both claims on a multi-thread runtime, so the two writes contend for the writer and either may come first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_claim_at_once_and_one_is_refused_naming_the_holder() {
    const ROUNDS: usize = 40;
    let fixture = Fixture::new(LEASE_MINUTES).await;
    let mut acquired = 0;
    for _ in 0..ROUNDS {
        let mut receiver = fixture.bus.subscribe();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let spawn_claim = |worker: WorkerRow| {
            let service = fixture.tasks.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                service.claim_task(&worker, t(1)).await
            })
        };
        let front = spawn_claim(fixture.frontend.main.clone());
        let back = spawn_claim(fixture.backend.main.clone());
        let (front, back) = (front.await.unwrap(), back.await.unwrap());
        let (won, lost, winner) = match (front, back) {
            (Ok(view), Err(error)) => (view, error, &fixture.frontend.main),
            (Err(error), Ok(view)) => (view, error, &fixture.backend.main),
            other => panic!("expected exactly one claim to succeed, got {other:?}"),
        };
        let name = winner.name.as_str();
        assert_eq!(won.holder, winner.name);
        assert_eq!(won.acquired_at, fixture.project.now());
        assert!(!won.stale);
        assert_eq!(lost.code(), Some("already_claimed"));
        let lost = refusal(lost);
        assert_eq!(details(&lost), json!({"holder": name, "stale": false}));
        assert!(lost.hint().unwrap().contains("takeover_claim"));
        assert!(lost.message().contains(name));

        acquired += 1;
        let stored = fixture.claim_events().await;
        let last = stored.last().unwrap();
        assert_eq!(
            stored
                .iter()
                .filter(|event| event.event_type == types::CLAIM_ACQUIRED)
                .count(),
            acquired
        );
        assert_eq!(last.event_type, types::CLAIM_ACQUIRED);
        assert_eq!(last.actor.as_str(), name);
        assert_eq!(last.task, Some(t(1)));
        assert_eq!(last.data, json!({"kind": "implementation", "worker": name}));
        let BusMessage::Durable(published) = receiver.try_recv().unwrap() else {
            panic!("expected a durable event");
        };
        assert_eq!(published.seq, last.seq);
        assert!(receiver.try_recv().is_err());

        fixture.tasks.release_task(winner, t(1)).await.unwrap();
    }
}

#[tokio::test]
async fn the_holder_repeating_the_call_renews_the_claim() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    let first = fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    let acquired = fixture.project.now();
    // Past the lease, so the repeat both renews and clears the stale flag.
    let later = fixture.advance(minutes(LEASE_MINUTES + 1));
    assert!(fixture.tasks.claim_view(t(1)).await.unwrap().unwrap().stale);

    let again = fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    assert_eq!(
        again,
        ClaimView {
            task: t(1),
            holder: WorkerName::parse("frontend").unwrap(),
            acquired_at: acquired,
            renewed_at: later,
            stale: false,
        }
    );
    assert_eq!(first.acquired_at, again.acquired_at);
    assert_eq!(fixture.renewed_at(t(1)).await, later);
    assert_eq!(fixture.tasks.claim_view(t(1)).await.unwrap(), Some(again));
    assert_eq!(fixture.claim_events().await.len(), 1);
}

#[tokio::test]
async fn only_the_holder_releases_and_the_claim_frees_the_task() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    let unclaimed = fixture
        .tasks
        .release_task(&fixture.frontend.main, t(1))
        .await
        .unwrap_err();
    assert_eq!(unclaimed.code(), Some("claim_not_found"));
    assert!(refusal(unclaimed).hint().unwrap().contains("claim_task"));

    fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    for intruder in [&fixture.backend.main, &fixture.ui] {
        let error = fixture
            .tasks
            .release_task(intruder, t(1))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("not_claim_holder"));
        assert_eq!(
            details(&refusal(error)),
            json!({"holder": "frontend", "stale": false})
        );
    }
    assert!(fixture.tasks.claim_view(t(1)).await.unwrap().is_some());

    let mut receiver = fixture.bus.subscribe();
    let released_at = fixture.advance(minutes(1));
    fixture
        .tasks
        .release_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    assert_eq!(fixture.tasks.claim_view(t(1)).await.unwrap(), None);
    let stored = fixture.claim_events().await;
    let released = stored.last().unwrap();
    assert_eq!(released.event_type, types::CLAIM_RELEASED);
    assert_eq!(released.actor.as_str(), "frontend");
    assert_eq!(released.task, Some(t(1)));
    assert_eq!(released.created_at, released_at);
    assert_eq!(
        released.data,
        json!({"kind": "implementation", "worker": "frontend", "reason": "released"})
    );
    assert!(matches!(
        receiver.try_recv().unwrap(),
        BusMessage::Durable(event) if event.event_type == types::CLAIM_RELEASED
    ));

    let again = fixture
        .tasks
        .release_task(&fixture.frontend.main, t(1))
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some("claim_not_found"));

    let view = fixture
        .tasks
        .claim_task(&fixture.backend.main, t(1))
        .await
        .unwrap();
    assert_eq!(view.holder, WorkerName::parse("backend").unwrap());
    assert_eq!(view.acquired_at, released_at);
}

#[tokio::test]
async fn a_call_from_any_tool_renews_every_claim_of_the_agent_with_last_call_at() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    fixture.tasks.claim_task(&fixture.ui, t(2)).await.unwrap();
    fixture
        .tasks
        .claim_task(&fixture.backend.main, t(3))
        .await
        .unwrap();
    let start = fixture.project.now();

    let now = fixture.advance(minutes(LEASE_MINUTES + 2));
    fixture
        .agents
        .record_call(&fixture.frontend.ctx, &fixture.ui, "get_task_graph", None)
        .await
        .unwrap();
    assert_eq!(fixture.renewed_at(t(1)).await, now);
    assert_eq!(fixture.renewed_at(t(2)).await, now);
    assert_eq!(fixture.renewed_at(t(3)).await, start);
    let session = agents::find_session(
        fixture.project.store().read(),
        fixture.frontend.ctx.session_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(session.last_call_at, Some(now));

    let views = [
        fixture.tasks.claim_view(t(1)).await.unwrap().unwrap(),
        fixture.tasks.claim_view(t(2)).await.unwrap().unwrap(),
        fixture.tasks.claim_view(t(3)).await.unwrap().unwrap(),
    ];
    assert_eq!(views.map(|view| view.stale), [false, false, true]);
    // A renewal is not an event.
    assert_eq!(fixture.claim_events().await.len(), 3);
}

#[tokio::test]
async fn renewal_and_last_call_at_commit_together() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    let start = fixture.project.now();
    // A worker row that does not belong to the session's agent makes the renewal fail inside the write; the session stamp must roll back with it.
    let broken = WorkerRow {
        agent_id: -1,
        ..fixture.frontend.main.clone()
    };
    fixture
        .project
        .store()
        .write(async |tx| {
            sqlx::query("CREATE TRIGGER refuse_renewal BEFORE UPDATE OF renewed_at ON claims BEGIN SELECT RAISE(ABORT, 'renewal refused'); END")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    fixture.advance(minutes(1));
    fixture
        .agents
        .record_call(
            &fixture.frontend.ctx,
            &fixture.frontend.main,
            "get_task",
            None,
        )
        .await
        .unwrap_err();
    let session = agents::find_session(
        fixture.project.store().read(),
        fixture.frontend.ctx.session_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(session.last_call_at, Some(start));
    assert_eq!(fixture.renewed_at(t(1)).await, start);
    // A worker of another agent renews nothing and still stamps the session.
    fixture
        .project
        .store()
        .write(async |tx| {
            sqlx::query("DROP TRIGGER refuse_renewal")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let now = fixture.project.now();
    fixture
        .agents
        .record_call(&fixture.frontend.ctx, &broken, "get_task", None)
        .await
        .unwrap();
    assert_eq!(fixture.renewed_at(t(1)).await, start);
    let session = agents::find_session(
        fixture.project.store().read(),
        fixture.frontend.ctx.session_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(session.last_call_at, Some(now));
}

#[tokio::test]
async fn a_stale_holder_still_blocks_a_new_claim() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();

    fixture.advance(minutes(LEASE_MINUTES) - Duration::from_millis(1));
    let fresh = fixture
        .tasks
        .claim_task(&fixture.backend.main, t(1))
        .await
        .unwrap_err();
    assert_eq!(
        details(&refusal(fresh)),
        json!({"holder": "frontend", "stale": false})
    );

    fixture.advance(Duration::from_millis(1));
    let stale = fixture
        .tasks
        .claim_task(&fixture.backend.main, t(1))
        .await
        .unwrap_err();
    assert_eq!(stale.code(), Some("already_claimed"));
    let stale = refusal(stale);
    assert_eq!(
        details(&stale),
        json!({"holder": "frontend", "stale": true})
    );
    assert!(stale.hint().unwrap().contains("takeover_claim"));
    let view = fixture.tasks.claim_view(t(1)).await.unwrap().unwrap();
    assert!(view.stale);
    assert_eq!(view.holder, WorkerName::parse("frontend").unwrap());
    // Staleness is computed, never written.
    let stale_at = claims::unreleased_for_task(fixture.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap()
        .stale_at;
    assert_eq!(stale_at, None);

    fixture
        .agents
        .record_call(
            &fixture.frontend.ctx,
            &fixture.frontend.main,
            "get_task",
            None,
        )
        .await
        .unwrap();
    assert!(!fixture.tasks.claim_view(t(1)).await.unwrap().unwrap().stale);
}

#[tokio::test]
async fn the_default_lease_is_thirty_minutes() {
    let fixture = Fixture::new(fold_core::ProjectConfig::default().lease_minutes).await;
    fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    fixture.advance(minutes(29));
    assert!(!fixture.tasks.claim_view(t(1)).await.unwrap().unwrap().stale);
    fixture.advance(minutes(1));
    let view = tasks::claim_view(fixture.project.store(), t(1), fixture.project.now(), 30)
        .await
        .unwrap()
        .unwrap();
    assert!(view.stale);
}

#[tokio::test]
async fn done_and_archived_tasks_take_no_claim() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    let now = fixture.project.now();
    fixture
        .project
        .store()
        .write(async |tx| {
            completions::insert(tx, t(1), CompletionKind::Children, None, now).await?;
            tasks::recompute(tx, &[t(1)], now).await?;
            sqlx::query("UPDATE tasks SET archived_at = ? WHERE number = 2")
                .bind(now.as_millis())
                .execute(tx.conn())
                .await
                .map_err(StoreError::from)?;
            Ok::<_, TaskError>(())
        })
        .await
        .unwrap();
    for (task, reason) in [(t(1), "done"), (t(2), "archived")] {
        let error = fixture
            .tasks
            .claim_task(&fixture.frontend.main, task)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("task_closed"));
        let error = refusal(error);
        assert_eq!(details(&error), json!({"reason": reason}));
        assert!(error.hint().is_some());
    }
    assert!(fixture.claim_events().await.is_empty());
}

#[tokio::test]
async fn a_claim_moves_no_card() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    let before = fold_store::tasks::get_by_number(fixture.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    let head = events::head_seq(fixture.project.store().read())
        .await
        .unwrap();
    fixture
        .tasks
        .claim_task(&fixture.frontend.main, t(1))
        .await
        .unwrap();
    let after = fold_store::tasks::get_by_number(fixture.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.status, TaskStatus::Todo);
    let new: Vec<_> = events::read_after(
        fixture.project.store().read(),
        head,
        &EventFilter::default(),
        100,
    )
    .await
    .unwrap()
    .into_iter()
    .map(|event| event.event_type)
    .collect();
    assert_eq!(new, vec![types::CLAIM_ACQUIRED]);
}

#[tokio::test]
async fn a_missing_task_is_task_not_found() {
    let fixture = Fixture::new(LEASE_MINUTES).await;
    let missing = t(99);
    let claim = fixture
        .tasks
        .claim_task(&fixture.frontend.main, missing)
        .await
        .unwrap_err();
    assert_eq!(claim.code(), Some("task_not_found"));
    let release = fixture
        .tasks
        .release_task(&fixture.frontend.main, missing)
        .await
        .unwrap_err();
    assert_eq!(release.code(), Some("task_not_found"));
    let view = fixture.tasks.claim_view(missing).await.unwrap_err();
    assert_eq!(view.code(), Some("task_not_found"));
    assert_eq!(fixture.tasks.claim_view(t(1)).await.unwrap(), None);
}

use std::collections::BTreeMap;
use std::sync::Arc;

use fold_core::dag::Dag;
use fold_core::events::types;
use fold_core::status::{ProposalFact, TaskFacts, calculate};
use fold_core::{
    Actor, AgentActor, Event, EventActor, EventFilter, TaskDescription, TaskId, TaskStatus,
    TaskTitle, WorkerName,
};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{self, CreateTask, TaskError, TaskService};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::{StoreError, events};
use serde_json::json;
use sqlx::Row;

use TaskStatus::{Backlog, Done, Todo};

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn agent() -> Actor {
    let worker = WorkerName::parse("frontend").unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    /// `count` tasks made through `create_task`, then the edges `(task, needs)` through `add_dependency`.
    async fn new(count: u32, edges: &[(u32, u32)]) -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let tasks = TaskService::new(
            project.store().clone(),
            bus,
            clock,
            fold_core::ProjectConfig::default().lease_minutes,
        );
        for _ in 0..count {
            tasks
                .create_task(
                    &agent(),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        for &(task, needs) in edges {
            tasks
                .add_dependency(&agent(), t(task), t(needs))
                .await
                .unwrap();
        }
        Self { project, tasks }
    }

    async fn status(&self, task: u32) -> TaskStatus {
        self.tasks.get(t(task)).await.unwrap().status
    }

    async fn statuses(&self, numbers: impl IntoIterator<Item = u32>) -> Vec<TaskStatus> {
        let mut out = Vec::new();
        for number in numbers {
            out.push(self.status(number).await);
        }
        out
    }

    /// Writes a completion for each task and runs recompute on them in one transaction; returns the events recompute appended.
    async fn complete(&self, numbers: &[u32]) -> Vec<Event> {
        let now = self.project.now();
        let seeds: Vec<TaskId> = numbers.iter().map(|&n| t(n)).collect();
        self.project
            .store()
            .write(async |tx| {
                for &task in &seeds {
                    completions::insert(tx, task, CompletionKind::Children, None, now).await?;
                }
                let recomputed = tasks::recompute(tx, &seeds[..1], now).await?;
                Ok::<_, TaskError>(recomputed.events.events().to_vec())
            })
            .await
            .unwrap()
    }

    async fn recompute(&self, numbers: &[u32]) -> Vec<Event> {
        let now = self.project.now();
        let seeds: Vec<TaskId> = numbers.iter().map(|&n| t(n)).collect();
        self.project
            .store()
            .write(async |tx| {
                let recomputed = tasks::recompute(tx, &seeds, now).await?;
                Ok::<_, TaskError>(recomputed.events.events().to_vec())
            })
            .await
            .unwrap()
    }

    async fn exec(&self, sql: &'static str) {
        self.project
            .store()
            .write(async |tx| {
                sqlx::query(sql).execute(tx.conn()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.project.store().read()).await.unwrap()
    }

    /// Recalculates every task from the stored completions and edges with the status table and requires the cache to agree. These fixtures have no workspaces or proposals.
    async fn assert_cache_matches_the_status_table(&self) {
        let completed: Vec<i64> =
            sqlx::query("SELECT t.number FROM completions c JOIN tasks t ON t.id = c.task_id")
                .fetch_all(self.project.store().read())
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<i64, _>(0))
                .collect();
        let edges = self.tasks.edges().await.unwrap();
        let dag = Dag::from_edges(edges.iter().map(|edge| (edge.task, edge.needs)));
        let views = self.tasks.list(true).await.unwrap();
        let stored: BTreeMap<TaskId, (TaskStatus, bool)> = views
            .iter()
            .map(|view| (view.id, (view.status, view.blocked)))
            .collect();
        let mut expected: BTreeMap<TaskId, TaskStatus> = BTreeMap::new();
        for task in dag.topological_order(stored.keys().copied()) {
            let facts = TaskFacts {
                completed: completed.contains(&task.to_db()),
                proposal: ProposalFact::None,
                has_active_workspace: false,
                prerequisites: dag
                    .prerequisites(task)
                    .into_iter()
                    .map(|needs| (needs, expected[&needs]))
                    .collect(),
            };
            let calculated = calculate(&facts);
            assert_eq!(
                stored[&task],
                (calculated.status, calculated.blocked()),
                "{task}"
            );
            expected.insert(task, calculated.status);
        }
    }
}

/// Rows inserted, updated or deleted on this connection since it opened.
#[cfg(test)]
async fn total_changes(tx: &mut fold_store::WriteTx) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar("SELECT total_changes()")
        .fetch_one(tx.conn())
        .await?)
}

/// The `(task, from, to)` of each `task.status_changed` event.
#[cfg(test)]
fn moves(events: &[Event]) -> Vec<(TaskId, String, String)> {
    events
        .iter()
        .filter(|event| event.event_type == types::TASK_STATUS_CHANGED)
        .map(|event| {
            assert_eq!(event.actor, EventActor::fold());
            (
                event.task.unwrap(),
                event.data["from"].as_str().unwrap().to_owned(),
                event.data["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
fn mv(task: u32, from: &str, to: &str) -> (TaskId, String, String) {
    (t(task), from.to_owned(), to.to_owned())
}

#[tokio::test]
async fn a_chain_of_four_follows_the_status_table() {
    // T-2 needs T-1, T-3 needs T-2, T-4 needs T-3.
    let fx = Fixture::new(4, &[(2, 1), (3, 2), (4, 3)]).await;
    assert_eq!(fx.statuses(1..=4).await, [Todo, Backlog, Backlog, Backlog]);
    fx.assert_cache_matches_the_status_table().await;

    let appended = fx.complete(&[1]).await;
    assert_eq!(
        moves(&appended),
        [mv(1, "todo", "done"), mv(2, "backlog", "todo")]
    );
    assert_eq!(fx.statuses(1..=4).await, [Done, Todo, Backlog, Backlog]);
    fx.assert_cache_matches_the_status_table().await;

    for (number, after) in [
        (2, [Done, Done, Todo, Backlog]),
        (3, [Done, Done, Done, Todo]),
        (4, [Done, Done, Done, Done]),
    ] {
        fx.complete(&[number]).await;
        assert_eq!(fx.statuses(1..=4).await, after);
        fx.assert_cache_matches_the_status_table().await;
    }
}

#[tokio::test]
async fn one_call_cascades_down_the_chain_in_dependency_order() {
    let fx = Fixture::new(4, &[(2, 1), (3, 2), (4, 3)]).await;
    // Completions for T-1 to T-3, but only T-1 is a seed: the others are reached as dependents and each reads its prerequisite's freshly written status.
    let appended = fx.complete(&[1, 2, 3]).await;
    assert_eq!(
        moves(&appended),
        [
            mv(1, "todo", "done"),
            mv(2, "backlog", "done"),
            mv(3, "backlog", "done"),
            mv(4, "backlog", "todo"),
        ]
    );
    assert_eq!(fx.statuses(1..=4).await, [Done, Done, Done, Todo]);
    fx.assert_cache_matches_the_status_table().await;
}

#[tokio::test]
async fn a_diamond_waits_for_both_sides() {
    // T-2 and T-3 need T-1; T-4 needs T-2 and T-3.
    let fx = Fixture::new(4, &[(2, 1), (3, 1), (4, 2), (4, 3)]).await;
    assert_eq!(fx.statuses(1..=4).await, [Todo, Backlog, Backlog, Backlog]);

    let appended = fx.complete(&[1]).await;
    assert_eq!(
        moves(&appended),
        [
            mv(1, "todo", "done"),
            mv(2, "backlog", "todo"),
            mv(3, "backlog", "todo"),
        ]
    );
    fx.assert_cache_matches_the_status_table().await;

    let appended = fx.complete(&[2]).await;
    assert_eq!(moves(&appended), [mv(2, "todo", "done")]);
    assert_eq!(fx.status(4).await, Backlog);

    let appended = fx.complete(&[3]).await;
    assert_eq!(
        moves(&appended),
        [mv(3, "todo", "done"), mv(4, "backlog", "todo")]
    );
    assert_eq!(fx.statuses(1..=4).await, [Done, Done, Done, Todo]);
    fx.assert_cache_matches_the_status_table().await;
}

#[tokio::test]
async fn a_wide_fan_out_moves_every_waiting_dependent_in_one_call() {
    let edges: Vec<(u32, u32)> = (2..=41).map(|task| (task, 1)).collect();
    let fx = Fixture::new(41, &edges).await;
    assert!(fx.statuses(2..=41).await.iter().all(|s| *s == Backlog));

    let appended = fx.complete(&[1]).await;
    let mut expected = vec![mv(1, "todo", "done")];
    expected.extend((2..=41).map(|task| mv(task, "backlog", "todo")));
    assert_eq!(moves(&appended), expected);
    assert!(fx.statuses(2..=41).await.iter().all(|s| *s == Todo));
    fx.assert_cache_matches_the_status_table().await;
}

#[tokio::test]
async fn completing_a_prerequisite_writes_status_and_events_in_the_same_transaction() {
    let fx = Fixture::new(2, &[(2, 1)]).await;
    let before = fx.head().await;
    let now = fx.project.now();

    // Rolled back after recompute ran: neither the completion, the cache nor the events survive.
    let outcome = fx
        .project
        .store()
        .write(async |tx| {
            completions::insert(tx, t(1), CompletionKind::Children, None, now).await?;
            let recomputed = tasks::recompute(tx, &[t(1)], now).await?;
            assert_eq!(recomputed.events.len(), 2);
            Err::<(), _>(TaskError::Store(StoreError::Invalid("roll back")))
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(fx.statuses(1..=2).await, [Todo, Backlog]);
    assert_eq!(fx.head().await, before);

    let appended = fx.complete(&[1]).await;
    let stored = events::read_after(
        fx.project.store().read(),
        before,
        &EventFilter::default(),
        100,
    )
    .await
    .unwrap();
    assert_eq!(stored, appended);
    assert_eq!(
        stored.iter().map(|e| &e.data).collect::<Vec<_>>(),
        [
            &json!({"from": "todo", "to": "done"}),
            &json!({"from": "backlog", "to": "todo"}),
        ]
    );
    assert!(stored.iter().all(|e| e.created_at == now));
}

#[tokio::test]
async fn a_recompute_that_changes_nothing_writes_nothing() {
    let fx = Fixture::new(4, &[(2, 1), (3, 1), (4, 3)]).await;
    fx.complete(&[1]).await;
    let before = fx.head().await;
    let now = fx.project.now();

    let (appended, changes) = fx
        .project
        .store()
        .write(async |tx| {
            let start = total_changes(tx).await?;
            let recomputed = tasks::recompute(tx, &[t(1), t(2), t(3), t(4)], now).await?;
            let end = total_changes(tx).await?;
            Ok::<_, TaskError>((recomputed.events.len(), end - start))
        })
        .await
        .unwrap();
    assert_eq!((appended, changes), (0, 0));
    assert_eq!(fx.head().await, before);
}

#[tokio::test]
async fn nothing_to_recompute_is_a_no_op() {
    let fx = Fixture::new(1, &[]).await;
    let before = fx.head().await;
    assert!(fx.recompute(&[]).await.is_empty());
    assert!(fx.recompute(&[9]).await.is_empty());
    assert_eq!(fx.head().await, before);
}

#[tokio::test]
async fn archived_tasks_are_untouched() {
    let fx = Fixture::new(3, &[(2, 1), (3, 2)]).await;
    fx.exec("UPDATE tasks SET archived_at = 1 WHERE number = 2")
        .await;

    let appended = fx.complete(&[1]).await;
    assert_eq!(moves(&appended), [mv(1, "todo", "done")]);
    // T-2 keeps its stored status, so T-3 still waits on it.
    assert_eq!(fx.statuses(1..=3).await, [Done, Backlog, Backlog]);
    assert!(fx.recompute(&[2]).await.is_empty());
    assert_eq!(fx.status(2).await, Backlog);
}

#[tokio::test]
async fn only_the_dependents_of_the_seeds_are_visited() {
    let fx = Fixture::new(3, &[(2, 1)]).await;
    // T-3 is unrelated; a stale value there shows whether recompute looked at it.
    fx.exec("UPDATE tasks SET status = 'review' WHERE number = 3")
        .await;
    fx.complete(&[1]).await;
    assert_eq!(fx.status(3).await, TaskStatus::Review);

    let appended = fx.recompute(&[3]).await;
    assert_eq!(moves(&appended), [mv(3, "review", "todo")]);
    fx.assert_cache_matches_the_status_table().await;
}

#[tokio::test]
async fn a_waiting_task_becomes_todo_after_its_prerequisite_completes() {
    let fx = Fixture::new(2, &[]).await;
    fx.tasks.add_dependency(&agent(), t(2), t(1)).await.unwrap();
    assert_eq!(fx.status(2).await, Backlog);

    fx.complete(&[1]).await;
    assert_eq!(fx.statuses(1..=2).await, [Done, Todo]);
}

#[tokio::test]
async fn removing_the_last_unfinished_prerequisite_makes_the_task_todo() {
    let fx = Fixture::new(3, &[(3, 1), (3, 2)]).await;
    fx.complete(&[1]).await;
    assert_eq!(fx.status(3).await, Backlog);
    fx.tasks
        .remove_dependency(&agent(), t(3), t(2))
        .await
        .unwrap();
    assert_eq!(fx.status(3).await, Todo);
    fx.assert_cache_matches_the_status_table().await;
}

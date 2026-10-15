use std::sync::Arc;

use fold_core::{
    Actor, AgentActor, LabelColor, LabelName, TaskDescription, TaskId, TaskTitle, Timestamp,
    WorkerName,
};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{self, CreateTask, ReadyTask, TaskError, TaskService};
use fold_store::StoreError;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn agent() -> Actor {
    let worker = WorkerName::parse("backend").unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let tasks = TaskService::new(
            project.store().clone(),
            EventBus::new(),
            Arc::new(project.clock().clone()),
            fold_core::ProjectConfig::default().lease_minutes,
        );
        let fixture = Self { project, tasks };
        for sql in [
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 1), (2, 'frontend', 1)",
            "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'api', 1), (2, 2, 'ui', 1)",
        ] {
            fixture.run(sql, None).await;
        }
        fixture
    }

    async fn run(&self, sql: &'static str, task: Option<u32>) {
        self.project
            .store()
            .write(async |tx| {
                let mut query = sqlx::query(sql);
                if let Some(task) = task {
                    query = query.bind(i64::from(task));
                }
                query.execute(tx.conn()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }

    /// Creates the next task with the clock at `created_at`.
    async fn task(&self, created_at: i64, labels: &[&str]) -> TaskId {
        self.project.clock().set(at(created_at));
        self.tasks
            .create_task(
                &agent(),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new(""),
                    labels: labels
                        .iter()
                        .map(|name| LabelName::parse(name).unwrap())
                        .collect(),
                },
            )
            .await
            .unwrap()
            .id
    }

    async fn needs(&self, task: u32, needs: u32) {
        self.tasks
            .add_dependency(&agent(), t(task), t(needs))
            .await
            .unwrap();
    }

    async fn complete(&self, task: u32) {
        let now = self.project.now();
        self.project
            .store()
            .write(async |tx| {
                completions::insert(tx, t(task), CompletionKind::Children, None, now).await?;
                tasks::recompute(tx, &[t(task)], now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }

    async fn ready(&self) -> Vec<ReadyTask> {
        self.tasks.ready().await.unwrap()
    }

    async fn ready_ids(&self) -> Vec<TaskId> {
        self.ready().await.into_iter().map(|task| task.id).collect()
    }
}

#[tokio::test]
async fn an_empty_project_has_no_ready_work() {
    let fx = Fixture::new().await;
    assert!(fx.ready().await.is_empty());
}

#[tokio::test]
async fn a_task_claimed_by_anyone_is_not_ready_until_the_claim_is_released() {
    let fx = Fixture::new().await;
    for created in [10, 20, 30] {
        fx.task(created, &[]).await;
    }
    fx.run(
        "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at)
         VALUES ('implementation', (SELECT id FROM tasks WHERE number = ?), 1, 1, 1)",
        Some(1),
    )
    .await;
    fx.run(
        "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at)
         VALUES ('implementation', (SELECT id FROM tasks WHERE number = ?), 2, 1, 1)",
        Some(2),
    )
    .await;
    assert_eq!(fx.ready_ids().await, vec![t(3)]);

    fx.run(
        "UPDATE claims SET released_at = 5, release_reason = 'released'
         WHERE task_id = (SELECT id FROM tasks WHERE number = ?)",
        Some(1),
    )
    .await;
    assert_eq!(fx.ready_ids().await, vec![t(1), t(3)]);
}

#[tokio::test]
async fn the_task_that_unblocks_more_sorts_first() {
    // T-1 is oldest and frees nothing; T-2 frees T-4 and T-5; T-3 frees T-6.
    let fx = Fixture::new().await;
    for created in [10, 20, 30, 40, 50, 60] {
        fx.task(created, &[]).await;
    }
    fx.needs(4, 2).await;
    fx.needs(5, 2).await;
    fx.needs(6, 3).await;

    let ready = fx.ready().await;
    let order: Vec<(TaskId, usize)> = ready.iter().map(|task| (task.id, task.unblocks)).collect();
    assert_eq!(order, vec![(t(2), 2), (t(3), 1), (t(1), 0)]);
}

#[tokio::test]
async fn equal_counts_sort_by_age_then_number() {
    let fx = Fixture::new().await;
    fx.task(300, &[]).await;
    fx.task(100, &[]).await;
    fx.task(200, &[]).await;
    fx.task(100, &[]).await;

    assert_eq!(fx.ready_ids().await, vec![t(2), t(4), t(3), t(1)]);
}

#[tokio::test]
async fn archived_and_done_tasks_never_appear() {
    // T-2 is Done and T-4 waits on it; T-3 is archived; T-1 stays Todo.
    let fx = Fixture::new().await;
    for created in [10, 20, 30, 40] {
        fx.task(created, &[]).await;
    }
    fx.needs(4, 2).await;
    assert_eq!(fx.ready_ids().await, vec![t(2), t(1), t(3)]);

    fx.complete(2).await;
    fx.tasks.archive_task(&agent(), t(3)).await.unwrap();

    assert_eq!(fx.ready_ids().await, vec![t(1), t(4)]);
}

#[tokio::test]
async fn a_ready_task_carries_its_title_labels_and_creation_time() {
    let fx = Fixture::new().await;
    for name in ["api", "web"] {
        fx.tasks
            .create_label(&agent(), LabelName::parse(name).unwrap(), LabelColor::Teal)
            .await
            .unwrap();
    }
    fx.task(1_000, &["web", "api"]).await;

    let ready = fx.ready().await;
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, t(1));
    assert_eq!(ready[0].title.as_str(), "task");
    assert_eq!(ready[0].created_at, at(1_000));
    assert_eq!(ready[0].unblocks, 0);
    let labels: Vec<&str> = ready[0]
        .labels
        .iter()
        .map(|label| label.name.as_str())
        .collect();
    assert_eq!(labels, vec!["api", "web"]);
}

#[tokio::test]
async fn a_started_dependent_counts_toward_unblocks() {
    // T-3 already has a workspace and waits only on T-2.
    let fx = Fixture::new().await;
    for created in [10, 20, 30] {
        fx.task(created, &[]).await;
    }
    fx.needs(3, 2).await;
    fx.run(
        "INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 1)",
        None,
    )
    .await;
    fx.run(
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at)
         VALUES (1, (SELECT id FROM tasks WHERE number = ?), 1, 'task', 'ws', 1, 'active', 1)",
        Some(3),
    )
    .await;
    let now = fx.project.now();
    fx.project
        .store()
        .write(async |tx| {
            tasks::recompute(tx, &[t(3)], now).await?;
            Ok::<_, TaskError>(())
        })
        .await
        .unwrap();

    let ready = fx.ready().await;
    let order: Vec<(TaskId, usize)> = ready.iter().map(|task| (task.id, task.unblocks)).collect();
    assert_eq!(order, vec![(t(2), 1), (t(1), 0)]);
}

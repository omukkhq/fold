use std::cell::Cell;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, Event, EventActor, TaskDescription, TaskId, TaskStatus, TaskTitle,
    WorkerName,
};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{self, CreateTask, TaskError, TaskService};
use fold_store::StoreError;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use serde_json::json;

use TaskStatus::{Done, InProgress, Review, Todo};

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
#[derive(Clone, Copy)]
enum Bind {
    Int(i64),
    Text(&'static str),
}

#[cfg(test)]
use Bind::{Int, Text};

/// Who wrote a review.
#[cfg(test)]
#[derive(Clone, Copy)]
enum Reviewer {
    Agent,
    Human,
}

/// An open proposal with its task workspace and its current version.
#[cfg(test)]
#[derive(Clone, Copy)]
struct Proposal {
    task: u32,
    id: i64,
    workspace: i64,
    version: i64,
}

/// Tasks come from `create_task`. The rows other domains own (workspaces, proposals, versions, reviews) are inserted here the way their commands write them, and recompute then runs as those commands will run it.
#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
    next_number: Cell<i64>,
}

#[cfg(test)]
impl Fixture {
    async fn new(count: u32) -> Self {
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
        let fixture = Self {
            project,
            tasks,
            next_number: Cell::new(1),
        };
        fixture
            .run(
                "INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 1)",
                &[],
            )
            .await;
        fixture
            .run(
                "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'api', 1)",
                &[],
            )
            .await;
        fixture
            .run(
                "INSERT INTO revisions (id, number, hash, tree_hash, created_at)
                 VALUES (1, 1, 'h1', 't1', 1)",
                &[],
            )
            .await;
        fixture
    }

    /// Runs one statement in its own write and returns the last inserted row id.
    async fn run(&self, sql: &'static str, binds: &[Bind]) -> i64 {
        self.project
            .store()
            .write(async |tx| {
                let mut query = sqlx::query(sql);
                for bind in binds {
                    query = match *bind {
                        Int(value) => query.bind(value),
                        Text(value) => query.bind(value),
                    };
                }
                let result = query.execute(tx.conn()).await?;
                Ok::<_, StoreError>(result.last_insert_rowid())
            })
            .await
            .unwrap()
    }

    fn number(&self) -> i64 {
        let number = self.next_number.get();
        self.next_number.set(number + 1);
        number
    }

    async fn workspace(&self, task: u32, kind: &'static str, state: &'static str) -> i64 {
        self.run(
            "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at)
             VALUES (?1, (SELECT id FROM tasks WHERE number = ?2), 1, ?3, 'ws/' || ?1, 1, ?4, 1)",
            &[Int(self.number()), Int(task.into()), Text(kind), Text(state)],
        )
        .await
    }

    async fn set_workspace_state(&self, workspace: i64, state: &'static str) {
        self.run(
            "UPDATE workspaces SET state = ? WHERE id = ?",
            &[Text(state), Int(workspace)],
        )
        .await;
    }

    /// A task workspace, an open proposal and its version 1.
    async fn open_proposal(&self, task: u32) -> Proposal {
        let workspace = self.workspace(task, "task", "active").await;
        let id = self
            .run(
                "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
                 VALUES (?, (SELECT id FROM tasks WHERE number = ?), 1, 'proposal', 1, 1)",
                &[Int(self.number()), Int(task.into())],
            )
            .await;
        let mut proposal = Proposal {
            task,
            id,
            workspace,
            version: 0,
        };
        proposal.version = self.new_version(&proposal, 1).await;
        proposal
    }

    async fn new_version(&self, proposal: &Proposal, version: i64) -> i64 {
        let changeset = self
            .run(
                "INSERT INTO changesets (number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at)
                 VALUES (?, ?, (SELECT id FROM tasks WHERE number = ?), 1, 't2', 'change', 'backend/api', 1)",
                &[
                    Int(self.number()),
                    Int(proposal.workspace),
                    Int(proposal.task.into()),
                ],
            )
            .await;
        self.run(
            "INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at)
             VALUES (?, ?, 1, 't2', ?, 'backend/api', 1)",
            &[Int(proposal.id), Int(version), Int(changeset)],
        )
        .await
    }

    async fn review(
        &self,
        version: i64,
        verdict: &'static str,
        created_at: i64,
        reviewer: Reviewer,
    ) {
        let sql = match reviewer {
            Reviewer::Agent => {
                "INSERT INTO reviews (version_id, reviewer_worker_id, verdict, body, created_at)
                 VALUES (?, 1, ?, 'review', ?)"
            }
            Reviewer::Human => {
                "INSERT INTO reviews (version_id, reviewer_human, verdict, body, created_at)
                 VALUES (?, 'ana', ?, 'review', ?)"
            }
        };
        self.run(sql, &[Int(version), Text(verdict), Int(created_at)])
            .await;
    }

    async fn set_proposal(&self, proposal: &Proposal, state: &'static str, conflicted: bool) {
        self.run(
            "UPDATE proposals SET state = ?, close_reason = CASE ? WHEN 'closed' THEN 'replaced' END, conflicted = ? WHERE id = ?",
            &[Text(state), Text(state), Int(conflicted.into()), Int(proposal.id)],
        )
        .await;
    }

    /// Recompute for `task`, as the command that wrote the rows above runs it; returns what it appended.
    async fn recompute(&self, task: u32) -> Vec<Event> {
        let now = self.project.now();
        self.project
            .store()
            .write(async |tx| {
                let recomputed = tasks::recompute(tx, &[t(task)], now).await?;
                Ok::<_, TaskError>(recomputed.events.events().to_vec())
            })
            .await
            .unwrap()
    }

    /// Recomputes and returns the status the cache holds afterwards.
    async fn settle(&self, task: u32) -> TaskStatus {
        self.recompute(task).await;
        self.status(task).await
    }

    async fn status(&self, task: u32) -> TaskStatus {
        self.tasks.get(t(task)).await.unwrap().status
    }

    async fn blocked(&self, task: u32) -> bool {
        self.tasks.get(t(task)).await.unwrap().blocked
    }
}

#[tokio::test]
async fn an_active_or_missing_task_workspace_counts_as_active() {
    let fx = Fixture::new(1).await;
    let workspace = fx.workspace(1, "task", "missing").await;
    assert_eq!(fx.settle(1).await, InProgress);

    fx.set_workspace_state(workspace, "active").await;
    assert!(fx.recompute(1).await.is_empty());
    assert_eq!(fx.status(1).await, InProgress);

    for closed in ["abandoned", "archived"] {
        fx.set_workspace_state(workspace, closed).await;
        assert_eq!(fx.settle(1).await, Todo, "{closed}");
        fx.set_workspace_state(workspace, "active").await;
        assert_eq!(fx.settle(1).await, InProgress);
    }
}

#[tokio::test]
async fn a_review_workspace_is_not_a_task_workspace() {
    let fx = Fixture::new(1).await;
    fx.workspace(1, "review", "active").await;
    assert!(fx.recompute(1).await.is_empty());
    assert_eq!(fx.status(1).await, Todo);
}

#[tokio::test]
async fn an_open_proposal_gives_review_and_a_closed_one_does_not() {
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    assert_eq!(fx.settle(1).await, Review);

    fx.set_proposal(&proposal, "closed", false).await;
    assert_eq!(fx.settle(1).await, InProgress);
}

#[tokio::test]
async fn the_latest_verdict_on_the_current_version_decides() {
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    assert_eq!(fx.settle(1).await, Review);

    fx.review(proposal.version, "request_changes", 10, Reviewer::Agent)
        .await;
    let appended = fx.recompute(1).await;
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].event_type, types::TASK_STATUS_CHANGED);
    assert_eq!(
        appended[0].data,
        json!({"from": "review", "to": "in_progress"})
    );
    assert_eq!(fx.status(1).await, InProgress);

    fx.review(proposal.version, "approve", 20, Reviewer::Agent)
        .await;
    assert_eq!(fx.settle(1).await, Review);
}

#[tokio::test]
async fn a_comment_review_changes_nothing() {
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    fx.recompute(1).await;

    fx.review(proposal.version, "comment", 10, Reviewer::Agent)
        .await;
    assert!(fx.recompute(1).await.is_empty());
    assert_eq!(fx.status(1).await, Review);

    fx.review(proposal.version, "request_changes", 20, Reviewer::Agent)
        .await;
    assert_eq!(fx.settle(1).await, InProgress);
    fx.review(proposal.version, "comment", 30, Reviewer::Human)
        .await;
    assert!(fx.recompute(1).await.is_empty());
    assert_eq!(fx.status(1).await, InProgress);
}

#[tokio::test]
async fn a_conflicted_proposal_gives_in_progress() {
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    assert_eq!(fx.settle(1).await, Review);

    fx.set_proposal(&proposal, "open", true).await;
    assert_eq!(fx.settle(1).await, InProgress);
    fx.set_proposal(&proposal, "open", false).await;
    assert_eq!(fx.settle(1).await, Review);
}

#[tokio::test]
async fn a_new_version_without_reviews_goes_back_to_the_reviewer() {
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    fx.review(proposal.version, "request_changes", 10, Reviewer::Agent)
        .await;
    assert_eq!(fx.settle(1).await, InProgress);

    fx.new_version(&proposal, 2).await;
    assert_eq!(fx.settle(1).await, Review);
}

#[tokio::test]
async fn a_human_approval_after_request_changes_is_the_latest_verdict() {
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    fx.review(proposal.version, "request_changes", 10, Reviewer::Agent)
        .await;
    assert_eq!(fx.settle(1).await, InProgress);

    fx.review(proposal.version, "approve", 20, Reviewer::Human)
        .await;
    assert_eq!(fx.settle(1).await, Review);
}

#[tokio::test]
async fn verdicts_are_ordered_by_time_then_by_row() {
    // Equal times: the later row wins.
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    fx.review(proposal.version, "approve", 10, Reviewer::Agent)
        .await;
    fx.review(proposal.version, "request_changes", 10, Reviewer::Human)
        .await;
    assert_eq!(fx.settle(1).await, InProgress);

    // An older verdict written later does not win.
    let fx = Fixture::new(1).await;
    let proposal = fx.open_proposal(1).await;
    fx.review(proposal.version, "approve", 20, Reviewer::Agent)
        .await;
    fx.review(proposal.version, "request_changes", 10, Reviewer::Agent)
        .await;
    assert_eq!(fx.settle(1).await, Review);
}

#[tokio::test]
async fn a_started_task_is_blocked_by_an_unfinished_prerequisite_until_it_is_done() {
    let fx = Fixture::new(2).await;
    fx.workspace(1, "task", "active").await;
    assert_eq!(fx.settle(1).await, InProgress);
    let head = fold_store::events::head_seq(fx.project.store().read())
        .await
        .unwrap();

    fx.tasks.add_dependency(&agent(), t(1), t(2)).await.unwrap();
    assert_eq!(
        (fx.status(1).await, fx.blocked(1).await),
        (InProgress, true)
    );
    let appended = fold_store::events::read_after(
        fx.project.store().read(),
        head,
        &fold_core::EventFilter::default(),
        10,
    )
    .await
    .unwrap();
    let blocked: Vec<&Event> = appended
        .iter()
        .filter(|event| event.event_type == types::TASK_BLOCKED_CHANGED)
        .collect();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].task, Some(t(1)));
    assert_eq!(blocked[0].actor, EventActor::fold());
    assert_eq!(
        blocked[0].data,
        json!({"blocked": true, "blocked_by": ["T-2"]})
    );
    assert!(
        !appended
            .iter()
            .any(|event| event.event_type == types::TASK_STATUS_CHANGED)
    );

    let now = fx.project.now();
    let appended = fx
        .project
        .store()
        .write(async |tx| {
            completions::insert(tx, t(2), CompletionKind::Children, None, now).await?;
            let recomputed = tasks::recompute(tx, &[t(2)], now).await?;
            Ok::<_, TaskError>(recomputed.events.events().to_vec())
        })
        .await
        .unwrap();
    assert_eq!(fx.status(2).await, Done);
    assert_eq!(
        (fx.status(1).await, fx.blocked(1).await),
        (InProgress, false)
    );
    let data: Vec<_> = appended
        .iter()
        .map(|event| (event.event_type, event.task, event.data.clone()))
        .collect();
    assert_eq!(
        data,
        [
            (
                types::TASK_STATUS_CHANGED,
                Some(t(2)),
                json!({"from": "todo", "to": "done"})
            ),
            (
                types::TASK_BLOCKED_CHANGED,
                Some(t(1)),
                json!({"blocked": false, "blocked_by": []})
            ),
        ]
    );
}

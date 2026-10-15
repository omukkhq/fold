use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, ChecklistName, Event, EventFilter, EventType, LabelColor, LabelName,
    StepText, TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName,
};
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_daemon::services::tasks::{
    self, CreateTask, ItemContent, NewChecklistItem, TaskError, TaskService,
};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::{StoreError, events};
use serde_json::{Value, json};
use tokio::sync::broadcast::Receiver;

use TaskStatus::{Backlog, Done, InProgress, Todo};

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

/// Tasks and checklist items come from their commands. The rows other domains own (workspaces, proposals, claims) are inserted the way their commands write them.
#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
    bus: EventBus,
}

#[cfg(test)]
impl Fixture {
    async fn new(count: u32) -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            Arc::new(project.clock().clone()),
            fold_core::ProjectConfig::default().lease_minutes,
        );
        tasks
            .create_label(&agent(), LabelName::parse("api").unwrap(), LabelColor::Sage)
            .await
            .unwrap();
        for _ in 0..count {
            tasks
                .create_task(
                    &agent(),
                    CreateTask {
                        title: TaskTitle::parse("task").unwrap(),
                        description: TaskDescription::new("body"),
                        labels: vec![LabelName::parse("api").unwrap()],
                    },
                )
                .await
                .unwrap();
        }
        let fixture = Self {
            project,
            tasks,
            bus,
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

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }

    async fn recompute(&self, task: u32) {
        let now = self.project.now();
        self.project
            .store()
            .write(async |tx| {
                tasks::recompute(tx, &[t(task)], now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }

    async fn needs(&self, task: u32, needs: u32) {
        self.tasks
            .add_dependency(&agent(), t(task), t(needs))
            .await
            .unwrap();
    }

    /// An item on `owner`'s `parts` checklist linked to `linked`, with its checklist edge.
    async fn linked_item(&self, owner: u32, linked: u32) {
        self.tasks
            .add_checklist_item(
                &agent(),
                t(owner),
                NewChecklistItem {
                    content: ItemContent::Linked(t(linked)),
                    checklist: Some(ChecklistName::parse("parts").unwrap()),
                },
            )
            .await
            .unwrap();
    }

    async fn text_item(&self, owner: u32) {
        self.tasks
            .add_checklist_item(
                &agent(),
                t(owner),
                NewChecklistItem {
                    content: ItemContent::Step(StepText::parse("write tests").unwrap()),
                    checklist: Some(ChecklistName::parse("notes").unwrap()),
                },
            )
            .await
            .unwrap();
    }

    async fn workspace(&self, task: u32, state: &'static str) -> i64 {
        let id = self
            .run(
                "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at)
                 VALUES (?, (SELECT id FROM tasks WHERE number = ?), 1, 'task', 'ws', 1, ?, 1)",
                &[Int(task.into()), Int(task.into()), Text(state)],
            )
            .await;
        self.recompute(task).await;
        id
    }

    async fn claim(&self, task: u32) {
        self.run(
            "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at)
             VALUES ('implementation', (SELECT id FROM tasks WHERE number = ?), 1, 1, 1)",
            &[Int(task.into())],
        )
        .await;
    }

    /// A task workspace, a proposal in `state` and its version 1.
    async fn proposal(&self, task: u32, state: &'static str) {
        let workspace = self.workspace(task, "active").await;
        let proposal = self
            .run(
                "INSERT INTO proposals (number, task_id, author_worker_id, title, state, created_at, updated_at)
                 VALUES (?, (SELECT id FROM tasks WHERE number = ?), 1, 'proposal', ?, 1, 1)",
                &[Int(task.into()), Int(task.into()), Text(state)],
            )
            .await;
        let changeset = self
            .run(
                "INSERT INTO changesets (number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at)
                 VALUES (?, ?, (SELECT id FROM tasks WHERE number = ?), 1, 't2', 'change', 'backend/api', 1)",
                &[Int(task.into()), Int(workspace), Int(task.into())],
            )
            .await;
        self.run(
            "INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_at)
             VALUES (?, 1, 1, 't2', ?, 1)",
            &[Int(proposal), Int(changeset)],
        )
        .await;
        self.recompute(task).await;
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

    async fn status(&self, task: u32) -> TaskStatus {
        self.tasks.get(t(task)).await.unwrap().status
    }

    async fn events_of(&self, task: u32) -> Vec<Event> {
        let filter = EventFilter {
            task: Some(t(task)),
            ..EventFilter::default()
        };
        events::read_after(self.project.store().read(), 0, &filter, 500)
            .await
            .unwrap()
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.project.store().read()).await.unwrap()
    }

    async fn events_after(&self, after: u64) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            after,
            &EventFilter::default(),
            500,
        )
        .await
        .unwrap()
    }
}

#[cfg(test)]
fn of_type(events: &[Event], event_type: EventType) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

#[cfg(test)]
fn refusal(error: TaskError) -> (String, Value, String) {
    let TaskError::Fold(error) = error else {
        panic!("expected a refusal, got {error:?}");
    };
    (
        error.code().to_owned(),
        error.details().cloned().map_or(Value::Null, Value::Object),
        error.hint().unwrap_or_default().to_owned(),
    )
}

#[cfg(test)]
fn published(receiver: &mut Receiver<BusMessage>) -> Vec<EventType> {
    let mut types = Vec::new();
    while let Ok(BusMessage::Durable(event)) = receiver.try_recv() {
        types.push(event.event_type);
    }
    types
}

/// The rows that mention a task, counted over the tables that can hold them.
#[cfg(test)]
async fn mentions(fx: &Fixture) -> [i64; 6] {
    [
        fx.count("SELECT COUNT(*) FROM tasks WHERE number = 1").await,
        fx.count(
            "SELECT COUNT(*) FROM dependencies d JOIN tasks t ON t.id IN (d.task_id, d.needs_task_id) WHERE t.number = 1",
        )
        .await,
        fx.count(
            "SELECT COUNT(*) FROM checklist_items i JOIN tasks t ON t.id = i.linked_task_id WHERE t.number = 1",
        )
        .await,
        fx.count(
            "SELECT COUNT(*) FROM checklists c JOIN tasks t ON t.id = c.task_id WHERE t.number = 1",
        )
        .await,
        fx.count(
            "SELECT COUNT(*) FROM task_labels l JOIN tasks t ON t.id = l.task_id WHERE t.number = 1",
        )
        .await,
        fx.count(
            "SELECT COUNT(*) FROM claims c JOIN tasks t ON t.id = c.task_id WHERE t.number = 1",
        )
        .await,
    ]
}

#[tokio::test]
async fn deleting_a_fresh_task_removes_every_row_that_mentions_it_and_frees_its_dependents() {
    // T-2 needs T-1 directly; T-3 holds a linked item for T-1; T-1 needs T-4 and has its own checklists, a label and a claim.
    let fx = Fixture::new(5).await;
    fx.needs(2, 1).await;
    fx.linked_item(3, 1).await;
    fx.needs(1, 4).await;
    fx.linked_item(1, 5).await;
    fx.text_item(1).await;
    fx.claim(1).await;
    assert_eq!(fx.status(2).await, Backlog);
    assert_eq!(fx.status(3).await, Backlog);
    assert_eq!(mentions(&fx).await, [1, 4, 1, 2, 1, 1]);
    let before = fx.head().await;
    let mut receiver = fx.bus.subscribe();

    fx.tasks.delete_task(&agent(), t(1)).await.unwrap();

    assert_eq!(mentions(&fx).await, [0; 6]);
    assert_eq!(fx.count("SELECT COUNT(*) FROM checklists").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM checklist_items").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM dependencies").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM labels").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM claims").await, 0);
    assert_eq!(fx.status(2).await, Todo);
    assert_eq!(fx.status(3).await, Todo);
    let error = fx.tasks.get(t(1)).await.unwrap_err();
    assert_eq!(error.code(), Some("task_not_found"));

    let appended = fx.events_after(before).await;
    let deleted = of_type(&appended, types::TASK_DELETED);
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].task, Some(t(1)));
    assert_eq!(deleted[0].actor, agent().event_actor());
    assert_eq!(deleted[0].data, json!({"number": 1, "title": "task"}));
    let removed: Vec<(Option<TaskId>, Value)> = of_type(&appended, types::DEPENDENCY_REMOVED)
        .into_iter()
        .map(|event| (event.task, event.data.clone()))
        .collect();
    assert_eq!(
        removed,
        vec![
            (Some(t(1)), json!({"needs": "T-4", "origin": "direct"})),
            (Some(t(1)), json!({"needs": "T-5", "origin": "checklist"})),
            (Some(t(2)), json!({"needs": "T-1", "origin": "direct"})),
            (Some(t(3)), json!({"needs": "T-1", "origin": "checklist"})),
        ]
    );
    let items = of_type(&appended, types::CHECKLIST_ITEM_REMOVED);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].task, Some(t(3)));
    assert_eq!(items[0].data, json!({"item": "ci-1", "linked_task": "T-1"}));
    let changed: Vec<Option<TaskId>> = of_type(&appended, types::TASK_STATUS_CHANGED)
        .into_iter()
        .map(|event| event.task)
        .collect();
    assert_eq!(changed, vec![Some(t(2)), Some(t(3))]);
    let types_published = published(&mut receiver);
    assert_eq!(
        types_published,
        appended
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn the_events_of_a_deleted_task_stay_readable_and_its_number_is_never_reused() {
    let fx = Fixture::new(1).await;
    fx.tasks.delete_task(&agent(), t(1)).await.unwrap();

    let history: Vec<EventType> = fx
        .events_of(1)
        .await
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(
        history,
        vec![
            types::TASK_CREATED,
            types::TASK_STATUS_CHANGED,
            types::TASK_DELETED
        ]
    );

    let next = fx
        .tasks
        .create_task(
            &agent(),
            CreateTask {
                title: TaskTitle::parse("next").unwrap(),
                description: TaskDescription::new(""),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(next.id, t(2));
}

#[tokio::test]
async fn delete_after_a_workspace_a_proposal_a_completion_or_a_message_is_refused() {
    let fx = Fixture::new(5).await;
    fx.workspace(1, "abandoned").await;
    fx.proposal(2, "closed").await;
    fx.complete(3).await;
    fx.workspace(4, "active").await;
    fx.run(
        "INSERT INTO messages (number, thread_id, sender_kind, sender_human, task_id, body, created_at)
         VALUES (1, 1, 'human', 'ana', (SELECT id FROM tasks WHERE number = ?), 'hello', 1)",
        &[Int(5)],
    )
    .await;

    for (task, kept) in [
        (1, json!(["workspace"])),
        (2, json!(["workspace", "proposal"])),
        (3, json!(["completion"])),
        (4, json!(["workspace"])),
        (5, json!(["message"])),
    ] {
        let before = fx.head().await;
        let error = fx.tasks.delete_task(&agent(), t(task)).await.unwrap_err();
        let (code, details, hint) = refusal(error);
        assert_eq!(code, "invalid_request");
        assert_eq!(
            details,
            json!({"reason": "task_has_history", "task": t(task).to_string(), "history": kept})
        );
        assert!(hint.contains("archive_task"), "{hint}");
        assert!(fx.tasks.get(t(task)).await.is_ok());
        assert_eq!(fx.head().await, before);
    }
}

#[tokio::test]
async fn a_missing_task_is_not_found_for_delete_and_archive() {
    let fx = Fixture::new(0).await;
    let error = fx.tasks.delete_task(&agent(), t(7)).await.unwrap_err();
    assert_eq!(error.code(), Some("task_not_found"));
    let error = fx.tasks.archive_task(&agent(), t(7)).await.unwrap_err();
    assert_eq!(error.code(), Some("task_not_found"));
}

#[tokio::test]
async fn archive_removes_edges_and_linked_items_archives_the_workspace_releases_the_claim_and_frees_dependents()
 {
    // T-1 is in progress with a workspace and a claim; T-2 needs it, T-3 holds a linked item for it, and it needs T-4.
    let fx = Fixture::new(4).await;
    fx.needs(2, 1).await;
    fx.linked_item(3, 1).await;
    fx.needs(1, 4).await;
    fx.complete(4).await;
    let workspace = fx.workspace(1, "active").await;
    fx.claim(1).await;
    assert_eq!(fx.status(1).await, InProgress);
    assert_eq!(fx.status(2).await, Backlog);
    let before = fx.head().await;
    fx.project
        .clock()
        .advance(std::time::Duration::from_secs(60))
        .unwrap();
    let now = fx.project.now();

    let view = fx.tasks.archive_task(&agent(), t(1)).await.unwrap();

    assert_eq!(view.archived_at, Some(now));
    assert_eq!(view.updated_at, now);
    assert_eq!(view.status, InProgress);
    assert_eq!(view.labels.len(), 1);
    assert_eq!(fx.status(2).await, Todo);
    assert_eq!(fx.status(3).await, Todo);
    let [task_rows, edges, linked, checklists, labels, claims] = mentions(&fx).await;
    assert_eq!([task_rows, edges, linked, labels, claims], [1, 0, 0, 1, 1]);
    assert_eq!(checklists, 0);
    let state: String = sqlx::query_scalar("SELECT state FROM workspaces WHERE id = ?")
        .bind(workspace)
        .fetch_one(fx.project.store().read())
        .await
        .unwrap();
    assert_eq!(state, "archived");
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE closed_at IS NOT NULL")
            .await,
        1
    );
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM claims WHERE released_at IS NOT NULL AND release_reason = 'task_archived'"
        )
        .await,
        1
    );
    let listed: Vec<TaskId> = fx
        .tasks
        .list(false)
        .await
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect();
    assert_eq!(listed, vec![t(2), t(3), t(4)]);

    let appended = fx.events_after(before).await;
    let archived = of_type(&appended, types::TASK_ARCHIVED);
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].task, Some(t(1)));
    let workspaces = of_type(&appended, types::WORKSPACE_ARCHIVED);
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].workspace.map(|w| w.number()), Some(1));
    assert!(of_type(&appended, types::WORKSPACE_ABANDONED).is_empty());
    let released = of_type(&appended, types::CLAIM_RELEASED);
    assert_eq!(released.len(), 1);
    assert_eq!(
        released[0].data,
        json!({"kind": "implementation", "worker": "api", "reason": "task_archived"})
    );
    assert_eq!(of_type(&appended, types::DEPENDENCY_REMOVED).len(), 3);
    let items = of_type(&appended, types::CHECKLIST_ITEM_REMOVED);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].task, Some(t(3)));
    assert_eq!(items[0].data, json!({"item": "ci-1", "linked_task": "T-1"}));
    // Recompute skips the archived task, so only the dependents move.
    let changed: Vec<Option<TaskId>> = of_type(&appended, types::TASK_STATUS_CHANGED)
        .into_iter()
        .map(|event| event.task)
        .collect();
    assert_eq!(changed, vec![Some(t(2)), Some(t(3))]);
}

#[tokio::test]
async fn archive_without_a_workspace_or_a_claim_appends_neither_event() {
    let fx = Fixture::new(1).await;
    let before = fx.head().await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    let appended: Vec<EventType> = fx
        .events_after(before)
        .await
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(appended, vec![types::TASK_ARCHIVED]);
}

#[tokio::test]
async fn archive_archives_a_missing_workspace_too() {
    let fx = Fixture::new(1).await;
    fx.workspace(1, "missing").await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE state = 'archived'")
            .await,
        1
    );
}

#[tokio::test]
async fn archive_with_an_open_proposal_is_refused_and_changes_nothing() {
    let fx = Fixture::new(2).await;
    fx.needs(2, 1).await;
    fx.proposal(1, "open").await;
    fx.claim(1).await;
    let before = fx.head().await;

    let error = fx.tasks.archive_task(&agent(), t(1)).await.unwrap_err();

    let (code, details, hint) = refusal(error);
    assert_eq!(code, "invalid_request");
    assert_eq!(details, json!({"reason": "proposal_open", "task": "T-1"}));
    assert!(hint.contains("close_proposal"), "{hint}");
    assert_eq!(fx.head().await, before);
    assert!(fx.tasks.get(t(1)).await.unwrap().archived_at.is_none());
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM proposals WHERE state = 'open'")
            .await,
        1
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE state = 'active'")
            .await,
        1
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM dependencies").await, 1);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE released_at IS NULL")
            .await,
        1
    );
}

#[tokio::test]
async fn archive_after_the_proposal_closed_is_allowed() {
    let fx = Fixture::new(1).await;
    fx.proposal(1, "closed").await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    assert!(fx.tasks.get(t(1)).await.unwrap().archived_at.is_some());
}

#[tokio::test]
async fn archiving_an_archived_task_is_refused() {
    let fx = Fixture::new(1).await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    let before = fx.head().await;
    let error = fx.tasks.archive_task(&agent(), t(1)).await.unwrap_err();
    let (code, details, _) = refusal(error);
    assert_eq!(code, "task_closed");
    assert_eq!(details, json!({"reason": "archived", "task": "T-1"}));
    assert_eq!(fx.head().await, before);
}

#[tokio::test]
async fn a_done_task_can_be_archived_and_its_dependents_stay_free() {
    let fx = Fixture::new(2).await;
    fx.needs(2, 1).await;
    fx.complete(1).await;
    assert_eq!(fx.status(1).await, Done);
    assert_eq!(fx.status(2).await, Todo);

    let view = fx.tasks.archive_task(&agent(), t(1)).await.unwrap();

    assert_eq!(view.status, Done);
    assert_eq!(fx.status(2).await, Todo);
    assert_eq!(fx.count("SELECT COUNT(*) FROM dependencies").await, 0);
}

#[tokio::test]
async fn deleting_an_archived_task_without_history_is_allowed() {
    let fx = Fixture::new(1).await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    fx.tasks.delete_task(&agent(), t(1)).await.unwrap();
    assert_eq!(fx.count("SELECT COUNT(*) FROM tasks").await, 0);
}

/// T-2 has started and waits only on T-1; T-3 waits on T-1 and on the unfinished T-4.
#[cfg(test)]
async fn started_and_waiting_dependents() -> Fixture {
    let fx = Fixture::new(4).await;
    fx.needs(2, 1).await;
    fx.needs(3, 1).await;
    fx.needs(3, 4).await;
    fx.workspace(2, "active").await;
    let started = fx.tasks.get(t(2)).await.unwrap();
    assert_eq!((started.status, started.blocked), (InProgress, true));
    assert_eq!(fx.status(3).await, Backlog);
    fx
}

#[cfg(test)]
async fn assert_started_dependent_unblocked(fx: &Fixture, before: u64) {
    let started = fx.tasks.get(t(2)).await.unwrap();
    assert_eq!((started.status, started.blocked), (InProgress, false));
    assert_eq!(fx.status(3).await, Backlog);
    let appended = fx.events_after(before).await;
    let unblocked: Vec<(Option<TaskId>, Value)> = of_type(&appended, types::TASK_BLOCKED_CHANGED)
        .into_iter()
        .map(|event| (event.task, event.data.clone()))
        .collect();
    assert_eq!(
        unblocked,
        vec![(Some(t(2)), json!({"blocked": false, "blocked_by": []}))]
    );
    assert!(of_type(&appended, types::TASK_STATUS_CHANGED).is_empty());
}

#[tokio::test]
async fn deleting_a_prerequisite_unblocks_a_started_dependent_and_leaves_a_waiting_one_in_backlog()
{
    let fx = started_and_waiting_dependents().await;
    let before = fx.head().await;
    fx.tasks.delete_task(&agent(), t(1)).await.unwrap();
    assert_started_dependent_unblocked(&fx, before).await;
}

#[tokio::test]
async fn archiving_a_prerequisite_unblocks_a_started_dependent_and_leaves_a_waiting_one_in_backlog()
{
    let fx = started_and_waiting_dependents().await;
    let before = fx.head().await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    assert_started_dependent_unblocked(&fx, before).await;
}

#[tokio::test]
async fn delete_after_a_missing_workspace_is_refused() {
    let fx = Fixture::new(1).await;
    fx.workspace(1, "missing").await;
    let error = fx.tasks.delete_task(&agent(), t(1)).await.unwrap_err();
    let (code, details, _) = refusal(error);
    assert_eq!(code, "invalid_request");
    assert_eq!(details["reason"], json!("task_has_history"));
    assert_eq!(details["history"], json!(["workspace"]));
}

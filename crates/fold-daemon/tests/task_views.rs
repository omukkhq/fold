use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, AgentName, ChecklistItemId, ChecklistName, DependencyOrigin, Event,
    EventFilter, HumanName, LabelColor, LabelName, StepText, TaskDescription, TaskId, TaskStatus,
    TaskTitle, WorkerName, WorkspaceId, codes,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{
    self, BOARD_DONE_LIMIT, ChecklistCounts, CreateTask, ItemContent, NewChecklistItem, TaskError,
    TaskService,
};
use fold_store::events;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::{TestProject, execute};
use serde_json::{Value, json};

const LEASE: u64 = 30;

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
fn human() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

#[cfg(test)]
fn task(title: &str) -> CreateTask {
    CreateTask {
        title: TaskTitle::parse(title).unwrap(),
        description: TaskDescription::new(""),
        labels: Vec::new(),
    }
}

#[cfg(test)]
fn step(text: &str, checklist: Option<&str>) -> NewChecklistItem {
    NewChecklistItem {
        content: ItemContent::Step(StepText::parse(text).unwrap()),
        checklist: checklist.map(|name| ChecklistName::parse(name).unwrap()),
    }
}

/// The code and the details of a refusal.
#[cfg(test)]
fn refusal(error: TaskError) -> (&'static str, Value, Option<String>) {
    match error {
        TaskError::Fold(error) => (
            error.code(),
            Value::Object(error.details().cloned().unwrap_or_default()),
            error.hint().map(str::to_owned),
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
}

#[cfg(test)]
impl Fixture {
    /// `count` tasks titled `task 1` to `task <count>`, and the agents `frontend` and `backend`.
    async fn new(count: u32) -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let bus = EventBus::new();
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        for name in ["frontend", "backend"] {
            let mut session = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut session, &AgentName::parse(name).unwrap(), &[])
                .await
                .unwrap();
        }
        let tasks = TaskService::new(project.store().clone(), bus, clock, LEASE);
        for n in 1..=count {
            tasks
                .create_task(&human(), task(&format!("task {n}")))
                .await
                .unwrap();
        }
        Self { project, tasks }
    }

    async fn complete(&self, numbers: &[u32]) {
        let now = self.project.now();
        let seeds: Vec<TaskId> = numbers.iter().map(|&n| t(n)).collect();
        self.project
            .store()
            .write(async |tx| {
                for &task in &seeds {
                    completions::insert(tx, task, CompletionKind::Children, None, now).await?;
                }
                tasks::recompute(tx, &seeds, now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }

    async fn need(&self, task: u32, needs: u32) {
        self.tasks
            .add_dependency(&human(), t(task), t(needs))
            .await
            .unwrap();
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.project.store().read()).await.unwrap()
    }

    async fn events_after(&self, after: u64) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            after,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
    }

    /// `(item, position)` of every item of `checklist` on `task`, in position order, from the table itself.
    async fn positions(&self, task: u32, checklist: &str) -> Vec<(String, i64)> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT i.number, i.position FROM checklist_items i
             JOIN checklists c ON c.id = i.checklist_id
             JOIN tasks t ON t.id = c.task_id
             WHERE t.number = ? AND c.name = ? ORDER BY i.position",
        )
        .bind(i64::from(task))
        .bind(checklist)
        .fetch_all(self.project.store().read())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(number, position)| (format!("ci-{number}"), position))
            .collect()
    }
}

#[tokio::test]
async fn get_view_shows_checklists_both_edge_directions_the_claim_and_the_workspace() {
    let fx = Fixture::new(4).await;
    fx.need(1, 2).await;
    fx.need(3, 1).await;
    fx.tasks
        .add_checklist_item(&human(), t(1), step("wire it", None))
        .await
        .unwrap();
    fx.tasks
        .add_checklist_item(
            &human(),
            t(1),
            NewChecklistItem {
                content: ItemContent::Linked(t(4)),
                checklist: None,
            },
        )
        .await
        .unwrap();
    fx.tasks.claim_task(&agent("frontend"), t(1)).await.unwrap();
    execute(
        fx.project.store(),
        "INSERT INTO agents (name, created_at) VALUES ('seed', 1);
         INSERT INTO revisions (number, hash, tree_hash, created_at) VALUES (1, 'h1', 't1', 1);
         INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at)
         VALUES (9, (SELECT id FROM tasks WHERE number = 1), 'task', 'ws', 1, 'active', 1);",
    )
    .await;

    let view = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(view.task.title.as_str(), "task 1");
    assert_eq!(view.checklists.len(), 1);
    assert_eq!(view.checklists[0].name.as_str(), "Subtasks");
    let items = &view.checklists[0].items;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].linked, None);
    let linked = items[1].linked.as_ref().unwrap();
    assert_eq!((linked.id, linked.title.as_str()), (t(4), "task 4"));
    assert_eq!(
        view.progress,
        ChecklistCounts {
            steps_done: 0,
            steps_total: 1,
            linked_done: 0,
            linked_total: 1,
        }
    );
    let needs: Vec<(TaskId, DependencyOrigin)> = view
        .needs
        .iter()
        .map(|edge| (edge.task.id, edge.origin))
        .collect();
    assert_eq!(
        needs,
        vec![
            (t(2), DependencyOrigin::Direct),
            (t(4), DependencyOrigin::Checklist)
        ]
    );
    let needed_by: Vec<TaskId> = view.needed_by.iter().map(|edge| edge.task.id).collect();
    assert_eq!(needed_by, vec![t(3)]);
    let claim = view.claim.as_ref().unwrap();
    assert_eq!(claim.holder.as_str(), "frontend");
    assert!(!claim.stale);
    assert_eq!(view.workspace, WorkspaceId::from_number(9));
    assert!(view.attention.is_empty());

    fx.project
        .clock()
        .advance(Duration::from_secs(LEASE * 60))
        .unwrap();
    assert!(fx.tasks.get_view(t(1)).await.unwrap().claim.unwrap().stale);

    let missing = fx.tasks.get_view(t(99)).await.unwrap_err();
    assert_eq!(refusal(missing).0, codes::TASK_NOT_FOUND);
}

#[tokio::test]
async fn the_graph_leaves_out_archived_tasks_and_done_tasks_unless_asked() {
    let fx = Fixture::new(4).await;
    fx.need(2, 1).await;
    fx.need(2, 3).await;
    fx.complete(&[1]).await;
    fx.tasks.archive_task(&human(), t(4)).await.unwrap();
    fx.tasks.claim_task(&agent("backend"), t(2)).await.unwrap();

    let open = fx.tasks.graph(false).await.unwrap();
    let ids: Vec<TaskId> = open.iter().map(|task| task.id).collect();
    assert_eq!(ids, vec![t(2), t(3)]);
    assert_eq!(open[0].needs, vec![t(1), t(3)]);
    assert!(open[0].blocked || open[0].status == TaskStatus::Backlog);
    assert_eq!(open[0].holder.as_ref().unwrap().as_str(), "backend");
    assert_eq!(open[1].holder, None);

    let all = fx.tasks.graph(true).await.unwrap();
    let ids: Vec<TaskId> = all.iter().map(|task| task.id).collect();
    assert_eq!(ids, vec![t(1), t(2), t(3)]);
    assert_eq!(all[0].status, TaskStatus::Done);
}

#[tokio::test]
async fn the_board_has_five_columns_with_blocked_by_blocks_and_the_latest_done_cards() {
    let fx = Fixture::new(4).await;
    fx.tasks
        .create_label(&human(), LabelName::parse("api").unwrap(), LabelColor::Teal)
        .await
        .unwrap();
    fx.tasks
        .update_task(
            &human(),
            t(3),
            tasks::TaskPatch {
                labels: Some(vec![LabelName::parse("api").unwrap()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    fx.need(3, 1).await;
    fx.need(3, 2).await;
    fx.need(4, 2).await;
    fx.complete(&[1]).await;
    execute(
        fx.project.store(),
        "INSERT INTO messages (number, thread_id, sender_kind, sender_human, task_id, body, created_at)
         VALUES (1, 1, 'human', 'ana', (SELECT id FROM tasks WHERE number = 3), 'hello', 1),
                (2, 1, 'human', 'ana', (SELECT id FROM tasks WHERE number = 3), 'again', 2);
         UPDATE tasks SET human_review = 1, human_reviewer = 'ana', human_review_reason = 'risky', human_review_set_by = 'human:ana' WHERE number = 4;",
    )
    .await;

    let board = fx.tasks.board().await.unwrap();
    let statuses: Vec<TaskStatus> = board.columns.iter().map(|c| c.status).collect();
    assert_eq!(statuses, TaskStatus::ALL.to_vec());
    let todo = &board.columns[1];
    let todo_ids: Vec<TaskId> = todo.cards.iter().map(|card| card.id).collect();
    assert_eq!(todo_ids, vec![t(2)]);
    assert_eq!(todo.cards[0].blocks, 2);
    let backlog = &board.columns[0];
    let card = backlog.cards.iter().find(|card| card.id == t(3)).unwrap();
    assert!(card.blocked || card.status == TaskStatus::Backlog);
    assert_eq!(card.needs, vec![t(1), t(2)]);
    assert_eq!(card.blocked_by, vec![t(2)]);
    assert_eq!(card.blocks, 0);
    assert_eq!(card.messages, 2);
    assert_eq!(card.labels[0].name.as_str(), "api");
    assert!(card.attention.is_empty());
    assert_eq!(card.human_review, None);
    let reviewed = backlog.cards.iter().find(|card| card.id == t(4)).unwrap();
    assert_eq!(
        reviewed.human_review.as_ref().unwrap().reviewer.as_deref(),
        Some("ana")
    );
    let done = &board.columns[4];
    assert_eq!(done.total, 1);
    assert_eq!(done.cards[0].id, t(1));
    assert_eq!(done.cards[0].blocks, 0);
    assert_eq!(done.cards[0].completed_at, Some(fx.project.now()));
}

#[tokio::test]
async fn the_done_column_keeps_the_latest_completions_and_counts_them_all() {
    let total = u32::try_from(BOARD_DONE_LIMIT).unwrap() + 2;
    let fx = Fixture::new(total).await;
    for n in 1..=total {
        fx.project.clock().advance(Duration::from_secs(1)).unwrap();
        fx.complete(&[n]).await;
    }
    let board = fx.tasks.board().await.unwrap();
    let done = &board.columns[4];
    assert_eq!(done.total, total);
    assert_eq!(done.cards.len(), BOARD_DONE_LIMIT);
    assert_eq!(done.cards[0].id, t(total));
    assert_eq!(done.cards[BOARD_DONE_LIMIT - 1].id, t(3));
}

#[tokio::test]
async fn create_linked_task_adds_edges_and_the_parent_link_in_one_transaction() {
    let fx = Fixture::new(2).await;
    let created = fx
        .tasks
        .create_linked_task(&human(), task("child"), &[t(2), t(2)], Some(t(1)))
        .await
        .unwrap();
    assert_eq!(created.id, t(3));
    let view = fx.tasks.get_view(t(3)).await.unwrap();
    let needs: Vec<TaskId> = view.needs.iter().map(|edge| edge.task.id).collect();
    assert_eq!(needs, vec![t(2)]);
    let parent = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(parent.checklists[0].name.as_str(), "Subtasks");
    assert_eq!(parent.needs[0].task.id, t(3));
    assert_eq!(parent.needs[0].origin, DependencyOrigin::Checklist);
    assert_eq!(parent.task.status, TaskStatus::Backlog);

    let head = fx.head().await;
    let cycle = fx
        .tasks
        .create_linked_task(&human(), task("loop"), &[t(1)], Some(t(1)))
        .await
        .unwrap_err();
    assert_eq!(refusal(cycle).0, codes::DEPENDENCY_CYCLE);
    assert_eq!(fx.head().await, head);
    assert_eq!(fx.tasks.list(true).await.unwrap().len(), 3);
    assert_eq!(fx.tasks.get_view(t(1)).await.unwrap().needs.len(), 1);
}

#[tokio::test]
async fn move_checklist_item_keeps_positions_unique_and_dense() {
    let fx = Fixture::new(1).await;
    let mut ids: Vec<ChecklistItemId> = Vec::new();
    for text in ["a", "b", "c", "d"] {
        let item = fx
            .tasks
            .add_checklist_item(&human(), t(1), step(text, None))
            .await
            .unwrap();
        ids.push(item.id);
    }
    let other = fx
        .tasks
        .add_checklist_item(&human(), t(1), step("x", Some("Later")))
        .await
        .unwrap();
    let name = |id: ChecklistItemId| id.to_string();

    let head = fx.head().await;
    fx.tasks
        .move_checklist_item(&human(), ids[3], 0, None)
        .await
        .unwrap();
    assert_eq!(
        fx.positions(1, "Subtasks").await,
        vec![
            (name(ids[3]), 0),
            (name(ids[0]), 1),
            (name(ids[1]), 2),
            (name(ids[2]), 3)
        ]
    );
    let events = fx.events_after(head).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, types::CHECKLIST_ITEM_UPDATED);
    assert_eq!(events[0].task, Some(t(1)));
    assert_eq!(events[0].data["item"], json!(name(ids[3])));
    assert_eq!(events[0].data["fields"], json!(["position"]));

    fx.tasks
        .move_checklist_item(&human(), ids[3], 99, None)
        .await
        .unwrap();
    assert_eq!(
        fx.positions(1, "Subtasks").await,
        vec![
            (name(ids[0]), 0),
            (name(ids[1]), 1),
            (name(ids[2]), 2),
            (name(ids[3]), 3)
        ]
    );

    let head = fx.head().await;
    let moved = fx
        .tasks
        .move_checklist_item(
            &human(),
            ids[1],
            0,
            Some(ChecklistName::parse("Later").unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(moved.checklist.as_str(), "Later");
    assert_eq!(
        fx.positions(1, "Subtasks").await,
        vec![(name(ids[0]), 0), (name(ids[2]), 1), (name(ids[3]), 2)]
    );
    assert_eq!(
        fx.positions(1, "Later").await,
        vec![(name(ids[1]), 0), (name(other.id), 1)]
    );
    let events = fx.events_after(head).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data["fields"], json!(["position", "checklist"]));
    assert_eq!(events[0].data["checklist"], json!("Later"));

    let head = fx.head().await;
    fx.tasks
        .move_checklist_item(&human(), ids[0], 0, None)
        .await
        .unwrap();
    assert_eq!(
        fx.head().await,
        head,
        "a move to the same place records nothing"
    );

    fx.tasks
        .move_checklist_item(
            &human(),
            ids[2],
            5,
            Some(ChecklistName::parse("Done later").unwrap()),
        )
        .await
        .unwrap();
    let kinds: Vec<_> = fx
        .events_after(head)
        .await
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(
        kinds,
        vec![types::CHECKLIST_CREATED, types::CHECKLIST_ITEM_UPDATED]
    );
    assert_eq!(fx.positions(1, "Done later").await, vec![(name(ids[2]), 0)]);

    let missing = fx
        .tasks
        .move_checklist_item(&human(), ChecklistItemId::from_number(99).unwrap(), 0, None)
        .await
        .unwrap_err();
    assert_eq!(refusal(missing).0, codes::CHECKLIST_ITEM_NOT_FOUND);
    fx.tasks.archive_task(&human(), t(1)).await.unwrap();
    let closed = fx
        .tasks
        .move_checklist_item(&human(), ids[0], 1, None)
        .await
        .unwrap_err();
    assert_eq!(refusal(closed).0, codes::TASK_CLOSED);
}

#[tokio::test]
async fn takeover_claim_moves_only_a_stale_claim() {
    let fx = Fixture::new(3).await;
    let nobody = fx
        .tasks
        .takeover_claim(&agent("backend"), t(1))
        .await
        .unwrap_err();
    let (code, _, hint) = refusal(nobody);
    assert_eq!(code, codes::NOT_CLAIMED);
    assert!(hint.unwrap().contains("claim_task"));

    fx.tasks.claim_task(&agent("frontend"), t(1)).await.unwrap();
    let mine = fx
        .tasks
        .takeover_claim(&agent("frontend"), t(1))
        .await
        .unwrap_err();
    assert_eq!(refusal(mine).0, codes::ALREADY_HOLDER);

    let acquired = fx.project.now();
    fx.project
        .clock()
        .advance(Duration::from_secs(LEASE * 60 - 1))
        .unwrap();
    let early = fx
        .tasks
        .takeover_claim(&agent("backend"), t(1))
        .await
        .unwrap_err();
    let (code, details, hint) = refusal(early);
    assert_eq!(code, codes::CLAIM_NOT_STALE);
    assert_eq!(
        details,
        json!({"holder": "frontend", "renewed_at": acquired, "lease_minutes": LEASE})
    );
    assert!(hint.is_some());

    fx.project.clock().advance(Duration::from_secs(1)).unwrap();
    let head = fx.head().await;
    let claim = fx
        .tasks
        .takeover_claim(&agent("backend"), t(1))
        .await
        .unwrap()
        .claim;
    assert_eq!(claim.holder.as_str(), "backend");
    assert!(!claim.stale);
    let events = fx.events_after(head).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, types::CLAIM_TAKEN_OVER);
    assert_eq!(events[0].actor.as_str(), "backend");
    assert_eq!(
        events[0].data,
        json!({"kind": "implementation", "from": "frontend", "to": "backend"})
    );
    let reason: String =
        sqlx::query_scalar("SELECT release_reason FROM claims WHERE released_at IS NOT NULL")
            .fetch_one(fx.project.store().read())
            .await
            .unwrap();
    assert_eq!(reason, "taken_over");
    let view = fx.tasks.claim_view(t(1)).await.unwrap().unwrap();
    assert_eq!(view.holder.as_str(), "backend");

    fx.complete(&[2]).await;
    let done = fx
        .tasks
        .takeover_claim(&agent("backend"), t(2))
        .await
        .unwrap_err();
    let (code, details, _) = refusal(done);
    assert_eq!(
        (code, &details["reason"]),
        (codes::TASK_CLOSED, &json!("done"))
    );
    fx.tasks.archive_task(&human(), t(3)).await.unwrap();
    let archived = fx
        .tasks
        .takeover_claim(&agent("backend"), t(3))
        .await
        .unwrap_err();
    let (_, details, _) = refusal(archived);
    assert_eq!(details["reason"], json!("archived"));
    let by_human = fx.tasks.takeover_claim(&human(), t(1)).await.unwrap_err();
    assert_eq!(refusal(by_human).0, codes::ACTOR_CANNOT_CLAIM);
}

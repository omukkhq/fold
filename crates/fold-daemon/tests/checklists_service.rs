use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, ChecklistItemId, ChecklistName, Event, EventActor, EventFilter, EventType,
    HumanName, StepText, Tag, TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName,
};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{
    self, ChecklistItemPatch, ChecklistProgress, CreateTask, ItemContent, NewChecklistItem,
    TaskError, TaskService,
};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::{StoreError, events};
use serde_json::{Value, json};

use TaskStatus::{Backlog, Done, InProgress, Todo};

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn ci(number: u32) -> ChecklistItemId {
    ChecklistItemId::from_number(number).unwrap()
}

/// `backend` with the `main` tag, so it may change every checklist.
#[cfg(test)]
fn agent() -> Actor {
    let worker = WorkerName::parse("backend").unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: vec![Tag::parse("main").unwrap()],
    })
}

/// An agent that neither holds nor leads any task, so the checklist rights refuse it.
#[cfg(test)]
fn stranger() -> Actor {
    let worker = WorkerName::parse("frontend").unwrap();
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
fn linked(child: u32) -> NewChecklistItem {
    NewChecklistItem {
        content: ItemContent::Linked(t(child)),
        checklist: None,
    }
}

#[cfg(test)]
fn step(text: &str) -> NewChecklistItem {
    NewChecklistItem {
        content: ItemContent::Step(StepText::parse(text).unwrap()),
        checklist: None,
    }
}

#[cfg(test)]
fn in_checklist(mut item: NewChecklistItem, name: &str) -> NewChecklistItem {
    item.checklist = Some(ChecklistName::parse(name).unwrap());
    item
}

/// The code and the details of a refusal.
#[cfg(test)]
fn refusal(error: TaskError) -> (&'static str, Value) {
    match error {
        TaskError::Fold(error) => (
            error.code(),
            Value::Object(error.details().cloned().unwrap_or_default()),
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
    async fn new(count: u32) -> Self {
        let project = TestProject::new().await;
        let tasks = TaskService::new(
            project.store().clone(),
            EventBus::new(),
            Arc::new(project.clock().clone()),
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
        Self { project, tasks }
    }

    async fn add(&self, task: u32, item: NewChecklistItem) -> ChecklistItemId {
        self.tasks
            .add_checklist_item(&agent(), t(task), item)
            .await
            .unwrap()
            .id
    }

    /// Each checklist of `task` as its name and its item count.
    async fn shape(&self, task: u32) -> Vec<(String, usize)> {
        self.tasks
            .checklists(t(task))
            .await
            .unwrap()
            .into_iter()
            .map(|checklist| (checklist.name.to_string(), checklist.items.len()))
            .collect()
    }

    async fn status(&self, task: u32) -> TaskStatus {
        self.tasks.get(t(task)).await.unwrap().status
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

    /// Writes a completion for each of `numbers` and recomputes from them in one transaction, as a completing command does.
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

    async fn completion_kind(&self, task: u32) -> Option<String> {
        sqlx::query_scalar(
            "SELECT c.kind FROM completions c JOIN tasks t ON t.id = c.task_id WHERE t.number = ?",
        )
        .bind(i64::from(task))
        .fetch_optional(self.project.store().read())
        .await
        .unwrap()
    }

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }

    /// A task workspace row for `task` in `state`, inserted as the workspace command writes it, then a recompute.
    async fn workspace(&self, task: u32, state: &'static str) {
        let now = self.project.now();
        self.project
            .store()
            .write(async |tx| {
                for sql in [
                    "INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 1)",
                    "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'api', 1)",
                    "INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 1)",
                ] {
                    sqlx::query(sql)
                        .execute(tx.conn())
                        .await
                        .map_err(StoreError::from)?;
                }
                sqlx::query(
                    "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at)
                     VALUES (1, (SELECT id FROM tasks WHERE number = ?), 1, 'task', 'ws', 1, ?, 1)",
                )
                .bind(i64::from(task))
                .bind(state)
                .execute(tx.conn())
                .await
                .map_err(StoreError::from)?;
                tasks::recompute(tx, &[t(task)], now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }
}

#[cfg(test)]
fn of_type(events: &[Event], event_type: EventType) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

#[tokio::test]
async fn a_parent_of_two_linked_children_completes_by_children_once_both_are_done() {
    let fx = Fixture::new(3).await;
    let first = fx.add(1, linked(2)).await;
    let second = fx.add(1, linked(3)).await;
    assert_eq!(fx.status(1).await, Backlog);
    assert_eq!(
        fx.tasks.checklist_progress(t(1)).await.unwrap(),
        ChecklistProgress { done: 0, total: 2 }
    );

    fx.complete(&[2]).await;
    assert_eq!(fx.status(1).await, Backlog);
    assert_eq!(fx.completion_kind(1).await, None);
    let item = fx.tasks.checklist_item(first).await.unwrap();
    assert_eq!(item.done_by.as_deref(), Some("fold"));
    assert_eq!(fx.tasks.checklist_item(second).await.unwrap().done_at, None);

    let before = fx.head().await;
    fx.complete(&[3]).await;
    assert_eq!(fx.status(1).await, Done);
    assert_eq!(fx.completion_kind(1).await.as_deref(), Some("children"));
    let checklists = fx.tasks.checklists(t(1)).await.unwrap();
    assert_eq!(checklists.len(), 1);
    assert_eq!(checklists[0].name.as_str(), "Subtasks");
    assert!(
        checklists[0]
            .items
            .iter()
            .all(|item| item.done_by.as_deref() == Some("fold") && item.done_at.is_some())
    );
    assert_eq!(
        fx.tasks.checklist_progress(t(1)).await.unwrap(),
        ChecklistProgress { done: 2, total: 2 }
    );

    // The tick, the completion and the status change are stored by the transaction that completed the last child.
    let after = fx.events_after(before).await;
    let ticked = of_type(&after, types::CHECKLIST_ITEM_DONE);
    assert_eq!(ticked.len(), 1);
    assert_eq!(ticked[0].task, Some(t(1)));
    assert_eq!(ticked[0].actor, EventActor::fold());
    assert_eq!(
        ticked[0].data,
        json!({"item": second.to_string(), "linked_task": "T-3"})
    );
    let completed = of_type(&after, types::TASK_COMPLETED);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].task, Some(t(1)));
    assert_eq!(completed[0].data, json!({"kind": "children"}));
    let moved: Vec<(Option<TaskId>, Value)> = of_type(&after, types::TASK_STATUS_CHANGED)
        .into_iter()
        .map(|event| (event.task, event.data.clone()))
        .collect();
    assert!(moved.contains(&(Some(t(1)), json!({"from": "backlog", "to": "done"}))));
}

#[tokio::test]
async fn a_parent_with_an_open_plain_step_goes_to_todo_and_completes_when_the_step_is_ticked() {
    let fx = Fixture::new(2).await;
    fx.add(1, linked(2)).await;
    let own = fx.add(1, step("Wire the parts together")).await;
    fx.complete(&[2]).await;
    assert_eq!(fx.status(1).await, Todo);
    assert_eq!(fx.completion_kind(1).await, None);

    let before = fx.head().await;
    let view = fx
        .tasks
        .update_checklist_item(
            &agent(),
            own,
            ChecklistItemPatch {
                text: None,
                done: Some(true),
            },
        )
        .await
        .unwrap();
    assert_eq!(view.done_by.as_deref(), Some("backend"));
    assert_eq!(fx.status(1).await, Done);
    assert_eq!(fx.completion_kind(1).await.as_deref(), Some("children"));
    let after = fx.events_after(before).await;
    let kinds: Vec<EventType> = after.iter().map(|event| event.event_type).collect();
    assert_eq!(
        kinds,
        [
            types::CHECKLIST_ITEM_DONE,
            types::TASK_COMPLETED,
            types::TASK_STATUS_CHANGED
        ]
    );
    assert_eq!(after[0].actor, agent().event_actor());
}

#[tokio::test]
async fn a_grandparent_completes_in_the_same_call_as_its_last_grandchild() {
    // T-1 holds T-2 and T-3; T-2 holds T-4 and T-5.
    let fx = Fixture::new(5).await;
    fx.add(1, linked(2)).await;
    fx.add(1, linked(3)).await;
    fx.add(2, linked(4)).await;
    fx.add(2, linked(5)).await;
    fx.complete(&[3, 4]).await;
    assert_eq!([fx.status(1).await, fx.status(2).await], [Backlog, Backlog]);

    let before = fx.head().await;
    fx.complete(&[5]).await;
    assert_eq!([fx.status(1).await, fx.status(2).await], [Done, Done]);
    assert_eq!(fx.completion_kind(1).await.as_deref(), Some("children"));
    assert_eq!(fx.completion_kind(2).await.as_deref(), Some("children"));
    let completed: Vec<Option<TaskId>> =
        of_type(&fx.events_after(before).await, types::TASK_COMPLETED)
            .into_iter()
            .map(|event| event.task)
            .collect();
    assert_eq!(completed, [Some(t(2)), Some(t(1))]);
    assert_eq!(
        fx.tasks.checklist_progress(t(1)).await.unwrap(),
        ChecklistProgress { done: 2, total: 2 }
    );
}

#[tokio::test]
async fn a_task_that_ever_had_a_workspace_never_completes_by_children() {
    for state in ["active", "archived", "abandoned"] {
        let fx = Fixture::new(2).await;
        fx.add(1, linked(2)).await;
        fx.workspace(1, state).await;
        fx.complete(&[2]).await;
        let expected = if state == "active" { InProgress } else { Todo };
        assert_eq!(fx.status(1).await, expected, "{state}");
        assert_eq!(fx.completion_kind(1).await, None, "{state}");
    }
}

#[tokio::test]
async fn a_link_to_a_done_child_ticks_at_once() {
    let fx = Fixture::new(3).await;
    fx.complete(&[3]).await;
    fx.add(1, step("own work")).await;
    let item = fx.add(1, linked(3)).await;
    let view = fx.tasks.checklist_item(item).await.unwrap();
    assert_eq!(view.done_by.as_deref(), Some("fold"));
    assert_eq!(fx.status(1).await, Todo);

    // Without work of its own, the parent completes in the same transaction as the link.
    let item = fx.add(2, linked(3)).await;
    assert!(
        fx.tasks
            .checklist_item(item)
            .await
            .unwrap()
            .done_at
            .is_some()
    );
    assert_eq!(fx.status(2).await, Done);
    assert_eq!(fx.completion_kind(2).await.as_deref(), Some("children"));
}

#[tokio::test]
async fn removing_a_linked_item_removes_its_edge_and_recomputes_the_parent() {
    let fx = Fixture::new(2).await;
    let item = fx.add(1, linked(2)).await;
    fx.add(1, step("own work")).await;
    assert_eq!(fx.status(1).await, Backlog);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM dependencies WHERE origin = 'checklist'")
            .await,
        1
    );

    let before = fx.head().await;
    fx.tasks
        .remove_checklist_item(&agent(), item)
        .await
        .unwrap();
    assert!(fx.tasks.edges().await.unwrap().is_empty());
    assert_eq!(fx.status(1).await, Todo);
    let after = fx.events_after(before).await;
    let kinds: Vec<EventType> = after.iter().map(|event| event.event_type).collect();
    assert_eq!(
        kinds,
        [
            types::CHECKLIST_ITEM_REMOVED,
            types::DEPENDENCY_REMOVED,
            types::TASK_STATUS_CHANGED
        ]
    );
    assert_eq!(
        after[0].data,
        json!({"item": item.to_string(), "linked_task": "T-2"})
    );
    assert_eq!(
        after[1].data,
        json!({"needs": "T-2", "origin": "checklist"})
    );
    let (code, _) = refusal(
        fx.tasks
            .remove_checklist_item(&agent(), item)
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "checklist_item_not_found");
}

#[tokio::test]
async fn removing_the_last_open_step_completes_the_parent() {
    let fx = Fixture::new(2).await;
    fx.add(1, linked(2)).await;
    let own = fx.add(1, step("own work")).await;
    fx.complete(&[2]).await;
    assert_eq!(fx.status(1).await, Todo);
    fx.tasks.remove_checklist_item(&agent(), own).await.unwrap();
    assert_eq!(fx.status(1).await, Done);
}

#[tokio::test]
async fn a_cycle_through_a_linked_item_is_refused() {
    let fx = Fixture::new(3).await;
    fx.add(1, linked(2)).await;
    fx.add(2, linked(3)).await;
    let (code, details) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(3), linked(1))
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "dependency_cycle");
    assert_eq!(details["path"], json!(["T-3", "T-1", "T-2", "T-3"]));
    let (code, _) = refusal(
        fx.tasks
            .add_dependency(&agent(), t(3), t(1))
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "dependency_cycle");
    // The refused add left no checklist and no item on T-3.
    assert!(fx.tasks.checklists(t(3)).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_linked_item_gets_the_refusals_of_a_direct_edge() {
    let fx = Fixture::new(3).await;
    let (code, details) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(1), linked(1))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (code, &details["reason"]),
        ("invalid_request", &json!("self_dependency"))
    );
    fx.add(1, linked(2)).await;
    let (code, _) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(1), linked(2))
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "dependency_exists");
    let (code, _) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(1), linked(9))
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "task_not_found");
    fx.tasks.archive_task(&agent(), t(3)).await.unwrap();
    let (code, details) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(1), linked(3))
            .await
            .unwrap_err(),
    );
    assert_eq!((code, &details["archived"]), ("task_closed", &json!("T-3")));
}

#[tokio::test]
async fn a_checklist_edge_is_removed_only_with_its_item() {
    let fx = Fixture::new(2).await;
    fx.add(1, linked(2)).await;
    let (code, details) = refusal(
        fx.tasks
            .remove_dependency(&agent(), t(1), t(2))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (code, &details["reason"]),
        ("invalid_request", &json!("checklist_origin"))
    );
    assert_eq!(fx.tasks.edges().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_linked_item_cannot_be_edited_or_ticked_by_hand() {
    let fx = Fixture::new(2).await;
    let item = fx.add(1, linked(2)).await;
    for patch in [
        ChecklistItemPatch {
            text: None,
            done: Some(true),
        },
        ChecklistItemPatch {
            text: Some(StepText::parse("rename").unwrap()),
            done: None,
        },
    ] {
        let (code, details) = refusal(
            fx.tasks
                .update_checklist_item(&agent(), item, patch)
                .await
                .unwrap_err(),
        );
        assert_eq!(code, "invalid_request");
        assert_eq!(details["reason"], json!("linked_item"));
        assert_eq!(details["item"], json!(item.to_string()));
    }
    assert_eq!(fx.tasks.checklist_item(item).await.unwrap().done_at, None);
}

#[tokio::test]
async fn a_plain_step_is_edited_ticked_and_unticked() {
    let fx = Fixture::new(1).await;
    let item = fx.add(1, step("draft")).await;
    let before = fx.head().await;
    let view = fx
        .tasks
        .update_checklist_item(
            &agent(),
            item,
            ChecklistItemPatch {
                text: Some(StepText::parse("final").unwrap()),
                done: Some(true),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        view.content,
        ItemContent::Step(StepText::parse("final").unwrap())
    );
    assert_eq!(view.done_at, Some(fx.project.now()));
    let view = fx
        .tasks
        .update_checklist_item(
            &agent(),
            item,
            ChecklistItemPatch {
                text: Some(StepText::parse("final").unwrap()),
                done: Some(false),
            },
        )
        .await
        .unwrap();
    assert_eq!((view.done_at, view.done_by), (None, None));
    // An unchanged patch writes nothing.
    let head = fx.head().await;
    fx.tasks
        .update_checklist_item(&agent(), item, ChecklistItemPatch::default())
        .await
        .unwrap();
    assert_eq!(fx.head().await, head);
    let data: Vec<(EventType, Value)> = fx
        .events_after(before)
        .await
        .into_iter()
        .map(|event| (event.event_type, event.data))
        .collect();
    let id = item.to_string();
    assert_eq!(
        data,
        [
            (
                types::CHECKLIST_ITEM_UPDATED,
                json!({"item": id, "fields": ["text"]})
            ),
            (types::CHECKLIST_ITEM_DONE, json!({"item": id})),
            (
                types::CHECKLIST_ITEM_UPDATED,
                json!({"item": id, "fields": ["done"]})
            ),
        ]
    );
    // A plain step alone never completes its task.
    assert_eq!(fx.status(1).await, Todo);
    let (code, _) = refusal(
        fx.tasks
            .update_checklist_item(&agent(), ci(99), ChecklistItemPatch::default())
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "checklist_item_not_found");
}

#[tokio::test]
async fn the_first_checklist_is_subtasks_and_items_without_a_name_join_the_first_checklist() {
    let fx = Fixture::new(2).await;
    let before = fx.head().await;
    fx.add(1, in_checklist(step("check"), "QA")).await;
    fx.add(1, step("one")).await;
    fx.add(1, in_checklist(step("two"), "Later")).await;
    fx.add(2, step("first")).await;
    assert_eq!(
        fx.shape(1).await,
        [("QA".to_owned(), 2), ("Later".to_owned(), 1)]
    );
    assert_eq!(fx.shape(2).await, [("Subtasks".to_owned(), 1)]);
    let created: Vec<(Option<TaskId>, Value)> =
        of_type(&fx.events_after(before).await, types::CHECKLIST_CREATED)
            .into_iter()
            .map(|event| (event.task, event.data.clone()))
            .collect();
    assert_eq!(
        created,
        [
            (Some(t(1)), json!({"checklist": "QA"})),
            (Some(t(1)), json!({"checklist": "Later"})),
            (Some(t(2)), json!({"checklist": "Subtasks"})),
        ]
    );
    let added = of_type(&fx.events_after(before).await, types::CHECKLIST_ITEM_ADDED)
        .first()
        .map(|event| event.data.clone());
    assert_eq!(added, Some(json!({"item": "ci-1", "checklist": "QA"})));
}

#[tokio::test]
async fn a_plain_step_without_a_name_follows_one_rule_by_the_number_of_checklists() {
    let fx = Fixture::new(2).await;
    fx.add(1, step("first")).await;
    assert_eq!(fx.shape(1).await, [("Subtasks".to_owned(), 1)]);

    fx.add(2, in_checklist(step("check"), "QA")).await;
    fx.add(2, step("joins")).await;
    assert_eq!(fx.shape(2).await, [("QA".to_owned(), 2)]);

    fx.add(2, in_checklist(step("later"), "Later")).await;
    let (code, details) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(2), step("secret text"))
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "checklist_ambiguous");
    assert_eq!(details["checklists"], json!(["QA", "Later"]));
    assert!(!details.to_string().contains("secret"));
    assert_eq!(
        fx.shape(2).await,
        [("QA".to_owned(), 2), ("Later".to_owned(), 1)]
    );
}

#[tokio::test]
async fn done_and_archived_tasks_take_no_new_items() {
    let fx = Fixture::new(3).await;
    let item = fx.add(3, step("kept")).await;
    fx.complete(&[1]).await;
    for new in [step("late"), linked(2)] {
        let (code, details) = refusal(
            fx.tasks
                .add_checklist_item(&agent(), t(1), new)
                .await
                .unwrap_err(),
        );
        assert_eq!((code, &details["reason"]), ("task_closed", &json!("done")));
    }
    fx.tasks.archive_task(&agent(), t(3)).await.unwrap();
    let (code, details) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(3), step("late"))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (code, &details["reason"]),
        ("task_closed", &json!("archived"))
    );
    let (code, _) = refusal(
        fx.tasks
            .update_checklist_item(
                &agent(),
                item,
                ChecklistItemPatch {
                    text: None,
                    done: Some(true),
                },
            )
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "task_closed");
    let (code, _) = refusal(
        fx.tasks
            .remove_checklist_item(&agent(), item)
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "task_closed");
    let (code, _) = refusal(
        fx.tasks
            .add_checklist_item(&agent(), t(9), step("x"))
            .await
            .unwrap_err(),
    );
    assert_eq!(code, "task_not_found");
    for error in [
        fx.tasks.checklists(t(9)).await.unwrap_err(),
        fx.tasks.checklist_progress(t(9)).await.unwrap_err(),
    ] {
        assert_eq!(refusal(error).0, "task_not_found");
    }
}

/// The checklists of a Done task record what its work covered, so every change to them is refused, not only a new item: more work belongs in a follow-up task.
#[tokio::test]
async fn a_done_task_locks_its_whole_checklist() {
    let fx = Fixture::new(2).await;
    let ticked = fx.add(1, step("ticked")).await;
    let open = fx.add(1, step("open")).await;
    fx.tasks
        .update_checklist_item(
            &agent(),
            ticked,
            ChecklistItemPatch {
                text: None,
                done: Some(true),
            },
        )
        .await
        .unwrap();
    fx.complete(&[1]).await;
    assert_eq!(fx.status(1).await, Done);
    let before = fx.head().await;
    let shape = fx.shape(1).await;

    let patch = |text: Option<&str>, done: Option<bool>| ChecklistItemPatch {
        text: text.map(|text| StepText::parse(text).unwrap()),
        done,
    };
    let refusals = [
        fx.tasks
            .update_checklist_item(&agent(), ticked, patch(None, Some(false)))
            .await
            .unwrap_err(),
        fx.tasks
            .update_checklist_item(&agent(), open, patch(Some("edited"), None))
            .await
            .unwrap_err(),
        fx.tasks
            .remove_checklist_item(&agent(), open)
            .await
            .unwrap_err(),
        fx.tasks
            .move_checklist_item(&agent(), open, Some(0), None)
            .await
            .unwrap_err(),
        fx.tasks
            .move_checklist_item(
                &agent(),
                open,
                Some(0),
                Some(ChecklistName::parse("QA").unwrap()),
            )
            .await
            .unwrap_err(),
        fx.tasks
            .update_checklist_item(&stranger(), open, patch(None, Some(true)))
            .await
            .unwrap_err(),
        fx.tasks
            .update_checklist_item(&human(), open, patch(None, Some(true)))
            .await
            .unwrap_err(),
    ];
    for error in refusals {
        let TaskError::Fold(error) = error else {
            panic!("expected a refusal, got {error:?}");
        };
        assert_eq!(error.code(), "task_closed");
        assert_eq!(error.details().unwrap()["reason"], json!("done"));
        assert!(error.hint().unwrap().contains("create_task"), "{error:?}");
    }
    assert_eq!(fx.head().await, before);
    assert_eq!(fx.shape(1).await, shape);
    assert!(
        fx.tasks
            .checklist_item(ticked)
            .await
            .unwrap()
            .done_at
            .is_some()
    );
    assert_eq!(fx.tasks.checklist_item(open).await.unwrap().done_at, None);
}

#[tokio::test]
async fn an_archived_parent_keeps_its_linked_items_and_never_completes() {
    let fx = Fixture::new(2).await;
    let item = fx.add(1, linked(2)).await;
    fx.tasks.archive_task(&agent(), t(1)).await.unwrap();
    assert!(fx.tasks.edges().await.unwrap().is_empty());
    fx.complete(&[2]).await;
    assert_eq!(fx.completion_kind(1).await, None);
    let view = fx.tasks.checklist_item(item).await.unwrap();
    assert_eq!(view.content, ItemContent::Linked(t(2)));
    assert_eq!(view.done_at, None);
}

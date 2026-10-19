use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, EventActor, EventFilter, LabelColor, LabelName, TaskDescription, TaskId,
    TaskStatus, TaskTitle, WorkerName,
};
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_daemon::services::tasks::{CreateTask, TaskError, TaskPatch, TaskService};
use fold_store::testing::TestProject;
use fold_store::{events, tasks};

#[cfg(test)]
fn actor() -> Actor {
    let worker = WorkerName::parse("frontend").unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn label(text: &str) -> LabelName {
    LabelName::parse(text).unwrap()
}

#[cfg(test)]
fn input(title: &str, labels: &[&str]) -> CreateTask {
    CreateTask {
        title: TaskTitle::parse(title).unwrap(),
        description: TaskDescription::new("## body"),
        labels: labels.iter().map(|l| label(l)).collect(),
    }
}

#[cfg(test)]
fn service(project: &TestProject) -> (TaskService, EventBus) {
    let bus = EventBus::new();
    let service = TaskService::new(
        project.store().clone(),
        bus.clone(),
        Arc::new(project.clock().clone()),
        fold_core::ProjectConfig::default().lease_minutes,
    );
    (service, bus)
}

#[cfg(test)]
async fn counts(project: &TestProject) -> (usize, u64) {
    let tasks = tasks::list(project.store().read(), true)
        .await
        .unwrap()
        .len();
    let head = events::head_seq(project.store().read()).await.unwrap();
    (tasks, head)
}

#[cfg(test)]
fn code(error: TaskError) -> &'static str {
    error.code().unwrap()
}

#[cfg(test)]
async fn stored(project: &TestProject) -> Vec<fold_core::Event> {
    events::read_after(project.store().read(), 0, &EventFilter::default(), 100)
        .await
        .unwrap()
}

#[tokio::test]
async fn create_with_labels_writes_the_row_its_event_and_its_status() {
    let project = TestProject::new().await;
    let (service, bus) = service(&project);
    service
        .create_label(&actor(), label("bug"), LabelColor::Plum)
        .await
        .unwrap();
    service
        .create_label(&actor(), label("api"), LabelColor::Teal)
        .await
        .unwrap();
    let mut receiver = bus.subscribe();

    let task = service
        .create_task(&actor(), input("  Build it ", &["bug", "api"]))
        .await
        .unwrap();
    assert_eq!(task.id, TaskId::from_number(1).unwrap());
    assert_eq!(task.title.as_str(), "Build it");
    assert_eq!(task.description.as_str(), "## body");
    assert_eq!(task.created_by, "frontend");
    // Recompute ran in the create: a task with no prerequisites is ready.
    assert_eq!(task.status, TaskStatus::Todo);
    assert!(!task.blocked);
    assert_eq!(task.human_review, None);
    assert_eq!(task.created_at, project.now());
    let names: Vec<_> = task.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["api", "bug"]);

    let created: Vec<_> = stored(&project)
        .await
        .into_iter()
        .filter(|e| e.event_type == types::TASK_CREATED)
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].task, Some(task.id));
    assert_eq!(created[0].actor, actor().event_actor());
    assert_eq!(
        created[0].data,
        serde_json::json!({"labels": ["api", "bug"]})
    );
    assert_eq!(created[0].created_at, project.now());

    let changed: Vec<_> = stored(&project)
        .await
        .into_iter()
        .filter(|e| e.event_type == types::TASK_STATUS_CHANGED)
        .collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].task, Some(task.id));
    assert_eq!(changed[0].actor, EventActor::fold());
    assert_eq!(
        changed[0].data,
        serde_json::json!({"from": "backlog", "to": "todo"})
    );
    assert_eq!(changed[0].seq, created[0].seq + 1);

    for expected in [created[0].seq, changed[0].seq] {
        let BusMessage::Durable(published) = receiver.try_recv().unwrap() else {
            unreachable!("a stored event")
        };
        assert_eq!(published.seq, expected);
    }
    assert!(receiver.try_recv().is_err());

    let second = service
        .create_task(&actor(), input("Second", &[]))
        .await
        .unwrap();
    assert_eq!(second.id.number(), 2);
    assert_eq!(service.get(task.id).await.unwrap(), task);
    let listed = service.list(false).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1], second);
}

#[tokio::test]
async fn a_failed_create_leaves_no_row_no_event_and_no_number() {
    let project = TestProject::new().await;
    let (service, bus) = service(&project);
    let mut receiver = bus.subscribe();
    let before = counts(&project).await;

    let error = service
        .create_task(&actor(), input("Broken", &["missing"]))
        .await
        .unwrap_err();
    assert_eq!(code(error), "label_not_found");
    assert_eq!(counts(&project).await, before);
    assert!(receiver.try_recv().is_err());

    let task = service
        .create_task(&actor(), input("Fine", &[]))
        .await
        .unwrap();
    assert_eq!(task.id.number(), 1);
}

#[tokio::test]
async fn update_changes_fields_and_names_them_in_one_event() {
    let project = TestProject::new().await;
    let (service, _bus) = service(&project);
    for (name, color) in [("a", LabelColor::Sage), ("b", LabelColor::Slate)] {
        service
            .create_label(&actor(), label(name), color)
            .await
            .unwrap();
    }
    let task = service
        .create_task(&actor(), input("Old", &["a"]))
        .await
        .unwrap();
    project.clock().advance(Duration::from_secs(60)).unwrap();

    let updated = service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                title: Some(TaskTitle::parse("New").unwrap()),
                description: None,
                labels: Some(vec![label("b")]),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.title.as_str(), "New");
    assert_eq!(updated.description, task.description);
    assert_eq!(updated.labels.len(), 1);
    assert_eq!(updated.labels[0].name.as_str(), "b");
    assert_eq!(updated.updated_at, project.now());
    assert_eq!(updated.created_at, task.created_at);

    let events = stored(&project).await;
    let last = events.last().unwrap();
    assert_eq!(last.event_type, types::TASK_UPDATED);
    assert_eq!(last.task, Some(task.id));
    assert_eq!(
        last.data,
        serde_json::json!({"fields": ["title", "labels"], "labels_added": ["b"]})
    );

    let described = service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                description: Some(TaskDescription::new("text")),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(described.description.as_str(), "text");
    assert_eq!(described.title.as_str(), "New");
    let events = stored(&project).await;
    assert_eq!(
        events.last().unwrap().data,
        serde_json::json!({"fields": ["description"], "labels_added": []})
    );

    project.clock().advance(Duration::from_secs(60)).unwrap();
    let relabelled = service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                labels: Some(vec![]),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(relabelled.labels.is_empty());
    assert_eq!(relabelled.updated_at, project.now());
    let events = stored(&project).await;
    assert_eq!(
        events.last().unwrap().data,
        serde_json::json!({"fields": ["labels"], "labels_added": []})
    );

    service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                title: Some(TaskTitle::parse("Newer").unwrap()),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    let events = stored(&project).await;
    assert_eq!(
        events.last().unwrap().data,
        serde_json::json!({"fields": ["title"], "labels_added": []})
    );
}

#[tokio::test]
async fn an_update_that_changes_nothing_writes_nothing() {
    let project = TestProject::new().await;
    let (service, bus) = service(&project);
    service
        .create_label(&actor(), label("a"), LabelColor::Sage)
        .await
        .unwrap();
    service
        .create_label(&actor(), label("b"), LabelColor::Slate)
        .await
        .unwrap();
    let task = service
        .create_task(&actor(), input("Same", &["a", "b"]))
        .await
        .unwrap();
    let before = counts(&project).await;
    let mut receiver = bus.subscribe();
    project.clock().advance(Duration::from_secs(60)).unwrap();

    let same = service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                title: Some(TaskTitle::parse("Same").unwrap()),
                description: Some(task.description.clone()),
                labels: Some(vec![label("b"), label("a")]),
            },
        )
        .await
        .unwrap();
    assert_eq!(same, task);
    let empty = service
        .update_task(&actor(), task.id, TaskPatch::default())
        .await
        .unwrap();
    assert_eq!(empty, task);
    assert_eq!(counts(&project).await, before);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn refusals_carry_their_codes_and_change_nothing() {
    let project = TestProject::new().await;
    let (service, bus) = service(&project);
    service
        .create_label(&actor(), label("a"), LabelColor::Sage)
        .await
        .unwrap();
    let task = service
        .create_task(&actor(), input("Task", &[]))
        .await
        .unwrap();
    let before = counts(&project).await;
    let mut receiver = bus.subscribe();

    let duplicate = service
        .create_label(&actor(), label("a"), LabelColor::Gold)
        .await
        .unwrap_err();
    assert_eq!(code(duplicate), "label_exists");
    assert_eq!(service.labels().await.unwrap()[0].color, LabelColor::Sage);

    let unknown_label = service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                title: Some(TaskTitle::parse("Changed").unwrap()),
                labels: Some(vec![label("a"), label("nope")]),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(code(unknown_label), "label_not_found");
    assert_eq!(service.get(task.id).await.unwrap(), task);

    let missing = TaskId::from_number(99).unwrap();
    let no_task = service
        .update_task(&actor(), missing, TaskPatch::default())
        .await
        .unwrap_err();
    assert_eq!(code(no_task), "task_not_found");
    assert_eq!(
        code(service.get(missing).await.unwrap_err()),
        "task_not_found"
    );

    project
        .store()
        .write(async |tx| {
            sqlx::query("UPDATE tasks SET archived_at = 1 WHERE number = 1")
                .execute(tx.conn())
                .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let closed = service
        .update_task(
            &actor(),
            task.id,
            TaskPatch {
                title: Some(TaskTitle::parse("Late").unwrap()),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap_err();
    let TaskError::Fold(refusal) = closed else {
        panic!("an archived task is a refusal");
    };
    assert_eq!(refusal.code(), "task_closed");
    assert_eq!(refusal.details().unwrap()["reason"], "archived");
    assert_eq!(counts(&project).await, before);
    assert!(receiver.try_recv().is_err());
    assert!(service.list(false).await.unwrap().is_empty());
    assert_eq!(service.list(true).await.unwrap().len(), 1);
}

#[tokio::test]
async fn create_label_records_one_event_and_the_colour() {
    let project = TestProject::new().await;
    let (service, _bus) = service(&project);
    let view = service
        .create_label(&actor(), label("ui"), LabelColor::Moss)
        .await
        .unwrap();
    assert_eq!(view.color, LabelColor::Moss);
    assert_eq!(view.created_at, project.now());
    let events = stored(&project).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, types::LABEL_CREATED);
    assert_eq!(events[0].task, None);
    assert_eq!(
        events[0].data,
        serde_json::json!({"name": "ui", "color": "moss"})
    );
}

#[tokio::test]
async fn the_error_type_reports_codes_only_for_refusals() {
    let internal = TaskError::from(fold_store::StoreError::Invalid("x"));
    assert_eq!(internal.code(), None);
    let schema = TaskError::from(fold_store::StoreError::SchemaTooNew { version: 9 });
    assert_eq!(schema.code(), Some("schema_too_new"));
    let event = TaskError::from(fold_core::EventError::DataNotObject);
    assert_eq!(event.code(), None);
}

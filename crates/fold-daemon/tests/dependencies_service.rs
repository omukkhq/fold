use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    DependencyOrigin, EventActor, EventFilter, TaskDescription, TaskId, TaskStatus, TaskTitle,
};
use fold_daemon::services::events::{BusMessage, EventBuffer, EventBus};
use fold_daemon::services::tasks::{self, CreateTask, Edge, TaskError, TaskService};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::{StoreError, events};
use serde_json::{Value, json};

#[cfg(test)]
fn frontend() -> EventActor {
    EventActor::parse("frontend").unwrap()
}

#[cfg(test)]
fn backend() -> EventActor {
    EventActor::parse("backend").unwrap()
}

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

/// A project with tasks `T-1` to `T-count`, its task service and its bus.
#[cfg(test)]
async fn project_with_tasks(count: u32) -> (TestProject, TaskService, EventBus) {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    let service = TaskService::new(
        project.store().clone(),
        bus.clone(),
        Arc::new(project.clock().clone()),
    );
    for _ in 0..count {
        service
            .create_task(
                &frontend(),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new("body"),
                    labels: Vec::new(),
                },
            )
            .await
            .unwrap();
    }
    (project, service, bus)
}

#[cfg(test)]
async fn head(project: &TestProject) -> u64 {
    events::head_seq(project.store().read()).await.unwrap()
}

#[cfg(test)]
async fn pairs(service: &TaskService) -> Vec<(TaskId, TaskId)> {
    service
        .edges()
        .await
        .unwrap()
        .into_iter()
        .map(|edge| (edge.task, edge.needs))
        .collect()
}

#[cfg(test)]
async fn last_event(project: &TestProject) -> fold_core::Event {
    events::read_after(project.store().read(), 0, &EventFilter::default(), 1000)
        .await
        .unwrap()
        .pop()
        .unwrap()
}

#[cfg(test)]
async fn events_after(project: &TestProject, after: u64) -> Vec<fold_core::Event> {
    events::read_after(project.store().read(), after, &EventFilter::default(), 1000)
        .await
        .unwrap()
}

#[cfg(test)]
async fn exec(project: &TestProject, sql: &'static str) {
    project
        .store()
        .write(async |tx| {
            sqlx::query(sql).execute(tx.conn()).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

/// The code and the details of a refusal.
#[cfg(test)]
fn refusal(error: TaskError) -> (&'static str, Value) {
    match error {
        TaskError::Fold(error) => {
            assert!(error.hint().is_some(), "{} has no hint", error.code());
            (
                error.code(),
                error.details().cloned().unwrap_or(Value::Null),
            )
        }
        other => panic!("internal failure: {other}"),
    }
}

/// Runs `add_dependency` and requires a refusal that wrote nothing: no edge and no event.
#[cfg(test)]
async fn refused_add(
    project: &TestProject,
    service: &TaskService,
    task: u32,
    needs: u32,
) -> (&'static str, Value) {
    let before = (head(project).await, pairs(service).await);
    let error = service
        .add_dependency(&backend(), t(task), t(needs))
        .await
        .unwrap_err();
    assert_eq!((head(project).await, pairs(service).await), before);
    refusal(error)
}

#[tokio::test]
async fn add_inserts_a_direct_edge_and_recomputes_the_task() {
    let (project, service, bus) = project_with_tasks(3).await;
    let before = head(&project).await;
    let mut receiver = bus.subscribe();

    let edge = service
        .add_dependency(&backend(), t(1), t(2))
        .await
        .unwrap();

    assert_eq!(
        edge,
        Edge {
            task: t(1),
            needs: t(2),
            origin: DependencyOrigin::Direct,
            created_by: "backend".to_owned(),
            created_at: project.now(),
        }
    );
    assert_eq!(service.edges().await.unwrap(), vec![edge]);
    // The edge and the recompute it triggers: T-1 now waits on T-2.
    assert_eq!(head(&project).await, before + 2);
    let appended = events_after(&project, before).await;
    let added = &appended[0];
    assert_eq!(added.event_type, types::DEPENDENCY_ADDED);
    assert_eq!(added.actor, backend());
    assert_eq!(added.task, Some(t(1)));
    assert_eq!(added.data, json!({"needs": "T-2", "origin": "direct"}));
    let changed = &appended[1];
    assert_eq!(changed.event_type, types::TASK_STATUS_CHANGED);
    assert_eq!(changed.actor, EventActor::fold());
    assert_eq!(changed.task, Some(t(1)));
    assert_eq!(changed.data, json!({"from": "todo", "to": "backlog"}));
    for stored in &appended {
        match receiver.try_recv().unwrap() {
            BusMessage::Durable(event) => assert_eq!(*event, *stored),
            BusMessage::Live(_) => panic!("expected the stored event"),
        }
    }
    assert!(receiver.try_recv().is_err());

    let row = fold_store::tasks::get_by_number(project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.status, row.blocked), (TaskStatus::Backlog, false));

    // Both agents see every edge, whoever made it.
    service
        .add_dependency(&frontend(), t(3), t(2))
        .await
        .unwrap();
    assert_eq!(pairs(&service).await, vec![(t(1), t(2)), (t(3), t(2))]);
}

#[tokio::test]
async fn remove_deletes_the_edge_and_recomputes_the_task() {
    let (project, service, bus) = project_with_tasks(2).await;
    service
        .add_dependency(&backend(), t(1), t(2))
        .await
        .unwrap();
    let before = head(&project).await;
    let mut receiver = bus.subscribe();

    service
        .remove_dependency(&frontend(), t(1), t(2))
        .await
        .unwrap();

    assert!(pairs(&service).await.is_empty());
    // The removal and the recompute it triggers: T-1 waits on nothing again.
    assert_eq!(head(&project).await, before + 2);
    let appended = events_after(&project, before).await;
    let removed = &appended[0];
    assert_eq!(removed.event_type, types::DEPENDENCY_REMOVED);
    assert_eq!(removed.actor, frontend());
    assert_eq!(removed.task, Some(t(1)));
    assert_eq!(removed.data, json!({"needs": "T-2", "origin": "direct"}));
    let changed = &appended[1];
    assert_eq!(changed.event_type, types::TASK_STATUS_CHANGED);
    assert_eq!(changed.task, Some(t(1)));
    assert_eq!(changed.data, json!({"from": "backlog", "to": "todo"}));
    for stored in &appended {
        assert!(matches!(
            receiver.try_recv().unwrap(),
            BusMessage::Durable(event) if *event == *stored
        ));
    }
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn a_direct_cycle_is_refused_with_its_path() {
    let (project, service, _bus) = project_with_tasks(2).await;
    service
        .add_dependency(&backend(), t(1), t(2))
        .await
        .unwrap();
    let (code, details) = refused_add(&project, &service, 2, 1).await;
    assert_eq!(code, "dependency_cycle");
    assert_eq!(details["path"], json!(["T-2", "T-1", "T-2"]));
}

#[tokio::test]
async fn a_three_task_cycle_is_refused_with_the_full_path() {
    let (project, service, _bus) = project_with_tasks(3).await;
    service
        .add_dependency(&backend(), t(1), t(2))
        .await
        .unwrap();
    service
        .add_dependency(&backend(), t(2), t(3))
        .await
        .unwrap();
    let (code, details) = refused_add(&project, &service, 3, 1).await;
    assert_eq!(code, "dependency_cycle");
    assert_eq!(details["path"], json!(["T-3", "T-1", "T-2", "T-3"]));
    assert_eq!(pairs(&service).await, vec![(t(1), t(2)), (t(2), t(3))]);
}

#[tokio::test]
async fn a_self_edge_is_an_invalid_request() {
    let (project, service, _bus) = project_with_tasks(1).await;
    let (code, details) = refused_add(&project, &service, 1, 1).await;
    assert_eq!(code, "invalid_request");
    assert_eq!(details["reason"], "self_dependency");
    assert_eq!(details["task"], "T-1");
}

#[tokio::test]
async fn an_unknown_task_is_not_found() {
    let (project, service, _bus) = project_with_tasks(1).await;
    assert_eq!(
        refused_add(&project, &service, 1, 9).await.0,
        "task_not_found"
    );
    assert_eq!(
        refused_add(&project, &service, 9, 1).await.0,
        "task_not_found"
    );
}

#[tokio::test]
async fn an_archived_task_on_either_end_is_closed() {
    let (project, service, _bus) = project_with_tasks(3).await;
    exec(
        &project,
        "UPDATE tasks SET archived_at = 1 WHERE number = 2",
    )
    .await;
    for (task, needs) in [(1, 2), (2, 3)] {
        let (code, details) = refused_add(&project, &service, task, needs).await;
        assert_eq!(code, "task_closed");
        assert_eq!(details["reason"], "archived");
        assert_eq!(details["archived"], "T-2");
    }
    service
        .add_dependency(&backend(), t(1), t(3))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_done_task_takes_no_new_dependencies_but_may_be_needed() {
    let (project, service, _bus) = project_with_tasks(2).await;
    let now = project.now();
    project
        .store()
        .write(async |tx| {
            completions::insert(tx, t(1), CompletionKind::Children, None, now).await?;
            tasks::recompute(tx, &[t(1)], now).await?;
            Ok::<_, TaskError>(())
        })
        .await
        .unwrap();
    let (code, details) = refused_add(&project, &service, 1, 2).await;
    assert_eq!(code, "task_closed");
    assert_eq!(details["reason"], "done");
    assert_eq!(details["task"], "T-1");

    service
        .add_dependency(&backend(), t(2), t(1))
        .await
        .unwrap();
    assert_eq!(pairs(&service).await, vec![(t(2), t(1))]);
    // A Done prerequisite holds nothing back.
    let row = fold_store::tasks::get_by_number(project.store().read(), t(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, TaskStatus::Todo);
}

#[tokio::test]
async fn a_second_edge_between_the_same_tasks_exists() {
    let (project, service, _bus) = project_with_tasks(2).await;
    service
        .add_dependency(&backend(), t(1), t(2))
        .await
        .unwrap();
    let (code, details) = refused_add(&project, &service, 1, 2).await;
    assert_eq!(code, "dependency_exists");
    assert_eq!(details, json!({"task": "T-1", "needs": "T-2"}));
}

#[tokio::test]
async fn removing_a_missing_edge_is_not_found() {
    let (project, service, _bus) = project_with_tasks(2).await;
    service
        .add_dependency(&backend(), t(1), t(2))
        .await
        .unwrap();
    let before = head(&project).await;
    for (task, needs) in [(2, 1), (1, 9)] {
        let error = service
            .remove_dependency(&backend(), t(task), t(needs))
            .await
            .unwrap_err();
        assert_eq!(refusal(error).0, "dependency_not_found");
    }
    assert_eq!(head(&project).await, before);
    assert_eq!(pairs(&service).await, vec![(t(1), t(2))]);
}

#[tokio::test]
async fn a_checklist_edge_is_added_in_the_callers_transaction_and_not_removed_directly() {
    let (project, service, bus) = project_with_tasks(2).await;
    let now = project.now();
    let mut buffer = EventBuffer::new();
    project
        .store()
        .write(async |tx| {
            tasks::add_edge_in_tx(
                tx,
                &mut buffer,
                t(1),
                t(2),
                DependencyOrigin::Checklist,
                &frontend(),
                now,
            )
            .await
        })
        .await
        .unwrap();
    assert_eq!(buffer.len(), 1);
    bus.publish(buffer);
    let stored = last_event(&project).await;
    assert_eq!(stored.event_type, types::DEPENDENCY_ADDED);
    assert_eq!(stored.data, json!({"needs": "T-2", "origin": "checklist"}));

    // One row per pair: a direct edge cannot sit beside the checklist edge.
    assert_eq!(
        refused_add(&project, &service, 1, 2).await.0,
        "dependency_exists"
    );

    let before = head(&project).await;
    let error = service
        .remove_dependency(&backend(), t(1), t(2))
        .await
        .unwrap_err();
    let TaskError::Fold(fold) = &error else {
        panic!("expected a refusal");
    };
    assert!(fold.hint().unwrap().contains("checklist item"));
    let (code, details) = refusal(error);
    assert_eq!(code, "invalid_request");
    assert_eq!(details["reason"], "checklist_origin");
    assert_eq!(head(&project).await, before);
    assert_eq!(
        service.edges().await.unwrap()[0].origin,
        DependencyOrigin::Checklist
    );
}

#[tokio::test]
async fn a_rolled_back_transaction_keeps_neither_the_edge_nor_its_event() {
    let (project, service, _bus) = project_with_tasks(2).await;
    let before = head(&project).await;
    let now = project.now();
    let mut buffer = EventBuffer::new();
    let outcome = project
        .store()
        .write(async |tx| {
            tasks::add_edge_in_tx(
                tx,
                &mut buffer,
                t(1),
                t(2),
                DependencyOrigin::Direct,
                &frontend(),
                now,
            )
            .await?;
            Err::<(), TaskError>(TaskError::Store(StoreError::Invalid("stop")))
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(head(&project).await, before);
    assert!(pairs(&service).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_cycle_closing_adds_at_once_leave_one_edge_and_one_refusal() {
    for _ in 0..10 {
        let (_project, service, _bus) = project_with_tasks(2).await;
        let first = tokio::spawn({
            let service = service.clone();
            async move { service.add_dependency(&frontend(), t(1), t(2)).await }
        });
        let second = tokio::spawn({
            let service = service.clone();
            async move { service.add_dependency(&backend(), t(2), t(1)).await }
        });
        let outcomes = [first.await.unwrap(), second.await.unwrap()];

        let accepted: Vec<&Edge> = outcomes.iter().filter_map(|o| o.as_ref().ok()).collect();
        assert_eq!(accepted.len(), 1);
        let refused: Vec<&TaskError> = outcomes.iter().filter_map(|o| o.as_ref().err()).collect();
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].code(), Some("dependency_cycle"));
        let edges = pairs(&service).await;
        assert_eq!(edges, vec![(accepted[0].task, accepted[0].needs)]);
    }
}

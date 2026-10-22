use std::sync::Arc;

use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, EventFilter, TaskDescription, TaskId, TaskStatus, TaskTitle,
    WorkerName,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceService};
use fold_store::agents::WorkerRow;
use fold_store::events;
use fold_store::tasks::claims;
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};

const FAILPOINT: &str = "workspace_create_after_rename";

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

/// The acting agent of a worker row, as `AgentService::actor` builds it.
#[cfg(test)]
fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

#[cfg(test)]
struct Services {
    tasks: TaskService,
    workspaces: WorkspaceService,
}

#[cfg(test)]
fn services(project: &TestProject) -> Services {
    let bus = EventBus::new();
    let clock = Arc::new(project.clock().clone());
    Services {
        tasks: TaskService::new(project.store().clone(), bus.clone(), clock.clone(), 5),
        workspaces: WorkspaceService::new(
            project.store().clone(),
            bus,
            clock,
            project.paths().clone(),
            5,
        ),
    }
}

/// `r1` with one file, task `T-1` and the agent `frontend`.
#[cfg(test)]
async fn seed(project: &TestProject) -> Actor {
    let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
    let hash = objects.write(ObjectKind::Blob, b"one\n").unwrap();
    let mut tree = FlatTree::new();
    tree.insert(
        b"a.txt".to_vec(),
        FlatEntry {
            kind: EntryKind::File,
            hash,
        },
    );
    let tree = write_initial_tree(&objects, tree).unwrap();
    let stored = write_revision(&objects, &initial_revision(tree, project.now()).unwrap()).unwrap();
    create_initial_revision(project.store(), &stored, None)
        .await
        .unwrap();
    crate::common::roles::lead_work_areas(project.store()).await;
    services(project)
        .tasks
        .create_task(
            &Actor::Agent(AgentActor {
                agent: AgentName::parse("frontend").unwrap(),
                worker: WorkerName::parse("frontend").unwrap(),
                tags: Vec::new(),
            }),
            CreateTask {
                title: TaskTitle::parse("task").unwrap(),
                description: TaskDescription::new("body"),
                labels: crate::common::roles::work_labels(),
            },
        )
        .await
        .unwrap();
    let agents = AgentService::new(
        project.store().clone(),
        EventBus::new(),
        Arc::new(project.clock().clone()),
    );
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse("frontend").unwrap(), &[])
        .await
        .unwrap();
    by(&agents.resolve_worker(&ctx).await.unwrap())
}

#[cfg(test)]
async fn event_count(project: &TestProject) -> usize {
    events::read_after(project.store().read(), 0, &EventFilter::default(), 1000)
        .await
        .unwrap()
        .len()
}

/// The failpoint stops a create between the rename and the transaction, as a crash there would. A restarted daemon shows the task as it was, with no row, no claim and no event; the directory left behind keeps its name, so the next create takes the one after it.
#[tokio::test]
async fn a_create_stopped_after_the_rename_leaves_no_row() {
    let scenario = fail::FailScenario::setup();
    let mut project = TestProject::new().await;
    let worker = seed(&project).await;
    let events_before = event_count(&project).await;

    fail::cfg(FAILPOINT, "return").unwrap();
    let error = services(&project)
        .workspaces
        .create_workspace(&worker, t(1))
        .await
        .unwrap_err();
    fail::remove(FAILPOINT);
    match &error {
        WorkspaceError::Fold(fold) => assert_eq!(fold.code(), "failpoint"),
        other => panic!("expected the failpoint, got {other:?}"),
    }
    let orphan = project.paths().root().join(".fold/ws/t-1");
    assert!(orphan.join("a.txt").is_file());

    project.reopen().await;
    let restarted = services(&project);
    let task = restarted.tasks.get(t(1)).await.unwrap();
    assert_eq!((task.status, task.blocked), (TaskStatus::Todo, false));
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces")
        .fetch_one(project.store().read())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert!(
        claims::unreleased_for_task(project.store().read(), t(1))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(event_count(&project).await, events_before);

    let view = restarted
        .workspaces
        .create_workspace(&worker, t(1))
        .await
        .unwrap()
        .workspace;
    assert_eq!(view.path, ".fold/ws/t-1-2");
    assert!(orphan.join("a.txt").is_file());
    let task = restarted.tasks.get(t(1)).await.unwrap();
    assert_eq!(task.status, TaskStatus::InProgress);
    scenario.teardown();
}

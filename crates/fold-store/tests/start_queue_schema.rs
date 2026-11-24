#![cfg(test)]

use std::collections::BTreeMap;

use fold_core::{AreaName, ProposalId, ReviewPlanId, Role, TaskId, Timestamp};
use fold_store::agents::{self, DropReason, NewQueuedStart, NewWorkerProcess, QueueEnd};
use fold_store::{Store, StoreError};

use crate::common::{count, exec, open};

const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-lead', 0), (2, 'review', 0), (3, 'web-worker', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'web-lead', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (11, 'web', 'sage', 0), (12, 'core', 'sage', 0);
INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (11, 'web', 11, 'main', 0), (12, 'core', 12, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'main', 0, 0), (2, 2, 'two', 'main', 0, 0), (3, 3, 'three', 'main', 0, 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0), (2, 2, 2, 1, 'q', 0, 0);
INSERT INTO review_plans (id, number, lead_agent_id, created_at, deadline_at) VALUES (1, 1, 2, 10, 100);
";

async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

fn task(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

fn proposal(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

fn task_start<'a>(area_id: i64, tasks: &'a [TaskId], now: i64) -> NewQueuedStart<'a> {
    NewQueuedStart {
        role: Role::TaskWorker,
        area_id: Some(area_id),
        name: None,
        tasks,
        proposals: &[],
        review_plan: None,
        model: None,
        instructions: None,
        queued_by: "web-lead",
        now: at(now),
    }
}

async fn queue(store: &Store, new: NewQueuedStart<'_>) -> Result<i64, StoreError> {
    store
        .write(async |tx| agents::insert_queued_start(tx, &new).await)
        .await
}

async fn worker_process(store: &Store) -> i64 {
    store
        .write(async |tx| {
            agents::insert_worker_process(
                tx,
                &NewWorkerProcess {
                    agent_id: 3,
                    role: Role::TaskWorker,
                    area_id: Some(11),
                    model: None,
                    session_id: "s-1",
                    started_by: "web-lead",
                    now: at(50),
                },
            )
            .await
        })
        .await
        .unwrap()
}

async fn waiting(store: &Store) -> Vec<agents::QueuedStartRow> {
    let mut conn = store.read().acquire().await.unwrap();
    agents::waiting_starts(&mut conn).await.unwrap()
}

#[tokio::test]
async fn a_queued_start_keeps_what_the_starter_gave() {
    let (_dir, store) = seeded().await;
    let tasks = [task(2), task(1)];
    let id = queue(
        &store,
        NewQueuedStart {
            name: Some("web-worker-2"),
            model: Some("sonnet"),
            instructions: Some("read the board rules"),
            ..task_start(11, &tasks, 20)
        },
    )
    .await
    .unwrap();
    let rows = waiting(&store).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, id);
    assert_eq!(row.role, Role::TaskWorker);
    assert_eq!(row.area_id, Some(11));
    assert_eq!(row.area, Some(AreaName::parse("web").unwrap()));
    assert_eq!(row.name.as_deref(), Some("web-worker-2"));
    assert_eq!(row.tasks, [task(2), task(1)]);
    assert!(row.proposals.is_empty());
    assert_eq!(row.review_plan, None);
    assert_eq!(row.model.as_deref(), Some("sonnet"));
    assert_eq!(row.instructions.as_deref(), Some("read the board rules"));
    assert_eq!(row.queued_by, "web-lead");
    assert_eq!(row.queued_at, at(20));
    assert_eq!(row.end, None);
}

#[tokio::test]
async fn a_reviewer_start_keeps_its_proposals_and_plan() {
    let (_dir, store) = seeded().await;
    let proposals = [proposal(2), proposal(1)];
    queue(
        &store,
        NewQueuedStart {
            role: Role::Reviewer,
            area_id: None,
            name: None,
            tasks: &[],
            proposals: &proposals,
            review_plan: Some(ReviewPlanId::from_number(1).unwrap()),
            model: None,
            instructions: None,
            queued_by: "review",
            now: at(30),
        },
    )
    .await
    .unwrap();
    let row = waiting(&store).await.remove(0);
    assert_eq!(row.role, Role::Reviewer);
    assert_eq!(row.area, None);
    assert_eq!(row.proposals, [proposal(2), proposal(1)]);
    assert_eq!(row.review_plan, ReviewPlanId::from_number(1));
}

#[tokio::test]
async fn waiting_starts_come_oldest_first_and_ended_ones_leave() {
    let (_dir, store) = seeded().await;
    let one = [task(1)];
    let two = [task(2)];
    let three = [task(3)];
    let late = queue(&store, task_start(11, &one, 40)).await.unwrap();
    let early = queue(&store, task_start(12, &two, 10)).await.unwrap();
    let dropped = queue(&store, task_start(11, &three, 20)).await.unwrap();
    let ids: Vec<i64> = waiting(&store).await.iter().map(|row| row.id).collect();
    assert_eq!(ids, [early, dropped, late]);

    let ended = store
        .write(async |tx| {
            agents::drop_queued_start(tx, dropped, DropReason::TaskDone, at(60)).await
        })
        .await
        .unwrap();
    assert!(ended);
    let ids: Vec<i64> = waiting(&store).await.iter().map(|row| row.id).collect();
    assert_eq!(ids, [early, late]);

    let mut conn = store.read().acquire().await.unwrap();
    let row = agents::queued_start(&mut conn, dropped)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.end,
        Some(QueueEnd::Dropped {
            reason: DropReason::TaskDone,
            at: at(60)
        })
    );
    assert_eq!(row.tasks, [task(3)]);
    assert_eq!(agents::queued_start(&mut conn, 999).await.unwrap(), None);
}

#[tokio::test]
async fn a_start_ends_once() {
    let (_dir, store) = seeded().await;
    let one = [task(1)];
    let id = queue(&store, task_start(11, &one, 10)).await.unwrap();
    let worker = worker_process(&store).await;
    let started = store
        .write(async |tx| agents::mark_queued_start_started(tx, id, worker, at(70)).await)
        .await
        .unwrap();
    assert!(started);
    let again = store
        .write(async |tx| {
            let started = agents::mark_queued_start_started(tx, id, worker, at(80)).await?;
            let dropped = agents::drop_queued_start(tx, id, DropReason::Cancelled, at(80)).await?;
            Ok::<_, StoreError>((started, dropped))
        })
        .await
        .unwrap();
    assert_eq!(again, (false, false));
    let mut conn = store.read().acquire().await.unwrap();
    let row = agents::queued_start(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(
        row.end,
        Some(QueueEnd::Started {
            worker_process_id: worker,
            at: at(70)
        })
    );
    assert!(waiting(&store).await.is_empty());
}

#[tokio::test]
async fn every_drop_reason_reads_back() {
    let (_dir, store) = seeded().await;
    let one = [task(1)];
    for reason in [
        DropReason::TaskClaimed,
        DropReason::TaskDone,
        DropReason::TaskArchived,
        DropReason::ProposalEnded,
        DropReason::Cancelled,
    ] {
        let id = queue(&store, task_start(11, &one, 10)).await.unwrap();
        store
            .write(async |tx| agents::drop_queued_start(tx, id, reason, at(20)).await)
            .await
            .unwrap();
        let mut conn = store.read().acquire().await.unwrap();
        let row = agents::queued_start(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.end, Some(QueueEnd::Dropped { reason, at: at(20) }));
    }
}

#[tokio::test]
async fn the_last_start_of_each_area_is_the_rotation_input() {
    let (_dir, store) = seeded().await;
    let one = [task(1)];
    let worker = worker_process(&store).await;
    let first = queue(&store, task_start(11, &one, 10)).await.unwrap();
    let second = queue(&store, task_start(11, &one, 11)).await.unwrap();
    let dropped = queue(&store, task_start(12, &one, 12)).await.unwrap();
    let review = queue(
        &store,
        NewQueuedStart {
            role: Role::Reviewer,
            area_id: None,
            queued_by: "review",
            ..task_start(11, &[], 13)
        },
    )
    .await
    .unwrap();
    store
        .write(async |tx| {
            agents::mark_queued_start_started(tx, first, worker, at(100)).await?;
            agents::mark_queued_start_started(tx, second, worker, at(300)).await?;
            agents::drop_queued_start(tx, dropped, DropReason::TaskClaimed, at(400)).await?;
            agents::mark_queued_start_started(tx, review, worker, at(200)).await
        })
        .await
        .unwrap();
    let last = agents::last_queued_starts_by_area(store.read())
        .await
        .unwrap();
    let mut expected = BTreeMap::new();
    expected.insert(Some(AreaName::parse("web").unwrap()), at(300));
    expected.insert(None, at(200));
    assert_eq!(last, expected);
}

#[tokio::test]
async fn unknown_items_and_unlimited_roles_are_refused() {
    let (_dir, store) = seeded().await;
    let missing = [task(1), task(9)];
    assert!(matches!(
        queue(&store, task_start(11, &missing, 10)).await,
        Err(StoreError::Invalid(_))
    ));
    let missing_proposal = [proposal(9)];
    assert!(matches!(
        queue(
            &store,
            NewQueuedStart {
                role: Role::Reviewer,
                area_id: None,
                proposals: &missing_proposal,
                ..task_start(11, &[], 10)
            }
        )
        .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        queue(
            &store,
            NewQueuedStart {
                role: Role::Reviewer,
                area_id: None,
                review_plan: ReviewPlanId::from_number(9),
                ..task_start(11, &[], 10)
            }
        )
        .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        queue(
            &store,
            NewQueuedStart {
                role: Role::AreaLead,
                ..task_start(11, &[], 10)
            }
        )
        .await,
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM worker_start_queue").await,
        0
    );
}

#[tokio::test]
async fn the_schema_holds_the_queue_rules() {
    let (_dir, store) = seeded().await;
    let one = [task(1)];
    let id = queue(&store, task_start(11, &one, 10)).await.unwrap();
    let refused = [
        // A task worker has an area, a reviewer none.
        "INSERT INTO worker_start_queue (role, queued_by, queued_at) VALUES ('task_worker', 'x', 0)",
        "INSERT INTO worker_start_queue (role, area_id, queued_by, queued_at) VALUES ('reviewer', 11, 'x', 0)",
        "INSERT INTO worker_start_queue (role, area_id, queued_by, queued_at) VALUES ('area_lead', 11, 'x', 0)",
        "INSERT INTO worker_start_queue (role, area_id, review_plan_id, queued_by, queued_at) VALUES ('task_worker', 11, 1, 'x', 0)",
        "INSERT INTO worker_start_queue (role, queued_by, queued_at) VALUES ('reviewer', '', 0)",
        "INSERT INTO worker_start_queue (role, model, queued_by, queued_at) VALUES ('reviewer', '', 'x', 0)",
        // An end needs its outcome, a drop its reason and a start its worker.
        "INSERT INTO worker_start_queue (role, queued_by, queued_at, ended_at) VALUES ('reviewer', 'x', 0, 1)",
        "INSERT INTO worker_start_queue (role, queued_by, queued_at, ended_at, outcome) VALUES ('reviewer', 'x', 0, 1, 'dropped')",
        "INSERT INTO worker_start_queue (role, queued_by, queued_at, ended_at, outcome) VALUES ('reviewer', 'x', 0, 1, 'started')",
        "INSERT INTO worker_start_queue (role, queued_by, queued_at, ended_at, outcome, drop_reason) VALUES ('reviewer', 'x', 0, 1, 'dropped', 'bored')",
        // The fixed columns never change, and nothing is deleted.
        "UPDATE worker_start_queue SET instructions = 'other'",
        "UPDATE worker_start_queue SET queued_at = 99",
        "DELETE FROM worker_start_queue",
        "INSERT INTO worker_start_queue_tasks (queued_start_id, position, task_id) VALUES (1, 1, 1)",
        "UPDATE worker_start_queue_tasks SET task_id = 2",
        "DELETE FROM worker_start_queue_tasks",
    ];
    for statement in refused {
        assert!(
            exec(&store, statement).await.is_err(),
            "accepted: {statement}"
        );
    }
    exec(
        &store,
        "INSERT INTO worker_start_queue_proposals (queued_start_id, position, proposal_id) VALUES (1, 0, 1)",
    )
    .await
    .unwrap();
    for statement in [
        "UPDATE worker_start_queue_proposals SET proposal_id = 2",
        "DELETE FROM worker_start_queue_proposals",
    ] {
        assert!(
            exec(&store, statement).await.is_err(),
            "accepted: {statement}"
        );
    }
    store
        .write(async |tx| agents::drop_queued_start(tx, id, DropReason::Cancelled, at(20)).await)
        .await
        .unwrap();
    assert!(
        exec(&store, "UPDATE worker_start_queue SET ended_at = 30")
            .await
            .is_err()
    );
}

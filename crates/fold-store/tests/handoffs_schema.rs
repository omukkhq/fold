#![cfg(test)]

use fold_core::{ProposalId, Role, TaskId, TaskStatus, Timestamp, WorkerState};
use fold_store::Store;
use fold_store::agents::{self, NewWorkerProcess};

use crate::common::{count, exec, open};

const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-lead', 0), (2, 'author', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (2, 2, 'author', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (1, 'ui', 'sage', 0), (2, 'server', 'sage', 0);
INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (1, 'web', 1, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at, status) VALUES (1, 1, 'one', 'fold', 0, 0, 'todo'), (2, 2, 'two', 'fold', 0, 0, 'done'), (3, 3, 'three', 'fold', 0, 0, 'in_progress'), (4, 4, 'four', 'fold', 0, 0, 'backlog'), (5, 5, 'five', 'fold', 0, 0, 'review');
UPDATE tasks SET archived_at = 1 WHERE number = 4;
INSERT INTO task_labels (task_id, label_id) VALUES (1, 1), (2, 1), (3, 1), (4, 1), (5, 2);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 3, 2, 'draw', 0, 0), (2, 2, 5, 2, 'serve', 0, 0);
";

async fn seeded() -> (tempfile::TempDir, Store, i64) {
    let (dir, _paths, store) = open().await;
    for statement in ROWS.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    let id = store
        .write(async |tx| {
            agents::insert_worker_process(
                tx.conn(),
                &NewWorkerProcess {
                    agent_id: 1,
                    role: Role::AreaLead,
                    area_id: Some(1),
                    model: None,
                    session_id: "s-1",
                    started_by: "human:ana",
                    now: at(10),
                },
            )
            .await
        })
        .await
        .unwrap();
    (dir, store, id)
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[tokio::test]
async fn notes_are_kept_per_session_and_never_change() {
    let (_dir, store, _id) = seeded().await;
    assert_eq!(agents::latest_handoff(store.read(), 1).await.unwrap(), None);
    for (session, body, time) in [
        ("s-1", "first", 20),
        ("s-1", "second", 30),
        ("s-2", "third", 40),
    ] {
        store
            .write(async |tx| agents::insert_handoff(tx.conn(), 1, session, body, at(time)).await)
            .await
            .unwrap();
    }
    let latest = agents::latest_handoff(store.read(), 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (latest.session_id.as_str(), latest.body.as_str()),
        ("s-2", "third")
    );
    assert_eq!(latest.created_at, at(40));
    let of_first = agents::session_handoff(store.read(), "s-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(of_first.body, "second");
    assert_eq!(
        agents::session_handoff(store.read(), "s-9").await.unwrap(),
        None
    );
    let update = exec(&store, "UPDATE handoffs SET body = 'x'")
        .await
        .unwrap_err();
    assert!(update.to_string().contains("append-only"), "{update}");
    let delete = exec(&store, "DELETE FROM handoffs").await.unwrap_err();
    assert!(delete.to_string().contains("append-only"), "{delete}");
    let empty = exec(
        &store,
        "INSERT INTO handoffs (agent_id, session_id, body, created_at) VALUES (1, 's-1', '', 0)",
    )
    .await
    .unwrap_err();
    assert!(
        empty.to_string().contains("handoffs_body_present"),
        "{empty}"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM handoffs").await, 3);
}

#[tokio::test]
async fn a_session_is_asked_once_and_a_refresh_starts_a_new_one() {
    let (_dir, store, id) = seeded().await;
    assert_eq!(agents::handoff_asked(store.read(), id).await.unwrap(), None);
    let first = store
        .write(async |tx| agents::set_handoff_asked(tx.conn(), id, 4, at(20)).await)
        .await
        .unwrap();
    let again = store
        .write(async |tx| agents::set_handoff_asked(tx.conn(), id, 6, at(30)).await)
        .await
        .unwrap();
    assert!(first && !again);
    assert_eq!(
        agents::handoff_asked(store.read(), id).await.unwrap(),
        Some(4)
    );
    exec(&store, "UPDATE worker_processes SET context_tokens = 700000, pid = 9, state = 'idle', idle_since = 25")
        .await
        .unwrap();
    store
        .write(async |tx| agents::refresh_session(tx.conn(), id, "s-2", at(50)).await)
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let row =
        agents::find_worker_process(&mut conn, &fold_core::AgentName::parse("web-lead").unwrap())
            .await
            .unwrap()
            .unwrap();
    drop(conn);
    assert_eq!(row.session_id, "s-2");
    assert_eq!(row.state, WorkerState::Starting);
    assert_eq!(row.started_at, at(50));
    assert_eq!(
        (row.context_tokens, row.pid, row.idle_since),
        (0, None, None)
    );
    assert_eq!(agents::handoff_asked(store.read(), id).await.unwrap(), None);
    let session = exec(
        &store,
        "UPDATE worker_processes SET session_id = 's-3', state = 'idle'",
    )
    .await
    .unwrap_err();
    assert!(
        session.to_string().contains("worker_processes_fixed"),
        "{session}"
    );
    let role = exec(
        &store,
        "UPDATE worker_processes SET role = 'review_lead', area_id = NULL",
    )
    .await
    .unwrap_err();
    assert!(
        role.to_string().contains("worker_processes_fixed"),
        "{role}"
    );
}

#[tokio::test]
async fn the_summary_reads_the_open_tasks_and_proposals_of_a_label() {
    let (_dir, store, _id) = seeded().await;
    let mut conn = store.read().acquire().await.unwrap();
    let (tasks, total) = agents::open_area_tasks(&mut conn, Some(1), 20)
        .await
        .unwrap();
    assert_eq!(total, 2, "done and archived tasks are not open");
    let ids: Vec<(TaskId, TaskStatus)> = tasks.iter().map(|task| (task.id, task.status)).collect();
    assert_eq!(
        ids,
        [
            (TaskId::from_number(1).unwrap(), TaskStatus::Todo),
            (TaskId::from_number(3).unwrap(), TaskStatus::InProgress)
        ]
    );
    assert_eq!(tasks[0].title, "one");
    let (first, all) = agents::open_area_tasks(&mut conn, None, 1).await.unwrap();
    assert_eq!((first.len(), all), (1, 3));
    let (proposals, total) = agents::open_area_proposals(&mut conn, Some(1), 20)
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(proposals[0].id, ProposalId::from_number(1).unwrap());
    assert_eq!(proposals[0].task, TaskId::from_number(3).unwrap());
    assert_eq!(proposals[0].title, "draw");
    let (every, total) = agents::open_area_proposals(&mut conn, None, 20)
        .await
        .unwrap();
    assert_eq!((every.len(), total), (2, 2));
}

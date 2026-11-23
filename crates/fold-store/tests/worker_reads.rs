use std::time::Instant;

use fold_core::{AgentName, Role, TaskId, WorkerState};
use fold_store::Store;
use fold_store::agents;
use sqlx::Row;

use crate::common::{exec, open};

/// Agents 1 to 8: a review lead, an area lead, task workers (running, idle, stopped, failed) and reviewers (running, stopped). Tasks 1 and 2 exist.
#[cfg(test)]
const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'rl', 0), (2, 'al', 0), (3, 'tw-run', 0), (4, 'tw-idle', 0), (5, 'tw-stop', 0), (6, 'tw-fail', 0), (7, 'rv-run', 0), (8, 'rv-stop', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (11, 'ui', 'sage', 0);
INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (11, 'web', 11, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (5, 5, 'tw-stop', 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 3, 2, 5, 'p', 0, 0);
INSERT INTO worker_processes (id, agent_id, role, area_id, session_id, state, started_by, started_at, stopped_at, idle_since) VALUES
 (1, 1, 'review_lead', NULL, 's1', 'working', 'main', 1, NULL, NULL),
 (2, 2, 'area_lead', 11, 's2', 'stopping', 'main', 2, NULL, NULL),
 (3, 3, 'task_worker', 11, 's3', 'starting', 'main', 3, NULL, NULL),
 (4, 4, 'task_worker', 11, 's4', 'idle', 'main', 4, NULL, 9),
 (5, 5, 'task_worker', 11, 's5', 'stopped', 'main', 5, 9, NULL),
 (6, 6, 'task_worker', 11, 's6', 'failed', 'main', 6, 9, NULL),
 (7, 7, 'reviewer', NULL, 's7', 'working', 'main', 7, NULL, NULL),
 (8, 8, 'reviewer', NULL, 's8', 'stopped', 'main', 8, 9, NULL);
INSERT INTO worker_tasks (worker_process_id, task_id) VALUES (3, 1), (3, 2), (5, 1);
INSERT INTO worker_proposals (worker_process_id, proposal_id) VALUES (7, 1), (8, 1);
";

#[cfg(test)]
fn name(value: &str) -> AgentName {
    AgentName::parse(value).unwrap()
}

#[tokio::test]
async fn each_read_gives_the_rows_of_the_full_listing_that_it_promises() {
    let (_dir, store) = open_with_rows().await;
    let mut conn = store.read().acquire().await.unwrap();
    let all = agents::list_worker_processes(&mut conn).await.unwrap();
    assert_eq!(all.len(), 8);
    let pick = |ids: &[i64]| -> Vec<_> {
        all.iter()
            .filter(|row| ids.contains(&row.id))
            .cloned()
            .collect()
    };

    let running = agents::list_running_worker_processes(&mut conn)
        .await
        .unwrap();
    assert_eq!(running, pick(&[1, 2, 3, 4, 7]));

    let unstopped = agents::list_unstopped_worker_processes(&mut conn)
        .await
        .unwrap();
    assert_eq!(unstopped, pick(&[1, 2, 3, 4, 6, 7]));
    assert_eq!(
        unstopped[2].tasks,
        vec![
            TaskId::try_from_db(1).unwrap(),
            TaskId::try_from_db(2).unwrap()
        ]
    );

    let coders = agents::list_worker_processes_of_role(&mut conn, Role::TaskWorker)
        .await
        .unwrap();
    assert_eq!(coders, pick(&[3, 4, 5, 6]));
    let reviewers = agents::list_worker_processes_of_role(&mut conn, Role::Reviewer)
        .await
        .unwrap();
    assert_eq!(reviewers, pick(&[7, 8]));
    assert!(
        agents::list_worker_processes_of_role(&mut conn, Role::Main)
            .await
            .unwrap()
            .is_empty()
    );

    let some = agents::list_worker_processes_of_agents(
        &mut conn,
        &[name("tw-stop"), name("rv-run"), name("al"), name("nobody")],
    )
    .await
    .unwrap();
    assert_eq!(some, pick(&[2, 5, 7]));
    assert!(
        agents::list_worker_processes_of_agents(&mut conn, &[])
            .await
            .unwrap()
            .is_empty()
    );

    // Bindings are read for the rows each read gives, ended ones included.
    assert_eq!(
        coders[0].tasks,
        vec![
            TaskId::try_from_db(1).unwrap(),
            TaskId::try_from_db(2).unwrap()
        ]
    );
    assert_eq!(some[1].tasks, vec![TaskId::try_from_db(1).unwrap()]);
    assert_eq!(some[2].proposals.len(), 1);
}

#[tokio::test]
async fn the_smaller_reads_give_the_agent_with_its_state_or_the_agents_of_a_role() {
    let (_dir, store) = open_with_rows().await;
    let states = agents::running_worker_states(store.read()).await.unwrap();
    assert_eq!(
        states,
        vec![
            (name("rl"), WorkerState::Working),
            (name("al"), WorkerState::Stopping),
            (name("tw-run"), WorkerState::Starting),
            (name("tw-idle"), WorkerState::Idle),
            (name("rv-run"), WorkerState::Working),
        ]
    );
    let coders = agents::worker_agents_of_role(store.read(), Role::TaskWorker)
        .await
        .unwrap();
    assert_eq!(
        coders,
        vec![
            name("tw-run"),
            name("tw-idle"),
            name("tw-stop"),
            name("tw-fail")
        ]
    );
}

#[cfg(test)]
async fn open_with_rows() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    exec(&store, ROWS).await.unwrap();
    (dir, store)
}

#[cfg(test)]
async fn plan(store: &Store, sql: &str) -> String {
    sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
        .fetch_all(store.read())
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[tokio::test]
async fn the_state_and_role_reads_use_an_index() {
    let (_dir, store) = open_with_rows().await;
    let running =
        plan(&store, "SELECT id FROM worker_processes w WHERE w.state IN ('starting', 'working', 'idle', 'stopping') ORDER BY w.id").await;
    let unstopped =
        plan(&store, "SELECT id FROM worker_processes w WHERE w.state IN ('starting', 'working', 'idle', 'stopping', 'failed') ORDER BY w.id").await;
    let role = plan(
        &store,
        "SELECT id FROM worker_processes w WHERE w.role = 'task_worker' ORDER BY w.id",
    )
    .await;
    println!("running: {running}\nrole: {role}");
    assert!(running.contains("worker_processes_state"), "{running}");
    assert!(unstopped.contains("worker_processes_state"), "{unstopped}");
    assert!(role.contains("worker_processes_role_state"), "{role}");
}

/// Prints the time of each read on 1,200 worker rows with 10 that run; run it with `--run-ignored all`.
#[tokio::test]
#[ignore = "prints timings"]
async fn timing_on_many_worker_rows() {
    let (_dir, _paths, store) = open().await;
    let mut sql = String::from(
        "INSERT INTO labels (id, name, color, created_at) VALUES (11, 'ui', 'sage', 0);
         INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (11, 'web', 11, 'main', 0);",
    );
    for i in 1..=1200 {
        let running = i > 1190;
        let (state, stopped, idle) = if running {
            ("idle", "NULL", "9")
        } else {
            ("stopped", "9", "NULL")
        };
        sql.push_str(&format!(
            "INSERT INTO agents (id, name, created_at) VALUES ({i}, 'w-{i}', 0);
             INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES ({i}, {i}, 't', 'fold', 0, 0);
             INSERT INTO worker_processes (id, agent_id, role, area_id, session_id, state, started_by, started_at, stopped_at, idle_since)
               VALUES ({i}, {i}, 'task_worker', 11, 's{i}', '{state}', 'main', {i}, {stopped}, {idle});
             INSERT INTO worker_tasks (worker_process_id, task_id) VALUES ({i}, {i});"
        ));
    }
    exec(&store, &sql).await.unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    macro_rules! time {
        ($label:expr, $call:expr) => {{
            let start = Instant::now();
            let mut rows = 0;
            for _ in 0..20 {
                rows = $call.await.unwrap().len();
            }
            println!(
                "{:<40} {:>5} rows {:>10.2?} per call",
                $label,
                rows,
                start.elapsed() / 20
            );
        }};
    }
    time!(
        "list_worker_processes",
        agents::list_worker_processes(&mut conn)
    );
    time!(
        "list_running_worker_processes",
        agents::list_running_worker_processes(&mut conn)
    );
    time!(
        "list_worker_processes_of_role",
        agents::list_worker_processes_of_role(&mut conn, Role::TaskWorker)
    );
    time!(
        "list_worker_processes_of_role(none)",
        agents::list_worker_processes_of_role(&mut conn, Role::Reviewer)
    );
    let ten: Vec<AgentName> = (1191..=1200).map(|i| name(&format!("w-{i}"))).collect();
    time!(
        "list_worker_processes_of_agents(10)",
        agents::list_worker_processes_of_agents(&mut conn, &ten)
    );
    time!(
        "running_worker_states",
        agents::running_worker_states(store.read())
    );
    time!(
        "worker_agents_of_role",
        agents::worker_agents_of_role(store.read(), Role::TaskWorker)
    );
}

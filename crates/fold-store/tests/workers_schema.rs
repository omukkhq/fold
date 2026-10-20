use fold_core::roles::AreaProfile;
use fold_core::{AgentName, ProposalId, Role, TaskId, Timestamp, WorkerState};
use fold_store::Store;
use fold_store::agents::{self, NewWorkerProcess, TurnTotals};

use crate::common::{count, exec, open};

const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-lead', 0), (2, 'web-worker', 0), (3, 'api-lead', 0), (4, 'review-lead', 0), (5, 'author', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (5, 5, 'author', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (11, 'ui', 'sage', 0), (12, 'server', 'sage', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0);
INSERT INTO task_labels (task_id, label_id) VALUES (1, 11);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 3, 2, 5, 'p', 0, 0);
";

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in ROWS.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

/// The areas but the built-in area main, which every project has.
#[cfg(test)]
async fn led_areas(store: &Store) -> Vec<agents::AreaRow> {
    let mut areas = agents::list_areas(store.read()).await.unwrap();
    areas.retain(|area| !area.name.is_main());
    areas
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
async fn process(
    store: &Store,
    agent_id: i64,
    role: Role,
    area_id: Option<i64>,
    session: &str,
) -> i64 {
    store
        .write(async |tx| {
            agents::insert_worker_process(
                tx,
                &NewWorkerProcess {
                    agent_id,
                    role,
                    area_id,
                    model: None,
                    session_id: session,
                    started_by: "human:ana",
                    now: at(10),
                },
            )
            .await
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn an_area_has_one_lead_for_good_and_one_label() {
    let (_dir, store) = seeded().await;
    let web = fold_core::AreaName::parse("web").unwrap();
    let ui = fold_core::LabelName::parse("ui").unwrap();
    let label = agents::label_id(store.read(), &ui).await.unwrap().unwrap();
    let area = store
        .write(async |tx| agents::insert_area(tx, &web, label, "main", at(1)).await)
        .await
        .unwrap();
    let same_label = exec(
        &store,
        "INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('site', 11, 'main', 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(same_label.constraint().as_deref(), Some("areas.label_id"));
    store
        .write(async |tx| agents::set_area_lead(tx, area, 1).await)
        .await
        .unwrap();
    let second = store
        .write(async |tx| agents::set_area_lead(tx, area, 3).await)
        .await
        .unwrap_err();
    assert!(second.to_string().contains("areas_lead_once"), "{second}");
    let row = agents::find_area(store.read(), &web)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.lead, Some(AgentName::parse("web-lead").unwrap()));
    assert_eq!(row.label, ui);
    assert_eq!(led_areas(&store).await, vec![row]);
    exec(
        &store,
        "INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('api', 12, 'main', 0)",
    )
    .await
    .unwrap();
    let lead_twice = exec(
        &store,
        "UPDATE areas SET lead_agent_id = 1 WHERE name = 'api'",
    )
    .await
    .unwrap_err();
    assert_eq!(
        lead_twice.constraint().as_deref(),
        Some("areas.lead_agent_id")
    );
    assert!(exec(&store, "DELETE FROM areas").await.is_err());
}

#[tokio::test]
async fn every_project_has_the_area_main_without_a_lead_and_keeps_its_label() {
    let (_dir, store) = seeded().await;
    let main = agents::find_area(store.read(), &fold_core::AreaName::main())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(main.label, fold_core::LabelName::main());
    assert_eq!(main.lead, None);
    assert_eq!(main.profile, AreaProfile::default());
    for sql in [
        "UPDATE areas SET lead_agent_id = 1 WHERE name = 'main'",
        "UPDATE areas SET instructions = 'draw' WHERE name = 'main'",
        "UPDATE areas SET model = 'opus' WHERE name = 'main'",
        "INSERT INTO area_mcp_servers (area_id, position, name) VALUES ((SELECT id FROM areas WHERE name = 'main'), 0, 'fold')",
    ] {
        let refused = exec(&store, sql).await.unwrap_err();
        assert!(
            refused.to_string().contains("areas_main_without_lead"),
            "{sql}: {refused}"
        );
    }
    for sql in [
        "UPDATE labels SET name = 'core' WHERE name = 'main'",
        "DELETE FROM labels WHERE name = 'main'",
    ] {
        let refused = exec(&store, sql).await.unwrap_err();
        assert!(
            refused.to_string().contains("labels_main_kept"),
            "{sql}: {refused}"
        );
    }
    exec(
        &store,
        "UPDATE labels SET color = 'gold' WHERE name = 'main'",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_area_profile_changes_while_its_lead_stays_set_once() {
    let (_dir, store) = seeded().await;
    let web = fold_core::AreaName::parse("web").unwrap();
    let area = store
        .write(async |tx| agents::insert_area(tx, &web, 11, "main", at(1)).await)
        .await
        .unwrap();
    let fresh = agents::find_area(store.read(), &web)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.profile, AreaProfile::default());
    store
        .write(async |tx| agents::set_area_lead(tx, area, 1).await)
        .await
        .unwrap();
    let design = AreaProfile {
        instructions: Some("draw screens in Pencil".to_owned()),
        model: Some("opus".to_owned()),
        mcp: Some(vec!["pencil".to_owned(), "fold".to_owned()]),
    };
    store
        .write(async |tx| agents::set_area_profile(tx, area, &design).await)
        .await
        .unwrap();
    let row = agents::find_area(store.read(), &web)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.profile, design);
    assert_eq!(row.lead, Some(AgentName::parse("web-lead").unwrap()));
    assert_eq!(led_areas(&store).await, vec![row]);

    let fewer = AreaProfile {
        mcp: Some(vec!["fold".to_owned()]),
        ..AreaProfile::default()
    };
    store
        .write(async |tx| agents::set_area_profile(tx, area, &fewer).await)
        .await
        .unwrap();
    let row = agents::find_area(store.read(), &web)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.profile, fewer);
    assert_eq!(
        count(&store, "SELECT count(*) FROM area_mcp_servers").await,
        1
    );

    let cleared = exec(
        &store,
        "UPDATE areas SET lead_agent_id = NULL WHERE name = 'web'",
    )
    .await
    .unwrap_err();
    assert!(cleared.to_string().contains("areas_lead_once"), "{cleared}");
    let renamed = exec(&store, "UPDATE areas SET name = 'site' WHERE name = 'web'")
        .await
        .unwrap_err();
    assert!(renamed.to_string().contains("areas_lead_once"), "{renamed}");
    let twice = exec(
        &store,
        "INSERT INTO area_mcp_servers (area_id, position, name) VALUES ((SELECT id FROM areas WHERE name = 'web'), 5, 'fold')",
    )
    .await
    .unwrap_err();
    assert_eq!(
        twice.constraint().as_deref(),
        Some("area_mcp_servers.area_id, area_mcp_servers.name")
    );
    let blank = exec(
        &store,
        "UPDATE areas SET instructions = '  ' WHERE name = 'web'",
    )
    .await
    .unwrap_err();
    assert_eq!(
        blank.constraint().as_deref(),
        Some("areas_instructions_present")
    );
}

#[tokio::test]
async fn a_worker_process_keeps_its_role_area_and_session_and_binds_its_own_kind_of_work() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (11, 'web', 11, 'main', 0)",
    )
    .await
    .unwrap();
    let no_area = exec(
        &store,
        "INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (1, 'area_lead', 's0', 'starting', 'main', 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        no_area.constraint().as_deref(),
        Some("worker_processes_area_of_role")
    );
    let lead = process(&store, 1, Role::AreaLead, Some(11), "s1").await;
    let task_worker = process(&store, 2, Role::TaskWorker, Some(11), "s2").await;
    let reviewer = process(&store, 4, Role::Reviewer, None, "s4").await;
    let twice = exec(
        &store,
        "INSERT INTO worker_processes (agent_id, role, area_id, session_id, state, started_by, started_at) VALUES (1, 'area_lead', 11, 's9', 'starting', 'main', 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        twice.constraint().as_deref(),
        Some("worker_processes.agent_id")
    );
    let task = TaskId::from_number(1).unwrap();
    let proposal = ProposalId::from_number(3).unwrap();
    store
        .write(async |tx| {
            assert!(agents::bind_task(tx, task_worker, task).await?);
            assert!(
                agents::bind_task(tx, task_worker, task).await?,
                "a task named twice counts as bound"
            );
            assert!(!agents::bind_task(tx, task_worker, TaskId::from_number(9).unwrap()).await?);
            assert!(agents::bind_proposal(tx, reviewer, proposal).await?);
            assert!(
                agents::bind_proposal(tx, reviewer, proposal).await?,
                "a proposal named twice counts as bound"
            );
            assert!(
                !agents::bind_proposal(tx, reviewer, ProposalId::from_number(9).unwrap()).await?
            );
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let rows = agents::list_worker_processes(&mut conn).await.unwrap();
    let bound: Vec<(Vec<TaskId>, Vec<ProposalId>)> = rows
        .iter()
        .map(|row| (row.tasks.clone(), row.proposals.clone()))
        .collect();
    assert_eq!(
        bound,
        [
            (vec![], vec![]),
            (vec![task], vec![]),
            (vec![], vec![proposal])
        ],
        "each row reads its own bindings once"
    );
    let found = agents::find_worker_process(&mut conn, &rows[2].agent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.proposals, [proposal]);
    drop(conn);
    let wrong_kind = store
        .write(async |tx| agents::bind_task(tx, lead, task).await)
        .await
        .unwrap_err();
    assert!(
        wrong_kind
            .to_string()
            .contains("worker_tasks_task_worker_only")
    );
    let wrong_kind = store
        .write(async |tx| agents::bind_proposal(tx, task_worker, proposal).await)
        .await
        .unwrap_err();
    assert!(
        wrong_kind
            .to_string()
            .contains("worker_proposals_reviewer_only")
    );
    for change in [
        "UPDATE worker_processes SET role = 'review_lead' WHERE id = 1",
        "UPDATE worker_processes SET session_id = 'other', state = 'working' WHERE id = 1",
        "UPDATE worker_processes SET area_id = NULL WHERE id = 1",
        "DELETE FROM worker_processes WHERE id = 1",
    ] {
        assert!(exec(&store, change).await.is_err(), "{change}");
    }
    let bad_idle = exec(
        &store,
        "UPDATE worker_processes SET state = 'idle' WHERE id = 1",
    )
    .await
    .unwrap_err();
    assert_eq!(
        bad_idle.constraint().as_deref(),
        Some("worker_processes_idle_since_iff_idle")
    );
    assert_eq!(
        agents::task_workers_of_task(store.read(), task)
            .await
            .unwrap(),
        [agents::BoundWorker {
            id: task_worker,
            agent: AgentName::parse("web-worker").unwrap()
        }]
    );
    assert_eq!(
        agents::tasks_awaiting_proposal(store.read(), &[task_worker])
            .await
            .unwrap()[&task_worker],
        1
    );
    exec(
        &store,
        "INSERT INTO proposals (id, number, task_id, author_worker_id, title, state, created_at, updated_at) VALUES (2, 4, 1, 5, 'q', 'integrated', 0, 0)",
    )
    .await
    .unwrap();
    assert_eq!(
        agents::tasks_awaiting_proposal(store.read(), &[task_worker])
            .await
            .unwrap()[&task_worker],
        0,
        "a proposal that integrated before fold looked counts as sent"
    );
    exec(&store, "DELETE FROM proposals WHERE id = 2")
        .await
        .unwrap();
    exec(&store, "UPDATE tasks SET status = 'done' WHERE id = 1")
        .await
        .unwrap();
    assert_eq!(
        agents::tasks_awaiting_proposal(store.read(), &[task_worker])
            .await
            .unwrap()[&task_worker],
        0,
        "a Done task waits for nothing"
    );
    assert_eq!(
        agents::reviewers_of_proposal(store.read(), proposal)
            .await
            .unwrap(),
        [agents::BoundWorker {
            id: reviewer,
            agent: AgentName::parse("review-lead").unwrap()
        }]
    );
    assert_eq!(
        agents::proposals_without_verdict(store.read(), &[reviewer])
            .await
            .unwrap()[&reviewer],
        1
    );
    let both = agents::proposals_without_verdict(store.read(), &[reviewer, lead])
        .await
        .unwrap();
    assert_eq!(
        (both[&reviewer], both[&lead]),
        (1, 0),
        "every asked worker gets a count"
    );
    exec(&store, "DELETE FROM tasks WHERE id = 1")
        .await
        .unwrap();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM worker_tasks").await, 0);
}

#[tokio::test]
async fn the_row_follows_its_process_through_turns_stops_and_restarts() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (11, 'web', 11, 'main', 0)",
    )
    .await
    .unwrap();
    let id = process(&store, 1, Role::AreaLead, Some(11), "s1").await;
    let web = Some(11);
    assert_eq!(
        agents::running_counts(store.read(), Role::AreaLead, web)
            .await
            .unwrap(),
        (1, 1)
    );
    assert_eq!(
        agents::running_counts(store.read(), Role::AreaLead, None)
            .await
            .unwrap(),
        (1, 0)
    );
    assert_eq!(
        agents::running_counts(store.read(), Role::TaskWorker, web)
            .await
            .unwrap(),
        (0, 0),
        "each role counts its own workers"
    );
    let lead = AgentName::parse("web-lead").unwrap();
    store
        .write(async |tx| {
            agents::set_worker_pid(tx, id, Some(42)).await?;
            assert!(agents::mark_worker_working(tx, id).await?);
            agents::set_worker_context(tx, id, Some("sonnet"), Some(900)).await?;
            agents::record_turn(
                tx,
                id,
                TurnTotals {
                    cost_micro_usd: 30,
                    cache_read_tokens: 5,
                    cache_write_tokens: 7,
                },
                at(20),
            )
            .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let row = agents::find_worker_process(&mut conn, &lead)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    assert_eq!(row.state, WorkerState::Idle);
    assert_eq!(row.idle_since, Some(at(20)));
    assert_eq!(
        (
            row.turns,
            row.cost_micro_usd,
            row.cache_read_tokens,
            row.cache_write_tokens,
            row.context_tokens
        ),
        (1, 30, 5, 7, 900)
    );
    assert_eq!(row.model.as_deref(), Some("sonnet"));
    assert_eq!(row.pid, Some(42));
    let pid_after_clear = async |pid: u32| {
        store
            .write(async |tx| agents::clear_worker_pid(tx, id, pid).await)
            .await
            .unwrap();
        let mut conn = store.read().acquire().await.unwrap();
        agents::find_worker_process(&mut conn, &lead)
            .await
            .unwrap()
            .unwrap()
            .pid
    };
    assert_eq!(
        pid_after_clear(41).await,
        Some(42),
        "an ended process clears only its own pid"
    );
    assert_eq!(pid_after_clear(42).await, None);
    store
        .write(async |tx| {
            agents::set_worker_state(tx, id, WorkerState::Stopping, at(30)).await?;
            assert!(!agents::mark_worker_working(tx, id).await?);
            agents::record_exit(tx, id, agents::ProcessEnd::Stopped, Some(0), at(31)).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    assert_eq!(
        agents::running_counts(store.read(), Role::AreaLead, web)
            .await
            .unwrap(),
        (0, 0)
    );
    store
        .write(async |tx| agents::restart_worker_process(tx, id, at(40)).await)
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let rows = agents::list_worker_processes(&mut conn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, WorkerState::Starting);
    assert_eq!(rows[0].started_at, at(40));
    assert_eq!(
        (rows[0].stopped_at, rows[0].exit_code, rows[0].pid),
        (None, None, None)
    );
    assert_eq!(rows[0].turns, 1, "the totals add up over every process");
}

#[tokio::test]
async fn a_project_has_one_review_lead() {
    let (_dir, store) = seeded().await;
    assert_eq!(agents::review_lead(store.read()).await.unwrap(), None);
    process(&store, 4, Role::ReviewLead, None, "s1").await;
    let lead = agents::review_lead(store.read()).await.unwrap().unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let row = agents::find_worker_process(&mut conn, &lead).await.unwrap();
    assert_eq!(row.map(|row| row.role), Some(Role::ReviewLead));
    drop(conn);
    let second = exec(
        &store,
        "INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (5, 'review_lead', 's2', 'starting', 'main', 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        second.constraint().as_deref(),
        Some("worker_processes.role")
    );
    process(&store, 5, Role::Reviewer, None, "s3").await;
}

#[tokio::test]
async fn a_new_session_comes_only_with_a_new_start() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (11, 'web', 11, 'main', 0)",
    )
    .await
    .unwrap();
    let task_worker = process(&store, 2, Role::TaskWorker, Some(11), "s2").await;
    store
        .write(async |tx| {
            agents::set_worker_state(tx, task_worker, WorkerState::Working, at(15)).await
        })
        .await
        .unwrap();
    for change in [
        format!("UPDATE worker_processes SET session_id = 's3' WHERE id = {task_worker}"),
        format!(
            "UPDATE worker_processes SET session_id = 's3', state = 'idle', idle_since = 20 WHERE id = {task_worker}"
        ),
    ] {
        let refused = exec(&store, &change).await.unwrap_err();
        assert!(
            refused.to_string().contains("worker_processes_fixed"),
            "{change}"
        );
    }
    exec(
        &store,
        &format!(
            "UPDATE worker_processes SET session_id = 's2b', state = 'starting' WHERE id = {task_worker}"
        ),
    )
    .await
    .expect("a running row moves to starting with a new session");
    store
        .write(async |tx| {
            agents::record_exit(
                tx,
                task_worker,
                agents::ProcessEnd::Failed("mcp_unavailable"),
                Some(1),
                at(30),
            )
            .await
        })
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let rows = agents::list_worker_processes(&mut conn).await.unwrap();
    assert_eq!(rows[0].failed_reason.as_deref(), Some("mcp_unavailable"));
    drop(conn);
    store
        .write(async |tx| agents::refresh_session(tx, task_worker, "s3", at(40)).await)
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let rows = agents::list_worker_processes(&mut conn).await.unwrap();
    assert_eq!(
        rows[0].failed_reason, None,
        "a new start forgets the failure"
    );
    let refused = exec(
        &store,
        &format!("UPDATE worker_processes SET failed_reason = 'exited' WHERE id = {task_worker}"),
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("worker_processes_failed_reason_only_failed"),
        "{refused}"
    );
    assert_eq!(rows[0].session_id, "s3");
    assert_eq!(rows[0].state, WorkerState::Starting);
    assert_eq!(rows[0].started_at, at(40));
    assert_eq!((rows[0].stopped_at, rows[0].exit_code), (None, None));
    for column in ["agent_id = 3", "started_by = 'main'", "role = 'area_lead'"] {
        let change = format!("UPDATE worker_processes SET {column} WHERE id = {task_worker}");
        let refused = exec(&store, &change).await.unwrap_err();
        assert!(
            refused.to_string().contains("worker_processes_fixed"),
            "{change}"
        );
    }
}

/// Workers stored with the role name `task_coder`, one stopped and one running, with a binding of each kind and a permission question on that worker.
const OLD_WORKERS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-lead', 0), (2, 'web-coder', 0), (3, 'batch-reviewer', 0), (4, 'author', 0), (5, 'web-coder-2', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (4, 4, 'author', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (1, 'ui', 'sage', 0);
INSERT INTO areas (id, name, label_id, lead_agent_id, created_by, created_at) VALUES (1, 'web', 1, 1, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 2, 4, 'p', 0, 0);
INSERT INTO worker_processes (id, agent_id, role, area_id, session_id, state, started_by, started_at, stopped_at, idle_since, turns) VALUES (1, 1, 'area_lead', 1, 's1', 'working', 'main', 10, NULL, NULL, 0), (2, 2, 'task_coder', 1, 's2', 'stopped', 'web-lead', 10, 20, NULL, 7), (3, 3, 'batch_reviewer', NULL, 's3', 'idle', 'review-lead', 10, NULL, 30, 0), (4, 5, 'task_coder', 1, 's5', 'working', 'web-lead', 15, NULL, NULL, 2);
INSERT INTO worker_tasks (worker_process_id, task_id) VALUES (2, 1), (4, 2);
INSERT INTO worker_proposals (worker_process_id, proposal_id) VALUES (3, 1);
INSERT INTO worker_permission_questions (id, number, kind, worker_process_id, session_id, request_id, tool, input, created_at) VALUES (1, 1, 'tool', 2, 's2', 'r1', 'Bash', '{}', 0);
";

#[tokio::test]
async fn a_stored_task_coder_becomes_a_task_worker_and_keeps_its_rows() {
    use sqlx::Connection;
    let dir = tempfile::TempDir::new().unwrap();
    let paths = fold_core::ProjectPaths::new(dir.path().join("project"));
    std::fs::create_dir_all(paths.db().parent().unwrap()).unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(paths.db())
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let mut before: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/project");
    let rename = before
        .migrations
        .iter()
        .position(|migration| migration.description == "task workers")
        .unwrap();
    before.migrations = std::borrow::Cow::Owned(before.migrations[..rename].to_vec());
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(OLD_WORKERS).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let roles: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, role FROM worker_processes ORDER BY id")
            .fetch_all(store.read())
            .await
            .unwrap();
    assert_eq!(
        roles,
        [
            (1, "area_lead".to_owned()),
            (2, "task_worker".to_owned()),
            (3, "reviewer".to_owned()),
            (4, "task_worker".to_owned())
        ]
    );
    let mut read = store.read().acquire().await.unwrap();
    let row = agents::find_worker_process(&mut read, &AgentName::parse("web-coder").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.role, row.state, row.turns),
        (Role::TaskWorker, WorkerState::Stopped, 7)
    );
    let running = agents::find_worker_process(&mut read, &AgentName::parse("web-coder-2").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (running.role, running.state, running.turns),
        (Role::TaskWorker, WorkerState::Working, 2)
    );
    drop(read);
    assert_eq!(
        count(
            &store,
            "SELECT (SELECT COUNT(*) FROM worker_tasks WHERE worker_process_id IN (2, 4)) + (SELECT COUNT(*) FROM worker_proposals WHERE worker_process_id = 3) + (SELECT COUNT(*) FROM worker_permission_questions WHERE worker_process_id = 2)"
        )
        .await,
        4
    );
    let broken = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert!(broken.is_empty());

    exec(&store, "INSERT INTO agents (id, name, created_at) VALUES (9, 'late', 0), (10, 'review-lead', 0), (11, 'review-lead-2', 0)")
        .await
        .unwrap();
    let old_role = exec(
        &store,
        "INSERT INTO worker_processes (agent_id, role, area_id, session_id, state, started_by, started_at) VALUES (9, 'task_coder', 1, 's9', 'working', 'web-lead', 40)",
    )
    .await
    .unwrap_err();
    assert!(
        old_role
            .to_string()
            .contains("worker_processes_role_values"),
        "{old_role}"
    );
    let lead_binding = exec(
        &store,
        "INSERT INTO worker_tasks (worker_process_id, task_id) VALUES (1, 2)",
    )
    .await
    .unwrap_err();
    assert!(
        lead_binding
            .to_string()
            .contains("worker_tasks_task_worker_only")
    );
    exec(
        &store,
        "INSERT INTO worker_tasks (worker_process_id, task_id) VALUES (2, 2)",
    )
    .await
    .unwrap();
    let moved = exec(
        &store,
        "UPDATE worker_processes SET role = 'area_lead' WHERE id = 2",
    )
    .await
    .unwrap_err();
    assert!(moved.to_string().contains("worker_processes_fixed"));
    let deleted = exec(&store, "DELETE FROM worker_processes WHERE id = 3")
        .await
        .unwrap_err();
    assert!(deleted.to_string().contains("keeps every worker"));
    let lead = "INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (10, 'review_lead', 's10', 'working', 'main', 40)";
    exec(&store, lead).await.unwrap();
    let second_lead = exec(
        &store,
        &lead.replace("(10,", "(11,").replace("'s10'", "'s11'"),
    )
    .await
    .unwrap_err();
    assert!(second_lead.to_string().contains("UNIQUE"), "{second_lead}");
}

/// A reviewer stored with the role name `batch_reviewer`, bound to a proposal, beside a task worker.
const BATCH_REVIEWERS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-worker', 0), (2, 'batch-reviewer', 0), (3, 'author', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (3, 3, 'author', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (1, 'ui', 'sage', 0);
INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (1, 'web', 1, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 3, 'p', 0, 0), (2, 2, 2, 3, 'q', 0, 0);
INSERT INTO worker_processes (id, agent_id, role, area_id, session_id, state, started_by, started_at, stopped_at, idle_since, turns) VALUES (1, 1, 'task_worker', 1, 's1', 'stopped', 'web-lead', 10, 20, NULL, 3), (2, 2, 'batch_reviewer', NULL, 's2', 'idle', 'review-lead', 10, NULL, 30, 4);
INSERT INTO worker_proposals (worker_process_id, proposal_id) VALUES (2, 1);
";

#[tokio::test]
async fn a_stored_batch_reviewer_becomes_a_reviewer_and_keeps_its_rows() {
    use sqlx::Connection;
    let dir = tempfile::TempDir::new().unwrap();
    let paths = fold_core::ProjectPaths::new(dir.path().join("project"));
    std::fs::create_dir_all(paths.db().parent().unwrap()).unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(paths.db())
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let mut before: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/project");
    let rename = before
        .migrations
        .iter()
        .position(|migration| migration.description == "reviewer role")
        .unwrap();
    before.migrations = std::borrow::Cow::Owned(before.migrations[..rename].to_vec());
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(BATCH_REVIEWERS)
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let mut read = store.read().acquire().await.unwrap();
    let row = agents::find_worker_process(&mut read, &AgentName::parse("batch-reviewer").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.role, row.state, row.turns),
        (Role::Reviewer, WorkerState::Idle, 4)
    );
    assert_eq!(row.proposals.len(), 1);
    drop(read);
    let broken = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert!(broken.is_empty());

    exec(
        &store,
        "INSERT INTO agents (id, name, created_at) VALUES (9, 'late', 0)",
    )
    .await
    .unwrap();
    let old_role = exec(
        &store,
        "INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (9, 'batch_reviewer', 's9', 'working', 'review-lead', 40)",
    )
    .await
    .unwrap_err();
    assert!(
        old_role
            .to_string()
            .contains("worker_processes_role_values"),
        "{old_role}"
    );
    let task_binding = exec(
        &store,
        "INSERT INTO worker_proposals (worker_process_id, proposal_id) VALUES (1, 2)",
    )
    .await
    .unwrap_err();
    assert!(
        task_binding
            .to_string()
            .contains("worker_proposals_reviewer_only")
    );
    exec(
        &store,
        "INSERT INTO worker_proposals (worker_process_id, proposal_id) VALUES (2, 2)",
    )
    .await
    .unwrap();
    let moved = exec(
        &store,
        "UPDATE worker_processes SET role = 'task_worker' WHERE id = 2",
    )
    .await
    .unwrap_err();
    assert!(moved.to_string().contains("worker_processes_fixed"));
}

#[tokio::test]
async fn a_failed_turn_keeps_its_error_until_a_turn_ends_without_one() {
    let (_dir, store) = seeded().await;
    let lead = process(&store, 1, Role::ReviewLead, None, "s1").await;
    let row = async || {
        let mut conn = store.read().acquire().await.unwrap();
        agents::find_worker_process(&mut conn, &AgentName::parse("web-lead").unwrap())
            .await
            .unwrap()
            .unwrap()
    };
    let failed = async |error: &'static str, reached: bool, now: i64| {
        store
            .write(async |tx| agents::record_turn_error(tx, lead, error, reached, at(now)).await)
            .await
            .unwrap()
    };
    assert_eq!(
        (row().await.last_error, row().await.failed_turns),
        (None, 0)
    );
    assert_eq!(failed("Not signed in", false, 20).await, Some(1));
    assert_eq!(failed("Not signed in", false, 30).await, Some(2));
    let kept = row().await;
    assert_eq!(kept.last_error.as_deref(), Some("Not signed in"));
    assert_eq!((kept.last_error_at, kept.failed_turns), (Some(at(30)), 2));
    assert_eq!(
        failed("Reached max turns", true, 40).await,
        Some(0),
        "a turn that reached the model starts the count again"
    );
    assert_eq!(row().await.last_error.as_deref(), Some("Reached max turns"));
    store
        .write(async |tx| agents::clear_turn_error(tx, lead).await)
        .await
        .unwrap();
    let cleared = row().await;
    assert_eq!(
        (
            cleared.last_error,
            cleared.last_error_at,
            cleared.failed_turns
        ),
        (None, None, 0)
    );
    store
        .write(async |tx| agents::set_worker_state(tx, lead, WorkerState::Stopping, at(50)).await)
        .await
        .unwrap();
    assert_eq!(
        failed("interrupted", false, 60).await,
        None,
        "the interrupt of a stop counts no failed turn"
    );
    store
        .write(async |tx| agents::set_exit_error(tx, lead, "fatal: no access", at(70)).await)
        .await
        .unwrap();
    let exited = row().await;
    assert_eq!(exited.last_error.as_deref(), Some("fatal: no access"));
    assert_eq!(exited.last_error_at, Some(at(70)));
    for change in [
        format!("UPDATE worker_processes SET last_error = '' WHERE id = {lead}"),
        format!(
            "UPDATE worker_processes SET last_error = '{}' WHERE id = {lead}",
            "a".repeat(1001)
        ),
        format!("UPDATE worker_processes SET last_error_at = NULL WHERE id = {lead}"),
        format!("UPDATE worker_processes SET failed_turns = -1 WHERE id = {lead}"),
    ] {
        assert!(exec(&store, &change).await.is_err(), "{change}");
    }
}

#[tokio::test]
async fn a_made_up_model_is_forgotten_and_a_real_one_kept() {
    let (_dir, store) = seeded().await;
    let lead = process(&store, 1, Role::ReviewLead, None, "s1").await;
    let model = async |set: &str| {
        exec(
            &store,
            &format!("UPDATE worker_processes SET model = {set} WHERE id = {lead}"),
        )
        .await
        .unwrap();
        store
            .write(async |tx| agents::forget_made_up_model(tx, lead).await)
            .await
            .unwrap();
        let mut conn = store.read().acquire().await.unwrap();
        agents::find_worker_process(&mut conn, &AgentName::parse("web-lead").unwrap())
            .await
            .unwrap()
            .unwrap()
            .model
    };
    assert_eq!(model("'<synthetic>'").await, None);
    assert_eq!(model("'opus'").await.as_deref(), Some("opus"));
    assert_eq!(model("NULL").await, None);
}

/// A project from before the area main: a plain label `main` on an open task, a Done task and a task with a mark of its own.
const PLAIN_MAIN_LABEL: &str = "
INSERT INTO labels (id, name, color, created_at) VALUES (1, 'ui', 'sage', 0), (2, 'main', 'gold', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at, status) VALUES (1, 1, 'notes', 'fold', 0, 0, 'todo'), (2, 2, 'old', 'fold', 0, 0, 'done'), (3, 3, 'checks', 'fold', 0, 0, 'todo'), (4, 4, 'ui', 'fold', 0, 0, 'todo');
UPDATE tasks SET human_review = 1, human_review_reason = 'billing', human_review_set_by = 'human:ana' WHERE id = 3;
INSERT INTO task_labels (task_id, label_id) VALUES (1, 2), (2, 2), (3, 2), (4, 1);
";

/// A project folder whose database has every migration before the area main and then `seed`.
#[cfg(test)]
async fn before_main_area(seed: &'static str) -> (tempfile::TempDir, fold_core::ProjectPaths) {
    use sqlx::Connection;
    let dir = tempfile::TempDir::new().unwrap();
    let paths = fold_core::ProjectPaths::new(dir.path().join("project"));
    std::fs::create_dir_all(paths.db().parent().unwrap()).unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(paths.db())
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let mut before: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/project");
    let area = before
        .migrations
        .iter()
        .position(|migration| migration.description == "main area")
        .unwrap();
    before.migrations = std::borrow::Cow::Owned(before.migrations[..area].to_vec());
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(seed).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
    (dir, paths)
}

#[tokio::test]
async fn a_plain_label_main_becomes_the_label_of_the_area_and_its_open_tasks_get_the_mark() {
    let (_dir, paths) = before_main_area(PLAIN_MAIN_LABEL).await;
    let store = Store::open_project(&paths).await.unwrap();
    let main = agents::find_area(store.read(), &fold_core::AreaName::main())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(main.label, fold_core::LabelName::main());
    assert_eq!(count(&store, "SELECT COUNT(*) FROM labels").await, 2);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM labels WHERE name = 'main' AND color = 'gold'"
        )
        .await,
        1
    );
    let marks: Vec<(i64, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT number, human_review, human_review_reason, human_review_set_by FROM tasks ORDER BY number",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    let reason = Some(fold_core::policy::MAIN_AREA_REASON.to_owned());
    let fold = Some("fold".to_owned());
    assert_eq!(
        marks,
        [
            (1, 1, reason, fold),
            (2, 0, None, None),
            (
                3,
                1,
                Some("billing".to_owned()),
                Some("human:ana".to_owned())
            ),
            (4, 0, None, None),
        ]
    );
}

#[tokio::test]
async fn an_area_of_the_project_that_holds_the_name_or_label_main_stops_the_migration() {
    let cases = [
        (
            "INSERT INTO labels (id, name, color, created_at) VALUES (1, 'main', 'gold', 0);
             INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('core', 1, 'fold', 0);",
            "Rename the label main",
        ),
        (
            "INSERT INTO labels (id, name, color, created_at) VALUES (1, 'core', 'gold', 0);
             INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('main', 1, 'fold', 0);",
            "define that area under another name",
        ),
        (
            "INSERT INTO labels (id, name, color, created_at) VALUES (1, 'main', 'gold', 0);
             INSERT INTO areas (name, label_id, created_by, created_at, model) VALUES ('main', 1, 'fold', 0, 'opus');",
            "a lead or a profile",
        ),
    ];
    for (seed, fix) in cases {
        let (_dir, paths) = before_main_area(seed).await;
        let error = Store::open_project(&paths).await.unwrap_err();
        let shown = format!("{error:?}");
        assert!(shown.contains("main_area_conflict"), "{shown}");
        assert!(shown.contains(fix), "{shown}");
    }
    let (_dir, paths) = before_main_area(
        "INSERT INTO labels (id, name, color, created_at) VALUES (1, 'main', 'gold', 0);
         INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('main', 1, 'fold', 0);",
    )
    .await;
    let store = Store::open_project(&paths).await.unwrap();
    let main = agents::find_area(store.read(), &fold_core::AreaName::main())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(main.label, fold_core::LabelName::main());
    assert_eq!(count(&store, "SELECT COUNT(*) FROM areas").await, 1);
}

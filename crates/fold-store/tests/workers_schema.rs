use fold_core::roles::AreaProfile;
use fold_core::{AgentName, ProposalId, Role, TaskId, Timestamp, WorkerState};
use fold_store::Store;
use fold_store::agents::{self, NewWorkerProcess, TurnTotals};

use crate::common::{count, exec, open};

const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-lead', 0), (2, 'web-coder', 0), (3, 'api-lead', 0), (4, 'review-lead', 0), (5, 'author', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (5, 5, 'author', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (1, 'ui', 'sage', 0), (2, 'server', 'sage', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0);
INSERT INTO task_labels (task_id, label_id) VALUES (1, 1);
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
        "INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('site', 1, 'main', 0)",
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
    assert_eq!(agents::list_areas(store.read()).await.unwrap(), vec![row]);
    exec(
        &store,
        "INSERT INTO areas (name, label_id, created_by, created_at) VALUES ('api', 2, 'main', 0)",
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
async fn an_area_profile_changes_while_its_lead_stays_set_once() {
    let (_dir, store) = seeded().await;
    let web = fold_core::AreaName::parse("web").unwrap();
    let area = store
        .write(async |tx| agents::insert_area(tx, &web, 1, "main", at(1)).await)
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
    assert_eq!(agents::list_areas(store.read()).await.unwrap(), vec![row]);

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

    let cleared = exec(&store, "UPDATE areas SET lead_agent_id = NULL WHERE id = 1")
        .await
        .unwrap_err();
    assert!(cleared.to_string().contains("areas_lead_once"), "{cleared}");
    let renamed = exec(&store, "UPDATE areas SET name = 'site' WHERE id = 1")
        .await
        .unwrap_err();
    assert!(renamed.to_string().contains("areas_lead_once"), "{renamed}");
    let twice = exec(
        &store,
        "INSERT INTO area_mcp_servers (area_id, position, name) VALUES (1, 5, 'fold')",
    )
    .await
    .unwrap_err();
    assert_eq!(
        twice.constraint().as_deref(),
        Some("area_mcp_servers.area_id, area_mcp_servers.name")
    );
    let blank = exec(&store, "UPDATE areas SET instructions = '  ' WHERE id = 1")
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
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (1, 'web', 1, 'main', 0)",
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
    let lead = process(&store, 1, Role::AreaLead, Some(1), "s1").await;
    let coder = process(&store, 2, Role::TaskCoder, Some(1), "s2").await;
    let reviewer = process(&store, 4, Role::BatchReviewer, None, "s4").await;
    let twice = exec(
        &store,
        "INSERT INTO worker_processes (agent_id, role, area_id, session_id, state, started_by, started_at) VALUES (1, 'area_lead', 1, 's9', 'starting', 'main', 0)",
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
            assert!(agents::bind_task(tx, coder, task).await?);
            assert!(
                agents::bind_task(tx, coder, task).await?,
                "a task named twice counts as bound"
            );
            assert!(!agents::bind_task(tx, coder, TaskId::from_number(9).unwrap()).await?);
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
    assert!(wrong_kind.to_string().contains("worker_tasks_coder_only"));
    let wrong_kind = store
        .write(async |tx| agents::bind_proposal(tx, coder, proposal).await)
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
        agents::coders_of_task(store.read(), task).await.unwrap(),
        [agents::BoundWorker {
            id: coder,
            agent: AgentName::parse("web-coder").unwrap()
        }]
    );
    assert_eq!(
        agents::tasks_awaiting_proposal(store.read(), &[coder])
            .await
            .unwrap()[&coder],
        1
    );
    exec(
        &store,
        "INSERT INTO proposals (id, number, task_id, author_worker_id, title, state, created_at, updated_at) VALUES (2, 4, 1, 5, 'q', 'integrated', 0, 0)",
    )
    .await
    .unwrap();
    assert_eq!(
        agents::tasks_awaiting_proposal(store.read(), &[coder])
            .await
            .unwrap()[&coder],
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
        agents::tasks_awaiting_proposal(store.read(), &[coder])
            .await
            .unwrap()[&coder],
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
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (1, 'web', 1, 'main', 0)",
    )
    .await
    .unwrap();
    let id = process(&store, 1, Role::AreaLead, Some(1), "s1").await;
    let web = Some(1);
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
        agents::running_counts(store.read(), Role::TaskCoder, web)
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
    store
        .write(async |tx| {
            agents::set_worker_state(tx, id, WorkerState::Stopping, at(30)).await?;
            assert!(!agents::mark_worker_working(tx, id).await?);
            agents::record_exit(tx, id, WorkerState::Stopped, Some(0), at(31)).await?;
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
    process(&store, 5, Role::BatchReviewer, None, "s3").await;
}

#[tokio::test]
async fn a_new_session_comes_only_with_a_new_start() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (1, 'web', 1, 'main', 0)",
    )
    .await
    .unwrap();
    let coder = process(&store, 2, Role::TaskCoder, Some(1), "s2").await;
    store
        .write(async |tx| agents::set_worker_state(tx, coder, WorkerState::Working, at(15)).await)
        .await
        .unwrap();
    for change in [
        format!("UPDATE worker_processes SET session_id = 's3' WHERE id = {coder}"),
        format!(
            "UPDATE worker_processes SET session_id = 's3', state = 'idle', idle_since = 20 WHERE id = {coder}"
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
            "UPDATE worker_processes SET session_id = 's2b', state = 'starting' WHERE id = {coder}"
        ),
    )
    .await
    .expect("a running row moves to starting with a new session");
    store
        .write(async |tx| {
            agents::record_exit(tx, coder, WorkerState::Failed, Some(1), at(30)).await?;
            agents::refresh_session(tx, coder, "s3", at(40)).await
        })
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let rows = agents::list_worker_processes(&mut conn).await.unwrap();
    assert_eq!(rows[0].session_id, "s3");
    assert_eq!(rows[0].state, WorkerState::Starting);
    assert_eq!(rows[0].started_at, at(40));
    assert_eq!((rows[0].stopped_at, rows[0].exit_code), (None, None));
    for column in ["agent_id = 3", "started_by = 'main'", "role = 'area_lead'"] {
        let change = format!("UPDATE worker_processes SET {column} WHERE id = {coder}");
        let refused = exec(&store, &change).await.unwrap_err();
        assert!(
            refused.to_string().contains("worker_processes_fixed"),
            "{change}"
        );
    }
}

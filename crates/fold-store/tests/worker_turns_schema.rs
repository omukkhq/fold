use fold_core::cost::{LONG_SLOT_MILLIS, SHORT_SLOT_MILLIS as SLOT_MILLIS};
use fold_core::{AgentName, ProposalId, Role, TaskId, Timestamp, WorkerState};
use fold_store::Store;
use fold_store::agents::{self, CostSlot, CostTotals, NewWorkerProcess, TurnTotals};
use fold_store::tasks;

use crate::common::{count, exec, open};

const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'web-lead', 0), (2, 'web-worker', 0), (3, 'reviewer-1', 0), (4, 'author', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (2, 2, 'web-worker', 0), (3, 3, 'reviewer-1', 0), (4, 4, 'author', 0);
INSERT INTO labels (id, name, color, created_at) VALUES (11, 'ui', 'sage', 0);
INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (21, 'web', 11, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0), (3, 3, 'three', 'fold', 0, 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 4, 2, 4, 'p', 0, 0), (2, 5, 3, 4, 'q', 0, 0);
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
async fn process(store: &Store, agent_id: i64, role: Role, area_id: Option<i64>) -> i64 {
    let session = format!("s-{agent_id}");
    store
        .write(async |tx| {
            agents::insert_worker_process(
                tx,
                &NewWorkerProcess {
                    agent_id,
                    role,
                    area_id,
                    model: Some("sonnet"),
                    session_id: &session,
                    started_by: "human:ana",
                    now: at(10),
                },
            )
            .await
        })
        .await
        .unwrap()
}

/// Ends a turn of row `id` whose process reports the session's running total `total`.
#[cfg(test)]
async fn turn(store: &Store, id: i64, total: u64, millis: i64) {
    store
        .write(async |tx| {
            agents::record_turn(
                tx,
                id,
                TurnTotals {
                    reported_micro_usd: total,
                    input_tokens: 3,
                    output_tokens: 4,
                    cache_read_tokens: 5,
                    cache_write_tokens: 6,
                    failed: false,
                },
                at(millis),
            )
            .await
        })
        .await
        .unwrap();
}

/// The subject of each stored turn of row `id` as `(task number, proposal number)`, oldest first.
#[cfg(test)]
async fn subjects(store: &Store, id: i64) -> Vec<(Option<i64>, Option<i64>)> {
    sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
        "SELECT t.number, p.number FROM worker_turns x
         LEFT JOIN tasks t ON t.id = x.task_id LEFT JOIN proposals p ON p.id = x.proposal_id
         WHERE x.worker_process_id = ? ORDER BY x.id",
    )
    .bind(id)
    .fetch_all(store.read())
    .await
    .unwrap()
}

#[tokio::test]
async fn each_turn_appends_the_cost_it_added_and_the_row_keeps_its_totals() {
    let (_dir, store) = seeded().await;
    let lead = process(&store, 1, Role::AreaLead, Some(21)).await;
    turn(&store, lead, 300, 1_000).await;
    turn(&store, lead, 450, 2_000).await;
    turn(&store, lead, 450, 3_000).await;
    // A process whose total starts again from nothing counts all of it for the turn.
    turn(&store, lead, 100, 4_000).await;
    // A result without a count, such as an interrupt's answer, keeps the session's total.
    turn(&store, lead, 0, 4_500).await;
    turn(&store, lead, 130, 4_600).await;
    // A new session counts from nothing.
    store
        .write(async |tx| agents::refresh_session(tx, lead, "s-2", at(4_700)).await)
        .await
        .unwrap();
    turn(&store, lead, 80, 5_000).await;
    let turns = sqlx::query_as::<_, (i64, String, Option<i64>, Option<String>, i64, i64, i64, i64, i64, i64)>(
        "SELECT cost_micro_usd, role, area_id, model, input_tokens, output_tokens, cache_read_tokens,
                cache_write_tokens, failed, created_at
         FROM worker_turns WHERE worker_process_id = ? ORDER BY id",
    )
    .bind(lead)
    .fetch_all(store.read())
    .await
    .unwrap();
    let costs: Vec<(i64, i64)> = turns.iter().map(|row| (row.0, row.9)).collect();
    assert_eq!(
        costs,
        [
            (300, 1_000),
            (150, 2_000),
            (0, 3_000),
            (100, 4_000),
            (0, 4_500),
            (30, 4_600),
            (80, 5_000)
        ]
    );
    let first = &turns[0];
    assert_eq!(
        (first.1.as_str(), first.2, first.3.as_deref()),
        ("area_lead", Some(21), Some("sonnet"))
    );
    assert_eq!(
        (first.4, first.5, first.6, first.7, first.8),
        (3, 4, 5, 6, 0)
    );
    assert_eq!(subjects(&store, lead).await, [(None, None); 7]);
    let mut conn = store.read().acquire().await.unwrap();
    let row = agents::find_worker_process(&mut conn, &AgentName::parse("web-lead").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.turns, row.cost_micro_usd), (7, 660));
    let session: i64 =
        sqlx::query_scalar("SELECT session_cost_micro_usd FROM worker_processes WHERE id = ?")
            .bind(lead)
            .fetch_one(store.read())
            .await
            .unwrap();
    assert_eq!(session, 80);
}

#[tokio::test]
async fn a_task_workers_turn_counts_for_its_claimed_or_its_only_task() {
    let (_dir, store) = seeded().await;
    let worker = process(&store, 2, Role::TaskWorker, Some(21)).await;
    turn(&store, worker, 10, 1_000).await;
    store
        .write(async |tx| {
            agents::bind_task(tx, worker, TaskId::from_number(1).unwrap()).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    turn(&store, worker, 20, 2_000).await;
    store
        .write(async |tx| {
            agents::bind_task(tx, worker, TaskId::from_number(2).unwrap()).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    // Two bound tasks and no claim name no task.
    turn(&store, worker, 30, 3_000).await;
    exec(
        &store,
        "INSERT INTO claims (id, kind, task_id, worker_id, acquired_at, renewed_at) VALUES (1, 'implementation', 2, 2, 0, 3500)",
    )
    .await
    .unwrap();
    turn(&store, worker, 40, 4_000).await;
    exec(
        &store,
        "UPDATE claims SET released_at = 4500, release_reason = 'released' WHERE id = 1",
    )
    .await
    .unwrap();
    // The claim it held last still names the task it worked on.
    turn(&store, worker, 50, 5_000).await;
    assert_eq!(
        subjects(&store, worker).await,
        [
            (None, None),
            (Some(1), None),
            (None, None),
            (Some(2), None),
            (Some(2), None)
        ]
    );
    let history = store
        .write(async |tx| tasks::history(tx.conn(), TaskId::from_number(1).unwrap()).await)
        .await
        .unwrap();
    assert!(history.turn && history.any(), "{history:?}");
    let untouched = store
        .write(async |tx| tasks::history(tx.conn(), TaskId::from_number(3).unwrap()).await)
        .await
        .unwrap();
    assert!(
        !untouched.turn,
        "a proposal of T-3 is history, but no turn counts for it"
    );
}

#[tokio::test]
async fn a_reviewers_turn_counts_for_its_claimed_or_its_only_proposal() {
    let (_dir, store) = seeded().await;
    let reviewer = process(&store, 3, Role::Reviewer, None).await;
    store
        .write(async |tx| {
            agents::bind_proposal(tx, reviewer, ProposalId::from_number(4).unwrap()).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    turn(&store, reviewer, 10, 1_000).await;
    store
        .write(async |tx| {
            agents::bind_proposal(tx, reviewer, ProposalId::from_number(5).unwrap()).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    exec(
        &store,
        "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 2, 3, 0, 0)",
    )
    .await
    .unwrap();
    turn(&store, reviewer, 25, 2_000).await;
    assert_eq!(
        subjects(&store, reviewer).await,
        [(None, Some(4)), (None, Some(5))]
    );
}

#[tokio::test]
async fn the_cost_groups_slots_and_totals_count_every_worker_in_its_range() {
    let (_dir, store) = seeded().await;
    let lead = process(&store, 1, Role::AreaLead, Some(21)).await;
    let worker = process(&store, 2, Role::TaskWorker, Some(21)).await;
    let reviewer = process(&store, 3, Role::Reviewer, None).await;
    store
        .write(async |tx| {
            agents::bind_task(tx, worker, TaskId::from_number(2).unwrap()).await?;
            agents::bind_proposal(tx, reviewer, ProposalId::from_number(4).unwrap()).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    turn(&store, lead, 100, 1_000).await;
    turn(&store, worker, 200, SLOT_MILLIS + 5).await;
    turn(&store, worker, 260, SLOT_MILLIS + 9).await;
    turn(&store, reviewer, 40, 3 * SLOT_MILLIS).await;
    // A stopped worker keeps its turns.
    store
        .write(async |tx| {
            agents::set_worker_state(tx, worker, WorkerState::Stopped, at(4 * SLOT_MILLIS)).await
        })
        .await
        .unwrap();

    let all = agents::cost_groups(store.read(), None, None).await.unwrap();
    let summary: Vec<(&str, u64, u64, Option<String>)> = all
        .iter()
        .map(|group| {
            (
                group.agent.as_str(),
                group.turns,
                group.cost_micro_usd,
                group
                    .subject
                    .as_ref()
                    .map(|(task, title)| format!("{task} {title}")),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("web-lead", 1, 100, None),
            ("web-worker", 2, 260, Some("T-2 two".to_owned())),
            ("reviewer-1", 1, 40, Some("T-2 two".to_owned())),
        ]
    );
    assert_eq!(all[1].state, WorkerState::Stopped);
    assert_eq!(
        all[1].tokens,
        fold_core::cost::Tokens {
            input: 6,
            output: 8,
            cache_read: 10,
            cache_write: 12
        },
        "each turn's own usage, summed"
    );
    assert_eq!(all[1].task, TaskId::from_number(2));
    assert_eq!(all[2].proposal, ProposalId::from_number(4));
    assert_eq!(
        (all[1].first_at, all[1].last_at),
        (at(SLOT_MILLIS + 5), at(SLOT_MILLIS + 9))
    );

    let recent = agents::cost_groups(store.read(), Some(at(SLOT_MILLIS)), None)
        .await
        .unwrap();
    assert_eq!(recent.len(), 2);
    let of_task = agents::cost_groups(store.read(), None, TaskId::from_number(2))
        .await
        .unwrap();
    assert_eq!(of_task.len(), 2, "the build and the review of T-2");
    assert!(
        agents::cost_groups(store.read(), None, TaskId::from_number(1))
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        agents::cost_slots(store.read(), None, SLOT_MILLIS)
            .await
            .unwrap(),
        [
            CostSlot {
                start: at(0),
                role: Role::AreaLead,
                cost_micro_usd: 100,
                tokens: 18
            },
            CostSlot {
                start: at(SLOT_MILLIS),
                role: Role::TaskWorker,
                cost_micro_usd: 260,
                tokens: 36
            },
            CostSlot {
                start: at(3 * SLOT_MILLIS),
                role: Role::Reviewer,
                cost_micro_usd: 40,
                tokens: 18
            },
        ]
    );
    let slot = |role: Role, cost_micro_usd: u64, tokens: u64| CostSlot {
        start: at(0),
        role,
        cost_micro_usd,
        tokens,
    };
    assert_eq!(
        agents::cost_slots(store.read(), None, LONG_SLOT_MILLIS)
            .await
            .unwrap(),
        [
            slot(Role::AreaLead, 100, 18),
            slot(Role::Reviewer, 40, 18),
            slot(Role::TaskWorker, 260, 36),
        ],
        "an hour slot holds the first three quarter hours"
    );
    assert_eq!(
        agents::cost_totals(store.read(), at(SLOT_MILLIS), at(3 * SLOT_MILLIS))
            .await
            .unwrap(),
        CostTotals {
            all_time: 400,
            today: 300,
            week: 40,
            all_time_tokens: 72
        }
    );
}

#[tokio::test]
async fn a_turn_is_append_only_and_carries_its_workers_role_and_area() {
    let (_dir, store) = seeded().await;
    let lead = process(&store, 1, Role::AreaLead, Some(21)).await;
    turn(&store, lead, 5, 1_000).await;
    let insert = |role: &str, area: &str| {
        format!(
            "INSERT INTO worker_turns (worker_process_id, role, area_id, cost_micro_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, failed, interrupted, created_at) VALUES ({lead}, '{role}', {area}, 0, 0, 0, 0, 0, 0, 0, 0)"
        )
    };
    for (role, area) in [("reviewer", "21"), ("area_lead", "NULL")] {
        let error = exec(&store, &insert(role, area)).await.unwrap_err();
        assert!(
            error.to_string().contains("worker_turns_of_worker"),
            "{error}"
        );
    }
    exec(&store, &insert("area_lead", "21")).await.unwrap();
    let both = exec(
        &store,
        &format!(
            "INSERT INTO worker_turns (worker_process_id, role, area_id, task_id, proposal_id, cost_micro_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, failed, interrupted, created_at) VALUES ({lead}, 'area_lead', 21, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0)"
        ),
    )
    .await
    .unwrap_err();
    assert!(both.to_string().contains("CHECK"), "{both}");
    for sql in [
        "UPDATE worker_turns SET cost_micro_usd = 9",
        "DELETE FROM worker_turns",
    ] {
        let error = exec(&store, sql).await.unwrap_err();
        assert!(
            error.to_string().contains("worker_turns is append-only"),
            "{error}"
        );
    }
    assert_eq!(count(&store, "SELECT COUNT(*) FROM worker_turns").await, 2);
}

#[tokio::test]
async fn a_stops_interrupt_keeps_its_cost_but_is_not_a_turn() {
    let (_dir, store) = seeded().await;
    let lead = process(&store, 1, Role::AreaLead, Some(21)).await;
    let failed = |reported: u64| TurnTotals {
        reported_micro_usd: reported,
        input_tokens: 7,
        failed: true,
        ..TurnTotals::default()
    };
    turn(&store, lead, 100, 1_000).await;
    // A failed turn of a running worker is one of its turns.
    store
        .write(async |tx| agents::record_turn(tx, lead, failed(130), at(2_000)).await)
        .await
        .unwrap();
    store
        .write(async |tx| {
            agents::set_worker_state(tx, lead, WorkerState::Stopping, at(2_500)).await?;
            agents::record_turn(tx, lead, failed(150), at(3_000)).await
        })
        .await
        .unwrap();
    let flags = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT cost_micro_usd, failed, interrupted FROM worker_turns WHERE worker_process_id = ? ORDER BY id",
    )
    .bind(lead)
    .fetch_all(store.read())
    .await
    .unwrap();
    assert_eq!(flags, [(100, 0, 0), (30, 1, 0), (20, 1, 1)]);
    let groups = agents::cost_groups(store.read(), None, None).await.unwrap();
    assert_eq!(
        (groups[0].turns, groups[0].cost_micro_usd),
        (2, 150),
        "the interrupt's cost counts, its turn does not"
    );
    assert_eq!(
        groups[0].tokens.input, 10,
        "a failed turn adds its tokens; an interrupt adds none, as it adds no turn"
    );
    let refused = exec(
        &store,
        &format!(
            "INSERT INTO worker_turns (worker_process_id, role, area_id, cost_micro_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, failed, interrupted, created_at) VALUES ({lead}, 'area_lead', 21, 0, 0, 0, 0, 0, 0, 1, 0)"
        ),
    )
    .await;
    assert!(refused.is_err(), "only a failed turn is an interrupt");
}

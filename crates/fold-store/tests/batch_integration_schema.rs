use std::path::Path;

use crate::common::{count, exec, open};
use fold_core::object::ObjectHash;
use fold_core::reviews::batches::BatchOutcome;
use fold_core::reviews::checks::{Purpose, RunState, Stage};
use fold_core::{
    CheckRunId, ProjectPaths, ProposalId, ReviewBatchId, RevisionId, TaskId, Timestamp,
};
use fold_store::Store;
use fold_store::reviews::batches;
use fold_store::reviews::checks::{self, NewCheckRun};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

/// A batch submitted before batches integrated together had its members integrate on their own, so the migration ends its turn; an open or closed batch keeps none.
#[tokio::test]
async fn the_migration_ends_the_turn_of_every_batch_submitted_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    std::fs::create_dir(&before).unwrap();
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"));
    for entry in std::fs::read_dir(source).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: u32 = name.split('_').next().unwrap().parse().unwrap();
        if name.ends_with(".sql") && version < 60 {
            std::fs::copy(source.join(&name), before.join(&name)).unwrap();
        }
    }
    let paths = ProjectPaths::new(dir.path().join("project"));
    std::fs::create_dir_all(paths.db().parent().unwrap()).unwrap();
    let options = SqliteConnectOptions::new()
        .filename(paths.db())
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::migrate::Migrator::new(before.as_path())
        .await
        .unwrap()
        .run(&mut conn)
        .await
        .unwrap();
    for sql in [
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'review', 0)",
        "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'review', 0)",
        "INSERT INTO review_batches (id, number, holder_worker_id, state, created_at, submitted_at, closed_at) VALUES (1, 1, 1, 'open', 0, NULL, NULL), (2, 2, 1, 'submitted', 0, 5, NULL), (3, 3, 1, 'closed', 0, NULL, 6), (4, 4, 1, 'submitted', 0, 7, NULL)",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let ended: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT b.number, i.outcome, i.created_at FROM review_batch_integrations i
         JOIN review_batches b ON b.id = i.batch_id ORDER BY b.number",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    assert_eq!(
        ended,
        [(2, "none".to_owned(), 5), (4, "none".to_owned(), 7)],
        "each submitted batch ends at its submission"
    );
    assert!(
        batches::undecided_batches(store.read())
            .await
            .unwrap()
            .is_empty()
    );
}

/// An open batch rb-1 and a submitted batch rb-2, each with one member, and a request-changes review of P-1 v1.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'review', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'review', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'review', 0, 0), (2, 2, 'two', 'review', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'review', 0), (2, 2, 2, 2, 1, 't3', 'm', 'review', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0), (2, 2, 2, 1, 'q', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'review', 0), (2, 2, 1, 1, 't3', 2, 'review', 0);
INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at) VALUES (1, 1, 1, 'request_changes', 'b', 0);
INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (1, 1, 1, 0), (2, 2, 1, 0);
INSERT INTO review_batch_members (id, batch_id, proposal_id, version_id, position) VALUES (1, 1, 1, 1, 1), (2, 2, 2, 2, 1);
UPDATE review_batches SET state = 'submitted', submitted_at = 5 WHERE id = 2;
";

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
async fn refused(store: &Store, sql: &str) -> String {
    exec(store, sql).await.unwrap_err().to_string()
}

#[cfg(test)]
fn rb(number: u32) -> ReviewBatchId {
    ReviewBatchId::from_number(number).unwrap()
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[tokio::test]
async fn a_batch_ends_its_integration_once_and_only_after_it_was_submitted() {
    let (_dir, store) = seeded().await;
    assert_eq!(
        batches::undecided_batches(store.read()).await.unwrap(),
        [rb(2)]
    );
    assert!(
        refused(
            &store,
            "INSERT INTO review_batch_integrations (batch_id, outcome, created_at) VALUES (1, 'none', 6)"
        )
        .await
        .contains("review_batch_integrations_submitted")
    );
    assert!(
        refused(
            &store,
            "INSERT INTO review_batch_integrations (batch_id, outcome, created_at) VALUES (2, 'one_at_a_time', 6)"
        )
        .await
        .contains("review_batch_integrations_run_of_fallback")
    );
    let ended = store
        .write(async |tx| {
            batches::end_batch_integration(tx, rb(2), BatchOutcome::Nothing, None, at(6)).await
        })
        .await
        .unwrap();
    assert!(ended);
    let again = store
        .write(async |tx| {
            batches::end_batch_integration(tx, rb(2), BatchOutcome::Together, None, at(7)).await
        })
        .await
        .unwrap();
    assert!(!again, "a turn ends once");
    let row = batches::batch_integration(store.read(), rb(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.outcome, BatchOutcome::Nothing);
    assert!(
        batches::undecided_batches(store.read())
            .await
            .unwrap()
            .is_empty()
    );
    for sql in [
        "UPDATE review_batch_integrations SET outcome = 'together'",
        "DELETE FROM review_batch_integrations",
    ] {
        assert!(refused(&store, sql).await.contains("append-only"), "{sql}");
    }
}

#[tokio::test]
async fn only_a_succeeded_proposal_integration_names_a_submitted_batch() {
    let (_dir, store) = seeded().await;
    for (sql, why) in [
        (
            "INSERT INTO integrations (number, kind, proposal_id, version_id, result, actor, created_at, review_batch_id) VALUES (1, 'proposal', 1, 1, 'succeeded', 'fold', 0, 1)",
            "an open batch",
        ),
        (
            "INSERT INTO integrations (number, kind, proposal_id, version_id, result, actor, created_at, review_batch_id) VALUES (1, 'proposal', 2, 2, 'conflicted', 'fold', 0, 2)",
            "a conflicted integration",
        ),
    ] {
        assert!(
            refused(&store, sql)
                .await
                .contains("integrations_review_batch_shape"),
            "{why}"
        );
    }
    assert!(
        batches::integrated_as(store.read(), ProposalId::from_number(2).unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn changes_requested_count_from_the_version_a_member_joined_at() {
    let (_dir, store) = seeded().await;
    let p1 = ProposalId::from_number(1).unwrap();
    assert!(
        batches::changes_requested_since(store.read(), p1, 1)
            .await
            .unwrap()
    );
    assert!(
        !batches::changes_requested_since(store.read(), p1, 2)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_batch_has_one_waiting_request_and_a_request_ends_once() {
    let (_dir, store) = seeded().await;
    let id = store
        .write(async |tx| batches::insert_batch_request(tx, rb(2), "human:ana", at(6)).await)
        .await
        .unwrap();
    assert!(
        batches::batch_request_waiting(store.read(), rb(2))
            .await
            .unwrap()
    );
    let second = store
        .write(async |tx| batches::insert_batch_request(tx, rb(2), "human:ana", at(7)).await)
        .await
        .unwrap_err();
    assert!(second.unique_violation(), "{second}");
    let waiting = batches::waiting_batch_requests(store.read()).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].batch, rb(2));
    assert_eq!(waiting[0].actor, "human:ana");
    let ended = store
        .write(async |tx| batches::end_batch_request(tx, id, at(8)).await)
        .await
        .unwrap();
    assert!(ended);
    let again = store
        .write(async |tx| batches::end_batch_request(tx, id, at(9)).await)
        .await
        .unwrap();
    assert!(!again);
    assert!(
        refused(&store, "UPDATE review_batch_requests SET actor = 'fold'")
            .await
            .contains("review_batch_requests_end_once")
    );
    assert!(
        refused(&store, "DELETE FROM review_batch_requests")
            .await
            .contains("append-only")
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM review_batch_requests WHERE ended_at IS NULL"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn a_batch_run_names_its_batch_and_shows_on_each_member_version() {
    let (_dir, store) = seeded().await;
    let p1 = ProposalId::from_number(1).unwrap();
    let p2 = ProposalId::from_number(2).unwrap();
    let run = |number: u32, batch: Option<ReviewBatchId>| NewCheckRun {
        id: CheckRunId::from_number(number).unwrap(),
        stage: Stage::Integration,
        purpose: Purpose::Integrate,
        task: TaskId::from_number(2).unwrap(),
        workspace: None,
        worker_id: None,
        proposal: Some(p2),
        version: Some(1),
        base: RevisionId::from_number(1).unwrap(),
        tree: ObjectHash::from_bytes([9; 32]),
        actor: "fold",
        title: None,
        evidence: None,
        batch,
    };
    let batched = run(1, Some(rb(2)));
    store
        .write(async |tx| {
            checks::insert_run(tx, &batched, at(6)).await?;
            checks::insert_run_members(tx, batched.id, &[1, 2]).await
        })
        .await
        .unwrap();
    let shown = checks::runs_of_version(store.read(), TaskId::from_number(1).unwrap(), p1, 1)
        .await
        .unwrap();
    assert_eq!(shown.len(), 1, "P-1 v1 lists the run stored on P-2");
    assert_eq!(shown[0].batch, Some(rb(2)));
    assert!(
        refused(
            &store,
            "UPDATE check_runs SET review_batch_id = NULL WHERE number = 1"
        )
        .await
        .contains("check_runs_review_batch_fixed")
    );
    store
        .write(async |tx| checks::finish_run(tx, batched.id, RunState::Passed, None, at(7)).await)
        .await
        .unwrap();
    let alone = run(2, None);
    store
        .write(async |tx| checks::insert_run(tx, &alone, at(8)).await)
        .await
        .unwrap();
    assert!(
        refused(
            &store,
            "INSERT INTO check_run_members (run_id, version_id) SELECT id, 1 FROM check_runs WHERE number = 2"
        )
        .await
        .contains("check_run_members_of_batch")
    );
    for sql in [
        "UPDATE check_run_members SET version_id = 2",
        "DELETE FROM check_run_members",
    ] {
        assert!(refused(&store, sql).await.contains("append-only"), "{sql}");
    }
}

#[tokio::test]
async fn fold_notes_a_wait_once_per_batch_and_version() {
    let (_dir, store) = seeded().await;
    let first = store
        .write(async |tx| batches::note_batch_wait(tx, rb(2), 2, at(6)).await)
        .await
        .unwrap();
    assert!(first);
    let again = store
        .write(async |tx| batches::note_batch_wait(tx, rb(2), 2, at(7)).await)
        .await
        .unwrap();
    assert!(!again);
    for sql in [
        "UPDATE review_batch_waits SET created_at = 9",
        "DELETE FROM review_batch_waits",
    ] {
        assert!(refused(&store, sql).await.contains("append-only"), "{sql}");
    }
}

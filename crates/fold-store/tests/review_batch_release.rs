use std::path::Path;

use crate::common::{count, exec, open};
use fold_core::object::ObjectHash;
use fold_core::{ProjectPaths, ProposalId, ReviewBatchId, Timestamp};
use fold_store::reviews::batches::{
    MemberRelease, has_open_member, members, release_member_reviewed_alone, remove_member,
};
use fold_store::{Store, StoreError};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

/// One worker, two tasks with an open proposal each, and batch 1 holding P-1 and P-2.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'review', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'review', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'review', 0, 0), (2, 2, 'two', 'review', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', '{base}', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, '{one}', 'm', 'review', 0), (2, 2, 2, 2, 1, '{two}', 'm', 'review', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0), (2, 2, 2, 1, 'q', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, '{one}', 1, 'review', 0), (2, 2, 1, 1, '{two}', 2, 'review', 0);
INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (1, 1, 1, 0);
INSERT INTO review_batch_members (id, batch_id, proposal_id, version_id, position) VALUES (1, 1, 1, 1, 1), (2, 1, 2, 2, 2);
";

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    let hash = |byte: u8| ObjectHash::from_bytes([byte; 32]).to_string();
    let seed = SEED
        .replace("{base}", &hash(1))
        .replace("{one}", &hash(2))
        .replace("{two}", &hash(3));
    for statement in seed.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
fn at_millis(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
fn batch() -> ReviewBatchId {
    ReviewBatchId::from_number(1).unwrap()
}

#[cfg(test)]
fn proposal(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

#[cfg(test)]
async fn release_alone(store: &Store, number: u32, at: i64) -> bool {
    store
        .write(async |tx| {
            release_member_reviewed_alone(tx, batch(), proposal(number), at_millis(at)).await
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_member_released_alone_reads_back_with_its_reason() {
    let (_dir, store) = seeded().await;
    assert!(has_open_member(store.read(), batch()).await.unwrap());
    assert!(release_alone(&store, 1, 7).await);

    let rows = members(store.read(), batch()).await.unwrap();
    assert_eq!(rows[0].release, Some(MemberRelease::ReviewedAlone));
    assert_eq!(rows[0].released_at, Some(at_millis(7)));
    assert!(!rows[0].is_open());
    assert!(rows[1].is_open());
    assert_eq!(MemberRelease::ReviewedAlone.as_str(), "reviewed_alone");
    assert!(has_open_member(store.read(), batch()).await.unwrap());
}

#[tokio::test]
async fn the_open_member_read_turns_false_with_the_last_release() {
    let (_dir, store) = seeded().await;
    assert!(release_alone(&store, 1, 7).await);
    assert!(release_alone(&store, 2, 8).await);
    assert!(!has_open_member(store.read(), batch()).await.unwrap());
    let batch_state = count(
        &store,
        "SELECT COUNT(*) FROM review_batches WHERE state = 'open'",
    )
    .await;
    assert_eq!(batch_state, 1, "the batch itself stays open");
}

#[tokio::test]
async fn releasing_alone_refuses_a_proposal_that_is_not_an_open_member() {
    let (_dir, store) = seeded().await;
    assert!(release_alone(&store, 1, 7).await);
    assert!(!release_alone(&store, 1, 9).await, "released once only");
    assert!(!release_alone(&store, 3, 9).await, "never a member");
    let removed = store
        .write(async |tx| remove_member(tx, batch(), proposal(1), at_millis(9)).await)
        .await
        .unwrap();
    assert!(!removed);
    let rows = members(store.read(), batch()).await.unwrap();
    assert_eq!(rows[0].released_at, Some(at_millis(7)));
}

#[tokio::test]
async fn the_rebuilt_table_keeps_its_rules() {
    let (_dir, store) = seeded().await;
    let unknown = exec(
        &store,
        "UPDATE review_batch_members SET released_at = 5, release_reason = 'bored' WHERE id = 1",
    )
    .await
    .unwrap_err();
    assert!(matches!(unknown, StoreError::Database(_)), "{unknown:?}");

    exec(
        &store,
        "UPDATE review_batch_members SET released_at = 5, release_reason = 'reviewed_alone' WHERE id = 1",
    )
    .await
    .unwrap();
    let twice = exec(
        &store,
        "UPDATE review_batch_members SET released_at = 6, release_reason = 'removed' WHERE id = 1",
    )
    .await
    .unwrap_err();
    assert!(
        twice
            .to_string()
            .contains("review_batch_members_release_only")
    );
    let delete = exec(&store, "DELETE FROM review_batch_members WHERE id = 2")
        .await
        .unwrap_err();
    assert!(delete.to_string().contains("review_batch_members"));
    let again = exec(
        &store,
        "INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (2, 2, 1, 0)",
    )
    .await;
    again.unwrap();
    let duplicate = exec(
        &store,
        "INSERT INTO review_batch_members (batch_id, proposal_id, version_id, position) VALUES (2, 2, 2, 1)",
    )
    .await
    .unwrap_err();
    assert!(
        duplicate
            .to_string()
            .contains("review_batch_members.proposal_id")
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM pragma_foreign_key_check").await,
        0
    );
}

/// A member row as the migration test reads it: id, proposal row id, position, release time and reason.
type MemberRow = (i64, i64, i64, Option<i64>, Option<String>);

/// The rebuild keeps every member row with its id and every line run that points to one, on a database that holds them before the migration.
#[tokio::test]
async fn the_migration_keeps_existing_members_and_their_lines() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    std::fs::create_dir(&before).unwrap();
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"));
    for entry in std::fs::read_dir(source).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: u32 = name.split('_').next().unwrap().parse().unwrap();
        if name.ends_with(".sql") && version < 78 {
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
    let hash = |byte: u8| ObjectHash::from_bytes([byte; 32]).to_string();
    let seed = SEED
        .replace("{base}", &hash(1))
        .replace("{one}", &hash(2))
        .replace("{two}", &hash(3))
        .replace(
            "(id, batch_id, proposal_id, version_id, position) VALUES (1, 1, 1, 1, 1), (2, 1, 2, 2, 2)",
            "(id, batch_id, proposal_id, version_id, position, released_at, release_reason) VALUES (11, 1, 1, 1, 1, 4, 'removed'), (12, 1, 2, 2, 2, NULL, NULL)",
        );
    for statement in seed.lines().filter(|line| !line.is_empty()) {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut conn)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO review_batch_lines (id, workspace_id, member_id, path, start_line, end_line, member_start_line) VALUES (1, 1, 11, 'a.rs', 1, 2, 1), (2, 1, 12, 'b.rs', 3, 4, 3)",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT id, proposal_id, position, released_at, release_reason FROM review_batch_members ORDER BY id",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (11, 1, 1, Some(4), Some("removed".to_owned())),
            (12, 2, 2, None, None)
        ]
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM review_batch_lines l JOIN review_batch_members m ON m.id = l.member_id"
        )
        .await,
        2
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM pragma_foreign_key_check").await,
        0
    );
    assert!(release_alone(&store, 2, 9).await);
    assert!(!has_open_member(store.read(), batch()).await.unwrap());
}

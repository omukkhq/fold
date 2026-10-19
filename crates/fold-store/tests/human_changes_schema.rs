#![cfg(test)]

use std::borrow::Cow;

use crate::common::{count, exec, open};
use sqlx::Connection;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn conflicted(id: i64, commit: &str) -> String {
    format!(
        "INSERT INTO human_changes (id, commit_id, human, base_tree_hash, tree_hash, state, created_at) VALUES ({id}, '{commit}', 'ana', 't1', 't2', 'conflicted', 0)"
    )
}

#[tokio::test]
async fn one_change_waits_at_a_time_and_a_closed_one_never_changes() {
    let (_dir, _paths, store) = open().await;
    exec(&store, &conflicted(1, COMMIT_A)).await.unwrap();
    assert!(exec(&store, &conflicted(2, COMMIT_B)).await.is_err());
    assert!(exec(&store, &conflicted(3, "not-a-commit")).await.is_err());
    exec(
        &store,
        "INSERT INTO human_change_conflicts (human_change_id, path, kind) VALUES (1, X'612E7478', 'both_modified_text')",
    )
    .await
    .unwrap();
    for sql in [
        "UPDATE human_change_conflicts SET kind = 'add_add'",
        "DELETE FROM human_change_conflicts",
        "DELETE FROM human_changes",
        "UPDATE human_changes SET tree_hash = 't3'",
    ] {
        assert!(exec(&store, sql).await.is_err(), "{sql}");
    }
    // A change leaves `conflicted` with its closing time, and a resolution names an integration that exists.
    assert!(
        exec(
            &store,
            "UPDATE human_changes SET state = 'superseded' WHERE id = 1"
        )
        .await
        .is_err(),
        "a closed change has its closing time"
    );
    exec(
        &store,
        "UPDATE human_changes SET state = 'superseded', closed_at = 1 WHERE id = 1",
    )
    .await
    .unwrap();
    assert!(
        exec(
            &store,
            "UPDATE human_changes SET closed_at = 2 WHERE id = 1"
        )
        .await
        .is_err()
    );
    exec(&store, &conflicted(2, COMMIT_B)).await.unwrap();
    assert!(
        exec(
            &store,
            "UPDATE human_changes SET state = 'resolved', closed_at = 1, integration_id = 99 WHERE id = 2"
        )
        .await
        .is_err()
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM human_changes").await, 2);
}

/// The migrations rebuild `integrations` and `human_changes` with their rows, so a database from before them keeps its integrations, and a recorded human change names the `record` integration of its own commit.
#[tokio::test]
async fn a_database_from_before_human_changes_records_them_against_its_integrations() {
    let dir = tempfile::TempDir::new().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("fold.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    let mut before: Migrator = sqlx::migrate!("./migrations/project");
    let added = before
        .migrations
        .iter()
        .position(|migration| migration.description == "human changes")
        .unwrap();
    before.migrations = Cow::Owned(before.migrations[..added].to_vec());
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO integrations (id, number, kind, remote_commit, result, actor, created_at) VALUES (9, 1, 'pull', '{COMMIT_A}', 'succeeded', 'human:ana', 0)"
    )))
    .execute(&mut conn)
    .await
    .unwrap();

    let after: Migrator = sqlx::migrate!("./migrations/project");
    after.run(&mut conn).await.unwrap();
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM integrations WHERE kind = 'pull'")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(kept, 1, "the rebuilt integrations keep their rows");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO integrations (id, number, kind, remote_commit, result, actor, created_at) VALUES (10, 2, 'record', '{COMMIT_A}', 'succeeded', 'human:ana', 0)"
    )))
    .execute(&mut conn)
    .await
    .unwrap();
    let recorded = |commit: &str, integration: i64| {
        format!(
            "INSERT INTO human_changes (commit_id, human, base_tree_hash, tree_hash, state, integration_id, created_at, closed_at) VALUES ('{commit}', 'ana', 't1', 't2', 'recorded', {integration}, 0, 0)"
        )
    };
    for refused in [recorded(COMMIT_A, 9), recorded(COMMIT_B, 10)] {
        assert!(
            sqlx::raw_sql(sqlx::AssertSqlSafe(refused))
                .execute(&mut conn)
                .await
                .is_err(),
            "a recorded change comes in through the record integration of its own commit"
        );
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(recorded(COMMIT_A, 10)))
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
}

#[tokio::test]
async fn a_change_read_from_the_folder_has_no_commit_and_its_record_integration_none() {
    let (_dir, _paths, store) = open().await;
    exec(
        &store,
        "INSERT INTO integrations (id, number, kind, result, actor, created_at) VALUES (5, 1, 'record', 'succeeded', 'human:ana', 0)",
    )
    .await
    .unwrap();
    assert!(
        exec(
            &store,
            "INSERT INTO integrations (id, number, kind, result, actor, created_at) VALUES (6, 2, 'record', 'conflicted', 'human:ana', 0)",
        )
        .await
        .is_err(),
        "a record integration always succeeded"
    );
    exec(
        &store,
        "INSERT INTO human_changes (commit_id, human, base_tree_hash, tree_hash, state, integration_id, created_at, closed_at) VALUES (NULL, 'ana', 't1', 't2', 'recorded', 5, 0, 0)",
    )
    .await
    .unwrap();
    assert!(
        exec(
            &store,
            &format!(
                "INSERT INTO human_changes (commit_id, human, base_tree_hash, tree_hash, state, integration_id, created_at, closed_at) VALUES ('{COMMIT_A}', 'ana', 't1', 't2', 'recorded', 5, 0, 0)"
            ),
        )
        .await
        .is_err(),
        "the integration names no commit"
    );
    exec(
        &store,
        "INSERT INTO human_changes (commit_id, human, base_tree_hash, tree_hash, state, created_at) VALUES (NULL, 'ana', 't1', 't3', 'conflicted', 0)",
    )
    .await
    .unwrap();
}

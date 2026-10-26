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

#[tokio::test]
async fn a_change_of_the_folder_may_carry_a_title_that_is_never_blank() {
    let (_dir, _paths, store) = open().await;
    exec(
        &store,
        "INSERT INTO integrations (id, number, kind, message, result, actor, created_at) VALUES (5, 1, 'record', 'Stop tracking a.txt', 'succeeded', 'human:ana', 0)",
    )
    .await
    .unwrap();
    let blank = exec(
        &store,
        "INSERT INTO integrations (id, number, kind, message, result, actor, created_at) VALUES (6, 2, 'record', '  ', 'succeeded', 'human:ana', 0)",
    )
    .await
    .unwrap_err();
    assert!(
        blank
            .to_string()
            .contains("integrations_columns_match_kind"),
        "{blank}"
    );
}

/// The migration that lets a change of the folder carry a title rebuilds `integrations`: its rows keep their ids and links, the rows that refer to them stay valid, and its indexes and append-only triggers are back.
#[tokio::test]
async fn the_record_title_rebuild_keeps_the_integrations_and_their_rules() {
    let dir = tempfile::TempDir::new().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("fold.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    let mut before: Migrator = sqlx::migrate!("./migrations/project");
    let rebuild = before
        .migrations
        .iter()
        .position(|migration| migration.description == "record message")
        .unwrap();
    before.migrations = Cow::Owned(before.migrations[..rebuild].to_vec());
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO integrations (id, number, kind, result, actor, created_at) VALUES (5, 1, 'record', 'succeeded', 'human:ana', 7);
        INSERT INTO human_changes (id, commit_id, human, base_tree_hash, tree_hash, state, integration_id, created_at, closed_at) VALUES (3, NULL, 'ana', 't1', 't2', 'recorded', 5, 7, 7);",
    )
    .execute(&mut conn)
    .await
    .unwrap();

    let after: Migrator = sqlx::migrate!("./migrations/project");
    after.run(&mut conn).await.unwrap();

    let kept: (i64, i64, String, Option<String>, i64) =
        sqlx::query_as("SELECT id, number, kind, message, created_at FROM integrations")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(kept, (5, 1, "record".to_owned(), None, 7));
    let linked: i64 = sqlx::query_scalar("SELECT integration_id FROM human_changes WHERE id = 3")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(linked, 5);
    let violations: Vec<(String,)> =
        sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    let indexes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = 'integrations' AND name IN ('integrations_proposal', 'integrations_number', 'integrations_one_succeeded_per_proposal', 'integrations_version', 'integrations_one_per_changeset', 'integrations_review_batch')",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(indexes, 6);
    let triggers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'integrations' AND name IN ('integrations_no_update', 'integrations_no_delete', 'integrations_direct_main_changeset', 'integrations_review_batch_shape')",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(triggers, 4);
    for change in [
        "UPDATE integrations SET actor = 'human:bo' WHERE id = 5",
        "DELETE FROM integrations WHERE id = 5",
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(change))
            .execute(&mut conn)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("append-only"),
            "{change}: {error}"
        );
    }
    let leftovers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'integrations_copy'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(leftovers, 0);
    conn.close().await.unwrap();
}

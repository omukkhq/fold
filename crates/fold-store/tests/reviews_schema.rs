use std::borrow::Cow;

use crate::common::{count, exec, open};
use fold_store::testing::assert_append_only;
use fold_store::{Store, StoreError};
use sqlx::Connection;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

/// A proposal `P-1` with version 1 by the worker `backend`, and the worker `review`. No review yet.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0), (2, 'review', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0), (2, 2, 'review', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'backend', 0);
";

/// `id, reviewer_worker_id, reviewer_human, verdict, created_at`.
type ReviewColumns = (i64, Option<i64>, Option<String>, String, i64);

/// `id, number, review_id, reply_to, side`.
type CommentColumns = (i64, i64, i64, Option<i64>, Option<String>);

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
fn review(id: u32, worker: &str, human: &str, verdict: &str, body: &str) -> String {
    format!(
        "INSERT INTO reviews (id, version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at) VALUES ({id}, 1, {worker}, {human}, '{verdict}', '{body}', 0)"
    )
}

/// A comment `rc-<number>` under review 1; `location` is the `path, start_line, end_line, side` values.
#[cfg(test)]
fn comment(number: u32, worker: &str, human: &str, location: &str) -> String {
    format!(
        "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, author_human, path, start_line, end_line, side, body, created_at) VALUES ({number}, 1, 1, {worker}, {human}, {location}, 'b', 0)"
    )
}

#[cfg(test)]
async fn refused_by(store: &Store, sql: &str) -> Option<String> {
    let error: StoreError = exec(store, sql).await.unwrap_err();
    error.constraint()
}

#[tokio::test]
async fn reviews_and_comments_are_append_only() {
    let (_dir, store) = seeded().await;
    exec(&store, &review(1, "2", "NULL", "approve", "fine"))
        .await
        .unwrap();
    exec(&store, &comment(1, "2", "NULL", "'a.rs', 1, 2, 'new'"))
        .await
        .unwrap();
    assert_append_only(&store, "reviews").await;
    assert_append_only(&store, "review_comments").await;
}

#[tokio::test]
async fn a_review_has_exactly_one_reviewer_and_a_known_verdict() {
    let (_dir, store) = seeded().await;
    for (worker, human) in [("2", "'tanim'"), ("NULL", "NULL")] {
        assert_eq!(
            refused_by(&store, &review(1, worker, human, "approve", "b"))
                .await
                .as_deref(),
            Some("reviews_exactly_one_reviewer"),
            "{worker} {human}"
        );
    }
    assert_eq!(
        refused_by(&store, &review(1, "2", "NULL", "veto", "b"))
            .await
            .as_deref(),
        Some("reviews_verdict_values")
    );
    for (id, verdict) in [
        (1, "comment"),
        (2, "request_changes"),
        (3, "approve"),
        (4, "reject"),
    ] {
        exec(&store, &review(id, "2", "NULL", verdict, "why"))
            .await
            .unwrap();
    }
    exec(&store, &review(5, "NULL", "'tanim'", "reject", "no"))
        .await
        .unwrap();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM reviews").await, 5);
}

#[tokio::test]
async fn a_reject_needs_a_body() {
    let (_dir, store) = seeded().await;
    for body in ["", "  "] {
        assert_eq!(
            refused_by(&store, &review(1, "2", "NULL", "reject", body))
                .await
                .as_deref(),
            Some("reviews_reject_has_body")
        );
    }
    exec(&store, &review(1, "2", "NULL", "approve", ""))
        .await
        .unwrap();
}

#[tokio::test]
async fn comment_location_and_author_checks() {
    let (_dir, store) = seeded().await;
    exec(&store, &review(1, "2", "NULL", "comment", ""))
        .await
        .unwrap();
    let cases = [
        ("'a.rs', 9, 3, 'new'", "review_comments_range_order"),
        ("'a.rs', 0, 3, 'new'", "review_comments_range_order"),
        ("'a.rs', 1, 3, NULL", "review_comments_location_complete"),
        (
            "NULL, NULL, NULL, 'old'",
            "review_comments_location_complete",
        ),
        ("NULL, 1, 1, 'new'", "review_comments_location_complete"),
        (
            "'a.rs', NULL, NULL, 'new'",
            "review_comments_location_complete",
        ),
        ("'a.rs', 1, 1, 'middle'", "review_comments_side_values"),
    ];
    for (location, constraint) in cases {
        assert_eq!(
            refused_by(&store, &comment(1, "2", "NULL", location))
                .await
                .as_deref(),
            Some(constraint),
            "{location}"
        );
    }
    for (worker, human) in [("2", "'tanim'"), ("NULL", "NULL")] {
        assert_eq!(
            refused_by(&store, &comment(1, worker, human, "NULL, NULL, NULL, NULL"))
                .await
                .as_deref(),
            Some("review_comments_exactly_one_author"),
            "{worker} {human}"
        );
    }
    exec(&store, &comment(1, "2", "NULL", "'a.rs', 3, 3, 'old'"))
        .await
        .unwrap();
    exec(
        &store,
        &comment(2, "NULL", "'tanim'", "'a.rs', 3, 9, 'new'"),
    )
    .await
    .unwrap();
    exec(&store, &comment(3, "2", "NULL", "NULL, NULL, NULL, NULL"))
        .await
        .unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM review_comments").await,
        3
    );
}

/// The migration that widens the verdict rebuilds both tables. Rows written before it keep their ids, links and replies, and the foreign keys still hold afterwards.
#[tokio::test]
async fn the_rebuild_keeps_existing_reviews_and_comments() {
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
        .position(|migration| migration.description == "review rules")
        .unwrap();
    before.migrations = Cow::Owned(before.migrations[..rebuild].to_vec());
    before.run(&mut conn).await.unwrap();
    let rows = format!(
        "{SEED}
        INSERT INTO reviews (id, version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at) VALUES (7, 1, 2, NULL, 'request_changes', 'fix', 5), (8, 1, NULL, 'tanim', 'approve', 'ok', 6);
        INSERT INTO review_comments (id, number, version_id, review_id, author_worker_id, author_human, path, start_line, end_line, side, reply_to, body, created_at) VALUES (3, 4, 1, 7, 2, NULL, 'a.rs', 2, 5, 'new', NULL, 'here', 5), (4, 5, 1, 8, NULL, 'tanim', NULL, NULL, NULL, NULL, 4, 'agreed', 6);"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(rows))
        .execute(&mut conn)
        .await
        .unwrap();

    let after: Migrator = sqlx::migrate!("./migrations/project");
    after.run(&mut conn).await.unwrap();

    let reviews: Vec<ReviewColumns> = sqlx::query_as(
        "SELECT id, reviewer_worker_id, reviewer_human, verdict, created_at FROM reviews ORDER BY id",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert_eq!(
        reviews,
        [
            (7, Some(2), None, "request_changes".to_owned(), 5),
            (8, None, Some("tanim".to_owned()), "approve".to_owned(), 6),
        ]
    );
    let comments: Vec<CommentColumns> = sqlx::query_as(
        "SELECT id, number, review_id, reply_to, side FROM review_comments ORDER BY id",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert_eq!(
        comments,
        [
            (3, 4, 7, None, Some("new".to_owned())),
            (4, 5, 8, Some(4), None),
        ]
    );
    let violations: Vec<(String,)> =
        sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    for orphan in [
        "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, body, created_at) VALUES (9, 1, 99, 2, 'b', 0)",
        "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, reply_to, body, created_at) VALUES (9, 1, 7, 2, 99, 'b', 0)",
        "INSERT INTO reviews (version_id, reviewer_worker_id, verdict, body, created_at) VALUES (99, 2, 'approve', 'b', 0)",
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(orphan))
            .execute(&mut conn)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("FOREIGN KEY"),
            "{orphan}: {error}"
        );
    }
    let indexes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name IN ('reviews_version', 'review_comments_version', 'review_comments_review')",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(indexes, 3);
    let triggers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name IN ('reviews_no_update', 'reviews_no_delete', 'review_comments_no_update', 'review_comments_no_delete')",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(triggers, 4);
    for change in [
        "UPDATE reviews SET body = 'changed' WHERE id = 7",
        "DELETE FROM reviews WHERE id = 8",
        "UPDATE review_comments SET body = 'changed' WHERE id = 3",
        "DELETE FROM review_comments WHERE id = 4",
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
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('reviews_copy', 'review_comments_copy')",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(leftovers, 0);
    conn.close().await.unwrap();
}

/// The migration of human edits applies to a database that already holds reviews: their verdicts read the same afterwards, and the new rules shape the kept approvals and the requests for review again.
#[tokio::test]
async fn the_human_edits_migration_keeps_existing_verdicts_and_shapes_kept_approvals() {
    let dir = tempfile::TempDir::new().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("fold.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    let mut before: Migrator = sqlx::migrate!("./migrations/project");
    let edits = before
        .migrations
        .iter()
        .position(|migration| migration.description == "human edits")
        .unwrap();
    before.migrations = Cow::Owned(before.migrations[..edits].to_vec());
    before.run(&mut conn).await.unwrap();
    let rows = format!(
        "{SEED}
        INSERT INTO reviews (id, version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at) VALUES (7, 1, 2, NULL, 'approve', 'ok', 5), (8, 1, NULL, 'tanim', 'request_changes', 'fix', 6), (9, 1, NULL, 'tanim', 'comment', 'note', 7);"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(rows))
        .execute(&mut conn)
        .await
        .unwrap();
    let verdict = "SELECT verdict FROM proposal_verdicts WHERE proposal_id = 1";
    let old: Option<String> = sqlx::query_scalar(verdict)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(old.as_deref(), Some("request_changes"));

    let after: Migrator = sqlx::migrate!("./migrations/project");
    after.run(&mut conn).await.unwrap();

    let new: Option<String> = sqlx::query_scalar(verdict)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(new, old);
    let kept: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reviews WHERE kept_from_review_id IS NOT NULL")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(kept, 0);
    let violations: Vec<(String,)> =
        sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert!(violations.is_empty(), "{violations:?}");

    sqlx::raw_sql(
        "INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (2, 2, 1, 1, 1, 't3', 'm', 'human:tanim', 8);
         INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (2, 1, 2, 1, 't3', 2, 'human:tanim', 8);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let kept_row = |id: u32, version: u32, verdict: &str, from: u32| {
        format!(
            "INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at, kept_from_review_id) VALUES ({id}, {version}, 2, '{verdict}', '', 9, {from})"
        )
    };
    for refused in [
        kept_row(20, 2, "request_changes", 7),
        kept_row(21, 2, "approve", 8),
        kept_row(22, 1, "approve", 7),
        kept_row(23, 2, "approve", 99),
        "INSERT INTO reviews (id, version_id, reviewer_human, verdict, body, created_at, kept_from_review_id) VALUES (25, 2, 'tanim', 'approve', '', 9, 7)".to_owned(),
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(refused.clone()))
            .execute(&mut conn)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("reviews_kept_shape"),
            "{refused}: {error}"
        );
    }
    sqlx::query(sqlx::AssertSqlSafe(kept_row(24, 2, "approve", 7)))
        .execute(&mut conn)
        .await
        .unwrap();
    let edited: Option<String> = sqlx::query_scalar(verdict)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(edited.as_deref(), Some("approve"));

    let error = sqlx::query(
        "INSERT INTO review_again_requests (version_id, requested_by_human, created_at) VALUES (2, '', 10)",
    )
    .execute(&mut conn)
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("review_again_requests_human_present"),
        "{error}"
    );
    sqlx::query(
        "INSERT INTO review_again_requests (version_id, requested_by_human, created_at) VALUES (2, 'tanim', 10)",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let sent: Option<String> = sqlx::query_scalar(verdict)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(
        sent, None,
        "a version sent for review again has no kept verdict"
    );
}

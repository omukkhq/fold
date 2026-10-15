use crate::common::{count, exec, open};
use fold_core::object::{ObjectKind, hash_object};
use fold_store::testing::assert_append_only;
use fold_store::workspaces::gc_roots;
use fold_store::{Store, StoreError};

/// One agent, a task, request 1 with a draft waiting for the requester, and one row in every append-only request table.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'frontend', 0, 0);
INSERT INTO requests (id, number, kind, title, body, state, requester, drafter_worker_id, drafter_since, created_at, updated_at) VALUES (1, 1, 'bug', 'Crash', 'It crashes', 'draft_ready', 'human:tanim', 1, 0, 0, 0);
INSERT INTO request_drafts (id, request_id, version, author, created_at) VALUES (1, 1, 1, 'frontend', 0);
INSERT INTO request_draft_cards (draft_id, key, position, title, description) VALUES (1, 'c1', 1, 'One', 'd'), (1, 'c2', 2, 'Two', 'd');
INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key) VALUES (1, 'c1', 'c2');
INSERT INTO request_comments (request_id, version, author, body, created_at) VALUES (1, 1, 'human:tanim', 'more', 0);
";

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    let object = hash_object(ObjectKind::Blob, b"screenshot");
    exec(
        &store,
        &format!(
            "INSERT INTO request_attachments (request_id, position, name, media_type, size, object, created_at) VALUES (1, 1, 'shot.png', 'image/png', 10, '{object}', 0)"
        ),
    )
    .await
    .unwrap();
    (dir, store)
}

#[cfg(test)]
fn request_sql(number: u32, columns: &str, values: &str) -> String {
    format!(
        "INSERT INTO requests (number, body, requester, created_at, updated_at{columns}) VALUES ({number}, 'b', 'frontend', 0, 0{values})"
    )
}

#[cfg(test)]
async fn refused(store: &Store, sql: &str) -> StoreError {
    exec(store, sql).await.unwrap_err()
}

#[tokio::test]
async fn the_request_history_tables_are_append_only() {
    let (_dir, store) = seeded().await;
    for table in [
        "request_attachments",
        "request_drafts",
        "request_draft_cards",
        "request_draft_edges",
        "request_comments",
    ] {
        assert_append_only(&store, table).await;
    }
}

#[tokio::test]
async fn state_kind_and_text_checks_hold() {
    let (_dir, store) = seeded().await;
    exec(&store, &request_sql(2, "", "")).await.unwrap();
    let cases = [
        (request_sql(3, ", state", ", 'waiting'"), "requests_state_values"),
        (
            request_sql(3, ", kind", ", 'chore'"),
            "requests_kind_values",
        ),
        (
            request_sql(3, ", title", ", 'two\nlines'"),
            "requests_title_single_line",
        ),
        (
            "INSERT INTO requests (number, body, requester, created_at, updated_at) VALUES (3, '  ', 'frontend', 0, 0)".to_owned(),
            "requests_body_present",
        ),
        (
            request_sql(3, ", state, drafter_worker_id, drafter_since", ", 'drafting', 1, 0"),
            "requests_sorted_once_claimed",
        ),
        (
            request_sql(
                3,
                ", kind, title, state",
                ", 'bug', 't', 'drafting'",
            ),
            "requests_drafter_matches_state",
        ),
        (
            request_sql(
                3,
                ", kind, title, state, drafter_worker_id",
                ", 'bug', 't', 'draft_ready', 1",
            ),
            "requests_drafter_since_with_drafter",
        ),
        (
            request_sql(3, ", state", ", 'closed'"),
            "requests_close_reason_iff_closed",
        ),
        (
            request_sql(3, ", close_reason", ", 'done'"),
            "requests_close_reason_iff_closed",
        ),
    ];
    for (sql, constraint) in cases {
        let error = refused(&store, &sql).await;
        assert_eq!(error.constraint().as_deref(), Some(constraint), "{sql}");
    }
    exec(
        &store,
        &request_sql(3, ", state, close_reason", ", 'closed', 'not needed'"),
    )
    .await
    .unwrap();
    let duplicate = refused(&store, &request_sql(2, "", "")).await;
    assert_eq!(duplicate.constraint().as_deref(), Some("requests.number"));
}

#[tokio::test]
async fn drafts_cards_and_edges_hold_their_shape() {
    let (_dir, store) = seeded().await;
    let version = refused(
        &store,
        "INSERT INTO request_drafts (request_id, version, author, created_at) VALUES (1, 1, 'frontend', 0)",
    )
    .await;
    assert_eq!(
        version.constraint().as_deref(),
        Some("request_drafts.request_id, request_drafts.version")
    );
    exec(
        &store,
        "INSERT INTO request_drafts (id, request_id, version, author, created_at) VALUES (2, 1, 2, 'human:tanim', 0)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO request_draft_cards (draft_id, key, position, title, description) VALUES (2, 'c1', 1, 'One', 'd')",
    )
    .await
    .unwrap();
    let self_edge = refused(
        &store,
        "INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key) VALUES (2, 'c1', 'c1')",
    )
    .await;
    assert_eq!(
        self_edge.constraint().as_deref(),
        Some("request_draft_edges_no_self_edge")
    );
    let unknown = refused(
        &store,
        "INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key) VALUES (2, 'c1', 'c2')",
    )
    .await;
    assert!(unknown.to_string().contains("FOREIGN KEY"), "{unknown}");
    let key = refused(
        &store,
        "INSERT INTO request_draft_cards (draft_id, key, position, title, description) VALUES (2, 'c1', 2, 'Again', 'd')",
    )
    .await;
    assert_eq!(
        key.constraint().as_deref(),
        Some("request_draft_cards.draft_id, request_draft_cards.key")
    );
    let comment = refused(
        &store,
        "INSERT INTO request_comments (request_id, version, author, body, created_at) VALUES (1, 9, 'human:tanim', 'x', 0)",
    )
    .await;
    assert!(comment.to_string().contains("FOREIGN KEY"), "{comment}");
}

#[tokio::test]
async fn attachments_keep_their_limits() {
    let (_dir, store) = seeded().await;
    let attachment = |position: u32, name: &str, size: u64| {
        format!(
            "INSERT INTO request_attachments (request_id, position, name, media_type, size, object, created_at) VALUES (1, {position}, '{name}', 'text/plain', {size}, 'h', 0)"
        )
    };
    for (sql, constraint) in [
        (
            attachment(11, "a.txt", 1),
            "request_attachments_position_range",
        ),
        (
            attachment(2, "a/b.txt", 1),
            "request_attachments_plain_name",
        ),
        (attachment(2, "a..txt", 1), "request_attachments_plain_name"),
        (attachment(2, "", 1), "request_attachments_plain_name"),
        (
            attachment(2, "big.bin", 10 * 1024 * 1024 + 1),
            "request_attachments_size_range",
        ),
    ] {
        let error = refused(&store, &sql).await;
        assert_eq!(error.constraint().as_deref(), Some(constraint), "{sql}");
    }
    exec(&store, &attachment(10, "last.txt", 10 * 1024 * 1024))
        .await
        .unwrap();
    let twice = refused(&store, &attachment(1, "again.txt", 1)).await;
    assert_eq!(
        twice.constraint().as_deref(),
        Some("request_attachments.request_id, request_attachments.position")
    );
}

#[tokio::test]
async fn a_task_keeps_the_request_that_made_it() {
    let (_dir, store) = seeded().await;
    let missing = refused(&store, "UPDATE tasks SET request_id = 9 WHERE number = 1").await;
    assert!(missing.to_string().contains("FOREIGN KEY"), "{missing}");
    exec(&store, "UPDATE tasks SET request_id = 1 WHERE number = 1")
        .await
        .unwrap();
    exec(&store, &request_sql(2, "", "")).await.unwrap();
    let moved = refused(&store, "UPDATE tasks SET request_id = 2 WHERE number = 1").await;
    assert!(
        moved.to_string().contains("tasks.request_id never changes"),
        "{moved}"
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM tasks WHERE request_id = 1").await,
        1
    );
    exec(
        &store,
        "INSERT INTO messages (number, thread_id, sender_kind, body, created_at, request_id) VALUES (1, 1, 'fold', 'b', 0, 1)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_attachment_blob_is_a_gc_root() {
    let (_dir, store) = seeded().await;
    let roots = gc_roots(store.read()).await.unwrap();
    assert_eq!(roots, vec![hash_object(ObjectKind::Blob, b"screenshot")]);
}

use crate::common::{count, exec, open};
use fold_store::testing::assert_append_only;
use fold_store::{Store, StoreError};

/// One worker, two tasks with an open proposal each, batch 1 holding P-1 at position 1, its checkout and one line origin.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'review', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'review', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'review', 0, 0), (2, 2, 'two', 'review', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'review', 0), (2, 2, 2, 2, 1, 't3', 'm', 'review', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0), (2, 2, 2, 1, 'q', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'review', 0), (2, 2, 1, 1, 't3', 2, 'review', 0);
INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (1, 1, 1, 0);
INSERT INTO review_batch_members (id, batch_id, proposal_id, version_id, position) VALUES (1, 1, 1, 1, 1);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, review_batch_id, state, created_at) VALUES (3, 3, 1, 1, 'review', 'ws/review-rb-1', 1, 1, 'active', 0);
INSERT INTO review_batch_lines (workspace_id, member_id, path, start_line, end_line, member_start_line) VALUES (3, 1, 'a.rs', 1, 2, 1);
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
    let error: StoreError = exec(store, sql).await.unwrap_err();
    error.to_string()
}

#[tokio::test]
async fn line_origins_are_append_only() {
    let (_dir, store) = seeded().await;
    assert_append_only(&store, "review_batch_lines").await;
}

#[tokio::test]
async fn a_proposal_is_in_one_open_batch_at_a_time() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (2, 2, 1, 0)",
    )
    .await
    .unwrap();
    let again = "INSERT INTO review_batch_members (batch_id, proposal_id, version_id, position) VALUES (2, 1, 1, 1)";
    assert!(
        refused(&store, again)
            .await
            .contains("review_batch_members.proposal_id")
    );
    exec(
        &store,
        "UPDATE review_batch_members SET released_at = 5, release_reason = 'removed' WHERE id = 1",
    )
    .await
    .unwrap();
    exec(&store, again).await.unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM review_batch_members").await,
        2
    );
}

#[tokio::test]
async fn a_member_names_a_version_of_its_own_proposal_once_per_position() {
    let (_dir, store) = seeded().await;
    let other_version = "INSERT INTO review_batch_members (batch_id, proposal_id, version_id, position) VALUES (1, 2, 1, 2)";
    assert!(
        refused(&store, other_version)
            .await
            .contains("review_batch_members_version_of_proposal")
    );
    let same_position = "INSERT INTO review_batch_members (batch_id, proposal_id, version_id, position) VALUES (1, 2, 2, 1)";
    assert!(
        refused(&store, same_position)
            .await
            .contains("review_batch_members.position")
    );
}

#[tokio::test]
async fn a_member_changes_only_by_leaving_once() {
    let (_dir, store) = seeded().await;
    for sql in [
        "UPDATE review_batch_members SET position = 3 WHERE id = 1",
        "UPDATE review_batch_members SET released_at = 5 WHERE id = 1",
        "DELETE FROM review_batch_members",
    ] {
        let message = refused(&store, sql).await;
        assert!(message.contains("review_batch_members"), "{sql}: {message}");
    }
    exec(&store, "UPDATE review_batch_members SET released_at = 5, release_reason = 'submitted' WHERE id = 1")
        .await
        .unwrap();
    let twice =
        "UPDATE review_batch_members SET released_at = 6, release_reason = 'removed' WHERE id = 1";
    assert!(
        refused(&store, twice)
            .await
            .contains("review_batch_members_release_only")
    );
}

#[tokio::test]
async fn a_batch_changes_only_by_its_submission() {
    let (_dir, store) = seeded().await;
    let holder = "UPDATE review_batches SET holder_worker_id = 1, created_at = 9 WHERE id = 1";
    assert!(
        refused(&store, holder)
            .await
            .contains("review_batches_end_only")
    );
    let unstamped = "UPDATE review_batches SET state = 'submitted' WHERE id = 1";
    assert!(
        refused(&store, unstamped)
            .await
            .contains("review_batches_submitted_at_iff_submitted")
    );
    exec(
        &store,
        "UPDATE review_batches SET state = 'submitted', submitted_at = 4 WHERE id = 1",
    )
    .await
    .unwrap();
    let reopen = "UPDATE review_batches SET state = 'open', submitted_at = NULL WHERE id = 1";
    assert!(
        refused(&store, reopen)
            .await
            .contains("review_batches_end_only")
    );
    assert!(
        refused(&store, "DELETE FROM review_batches")
            .await
            .contains("review_batches")
    );
}

#[tokio::test]
async fn a_batch_checkout_is_a_review_workspace_of_no_single_proposal() {
    let (_dir, store) = seeded().await;
    let both = "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, proposal_id, version_id, review_batch_id, state, created_at) VALUES (4, 1, 1, 'review', 'ws/x', 1, 1, 1, 1, 'archived', 0)";
    let task = "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, review_batch_id, state, created_at) VALUES (6, 2, 1, 'task', 'ws/z', 1, 1, 'archived', 0)";
    for sql in [both, task] {
        assert!(
            refused(&store, sql)
                .await
                .contains("workspaces_review_target"),
            "{sql}"
        );
    }
    let second = "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, review_batch_id, state, created_at) VALUES (7, 1, 1, 'review', 'ws/review-rb-1-2', 1, 1, 'active', 0)";
    assert!(
        refused(&store, second)
            .await
            .contains("workspaces.review_batch_id")
    );
    let retarget = "UPDATE workspaces SET proposal_id = 1 WHERE id = 3";
    assert!(
        refused(&store, retarget)
            .await
            .contains("workspaces_review_target")
    );
}

#[tokio::test]
async fn a_batch_closes_once_with_its_time() {
    let (_dir, store) = seeded().await;
    let unstamped = "UPDATE review_batches SET state = 'closed' WHERE id = 1";
    assert!(
        refused(&store, unstamped)
            .await
            .contains("review_batches_closed_at_iff_closed")
    );
    exec(
        &store,
        "UPDATE review_batches SET state = 'closed', closed_at = 4 WHERE id = 1",
    )
    .await
    .unwrap();
    let submit = "UPDATE review_batches SET state = 'submitted', submitted_at = 5, closed_at = NULL WHERE id = 1";
    assert!(
        refused(&store, submit)
            .await
            .contains("review_batches_end_only")
    );
}

#[tokio::test]
async fn a_member_row_of_a_batch_comment_leaves_its_body_to_the_batch_comment() {
    let (_dir, store) = seeded().await;
    for sql in [
        "INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at, review_batch_id) VALUES (1, 1, 1, 'comment', '', 0, 1)",
        "INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at) VALUES (2, 1, 1, 'comment', '', 0)",
        "INSERT INTO review_batch_comments (id, batch_id, number, body, created_at) VALUES (1, 1, 1, 'together', 0)",
    ] {
        exec(&store, sql).await.unwrap();
    }
    assert!(
        refused(&store, "INSERT INTO review_batch_comments (batch_id, number, body, created_at) VALUES (1, 1, 'again', 0)")
            .await
            .contains("review_batch_comments.batch_id")
    );
    assert_append_only(&store, "review_batch_comments").await;
    let row = |number: u32, review: u32, body: &str| {
        format!(
            "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, body, created_at, batch_comment_id) VALUES ({number}, 1, {review}, 1, '{body}', 0, 1)"
        )
    };
    for sql in [row(1, 1, "copy"), row(2, 2, "")] {
        assert!(
            refused(&store, &sql)
                .await
                .contains("review_comments_batch_comment_shape"),
            "{sql}"
        );
    }
    exec(&store, &row(3, 1, "")).await.unwrap();
}

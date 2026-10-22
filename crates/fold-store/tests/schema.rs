use std::collections::BTreeSet;

use crate::common::{count, exec, open};
use fold_store::{Store, StoreError};

const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'frontend', 0, 0), (2, 2, 'two', 'frontend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'frontend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'frontend', 0);
INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at) VALUES (1, 1, 1, 'comment', 'b', 0);
INSERT INTO review_comments (id, number, version_id, review_id, author_worker_id, body, created_at) VALUES (1, 1, 1, 1, 1, 'b', 0);
INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 1, 'proposal', 1, 1, 'succeeded', 'fold', 0);
INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (1, 'integration', 1, 0);
INSERT INTO messages (id, number, thread_id, sender_kind, body, created_at) VALUES (1, 1, 1, 'fold', 'b', 0);
INSERT INTO events (type, actor, data, created_at) VALUES ('task.created', 'fold', '{}', 0);
INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (1, 1, 1, 0);
INSERT INTO review_batch_members (id, batch_id, proposal_id, version_id, position) VALUES (1, 1, 1, 1, 1);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, review_batch_id, state, created_at) VALUES (2, 2, 1, 1, 'review', 'ws/review-rb-1', 1, 1, 'active', 0);
INSERT INTO review_batch_lines (workspace_id, member_id, path, start_line, end_line, member_start_line) VALUES (2, 1, 'a.rs', 1, 1, 1);
INSERT INTO review_batch_comments (id, batch_id, number, body, created_at) VALUES (1, 1, 1, 'b', 0);
INSERT INTO handoffs (agent_id, session_id, body, created_at) VALUES (1, 's', 'b', 0);
INSERT INTO worker_processes (id, agent_id, role, session_id, state, started_by, started_at) VALUES (1, 1, 'review_lead', 's-1', 'working', 'human:ana', 0);
INSERT INTO worker_permission_questions (id, number, kind, worker_process_id, session_id, request_id, tool, input, created_at) VALUES (1, 1, 'tool', 1, 's-1', 'r-1', 'Bash', '{}', 0), (2, NULL, 'ask', 1, 's-1', 'r-2', 'AskUserQuestion', '{}', 0), (3, 2, 'tool', 1, 's-1', 'r-3', 'Bash', '{}', 0);
INSERT INTO worker_permission_answers (question_id, behavior, answered_by, created_at) VALUES (1, 'allow', 'human:ana', 0);
INSERT INTO worker_permission_closures (question_id, created_at) VALUES (3, 0);
INSERT INTO worker_question_messages (question_id, position, message_id) VALUES (2, 1, 1);
INSERT INTO message_choices (message_id, position, label) VALUES (1, 1, 'Red');
INSERT INTO review_again_requests (version_id, requested_by_human, created_at) VALUES (1, 'ana', 0);
INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at, asks_human) VALUES (9, 9, 9, 'worker', 1, 'b', 0, 1);
INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (9, 'ana', 'human');
INSERT INTO message_ask_closures (message_id, reason, closed_by, created_at) VALUES (9, 'dismissed', 'human:ana', 0);
INSERT INTO worker_turns (worker_process_id, role, cost_micro_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, failed, interrupted, created_at) VALUES (1, 'review_lead', 10, 0, 0, 0, 0, 0, 0, 0);
";

const APPEND_ONLY: [&str; 20] = [
    "completions",
    "revisions",
    "changesets",
    "proposal_versions",
    "reviews",
    "review_comments",
    "integrations",
    "messages",
    "events",
    "review_batch_lines",
    "review_batch_comments",
    "handoffs",
    "worker_permission_questions",
    "worker_permission_answers",
    "worker_permission_closures",
    "worker_question_messages",
    "message_choices",
    "review_again_requests",
    "message_ask_closures",
    "worker_turns",
];

/// The append-only tables whose rows are derived in the transaction of a row that carries the time, so they have no `created_at` of their own.
const WITHOUT_CREATED_AT: [&str; 3] = [
    "review_batch_lines",
    "worker_question_messages",
    "message_choices",
];

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
async fn refused(store: &Store, sql: &str) -> StoreError {
    exec(store, sql).await.unwrap_err()
}

#[tokio::test]
async fn baseline_creates_every_table_of_the_data_model() {
    let (_dir, _paths, store) = open().await;
    let found: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    let expected: BTreeSet<&str> = [
        "agent_tags",
        "agents",
        "area_mcp_servers",
        "areas",
        "changesets",
        "check_results",
        "check_runs",
        "checklist_items",
        "checklists",
        "checkout",
        "checkpoints",
        "claims",
        "completions",
        "counters",
        "dependencies",
        "events",
        "folder_stash_conflicts",
        "folder_stashes",
        "git_sync",
        "handoffs",
        "human_change_conflicts",
        "human_changes",
        "integration_requests",
        "integrations",
        "labels",
        "message_ask_closures",
        "message_choices",
        "message_question_headers",
        "message_recipients",
        "messages",
        "project_pause",
        "proposal_versions",
        "proposals",
        "refs",
        "remote_states",
        "request_attachments",
        "request_comments",
        "request_draft_cards",
        "request_draft_edges",
        "request_drafts",
        "requests",
        "review_batch_comments",
        "review_batch_lines",
        "review_batch_members",
        "review_again_requests",
        "review_batches",
        "review_comments",
        "review_scope_choices",
        "review_scopes",
        "reviews",
        "revision_commits",
        "revisions",
        "sessions",
        "task_labels",
        "task_reports",
        "tasks",
        "worker_cursors",
        "worker_permission_answers",
        "worker_permission_closures",
        "worker_permission_questions",
        "worker_previous_sessions",
        "worker_processes",
        "worker_proposals",
        "worker_question_messages",
        "worker_tasks",
        "worker_turns",
        "workers",
        "workspace_conflicts",
        "workspaces",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        found.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        expected
    );
}

#[tokio::test]
async fn append_only_tables_have_both_triggers() {
    let (_dir, store) = seeded().await;
    for table in APPEND_ONLY {
        for operation in ["update", "delete"] {
            let name = format!("{table}_no_{operation}");
            let triggers = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1 AND tbl_name = ?2",
            )
            .bind(&name)
            .bind(table)
            .fetch_one(store.read())
            .await
            .unwrap();
            assert_eq!(triggers, 1, "{name}");
        }
        let created_at = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = 'created_at'",
        )
        .bind(table)
        .fetch_one(store.read())
        .await
        .unwrap();
        let expected_created_at = i64::from(!WITHOUT_CREATED_AT.contains(&table));
        assert_eq!(created_at, expected_created_at, "{table}.created_at");

        let expected = format!("{table} is append-only");
        let update = refused(&store, &format!("UPDATE {table} SET rowid = rowid")).await;
        assert!(update.to_string().contains(&expected), "{update}");
        let delete = refused(&store, &format!("DELETE FROM {table}")).await;
        assert!(delete.to_string().contains(&expected), "{delete}");
        assert!(count(&store, &format!("SELECT COUNT(*) FROM {table}")).await >= 1);
    }
}

#[tokio::test]
async fn one_unreleased_implementation_claim_per_task() {
    let (_dir, store) = seeded().await;
    let claim = "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at) VALUES ('implementation', 2, 1, 0, 0)";
    exec(&store, claim).await.unwrap();
    let error = refused(&store, claim).await;
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("claims.task_id"));
    exec(
        &store,
        "UPDATE claims SET released_at = 1, release_reason = 'done' WHERE task_id = 2",
    )
    .await
    .unwrap();
    exec(&store, claim).await.unwrap();
    // Review claims on the same table do not collide with the task index.
    exec(
        &store,
        "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 1, 1, 0, 0)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn one_unreleased_review_claim_per_proposal() {
    let (_dir, store) = seeded().await;
    let claim = "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 1, 1, 0, 0)";
    exec(&store, claim).await.unwrap();
    let error = refused(&store, claim).await;
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("claims.proposal_id"));
    exec(
        &store,
        "UPDATE claims SET released_at = 1 WHERE proposal_id = 1",
    )
    .await
    .unwrap();
    exec(&store, claim).await.unwrap();
}

#[tokio::test]
async fn a_claim_needs_the_target_its_kind_names() {
    let (_dir, store) = seeded().await;
    for sql in [
        "INSERT INTO claims (kind, worker_id, acquired_at, renewed_at) VALUES ('implementation', 1, 0, 0)",
        "INSERT INTO claims (kind, task_id, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('implementation', 2, 1, 1, 0, 0)",
        "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at) VALUES ('review', 2, 1, 0, 0)",
    ] {
        let error = refused(&store, sql).await;
        assert_eq!(
            error.constraint().as_deref(),
            Some("claims_target_matches_kind"),
            "{sql}"
        );
    }
}

#[tokio::test]
async fn one_active_task_workspace_per_task() {
    let (_dir, store) = seeded().await;
    let again = |state: &str, kind: &str, number: u32| {
        format!(
            "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES ({number}, 1, 1, '{kind}', 'ws/x{number}', 1, '{state}', 0)"
        )
    };
    let error = refused(&store, &again("active", "task", 2)).await;
    assert_eq!(error.constraint().as_deref(), Some("workspaces.task_id"));
    let missing = refused(&store, &again("missing", "task", 2)).await;
    assert!(missing.unique_violation());
    exec(&store, &again("archived", "task", 3)).await.unwrap();
    exec(&store, &again("active", "review", 4)).await.unwrap();
    exec(
        &store,
        "UPDATE workspaces SET state = 'abandoned' WHERE id = 1",
    )
    .await
    .unwrap();
    exec(&store, &again("active", "task", 5)).await.unwrap();
    let ownerless = "INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at) VALUES (6, 2, 'task', 'ws/sync', 1, 'active', 0)";
    exec(&store, ownerless).await.unwrap();
}

#[tokio::test]
async fn one_open_proposal_per_task() {
    let (_dir, store) = seeded().await;
    let proposal = |number: u32, state: &str| {
        let reason = if state == "closed" {
            "'replaced'"
        } else {
            "NULL"
        };
        format!(
            "INSERT INTO proposals (number, task_id, author_worker_id, title, state, close_reason, created_at, updated_at) VALUES ({number}, 1, 1, 'p', '{state}', {reason}, 0, 0)"
        )
    };
    let error = refused(&store, &proposal(2, "open")).await;
    assert_eq!(error.constraint().as_deref(), Some("proposals.task_id"));
    exec(&store, &proposal(2, "closed")).await.unwrap();
    exec(&store, &proposal(3, "integrated")).await.unwrap();
    exec(
        &store,
        "UPDATE proposals SET state = 'closed', close_reason = 'replaced' WHERE id = 1",
    )
    .await
    .unwrap();
    exec(&store, &proposal(4, "open")).await.unwrap();
}

#[tokio::test]
async fn proposal_versions_are_unique_per_proposal() {
    let (_dir, store) = seeded().await;
    let version = |n: u32| {
        format!(
            "INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, {n}, 1, 't', 1, 'frontend', 0)"
        )
    };
    let error = refused(&store, &version(1)).await;
    assert!(error.unique_violation());
    exec(&store, &version(2)).await.unwrap();
    let zero = refused(&store, &version(0)).await;
    assert_eq!(
        zero.constraint().as_deref(),
        Some("proposal_versions_version_positive")
    );
}

#[tokio::test]
async fn dependency_self_edge_is_refused() {
    let (_dir, store) = seeded().await;
    let error = refused(
        &store,
        "INSERT INTO dependencies (task_id, needs_task_id, origin, created_by, created_at) VALUES (1, 1, 'direct', 'x', 0)",
    )
    .await;
    assert_eq!(
        error.constraint().as_deref(),
        Some("dependencies_no_self_edge")
    );
    exec(
        &store,
        "INSERT INTO dependencies (task_id, needs_task_id, origin, created_by, created_at) VALUES (1, 2, 'direct', 'x', 0)",
    )
    .await
    .unwrap();
    let origin = refused(
        &store,
        "INSERT INTO dependencies (task_id, needs_task_id, origin, created_by, created_at) VALUES (2, 1, 'other', 'x', 0)",
    )
    .await;
    assert_eq!(
        origin.constraint().as_deref(),
        Some("dependencies_origin_values")
    );
}

#[tokio::test]
async fn review_needs_exactly_one_reviewer() {
    let (_dir, store) = seeded().await;
    let review = |worker: &str, human: &str| {
        format!(
            "INSERT INTO reviews (version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at) VALUES (1, {worker}, {human}, 'approve', 'b', 0)"
        )
    };
    for sql in [review("NULL", "NULL"), review("1", "'human:ana'")] {
        let error = refused(&store, &sql).await;
        assert_eq!(
            error.constraint().as_deref(),
            Some("reviews_exactly_one_reviewer")
        );
    }
    exec(&store, &review("NULL", "'human:ana'")).await.unwrap();
    exec(&store, &review("1", "NULL")).await.unwrap();
}

#[tokio::test]
async fn message_recipient_needs_exactly_one_target() {
    let (_dir, store) = seeded().await;
    let recipient = |worker: &str, human: &str| {
        format!(
            "INSERT INTO message_recipients (message_id, worker_id, human, addressed_as) VALUES (1, {worker}, {human}, 'agent')"
        )
    };
    for sql in [recipient("NULL", "NULL"), recipient("1", "'human:ana'")] {
        let error = refused(&store, &sql).await;
        assert_eq!(
            error.constraint().as_deref(),
            Some("message_recipients_exactly_one_target")
        );
    }
    exec(&store, &recipient("1", "NULL")).await.unwrap();
    let duplicate = refused(&store, &recipient("1", "NULL")).await;
    assert!(duplicate.unique_violation());
    exec(&store, &recipient("NULL", "'human:ana'"))
        .await
        .unwrap();
    let duplicate = refused(&store, &recipient("NULL", "'human:ana'")).await;
    assert!(duplicate.unique_violation());
}

#[tokio::test]
async fn message_sender_matches_its_kind() {
    let (_dir, store) = seeded().await;
    let message = |number: u32, kind: &str, worker: &str, human: &str| {
        format!(
            "INSERT INTO messages (number, thread_id, sender_kind, sender_worker_id, sender_human, body, created_at) VALUES ({number}, 1, '{kind}', {worker}, {human}, 'b', 0)"
        )
    };
    for sql in [
        message(2, "worker", "NULL", "NULL"),
        message(2, "human", "1", "NULL"),
        message(2, "fold", "1", "NULL"),
        message(2, "fold", "NULL", "'human:ana'"),
    ] {
        let error = refused(&store, &sql).await;
        assert_eq!(
            error.constraint().as_deref(),
            Some("messages_sender_matches_kind"),
            "{sql}"
        );
    }
    exec(&store, &message(2, "worker", "1", "NULL"))
        .await
        .unwrap();
    exec(&store, &message(3, "human", "NULL", "'human:ana'"))
        .await
        .unwrap();
    let orphan = refused(
        &store,
        "INSERT INTO messages (number, thread_id, sender_kind, body, created_at) VALUES (4, 99, 'fold', 'b', 0)",
    )
    .await;
    assert_eq!(orphan.constraint(), None);
}

#[tokio::test]
async fn comment_range_check_refuses_start_after_end() {
    let (_dir, store) = seeded().await;
    let comment = |number: u32, start: &str, end: &str, worker: &str, human: &str| {
        format!(
            "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, author_human, path, start_line, end_line, side, body, created_at) VALUES ({number}, 1, 1, {worker}, {human}, 'a.rs', {start}, {end}, 'new', 'b', 0)"
        )
    };
    let backwards = refused(&store, &comment(2, "9", "3", "1", "NULL")).await;
    assert_eq!(
        backwards.constraint().as_deref(),
        Some("review_comments_range_order")
    );
    let zero = refused(&store, &comment(2, "0", "3", "1", "NULL")).await;
    assert_eq!(
        zero.constraint().as_deref(),
        Some("review_comments_range_order")
    );
    let half = refused(&store, &comment(2, "3", "NULL", "1", "NULL")).await;
    assert_eq!(
        half.constraint().as_deref(),
        Some("review_comments_lines_both_or_neither")
    );
    let authors = refused(&store, &comment(2, "1", "2", "1", "'human:ana'")).await;
    assert_eq!(
        authors.constraint().as_deref(),
        Some("review_comments_exactly_one_author")
    );
    exec(&store, &comment(2, "3", "3", "1", "NULL"))
        .await
        .unwrap();
    exec(&store, &comment(3, "3", "9", "NULL", "'human:ana'"))
        .await
        .unwrap();
    exec(
        &store,
        "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, body, created_at, reply_to) VALUES (4, 1, 1, 1, 'whole version', 0, 2)",
    )
    .await
    .unwrap();
    let side = refused(
        &store,
        "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, side, body, created_at) VALUES (5, 1, 1, 1, 'middle', 'b', 0)",
    )
    .await;
    assert_eq!(
        side.constraint().as_deref(),
        Some("review_comments_side_values")
    );
}

#[tokio::test]
async fn integration_columns_match_their_kind() {
    let (_dir, store) = seeded().await;
    let integration = |kind: &str, proposal: &str, version: &str, remote: &str| {
        format!(
            "INSERT INTO integrations (number, kind, proposal_id, version_id, remote_commit, result, actor, created_at) VALUES ((SELECT MAX(number) + 1 FROM integrations), '{kind}', {proposal}, {version}, {remote}, 'refused', 'fold', 0)"
        )
    };
    for sql in [
        integration("proposal", "1", "NULL", "NULL"),
        integration("proposal", "NULL", "1", "NULL"),
        integration("pull", "1", "NULL", "'abc'"),
        integration("pull", "NULL", "NULL", "NULL"),
        integration("pull", "NULL", "1", "'abc'"),
    ] {
        let error = refused(&store, &sql).await;
        assert_eq!(
            error.constraint().as_deref(),
            Some("integrations_columns_match_kind"),
            "{sql}"
        );
    }
    exec(&store, &integration("proposal", "1", "1", "'abc'"))
        .await
        .unwrap();
    exec(&store, &integration("proposal", "1", "1", "NULL"))
        .await
        .unwrap();
    exec(&store, &integration("pull", "NULL", "NULL", "'abc'"))
        .await
        .unwrap();
}

#[tokio::test]
async fn completion_integration_matches_its_kind() {
    let (_dir, store) = seeded().await;
    for sql in [
        "INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (2, 'integration', NULL, 0)",
        "INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (2, 'children', 1, 0)",
    ] {
        let error = refused(&store, sql).await;
        assert_eq!(
            error.constraint().as_deref(),
            Some("completions_integration_matches_kind")
        );
    }
    exec(
        &store,
        "INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (2, 'children', NULL, 0)",
    )
    .await
    .unwrap();
    let again = refused(
        &store,
        "INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (2, 'children', NULL, 0)",
    )
    .await;
    assert!(again.unique_violation());
}

#[tokio::test]
async fn import_source_is_only_on_the_first_revision() {
    let (_dir, store) = seeded().await;
    let revision = |number: u32, source: &str| {
        format!(
            "INSERT INTO revisions (number, hash, tree_hash, parent_id, import_source, created_at) VALUES ({number}, 'h{number}', 't', 1, {source}, 0)"
        )
    };
    let error = refused(&store, &revision(2, "'/src/app'")).await;
    assert_eq!(
        error.constraint().as_deref(),
        Some("revisions_import_source_first_only")
    );
    exec(&store, &revision(2, "NULL")).await.unwrap();
    let duplicate_hash = refused(
        &store,
        "INSERT INTO revisions (number, hash, tree_hash, parent_id, created_at) VALUES (3, 'h2', 't', 1, 0)",
    )
    .await;
    assert_eq!(
        duplicate_hash.constraint().as_deref(),
        Some("revisions.hash")
    );
    let (_dir, _paths, fresh) = open().await;
    exec(
        &fresh,
        "INSERT INTO revisions (number, hash, tree_hash, import_source, created_at) VALUES (1, 'h1', 't', '/src/app', 0)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn events_keep_rows_when_their_task_is_deleted() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO events (type, actor, task_id, proposal_id, workspace_id, revision_id, data, created_at) VALUES ('task.created', 'fold', 12, 3, 4, 5, '{}', 0)",
    )
    .await
    .unwrap();
    exec(&store, "INSERT INTO dependencies (task_id, needs_task_id, origin, created_by, created_at) VALUES (1, 2, 'direct', 'x', 0)")
        .await
        .unwrap();
    exec(&store, "DELETE FROM tasks WHERE id = 2")
        .await
        .unwrap();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM dependencies").await, 0);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM events WHERE task_id = 12").await,
        1
    );
    // A message is never deleted, so `message_id` is the one reference column with a foreign key.
    let foreign_keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT f.\"from\", f.\"table\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f WHERE m.name = 'events'",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    assert_eq!(
        foreign_keys,
        [("message_id".to_owned(), "messages".to_owned())]
    );
    let bad_data = refused(
        &store,
        "INSERT INTO events (type, actor, data, created_at) VALUES ('task.created', 'fold', '[1]', 0)",
    )
    .await;
    assert_eq!(
        bad_data.constraint().as_deref(),
        Some("events_data_is_object")
    );
}

#[tokio::test]
async fn other_foreign_keys_are_enforced() {
    let (_dir, store) = seeded().await;
    for sql in [
        "INSERT INTO workers (agent_id, name, created_at) VALUES (99, 'ghost', 0)",
        "INSERT INTO agent_tags (agent_id, tag, created_at) VALUES (99, 'review', 0)",
        "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (9, 99, 1, 'p', 0, 0)",
        "INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at) VALUES (9, 2, 'task', 'p', 99, 'active', 0)",
        "INSERT INTO refs (name, revision_id) VALUES ('main', 99)",
        "INSERT INTO task_labels (task_id, label_id) VALUES (1, 99)",
        "DELETE FROM agents WHERE id = 1",
        "DELETE FROM revisions WHERE id = 1",
    ] {
        let error = refused(&store, sql).await;
        let message = error.to_string();
        assert!(
            message.contains("FOREIGN KEY") || message.contains("append-only"),
            "{sql}: {message}"
        );
    }
    exec(
        &store,
        "INSERT INTO refs (name, revision_id) VALUES ('main', 1)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn status_and_flags_are_checked_columns() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "UPDATE tasks SET status = 'review', blocked = 1 WHERE id = 1",
    )
    .await
    .unwrap();
    for sql in [
        "UPDATE tasks SET blocked = 2 WHERE id = 1",
        "UPDATE tasks SET human_review = 2 WHERE id = 1",
        "UPDATE proposals SET state = 'merged' WHERE id = 1",
        "UPDATE workspaces SET state = 'gone' WHERE id = 1",
    ] {
        assert!(exec(&store, sql).await.is_err(), "{sql}");
    }
    let defaults = count(
        &store,
        "SELECT COUNT(*) FROM tasks WHERE id = 2 AND status = 'backlog' AND blocked = 0 AND human_review = 0",
    )
    .await;
    assert_eq!(defaults, 1);
}

#[tokio::test]
async fn label_links_go_with_their_task_or_their_label() {
    let (_dir, store) = seeded().await;
    for sql in [
        "INSERT INTO labels (id, name, color, created_at) VALUES (11, 'a', 'sage', 0), (12, 'b', 'teal', 0)",
        "INSERT INTO task_labels (task_id, label_id) VALUES (1, 11), (1, 12), (2, 11), (2, 12)",
        "DELETE FROM labels WHERE id = 11",
    ] {
        exec(&store, sql).await.unwrap();
    }
    assert_eq!(count(&store, "SELECT COUNT(*) FROM task_labels").await, 2);
    exec(&store, "DELETE FROM tasks WHERE id = 2")
        .await
        .unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM task_labels WHERE task_id = 1 AND label_id = 12"
        )
        .await,
        1
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM task_labels").await, 1);
}

#[tokio::test]
async fn task_titles_label_names_and_colours_are_checked() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO labels (name, color, created_at) VALUES ('a', 'sage', 0)",
    )
    .await
    .unwrap();
    for (sql, rule) in [
        (
            "INSERT INTO tasks (number, title, created_by, created_at, updated_at) VALUES (9, '  ', 'x', 0, 0)",
            "tasks_title_single_line",
        ),
        (
            "UPDATE tasks SET title = 'a' || char(10) || 'b' WHERE id = 1",
            "tasks_title_single_line",
        ),
        (
            "UPDATE tasks SET title = 'a' || char(13) WHERE id = 1",
            "tasks_title_single_line",
        ),
        (
            "INSERT INTO labels (name, color, created_at) VALUES ('', 'sage', 0)",
            "labels_name_single_line",
        ),
        (
            "UPDATE labels SET name = 'x' || char(10) WHERE name = 'a'",
            "labels_name_single_line",
        ),
        (
            "INSERT INTO labels (name, color, created_at) VALUES ('b', 'green', 0)",
            "labels_color_values",
        ),
        (
            "UPDATE labels SET color = 'Sage' WHERE name = 'a'",
            "labels_color_values",
        ),
    ] {
        let message = refused(&store, sql).await.to_string();
        assert!(message.contains(rule), "{sql}: {message}");
    }
    for color in [
        "sage", "slate", "teal", "ink", "gold", "moss", "plum", "stone",
    ] {
        exec(
            &store,
            &format!("UPDATE labels SET color = '{color}' WHERE name = 'a'"),
        )
        .await
        .unwrap();
    }
    exec(&store, "UPDATE tasks SET title = 'renamed' WHERE id = 1")
        .await
        .unwrap();
}

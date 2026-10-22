use std::borrow::Cow;

use crate::common::{count, exec, open};
use crate::seed;
use fold_core::{CheckpointId, Timestamp, WorkspaceId};
use fold_store::workspaces::{
    CheckpointKind, CheckpointReason, insert_checkpoint, set_sync_checkpoint,
};
use fold_store::{Store, StoreError};
use sqlx::Connection;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

/// Tasks `T-1` and `T-2` with workspaces `ws-1` and `ws-2`, a changeset in each, proposal `P-1` on `T-1` with versions 1 and 2, proposal `P-2` on `T-2`, a review on each proposal's first version and comment `rc-1` on `P-1`.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0), (2, 2, 'two', 'backend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO refs (name, revision_id) VALUES ('main', 1);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'backend', 0), (2, 2, 2, 2, 1, 't3', 'm', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0), (2, 2, 2, 1, 'q', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'backend', 0), (2, 1, 2, 1, 't2', 1, 'backend', 0), (3, 2, 1, 1, 't3', 2, 'backend', 0);
INSERT INTO reviews (id, version_id, reviewer_human, verdict, body, created_at) VALUES (1, 1, 'ana', 'comment', 'b', 0), (2, 3, 'ana', 'comment', 'b', 0);
INSERT INTO review_comments (number, version_id, review_id, author_human, body, created_at) VALUES (1, 1, 1, 'ana', 'b', 0);
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

#[cfg(test)]
fn integration(number: u32, result: &str) -> String {
    format!(
        "INSERT INTO integrations (number, kind, proposal_id, version_id, result, actor, created_at) VALUES ({number}, 'proposal', 1, 2, '{result}', 'human:ana', 0)"
    )
}

/// The migration that adds these rules applies to a database that already holds valid rows of every table it guards.
#[tokio::test]
async fn the_rules_apply_to_a_populated_database() {
    let dir = tempfile::TempDir::new().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("fold.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    let mut before: Migrator = sqlx::migrate!("./migrations/project");
    let rules = before
        .migrations
        .iter()
        .position(|migration| migration.description == "store rules")
        .unwrap();
    before.migrations = Cow::Owned(before.migrations[..rules].to_vec());
    before.run(&mut conn).await.unwrap();
    let rows = format!(
        "{SEED}
        INSERT INTO integrations (number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 'proposal', 1, 1, 'conflicted', 'human:ana', 0), (2, 'proposal', 1, 2, 'succeeded', 'human:ana', 0);
        INSERT INTO review_comments (number, version_id, review_id, author_human, reply_to, body, created_at) VALUES (2, 2, 1, 'ana', 1, 'b', 0);
        INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, reason, created_at, pruned_at) VALUES (1, 1, 't2', 'auto', 'quiet', 0, NULL), (2, 1, 't2', 'auto', 'quiet', 0, 5);
        UPDATE workspaces SET sync_checkpoint_id = 1 WHERE id = 1;
        UPDATE tasks SET human_review = 1, human_review_reason = 'risky', human_review_set_by = 'human:ana' WHERE id = 2;"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(rows))
        .execute(&mut conn)
        .await
        .unwrap();

    let after: Migrator = sqlx::migrate!("./migrations/project");
    after.run(&mut conn).await.unwrap();

    let kept: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM integrations) + (SELECT COUNT(*) FROM review_comments) + (SELECT COUNT(*) FROM checkpoints)",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(kept, 6);
    let second = sqlx::query(sqlx::AssertSqlSafe(integration(3, "succeeded")))
        .execute(&mut conn)
        .await
        .unwrap_err();
    assert!(second.to_string().contains("UNIQUE"), "{second}");
}

#[tokio::test]
async fn a_proposal_has_one_succeeded_integration() {
    let (_dir, store) = seeded().await;
    exec(&store, &integration(1, "refused")).await.unwrap();
    exec(&store, &integration(2, "succeeded")).await.unwrap();
    let error = exec(&store, &integration(3, "succeeded"))
        .await
        .unwrap_err();
    assert!(error.unique_violation(), "{error}");
    exec(&store, &integration(4, "conflicted")).await.unwrap();
}

#[tokio::test]
async fn a_reply_stays_on_the_proposal_of_its_comment() {
    let (_dir, store) = seeded().await;
    let reply = |number: u32, version: u32, review: u32| {
        format!(
            "INSERT INTO review_comments (number, version_id, review_id, author_human, reply_to, body, created_at) VALUES ({number}, {version}, {review}, 'ana', 1, 'b', 0)"
        )
    };
    exec(&store, &reply(2, 2, 1)).await.unwrap();
    let message = refused(&store, &reply(3, 3, 2)).await;
    assert!(
        message.contains("review_comments_reply_same_proposal"),
        "{message}"
    );
}

#[tokio::test]
async fn a_checkpoint_has_a_reason_and_changes_only_by_its_pruning() {
    let (_dir, store) = seeded().await;
    let message = refused(
        &store,
        "INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, created_at) VALUES (1, 1, 't2', 'auto', 0)",
    )
    .await;
    assert!(message.contains("checkpoints_reason_required"), "{message}");
    exec(
        &store,
        "INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, reason, created_at) VALUES (1, 1, 't2', 'auto', 'quiet', 0)",
    )
    .await
    .unwrap();
    for sql in [
        "UPDATE checkpoints SET label = 'x' WHERE number = 1",
        "UPDATE checkpoints SET pruned_at = NULL WHERE number = 1",
    ] {
        let message = refused(&store, sql).await;
        assert!(
            message.contains("checkpoints_prune_only"),
            "{sql}: {message}"
        );
    }
    exec(
        &store,
        "UPDATE checkpoints SET pruned_at = 5 WHERE number = 1",
    )
    .await
    .unwrap();
    let message = refused(
        &store,
        "UPDATE checkpoints SET pruned_at = 6 WHERE number = 1",
    )
    .await;
    assert!(message.contains("checkpoints_prune_only"), "{message}");
}

#[tokio::test]
async fn a_sync_checkpoint_belongs_to_its_workspace() {
    let project = fold_store::testing::TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    let now = project.now();
    let taken = |workspace: WorkspaceId| async move {
        store
            .write(async |tx| {
                insert_checkpoint(
                    tx,
                    workspace,
                    seed::tree("files"),
                    CheckpointKind::Auto,
                    CheckpointReason::BeforeSync,
                    None,
                    now,
                )
                .await
            })
            .await
            .unwrap()
            .id
    };
    let of_two: CheckpointId = taken(two).await;
    let set = |checkpoint| async move {
        store
            .write(async |tx| set_sync_checkpoint(tx, one, Some(checkpoint)).await)
            .await
            .unwrap()
    };
    assert!(!set(of_two).await);
    assert_eq!(
        count(
            store,
            "SELECT COUNT(*) FROM workspaces WHERE sync_checkpoint_id IS NOT NULL"
        )
        .await,
        0
    );
    assert!(set(taken(one).await).await);
    let message = refused(
        store,
        "UPDATE workspaces SET sync_checkpoint_id = (SELECT id FROM checkpoints WHERE number = 1) WHERE number = 1",
    )
    .await;
    assert!(
        message.contains("workspaces_sync_checkpoint_own"),
        "{message}"
    );
}

#[tokio::test]
async fn main_is_the_only_ref_and_it_stays() {
    let (_dir, store) = seeded().await;
    for (sql, rule) in [
        (
            "INSERT INTO refs (name, revision_id) VALUES ('dev', 1)",
            "refs_main_only",
        ),
        ("UPDATE refs SET name = 'dev'", "refs_name_fixed"),
        ("DELETE FROM refs", "refs keeps main"),
    ] {
        let message = refused(&store, sql).await;
        assert!(message.contains(rule), "{sql}: {message}");
    }
}

#[tokio::test]
async fn a_changeset_records_its_workspace_task_and_parent() {
    let (_dir, store) = seeded().await;
    let changeset = |number: u32, task: u32, parent: &str| {
        format!(
            "INSERT INTO changesets (number, workspace_id, task_id, parent_changeset_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES ({number}, 1, {task}, {parent}, 1, 't4', 'm', 'backend', 0)"
        )
    };
    let message = refused(&store, &changeset(3, 2, "1")).await;
    assert!(
        message.contains("changesets_task_of_workspace"),
        "{message}"
    );
    let message = refused(&store, &changeset(3, 1, "2")).await;
    assert!(
        message.contains("changesets_parent_same_workspace"),
        "{message}"
    );
    exec(&store, &changeset(3, 1, "1")).await.unwrap();
}

#[tokio::test]
async fn the_human_review_mark_keeps_its_shape() {
    let (_dir, store) = seeded().await;
    for sql in [
        "UPDATE tasks SET human_review = 1 WHERE number = 1",
        "UPDATE tasks SET human_review = 1, human_review_reason = 'r' WHERE number = 1",
        "UPDATE tasks SET human_review_reason = 'r' WHERE number = 1",
        "INSERT INTO tasks (number, title, created_by, created_at, updated_at, human_reviewer) VALUES (3, 't', 'b', 0, 0, 'ana')",
    ] {
        let message = refused(&store, sql).await;
        assert!(
            message.contains("tasks_human_review_shape"),
            "{sql}: {message}"
        );
    }
    exec(
        &store,
        "UPDATE tasks SET human_review = 1, human_review_reason = 'r', human_review_set_by = 'human:ana' WHERE number = 1",
    )
    .await
    .unwrap();
    exec(
        &store,
        "UPDATE tasks SET human_review = 0, human_review_reason = NULL, human_review_set_by = NULL WHERE number = 1",
    )
    .await
    .unwrap();
}

#[cfg(test)]
async fn registry() -> (tempfile::TempDir, Store) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open_registry(&dir.path().join("registry.db"))
        .await
        .unwrap();
    (dir, store)
}

#[tokio::test]
async fn a_session_acts_for_the_human_of_its_token() {
    let (_dir, store) = registry().await;
    for sql in [
        "INSERT INTO humans (id, name, created_at) VALUES (1, 'ana', 0), (2, 'bo', 0)",
        "INSERT INTO web_tokens (id, number, token_hash, human_id, created_at) VALUES (1, 1, x'01', 1, 0)",
    ] {
        exec(&store, sql).await.unwrap();
    }
    let session = |hash: &str, human: u32| {
        format!(
            "INSERT INTO web_sessions (session_hash, token_id, human_id, created_at, last_used_at, expires_at) VALUES (x'{hash}', 1, {human}, 0, 0, 10)"
        )
    };
    let message = refused(&store, &session("0a", 2)).await;
    assert!(message.contains("web_sessions_human_of_token"), "{message}");
    exec(&store, &session("0b", 1)).await.unwrap();
    for (sql, rule) in [
        (
            "UPDATE web_sessions SET human_id = 2",
            "web_sessions_human_of_token",
        ),
        (
            "UPDATE web_tokens SET human_id = 2",
            "web_tokens_human_fixed",
        ),
    ] {
        let message = refused(&store, sql).await;
        assert!(message.contains(rule), "{sql}: {message}");
    }
}

#[tokio::test]
async fn a_session_without_a_token_comes_from_a_sign_in_link_and_takes_any_human() {
    let (_dir, store) = registry().await;
    for sql in [
        "INSERT INTO humans (id, name, created_at) VALUES (1, 'ana', 0), (2, 'bo', 0)",
        "INSERT INTO sign_in_links (link_hash, human_id, created_at, expires_at) VALUES (x'01', 2, 0, 10)",
        "INSERT INTO web_sessions (session_hash, token_id, human_id, created_at, last_used_at, expires_at) VALUES (x'0c', NULL, 2, 0, 0, 10)",
        "UPDATE web_sessions SET human_id = 1",
    ] {
        exec(&store, sql).await.unwrap();
    }
    let message = refused(
        &store,
        "INSERT INTO sign_in_links (link_hash, human_id, created_at, expires_at) VALUES (x'02', 1, 10, 10)",
    )
    .await;
    assert!(message.contains("CHECK"), "{message}");
    let message = refused(
        &store,
        "INSERT INTO sign_in_links (link_hash, human_id, created_at, expires_at) VALUES (x'01', 1, 0, 10)",
    )
    .await;
    assert!(message.contains("UNIQUE"), "{message}");
}

#[tokio::test]
async fn human_names_and_slugs_keep_their_pattern() {
    let (_dir, store) = registry().await;
    let at = Timestamp::from_millis(0).unwrap();
    for name in ["ana", "9lives", "a_b-c", "abcdefghijklmnopqrstuvwxyz012345"] {
        exec(
            &store,
            &format!(
                "INSERT INTO humans (name, created_at) VALUES ('{name}', {})",
                at.as_millis()
            ),
        )
        .await
        .unwrap();
    }
    for name in [
        "",
        "Ana",
        "-ana",
        "_ana",
        "a b",
        "abcdefghijklmnopqrstuvwxyz0123456",
    ] {
        let message = refused(
            &store,
            &format!("INSERT INTO humans (name, created_at) VALUES ('{name}', 0)"),
        )
        .await;
        assert!(
            message.contains("humans_name_pattern"),
            "{name:?}: {message}"
        );
    }
    let message = refused(&store, "UPDATE humans SET name = 'Ana' WHERE name = 'ana'").await;
    assert!(message.contains("humans_name_pattern"), "{message}");
    let project = |slug: &str, root: &str| {
        format!(
            "INSERT INTO projects (slug, name, root, created_at) VALUES ('{slug}', 'n', '{root}', 0)"
        )
    };
    for (slug, root) in [("project", "/a"), ("my-app-2", "/b"), ("x", "/c")] {
        exec(&store, &project(slug, root)).await.unwrap();
    }
    for slug in ["", "My-App", "-app", "app-", "my--app", "my_app", "my app"] {
        let message = refused(&store, &project(slug, "/z")).await;
        assert!(
            message.contains("projects_slug_pattern"),
            "{slug:?}: {message}"
        );
    }
    let message = refused(&store, "UPDATE projects SET slug = 'Bad' WHERE slug = 'x'").await;
    assert!(message.contains("projects_slug_pattern"), "{message}");
}

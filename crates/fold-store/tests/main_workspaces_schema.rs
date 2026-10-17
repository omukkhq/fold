#![cfg(test)]

use fold_core::object::{ObjectKind, hash_object};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{ChangesetId, IntegrationId, RevisionId, Timestamp, WorkspaceId};
use fold_store::reviews::{IntegrationSubject, NewIntegration, insert_succeeded};
use fold_store::testing::assert_append_only;
use fold_store::workspaces::{
    self, IntegrationKind, NewMainWorkspace, WorkspaceKind, create_initial_revision,
    initial_revision,
};
use fold_store::{Store, StoreError};

use crate::common::{count, exec, open};

/// A task workspace with a changeset that a proposal integrated as `r2`, with a checkpoint, a conflict and the task's completion, as a project holds them before the main workspace exists.
const OLD_ROWS: &str = r#"
BEGIN;
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO refs (name, revision_id) VALUES ('main', 1);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (7, 3, 1, 1, 'task', '.fold/ws/t-1', 1, 'active', 0);
INSERT INTO checkpoints (id, number, workspace_id, tree_hash, kind, reason, created_at) VALUES (4, 2, 7, 't2', 'auto', 'before_record', 0);
INSERT INTO workspace_conflicts (workspace_id, path, kind) VALUES (7, X'612E7273', 'both_modified_text');
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (5, 9, 7, 1, 1, 't2', 'm', '{"files_added":1,"files_modified":0,"files_deleted":0}', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 5, 'backend', 0);
INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, revision_id, actor, created_at) VALUES (6, 1, 'proposal', 1, 1, 'succeeded', 2, 'review', 0);
INSERT INTO revisions (id, number, hash, tree_hash, parent_id, integration_id, created_at) VALUES (2, 2, 'h2', 't2', 1, 6, 0);
INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (1, 'integration', 6, 0);
COMMIT;
"#;

#[tokio::test]
async fn the_rebuilt_tables_keep_every_row_id_and_reference() {
    use sqlx::Connection;
    let dir = tempfile::TempDir::new().unwrap();
    let paths = fold_core::ProjectPaths::new(dir.path().join("project"));
    std::fs::create_dir_all(paths.db().parent().unwrap()).unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(paths.db())
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let mut before: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/project");
    let rebuild = before
        .migrations
        .iter()
        .position(|migration| migration.description == "main workspaces")
        .unwrap();
    before.migrations = std::borrow::Cow::Owned(before.migrations[..rebuild].to_vec());
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(OLD_ROWS).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM workspaces WHERE id = 7 AND number = 3 AND task_id = 1 AND kind = 'task'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM changesets WHERE id = 5 AND number = 9 AND workspace_id = 7 AND task_id = 1"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM integrations WHERE id = 6 AND number = 1 AND kind = 'proposal' AND revision_id = 2 AND changeset_id IS NULL AND message IS NULL"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT (SELECT COUNT(*) FROM checkpoints WHERE workspace_id = 7) + (SELECT COUNT(*) FROM workspace_conflicts WHERE workspace_id = 7) + (SELECT COUNT(*) FROM proposal_versions WHERE head_changeset_id = 5) + (SELECT COUNT(*) FROM completions WHERE integration_id = 6) + (SELECT COUNT(*) FROM revisions WHERE integration_id = 6)"
        )
        .await,
        5
    );
    let broken = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert!(broken.is_empty());
    for table in ["changesets", "integrations"] {
        assert_append_only(&store, table).await;
    }
    let moved = exec(&store, "UPDATE workspaces SET task_id = NULL WHERE id = 7").await;
    assert!(
        moved
            .unwrap_err()
            .to_string()
            .contains("workspaces_task_fixed")
    );
}

fn now() -> Timestamp {
    Timestamp::from_millis(1_700_000_000_000).unwrap()
}

const FIXTURE: &str = r#"
INSERT INTO agents (id, name, created_at) VALUES (1, 'main', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'main', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', '.fold/ws/t-1', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (1, 5, 1, 1, 1, 't2', 'm', '{"files_added":1,"files_modified":0,"files_deleted":0}', 'main', 0);
"#;

fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

fn changeset(number: u32) -> ChangesetId {
    ChangesetId::from_number(number).unwrap()
}

/// A project with `r1`, a task workspace `ws-1` with the changeset `C-5`, and the main workspace `ws-2` with the changeset `C-1`, which has no task.
async fn seeded() -> (tempfile::TempDir, Store, StoredRevision) {
    let (dir, _paths, store) = open().await;
    let tree = hash_object(ObjectKind::Tree, b"r1");
    let first = StoredRevision::confirmed(initial_revision(tree, now()).unwrap());
    create_initial_revision(&store, &first, None).await.unwrap();
    exec(&store, FIXTURE).await.unwrap();
    let r1 = RevisionId::from_number(1).unwrap();
    store
        .write(async |tx| {
            workspaces::insert_main_workspace(
                tx,
                &NewMainWorkspace {
                    id: ws(2),
                    worker_id: 1,
                    path: ".fold/ws/main",
                    base: r1,
                    now: now(),
                },
            )
            .await?;
            workspaces::insert_changeset(
                tx,
                ws(2),
                None,
                r1,
                hash_object(ObjectKind::Tree, b"main"),
                "rules",
                Default::default(),
                "main",
                now(),
            )
            .await
        })
        .await
        .unwrap();
    (dir, store, first)
}

fn constraint(error: StoreError) -> String {
    error.constraint().unwrap_or_else(|| error.to_string())
}

#[tokio::test]
async fn the_main_workspace_has_no_task_and_there_is_one_live_main_workspace() {
    let (_dir, store, _) = seeded().await;
    let read = store.read();
    let main = workspaces::live_main_workspace(read)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(main.id, ws(2));
    assert_eq!(main.kind, WorkspaceKind::Main);
    assert_eq!(main.task, None);
    assert_eq!(
        workspaces::main_workspace_paths(read).await.unwrap(),
        [".fold/ws/main"]
    );
    let recorded = workspaces::latest_changeset(read, ws(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recorded.id, changeset(1));
    assert_eq!(recorded.task, None);

    let second = store
        .write(async |tx| {
            workspaces::insert_main_workspace(
                tx,
                &NewMainWorkspace {
                    id: ws(3),
                    worker_id: 1,
                    path: ".fold/ws/main-2",
                    base: RevisionId::from_number(1).unwrap(),
                    now: now(),
                },
            )
            .await
        })
        .await
        .unwrap_err();
    assert!(second.unique_violation(), "{second}");
    for (sql, rule) in [
        (
            "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (4, NULL, 1, 'task', '.fold/ws/x', 1, 'active', 0)",
            "workspaces_task_unless_main",
        ),
        (
            "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (4, 1, 1, 'main', '.fold/ws/y', 1, 'archived', 0)",
            "workspaces_task_unless_main",
        ),
        (
            "UPDATE workspaces SET kind = 'task', task_id = 1 WHERE number = 2",
            "workspaces_task_fixed",
        ),
        (
            "UPDATE workspaces SET kind = 'task' WHERE number = 2",
            "workspaces_task_unless_main",
        ),
        (
            "INSERT INTO changesets (number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (5, 2, 1, 1, 't', 'm', 'main', 0)",
            "changesets_task_of_workspace",
        ),
        (
            "INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0); INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 't', (SELECT id FROM changesets WHERE number = 1), 'main', 0)",
            "proposal_versions_task_changeset",
        ),
    ] {
        let refused = exec(&store, sql).await.unwrap_err();
        assert!(constraint(refused).contains(rule), "{sql}");
    }
}

fn direct(number: u32, of: ChangesetId) -> NewIntegration<'static> {
    NewIntegration {
        number: IntegrationId::from_number(number).unwrap(),
        subject: IntegrationSubject::Direct {
            changeset: of,
            message: "Add the agent rules",
        },
        actor: "main",
        created_at: now(),
    }
}

fn second(first: &StoredRevision, number: u32) -> StoredRevision {
    StoredRevision::confirmed(Revision {
        tree: hash_object(ObjectKind::Tree, b"main"),
        parent: Some(first.hash()),
        number: RevisionId::from_number(2).unwrap(),
        integration: Some(IntegrationId::from_number(number).unwrap()),
        created_at: now(),
    })
}

#[tokio::test]
async fn a_direct_integration_names_a_changeset_of_the_main_workspace_and_its_message() {
    let (_dir, store, first) = seeded().await;
    let task_work = store
        .write(async |tx| insert_succeeded(tx, &direct(1, changeset(5)), &second(&first, 1)).await)
        .await
        .unwrap_err();
    assert!(constraint(task_work).contains("integrations_direct_main_changeset"));

    store
        .write(async |tx| insert_succeeded(tx, &direct(2, changeset(1)), &second(&first, 2)).await)
        .await
        .unwrap();
    let row = workspaces::get_by_number(store.read(), RevisionId::from_number(2).unwrap())
        .await
        .unwrap();
    let integration = row.integration.unwrap();
    assert_eq!(integration.kind, IntegrationKind::Direct);
    assert_eq!(integration.changeset, Some(changeset(1)));
    assert_eq!(integration.message.as_deref(), Some("Add the agent rules"));
    assert_eq!(integration.remote_commit, None);
    assert_eq!(integration.proposal, None);

    for (sql, rule) in [
        (
            "INSERT INTO integrations (number, kind, changeset_id, message, result, actor, created_at) VALUES (3, 'direct', 2, 'again', 'succeeded', 'main', 0)",
            "integrations.changeset_id",
        ),
        (
            "INSERT INTO integrations (number, kind, changeset_id, message, result, actor, created_at) VALUES (3, 'direct', 2, ' ', 'refused', 'main', 0)",
            "integrations_columns_match_kind",
        ),
        (
            "INSERT INTO integrations (number, kind, changeset_id, result, actor, created_at) VALUES (3, 'direct', 2, 'succeeded', 'main', 0)",
            "integrations_columns_match_kind",
        ),
        (
            "INSERT INTO integrations (number, kind, remote_commit, changeset_id, message, result, actor, created_at) VALUES (3, 'pull', 'abc', 2, 'm', 'succeeded', 'main', 0)",
            "integrations_columns_match_kind",
        ),
    ] {
        let refused = exec(&store, sql).await.unwrap_err();
        assert!(constraint(refused).contains(rule), "{sql}");
    }
}

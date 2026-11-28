use std::path::Path;

use fold_core::object::ObjectHash;
use fold_core::reviews::checks::{Purpose, RunState, Stage};
use fold_core::{
    ChangesetId, CheckRunId, ProjectPaths, ProposalId, ReviewBatchId, RevisionId, TaskId,
    Timestamp, WorkspaceId,
};
use fold_store::reviews::checks::{self, NewCheckRun};
use fold_store::{Store, StoreError};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

use crate::common::{count, exec, open};

/// A task `T-1` with a task workspace and its changeset `C-1`, a proposal `P-1` with v1, and the main workspace with its changeset `C-2`.
const SEED: &str = r#"
INSERT INTO agents (id, name, created_at) VALUES (1, 'main', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'main', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'main', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, NULL, 1, 'main', 'ws/main', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', '{"files_added":1,"files_modified":0,"files_deleted":0}', 'main', 0), (2, 2, 2, NULL, 1, 't3', 'm', '{"files_added":1,"files_modified":0,"files_deleted":0}', 'main', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'main', 0);
"#;

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
fn run_id(number: u32) -> CheckRunId {
    CheckRunId::from_number(number).unwrap()
}

#[cfg(test)]
fn changeset(number: u32) -> Option<ChangesetId> {
    ChangesetId::from_number(number)
}

#[cfg(test)]
fn direct_run(number: u32) -> NewCheckRun<'static> {
    NewCheckRun {
        id: run_id(number),
        stage: Stage::Integration,
        purpose: Purpose::IntegrateDirect,
        task: None,
        workspace: None,
        worker_id: None,
        proposal: None,
        version: None,
        base: RevisionId::from_number(1).unwrap(),
        tree: ObjectHash::from_bytes([5; 32]),
        actor: "fold",
        title: None,
        evidence: None,
        intent: None,
        batch: None,
        changeset: changeset(2),
    }
}

#[cfg(test)]
async fn insert(store: &Store, new: &NewCheckRun<'_>) -> Result<(), StoreError> {
    store
        .write(async |tx| checks::insert_run(tx, new, at(1)).await)
        .await
}

#[tokio::test]
async fn a_direct_change_run_has_only_its_changeset_and_every_read_returns_it() {
    let (_dir, store) = seeded().await;
    insert(&store, &direct_run(1)).await.unwrap();

    let stored = checks::get_run(store.read(), run_id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.purpose, Purpose::IntegrateDirect);
    assert_eq!(stored.task, None);
    assert_eq!(stored.changeset, changeset(2));
    assert_eq!(stored.proposal, None);
    assert_eq!(stored.subject(), "C-2");
    assert!(matches!(stored.task_run(), Err(StoreError::Invalid(_))));

    let running = checks::running_runs(store.read()).await.unwrap();
    assert_eq!(running.len(), 1, "a restart finds the direct change's run");
    assert_eq!(running[0].id, run_id(1));

    store
        .write(async |tx| checks::finish_run(tx, run_id(1), RunState::Failed, None, at(2)).await)
        .await
        .unwrap();
    insert(&store, &direct_run(2)).await.unwrap();
    let of_changeset = checks::runs_of_changeset(store.read(), changeset(2).unwrap())
        .await
        .unwrap();
    let ids: Vec<_> = of_changeset.iter().map(|run| run.id).collect();
    assert_eq!(ids, [run_id(2), run_id(1)], "newest first");
    assert!(
        checks::runs_of_changeset(store.read(), changeset(1).unwrap())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_shape_of_a_direct_change_run_is_checked() {
    let (_dir, store) = seeded().await;
    let with_task = NewCheckRun {
        task: TaskId::from_number(1),
        ..direct_run(1)
    };
    let with_proposal = NewCheckRun {
        proposal: ProposalId::from_number(1),
        version: Some(1),
        ..direct_run(1)
    };
    let without_changeset = NewCheckRun {
        changeset: None,
        ..direct_run(1)
    };
    let in_a_proposal_stage = NewCheckRun {
        stage: Stage::Proposal,
        ..direct_run(1)
    };
    let with_workspace = NewCheckRun {
        workspace: WorkspaceId::from_number(2),
        ..direct_run(1)
    };
    exec(
        &store,
        "INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (1, 1, 1, 0)",
    )
    .await
    .unwrap();
    let with_batch = NewCheckRun {
        batch: ReviewBatchId::from_number(1),
        ..direct_run(1)
    };
    let proposal_run_with_changeset = NewCheckRun {
        stage: Stage::Proposal,
        purpose: Purpose::Check,
        task: TaskId::from_number(1),
        workspace: WorkspaceId::from_number(1),
        worker_id: Some(1),
        ..direct_run(1)
    };
    for refused in [
        with_task,
        with_proposal,
        without_changeset,
        in_a_proposal_stage,
        with_workspace,
        with_batch,
        proposal_run_with_changeset,
    ] {
        let error = insert(&store, &refused).await.unwrap_err();
        assert!(error.to_string().contains("CHECK"), "{refused:?}: {error}");
    }

    let task_changeset = insert(
        &store,
        &NewCheckRun {
            changeset: changeset(1),
            ..direct_run(1)
        },
    )
    .await
    .unwrap_err();
    assert!(
        task_changeset
            .to_string()
            .contains("check_runs_direct_main_changeset")
    );
    let missing = insert(
        &store,
        &NewCheckRun {
            changeset: changeset(9),
            ..direct_run(1)
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, StoreError::Invalid(_)), "{missing}");

    let integration = NewCheckRun {
        purpose: Purpose::Integrate,
        task: TaskId::from_number(1),
        changeset: None,
        proposal: ProposalId::from_number(1),
        version: Some(1),
        ..direct_run(1)
    };
    for refused in [
        NewCheckRun {
            proposal: None,
            version: None,
            ..integration.clone()
        },
        NewCheckRun {
            changeset: changeset(2),
            ..integration.clone()
        },
        NewCheckRun {
            task: None,
            ..integration.clone()
        },
    ] {
        assert!(insert(&store, &refused).await.is_err(), "{refused:?}");
    }
    insert(&store, &integration).await.unwrap();
}

#[tokio::test]
async fn a_direct_change_run_ends_once_and_is_never_deleted() {
    let (_dir, store) = seeded().await;
    insert(&store, &direct_run(1)).await.unwrap();
    let moved = exec(&store, "UPDATE check_runs SET changeset_id = 1")
        .await
        .unwrap_err();
    assert!(moved.to_string().contains("check_runs_end_once"), "{moved}");
    assert!(
        store
            .write(
                async |tx| checks::finish_run(tx, run_id(1), RunState::Passed, None, at(2)).await
            )
            .await
            .unwrap()
    );
    let again = exec(&store, "UPDATE check_runs SET state = 'failed'")
        .await
        .unwrap_err();
    assert!(again.to_string().contains("check_runs_end_once"), "{again}");
    let deleted = exec(&store, "DELETE FROM check_runs").await.unwrap_err();
    assert!(deleted.to_string().contains("append-only"), "{deleted}");
}

/// Rows of every kind that refers to a check run, as a project holds them before the migration: a finished proposal run with an intent and its results, a batch's integration run with its members, and the batch integration that names that run.
const OLD_ROWS: &[&str] = &[
    "INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0)",
    "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0)",
    "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0)",
    "INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0)",
    "INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0)",
    "INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', '{\"files_added\":1,\"files_modified\":0,\"files_deleted\":0}', 'backend', 0)",
    "INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0)",
    "INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'backend', 0)",
    "INSERT INTO review_batches (id, number, holder_worker_id, created_at) VALUES (1, 1, 1, 0)",
    "INSERT INTO review_batch_members (id, batch_id, proposal_id, version_id, position) VALUES (1, 1, 1, 1, 1)",
    "UPDATE review_batches SET state = 'submitted', submitted_at = 5 WHERE id = 1",
    "INSERT INTO check_runs (id, number, stage, purpose, task_id, workspace_id, worker_id, base_revision_id, tree_hash, actor, title, evidence, intent, created_at) VALUES (3, 1, 'proposal', 'create_proposal', 1, 1, 1, 1, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'backend', 'Title', 'ran it', '{\"summary\":\"s\",\"parts\":[{\"name\":\"n\",\"does\":\"d\",\"files\":[\"a\"]}]}', 10)",
    "INSERT INTO check_results (run_id, position, kind, name, required, outcome, exit_code, duration_ms, log, log_bytes, truncated) VALUES (3, 0, 'setup', 'setup', 1, 'passed', 0, 4, X'6F6B', 2, 0), (3, 1, 'check', 'test', 1, 'failed', 1, 9, X'6E6F', 20, 1)",
    "UPDATE check_runs SET state = 'failed', reason = 'check_failed', finished_at = 12 WHERE id = 3",
    "INSERT INTO check_runs (id, number, stage, purpose, task_id, proposal_id, version_id, base_revision_id, tree_hash, actor, review_batch_id, created_at) VALUES (8, 2, 'integration', 'integrate', 1, 1, 1, 1, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'fold', 1, 20)",
    "INSERT INTO check_run_members (run_id, version_id) VALUES (8, 1)",
    "UPDATE check_runs SET state = 'failed', reason = 'check_failed', finished_at = 25 WHERE id = 8",
    "INSERT INTO review_batch_integrations (batch_id, outcome, check_run_id, created_at) VALUES (1, 'one_at_a_time', 8, 30)",
    "INSERT INTO check_runs (id, number, stage, purpose, task_id, proposal_id, version_id, base_revision_id, tree_hash, actor, created_at) VALUES (9, 3, 'integration', 'integrate', 1, 1, 1, 1, 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', 'fold', 40)",
];

/// Every row of the tables that hold or refer to check runs, as JSON text, in id order.
const DUMPS: &[&str] = &[
    "SELECT json_array(id, number, stage, purpose, task_id, workspace_id, worker_id, proposal_id, version_id, base_revision_id, tree_hash, actor, title, evidence, state, reason, created_at, finished_at, review_batch_id, intent) FROM check_runs ORDER BY id",
    "SELECT json_array(id, run_id, position, kind, name, required, outcome, exit_code, duration_ms, hex(log), log_bytes, truncated) FROM check_results ORDER BY id",
    "SELECT json_array(run_id, version_id) FROM check_run_members ORDER BY run_id, version_id",
    "SELECT json_array(id, batch_id, outcome, check_run_id, created_at) FROM review_batch_integrations ORDER BY id",
];

#[cfg(test)]
async fn dump(conn: &mut SqliteConnection) -> Vec<String> {
    let mut out = Vec::new();
    for sql in DUMPS {
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(*sql))
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        out.extend(rows);
    }
    out
}

#[tokio::test]
async fn the_migration_keeps_every_run_and_every_row_that_refers_to_one() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    std::fs::create_dir(&before).unwrap();
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"));
    for entry in std::fs::read_dir(source).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: u32 = name.split('_').next().unwrap().parse().unwrap();
        if name.ends_with(".sql") && version < 85 {
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
    for sql in OLD_ROWS {
        sqlx::query(sqlx::AssertSqlSafe(*sql))
            .execute(&mut conn)
            .await
            .unwrap();
    }
    let old = dump(&mut conn).await;
    assert_eq!(old.len(), 3 + 2 + 1 + 1, "the seed wrote every row");
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    assert_eq!(dump(&mut conn).await, old, "every row is unchanged");
    let broken: Vec<(String, i64)> =
        sqlx::query_as("SELECT \"table\", rowid FROM pragma_foreign_key_check")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert!(broken.is_empty(), "{broken:?}");
    conn.close().await.unwrap();

    for name in [
        "check_runs_no_delete",
        "check_runs_end_once",
        "check_runs_review_batch_shape",
        "check_runs_review_batch_fixed",
        "check_runs_intent_fixed",
        "check_runs_direct_main_changeset",
        "check_results_running_only",
        "check_run_members_of_batch",
    ] {
        let found = count(
            &store,
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = '{name}'"
            ),
        )
        .await;
        assert_eq!(found, 1, "{name}");
    }
    for name in [
        "check_runs_one_running_per_workspace",
        "check_runs_one_running_integration",
        "check_runs_task",
        "check_runs_workspace",
        "check_runs_version",
        "check_runs_tree_hash",
        "check_runs_changeset",
    ] {
        let found = count(
            &store,
            &format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{name}'"),
        )
        .await;
        assert_eq!(found, 1, "{name}");
    }

    let run = checks::get_run(store.read(), run_id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.task, TaskId::from_number(1));
    assert_eq!(run.changeset, None);
    assert_eq!(run.title.as_deref(), Some("Title"));
    assert!(run.intent.is_some());
    assert_eq!(
        checks::list_results(store.read(), run_id(1), true)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        checks::run_member_versions(store.read(), run_id(2))
            .await
            .unwrap(),
        [1]
    );
    let running = checks::running_runs(store.read()).await.unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, run_id(3));
    for sql in [
        "UPDATE check_runs SET tree_hash = 'x' WHERE id = 3",
        "DELETE FROM check_runs WHERE id = 3",
        "UPDATE check_results SET outcome = 'passed'",
        "DELETE FROM check_results",
    ] {
        assert!(exec(&store, sql).await.is_err(), "{sql}");
    }
}

use std::path::Path;

use fold_core::ProjectPaths;
use fold_store::{Store, events};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row};

use crate::common::{count, exec, open};

/// The cursors of every worker, as `(worker_id, last_seq)` in worker order.
#[cfg(test)]
async fn cursors(store: &Store) -> Vec<(i64, i64)> {
    sqlx::query("SELECT worker_id, last_seq FROM worker_cursors ORDER BY worker_id")
        .fetch_all(store.read())
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect()
}

#[tokio::test]
async fn every_worker_of_an_agent_starts_where_the_agent_cursor_stood() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    std::fs::create_dir(&before).unwrap();
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"));
    for entry in std::fs::read_dir(source).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: u32 = name.split('_').next().unwrap().parse().unwrap();
        if name.ends_with(".sql") && version < 53 {
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
    for sql in [
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0), (2, 'frontend', 0), (3, 'docs', 0)",
        "INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (10, 1, 'backend', NULL, 0), (11, 1, 'backend/sub', 10, 0), (12, 1, 'backend/sub/x', 11, 0), (20, 2, 'frontend', NULL, 0), (30, 3, 'docs', NULL, 0)",
        "INSERT INTO agent_cursors (agent_id, last_seq) VALUES (1, 50), (2, 7)",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    assert_eq!(
        cursors(&store).await,
        [(10, 50), (11, 50), (12, 50), (20, 7)],
        "a worker of an agent without a cursor gets none"
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'agent_cursors'"
        )
        .await,
        0
    );
    assert_eq!(events::cursor(store.read(), 11).await.unwrap(), 50);
    assert_eq!(events::cursor(store.read(), 30).await.unwrap(), 0);
}

#[tokio::test]
async fn a_worker_has_one_cursor() {
    let (_dir, _paths, store) = open().await;
    for sql in [
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0)",
        "INSERT INTO workers (id, agent_id, name, created_at) VALUES (10, 1, 'backend', 0)",
        "INSERT INTO worker_cursors (worker_id, last_seq) VALUES (10, 3)",
    ] {
        exec(&store, sql).await.unwrap();
    }
    let refused = exec(
        &store,
        "INSERT INTO worker_cursors (worker_id, last_seq) VALUES (10, 4)",
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("UNIQUE"), "{refused}");
    let orphan = exec(
        &store,
        "INSERT INTO worker_cursors (worker_id, last_seq) VALUES (99, 4)",
    )
    .await
    .unwrap_err();
    assert!(orphan.to_string().contains("FOREIGN KEY"), "{orphan}");
}

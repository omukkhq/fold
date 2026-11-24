use std::path::Path;
use std::time::Duration;

use crate::common::{count, exec};
use fold_core::{ProjectPaths, codes};
use fold_store::{Store, StoreError};
use tempfile::TempDir;

const IDLE: Duration = Duration::from_millis(50);
const ACQUIRE: Duration = Duration::from_millis(500);
const WAIT: Duration = Duration::from_secs(10);

#[cfg(test)]
async fn open_reconnecting() -> (TempDir, ProjectPaths, Store) {
    let dir = TempDir::new().unwrap();
    let paths = ProjectPaths::new(dir.path().join("project"));
    let store = Store::open_project_reconnecting(&paths, IDLE, ACQUIRE)
        .await
        .unwrap();
    (dir, paths, store)
}

#[cfg(test)]
/// Waits until both pools of `store` closed their idle connections, so the next statement opens a new one by path.
async fn wait_until_disconnected(store: &Store) {
    tokio::time::timeout(WAIT, async {
        while store.connections() != (0, 0) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[cfg(test)]
fn remove_database(db: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = db.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::remove_file(&name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            other => other.unwrap(),
        }
    }
}

#[cfg(test)]
async fn marker_store(paths: &ProjectPaths) -> Store {
    let store = Store::open_project(paths).await.unwrap();
    exec(&store, "CREATE TABLE marker (n INTEGER NOT NULL)")
        .await
        .unwrap();
    store
}

#[tokio::test]
async fn a_store_whose_file_was_replaced_writes_nothing_into_the_new_file() {
    let (_dir, paths, old) = open_reconnecting().await;
    exec(&old, "CREATE TABLE marker (n INTEGER NOT NULL)")
        .await
        .unwrap();
    exec(&old, "INSERT INTO marker VALUES (1)").await.unwrap();
    assert_eq!(count(&old, "SELECT count(*) FROM marker").await, 1);
    wait_until_disconnected(&old).await;

    remove_database(&paths.db());
    let new = marker_store(&paths).await;

    let error = tokio::time::timeout(WAIT, exec(&old, "INSERT INTO marker VALUES (2)"))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(
            error,
            StoreError::FileReplaced {
                database: "project"
            }
        ),
        "{error:?}"
    );
    assert_eq!(error.code(), Some(codes::DATABASE_REPLACED));
    assert!(error.to_string().contains("replaced"), "{error}");

    let read = tokio::time::timeout(
        WAIT,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM marker").fetch_one(old.read()),
    )
    .await
    .unwrap();
    assert!(read.is_err(), "the old store read the new file: {read:?}");

    let again = exec(&old, "INSERT INTO marker VALUES (3)")
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some(codes::DATABASE_REPLACED));
    let closed = old.close().await.unwrap_err();
    assert_eq!(closed.code(), Some(codes::DATABASE_REPLACED));
    let after_close = exec(&old, "INSERT INTO marker VALUES (5)")
        .await
        .unwrap_err();
    assert!(after_close.closed(), "{after_close:?}");

    assert_eq!(count(&new, "SELECT count(*) FROM marker").await, 0);
    exec(&new, "INSERT INTO marker VALUES (4)").await.unwrap();
    assert_eq!(count(&new, "SELECT count(*) FROM marker").await, 1);
    new.close().await.unwrap();
}

#[tokio::test]
async fn a_store_whose_file_was_deleted_refuses_and_creates_no_file() {
    let (_dir, paths, old) = open_reconnecting().await;
    exec(&old, "CREATE TABLE marker (n INTEGER NOT NULL)")
        .await
        .unwrap();
    wait_until_disconnected(&old).await;
    remove_database(&paths.db());

    let error = tokio::time::timeout(WAIT, exec(&old, "INSERT INTO marker VALUES (1)"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::DATABASE_REPLACED));
    let read = tokio::time::timeout(
        WAIT,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM marker").fetch_one(old.read()),
    )
    .await
    .unwrap();
    assert!(read.is_err(), "{read:?}");
    assert!(!paths.db().exists());
}

#[tokio::test]
async fn a_store_whose_file_stays_opens_new_connections_and_keeps_working() {
    let (_dir, paths, store) = open_reconnecting().await;
    exec(&store, "CREATE TABLE marker (n INTEGER NOT NULL)")
        .await
        .unwrap();
    wait_until_disconnected(&store).await;

    exec(&store, "INSERT INTO marker VALUES (1)").await.unwrap();
    assert_eq!(count(&store, "SELECT count(*) FROM marker").await, 1);
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(store.read())
        .await
        .unwrap();
    assert_eq!(mode, "wal");
    store.close().await.unwrap();

    let reopened = Store::open_project(&paths).await.unwrap();
    assert_eq!(count(&reopened, "SELECT count(*) FROM marker").await, 1);
    reopened.close().await.unwrap();
}

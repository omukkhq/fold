use crate::common::{count, exec, open};
use fold_core::codes;
use fold_store::{Store, StoreError};
use sqlx::{Connection, Row};

/// The version of every `.sql` file in `migrations/project`, the files sqlx loads, so the test follows the migrations as they are added.
#[cfg(test)]
fn project_migration_versions() -> Vec<i64> {
    migration_versions(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"))
}

#[cfg(test)]
fn registry_migration_count() -> i64 {
    let versions = migration_versions(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/registry"));
    i64::try_from(versions.len()).unwrap()
}

#[cfg(test)]
fn migration_versions(dir: &str) -> Vec<i64> {
    let mut versions: Vec<i64> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".sql"))
        .map(|name| name.split('_').next().unwrap().parse().unwrap())
        .collect();
    versions.sort_unstable();
    versions
}

#[tokio::test]
async fn opens_in_wal_mode_with_full_sync_and_foreign_keys() {
    let (_dir, paths, store) = open().await;
    assert!(paths.db().is_file());
    let mut seen = Vec::new();
    // Holding every reader at once makes the pool open all of them, so each connection is checked.
    let mut readers = Vec::new();
    for _ in 0..4 {
        readers.push(store.read().acquire().await.unwrap());
    }
    for reader in &mut readers {
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut **reader)
            .await
            .unwrap();
        let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut **reader)
            .await
            .unwrap();
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **reader)
            .await
            .unwrap();
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut **reader)
            .await
            .unwrap();
        seen.push((mode, sync, fk, busy));
    }
    drop(readers);
    let on_writer = store
        .write(async |tx| {
            let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&mut *tx.conn())
                .await?;
            let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
                .fetch_one(&mut *tx.conn())
                .await?;
            let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut *tx.conn())
                .await?;
            let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&mut *tx.conn())
                .await?;
            Ok::<_, StoreError>((mode, sync, fk, busy))
        })
        .await
        .unwrap();
    seen.push(on_writer);
    assert_eq!(seen.len(), 5);
    for (mode, sync, fk, busy) in seen {
        assert_eq!(mode, "wal");
        assert_eq!(sync, 2, "synchronous=FULL");
        assert_eq!(fk, 1);
        assert_eq!(busy, 5000);
    }
}

#[tokio::test]
async fn readers_cannot_write() {
    let (_dir, _paths, store) = open().await;
    let error = sqlx::query("INSERT INTO agents (name, created_at) VALUES ('a', 0)")
        .execute(store.read())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("readonly"),
        "unexpected error: {error}"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM agents").await, 0);
}

#[tokio::test]
async fn write_commits_on_ok_and_rolls_back_on_err() {
    let (_dir, _paths, store) = open().await;
    let value = store
        .write(async |tx| {
            sqlx::query("INSERT INTO agents (name, created_at) VALUES ('kept', 0)")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(7)
        })
        .await
        .unwrap();
    assert_eq!(value, 7);

    let failed = store
        .write(async |tx| {
            sqlx::query("INSERT INTO agents (name, created_at) VALUES ('dropped', 0)")
                .execute(tx.conn())
                .await?;
            Err::<(), _>(StoreError::SchemaTooNew { version: 1 })
        })
        .await;
    assert!(matches!(failed, Err(StoreError::SchemaTooNew { .. })));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM agents").await, 1);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM agents WHERE name = 'dropped'").await,
        0
    );
}

#[tokio::test]
async fn a_panicking_write_rolls_back_and_frees_the_writer() {
    let (_dir, _paths, store) = open().await;
    let clone = store.clone();
    let joined = tokio::spawn(async move {
        clone
            .write(async |tx| {
                sqlx::query("INSERT INTO agents (name, created_at) VALUES ('ghost', 0)")
                    .execute(tx.conn())
                    .await?;
                if true {
                    panic!("boom");
                }
                Ok::<_, StoreError>(())
            })
            .await
    })
    .await;
    assert!(joined.unwrap_err().is_panic());
    exec(
        &store,
        "INSERT INTO agents (name, created_at) VALUES ('after', 0)",
    )
    .await
    .unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM agents WHERE name = 'ghost'").await,
        0
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM agents WHERE name = 'after'").await,
        1
    );
}

#[tokio::test]
async fn readers_see_only_committed_data() {
    let (_dir, _paths, store) = open().await;
    let inside = store
        .write(async |tx| {
            sqlx::query("INSERT INTO agents (name, created_at) VALUES ('pending', 0)")
                .execute(tx.conn())
                .await?;
            let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
                .fetch_one(store.read())
                .await?;
            Ok::<_, StoreError>(seen)
        })
        .await
        .unwrap();
    assert_eq!(inside, 0, "a reader must not see an open transaction");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM agents").await, 1);
}

#[tokio::test]
async fn reopen_does_not_reapply_migrations() {
    let (_dir, paths, store) = open().await;
    exec(
        &store,
        "INSERT INTO agents (name, created_at) VALUES ('stays', 0)",
    )
    .await
    .unwrap();
    let applied = |store: Store| async move {
        sqlx::query(
            "SELECT version, checksum, execution_time FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(store.read())
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get::<i64, _>(0),
                row.get::<Vec<u8>, _>(1),
                row.get::<i64, _>(2),
            )
        })
        .collect::<Vec<_>>()
    };
    let before = applied(store.clone()).await;
    assert_eq!(
        before.iter().map(|row| row.0).collect::<Vec<_>>(),
        project_migration_versions()
    );
    store.close().await.unwrap();

    let reopened = Store::open_project(&paths).await.unwrap();
    assert_eq!(applied(reopened.clone()).await, before);
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM agents").await, 1);
}

#[tokio::test]
async fn opens_of_a_new_file_at_once_apply_each_migration_once() {
    for _ in 0..8 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.db");
        let (first, second, third) = tokio::join!(
            Store::open_registry(&path),
            Store::open_registry(&path),
            Store::open_registry(&path),
        );
        let stores = [first.unwrap(), second.unwrap(), third.unwrap()];
        assert_eq!(
            count(&stores[0], "SELECT COUNT(*) FROM _sqlx_migrations").await,
            registry_migration_count()
        );
        for store in stores {
            store.close().await.unwrap();
        }
    }
}

/// Holds the write lock on a new file for longer than the opener's busy timeout, the way a first opener's migration transaction does on a loaded host, and checks that the open waits it out instead of failing with `SQLITE_BUSY`.
#[cfg(test)]
async fn open_outlasts_a_long_write_lock(wal: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let mut options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    if wal {
        options = options.journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    }
    let mut holder = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE held (a)")
        .execute(&mut holder)
        .await
        .unwrap();

    let busy_timeout = std::time::Duration::from_millis(100);
    let opener = tokio::spawn({
        let path = path.clone();
        async move {
            Store::open_registry_waiting(&path, std::time::Duration::from_secs(30), busy_timeout)
                .await
        }
    });
    tokio::time::sleep(busy_timeout * 4).await;
    assert!(
        !opener.is_finished(),
        "the opener gave up while the lock was held"
    );
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();

    let store = tokio::time::timeout(std::time::Duration::from_secs(30), opener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM _sqlx_migrations").await,
        registry_migration_count()
    );
    store.close().await.unwrap();
    holder.close().await.unwrap();
}

#[tokio::test]
async fn open_waits_out_a_migration_transaction_longer_than_the_busy_timeout() {
    open_outlasts_a_long_write_lock(true).await;
}

#[tokio::test]
async fn open_waits_out_a_lock_on_the_wal_switch_longer_than_the_busy_timeout() {
    open_outlasts_a_long_write_lock(false).await;
}

#[tokio::test]
async fn open_gives_up_with_database_locked_when_the_lock_is_never_released() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let mut holder = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut holder)
        .await
        .unwrap();

    let wait = std::time::Duration::from_secs(1);
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Store::open_registry_waiting(&path, wait, std::time::Duration::from_millis(100)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code(), Some(codes::DATABASE_LOCKED));
    assert!(
        matches!(error, StoreError::Locked { database: "registry", waited } if waited >= wait),
        "{error:?}"
    );
    assert!(error.to_string().contains("another fold process"));
    holder.close().await.unwrap();
}

#[tokio::test]
async fn a_migration_that_fails_for_another_reason_fails_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE projects (a)")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();

    // Far shorter than the wait: a refused migration is not retried.
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Store::open_registry_waiting(
            &path,
            std::time::Duration::from_secs(600),
            std::time::Duration::from_secs(5),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(error, StoreError::Migrate(_)), "{error:?}");
    assert_eq!(error.code(), None);
}

#[tokio::test]
async fn registry_opens_with_its_own_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Store::open_registry(&dir.path().join("nested").join("registry.db"))
        .await
        .unwrap();
    for table in ["projects", "humans", "web_tokens", "web_sessions"] {
        let found = count(
            &registry,
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
            ),
        )
        .await;
        assert_eq!(found, 1, "{table}");
    }
    assert_eq!(
        count(
            &registry,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'tasks'"
        )
        .await,
        0
    );
    registry.close().await.unwrap();
}

const SHORT_BUSY: std::time::Duration = std::time::Duration::from_millis(100);

#[tokio::test]
async fn reading_a_missing_registry_fails_and_creates_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let error = Store::read_registry(&path, SHORT_BUSY).await.unwrap_err();
    assert!(matches!(error, StoreError::Database(_)), "{error:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_registry_read_lists_its_projects_and_refuses_a_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    Store::open_registry(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let reader = Store::read_registry(&path, SHORT_BUSY).await.unwrap();
    assert!(
        fold_store::projects::list_projects(&reader)
            .await
            .unwrap()
            .is_empty()
    );
    let write = sqlx::query("DELETE FROM humans").execute(&reader).await;
    assert!(write.is_err(), "{write:?}");
    reader.close().await;
}

/// A registry made by this build, with `change` run on its migration table afterwards.
#[cfg(test)]
async fn registry_with(change: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let registry = Store::open_registry(&path).await.unwrap();
    exec(&registry, change).await.unwrap();
    registry.close().await.unwrap();
    (dir, path)
}

#[tokio::test]
async fn a_registry_of_a_newer_fold_is_not_read() {
    let (_dir, path) = registry_with(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (99990101000000, 'later', 1, x'00', 0)",
    )
    .await;
    let error = Store::read_registry(&path, SHORT_BUSY).await.unwrap_err();
    assert!(
        matches!(
            error,
            StoreError::SchemaTooNew {
                version: 99_990_101_000_000
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_registry_that_lacks_a_migration_is_not_read_and_not_migrated() {
    let (_dir, path) = registry_with(
        "DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
    )
    .await;
    let error = Store::read_registry(&path, SHORT_BUSY).await.unwrap_err();
    assert!(matches!(error, StoreError::SchemaOutdated), "{error:?}");
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(applied, registry_migration_count() - 1);
    conn.close().await.unwrap();
}

#[tokio::test]
async fn a_locked_registry_is_given_up_on_after_the_busy_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let mut holder = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE held (a)")
        .execute(&mut holder)
        .await
        .unwrap();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Store::read_registry(&path, SHORT_BUSY),
    )
    .await
    .unwrap();
    assert!(read.is_err(), "{read:?}");
    holder.close().await.unwrap();
}

#[tokio::test]
async fn a_web_token_made_before_sessions_keeps_its_row_id_as_its_number() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = dir.path().join("baseline");
    std::fs::create_dir(&baseline).unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/migrations/registry/0001_baseline.sql"
        ),
        baseline.join("0001_baseline.sql"),
    )
    .unwrap();
    let path = dir.path().join("registry.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(baseline.as_path())
        .await
        .unwrap()
        .run(&mut conn)
        .await
        .unwrap();
    for sql in [
        "INSERT INTO humans (id, name, created_at) VALUES (1, 'ana', 0)",
        "INSERT INTO web_tokens (id, token_hash, human_id, label, created_at, last_used_at) VALUES (3, x'01', 1, 'laptop', 5, 6), (7, x'02', 1, NULL, 8, NULL)",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();

    let registry = Store::open_registry(&path).await.unwrap();
    let numbers: Vec<(i64, i64)> = sqlx::query("SELECT id, number FROM web_tokens ORDER BY id")
        .fetch_all(registry.read())
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert_eq!(numbers, [(3, 3), (7, 7)]);
    assert_eq!(
        count(
            &registry,
            "SELECT COUNT(*) FROM web_tokens WHERE revoked_at IS NULL AND ((id = 3 AND label = 'laptop' AND created_at = 5 AND last_used_at = 6) OR (id = 7 AND label IS NULL AND created_at = 8 AND last_used_at IS NULL))"
        )
        .await,
        2
    );
    let duplicate = exec(
        &registry,
        "INSERT INTO web_tokens (number, token_hash, human_id, created_at) VALUES (7, x'03', 1, 9)",
    )
    .await;
    assert!(duplicate.is_err());
    let missing = exec(
        &registry,
        "INSERT INTO web_tokens (token_hash, human_id, created_at) VALUES (x'04', 1, 9)",
    )
    .await;
    assert!(missing.is_err());
    registry.close().await.unwrap();
}

#[tokio::test]
async fn newer_schema_is_refused_with_schema_too_new() {
    let (_dir, paths, store) = open().await;
    store.close().await.unwrap();
    let mut raw = sqlx::SqliteConnection::connect(&format!("sqlite:{}", paths.db().display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (9999, 'from the future', 1, x'00', 0)",
    )
    .execute(&mut raw)
    .await
    .unwrap();
    raw.close().await.unwrap();

    let error = Store::open_project(&paths).await.unwrap_err();
    assert!(
        matches!(error, StoreError::SchemaTooNew { version: 9999 }),
        "{error:?}"
    );
    assert_eq!(error.code(), Some(codes::SCHEMA_TOO_NEW));
    assert!(!error.unique_violation());
    assert_eq!(error.constraint(), None);
}

#[tokio::test]
async fn unique_violation_is_classified_with_its_constraint() {
    let (_dir, _paths, store) = open().await;
    exec(
        &store,
        "INSERT INTO agents (name, created_at) VALUES ('a', 0)",
    )
    .await
    .unwrap();
    let error = exec(
        &store,
        "INSERT INTO agents (name, created_at) VALUES ('a', 1)",
    )
    .await
    .unwrap_err();
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("agents.name"));
    assert_eq!(error.code(), None);
    assert!(
        !error.to_string().contains("'a'"),
        "no bound value in the detail"
    );

    let check = exec(
        &store,
        "INSERT INTO tasks (number, title, created_by, created_at, updated_at, status) VALUES (1, 't', 'x', 0, 0, 'nope')",
    )
    .await
    .unwrap_err();
    assert!(!check.unique_violation());
    assert_eq!(check.constraint().as_deref(), Some("tasks_status_values"));

    let foreign = exec(
        &store,
        "INSERT INTO agent_tags (agent_id, tag, created_at) VALUES (99, 't', 0)",
    )
    .await
    .unwrap_err();
    assert!(!foreign.unique_violation());
    assert_eq!(foreign.constraint(), None);
}

#[tokio::test]
async fn open_fails_when_the_path_cannot_be_created() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, b"x").unwrap();
    let error = Store::open_registry(&blocker.join("registry.db"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Io(_)), "{error:?}");
    assert_eq!(error.code(), None);
}

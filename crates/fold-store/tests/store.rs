#[cfg(test)]
mod common;

use common::{count, exec, open};
use fold_core::codes;
use fold_store::{Store, StoreError};
use sqlx::{Connection, Row};

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
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].0, 1);
    store.close().await.unwrap();

    let reopened = Store::open_project(&paths).await.unwrap();
    assert_eq!(applied(reopened.clone()).await, before);
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM agents").await, 1);
}

#[tokio::test]
async fn registry_opens_with_its_own_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Store::open_registry(&dir.path().join("nested").join("registry.db"))
        .await
        .unwrap();
    for table in ["projects", "humans", "web_tokens"] {
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

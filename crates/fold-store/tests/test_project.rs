use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use fold_core::Clock;
use fold_store::StoreError;
use fold_store::testing::{TestProject, assert_append_only, eventually, remove_dir_eventually};

/// The version of every `.sql` file in `migrations/project`, the files sqlx loads, so the test follows the migrations as they are added.
#[cfg(test)]
fn project_migration_versions() -> Vec<i64> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project");
    let mut versions: Vec<i64> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".sql"))
        .map(|name| name.split('_').next().unwrap().parse().unwrap())
        .collect();
    versions.sort_unstable();
    versions
}

#[cfg(test)]
async fn insert_agent(project: &TestProject, name: &'static str) -> Result<(), StoreError> {
    project
        .store()
        .write(async |tx| {
            sqlx::query("INSERT INTO agents (name, created_at) VALUES (?, 0)")
                .bind(name)
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
}

#[cfg(test)]
async fn agents(project: &TestProject) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM agents ORDER BY name")
        .fetch_all(project.store().read())
        .await
        .unwrap()
}

#[tokio::test]
async fn new_project_has_a_migrated_database() {
    let project = TestProject::new().await;
    assert!(project.paths().db().is_file());
    assert!(project.paths().fold_dir().is_dir());
    assert!(!project.paths().objects().exists(), "no object store yet");
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(project.store().read())
            .await
            .unwrap();
    assert_eq!(versions, project_migration_versions());
    let registry_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
    )
    .fetch_one(project.registry().read())
    .await
    .unwrap();
    assert_eq!(registry_tables, 1);
}

#[tokio::test]
async fn projects_are_isolated_from_each_other() {
    let first = TestProject::new().await;
    let second = TestProject::new().await;
    assert_ne!(first.paths().root(), second.paths().root());
    insert_agent(&first, "only-in-first").await.unwrap();
    assert_eq!(agents(&first).await, vec!["only-in-first"]);
    assert!(agents(&second).await.is_empty());
    first
        .registry()
        .write(async |tx| {
            sqlx::query("INSERT INTO humans (name, created_at) VALUES ('ana', 0)")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let humans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM humans")
        .fetch_one(second.registry().read())
        .await
        .unwrap();
    assert_eq!(humans, 0);
}

#[tokio::test]
async fn reopen_keeps_committed_rows_and_drops_uncommitted_ones() {
    let mut project = TestProject::new().await;
    insert_agent(&project, "committed").await.unwrap();
    let uncommitted = project
        .store()
        .write(async |tx| {
            sqlx::query("INSERT INTO agents (name, created_at) VALUES ('uncommitted', 0)")
                .execute(tx.conn())
                .await?;
            Err::<(), _>(StoreError::SchemaTooNew { version: 0 })
        })
        .await;
    assert!(uncommitted.is_err());
    project.reopen().await;
    assert_eq!(agents(&project).await, vec!["committed"]);
    insert_agent(&project, "after-reopen").await.unwrap();
    assert_eq!(agents(&project).await, vec!["after-reopen", "committed"]);
}

#[tokio::test]
async fn fixture_clock_starts_fixed_and_is_shared_with_services() {
    let project = TestProject::new().await;
    assert_eq!(project.now().to_string(), "2026-01-01T00:00:00.000Z");
    let service_clock: Arc<dyn Clock> = Arc::new(project.clock().clone());
    project.clock().advance(Duration::from_secs(60)).unwrap();
    assert_eq!(service_clock.now(), project.now());
    assert_eq!(service_clock.now().to_string(), "2026-01-01T00:01:00.000Z");
    let other = TestProject::new().await;
    assert_eq!(other.now().to_string(), "2026-01-01T00:00:00.000Z");
}

#[tokio::test]
async fn eventually_returns_when_the_condition_holds_and_fails_on_timeout() {
    let calls = AtomicU32::new(0);
    let value = eventually(Duration::from_secs(5), async || {
        let seen = calls.fetch_add(1, Ordering::SeqCst);
        if seen >= 3 { Ok(seen) } else { Err(seen) }
    })
    .await;
    assert_eq!(value, 3);

    let failed = tokio::spawn(eventually(Duration::from_millis(50), async || {
        Err::<(), _>("never ready")
    }))
    .await
    .unwrap_err();
    assert!(failed.is_panic());
    let message = failed.into_panic().downcast::<String>().unwrap();
    assert!(message.contains("last value: \"never ready\""), "{message}");
}

#[tokio::test]
async fn remove_dir_eventually_removes_a_folder_that_a_writer_fills() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("data");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let writer = {
        let sub = dir.join("sub");
        std::thread::spawn(move || {
            let mut written = 0_u64;
            while std::fs::write(sub.join(format!("f{written}")), b"x").is_ok() {
                written += 1;
            }
        })
    };
    remove_dir_eventually(&dir, Duration::from_secs(10)).await;
    writer.join().unwrap();
    assert!(!dir.exists());
    remove_dir_eventually(&dir, Duration::from_secs(10)).await;

    let file = parent.path().join("file");
    std::fs::write(&file, b"x").unwrap();
    // A runtime of its own, because the future of `eventually` with a borrowing closure is not `Send`.
    let failed = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(remove_dir_eventually(&file, Duration::from_secs(10)));
    })
    .join();
    assert!(failed.is_err());
}

#[tokio::test]
async fn dropping_the_fixture_deletes_its_directory() {
    let project = TestProject::new().await;
    let root = project.paths().root().to_path_buf();
    let parent = root.parent().unwrap().to_path_buf();
    assert!(root.is_dir());
    drop(project);
    assert!(!root.exists());
    assert!(!parent.exists());
}

#[cfg(test)]
async fn scratch_table(project: &TestProject, with_triggers: bool, with_row: bool) {
    project
        .store()
        .write(async |tx| {
            sqlx::query("CREATE TABLE scratch (id INTEGER PRIMARY KEY, created_at INTEGER)")
                .execute(&mut *tx.conn())
                .await?;
            if with_triggers {
                for op in ["UPDATE", "DELETE"] {
                    let sql = format!(
                        "CREATE TRIGGER scratch_no_{lower} BEFORE {op} ON scratch BEGIN SELECT RAISE(ABORT, 'scratch is append-only'); END",
                        lower = op.to_lowercase()
                    );
                    sqlx::query(sqlx::AssertSqlSafe(sql))
                        .execute(&mut *tx.conn())
                        .await?;
                }
            }
            if with_row {
                sqlx::query("INSERT INTO scratch (created_at) VALUES (0)")
                    .execute(&mut *tx.conn())
                    .await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

#[cfg(test)]
async fn panic_message<F: Future<Output = ()> + Send + 'static>(future: F) -> String {
    let error = tokio::spawn(future).await.unwrap_err();
    assert!(error.is_panic());
    *error.into_panic().downcast::<String>().unwrap()
}

#[tokio::test]
async fn assert_append_only_detects_a_missing_trigger() {
    let guarded = TestProject::new().await;
    scratch_table(&guarded, true, true).await;
    assert_append_only(guarded.store(), "scratch").await;
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scratch")
        .fetch_one(guarded.store().read())
        .await
        .unwrap();
    assert_eq!(rows, 1, "the probe leaves the rows alone");

    let open = TestProject::new().await;
    scratch_table(&open, false, true).await;
    let store = open.store().clone();
    let message = panic_message(async move { assert_append_only(&store, "scratch").await }).await;
    assert!(message.contains("was accepted"), "{message}");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scratch")
        .fetch_one(open.store().read())
        .await
        .unwrap();
    assert_eq!(rows, 1, "a wrongly accepted statement is rolled back");

    let empty = TestProject::new().await;
    scratch_table(&empty, true, false).await;
    let store = empty.store().clone();
    let message = panic_message(async move { assert_append_only(&store, "scratch").await }).await;
    assert!(message.contains("insert a row first"), "{message}");
}

#[tokio::test]
async fn assert_append_only_checks_the_table_name_and_the_message() {
    let project = TestProject::new().await;
    for bad in ["", "Events", "events; DROP TABLE agents", "a-b", "events2"] {
        let store = project.store().clone();
        let name = bad.to_owned();
        let message = panic_message(async move { assert_append_only(&store, &name).await }).await;
        assert!(message.contains("[a-z_]+"), "{bad}: {message}");
    }
    let store = project.store().clone();
    let message = panic_message(async move { assert_append_only(&store, "nope").await }).await;
    assert!(message.contains("cannot count"), "{message}");

    project
        .store()
        .write(async |tx| {
            sqlx::query("CREATE TABLE scratch (id INTEGER PRIMARY KEY)")
                .execute(&mut *tx.conn())
                .await?;
            sqlx::query("INSERT INTO scratch DEFAULT VALUES")
                .execute(&mut *tx.conn())
                .await?;
            sqlx::query(
                "CREATE TRIGGER scratch_no_update BEFORE UPDATE ON scratch BEGIN SELECT RAISE(ABORT, 'something else'); END",
            )
            .execute(&mut *tx.conn())
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let store = project.store().clone();
    let message = panic_message(async move { assert_append_only(&store, "scratch").await }).await;
    assert!(message.contains("different message"), "{message}");

    insert_agent(&project, "x").await.unwrap();
    let store = project.store().clone();
    let message = panic_message(async move { assert_append_only(&store, "agents").await }).await;
    assert!(message.contains("was accepted"), "{message}");
}

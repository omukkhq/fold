#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fixture fails the test that uses it instead of returning errors"
)]

use std::fmt::Debug;
use std::path::Path;
use std::time::Duration;

use fold_core::{Clock, ManualClock, ProjectPaths, Timestamp};
use sqlx::AssertSqlSafe;
use tempfile::TempDir;

use crate::{Store, StoreError};

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A temporary project: the layout on disk, a migrated project database and a manual clock. Dropping it deletes the directory.
#[derive(Debug)]
pub struct TestProject {
    _dir: TempDir,
    paths: ProjectPaths,
    store: Store,
    _registry_dir: TempDir,
    registry: Store,
    clock: ManualClock,
}

impl Drop for TestProject {
    /// `main/` keeps its directories without write access, which would stop the temporary directory from being removed.
    fn drop(&mut self) {
        restore_write_access(self._dir.path());
    }
}

#[cfg(unix)]
fn restore_write_access(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let mode = meta.permissions().mode() | 0o700;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        restore_write_access(&entry.path());
    }
}

#[cfg(not(unix))]
fn restore_write_access(_dir: &Path) {}

impl TestProject {
    pub async fn new() -> Self {
        let dir = TempDir::new().expect("create the project directory");
        let paths = ProjectPaths::new(dir.path().join("project"));
        let store = Store::open_project(&paths)
            .await
            .expect("open the project database");
        // The host registry lives in its own directory so host-level tests never share state with a project.
        let registry_dir = TempDir::new().expect("create the registry directory");
        let registry = Store::open_registry(&registry_dir.path().join("registry.db"))
            .await
            .expect("open the registry database");
        Self {
            _dir: dir,
            paths,
            store,
            _registry_dir: registry_dir,
            registry,
            clock: ManualClock::new(),
        }
    }

    pub fn paths(&self) -> &ProjectPaths {
        &self.paths
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn registry(&self) -> &Store {
        &self.registry
    }

    /// Clones share one time, so a service that holds `Arc::new(project.clock().clone())` sees every `advance` and `set`.
    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }

    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// Closes the store, which checkpoints the WAL, and opens the same files again, as a daemon restart does.
    pub async fn reopen(&mut self) {
        self.store
            .close()
            .await
            .expect("close the project database");
        self.store = Store::open_project(&self.paths)
            .await
            .expect("reopen the project database");
    }
}

/// Runs `sql`, which may hold several statements, in one committed write. Tests use it for rows no store function writes.
pub async fn execute(store: &Store, sql: &str) {
    let outcome = store
        .write(async |tx| {
            sqlx::raw_sql(AssertSqlSafe(sql)).execute(tx.conn()).await?;
            Ok::<_, StoreError>(())
        })
        .await;
    if let Err(error) = outcome {
        panic!("`{sql}` failed: {error}");
    }
}

enum Probe {
    Refused(String),
    Accepted,
    Failed(StoreError),
}

impl From<StoreError> for Probe {
    fn from(error: StoreError) -> Self {
        Probe::Failed(error)
    }
}

/// Runs `sql` in a write that always rolls back, so a statement that is wrongly accepted changes nothing.
async fn probe(store: &Store, sql: &str) -> Probe {
    let outcome = store
        .write(async |tx| -> Result<(), Probe> {
            match sqlx::query(AssertSqlSafe(sql)).execute(tx.conn()).await {
                Ok(_) => Err(Probe::Accepted),
                Err(error) => Err(Probe::Refused(error.to_string())),
            }
        })
        .await;
    match outcome {
        Err(probe) => probe,
        Ok(()) => Probe::Accepted,
    }
}

/// Requires `UPDATE` and `DELETE` on `table` to fail with the trigger's `<table> is append-only` message. The table must hold a row, so the statements have something to refuse. A failure panics with a message that names the table.
pub async fn assert_append_only(store: &Store, table: &str) {
    if let Err(message) = check_append_only(store, table).await {
        panic!("{message}");
    }
}

async fn check_append_only(store: &Store, table: &str) -> Result<(), String> {
    if table.is_empty() || !table.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Err(format!("table name {table:?} does not match [a-z_]+"));
    }
    let rows: i64 = sqlx::query_scalar(AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(store.read())
        .await
        .map_err(|error| format!("cannot count the rows of {table}: {error}"))?;
    if rows == 0 {
        return Err(format!("{table} is empty: insert a row first"));
    }
    let expected = format!("{table} is append-only");
    for sql in [
        format!("UPDATE {table} SET rowid = rowid"),
        format!("DELETE FROM {table}"),
    ] {
        match probe(store, &sql).await {
            Probe::Refused(message) if message.contains(&expected) => {}
            Probe::Refused(message) => {
                return Err(format!(
                    "`{sql}` failed with a different message than {expected:?}: {message}"
                ));
            }
            Probe::Accepted => {
                return Err(format!("`{sql}` was accepted; {table} is not append-only"));
            }
            Probe::Failed(error) => return Err(format!("`{sql}` could not run: {error}")),
        }
    }
    Ok(())
}

/// Polls `check` every 10 ms until it returns `Ok` and yields that value. When `timeout` passes it panics with the last `Err` value, so a failed wait shows what the condition saw. Tests wait for an effect with this and never with a fixed sleep.
pub async fn eventually<T, V: Debug>(
    timeout: Duration,
    mut check: impl AsyncFnMut() -> Result<T, V>,
) -> T {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match check().await {
            Ok(value) => return value,
            Err(last) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("condition not met within {timeout:?}; last value: {last:?}");
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

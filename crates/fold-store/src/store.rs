use std::path::Path;
use std::time::Duration;

use fold_core::ProjectPaths;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
    SqliteSynchronous,
};
use sqlx::{Sqlite, Transaction};

use crate::error::StoreError;

static PROJECT_MIGRATIONS: Migrator = sqlx::migrate!("./migrations/project");
static REGISTRY_MIGRATIONS: Migrator = sqlx::migrate!("./migrations/registry");

const READ_CONNECTIONS: u32 = 4;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an open waits for another process to release the write lock before it gives up with `database_locked`.
const OPEN_WAIT: Duration = Duration::from_secs(60);
const BUSY_RETRY: Duration = Duration::from_millis(10);
const BUSY_RETRY_MAX: Duration = Duration::from_millis(250);
const SQLITE_BUSY: i32 = 5;

/// One SQLite database: a single writer connection and a pool of read-only connections. Clones share both pools.
#[derive(Debug, Clone)]
pub struct Store {
    writer: SqlitePool,
    readers: SqlitePool,
}

/// The one open write transaction. Commands and `events::append` take it by `&mut`; it is begun with `BEGIN IMMEDIATE` so the writer lock is held from the first statement.
#[derive(Debug)]
pub struct WriteTx {
    tx: Transaction<'static, Sqlite>,
}

impl WriteTx {
    pub fn conn(&mut self) -> &mut SqliteConnection {
        &mut self.tx
    }
}

impl Store {
    /// Opens `fold.db`, creating the file when it is absent, and applies the project migrations before returning.
    pub async fn open_project(paths: &ProjectPaths) -> Result<Store, StoreError> {
        Self::open(
            &paths.db(),
            &PROJECT_MIGRATIONS,
            "project",
            OPEN_WAIT,
            BUSY_TIMEOUT,
        )
        .await
    }

    /// Opens the host registry database and applies the registry migrations before returning.
    pub async fn open_registry(path: &Path) -> Result<Store, StoreError> {
        Self::open(
            path,
            &REGISTRY_MIGRATIONS,
            "registry",
            OPEN_WAIT,
            BUSY_TIMEOUT,
        )
        .await
    }

    /// `open_registry` with a chosen wait for the write lock and a chosen busy timeout, so a test can see the open give up, or outlast a lock held past the busy timeout, in well under a second.
    #[cfg(any(test, feature = "testing"))]
    pub async fn open_registry_waiting(
        path: &Path,
        wait: Duration,
        busy_timeout: Duration,
    ) -> Result<Store, StoreError> {
        Self::open(path, &REGISTRY_MIGRATIONS, "registry", wait, busy_timeout).await
    }

    /// Another opener's WAL switch or migration transaction refuses this one with `SQLITE_BUSY`, and a loaded host can hold the migration transaction past the busy timeout, so the whole open is tried again from a fresh connection until `wait` has passed. The last attempt can run up to the busy timeout past `wait`.
    async fn open(
        path: &Path,
        migrations: &'static Migrator,
        database: &'static str,
        wait: Duration,
        busy_timeout: Duration,
    ) -> Result<Store, StoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let started = tokio::time::Instant::now();
        let mut retry = BUSY_RETRY;
        let mut logged = false;
        let writer = loop {
            match connect_and_migrate(path, migrations, busy_timeout).await {
                Err(error) if is_busy(&error) => {
                    let waited = started.elapsed();
                    if waited >= wait {
                        return Err(StoreError::Locked { database, waited });
                    }
                    if waited > busy_timeout && !logged {
                        tracing::warn!(
                            database,
                            waited_ms = waited.as_millis(),
                            "waiting for another process to release the database write lock"
                        );
                        logged = true;
                    }
                    tokio::time::sleep(retry.min(wait.saturating_sub(waited))).await;
                    retry = retry.saturating_mul(2).min(BUSY_RETRY_MAX);
                }
                other => break other?,
            }
        };
        let readers = SqlitePoolOptions::new()
            .max_connections(READ_CONNECTIONS)
            .connect_lazy_with(connect_options(path, busy_timeout).read_only(true));
        Ok(Store { writer, readers })
    }

    /// The read pool. Its connections are opened read-only, so a write through them fails.
    pub fn read(&self) -> &SqlitePool {
        &self.readers
    }

    /// Runs `body` on the writer connection inside `BEGIN IMMEDIATE`. It commits when `body` returns `Ok` and rolls back when it returns `Err`, panics or is dropped. The writer pool has one connection, so writes never interleave.
    pub async fn write<T, E>(
        &self,
        body: impl AsyncFnOnce(&mut WriteTx) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<StoreError>,
    {
        let tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let mut tx = WriteTx { tx };
        match body(&mut tx).await {
            Ok(value) => {
                tx.tx.commit().await.map_err(StoreError::from)?;
                Ok(value)
            }
            Err(error) => {
                // The original error is the one the caller needs; a failed rollback leaves the connection to be reset by the pool.
                let _ = tx.tx.rollback().await;
                Err(error)
            }
        }
    }

    /// Checkpoints the WAL and closes both pools.
    pub async fn close(&self) -> Result<(), StoreError> {
        self.readers.close().await;
        let checkpoint = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.writer)
            .await;
        self.writer.close().await;
        checkpoint?;
        Ok(())
    }
}

/// One attempt to open the writer and apply the migrations. Switching a new file to WAL takes the exclusive lock without waiting on the busy timeout, so it fails at once with `SQLITE_BUSY` while another process holds a lock; the migration's `BEGIN IMMEDIATE` waits up to the busy timeout first.
async fn connect_and_migrate(
    path: &Path,
    migrations: &Migrator,
    busy_timeout: Duration,
) -> Result<SqlitePool, StoreError> {
    let writer = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options(path, busy_timeout).create_if_missing(true))
        .await?;
    if let Err(error) = migrate(&writer, migrations).await {
        writer.close().await;
        return Err(error);
    }
    Ok(writer)
}

fn is_busy(error: &StoreError) -> bool {
    let code = match error {
        StoreError::Database(sqlx::Error::Database(error))
        | StoreError::Migrate(
            MigrateError::Execute(sqlx::Error::Database(error))
            | MigrateError::ExecuteMigration(sqlx::Error::Database(error), _),
        ) => error.code(),
        _ => None,
    };
    // Extended codes keep the primary code in the low byte.
    code.and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| code & 0xff == SQLITE_BUSY)
}

/// SQLite gives sqlx no migration lock, so two processes that open a new file at once would both apply the first migration. Holding the write lock for the whole run makes the second one wait and then find every migration applied.
async fn migrate(writer: &SqlitePool, migrations: &Migrator) -> Result<(), StoreError> {
    let mut tx = writer.begin_with("BEGIN IMMEDIATE").await?;
    migrations.run_direct(None, &mut *tx, false).await?;
    tx.commit().await?;
    Ok(())
}

fn connect_options(path: &Path, busy_timeout: Duration) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true)
        .busy_timeout(busy_timeout)
}

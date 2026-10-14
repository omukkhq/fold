use std::path::Path;
use std::time::Duration;

use fold_core::ProjectPaths;
use sqlx::migrate::Migrator;
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
const BUSY_RETRY: Duration = Duration::from_millis(10);
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
        Self::open(&paths.db(), &PROJECT_MIGRATIONS).await
    }

    /// Opens the host registry database and applies the registry migrations before returning.
    pub async fn open_registry(path: &Path) -> Result<Store, StoreError> {
        Self::open(path, &REGISTRY_MIGRATIONS).await
    }

    async fn open(path: &Path, migrations: &'static Migrator) -> Result<Store, StoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let writer = connect_writer(path).await?;
        if let Err(error) = migrate(&writer, migrations).await {
            writer.close().await;
            return Err(error);
        }
        let readers = SqlitePoolOptions::new()
            .max_connections(READ_CONNECTIONS)
            .connect_lazy_with(connect_options(path).read_only(true));
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

/// Switching a new file to WAL takes the exclusive lock without waiting on the busy timeout, so a second process that opens the same new file at that moment is refused with `SQLITE_BUSY`. It tries again until the busy timeout has passed.
async fn connect_writer(path: &Path) -> Result<SqlitePool, StoreError> {
    let deadline = tokio::time::Instant::now() + BUSY_TIMEOUT;
    loop {
        let connected = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_options(path).create_if_missing(true))
            .await;
        match connected {
            Err(error) if is_busy(&error) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(BUSY_RETRY).await;
            }
            other => return Ok(other?),
        }
    }
}

fn is_busy(error: &sqlx::Error) -> bool {
    let code = match error {
        sqlx::Error::Database(error) => error.code(),
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

fn connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
}

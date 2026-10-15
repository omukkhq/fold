use fold_core::{RevisionId, codes};
use sqlx::migrate::MigrateError;
use thiserror::Error;

/// Errors from the database layer. Details come from SQLite and name tables, columns and constraints, never a bound value.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[source] MigrateError),
    #[error(
        "the {database} database is still locked after {} s; another fold process (a daemon, fold serve or fold init) holds its write lock: stop that process or let it finish, then try again",
        waited.as_secs()
    )]
    Locked {
        database: &'static str,
        waited: std::time::Duration,
    },
    #[error("the database was written by a newer fold (schema version {version})")]
    SchemaTooNew { version: i64 },
    /// A caller handed the store a value it refuses to write.
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    /// A stored value no longer parses. The message names the column, never the value.
    #[error("stored data is corrupt: {0}")]
    Corrupt(&'static str),
    #[error("revision {0} does not exist")]
    RevisionNotFound(RevisionId),
    #[error("main is no longer at {expected}")]
    HeadMoved { expected: RevisionId },
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<MigrateError> for StoreError {
    fn from(error: MigrateError) -> Self {
        match error {
            MigrateError::VersionMissing(version) | MigrateError::VersionTooNew(version, _) => {
                StoreError::SchemaTooNew { version }
            }
            other => StoreError::Migrate(other),
        }
    }
}

impl StoreError {
    /// The stable code of the errors that have one; the others are internal failures.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            StoreError::SchemaTooNew { .. } => Some(codes::SCHEMA_TOO_NEW),
            StoreError::Locked { .. } => Some(codes::DATABASE_LOCKED),
            StoreError::RevisionNotFound(_) => Some(codes::REVISION_NOT_FOUND),
            StoreError::HeadMoved { .. } => Some(codes::HEAD_MOVED),
            _ => None,
        }
    }

    /// True when a `UNIQUE` constraint or a unique index refused the statement.
    pub fn unique_violation(&self) -> bool {
        self.database_error()
            .is_some_and(|error| error.is_unique_violation())
    }

    /// What SQLite names after `constraint failed:`. A `CHECK` constraint reports its name (`dependencies_no_self_edge`); a unique constraint or unique index reports `table.column` (`claims.task_id`, `tasks.number`). A foreign key violation has no name and returns `None`.
    pub fn constraint(&self) -> Option<String> {
        let message = self.database_error()?.message();
        let (_, name) = message.split_once(" constraint failed: ")?;
        Some(name.to_owned())
    }

    fn database_error(&self) -> Option<&dyn sqlx::error::DatabaseError> {
        match self {
            StoreError::Database(sqlx::Error::Database(error)) => Some(error.as_ref()),
            _ => None,
        }
    }
}

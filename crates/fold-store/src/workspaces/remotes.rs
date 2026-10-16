use fold_core::{RevisionId, Timestamp};
use sqlx::SqlitePool;

use super::columns::{number, timestamp};
use crate::error::StoreError;
use crate::store::WriteTx;

/// What fold last did with one remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteState {
    pub name: String,
    /// The newest push that reached the remote.
    pub pushed: Option<RemotePushed>,
    /// The failed push after the newest successful one.
    pub failed: Option<RemoteFailed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePushed {
    /// The newest revision the push sent.
    pub revision: RevisionId,
    /// The Git commit of `revision`; `None` while it has no `revision_commits` row.
    pub commit: Option<String>,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFailed {
    pub code: String,
    pub at: Timestamp,
}

/// Records in the caller's write transaction that a push sent main up to `last` to the remote `name`, which clears its failure.
pub async fn record_push(
    tx: &mut WriteTx,
    name: &str,
    last: RevisionId,
    now: Timestamp,
) -> Result<(), StoreError> {
    let revision = last.to_db();
    let at = now.as_millis();
    let recorded = sqlx::query!(
        r#"INSERT INTO remote_states (name, pushed_revision_id, pushed_at)
           SELECT ?1, id, ?3 FROM revisions WHERE number = ?2
           ON CONFLICT (name) DO UPDATE SET
               pushed_revision_id = excluded.pushed_revision_id,
               pushed_at = excluded.pushed_at,
               failure_code = NULL,
               failed_at = NULL"#,
        name,
        revision,
        at
    )
    .execute(tx.conn())
    .await?;
    if recorded.rows_affected() == 0 {
        return Err(StoreError::RevisionNotFound(last));
    }
    Ok(())
}

/// Records in the caller's write transaction that a push to the remote `name` failed with `code`; its last successful push stays.
pub async fn record_push_failure(
    tx: &mut WriteTx,
    name: &str,
    code: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let at = now.as_millis();
    sqlx::query!(
        r#"INSERT INTO remote_states (name, failure_code, failed_at) VALUES (?1, ?2, ?3)
           ON CONFLICT (name) DO UPDATE SET
               failure_code = excluded.failure_code,
               failed_at = excluded.failed_at"#,
        name,
        code,
        at
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Every remote fold pushed to or failed to push to, ordered by name.
pub async fn remote_states(pool: &SqlitePool) -> Result<Vec<RemoteState>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT s.name AS "name!", r.number AS "number?: i64", s.pushed_at, c."commit" AS "commit?", s.failure_code, s.failed_at
           FROM remote_states s
           LEFT JOIN revisions r ON r.id = s.pushed_revision_id
           LEFT JOIN revision_commits c ON c.revision_id = s.pushed_revision_id
           ORDER BY s.name"#
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let pushed = match (row.number, row.pushed_at) {
                (Some(revision), Some(at)) => Some(RemotePushed {
                    revision: number(
                        revision,
                        RevisionId::try_from_db,
                        "remote_states.pushed_revision_id",
                    )?,
                    commit: row.commit,
                    at: timestamp(at, "remote_states.pushed_at")?,
                }),
                _ => None,
            };
            let failed = match (row.failure_code, row.failed_at) {
                (Some(code), Some(at)) => Some(RemoteFailed {
                    code,
                    at: timestamp(at, "remote_states.failed_at")?,
                }),
                _ => None,
            };
            Ok(RemoteState {
                name: row.name,
                pushed,
                failed,
            })
        })
        .collect()
}

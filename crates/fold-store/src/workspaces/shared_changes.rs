use fold_core::{AgentName, Timestamp};
use sqlx::SqliteExecutor;

use super::columns::timestamp;
use crate::error::StoreError;
use crate::store::WriteTx;

/// A change of shared files during a worker's tool call that no one dismissed yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedChangeRow {
    pub id: i64,
    pub agent: AgentName,
    /// The changed files, relative to the project folder, in path order.
    pub paths: Vec<String>,
    pub created_at: Timestamp,
}

/// Records that `paths` of the shared files changed during a tool call of `agent`'s worker. `None` for an agent that is not registered.
pub async fn insert_shared_change(
    tx: &mut WriteTx,
    agent: &AgentName,
    paths: &[String],
    now: Timestamp,
) -> Result<Option<i64>, StoreError> {
    let (name, created_at) = (agent.as_str(), now.as_millis());
    let paths =
        serde_json::to_string(paths).map_err(|_| StoreError::Corrupt("shared_changes.paths"))?;
    let id = sqlx::query_scalar!(
        r#"INSERT INTO shared_changes (agent_id, paths, created_at)
           SELECT a.id, ?, ? FROM agents a WHERE a.name = ?
           RETURNING id AS "id!: i64""#,
        paths,
        created_at,
        name
    )
    .fetch_optional(tx.conn())
    .await?;
    Ok(id)
}

/// The changes no one dismissed yet, oldest first.
pub async fn open_shared_changes(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<SharedChangeRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", a.name AS "agent!: String", c.paths AS "paths!: String",
                  c.created_at AS "created_at!: i64"
           FROM shared_changes c JOIN agents a ON a.id = c.agent_id
           WHERE c.dismissed_at IS NULL
           ORDER BY c.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(SharedChangeRow {
                id: row.id,
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                paths: serde_json::from_str(&row.paths)
                    .map_err(|_| StoreError::Corrupt("shared_changes.paths"))?,
                created_at: timestamp(row.created_at, "shared_changes.created_at")?,
            })
        })
        .collect()
}

/// Dismisses every open change for `by`, an event actor label; returns how many it dismissed.
pub async fn dismiss_shared_changes(
    tx: &mut WriteTx,
    by: &str,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let at = now.as_millis();
    let done = sqlx::query!(
        "UPDATE shared_changes SET dismissed_at = ?, dismissed_by = ? WHERE dismissed_at IS NULL",
        at,
        by
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(done)
}

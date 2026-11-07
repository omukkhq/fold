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

/// Records that `paths` of the shared files changed during a tool call of `agent`'s worker, in the agent's one open change: a new one, or the open one with the paths it does not hold yet added, in path order. Returns the paths it added, in path order; `None` for an agent that is not registered or when the open change holds every path already.
pub async fn note_shared_change(
    tx: &mut WriteTx,
    agent: &AgentName,
    paths: &[String],
    now: Timestamp,
) -> Result<Option<Vec<String>>, StoreError> {
    let name = agent.as_str();
    let open = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", c.paths AS "paths!: String"
           FROM shared_changes c JOIN agents a ON a.id = c.agent_id
           WHERE a.name = ? AND c.dismissed_at IS NULL"#,
        name
    )
    .fetch_optional(tx.conn())
    .await?;
    let held: std::collections::BTreeSet<String> = match &open {
        Some(row) => serde_json::from_str(&row.paths)
            .map_err(|_| StoreError::Corrupt("shared_changes.paths"))?,
        None => std::collections::BTreeSet::new(),
    };
    let added: std::collections::BTreeSet<String> = paths
        .iter()
        .filter(|path| !held.contains(*path))
        .cloned()
        .collect();
    if added.is_empty() {
        return Ok(None);
    }
    let all: std::collections::BTreeSet<&String> = held.iter().chain(&added).collect();
    let text =
        serde_json::to_string(&all).map_err(|_| StoreError::Corrupt("shared_changes.paths"))?;
    match open {
        Some(row) => {
            sqlx::query!(
                "UPDATE shared_changes SET paths = ? WHERE id = ?",
                text,
                row.id
            )
            .execute(tx.conn())
            .await?;
        }
        None => {
            let created_at = now.as_millis();
            let inserted = sqlx::query_scalar!(
                r#"INSERT INTO shared_changes (agent_id, paths, created_at)
                   SELECT a.id, ?, ? FROM agents a WHERE a.name = ?
                   RETURNING id AS "id!: i64""#,
                text,
                created_at,
                name
            )
            .fetch_optional(tx.conn())
            .await?;
            if inserted.is_none() {
                return Ok(None);
            }
        }
    }
    Ok(Some(added.into_iter().collect()))
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

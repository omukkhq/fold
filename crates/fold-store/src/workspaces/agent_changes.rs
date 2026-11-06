use fold_core::{AgentName, Timestamp};
use sqlx::SqliteExecutor;

use super::columns::timestamp;
use crate::error::StoreError;
use crate::store::WriteTx;

/// How an open change of a worker in the project folder ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChangeEnd {
    /// The human takes the change as their own unrecorded change.
    Ignored,
    /// The paths went into the stash and back to the checkout record's entries.
    Restored,
    /// The human recorded the change knowing a worker made it.
    Recorded,
    /// The folder holds the checkout record's entry again by any other way.
    Settled,
}

impl AgentChangeEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ignored => "ignored",
            Self::Restored => "restored",
            Self::Recorded => "recorded",
            Self::Settled => "settled",
        }
    }
}

/// A path of the project folder that changed during a tool call of `agent`'s worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChangeRow {
    pub path: Vec<u8>,
    pub agent: AgentName,
    /// The file's time that placed the change in the call.
    pub changed_at: Timestamp,
    pub seen_at: Timestamp,
}

/// The open changes, in path order and then agent order.
pub async fn open_agent_changes(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<AgentChangeRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.path AS "path!: Vec<u8>", a.name AS "agent!: String",
                  c.changed_at AS "changed_at!: i64", c.seen_at AS "seen_at!: i64"
           FROM agent_changes c JOIN agents a ON a.id = c.agent_id
           WHERE c.cleared_at IS NULL
           ORDER BY c.path, a.name"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AgentChangeRow {
                path: row.path,
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                changed_at: timestamp(row.changed_at, "agent_changes.changed_at")?,
                seen_at: timestamp(row.seen_at, "agent_changes.seen_at")?,
            })
        })
        .collect()
}

/// Opens the change of `path` for `agent`, unless one is open already, one with the same file time ended already, which is the same change a human settled, or the agent is not registered. True when a row was added.
pub async fn insert_agent_change(
    tx: &mut WriteTx,
    path: &[u8],
    agent: &AgentName,
    changed_at: Timestamp,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (name, changed_at, seen_at) = (agent.as_str(), changed_at.as_millis(), now.as_millis());
    let added = sqlx::query!(
        "INSERT INTO agent_changes (path, agent_id, changed_at, seen_at)
         SELECT ?, a.id, ?, ? FROM agents a
         WHERE a.name = ? AND NOT EXISTS (
             SELECT 1 FROM agent_changes c
             WHERE c.path = ? AND c.agent_id = a.id
               AND (c.cleared_at IS NULL OR c.changed_at = ?)
         )",
        path,
        changed_at,
        seen_at,
        name,
        path,
        changed_at
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(added > 0)
}

/// Ends the open changes of `path`, for every agent, as `how`. Returns how many rows ended.
pub async fn clear_agent_change(
    tx: &mut WriteTx,
    path: &[u8],
    how: AgentChangeEnd,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let (how, cleared_at) = (how.as_str(), now.as_millis());
    Ok(sqlx::query!(
        "UPDATE agent_changes SET cleared_at = ?, cleared_how = ? WHERE path = ? AND cleared_at IS NULL",
        cleared_at,
        how,
        path
    )
    .execute(tx.conn())
    .await?
    .rows_affected())
}

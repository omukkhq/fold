use std::collections::HashMap;

use fold_core::{AgentName, Timestamp};
use sqlx::SqliteExecutor;

use crate::columns::time;
use crate::error::StoreError;
use crate::store::WriteTx;

/// Why a worker left a Claude Code session for a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// fold gave a long-lived worker a fresh context after its hand-off note.
    Handoff,
    /// Claude Code no longer had the session, so fold started the worker in a new one.
    Lost,
    /// A human or the main agent asked for a new session.
    Requested,
}

impl SessionEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Handoff => "handoff",
            Self::Lost => "lost",
            Self::Requested => "requested",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "handoff" => Ok(Self::Handoff),
            "lost" => Ok(Self::Lost),
            "requested" => Ok(Self::Requested),
            _ => Err(StoreError::Corrupt("worker_previous_sessions.reason")),
        }
    }
}

/// A session a worker had before its current one; its id finds the session's Claude Code transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousSessionRow {
    pub session_id: String,
    pub reason: SessionEnd,
    pub ended_at: Timestamp,
}

/// Records the row's current session as a previous one. Call it in the transaction that gives the row its new session, before [`super::refresh_session`].
pub async fn record_previous_session(
    tx: &mut WriteTx,
    worker_process_id: i64,
    reason: SessionEnd,
    now: Timestamp,
) -> Result<(), StoreError> {
    let reason = reason.as_str();
    let now = now.as_millis();
    sqlx::query!(
        "INSERT INTO worker_previous_sessions (worker_process_id, session_id, reason, ended_at)
         SELECT id, session_id, ?2, ?3 FROM worker_processes WHERE id = ?1",
        worker_process_id,
        reason,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The previous sessions of every worker that has one, oldest first, by agent name.
pub async fn previous_sessions(
    executor: impl SqliteExecutor<'_>,
) -> Result<HashMap<AgentName, Vec<PreviousSessionRow>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT g.name AS "agent!: String", s.session_id, s.reason, s.ended_at
           FROM worker_previous_sessions s
           JOIN worker_processes w ON w.id = s.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           ORDER BY s.id"#
    )
    .fetch_all(executor)
    .await?;
    let mut sessions: HashMap<AgentName, Vec<PreviousSessionRow>> = HashMap::new();
    for row in rows {
        let agent = AgentName::parse(&row.agent).map_err(|_| StoreError::Corrupt("agents.name"))?;
        sessions.entry(agent).or_default().push(PreviousSessionRow {
            session_id: row.session_id,
            reason: SessionEnd::parse(&row.reason)?,
            ended_at: time(row.ended_at, "worker_previous_sessions.ended_at")?,
        });
    }
    Ok(sessions)
}

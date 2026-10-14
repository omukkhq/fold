use fold_core::{
    Event, EventActor, EventFilter, EventType, NewEvent, ProposalId, RevisionId, TaskId, Timestamp,
    WorkspaceId,
};
use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::store::WriteTx;

/// The most events one `read_after` call returns.
pub const MAX_PAGE: usize = 1000;

/// Inserts `event` in the caller's write transaction and returns it with its `seq`. `now` comes from the project's `Clock`. The reference columns hold public numbers, not row ids.
pub async fn append(
    tx: &mut WriteTx,
    event: &NewEvent,
    now: Timestamp,
) -> Result<Event, StoreError> {
    if event.event_type.is_live() {
        return Err(StoreError::Invalid("a live event type is never stored"));
    }
    if !event.data.is_object() {
        return Err(StoreError::Invalid("event data is not a JSON object"));
    }
    let event_type = event.event_type.as_str();
    let actor = event.actor.as_str();
    let task = event.task.map(TaskId::to_db);
    let proposal = event.proposal.map(ProposalId::to_db);
    let workspace = event.workspace.map(WorkspaceId::to_db);
    let revision = event.revision.map(RevisionId::to_db);
    let data = event.data.to_string();
    let created_at = now.as_millis();
    let seq = sqlx::query_scalar!(
        r#"INSERT INTO events (type, actor, task_id, proposal_id, workspace_id, revision_id, data, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING seq AS "seq!: i64""#,
        event_type,
        actor,
        task,
        proposal,
        workspace,
        revision,
        data,
        created_at
    )
    .fetch_one(tx.conn())
    .await?;
    Ok(Event {
        seq: u64::try_from(seq).map_err(|_| StoreError::Corrupt("events.seq"))?,
        event_type: event.event_type,
        actor: event.actor.clone(),
        task: event.task,
        proposal: event.proposal,
        workspace: event.workspace,
        revision: event.revision,
        data: event.data.clone(),
        created_at: now,
    })
}

/// Events with `seq > after` in ascending order, selected by `filter` in SQL. `limit` is capped at [`MAX_PAGE`].
pub async fn read_after(
    pool: &SqlitePool,
    after: u64,
    filter: &EventFilter,
    limit: usize,
) -> Result<Vec<Event>, StoreError> {
    let after = i64::try_from(after).map_err(|_| StoreError::Invalid("seq is out of range"))?;
    let limit = i64::try_from(limit.min(MAX_PAGE)).unwrap_or(MAX_PAGE as i64);
    let types = if filter.types.is_empty() {
        None
    } else {
        let names: Vec<&str> = filter.types.iter().map(|t| t.as_str()).collect();
        Some(serde_json::Value::from(names).to_string())
    };
    let task = filter.task.map(TaskId::to_db);
    let proposal = filter.proposal.map(ProposalId::to_db);
    let rows = sqlx::query!(
        r#"SELECT seq AS "seq!: i64", type AS event_type, actor, task_id, proposal_id,
                  workspace_id, revision_id, data, created_at
           FROM events
           WHERE seq > ?1
             AND (?2 IS NULL OR type IN (SELECT value FROM json_each(?2)))
             AND (?3 IS NULL OR task_id = ?3)
             AND (?4 IS NULL OR proposal_id = ?4)
           ORDER BY seq ASC
           LIMIT ?5"#,
        after,
        types,
        task,
        proposal,
        limit
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Event {
                seq: u64::try_from(row.seq).map_err(|_| StoreError::Corrupt("events.seq"))?,
                event_type: EventType::parse(&row.event_type)
                    .map_err(|_| StoreError::Corrupt("events.type"))?,
                actor: EventActor::parse(&row.actor)
                    .map_err(|_| StoreError::Corrupt("events.actor"))?,
                task: reference(row.task_id, TaskId::try_from_db, "events.task_id")?,
                proposal: reference(
                    row.proposal_id,
                    ProposalId::try_from_db,
                    "events.proposal_id",
                )?,
                workspace: reference(
                    row.workspace_id,
                    WorkspaceId::try_from_db,
                    "events.workspace_id",
                )?,
                revision: reference(
                    row.revision_id,
                    RevisionId::try_from_db,
                    "events.revision_id",
                )?,
                data: serde_json::from_str(&row.data)
                    .map_err(|_| StoreError::Corrupt("events.data"))?,
                created_at: Timestamp::from_millis(row.created_at)
                    .map_err(|_| StoreError::Corrupt("events.created_at"))?,
            })
        })
        .collect()
}

/// The newest `seq`, or 0 when no event was ever stored.
pub async fn head_seq(pool: &SqlitePool) -> Result<u64, StoreError> {
    let head = sqlx::query_scalar!(r#"SELECT COALESCE(MAX(seq), 0) AS "head!: i64" FROM events"#)
        .fetch_one(pool)
        .await?;
    u64::try_from(head).map_err(|_| StoreError::Corrupt("events.seq"))
}

fn reference<T>(
    value: Option<i64>,
    convert: fn(i64) -> Option<T>,
    column: &'static str,
) -> Result<Option<T>, StoreError> {
    match value {
        None => Ok(None),
        Some(raw) => convert(raw).map(Some).ok_or(StoreError::Corrupt(column)),
    }
}

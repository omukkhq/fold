use std::collections::BTreeSet;

use fold_core::{
    AgentEventFacts, AgentName, Event, EventActor, EventFilter, EventType, MessageId, NewEvent,
    ProposalId, RevisionId, TaskId, Timestamp, WorkspaceId,
};
use sqlx::{SqliteConnection, SqliteExecutor, SqlitePool};

use crate::error::StoreError;
use crate::store::WriteTx;

/// The most events one `read_after` call returns.
pub const MAX_PAGE: usize = 1000;

/// Inserts `event` in the caller's write transaction and returns it with its `seq`. `now` comes from the project's `Clock`. The reference columns hold public numbers, not row ids; `message_id` must name a stored message.
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
    let message = event.message.map(MessageId::to_db);
    let data = event.data.to_string();
    let created_at = now.as_millis();
    let seq = sqlx::query_scalar!(
        r#"INSERT INTO events (type, actor, task_id, proposal_id, workspace_id, revision_id, message_id, data, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING seq AS "seq!: i64""#,
        event_type,
        actor,
        task,
        proposal,
        workspace,
        revision,
        message,
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

struct EventRecord {
    seq: i64,
    event_type: String,
    actor: String,
    task_id: Option<i64>,
    proposal_id: Option<i64>,
    workspace_id: Option<i64>,
    revision_id: Option<i64>,
    data: String,
    created_at: i64,
}

impl EventRecord {
    fn into_event(self) -> Result<Event, StoreError> {
        Ok(Event {
            seq: u64::try_from(self.seq).map_err(|_| StoreError::Corrupt("events.seq"))?,
            event_type: EventType::parse(&self.event_type)
                .map_err(|_| StoreError::Corrupt("events.type"))?,
            actor: EventActor::parse(&self.actor)
                .map_err(|_| StoreError::Corrupt("events.actor"))?,
            task: reference(self.task_id, TaskId::try_from_db, "events.task_id")?,
            proposal: reference(
                self.proposal_id,
                ProposalId::try_from_db,
                "events.proposal_id",
            )?,
            workspace: reference(
                self.workspace_id,
                WorkspaceId::try_from_db,
                "events.workspace_id",
            )?,
            revision: reference(
                self.revision_id,
                RevisionId::try_from_db,
                "events.revision_id",
            )?,
            data: serde_json::from_str(&self.data)
                .map_err(|_| StoreError::Corrupt("events.data"))?,
            created_at: Timestamp::from_millis(self.created_at)
                .map_err(|_| StoreError::Corrupt("events.created_at"))?,
        })
    }
}

fn seq_param(seq: u64) -> Result<i64, StoreError> {
    i64::try_from(seq).map_err(|_| StoreError::Invalid("seq is out of range"))
}

fn json_list<T>(
    items: impl IntoIterator<Item = T>,
    convert: impl Fn(T) -> serde_json::Value,
) -> String {
    serde_json::Value::Array(items.into_iter().map(convert).collect()).to_string()
}

/// Events with `seq > after` in ascending order, selected by `filter` in SQL. `limit` is capped at [`MAX_PAGE`]. Inside a write transaction it sees every committed event, which a cursor move needs.
pub async fn read_after(
    executor: impl SqliteExecutor<'_>,
    after: u64,
    filter: &EventFilter,
    limit: usize,
) -> Result<Vec<Event>, StoreError> {
    let after = seq_param(after)?;
    let limit = i64::try_from(limit.min(MAX_PAGE)).unwrap_or(MAX_PAGE as i64);
    let types =
        (!filter.types.is_empty()).then(|| json_list(filter.types.iter(), |t| t.as_str().into()));
    let task = filter.task.map(TaskId::to_db);
    let proposal = filter.proposal.map(ProposalId::to_db);
    let rows = sqlx::query_as!(
        EventRecord,
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
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(EventRecord::into_event).collect()
}

/// Events with `seq > after` that carry one of `tasks` or `proposals`, or have one of `types`, in ascending order. It is the coarse SQL side of the relevance rule; the caller applies the rule itself to what it returns.
pub async fn touching_after(
    executor: impl SqliteExecutor<'_>,
    after: u64,
    tasks: &BTreeSet<TaskId>,
    proposals: &BTreeSet<ProposalId>,
    types: &[EventType],
) -> Result<Vec<Event>, StoreError> {
    let after = seq_param(after)?;
    let tasks = json_list(tasks.iter(), |t| t.to_db().into());
    let proposals = json_list(proposals.iter(), |p| p.to_db().into());
    let types = json_list(types.iter(), |t| t.as_str().into());
    let rows = sqlx::query_as!(
        EventRecord,
        r#"SELECT seq AS "seq!: i64", type AS event_type, actor, task_id, proposal_id,
                  workspace_id, revision_id, data, created_at
           FROM events
           WHERE seq > ?1
             AND (type IN (SELECT value FROM json_each(?4))
                  OR task_id IN (SELECT value FROM json_each(?2))
                  OR proposal_id IN (SELECT value FROM json_each(?3)))
           ORDER BY seq ASC"#,
        after,
        tasks,
        proposals,
        types
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(EventRecord::into_event).collect()
}

/// The `seq` the agent has read up to, or 0 when it never moved its cursor.
pub async fn cursor(executor: impl SqliteExecutor<'_>, agent_id: i64) -> Result<u64, StoreError> {
    let last = sqlx::query_scalar!(
        r#"SELECT last_seq AS "last_seq!: i64" FROM agent_cursors WHERE agent_id = ?"#,
        agent_id
    )
    .fetch_optional(executor)
    .await?;
    match last {
        None => Ok(0),
        Some(last) => {
            u64::try_from(last).map_err(|_| StoreError::Corrupt("agent_cursors.last_seq"))
        }
    }
}

/// Moves the agent's cursor to `seq`. A cursor already past `seq` stays where it is, so the cursor only moves forward.
pub async fn advance_cursor(tx: &mut WriteTx, agent_id: i64, seq: u64) -> Result<(), StoreError> {
    let seq = seq_param(seq)?;
    sqlx::query!(
        "INSERT INTO agent_cursors (agent_id, last_seq) VALUES (?1, ?2)
         ON CONFLICT (agent_id) DO UPDATE SET last_seq = MAX(last_seq, excluded.last_seq)",
        agent_id,
        seq
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Whether a worker of the agent receives the message that the event `seq` is about, read from `events.message_id`. An event about no message reaches nobody here.
pub async fn is_recipient(
    executor: impl SqliteExecutor<'_>,
    seq: u64,
    agent_id: i64,
) -> Result<bool, StoreError> {
    let seq = seq_param(seq)?;
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM events e
               JOIN messages m ON m.number = e.message_id
               JOIN message_recipients r ON r.message_id = m.id
               JOIN workers w ON w.id = r.worker_id
               WHERE e.seq = ?1 AND w.agent_id = ?2
           ) AS "found!: bool""#,
        seq,
        agent_id
    )
    .fetch_one(executor)
    .await?;
    Ok(found)
}

/// The claims, prerequisites and proposals of every worker of the agent. The caller passes one read transaction, so the four reads see one state.
pub async fn agent_facts(
    conn: &mut SqliteConnection,
    agent_id: i64,
    agent: &AgentName,
) -> Result<AgentEventFacts, StoreError> {
    let mut facts = AgentEventFacts::new(agent.clone());
    let claims = sqlx::query!(
        r#"SELECT t.number AS "task?: i64", p.number AS "proposal?: i64"
           FROM claims c
           JOIN workers w ON w.id = c.worker_id
           LEFT JOIN tasks t ON t.id = c.task_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           WHERE w.agent_id = ? AND c.released_at IS NULL"#,
        agent_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for claim in claims {
        if let Some(task) = claim.task {
            facts
                .claimed
                .insert(TaskId::try_from_db(task).ok_or(StoreError::Corrupt("tasks.number"))?);
        }
        if let Some(proposal) = claim.proposal {
            facts.reviewing.insert(
                ProposalId::try_from_db(proposal).ok_or(StoreError::Corrupt("proposals.number"))?,
            );
        }
    }
    let prerequisites = sqlx::query_scalar!(
        r#"SELECT n.number AS "number!: i64"
           FROM dependencies d
           JOIN tasks n ON n.id = d.needs_task_id
           JOIN claims c ON c.task_id = d.task_id
           JOIN workers w ON w.id = c.worker_id
           WHERE w.agent_id = ? AND c.kind = 'implementation' AND c.released_at IS NULL"#,
        agent_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for number in prerequisites {
        facts
            .prerequisites
            .insert(TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number"))?);
    }
    let authored = sqlx::query_scalar!(
        r#"SELECT p.number AS "number!: i64"
           FROM proposals p
           JOIN workers w ON w.id = p.author_worker_id
           WHERE w.agent_id = ?"#,
        agent_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for number in authored {
        facts.authored.insert(
            ProposalId::try_from_db(number).ok_or(StoreError::Corrupt("proposals.number"))?,
        );
    }
    Ok(facts)
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

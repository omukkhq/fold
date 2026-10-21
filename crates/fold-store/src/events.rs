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
    let reason = event.reason.as_deref();
    let data = event.data.to_string();
    let created_at = now.as_millis();
    let seq = sqlx::query_scalar!(
        r#"INSERT INTO events (type, actor, task_id, proposal_id, workspace_id, revision_id, message_id, reason, data, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING seq AS "seq!: i64""#,
        event_type,
        actor,
        task,
        proposal,
        workspace,
        revision,
        message,
        reason,
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

/// Every event with `seq > after` in ascending order, at most `limit` (capped at [`MAX_PAGE`]). It is the replay read of the event stream, which sends every type.
pub async fn after(pool: &SqlitePool, after: u64, limit: usize) -> Result<Vec<Event>, StoreError> {
    read_after(pool, after, &EventFilter::default(), limit).await
}

/// Who reads a wait's page, and which events reach them besides the filter's.
#[derive(Debug, Clone, Copy)]
pub struct Reach<'a> {
    pub agent_id: i64,
    /// An event of one of these types is selected only when a worker of the agent receives the message it names in `events.message_id`.
    pub message_types: &'a [EventType],
    /// An event of one of these types is selected only when a worker of the agent performed it.
    pub own_types: &'a [EventType],
    /// The messages that end a wait whatever its filter says.
    pub wake: Option<MessageWake>,
}

/// An event of `event_type` is selected past the filter when the worker row `worker_id` receives the message it names, or with `every_worker` when any worker of the agent does, on a row that no inbox read or acknowledgement has reached yet, and only when that worker did not send it: a wait wakes for a new message, never for one the agent has seen or its own.
#[derive(Debug, Clone, Copy)]
pub struct MessageWake {
    pub event_type: EventType,
    pub worker_id: i64,
    pub every_worker: bool,
}

/// [`read_after`] for one reader: the filter's events that reach the agent by the rules of [`Reach`], and the wake's message events whatever the filter says, so one query returns the page an agent's wait keeps.
pub async fn read_reaching(
    executor: impl SqliteExecutor<'_>,
    after: u64,
    filter: &EventFilter,
    reach: Reach<'_>,
    limit: usize,
) -> Result<Vec<Event>, StoreError> {
    let after = seq_param(after)?;
    let limit = i64::try_from(limit.min(MAX_PAGE)).unwrap_or(MAX_PAGE as i64);
    let types =
        (!filter.types.is_empty()).then(|| json_list(filter.types.iter(), |t| t.as_str().into()));
    let task = filter.task.map(TaskId::to_db);
    let proposal = filter.proposal.map(ProposalId::to_db);
    let agent_id = reach.agent_id;
    let message_types = json_list(reach.message_types.iter(), |t| t.as_str().into());
    let own_types = json_list(reach.own_types.iter(), |t| t.as_str().into());
    let wake_type = reach.wake.map(|wake| wake.event_type.as_str());
    let wake_worker = reach.wake.map(|wake| wake.worker_id);
    let wake_every = reach.wake.is_some_and(|wake| wake.every_worker);
    let rows = sqlx::query_as!(
        EventRecord,
        r#"SELECT e.seq AS "seq!: i64", e.type AS event_type, e.actor, e.task_id, e.proposal_id,
                  e.workspace_id, e.revision_id, e.data, e.created_at
           FROM events e
           WHERE e.seq > ?1
             AND (((?2 IS NULL OR e.type IN (SELECT value FROM json_each(?2)))
                   AND (?3 IS NULL OR e.task_id = ?3)
                   AND (?4 IS NULL OR e.proposal_id = ?4))
                  OR (e.type = ?9
                      AND e.actor <> (SELECT s.name FROM workers s WHERE s.id = ?10)
                      AND EXISTS (
                          SELECT 1 FROM messages m
                          JOIN message_recipients r ON r.message_id = m.id
                          JOIN workers w ON w.id = r.worker_id
                          WHERE m.number = e.message_id
                            AND r.delivered_at IS NULL AND r.acked_at IS NULL
                            AND (r.worker_id = ?10 OR (?11 AND w.agent_id = ?5)))))
             AND (e.type NOT IN (SELECT value FROM json_each(?6))
                  OR EXISTS (
                      SELECT 1 FROM messages m
                      JOIN message_recipients r ON r.message_id = m.id
                      JOIN workers w ON w.id = r.worker_id
                      WHERE m.number = e.message_id AND w.agent_id = ?5))
             AND (e.type NOT IN (SELECT value FROM json_each(?8))
                  OR EXISTS (
                      SELECT 1 FROM agents g
                      WHERE g.id = ?5
                        AND (e.actor = g.name OR substr(e.actor, 1, length(g.name) + 1) = g.name || '/')))
           ORDER BY e.seq ASC
           LIMIT ?7"#,
        after,
        types,
        task,
        proposal,
        agent_id,
        message_types,
        limit,
        own_types,
        wake_type,
        wake_worker,
        wake_every
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(EventRecord::into_event).collect()
}

/// How many events with `seq > after` concern `agent`, counted up to `cap` and no further: the event has one of `types`, or carries one of `tasks` or `proposals`, and no worker of `agent` performed it. With the agent's facts as `tasks`, `proposals` and `types`, it is the rule of `fold_core::concerns` in SQL, so the count never loads the events. A worker actor is the agent's name or a name under `agent/`, the range from `agent/` up to `agent0` that [`newest_by_agent`] also reads.
pub async fn count_concerning(
    executor: impl SqliteExecutor<'_>,
    after: u64,
    agent: &AgentName,
    tasks: &BTreeSet<TaskId>,
    proposals: &BTreeSet<ProposalId>,
    types: &[EventType],
    cap: u64,
) -> Result<u64, StoreError> {
    let after = seq_param(after)?;
    let agent = agent.as_str();
    let tasks = json_list(tasks.iter(), |t| t.to_db().into());
    let proposals = json_list(proposals.iter(), |p| p.to_db().into());
    let types = json_list(types.iter(), |t| t.as_str().into());
    let cap = i64::try_from(cap).unwrap_or(i64::MAX);
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM (
               SELECT 1 FROM events
               WHERE seq > ?1
                 AND NOT (actor = ?2 OR (actor >= ?2 || '/' AND actor < ?2 || '0'))
                 AND (type IN (SELECT value FROM json_each(?5))
                      OR task_id IN (SELECT value FROM json_each(?3))
                      OR proposal_id IN (SELECT value FROM json_each(?4)))
               LIMIT ?6
           )"#,
        after,
        agent,
        tasks,
        proposals,
        types,
        cap
    )
    .fetch_one(executor)
    .await?;
    u64::try_from(count).map_err(|_| StoreError::Corrupt("events"))
}

/// The newest `limit` events that carry `task`, newest first.
pub async fn newest_for_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
    limit: usize,
) -> Result<Vec<Event>, StoreError> {
    let task = task.to_db();
    let limit = i64::try_from(limit.min(MAX_PAGE)).unwrap_or(MAX_PAGE as i64);
    let rows = sqlx::query_as!(
        EventRecord,
        r#"SELECT seq AS "seq!: i64", type AS event_type, actor, task_id, proposal_id,
                  workspace_id, revision_id, data, created_at
           FROM events
           WHERE task_id = ?1
           ORDER BY seq DESC
           LIMIT ?2"#,
        task,
        limit
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(EventRecord::into_event).collect()
}

/// When the newest event of `event_type` that carries `task` happened, or `None` without one.
pub async fn newest_at_for_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
    event_type: EventType,
) -> Result<Option<Timestamp>, StoreError> {
    let task = task.to_db();
    let event_type = event_type.as_str();
    let at = sqlx::query_scalar!(
        "SELECT created_at FROM events WHERE task_id = ?1 AND type = ?2 ORDER BY seq DESC LIMIT 1",
        task,
        event_type
    )
    .fetch_optional(executor)
    .await?;
    at.map(|millis| {
        Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt("events.created_at"))
    })
    .transpose()
}

/// The newest `limit` events that a worker of `agent` performed, newest first. The actor column holds the worker name, so the agent's own name or a name under `agent/` matches. The children are the range from `agent/` up to `agent0`, because `0` follows `/`; a range, unlike a pattern built from a parameter, can use `events_actor_seq`.
pub async fn newest_by_agent(
    executor: impl SqliteExecutor<'_>,
    agent: &AgentName,
    limit: usize,
) -> Result<Vec<Event>, StoreError> {
    let agent = agent.as_str();
    let limit = i64::try_from(limit.min(MAX_PAGE)).unwrap_or(MAX_PAGE as i64);
    let rows = sqlx::query_as!(
        EventRecord,
        r#"SELECT seq AS "seq!: i64", type AS event_type, actor, task_id, proposal_id,
                  workspace_id, revision_id, data, created_at
           FROM events
           WHERE actor = ?1 OR (actor >= ?1 || '/' AND actor < ?1 || '0')
           ORDER BY seq DESC
           LIMIT ?2"#,
        agent,
        limit
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

/// Moves the agent's cursor from `from` to `to` only while it still stands at `from` (0 also when the agent has no cursor yet). False when another read moved it first, and then nothing changes.
pub async fn move_cursor(
    tx: &mut WriteTx,
    agent_id: i64,
    from: u64,
    to: u64,
) -> Result<bool, StoreError> {
    let (from, to) = (seq_param(from)?, seq_param(to)?);
    let moved = if from == 0 {
        sqlx::query!(
            "INSERT INTO agent_cursors (agent_id, last_seq) VALUES (?1, ?2)
             ON CONFLICT (agent_id) DO UPDATE SET last_seq = excluded.last_seq
             WHERE agent_cursors.last_seq = 0",
            agent_id,
            to
        )
        .execute(tx.conn())
        .await?
    } else {
        sqlx::query!(
            "UPDATE agent_cursors SET last_seq = ?3 WHERE agent_id = ?1 AND last_seq = ?2",
            agent_id,
            from,
            to
        )
        .execute(tx.conn())
        .await?
    };
    Ok(moved.rows_affected() == 1)
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

/// The agents with a worker among the recipients of the message that the event `seq` names in `events.message_id`, in name order. Human recipients are left out.
pub async fn recipient_agents(
    executor: impl SqliteExecutor<'_>,
    seq: u64,
) -> Result<Vec<AgentName>, StoreError> {
    let seq = seq_param(seq)?;
    let names = sqlx::query_scalar!(
        r#"SELECT DISTINCT a.name AS "name!: String" FROM events e
           JOIN messages m ON m.number = e.message_id
           JOIN message_recipients r ON r.message_id = m.id
           JOIN workers w ON w.id = r.worker_id
           JOIN agents a ON a.id = w.agent_id
           WHERE e.seq = ?1
           ORDER BY a.name"#,
        seq
    )
    .fetch_all(executor)
    .await?;
    names
        .iter()
        .map(|name| AgentName::parse(name).map_err(|_| StoreError::Corrupt("agents.name")))
        .collect()
}

/// The claims, prerequisites and proposals of every worker of the agent. The caller passes one read transaction, so its three queries see one state.
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

/// When the newest stored event happened, or `None` when no event was ever stored. `created_at` has no index, so the newest event is found by `seq`.
pub async fn newest_at(pool: &SqlitePool) -> Result<Option<Timestamp>, StoreError> {
    let at = sqlx::query_scalar!("SELECT created_at FROM events ORDER BY seq DESC LIMIT 1")
        .fetch_optional(pool)
        .await?;
    at.map(|millis| {
        Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt("events.created_at"))
    })
    .transpose()
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

use std::collections::{BTreeSet, HashMap};

use fold_core::{
    AddressedAs, AgentName, Directory, DirectoryAgent, HumanName, MessageId, ProposalId, Recipient,
    RequestId, Sender, Tag, Target, TaskId, Timestamp, WorkerName,
};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::agents;
use crate::error::StoreError;
use crate::store::WriteTx;

/// The values of a new message. Every reference is a public id; the insert resolves each to its row.
#[derive(Debug, Clone)]
pub struct NewMessage<'a> {
    pub number: MessageId,
    /// The number of the thread's first message; equal to `number` for a message that starts a thread.
    pub thread: MessageId,
    pub reply_to: Option<MessageId>,
    pub sender: &'a Sender,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    /// The request the message clarifies; every message of a request shares one thread.
    pub request: Option<RequestId>,
    pub body: &'a str,
    pub now: Timestamp,
}

/// What a reply needs from its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHead {
    pub id: MessageId,
    pub thread: MessageId,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub request: Option<RequestId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientRow {
    pub target: Target,
    pub addressed_as: AddressedAs,
    pub delivered_at: Option<Timestamp>,
    pub acked_at: Option<Timestamp>,
}

/// A stored message with its recipients in the order they were written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: MessageId,
    pub thread: MessageId,
    pub reply_to: Option<MessageId>,
    pub sender: Sender,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub request: Option<RequestId>,
    pub body: String,
    pub created_at: Timestamp,
    pub recipients: Vec<RecipientRow>,
    /// The options of a multiple-choice question, in order; empty for any other message.
    pub choices: Vec<String>,
}

/// Inserts the message. A sender worker, task or proposal that does not exist leaves its column empty, which the sender check or the caller's earlier lookup refuses.
pub async fn insert(tx: &mut WriteTx, message: &NewMessage<'_>) -> Result<(), StoreError> {
    let number = message.number.to_db();
    let thread = message.thread.to_db();
    let reply_to = message.reply_to.map(MessageId::to_db);
    let kind = message.sender.kind();
    let (agent, worker, human) = match message.sender {
        Sender::Worker(worker) => (Some(worker.agent()), Some(worker.as_str()), None),
        Sender::Human(name) => (None, None, Some(name.as_str())),
        Sender::Fold => (None, None, None),
    };
    let agent = agent.as_ref().map(AgentName::as_str);
    let task = message.task.map(TaskId::to_db);
    let proposal = message.proposal.map(ProposalId::to_db);
    let request = message.request.map(RequestId::to_db);
    let body = message.body;
    let now = message.now.as_millis();
    sqlx::query!(
        "INSERT INTO messages
             (number, thread_id, reply_to, sender_kind, sender_worker_id, sender_human,
              task_id, proposal_id, request_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4,
                 (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                  WHERE a.name = ?11 AND w.name = ?5), ?6,
                 (SELECT id FROM tasks WHERE number = ?7),
                 (SELECT id FROM proposals WHERE number = ?8),
                 (SELECT id FROM requests WHERE number = ?12), ?9, ?10)",
        number,
        thread,
        reply_to,
        kind,
        worker,
        human,
        task,
        proposal,
        body,
        now,
        agent,
        request
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Stores the options of a question message, in their order.
pub async fn insert_choices(
    tx: &mut WriteTx,
    message: MessageId,
    labels: &[String],
) -> Result<(), StoreError> {
    let number = message.to_db();
    for (position, label) in (1_i64..).zip(labels) {
        sqlx::query!(
            "INSERT INTO message_choices (message_id, position, label)
             VALUES ((SELECT id FROM messages WHERE number = ?1), ?2, ?3)",
            number,
            position,
            label
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Inserts one undelivered, unacknowledged recipient row of `message`. A second row for the same target fails on the unique constraint.
pub async fn insert_recipient(
    tx: &mut WriteTx,
    message: MessageId,
    recipient: &Recipient,
) -> Result<(), StoreError> {
    let number = message.to_db();
    let (agent, worker, human) = match &recipient.target {
        Target::Worker(worker) => (Some(worker.agent()), Some(worker.as_str()), None),
        Target::Human(name) => (None, None, Some(name.as_str())),
    };
    let agent = agent.as_ref().map(AgentName::as_str);
    let addressed_as = recipient.addressed_as.as_str();
    sqlx::query!(
        "INSERT INTO message_recipients (message_id, worker_id, human, addressed_as)
         VALUES ((SELECT id FROM messages WHERE number = ?1),
                 (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                  WHERE a.name = ?5 AND w.name = ?2), ?3, ?4)",
        number,
        worker,
        human,
        addressed_as,
        agent
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub async fn find(
    conn: &mut SqliteConnection,
    message: MessageId,
) -> Result<Option<MessageHead>, StoreError> {
    let number = message.to_db();
    let row = sqlx::query!(
        r#"SELECT m.thread_id, t.number AS "task?: i64", p.number AS "proposal?: i64",
                  rq.number AS "request?: i64"
           FROM messages m
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE m.number = ?"#,
        number
    )
    .fetch_optional(conn)
    .await?;
    row.map(|row| {
        Ok(MessageHead {
            id: message,
            thread: message_id(row.thread_id, "messages.thread_id")?,
            task: row.task.map(task_id).transpose()?,
            proposal: row.proposal.map(proposal_id).transpose()?,
            request: row.request.map(request_id).transpose()?,
        })
    })
    .transpose()
}

/// The thread of the request's messages: the thread its first message started. `None` before anyone wrote about the request.
pub async fn request_thread(
    conn: &mut SqliteConnection,
    request: RequestId,
) -> Result<Option<MessageId>, StoreError> {
    let number = request.to_db();
    let thread = sqlx::query_scalar!(
        r#"SELECT m.thread_id AS "thread!: i64"
           FROM messages m JOIN requests rq ON rq.id = m.request_id
           WHERE rq.number = ?
           ORDER BY m.number
           LIMIT 1"#,
        number
    )
    .fetch_optional(conn)
    .await?;
    thread
        .map(|thread| message_id(thread, "messages.thread_id"))
        .transpose()
}

/// The agents with their tags, every worker, and `humans` from the registry, as one snapshot for `fold_core::messages::resolve`. Pass the writer's connection so the snapshot matches the rows the send inserts.
pub async fn directory(
    conn: &mut SqliteConnection,
    humans: Vec<HumanName>,
) -> Result<Directory, StoreError> {
    let rows = agents::list_agents(&mut *conn).await?;
    let mut tags_of: HashMap<i64, Vec<Tag>> = HashMap::new();
    for (agent_id, tag) in agents::all_tags(&mut *conn).await? {
        tags_of.entry(agent_id).or_default().push(tag);
    }
    let workers = agents::all_workers(&mut *conn).await?;
    Ok(Directory {
        agents: rows
            .into_iter()
            .map(|row| DirectoryAgent {
                tags: tags_of.remove(&row.id).unwrap_or_default(),
                name: row.name,
            })
            .collect(),
        workers: workers.into_iter().map(|worker| worker.name).collect(),
        humans,
    })
}

/// The messages of one thread in number order. An unknown thread has none.
pub async fn thread(
    conn: &mut SqliteConnection,
    thread: MessageId,
) -> Result<Vec<MessageRow>, StoreError> {
    let number = thread.to_db();
    let rows = sqlx::query_as!(
        RawMessage,
        r#"SELECT m.number, m.thread_id, m.reply_to, m.sender_kind, w.name AS "worker?: String",
                  m.sender_human, t.number AS "task?: i64", p.number AS "proposal?: i64",
                  rq.number AS "request?: i64", m.body, m.created_at
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE m.thread_id = ?
           ORDER BY m.number"#,
        number
    )
    .fetch_all(&mut *conn)
    .await?;
    with_recipients(conn, rows).await
}

/// Messages from a worker to a human with no later message from a human in their thread, in number order.
pub async fn awaiting_human(conn: &mut SqliteConnection) -> Result<Vec<MessageRow>, StoreError> {
    let rows = sqlx::query_as!(
        RawMessage,
        r#"SELECT m.number, m.thread_id, m.reply_to, m.sender_kind, w.name AS "worker?: String",
                  m.sender_human, t.number AS "task?: i64", p.number AS "proposal?: i64",
                  rq.number AS "request?: i64", m.body, m.created_at
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE m.sender_kind = 'worker'
             AND EXISTS (
                 SELECT 1 FROM message_recipients r
                 WHERE r.message_id = m.id AND r.addressed_as IN ('human', 'humans')
             )
             AND NOT EXISTS (
                 SELECT 1 FROM messages later
                 WHERE later.thread_id = m.thread_id
                   AND later.number > m.number
                   AND later.sender_kind = 'human'
             )
           ORDER BY m.number"#
    )
    .fetch_all(&mut *conn)
    .await?;
    with_recipients(conn, rows).await
}

/// One message with its recipients; an unknown number has none.
pub async fn get(
    conn: &mut SqliteConnection,
    message: MessageId,
) -> Result<Option<MessageRow>, StoreError> {
    let number = message.to_db();
    let rows = sqlx::query_as!(
        RawMessage,
        r#"SELECT m.number, m.thread_id, m.reply_to, m.sender_kind, w.name AS "worker?: String",
                  m.sender_human, t.number AS "task?: i64", p.number AS "proposal?: i64",
                  rq.number AS "request?: i64", m.body, m.created_at
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE m.number = ?"#,
        number
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(with_recipients(conn, rows).await?.pop())
}

/// One unacknowledged recipient row of a worker. `message` carries no recipients: the row itself is the one that matters to its reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxRow {
    /// The `message_recipients` row id; it never leaves the daemon.
    pub row: i64,
    pub worker: WorkerName,
    pub addressed_as: AddressedAs,
    pub delivered_at: Option<Timestamp>,
    pub message: MessageRow,
}

/// The unacknowledged rows of the workers `worker_ids`, ordered by message number and then worker.
pub async fn inbox(
    conn: &mut SqliteConnection,
    worker_ids: &[i64],
) -> Result<Vec<InboxRow>, StoreError> {
    let ids = id_list(worker_ids)?;
    let rows = sqlx::query!(
        r#"SELECT r.id AS "row!: i64", rw.name AS recipient, r.addressed_as, r.delivered_at,
                  m.number, m.thread_id, m.reply_to, m.sender_kind,
                  w.name AS "worker?: String", m.sender_human, t.number AS "task?: i64",
                  p.number AS "proposal?: i64", rq.number AS "request?: i64", m.body,
                  m.created_at
           FROM message_recipients r
           JOIN workers rw ON rw.id = r.worker_id
           JOIN messages m ON m.id = r.message_id
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE r.worker_id IN (SELECT value FROM json_each(?)) AND r.acked_at IS NULL
           ORDER BY m.number, rw.id"#,
        ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let numbers = serde_json::to_string(&rows.iter().map(|row| row.number).collect::<Vec<_>>())
        .map_err(|_| StoreError::Invalid("message numbers"))?;
    let choices = choices_of(conn, &numbers).await?;
    rows.into_iter()
        .map(|row| {
            let labels = choices.get(&row.number).cloned().unwrap_or_default();
            let message = RawMessage {
                number: row.number,
                thread_id: row.thread_id,
                reply_to: row.reply_to,
                sender_kind: row.sender_kind,
                worker: row.worker,
                sender_human: row.sender_human,
                task: row.task,
                proposal: row.proposal,
                request: row.request,
                body: row.body,
                created_at: row.created_at,
            }
            .into_row(Vec::new(), labels)?;
            Ok(InboxRow {
                row: row.row,
                worker: WorkerName::parse(&row.recipient)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                addressed_as: row
                    .addressed_as
                    .parse()
                    .map_err(|()| StoreError::Corrupt("message_recipients.addressed_as"))?,
                delivered_at: row
                    .delivered_at
                    .map(|millis| time(millis, "message_recipients.delivered_at"))
                    .transpose()?,
                message,
            })
        })
        .collect()
}

/// Sets `delivered_at` on the rows `rows` that have none; a delivered row keeps its first time.
pub async fn mark_delivered(
    tx: &mut WriteTx,
    rows: &[i64],
    now: Timestamp,
) -> Result<(), StoreError> {
    let ids = id_list(rows)?;
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE message_recipients SET delivered_at = ?1
         WHERE id IN (SELECT value FROM json_each(?2)) AND delivered_at IS NULL",
        now,
        ids
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Acknowledges the worker's row of `message`, or with `up_to` every unacknowledged row of the worker up to that number, and returns the numbers it changed in order. `None` when the worker has no row for `message`; a row acknowledged before keeps its time.
pub async fn acknowledge(
    tx: &mut WriteTx,
    worker_id: i64,
    message: MessageId,
    up_to: bool,
    now: Timestamp,
) -> Result<Option<Vec<MessageId>>, StoreError> {
    let number = message.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM message_recipients r JOIN messages m ON m.id = r.message_id
               WHERE r.worker_id = ?1 AND m.number = ?2
           ) AS "found!: bool""#,
        worker_id,
        number
    )
    .fetch_one(tx.conn())
    .await?;
    if !found {
        return Ok(None);
    }
    let low = if up_to { 1 } else { number };
    let rows = sqlx::query!(
        r#"SELECT r.id AS "row!: i64", m.number
           FROM message_recipients r JOIN messages m ON m.id = r.message_id
           WHERE r.worker_id = ?1 AND r.acked_at IS NULL AND m.number BETWEEN ?2 AND ?3
           ORDER BY m.number"#,
        worker_id,
        low,
        number
    )
    .fetch_all(tx.conn())
    .await?;
    let ids = id_list(&rows.iter().map(|row| row.row).collect::<Vec<_>>())?;
    let millis = now.as_millis();
    sqlx::query!(
        "UPDATE message_recipients SET acked_at = ?1 WHERE id IN (SELECT value FROM json_each(?2))",
        millis,
        ids
    )
    .execute(tx.conn())
    .await?;
    rows.into_iter()
        .map(|row| message_id(row.number, "messages.number"))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The unacknowledged rows of every worker of the agent.
pub async fn unread_count(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<u64, StoreError> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM workers w
           JOIN message_recipients r ON r.worker_id = w.id
           WHERE w.agent_id = ? AND r.acked_at IS NULL"#,
        agent_id
    )
    .fetch_one(executor)
    .await?;
    u64::try_from(count).map_err(|_| StoreError::Corrupt("message_recipients"))
}

/// Which threads a listing returns. Every set field must match some message of the thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFilter {
    /// A worker of this agent sent or received a message of the thread.
    pub agent: Option<AgentName>,
    /// This exact worker sent or received a message of the thread.
    pub worker: Option<WorkerName>,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    /// Only threads that hold a question no human has answered.
    pub awaiting_human: bool,
    /// Only threads whose newest message is older than this number.
    pub before: Option<MessageId>,
}

/// A thread of a listing: its id and its newest message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadHead {
    pub thread: MessageId,
    pub last: MessageId,
}

/// Up to `limit` threads that match, newest message first.
pub async fn thread_page(
    conn: &mut SqliteConnection,
    filter: &ThreadFilter,
    limit: u32,
) -> Result<Vec<ThreadHead>, StoreError> {
    let agent = filter.agent.as_ref().map(AgentName::as_str);
    let worker = filter.worker.as_ref().map(WorkerName::as_str);
    let task = filter.task.map(TaskId::to_db);
    let proposal = filter.proposal.map(ProposalId::to_db);
    let awaiting = filter.awaiting_human;
    let before = filter.before.map(MessageId::to_db);
    let limit = i64::from(limit);
    let rows = sqlx::query!(
        r#"SELECT m.thread_id AS "thread!: i64", MAX(m.number) AS "last!: i64"
           FROM messages m
           WHERE (?1 IS NULL OR EXISTS (
                     SELECT 1 FROM messages x JOIN tasks t ON t.id = x.task_id
                     WHERE x.thread_id = m.thread_id AND t.number = ?1))
             AND (?2 IS NULL OR EXISTS (
                     SELECT 1 FROM messages x JOIN proposals p ON p.id = x.proposal_id
                     WHERE x.thread_id = m.thread_id AND p.number = ?2))
             AND (?3 IS NULL OR EXISTS (
                     SELECT 1 FROM messages x
                     JOIN workers w ON w.id = x.sender_worker_id
                     JOIN agents a ON a.id = w.agent_id
                     WHERE x.thread_id = m.thread_id AND a.name = ?3
                     UNION ALL
                     SELECT 1 FROM messages x
                     JOIN message_recipients r ON r.message_id = x.id
                     JOIN workers w ON w.id = r.worker_id
                     JOIN agents a ON a.id = w.agent_id
                     WHERE x.thread_id = m.thread_id AND a.name = ?3))
             AND (?4 IS NULL OR EXISTS (
                     SELECT 1 FROM messages x
                     JOIN workers w ON w.id = x.sender_worker_id
                     WHERE x.thread_id = m.thread_id AND w.name = ?4
                     UNION ALL
                     SELECT 1 FROM messages x
                     JOIN message_recipients r ON r.message_id = x.id
                     JOIN workers w ON w.id = r.worker_id
                     WHERE x.thread_id = m.thread_id AND w.name = ?4))
             AND (NOT ?5 OR EXISTS (
                     SELECT 1 FROM messages q
                     WHERE q.thread_id = m.thread_id
                       AND q.sender_kind = 'worker'
                       AND EXISTS (
                           SELECT 1 FROM message_recipients r
                           WHERE r.message_id = q.id AND r.addressed_as IN ('human', 'humans'))
                       AND NOT EXISTS (
                           SELECT 1 FROM messages later
                           WHERE later.thread_id = q.thread_id
                             AND later.number > q.number
                             AND later.sender_kind = 'human')))
           GROUP BY m.thread_id
           HAVING ?6 IS NULL OR MAX(m.number) < ?6
           ORDER BY MAX(m.number) DESC
           LIMIT ?7"#,
        task,
        proposal,
        agent,
        worker,
        awaiting,
        before,
        limit
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ThreadHead {
                thread: message_id(row.thread, "messages.thread_id")?,
                last: message_id(row.last, "messages.number")?,
            })
        })
        .collect()
}

/// Every message of the threads `threads`, ordered by thread and then number.
pub async fn threads(
    conn: &mut SqliteConnection,
    threads: &[MessageId],
) -> Result<Vec<MessageRow>, StoreError> {
    let numbers = id_list(&threads.iter().map(|id| id.to_db()).collect::<Vec<_>>())?;
    let rows = sqlx::query_as!(
        RawMessage,
        r#"SELECT m.number, m.thread_id, m.reply_to, m.sender_kind, w.name AS "worker?: String",
                  m.sender_human, t.number AS "task?: i64", p.number AS "proposal?: i64",
                  rq.number AS "request?: i64", m.body, m.created_at
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE m.thread_id IN (SELECT value FROM json_each(?))
           ORDER BY m.thread_id, m.number"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    with_recipients(conn, rows).await
}

/// The threads that hold a question from a worker to a human with no later human message, as `awaiting_human` decides.
pub async fn threads_awaiting_human(
    executor: impl SqliteExecutor<'_>,
) -> Result<BTreeSet<MessageId>, StoreError> {
    let rows = sqlx::query_scalar!(
        r#"SELECT DISTINCT m.thread_id AS "thread!: i64"
           FROM messages m
           WHERE m.sender_kind = 'worker'
             AND EXISTS (
                 SELECT 1 FROM message_recipients r
                 WHERE r.message_id = m.id AND r.addressed_as IN ('human', 'humans')
             )
             AND NOT EXISTS (
                 SELECT 1 FROM messages later
                 WHERE later.thread_id = m.thread_id
                   AND later.number > m.number
                   AND later.sender_kind = 'human'
             )"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|thread| message_id(thread, "messages.thread_id"))
        .collect()
}

/// The tasks linked to a question from a worker to a human with no later human message in its thread: the source of the `question` attention kind.
pub async fn tasks_awaiting_human(
    executor: impl SqliteExecutor<'_>,
) -> Result<BTreeSet<TaskId>, StoreError> {
    let rows = sqlx::query_scalar!(
        r#"SELECT DISTINCT t.number AS "task!: i64"
           FROM messages m
           JOIN tasks t ON t.id = m.task_id
           WHERE m.sender_kind = 'worker'
             AND EXISTS (
                 SELECT 1 FROM message_recipients r
                 WHERE r.message_id = m.id AND r.addressed_as IN ('human', 'humans')
             )
             AND NOT EXISTS (
                 SELECT 1 FROM messages later
                 WHERE later.thread_id = m.thread_id
                   AND later.number > m.number
                   AND later.sender_kind = 'human'
             )"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(task_id).collect()
}

fn id_list(ids: &[i64]) -> Result<String, StoreError> {
    serde_json::to_string(ids).map_err(|_| StoreError::Invalid("row ids"))
}

struct RawMessage {
    number: i64,
    thread_id: i64,
    reply_to: Option<i64>,
    sender_kind: String,
    worker: Option<String>,
    sender_human: Option<String>,
    task: Option<i64>,
    proposal: Option<i64>,
    request: Option<i64>,
    body: String,
    created_at: i64,
}

impl RawMessage {
    fn into_row(
        self,
        recipients: Vec<RecipientRow>,
        choices: Vec<String>,
    ) -> Result<MessageRow, StoreError> {
        let sender = match (self.sender_kind.as_str(), self.worker, self.sender_human) {
            ("worker", Some(worker), None) => Sender::Worker(
                WorkerName::parse(&worker).map_err(|_| StoreError::Corrupt("workers.name"))?,
            ),
            ("human", None, Some(name)) => Sender::Human(
                HumanName::parse(&name)
                    .map_err(|_| StoreError::Corrupt("messages.sender_human"))?,
            ),
            ("fold", None, None) => Sender::Fold,
            _ => return Err(StoreError::Corrupt("messages.sender_kind")),
        };
        Ok(MessageRow {
            id: message_id(self.number, "messages.number")?,
            thread: message_id(self.thread_id, "messages.thread_id")?,
            reply_to: self
                .reply_to
                .map(|n| message_id(n, "messages.reply_to"))
                .transpose()?,
            sender,
            task: self.task.map(task_id).transpose()?,
            proposal: self.proposal.map(proposal_id).transpose()?,
            request: self.request.map(request_id).transpose()?,
            body: self.body,
            created_at: time(self.created_at, "messages.created_at")?,
            recipients,
            choices,
        })
    }
}

async fn with_recipients(
    conn: &mut SqliteConnection,
    rows: Vec<RawMessage>,
) -> Result<Vec<MessageRow>, StoreError> {
    let numbers = serde_json::to_string(&rows.iter().map(|row| row.number).collect::<Vec<_>>())
        .map_err(|_| StoreError::Invalid("message numbers"))?;
    let recipients = sqlx::query!(
        r#"SELECT m.number, w.name AS "worker?: String", r.human, r.addressed_as,
                  r.delivered_at, r.acked_at
           FROM message_recipients r
           JOIN messages m ON m.id = r.message_id
           LEFT JOIN workers w ON w.id = r.worker_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number, r.id"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut choices = choices_of(conn, &numbers).await?;
    let mut of: HashMap<i64, Vec<RecipientRow>> = HashMap::new();
    for row in recipients {
        let target = match (row.worker, row.human) {
            (Some(worker), None) => Target::Worker(
                WorkerName::parse(&worker).map_err(|_| StoreError::Corrupt("workers.name"))?,
            ),
            (None, Some(name)) => Target::Human(
                HumanName::parse(&name)
                    .map_err(|_| StoreError::Corrupt("message_recipients.human"))?,
            ),
            _ => return Err(StoreError::Corrupt("message_recipients.worker_id")),
        };
        of.entry(row.number).or_default().push(RecipientRow {
            target,
            addressed_as: row
                .addressed_as
                .parse()
                .map_err(|()| StoreError::Corrupt("message_recipients.addressed_as"))?,
            delivered_at: row
                .delivered_at
                .map(|millis| time(millis, "message_recipients.delivered_at"))
                .transpose()?,
            acked_at: row
                .acked_at
                .map(|millis| time(millis, "message_recipients.acked_at"))
                .transpose()?,
        });
    }
    rows.into_iter()
        .map(|row| {
            let recipients = of.remove(&row.number).unwrap_or_default();
            let choices = choices.remove(&row.number).unwrap_or_default();
            row.into_row(recipients, choices)
        })
        .collect()
}

/// The first message a human wrote in the thread of `message` after it: the human's answer to a question. Returns its body and its human.
pub async fn first_human_reply(
    executor: impl SqliteExecutor<'_>,
    message: MessageId,
) -> Result<Option<(String, HumanName)>, StoreError> {
    let number = message.to_db();
    let row = sqlx::query!(
        r#"SELECT later.body, later.sender_human AS "human!: String"
           FROM messages m
           JOIN messages later ON later.thread_id = m.thread_id AND later.number > m.number
           WHERE m.number = ?1 AND later.sender_kind = 'human'
           ORDER BY later.number LIMIT 1"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        let human = HumanName::parse(&row.human)
            .map_err(|_| StoreError::Corrupt("messages.sender_human"))?;
        Ok((row.body, human))
    })
    .transpose()
}

/// The choices of each message whose number is in the JSON list `numbers`, in order.
async fn choices_of(
    conn: &mut SqliteConnection,
    numbers: &str,
) -> Result<HashMap<i64, Vec<String>>, StoreError> {
    let labels = sqlx::query!(
        r#"SELECT m.number, c.label
           FROM message_choices c
           JOIN messages m ON m.id = c.message_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number, c.position"#,
        numbers
    )
    .fetch_all(conn)
    .await?;
    let mut choices: HashMap<i64, Vec<String>> = HashMap::new();
    for row in labels {
        choices.entry(row.number).or_default().push(row.label);
    }
    Ok(choices)
}

fn message_id(number: i64, column: &'static str) -> Result<MessageId, StoreError> {
    MessageId::try_from_db(number).ok_or(StoreError::Corrupt(column))
}

fn task_id(number: i64) -> Result<TaskId, StoreError> {
    TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number"))
}

fn proposal_id(number: i64) -> Result<ProposalId, StoreError> {
    ProposalId::try_from_db(number).ok_or(StoreError::Corrupt("proposals.number"))
}

fn request_id(number: i64) -> Result<RequestId, StoreError> {
    RequestId::try_from_db(number).ok_or(StoreError::Corrupt("requests.number"))
}

fn time(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}

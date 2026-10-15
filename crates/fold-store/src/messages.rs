use std::collections::HashMap;

use fold_core::{
    AddressedAs, AgentName, Directory, DirectoryAgent, HumanName, MessageId, ProposalId, Recipient,
    Sender, Tag, Target, TaskId, Timestamp, WorkerName,
};
use sqlx::SqliteConnection;

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
    pub body: String,
    pub created_at: Timestamp,
    pub recipients: Vec<RecipientRow>,
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
    let body = message.body;
    let now = message.now.as_millis();
    sqlx::query!(
        "INSERT INTO messages
             (number, thread_id, reply_to, sender_kind, sender_worker_id, sender_human,
              task_id, proposal_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4,
                 (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                  WHERE a.name = ?11 AND w.name = ?5), ?6,
                 (SELECT id FROM tasks WHERE number = ?7),
                 (SELECT id FROM proposals WHERE number = ?8), ?9, ?10)",
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
        agent
    )
    .execute(tx.conn())
    .await?;
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
        r#"SELECT m.thread_id, t.number AS "task?: i64", p.number AS "proposal?: i64"
           FROM messages m
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
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
        })
    })
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
                  m.body, m.created_at
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
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
                  m.body, m.created_at
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
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

struct RawMessage {
    number: i64,
    thread_id: i64,
    reply_to: Option<i64>,
    sender_kind: String,
    worker: Option<String>,
    sender_human: Option<String>,
    task: Option<i64>,
    proposal: Option<i64>,
    body: String,
    created_at: i64,
}

impl RawMessage {
    fn into_row(self, recipients: Vec<RecipientRow>) -> Result<MessageRow, StoreError> {
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
            body: self.body,
            created_at: time(self.created_at, "messages.created_at")?,
            recipients,
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
    .fetch_all(conn)
    .await?;
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
            row.into_row(recipients)
        })
        .collect()
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

fn time(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}

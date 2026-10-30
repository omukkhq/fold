use std::collections::{BTreeMap, HashMap};

use fold_core::reviews::Side;
use fold_core::{
    AddressedAs, AgentName, Choice, Directory, DirectoryAgent, EventActor, HumanName, MessageId,
    ProposalId, Recipient, RequestId, Sender, SetQuestion, Tag, Target, TaskId, Timestamp,
    WorkerName,
};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::agents;
use crate::columns::time;
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
    /// The sender marked the message as asking a human for an answer.
    pub asks_human: bool,
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
    /// The first time a notice about the message reached the recipient's agent.
    pub pushed_at: Option<Timestamp>,
    /// The worker fold sent the message on to because this recipient, a stopped worker, could not start to answer it.
    pub forwarded_to: Option<WorkerName>,
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
    pub choices: Vec<Choice>,
    /// The short header of a multiple-choice question.
    pub header: Option<String>,
    /// The sender marked the message as asking a human for an answer.
    pub asks_human: bool,
    /// The lines of a proposal version the message asks about.
    pub lines: Option<MessageLines>,
    /// The questions of a question set, in order; empty for any other message.
    pub questions: Vec<SetQuestion>,
    /// The answers a human's reply gives, one per question of the ask it answers, in order; empty for any other message.
    pub answers: Vec<String>,
    /// The ask whose questions `answers` answer.
    pub answers_ask: Option<MessageId>,
    /// The message stands for a worker's `AskUserQuestion` call.
    pub worker_question: bool,
}

/// A run of lines of one file of a proposal version that a message asks about, 1-based and inclusive, in the version's base tree (`Old`) or its own tree (`New`). The version belongs to the message's proposal. The excerpt is file content, which is never logged, so the `Debug` form gives its length alone.
#[derive(Clone, PartialEq, Eq)]
pub struct MessageLines {
    pub version: u32,
    pub path: String,
    pub side: Side,
    pub start_line: u32,
    pub end_line: u32,
    /// The lines as the tree holds them, joined by newlines.
    pub excerpt: String,
    /// `excerpt` holds only the first lines of the run.
    pub excerpt_cut: bool,
}

impl std::fmt::Debug for MessageLines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageLines")
            .field("version", &self.version)
            .field("path", &self.path)
            .field("side", &self.side)
            .field("start_line", &self.start_line)
            .field("end_line", &self.end_line)
            .field("excerpt_len", &self.excerpt.len())
            .field("excerpt_cut", &self.excerpt_cut)
            .finish()
    }
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
    let asks = message.asks_human;
    sqlx::query!(
        "INSERT INTO messages
             (number, thread_id, reply_to, sender_kind, sender_worker_id, sender_human,
              task_id, proposal_id, request_id, body, created_at, asks_human)
         VALUES (?1, ?2, ?3, ?4,
                 (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                  WHERE a.name = ?11 AND w.name = ?5), ?6,
                 (SELECT id FROM tasks WHERE number = ?7),
                 (SELECT id FROM proposals WHERE number = ?8),
                 (SELECT id FROM requests WHERE number = ?12), ?9, ?10, ?13)",
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
        request,
        asks
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Stores the options of a question message, in their order, and its header.
pub async fn insert_choices(
    tx: &mut WriteTx,
    message: MessageId,
    options: &[Choice],
    header: Option<&str>,
) -> Result<(), StoreError> {
    let number = message.to_db();
    for (position, option) in (1_i64..).zip(options) {
        let description = option.description.as_deref();
        sqlx::query!(
            "INSERT INTO message_choices (message_id, position, label, description)
             VALUES ((SELECT id FROM messages WHERE number = ?1), ?2, ?3, ?4)",
            number,
            position,
            option.label,
            description
        )
        .execute(tx.conn())
        .await?;
    }
    if let Some(header) = header {
        sqlx::query!(
            "INSERT INTO message_question_headers (message_id, header)
             VALUES ((SELECT id FROM messages WHERE number = ?1), ?2)",
            number,
            header
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Stores the questions of a question set message, in their order, each with its header and options.
pub async fn insert_questions(
    tx: &mut WriteTx,
    message: MessageId,
    questions: &[SetQuestion],
) -> Result<(), StoreError> {
    let number = message.to_db();
    for (position, question) in (1_i64..).zip(questions) {
        let header = question.header.as_deref();
        sqlx::query!(
            "INSERT INTO message_questions (message_id, position, text, header)
             VALUES ((SELECT id FROM messages WHERE number = ?1), ?2, ?3, ?4)",
            number,
            position,
            question.text,
            header
        )
        .execute(tx.conn())
        .await?;
        for (choice, option) in (1_i64..).zip(&question.choices) {
            let description = option.description.as_deref();
            sqlx::query!(
                "INSERT INTO message_question_choices (message_id, question, position, label, description)
                 VALUES ((SELECT id FROM messages WHERE number = ?1), ?2, ?3, ?4, ?5)",
                number,
                position,
                choice,
                option.label,
                description
            )
            .execute(tx.conn())
            .await?;
        }
    }
    Ok(())
}

/// Stores the answers that the human reply `reply` gives to the ask `ask`, one per question in order. The database refuses a reply outside the ask's thread, a reply that already answers another ask, and a second answer to a question.
pub async fn insert_answers(
    tx: &mut WriteTx,
    reply: MessageId,
    ask: MessageId,
    answers: &[String],
) -> Result<(), StoreError> {
    let reply = reply.to_db();
    let ask = ask.to_db();
    for (question, answer) in (1_i64..).zip(answers) {
        sqlx::query!(
            "INSERT INTO message_answers (reply_id, ask_id, question, answer)
             VALUES ((SELECT id FROM messages WHERE number = ?1),
                     (SELECT id FROM messages WHERE number = ?2), ?3, ?4)",
            reply,
            ask,
            question,
            answer
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Stores the lines of `proposal`'s version that `message` asks about. A version the message's proposal does not have is refused by the database.
pub async fn insert_lines(
    tx: &mut WriteTx,
    message: MessageId,
    proposal: ProposalId,
    lines: &MessageLines,
) -> Result<(), StoreError> {
    let number = message.to_db();
    let proposal = proposal.to_db();
    let version = i64::from(lines.version);
    let side = lines.side.as_str();
    let start = i64::from(lines.start_line);
    let end = i64::from(lines.end_line);
    sqlx::query!(
        "INSERT INTO message_line_refs
             (message_id, version_id, path, side, start_line, end_line, excerpt, excerpt_cut)
         VALUES ((SELECT id FROM messages WHERE number = ?1),
                 (SELECT v.id FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
                  WHERE p.number = ?2 AND v.version = ?3),
                 ?4, ?5, ?6, ?7, ?8, ?9)",
        number,
        proposal,
        version,
        lines.path,
        side,
        start,
        end,
        lines.excerpt,
        lines.excerpt_cut
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

/// The agents with their tags and `humans` from the registry, as one snapshot for `fold_core::messages::resolve`. Pass the writer's connection so the snapshot matches the rows the send inserts.
pub async fn directory(
    conn: &mut SqliteConnection,
    humans: Vec<HumanName>,
) -> Result<Directory, StoreError> {
    let rows = agents::list_agents(&mut *conn).await?;
    let mut tags_of: HashMap<i64, Vec<Tag>> = HashMap::new();
    for (agent_id, tag) in agents::all_tags(&mut *conn).await? {
        tags_of.entry(agent_id).or_default().push(tag);
    }
    Ok(Directory {
        agents: rows
            .into_iter()
            .map(|row| DirectoryAgent {
                tags: tags_of.remove(&row.id).unwrap_or_default(),
                name: row.name,
            })
            .collect(),
        humans,
    })
}

/// The messages of one thread in number order. An unknown thread has none.
pub async fn thread(
    conn: &mut SqliteConnection,
    thread: MessageId,
) -> Result<Vec<MessageRow>, StoreError> {
    let number = thread.to_db();
    let numbers = sqlx::query_scalar!(
        r#"SELECT number AS "number!: i64" FROM messages WHERE thread_id = ? ORDER BY number"#,
        number
    )
    .fetch_all(&mut *conn)
    .await?;
    by_numbers(conn, &numbers).await
}

/// The open asks: messages from a worker to a human that ask for an answer, with no later message from a human in their thread and no closure, in number order. The `open_human_questions` view holds the rule, so the board's question mark, Needs you and the Messages count agree.
pub async fn awaiting_human(conn: &mut SqliteConnection) -> Result<Vec<MessageRow>, StoreError> {
    let numbers = sqlx::query_scalar!(
        r#"SELECT number AS "number!: i64" FROM open_human_questions ORDER BY number"#
    )
    .fetch_all(&mut *conn)
    .await?;
    by_numbers(conn, &numbers).await
}

/// One message with its recipients; an unknown number has none.
pub async fn get(
    conn: &mut SqliteConnection,
    message: MessageId,
) -> Result<Option<MessageRow>, StoreError> {
    Ok(by_numbers(conn, &[message.to_db()]).await?.pop())
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
                  m.number
           FROM message_recipients r
           JOIN workers rw ON rw.id = r.worker_id
           JOIN messages m ON m.id = r.message_id
           WHERE r.worker_id IN (SELECT value FROM json_each(?)) AND r.acked_at IS NULL
           ORDER BY m.number, rw.id"#,
        ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let numbers = id_list(&rows.iter().map(|row| row.number).collect::<Vec<_>>())?;
    let messages: HashMap<i64, RawMessage> = raw_by_numbers(&mut *conn, &numbers)
        .await?
        .into_iter()
        .map(|message| (message.number, message))
        .collect();
    let choices = choices_of(conn, &numbers).await?;
    rows.into_iter()
        .map(|row| {
            let asked = choices.get(&row.number).cloned().unwrap_or_default();
            let message = messages
                .get(&row.number)
                .cloned()
                .ok_or(StoreError::Corrupt("message_recipients.message_id"))?
                .into_row(Vec::new(), asked)?;
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

/// Sets `pushed_at` on the rows of `agent`'s workers for `message`, where they have none, and returns how many rows it set. A row keeps the time of the first notice that got through.
pub async fn mark_pushed(
    tx: &mut WriteTx,
    message: MessageId,
    agent: &AgentName,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let message = message.to_db();
    let agent = agent.as_str();
    let now = now.as_millis();
    let done = sqlx::query!(
        "UPDATE message_recipients SET pushed_at = ?1
         WHERE pushed_at IS NULL
           AND message_id = (SELECT id FROM messages WHERE number = ?2)
           AND worker_id IN (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                             WHERE a.name = ?3)",
        now,
        message,
        agent
    )
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// A message to a stopped or failed worker that it has not read, that no notice reached and that fold has not settled by sending it on: the worker must start again to answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedMessage {
    /// The recipient worker's agent.
    pub agent: AgentName,
    pub message: MessageId,
    pub thread: MessageId,
    pub task: Option<TaskId>,
    pub sender: Sender,
}

/// Every [`OwedMessage`] in message number order, of `agent` alone when it is given. Only rows addressed to the agent by name count, also those to a row named `agent/name` that a project still holds, so a message to a tag or to every agent starts no stopped worker, and fold's own messages never do. An agent with a connected session, or with a session opened since its process stopped, such as one a person runs in a terminal of their own, is left out: that session takes the message, and a second process must not run beside it.
pub async fn owed_to_stopped(
    executor: impl SqliteExecutor<'_>,
    agent: Option<&AgentName>,
) -> Result<Vec<OwedMessage>, StoreError> {
    let agent = agent.map(AgentName::as_str);
    let rows = sqlx::query!(
        r#"SELECT DISTINCT g.name AS "agent!: String", m.number AS "number!: i64",
                  m.thread_id AS "thread!: i64", t.number AS "task?: i64", m.sender_kind,
                  sw.name AS "worker?: String", m.sender_human
           FROM message_recipients r
           JOIN messages m ON m.id = r.message_id
           JOIN workers k ON k.id = r.worker_id
           JOIN agents g ON g.id = k.agent_id
           JOIN worker_processes p ON p.agent_id = k.agent_id
           LEFT JOIN workers sw ON sw.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           WHERE p.state IN ('stopped', 'failed') AND (?1 IS NULL OR g.name = ?1)
             AND r.addressed_as IN ('agent', 'worker') AND m.sender_kind <> 'fold'
             AND r.delivered_at IS NULL AND r.acked_at IS NULL AND r.pushed_at IS NULL
             AND r.forwarded_at IS NULL
             AND NOT EXISTS (
                 SELECT 1 FROM sessions s
                 WHERE s.agent_id = g.id AND (s.ended_at IS NULL OR s.started_at > p.stopped_at)
             )
           ORDER BY m.number, g.name"#,
        agent
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let sender = match (row.sender_kind.as_str(), row.worker, row.sender_human) {
                ("worker", Some(worker), None) => Sender::Worker(
                    WorkerName::parse(&worker).map_err(|_| StoreError::Corrupt("workers.name"))?,
                ),
                ("human", None, Some(name)) => Sender::Human(
                    HumanName::parse(&name)
                        .map_err(|_| StoreError::Corrupt("messages.sender_human"))?,
                ),
                _ => return Err(StoreError::Corrupt("messages.sender_kind")),
            };
            Ok(OwedMessage {
                agent: AgentName::parse(&row.agent)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                message: message_id(row.number, "messages.number")?,
                thread: message_id(row.thread, "messages.thread_id")?,
                task: row.task.map(task_id).transpose()?,
                sender,
            })
        })
        .collect()
}

/// Records on `agent`'s rows of `message` that fold settled them at `now`, because the stopped worker could not start to answer it: sent on to `to`, or held with nobody to take it when `to` is `None`. Rows settled before keep their record. Returns how many rows it set.
pub async fn mark_forwarded(
    tx: &mut WriteTx,
    message: MessageId,
    agent: &AgentName,
    to: Option<&WorkerName>,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let message = message.to_db();
    let agent = agent.as_str();
    let to_agent = to.map(WorkerName::agent);
    let to_agent = to_agent.as_ref().map(AgentName::as_str);
    let to = to.map(WorkerName::as_str);
    let now = now.as_millis();
    let done = sqlx::query!(
        "UPDATE message_recipients
         SET forwarded_to = (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                             WHERE a.name = ?4 AND w.name = ?3),
             forwarded_at = ?1
         WHERE forwarded_at IS NULL
           AND message_id = (SELECT id FROM messages WHERE number = ?2)
           AND worker_id IN (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                             WHERE a.name = ?5)",
        now,
        message,
        to,
        to_agent,
        agent
    )
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// Whether `worker` has a row of `message`.
pub async fn is_recipient(
    conn: &mut SqliteConnection,
    message: MessageId,
    worker: &WorkerName,
) -> Result<bool, StoreError> {
    let message = message.to_db();
    let agent = worker.agent();
    let agent = agent.as_str();
    let worker = worker.as_str();
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM message_recipients r
               JOIN workers w ON w.id = r.worker_id
               JOIN agents a ON a.id = w.agent_id
               WHERE r.message_id = (SELECT id FROM messages WHERE number = ?1)
                 AND a.name = ?2 AND w.name = ?3
           ) AS "found!: bool""#,
        message,
        agent,
        worker
    )
    .fetch_one(conn)
    .await?)
}

/// How many rows of `agent`'s worker no inbox has returned yet.
pub async fn undelivered_count(
    executor: impl SqliteExecutor<'_>,
    agent: &AgentName,
) -> Result<u64, StoreError> {
    let agent = agent.as_str();
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM message_recipients r
           JOIN workers w ON w.id = r.worker_id
           JOIN agents a ON a.id = w.agent_id
           WHERE a.name = ? AND w.parent_worker_id IS NULL
             AND r.delivered_at IS NULL AND r.acked_at IS NULL"#,
        agent
    )
    .fetch_one(executor)
    .await?;
    u64::try_from(count).map_err(|_| StoreError::Corrupt("message_recipients"))
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

/// Acknowledges `human`'s unacknowledged rows of the messages of `thread`, only those numbered below `before` when it is given, and returns the numbers it changed in order. A row acknowledged before keeps its time.
pub async fn acknowledge_human(
    tx: &mut WriteTx,
    human: &HumanName,
    thread: MessageId,
    before: Option<MessageId>,
    now: Timestamp,
) -> Result<Vec<MessageId>, StoreError> {
    let name = human.as_str();
    let thread = thread.to_db();
    let before = before.map(MessageId::to_db);
    let rows = sqlx::query!(
        r#"SELECT r.id AS "row!: i64", m.number
           FROM message_recipients r JOIN messages m ON m.id = r.message_id
           WHERE r.human = ?1 AND r.acked_at IS NULL AND m.thread_id = ?2
             AND (?3 IS NULL OR m.number < ?3)
           ORDER BY m.number"#,
        name,
        thread,
        before
    )
    .fetch_all(tx.conn())
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
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
        .collect()
}

/// The unacknowledged rows of the agent's worker.
pub async fn unread_count(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<u64, StoreError> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM workers w
           JOIN message_recipients r ON r.worker_id = w.id
           WHERE w.agent_id = ? AND w.parent_worker_id IS NULL AND r.acked_at IS NULL"#,
        agent_id
    )
    .fetch_one(executor)
    .await?;
    u64::try_from(count).map_err(|_| StoreError::Corrupt("message_recipients"))
}

/// Which threads a listing returns. Every set field must match some message of the thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFilter {
    /// The agent sent or received a message of the thread.
    pub agent: Option<AgentName>,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    /// Only threads that concern this human: a message of the thread is from them, addressed to them by name or addressed to `humans`.
    pub human: Option<HumanName>,
    /// Only threads that hold an open ask; with `human`, an open ask that waits for that human.
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
    let task = filter.task.map(TaskId::to_db);
    let proposal = filter.proposal.map(ProposalId::to_db);
    let awaiting = filter.awaiting_human;
    let before = filter.before.map(MessageId::to_db);
    let limit = i64::from(limit);
    let human = filter.human.as_ref().map(HumanName::as_str);
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
             AND (NOT ?4 OR EXISTS (
                     SELECT 1 FROM open_human_questions q
                     JOIN message_recipients r ON r.message_id = q.id
                     WHERE q.thread_id = m.thread_id
                       AND (?7 IS NULL OR r.human = ?7 OR r.addressed_as = 'humans')))
             AND (?7 IS NULL OR EXISTS (
                     SELECT 1 FROM messages x
                     WHERE x.thread_id = m.thread_id AND x.sender_human = ?7
                     UNION ALL
                     SELECT 1 FROM messages x
                     JOIN message_recipients r ON r.message_id = x.id
                     WHERE x.thread_id = m.thread_id
                       AND (r.human = ?7 OR r.addressed_as = 'humans')))
           GROUP BY m.thread_id
           HAVING ?5 IS NULL OR MAX(m.number) < ?5
           ORDER BY MAX(m.number) DESC
           LIMIT ?6"#,
        task,
        proposal,
        agent,
        awaiting,
        before,
        limit,
        human
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
    let threads = id_list(&threads.iter().map(|id| id.to_db()).collect::<Vec<_>>())?;
    let numbers = sqlx::query_scalar!(
        r#"SELECT number AS "number!: i64" FROM messages
           WHERE thread_id IN (SELECT value FROM json_each(?))"#,
        threads
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut rows = by_numbers(conn, &numbers).await?;
    rows.sort_by_key(|row| (row.thread, row.id));
    Ok(rows)
}

/// The threads that hold an open ask, as [`awaiting_human`] decides; with `human`, an open ask addressed to that human by name or to `humans`. Each thread maps to whether one of those asks is closable: a human can mark it answered, because it is no worker's `AskUserQuestion`, which only a reply ends.
pub async fn threads_awaiting_human(
    executor: impl SqliteExecutor<'_>,
    human: Option<&HumanName>,
) -> Result<BTreeMap<MessageId, bool>, StoreError> {
    let human = human.map(HumanName::as_str);
    let rows = sqlx::query!(
        r#"SELECT q.thread_id AS "thread!: i64",
                  MAX(NOT EXISTS (SELECT 1 FROM worker_question_messages w WHERE w.message_id = q.id))
                      AS "closable!: bool"
           FROM open_human_questions q
           WHERE ?1 IS NULL OR EXISTS (
                     SELECT 1 FROM message_recipients r
                     WHERE r.message_id = q.id AND (r.human = ?1 OR r.addressed_as = 'humans'))
           GROUP BY q.thread_id"#,
        human
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| Ok((message_id(row.thread, "messages.thread_id")?, row.closable)))
        .collect()
}

/// Whether thread `thread` holds an open ask, as [`threads_awaiting_human`] decides for every thread: `None` when none waits, else whether one of them is closable.
pub async fn thread_awaits_human(
    executor: impl SqliteExecutor<'_>,
    thread: MessageId,
) -> Result<Option<bool>, StoreError> {
    let thread = thread.to_db();
    Ok(sqlx::query_scalar!(
        r#"SELECT MAX(NOT EXISTS (SELECT 1 FROM worker_question_messages w WHERE w.message_id = q.id))
                      AS "closable: bool"
           FROM open_human_questions q WHERE q.thread_id = ?"#,
        thread
    )
    .fetch_one(executor)
    .await?)
}

/// The tasks linked to an open ask, the source of the `question` attention kind, each with whether one of its asks is closable, as [`threads_awaiting_human`] decides it.
pub async fn tasks_awaiting_human(
    executor: impl SqliteExecutor<'_>,
) -> Result<BTreeMap<TaskId, bool>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64",
                  MAX(NOT EXISTS (SELECT 1 FROM worker_question_messages w WHERE w.message_id = q.id))
                      AS "closable!: bool"
           FROM open_human_questions q
           JOIN tasks t ON t.id = q.task_id
           GROUP BY t.number"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| Ok((task_id(row.task)?, row.closable)))
        .collect()
}

/// Why an ask stopped waiting without a human's reply in its thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskClosure {
    /// The asking agent or the main agent recorded that the user answered elsewhere.
    AnsweredElsewhere,
    /// A human marked the ask answered.
    Dismissed,
}

impl AskClosure {
    pub fn as_str(self) -> &'static str {
        match self {
            AskClosure::AnsweredElsewhere => "answered_elsewhere",
            AskClosure::Dismissed => "dismissed",
        }
    }
}

/// An open ask as a closing command reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAsk {
    pub id: MessageId,
    pub thread: MessageId,
    pub task: Option<TaskId>,
    pub sender: WorkerName,
    /// The message stands for a worker's `AskUserQuestion` call, whose worker waits for the human's reply, so no closure ends it.
    pub worker_question: bool,
}

/// Which open asks a read returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskScope {
    Message(MessageId),
    Thread(MessageId),
    Task(TaskId),
}

/// The open asks of `scope`, in number order.
pub async fn open_asks(
    conn: &mut SqliteConnection,
    scope: AskScope,
) -> Result<Vec<OpenAsk>, StoreError> {
    let (message, thread, task) = match scope {
        AskScope::Message(id) => (Some(id.to_db()), None, None),
        AskScope::Thread(id) => (None, Some(id.to_db()), None),
        AskScope::Task(id) => (None, None, Some(id.to_db())),
    };
    let rows = sqlx::query!(
        r#"SELECT q.number AS "number!: i64", q.thread_id AS "thread!: i64",
                  t.number AS "task?: i64", w.name AS "worker!: String",
                  EXISTS (SELECT 1 FROM worker_question_messages x WHERE x.message_id = q.id)
                      AS "worker_question!: bool"
           FROM open_human_questions q
           JOIN messages m ON m.id = q.id
           JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = q.task_id
           WHERE (?1 IS NULL OR q.number = ?1)
             AND (?2 IS NULL OR q.thread_id = ?2)
             AND (?3 IS NULL OR t.number = ?3)
           ORDER BY q.number"#,
        message,
        thread,
        task
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OpenAsk {
                id: message_id(row.number, "messages.number")?,
                thread: message_id(row.thread, "messages.thread_id")?,
                task: row.task.map(task_id).transpose()?,
                sender: WorkerName::parse(&row.worker)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                worker_question: row.worker_question,
            })
        })
        .collect()
}

/// Records that the open ask `message` stopped waiting. The database refuses a message that is no open ask, and an `AskUserQuestion` message.
pub async fn close_ask(
    tx: &mut WriteTx,
    message: MessageId,
    reason: AskClosure,
    closed_by: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = message.to_db();
    let reason = reason.as_str();
    let now = now.as_millis();
    sqlx::query!(
        "INSERT INTO message_ask_closures (message_id, reason, closed_by, created_at)
         VALUES ((SELECT id FROM messages WHERE number = ?1), ?2, ?3, ?4)",
        number,
        reason,
        closed_by,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

fn id_list(ids: &[i64]) -> Result<String, StoreError> {
    serde_json::to_string(ids).map_err(|_| StoreError::Invalid("row ids"))
}

#[derive(Clone)]
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
    asks_human: bool,
    worker_question: bool,
}

impl RawMessage {
    fn into_row(
        self,
        recipients: Vec<RecipientRow>,
        asked: Asked,
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
            choices: asked.choices,
            header: asked.header,
            asks_human: self.asks_human,
            lines: asked.lines,
            questions: asked.questions,
            answers: asked.answers,
            answers_ask: asked.answers_ask,
            worker_question: self.worker_question,
        })
    }
}

/// The messages whose numbers are in `numbers`, with their recipients, in number order: the one read that builds a `MessageRow`.
async fn by_numbers(
    conn: &mut SqliteConnection,
    numbers: &[i64],
) -> Result<Vec<MessageRow>, StoreError> {
    let rows = raw_by_numbers(&mut *conn, &id_list(numbers)?).await?;
    with_recipients(conn, rows).await
}

/// The messages whose numbers are in the JSON list `numbers`, in number order.
async fn raw_by_numbers(
    conn: &mut SqliteConnection,
    numbers: &str,
) -> Result<Vec<RawMessage>, StoreError> {
    Ok(sqlx::query_as!(
        RawMessage,
        r#"SELECT m.number, m.thread_id, m.reply_to, m.sender_kind, w.name AS "worker?: String",
                  m.sender_human, t.number AS "task?: i64", p.number AS "proposal?: i64",
                  rq.number AS "request?: i64", m.body, m.created_at,
                  m.asks_human AS "asks_human!: bool",
                  EXISTS (SELECT 1 FROM worker_question_messages q WHERE q.message_id = m.id)
                      AS "worker_question!: bool"
           FROM messages m
           LEFT JOIN workers w ON w.id = m.sender_worker_id
           LEFT JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN requests rq ON rq.id = m.request_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number"#,
        numbers
    )
    .fetch_all(conn)
    .await?)
}

async fn with_recipients(
    conn: &mut SqliteConnection,
    rows: Vec<RawMessage>,
) -> Result<Vec<MessageRow>, StoreError> {
    let numbers = serde_json::to_string(&rows.iter().map(|row| row.number).collect::<Vec<_>>())
        .map_err(|_| StoreError::Invalid("message numbers"))?;
    let recipients = sqlx::query!(
        r#"SELECT m.number, w.name AS "worker?: String", r.human, r.addressed_as,
                  r.delivered_at, r.acked_at, r.pushed_at, fw.name AS "forwarded_to?: String"
           FROM message_recipients r
           JOIN messages m ON m.id = r.message_id
           LEFT JOIN workers w ON w.id = r.worker_id
           LEFT JOIN workers fw ON fw.id = r.forwarded_to
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
            pushed_at: row
                .pushed_at
                .map(|millis| time(millis, "message_recipients.pushed_at"))
                .transpose()?,
            forwarded_to: row
                .forwarded_to
                .map(|name| {
                    WorkerName::parse(&name).map_err(|_| StoreError::Corrupt("workers.name"))
                })
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

/// The first message a human wrote in the thread of `message` after it that answers no other ask of the thread: the human's answer to a question that is no set. Returns its body and its human.
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
             AND NOT EXISTS (
                 SELECT 1 FROM message_answers o WHERE o.reply_id = later.id AND o.ask_id <> m.id
             )
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

/// The answers that a reply gave to the ask `ask`, one per question in order, and who gave them: the human, or the main agent's worker that answered for the user; `None` while no reply carries answers to it.
pub async fn answers_of(
    executor: impl SqliteExecutor<'_>,
    ask: MessageId,
) -> Result<Option<(Vec<String>, EventActor)>, StoreError> {
    let number = ask.to_db();
    let rows = sqlx::query!(
        r#"SELECT a.answer, r.sender_human AS "human?: String", w.name AS "worker?: String"
           FROM message_answers a
           JOIN messages k ON k.id = a.ask_id
           JOIN messages r ON r.id = a.reply_id
           LEFT JOIN workers w ON w.id = r.sender_worker_id
           WHERE k.number = ?1
           ORDER BY a.question"#,
        number
    )
    .fetch_all(executor)
    .await?;
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let by = match (&first.human, &first.worker) {
        (Some(human), None) => HumanName::parse(human)
            .map(|human| EventActor::human(&human))
            .map_err(|_| StoreError::Corrupt("messages.sender_human"))?,
        (None, Some(worker)) => WorkerName::parse(worker)
            .map(|worker| EventActor::worker(&worker))
            .map_err(|_| StoreError::Corrupt("workers.name"))?,
        _ => return Err(StoreError::Corrupt("message_answers.reply_id")),
    };
    Ok(Some((rows.into_iter().map(|row| row.answer).collect(), by)))
}

/// What a message asks beyond its body: the options and the header of a question, and the lines of a proposal version it asks about.
#[derive(Debug, Clone, Default)]
struct Asked {
    choices: Vec<Choice>,
    header: Option<String>,
    lines: Option<MessageLines>,
    questions: Vec<SetQuestion>,
    answers: Vec<String>,
    answers_ask: Option<MessageId>,
}

/// The choices, the header, the lines, the set questions and the answers of each message whose number is in the JSON list `numbers`, each list in order.
async fn choices_of(
    conn: &mut SqliteConnection,
    numbers: &str,
) -> Result<HashMap<i64, Asked>, StoreError> {
    let options = sqlx::query!(
        r#"SELECT m.number, c.label, c.description
           FROM message_choices c
           JOIN messages m ON m.id = c.message_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number, c.position"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    let headers = sqlx::query!(
        r#"SELECT m.number, h.header
           FROM message_question_headers h
           JOIN messages m ON m.id = h.message_id
           WHERE m.number IN (SELECT value FROM json_each(?))"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    let lines = sqlx::query!(
        r#"SELECT m.number, v.version, l.path, l.side, l.start_line, l.end_line, l.excerpt,
                  l.excerpt_cut AS "excerpt_cut!: bool"
           FROM message_line_refs l
           JOIN messages m ON m.id = l.message_id
           JOIN proposal_versions v ON v.id = l.version_id
           WHERE m.number IN (SELECT value FROM json_each(?))"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    let questions = sqlx::query!(
        r#"SELECT m.number, q.position, q.text, q.header
           FROM message_questions q
           JOIN messages m ON m.id = q.message_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number, q.position"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    let set_choices = sqlx::query!(
        r#"SELECT m.number, c.question, c.label, c.description
           FROM message_question_choices c
           JOIN messages m ON m.id = c.message_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number, c.question, c.position"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    let answers = sqlx::query!(
        r#"SELECT m.number, k.number AS "ask!: i64", a.answer
           FROM message_answers a
           JOIN messages m ON m.id = a.reply_id
           JOIN messages k ON k.id = a.ask_id
           WHERE m.number IN (SELECT value FROM json_each(?))
           ORDER BY m.number, a.question"#,
        numbers
    )
    .fetch_all(conn)
    .await?;
    let mut asked: HashMap<i64, Asked> = HashMap::new();
    for row in options {
        asked.entry(row.number).or_default().choices.push(Choice {
            label: row.label,
            description: row.description,
        });
    }
    for row in headers {
        asked.entry(row.number).or_default().header = Some(row.header);
    }
    for row in lines {
        let line =
            |value: i64, column| u32::try_from(value).map_err(|_| StoreError::Corrupt(column));
        asked.entry(row.number).or_default().lines = Some(MessageLines {
            version: line(row.version, "proposal_versions.version")?,
            path: row.path,
            side: Side::parse(&row.side).ok_or(StoreError::Corrupt("message_line_refs.side"))?,
            start_line: line(row.start_line, "message_line_refs.start_line")?,
            end_line: line(row.end_line, "message_line_refs.end_line")?,
            excerpt: row.excerpt,
            excerpt_cut: row.excerpt_cut,
        });
    }
    let mut positions: HashMap<(i64, i64), usize> = HashMap::new();
    for row in questions {
        let entry = asked.entry(row.number).or_default();
        positions.insert((row.number, row.position), entry.questions.len());
        entry.questions.push(SetQuestion {
            text: row.text,
            header: row.header,
            choices: Vec::new(),
        });
    }
    for row in set_choices {
        let at = positions
            .get(&(row.number, row.question))
            .copied()
            .ok_or(StoreError::Corrupt("message_question_choices.question"))?;
        if let Some(question) = asked
            .get_mut(&row.number)
            .and_then(|entry| entry.questions.get_mut(at))
        {
            question.choices.push(Choice {
                label: row.label,
                description: row.description,
            });
        }
    }
    for row in answers {
        let entry = asked.entry(row.number).or_default();
        entry.answers_ask = Some(message_id(row.ask, "message_answers.ask_id")?);
        entry.answers.push(row.answer);
    }
    Ok(asked)
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

use fold_core::roles::PermissionBehavior;
use fold_core::{AgentName, EventActor, HumanName, MessageId, PermissionQuestionId, Timestamp};
use serde_json::Value;
use sqlx::{SqliteConnection, SqliteExecutor};

use super::optional_time;
use crate::columns::time;
use crate::error::StoreError;
use crate::store::WriteTx;

/// What a worker's `can_use_tool` request asked: a call that a permission rule marks `ask`, or an `AskUserQuestion` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionKind {
    Tool,
    Ask,
}

impl QuestionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionKind::Tool => "tool",
            QuestionKind::Ask => "ask",
        }
    }

    fn parse(text: &str) -> Option<QuestionKind> {
        match text {
            "tool" => Some(QuestionKind::Tool),
            "ask" => Some(QuestionKind::Ask),
            _ => None,
        }
    }
}

pub struct NewQuestion<'a> {
    pub worker_process_id: i64,
    pub kind: QuestionKind,
    /// The public id of a `tool` question; `None` for an `ask` question.
    pub number: Option<PermissionQuestionId>,
    pub session_id: &'a str,
    pub request_id: &'a str,
    pub tool_use_id: Option<&'a str>,
    pub tool: &'a str,
    pub input: &'a Value,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerRow {
    pub behavior: PermissionBehavior,
    /// `human:<name>`.
    pub answered_by: String,
    pub message: Option<String>,
    pub created_at: Timestamp,
}

/// A stored question with its answer or its closure. It is open while it has neither.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionRow {
    pub id: i64,
    pub number: Option<PermissionQuestionId>,
    pub kind: QuestionKind,
    pub worker_process_id: i64,
    pub agent: AgentName,
    pub session_id: String,
    pub request_id: String,
    pub tool_use_id: Option<String>,
    pub tool: String,
    pub input: Value,
    pub created_at: Timestamp,
    pub answer: Option<AnswerRow>,
    pub closed_at: Option<Timestamp>,
}

impl QuestionRow {
    pub fn is_open(&self) -> bool {
        self.answer.is_none() && self.closed_at.is_none()
    }
}

struct RawQuestion {
    id: i64,
    number: Option<i64>,
    kind: String,
    worker_process_id: i64,
    agent: String,
    session_id: String,
    request_id: String,
    tool_use_id: Option<String>,
    tool: String,
    input: String,
    created_at: i64,
    behavior: Option<String>,
    answered_by: Option<String>,
    message: Option<String>,
    answered_at: Option<i64>,
    closed_at: Option<i64>,
}

impl RawQuestion {
    fn into_row(self) -> Result<QuestionRow, StoreError> {
        let answer = match (self.behavior, self.answered_by, self.answered_at) {
            (Some(behavior), Some(answered_by), Some(at)) => Some(AnswerRow {
                behavior: PermissionBehavior::parse(&behavior)
                    .ok_or(StoreError::Corrupt("worker_permission_answers.behavior"))?,
                answered_by,
                message: self.message,
                created_at: time(at, "worker_permission_answers.created_at")?,
            }),
            (None, None, None) => None,
            _ => return Err(StoreError::Corrupt("worker_permission_answers")),
        };
        Ok(QuestionRow {
            id: self.id,
            number: self
                .number
                .map(|number| {
                    PermissionQuestionId::try_from_db(number)
                        .ok_or(StoreError::Corrupt("worker_permission_questions.number"))
                })
                .transpose()?,
            kind: QuestionKind::parse(&self.kind)
                .ok_or(StoreError::Corrupt("worker_permission_questions.kind"))?,
            worker_process_id: self.worker_process_id,
            agent: AgentName::parse(&self.agent).map_err(|_| StoreError::Corrupt("agents.name"))?,
            session_id: self.session_id,
            request_id: self.request_id,
            tool_use_id: self.tool_use_id,
            tool: self.tool,
            input: serde_json::from_str(&self.input)
                .map_err(|_| StoreError::Corrupt("worker_permission_questions.input"))?,
            created_at: time(self.created_at, "worker_permission_questions.created_at")?,
            answer,
            closed_at: optional_time(self.closed_at, "worker_permission_closures.created_at")?,
        })
    }
}

/// Stores a question and returns its row id.
pub async fn insert_question(
    tx: &mut WriteTx,
    question: &NewQuestion<'_>,
) -> Result<i64, StoreError> {
    let number = question.number.map(PermissionQuestionId::to_db);
    let kind = question.kind.as_str();
    let input = question.input.to_string();
    let now = question.now.as_millis();
    let id = sqlx::query_scalar!(
        r#"INSERT INTO worker_permission_questions
               (number, kind, worker_process_id, session_id, request_id, tool_use_id, tool, input, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
           RETURNING id AS "id!: i64""#,
        number,
        kind,
        question.worker_process_id,
        question.session_id,
        question.request_id,
        question.tool_use_id,
        question.tool,
        input,
        now
    )
    .fetch_one(tx.conn())
    .await?;
    Ok(id)
}

/// Stores the answer to an open question. A second answer, or an answer to a closed question, fails on the table's constraints.
pub async fn insert_answer(
    tx: &mut WriteTx,
    question_id: i64,
    behavior: PermissionBehavior,
    by: &HumanName,
    message: Option<&str>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let behavior = behavior.as_str();
    let by = EventActor::human(by);
    let by = by.as_str();
    let now = now.as_millis();
    sqlx::query!(
        "INSERT INTO worker_permission_answers (question_id, behavior, answered_by, message, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        question_id,
        behavior,
        by,
        message,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Closes every open question of the worker, when fold stops it or its process ends. Returns how many it closed.
pub async fn close_open_questions(
    tx: &mut WriteTx,
    worker_process_id: i64,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let now = now.as_millis();
    let done = sqlx::query!(
        "INSERT INTO worker_permission_closures (question_id, created_at)
         SELECT q.id, ?2 FROM worker_permission_questions q
         WHERE q.worker_process_id = ?1
           AND NOT EXISTS (SELECT 1 FROM worker_permission_answers a WHERE a.question_id = q.id)
           AND NOT EXISTS (SELECT 1 FROM worker_permission_closures c WHERE c.question_id = q.id)",
        worker_process_id,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// Every question of the worker, oldest first.
pub async fn questions_of_worker(
    executor: impl SqliteExecutor<'_>,
    worker_process_id: i64,
) -> Result<Vec<QuestionRow>, StoreError> {
    let rows = sqlx::query_as!(
        RawQuestion,
        r#"SELECT q.id AS "id!: i64", q.number, q.kind, q.worker_process_id, g.name AS agent,
                  q.session_id, q.request_id, q.tool_use_id, q.tool, q.input, q.created_at,
                  a.behavior AS "behavior?: String", a.answered_by AS "answered_by?: String",
                  a.message AS "message?: String", a.created_at AS "answered_at?: i64",
                  c.created_at AS "closed_at?: i64"
           FROM worker_permission_questions q
           JOIN worker_processes w ON w.id = q.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           LEFT JOIN worker_permission_answers a ON a.question_id = q.id
           LEFT JOIN worker_permission_closures c ON c.question_id = q.id
           WHERE q.worker_process_id = ?
           ORDER BY q.id"#,
        worker_process_id
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(RawQuestion::into_row).collect()
}

/// The open questions of every worker, oldest first.
pub async fn open_questions(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<QuestionRow>, StoreError> {
    let rows = sqlx::query_as!(
        RawQuestion,
        r#"SELECT q.id AS "id!: i64", q.number, q.kind, q.worker_process_id, g.name AS agent,
                  q.session_id, q.request_id, q.tool_use_id, q.tool, q.input, q.created_at,
                  NULL AS "behavior?: String", NULL AS "answered_by?: String",
                  NULL AS "message?: String", NULL AS "answered_at?: i64",
                  NULL AS "closed_at?: i64"
           FROM worker_permission_questions q
           JOIN worker_processes w ON w.id = q.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           WHERE NOT EXISTS (SELECT 1 FROM worker_permission_answers a WHERE a.question_id = q.id)
             AND NOT EXISTS (SELECT 1 FROM worker_permission_closures c WHERE c.question_id = q.id)
           ORDER BY q.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(RawQuestion::into_row).collect()
}

/// The question with public id `number`.
pub async fn find_question(
    executor: impl SqliteExecutor<'_>,
    number: PermissionQuestionId,
) -> Result<Option<QuestionRow>, StoreError> {
    let number = number.to_db();
    let row = sqlx::query_as!(
        RawQuestion,
        r#"SELECT q.id AS "id!: i64", q.number, q.kind, q.worker_process_id, g.name AS agent,
                  q.session_id, q.request_id, q.tool_use_id, q.tool, q.input, q.created_at,
                  a.behavior AS "behavior?: String", a.answered_by AS "answered_by?: String",
                  a.message AS "message?: String", a.created_at AS "answered_at?: i64",
                  c.created_at AS "closed_at?: i64"
           FROM worker_permission_questions q
           JOIN worker_processes w ON w.id = q.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           LEFT JOIN worker_permission_answers a ON a.question_id = q.id
           LEFT JOIN worker_permission_closures c ON c.question_id = q.id
           WHERE q.number = ?"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    row.map(RawQuestion::into_row).transpose()
}

/// The question with row id `id`.
pub async fn question_by_id(
    executor: impl SqliteExecutor<'_>,
    id: i64,
) -> Result<Option<QuestionRow>, StoreError> {
    let row = sqlx::query_as!(
        RawQuestion,
        r#"SELECT q.id AS "id!: i64", q.number, q.kind, q.worker_process_id, g.name AS agent,
                  q.session_id, q.request_id, q.tool_use_id, q.tool, q.input, q.created_at,
                  a.behavior AS "behavior?: String", a.answered_by AS "answered_by?: String",
                  a.message AS "message?: String", a.created_at AS "answered_at?: i64",
                  c.created_at AS "closed_at?: i64"
           FROM worker_permission_questions q
           JOIN worker_processes w ON w.id = q.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           LEFT JOIN worker_permission_answers a ON a.question_id = q.id
           LEFT JOIN worker_permission_closures c ON c.question_id = q.id
           WHERE q.id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    row.map(RawQuestion::into_row).transpose()
}

/// Links the question message for the `position`-th question of an `ask` question.
pub async fn insert_question_message(
    tx: &mut WriteTx,
    question_id: i64,
    position: i64,
    message: MessageId,
) -> Result<(), StoreError> {
    let number = message.to_db();
    sqlx::query!(
        "INSERT INTO worker_question_messages (question_id, position, message_id)
         VALUES (?1, ?2, (SELECT id FROM messages WHERE number = ?3))",
        question_id,
        position,
        number
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The question messages of an `ask` question, in the call's order.
pub async fn question_messages(
    executor: impl SqliteExecutor<'_>,
    question_id: i64,
) -> Result<Vec<MessageId>, StoreError> {
    let numbers = sqlx::query_scalar!(
        r#"SELECT m.number AS "number!: i64" FROM worker_question_messages q
           JOIN messages m ON m.id = q.message_id
           WHERE q.question_id = ? ORDER BY q.position"#,
        question_id
    )
    .fetch_all(executor)
    .await?;
    numbers
        .into_iter()
        .map(|number| MessageId::try_from_db(number).ok_or(StoreError::Corrupt("messages.number")))
        .collect()
}

/// The open `ask` question that has a question message in thread `thread`, as a row id.
pub async fn open_ask_of_thread(
    conn: &mut SqliteConnection,
    thread: MessageId,
) -> Result<Option<i64>, StoreError> {
    let thread = thread.to_db();
    let id = sqlx::query_scalar!(
        r#"SELECT q.question_id AS "id!: i64" FROM worker_question_messages q
           JOIN messages m ON m.id = q.message_id
           WHERE m.thread_id = ?1
             AND NOT EXISTS (SELECT 1 FROM worker_permission_answers a WHERE a.question_id = q.question_id)
             AND NOT EXISTS (SELECT 1 FROM worker_permission_closures c WHERE c.question_id = q.question_id)
           LIMIT 1"#,
        thread
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(id)
}

/// The agent whose `ask` question takes human message `message` as its answer: `message` is the first human message in the thread of one of the question's messages, and fold has not closed the question. The reply reaches the worker as the tool's answer, never as one more message turn.
pub async fn asker_answered_by(
    executor: impl SqliteExecutor<'_>,
    message: MessageId,
) -> Result<Option<AgentName>, StoreError> {
    let number = message.to_db();
    let name = sqlx::query_scalar!(
        r#"SELECT g.name AS "name!: String" FROM messages r
           JOIN messages m ON m.thread_id = r.thread_id AND m.number < r.number
           JOIN worker_question_messages wq ON wq.message_id = m.id
           JOIN worker_permission_questions q ON q.id = wq.question_id
           JOIN worker_processes w ON w.id = q.worker_process_id
           JOIN agents g ON g.id = w.agent_id
           WHERE r.number = ?1 AND r.sender_kind = 'human'
             AND NOT EXISTS (SELECT 1 FROM worker_permission_closures c WHERE c.question_id = q.id)
             AND NOT EXISTS (
                 SELECT 1 FROM messages e
                 WHERE e.thread_id = m.thread_id AND e.number > m.number AND e.number < r.number
                   AND e.sender_kind = 'human'
             )
           LIMIT 1"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    name.map(|name| AgentName::parse(&name).map_err(|_| StoreError::Corrupt("agents.name")))
        .transpose()
}

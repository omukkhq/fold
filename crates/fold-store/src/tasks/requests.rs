use std::collections::HashMap;

use fold_core::object::ObjectHash;
use fold_core::requests::{Draft, DraftCard, DraftEdge, RequestKind, RequestState, Requester};
use fold_core::{LabelName, RequestId, TaskId, Timestamp, WorkerName};
use sqlx::SqliteConnection;

use crate::columns::{count, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// The worker drafting a request. The row ids stay inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drafter {
    pub worker_id: i64,
    pub agent_id: i64,
    pub worker: WorkerName,
    pub since: Timestamp,
}

/// A request as stored, with the number of its newest draft version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRow {
    pub id: RequestId,
    /// `None` until an orchestrator claims and sorts the request.
    pub kind: Option<RequestKind>,
    /// `None` until an orchestrator claims and names the request.
    pub title: Option<String>,
    pub body: String,
    pub state: RequestState,
    pub requester: Requester,
    pub drafter: Option<Drafter>,
    pub close_reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub latest_version: Option<u32>,
}

/// The values of a new request, which starts `open` without a kind, a title or a drafter.
#[derive(Debug, Clone)]
pub struct NewRequest<'a> {
    pub number: RequestId,
    pub body: &'a str,
    pub requester: &'a Requester,
    pub now: Timestamp,
}

/// The metadata of a stored attachment; its bytes are the blob `object`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRow {
    pub position: u32,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub object: ObjectHash,
    pub created_at: Timestamp,
}

/// The values of one attachment row. The blob must already be in the object store.
#[derive(Debug, Clone)]
pub struct NewAttachment<'a> {
    pub position: u32,
    pub name: &'a str,
    pub media_type: &'a str,
    pub size: u64,
    pub object: &'a ObjectHash,
}

/// Selects requests; a `None` field matches every request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestFilter {
    pub state: Option<RequestState>,
    pub kind: Option<RequestKind>,
    pub requester: Option<Requester>,
    /// At most this many of the newest; every match when `None`.
    pub limit: Option<u32>,
}

/// One stored draft version with its cards in position order and its edges in insertion order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    pub version: u32,
    pub author: String,
    pub note: Option<String>,
    pub created_at: Timestamp,
    pub cards: Vec<DraftCard>,
    pub edges: Vec<DraftEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRow {
    pub version: u32,
    pub author: String,
    pub body: String,
    pub created_at: Timestamp,
}

/// The columns of [`RawRequest`] and the join that gives them, followed by the tail of each read: every request row maps the same way.
macro_rules! select_requests {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            RawRequest,
            r#"SELECT r.number, r.kind, r.title, r.body, r.state, r.requester,
                      r.drafter_worker_id, w.agent_id AS "drafter_agent_id?: i64",
                      w.name AS "drafter_name?: String", r.drafter_since,
                      r.close_reason, r.created_at, r.updated_at,
                      (SELECT MAX(d.version) FROM request_drafts d WHERE d.request_id = r.id) AS "latest_version?: i64"
               FROM requests r
               LEFT JOIN workers w ON w.id = r.drafter_worker_id
               "# + $tail
            $(, $arg)*
        )
    };
}

struct RawRequest {
    number: i64,
    kind: Option<String>,
    title: Option<String>,
    body: String,
    state: String,
    requester: String,
    drafter_worker_id: Option<i64>,
    drafter_agent_id: Option<i64>,
    drafter_name: Option<String>,
    drafter_since: Option<i64>,
    close_reason: Option<String>,
    created_at: i64,
    updated_at: i64,
    latest_version: Option<i64>,
}

impl RawRequest {
    fn into_row(self) -> Result<RequestRow, StoreError> {
        let drafter = match (
            self.drafter_worker_id,
            self.drafter_agent_id,
            self.drafter_name,
            self.drafter_since,
        ) {
            (Some(worker_id), Some(agent_id), Some(name), Some(since)) => Some(Drafter {
                worker_id,
                agent_id,
                worker: WorkerName::parse(&name)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                since: time(since, "requests.drafter_since")?,
            }),
            (None, _, _, None) => None,
            _ => return Err(StoreError::Corrupt("requests.drafter_worker_id")),
        };
        Ok(RequestRow {
            id: RequestId::try_from_db(self.number)
                .ok_or(StoreError::Corrupt("requests.number"))?,
            kind: self
                .kind
                .map(|kind| RequestKind::parse(&kind).ok_or(StoreError::Corrupt("requests.kind")))
                .transpose()?,
            title: self.title,
            body: self.body,
            state: RequestState::parse(&self.state).ok_or(StoreError::Corrupt("requests.state"))?,
            requester: Requester::parse(&self.requester)
                .ok_or(StoreError::Corrupt("requests.requester"))?,
            drafter,
            close_reason: self.close_reason,
            created_at: time(self.created_at, "requests.created_at")?,
            updated_at: time(self.updated_at, "requests.updated_at")?,
            latest_version: self
                .latest_version
                .map(|value| count(value, "request_drafts.version"))
                .transpose()?,
        })
    }
}

/// Inserts an `open` request without a kind or a title.
pub async fn insert(tx: &mut WriteTx, request: &NewRequest<'_>) -> Result<(), StoreError> {
    let number = request.number.to_db();
    let requester = request.requester.label();
    let now = request.now.as_millis();
    sqlx::query!(
        "INSERT INTO requests (number, body, requester, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        number,
        request.body,
        requester,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Inserts one attachment row, which names a blob the caller has already written.
pub async fn insert_attachment(
    tx: &mut WriteTx,
    request: RequestId,
    attachment: &NewAttachment<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let position = i64::from(attachment.position);
    let size =
        i64::try_from(attachment.size).map_err(|_| StoreError::Invalid("attachment size"))?;
    let object = attachment.object.to_string();
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO request_attachments (request_id, position, name, media_type, size, object, created_at)
         SELECT id, ?2, ?3, ?4, ?5, ?6, ?7 FROM requests WHERE number = ?1",
        number,
        position,
        attachment.name,
        attachment.media_type,
        size,
        object,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// The request's attachments in position order.
pub async fn attachments(
    conn: &mut SqliteConnection,
    request: RequestId,
) -> Result<Vec<AttachmentRow>, StoreError> {
    let number = request.to_db();
    sqlx::query!(
        "SELECT a.position, a.name, a.media_type, a.size, a.object, a.created_at
         FROM request_attachments a JOIN requests r ON r.id = a.request_id
         WHERE r.number = ? ORDER BY a.position",
        number
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        Ok(AttachmentRow {
            position: count(row.position, "request_attachments.position")?,
            name: row.name,
            media_type: row.media_type,
            size: u64::try_from(row.size)
                .map_err(|_| StoreError::Corrupt("request_attachments.size"))?,
            object: row
                .object
                .parse()
                .map_err(|_| StoreError::Corrupt("request_attachments.object"))?,
            created_at: time(row.created_at, "request_attachments.created_at")?,
        })
    })
    .collect()
}

/// The request with this number, read on the given connection so a command sees its own uncommitted changes.
pub async fn fetch(
    conn: &mut SqliteConnection,
    request: RequestId,
) -> Result<Option<RequestRow>, StoreError> {
    let number = request.to_db();
    select_requests!("WHERE r.number = ?", number)
        .fetch_optional(conn)
        .await?
        .map(RawRequest::into_row)
        .transpose()
}

/// The requests `filter` selects, newest first.
pub async fn list(
    conn: &mut SqliteConnection,
    filter: &RequestFilter,
) -> Result<Vec<RequestRow>, StoreError> {
    let state = filter.state.map(RequestState::as_str);
    let kind = filter.kind.map(RequestKind::as_str);
    let requester = filter.requester.as_ref().map(Requester::label);
    // SQLite reads a negative LIMIT as no limit.
    let limit = filter.limit.map_or(-1, i64::from);
    select_requests!(
        "WHERE (?1 IS NULL OR r.state = ?1)
           AND (?2 IS NULL OR r.kind = ?2)
           AND (?3 IS NULL OR r.requester = ?3)
         ORDER BY r.number DESC
         LIMIT ?4",
        state,
        kind,
        requester,
        limit
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(RawRequest::into_row)
    .collect()
}

fn expect_one(affected: u64) -> Result<(), StoreError> {
    if affected == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the request does not exist"))
    }
}

/// Clears the drafter and moves the request to `state`.
pub async fn release(
    tx: &mut WriteTx,
    request: RequestId,
    state: RequestState,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let state = state.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE requests SET drafter_worker_id = NULL, drafter_since = NULL, state = ?2, updated_at = ?3
         WHERE number = ?1",
        number,
        state,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Makes `worker` (a worker row id) the drafter from `now`, sorts and names the request, and moves it to `state`, in one statement so the row never shows a claimed request without its kind and title.
pub async fn claim(
    tx: &mut WriteTx,
    request: RequestId,
    worker: i64,
    state: RequestState,
    kind: RequestKind,
    title: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let state = state.as_str();
    let kind = kind.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE requests SET drafter_worker_id = ?2, drafter_since = ?6, state = ?3, kind = ?4, title = ?5, updated_at = ?6
         WHERE number = ?1",
        number,
        worker,
        state,
        kind,
        title,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Names the request.
pub async fn set_title(
    tx: &mut WriteTx,
    request: RequestId,
    title: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE requests SET title = ?2, updated_at = ?3 WHERE number = ?1",
        number,
        title,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Sorts the request.
pub async fn set_kind(
    tx: &mut WriteTx,
    request: RequestId,
    kind: RequestKind,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let kind = kind.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE requests SET kind = ?2, updated_at = ?3 WHERE number = ?1",
        number,
        kind,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Moves the request to `state`, keeping its drafter. A closed request needs `close`, which records the reason.
pub async fn set_state(
    tx: &mut WriteTx,
    request: RequestId,
    state: RequestState,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let state = state.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE requests SET state = ?2, updated_at = ?3 WHERE number = ?1",
        number,
        state,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

pub async fn close(
    tx: &mut WriteTx,
    request: RequestId,
    reason: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE requests SET state = 'closed', close_reason = ?2, updated_at = ?3 WHERE number = ?1",
        number,
        reason,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Inserts draft `version` with its cards and edges. A version that exists fails on `request_drafts.request_id, request_drafts.version`. The caller checks that each card label exists; a missing one is stored as no label.
pub async fn insert_draft(
    tx: &mut WriteTx,
    request: RequestId,
    version: u32,
    author: &str,
    note: Option<&str>,
    draft: &Draft,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let version = i64::from(version);
    let now = now.as_millis();
    let draft_id = sqlx::query_scalar!(
        r#"INSERT INTO request_drafts (request_id, version, author, note, created_at)
           SELECT id, ?2, ?3, ?4, ?5 FROM requests WHERE number = ?1
           RETURNING id AS "id!: i64""#,
        number,
        version,
        author,
        note,
        now
    )
    .fetch_optional(tx.conn())
    .await?
    .ok_or(StoreError::Invalid("the request does not exist"))?;
    for (index, card) in draft.cards().iter().enumerate() {
        let position =
            i64::try_from(index + 1).map_err(|_| StoreError::Invalid("too many cards"))?;
        let label = card.label.as_ref().map(LabelName::as_str);
        sqlx::query!(
            "INSERT INTO request_draft_cards (draft_id, key, position, title, description, label_id)
             VALUES (?, ?, ?, ?, ?, (SELECT id FROM labels WHERE name = ?))",
            draft_id,
            card.key,
            position,
            card.title,
            card.description,
            label
        )
        .execute(tx.conn())
        .await?;
    }
    for edge in draft.edges() {
        sqlx::query!(
            "INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key)
             VALUES (?, ?, ?)",
            draft_id,
            edge.prerequisite,
            edge.dependent
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Every draft version of the request, oldest first.
pub async fn drafts(
    conn: &mut SqliteConnection,
    request: RequestId,
) -> Result<Vec<DraftRow>, StoreError> {
    let number = request.to_db();
    let heads = sqlx::query!(
        r#"SELECT d.id AS "id!: i64", d.version, d.author, d.note, d.created_at
           FROM request_drafts d JOIN requests r ON r.id = d.request_id
           WHERE r.number = ? ORDER BY d.version"#,
        number
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut cards: HashMap<i64, Vec<DraftCard>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT c.draft_id, c.key, c.title, c.description, l.name AS "label?: String"
           FROM request_draft_cards c
           JOIN request_drafts d ON d.id = c.draft_id
           JOIN requests r ON r.id = d.request_id
           LEFT JOIN labels l ON l.id = c.label_id
           WHERE r.number = ? ORDER BY c.draft_id, c.position"#,
        number
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let label = row
            .label
            .map(|name| LabelName::parse(&name))
            .transpose()
            .map_err(|_| StoreError::Corrupt("labels.name"))?;
        cards.entry(row.draft_id).or_default().push(DraftCard {
            key: row.key,
            title: row.title,
            description: row.description,
            label,
        });
    }
    let mut edges: HashMap<i64, Vec<DraftEdge>> = HashMap::new();
    for row in sqlx::query!(
        "SELECT e.draft_id, e.prerequisite_key, e.dependent_key
         FROM request_draft_edges e
         JOIN request_drafts d ON d.id = e.draft_id
         JOIN requests r ON r.id = d.request_id
         WHERE r.number = ? ORDER BY e.draft_id, e.id",
        number
    )
    .fetch_all(&mut *conn)
    .await?
    {
        edges.entry(row.draft_id).or_default().push(DraftEdge {
            prerequisite: row.prerequisite_key,
            dependent: row.dependent_key,
        });
    }
    let mut drafts = Vec::with_capacity(heads.len());
    for head in heads {
        drafts.push(DraftRow {
            version: count(head.version, "request_drafts.version")?,
            author: head.author,
            note: head.note,
            created_at: time(head.created_at, "request_drafts.created_at")?,
            cards: cards.remove(&head.id).unwrap_or_default(),
            edges: edges.remove(&head.id).unwrap_or_default(),
        });
    }
    Ok(drafts)
}

pub async fn insert_comment(
    tx: &mut WriteTx,
    request: RequestId,
    version: u32,
    author: &str,
    body: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = request.to_db();
    let version = i64::from(version);
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO request_comments (request_id, version, author, body, created_at)
         SELECT id, ?2, ?3, ?4, ?5 FROM requests WHERE number = ?1",
        number,
        version,
        author,
        body,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// The request's comments in the order they were written.
pub async fn comments(
    conn: &mut SqliteConnection,
    request: RequestId,
) -> Result<Vec<CommentRow>, StoreError> {
    let number = request.to_db();
    sqlx::query!(
        "SELECT c.version, c.author, c.body, c.created_at
         FROM request_comments c JOIN requests r ON r.id = c.request_id
         WHERE r.number = ? ORDER BY c.id",
        number
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        Ok(CommentRow {
            version: count(row.version, "request_comments.version")?,
            author: row.author,
            body: row.body,
            created_at: time(row.created_at, "request_comments.created_at")?,
        })
    })
    .collect()
}

/// Marks a task created in this transaction as made by the request's approval. A trigger keeps the link once it is set. An unknown task or request changes nothing and fails with `StoreError::Invalid`.
pub async fn set_task_request(
    tx: &mut WriteTx,
    task: TaskId,
    request: RequestId,
) -> Result<(), StoreError> {
    let task = task.to_db();
    let number = request.to_db();
    let result = sqlx::query!(
        "UPDATE tasks SET request_id = (SELECT id FROM requests WHERE number = ?2)
         WHERE number = ?1 AND EXISTS (SELECT 1 FROM requests WHERE number = ?2)",
        task,
        number
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// The tasks the request's approval created, in number order.
pub async fn tasks_of(
    conn: &mut SqliteConnection,
    request: RequestId,
) -> Result<Vec<TaskId>, StoreError> {
    let number = request.to_db();
    sqlx::query_scalar!(
        r#"SELECT t.number AS "number!: i64" FROM tasks t JOIN requests r ON r.id = t.request_id
           WHERE r.number = ? ORDER BY t.number"#,
        number
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|number| TaskId::try_from_db(number).ok_or(StoreError::Corrupt("tasks.number")))
    .collect()
}

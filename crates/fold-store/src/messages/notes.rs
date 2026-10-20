use fold_core::{AgentName, AreaName, HumanName, NoteDraft, NoteId, Timestamp, WorkerName};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::columns::{count, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// The values of a new note. The author and the area are named; the insert resolves each to its row.
#[derive(Debug, Clone)]
pub struct NewNote<'a> {
    pub number: NoteId,
    pub author: &'a WorkerName,
    /// The area of the area lead that writes the note; `None` for the main agent and the review lead.
    pub area: Option<&'a AreaName>,
    pub draft: &'a NoteDraft,
    pub now: Timestamp,
}

/// A stored note with public names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub id: NoteId,
    pub author: WorkerName,
    pub area: Option<AreaName>,
    pub title: String,
    pub decision: String,
    pub reason: String,
    /// The links in the mention forms the author wrote, in order.
    pub links: Vec<String>,
    pub created_at: Timestamp,
}

/// Which notes a read takes by their area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteArea<'a> {
    Any,
    /// The notes that belong to no area: those of the main agent and the review lead.
    Without,
    Of(&'a AreaName),
}

/// One page of notes: the newest `limit` older than `before` when given, of the agent `author` when given, in the areas `area` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteQuery<'a> {
    pub author: Option<&'a AgentName>,
    pub area: NoteArea<'a>,
    pub before: Option<NoteId>,
    pub limit: u32,
}

/// Inserts the note. An author worker or an area that does not exist leaves its column empty, which the author's `NOT NULL` refuses and which the caller's earlier lookup of the area prevents.
pub async fn insert(tx: &mut WriteTx, note: &NewNote<'_>) -> Result<(), StoreError> {
    let number = note.number.to_db();
    let agent = note.author.agent();
    let agent = agent.as_str();
    let worker = note.author.as_str();
    let area = note.area.map(AreaName::as_str);
    let title = note.draft.title();
    let decision = note.draft.decision();
    let reason = note.draft.reason();
    let links: Vec<String> = note.draft.links().iter().map(ToString::to_string).collect();
    let links = serde_json::to_string(&links).map_err(|_| StoreError::Invalid("note links"))?;
    let now = note.now.as_millis();
    sqlx::query!(
        "INSERT INTO notes (number, author_worker_id, area_id, title, decision, reason, links, created_at)
         VALUES (?1,
                 (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id
                  WHERE a.name = ?2 AND w.name = ?3),
                 (SELECT id FROM areas WHERE name = ?4), ?5, ?6, ?7, ?8, ?9)",
        number,
        agent,
        worker,
        area,
        title,
        decision,
        reason,
        links,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The notes the query takes, newest first.
pub async fn page(
    conn: &mut SqliteConnection,
    query: &NoteQuery<'_>,
) -> Result<Vec<NoteRow>, StoreError> {
    let author = query.author.map(AgentName::as_str);
    let (scope, area) = match query.area {
        NoteArea::Any => ("any", None),
        NoteArea::Without => ("none", None),
        NoteArea::Of(area) => ("area", Some(area.as_str())),
    };
    let before = query.before.map(NoteId::to_db);
    let limit = i64::from(query.limit);
    let rows = sqlx::query!(
        r#"SELECT n.number, w.name AS author, ar.name AS "area?: String", n.title, n.decision,
                  n.reason, n.links, n.created_at
           FROM notes n
           JOIN workers w ON w.id = n.author_worker_id
           JOIN agents a ON a.id = w.agent_id
           LEFT JOIN areas ar ON ar.id = n.area_id
           WHERE (?1 IS NULL OR a.name = ?1)
             AND (?2 = 'any' OR (?2 = 'none' AND n.area_id IS NULL) OR (?2 = 'area' AND ar.name = ?3))
             AND (?4 IS NULL OR n.number < ?4)
           ORDER BY n.number DESC
           LIMIT ?5"#,
        author,
        scope,
        area,
        before,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(NoteRow {
                id: NoteId::try_from_db(row.number).ok_or(StoreError::Corrupt("notes.number"))?,
                author: WorkerName::parse(&row.author)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                area: row
                    .area
                    .map(|area| AreaName::parse(&area))
                    .transpose()
                    .map_err(|_| StoreError::Corrupt("areas.name"))?,
                title: row.title,
                decision: row.decision,
                reason: row.reason,
                links: serde_json::from_str(&row.links)
                    .map_err(|_| StoreError::Corrupt("notes.links"))?,
                created_at: time(row.created_at, "notes.created_at")?,
            })
        })
        .collect()
}

/// The number of the newest note; `None` before the first.
pub async fn newest(executor: impl SqliteExecutor<'_>) -> Result<Option<NoteId>, StoreError> {
    let number = sqlx::query_scalar!(r#"SELECT MAX(number) AS "number?: i64" FROM notes"#)
        .fetch_one(executor)
        .await?;
    number
        .map(|number| NoteId::try_from_db(number).ok_or(StoreError::Corrupt("notes.number")))
        .transpose()
}

/// The notes newer than the newest one `human` has read; every note for a human who has read none.
pub async fn unread(
    executor: impl SqliteExecutor<'_>,
    human: &HumanName,
) -> Result<u64, StoreError> {
    let human = human.as_str();
    let unread = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM notes
           WHERE number > COALESCE((SELECT through FROM note_reads WHERE human = ?), 0)"#,
        human
    )
    .fetch_one(executor)
    .await?;
    count(unread, "notes")
}

/// Moves the read mark of `human` up to `through`; a mark that is already further keeps its place.
pub async fn mark_read(
    tx: &mut WriteTx,
    human: &HumanName,
    through: NoteId,
    now: Timestamp,
) -> Result<(), StoreError> {
    let human = human.as_str();
    let through = through.to_db();
    let now = now.as_millis();
    sqlx::query!(
        "INSERT INTO note_reads (human, through, read_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (human) DO UPDATE SET through = max(through, excluded.through), read_at = excluded.read_at",
        human,
        through,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

use fold_core::events::types;
use fold_core::policy::NoteAuthor;
use fold_core::{
    Actor, AgentName, AreaName, FoldError, HumanName, IdKind, NewEvent, NoteDraft, NoteId,
    Timestamp, WorkerName, policy,
};
use fold_store::messages::notes::{self, NewNote, NoteArea, NoteQuery, NoteRow};
use fold_store::{SqliteConnection, StoreError, agents, counters};
use serde_json::json;

use super::{MessageError, MessageService, require_sender};
use crate::services::events::EventBuffer;
use crate::services::page_limit;

/// The page size of a notes read that names no `limit`.
pub const DEFAULT_NOTE_LIMIT: u32 = 50;

/// The largest notes page one read returns; a larger `limit` is lowered to it.
pub const MAX_NOTE_LIMIT: u32 = 200;

/// A decision note with public names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteView {
    pub id: NoteId,
    /// The worker that wrote the note.
    pub author: WorkerName,
    /// The area of the area lead that wrote it; `None` for a note of the main agent or the review lead.
    pub area: Option<AreaName>,
    pub title: String,
    pub decision: String,
    pub reason: String,
    /// Links in fold's mention forms, such as `#T-12` or `@web/src/board.ts`.
    pub links: Vec<String>,
    pub written_at: Timestamp,
}

impl From<NoteRow> for NoteView {
    fn from(row: NoteRow) -> Self {
        Self {
            id: row.id,
            author: row.author,
            area: row.area,
            title: row.title,
            decision: row.decision,
            reason: row.reason,
            links: row.links,
            written_at: row.created_at,
        }
    }
}

/// Which notes a read returns: those of one agent, of one area, or both, older than `before`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoteFilter {
    pub agent: Option<AgentName>,
    pub area: Option<AreaName>,
    pub before: Option<NoteId>,
    pub limit: Option<u32>,
}

/// One page of notes, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotePage {
    pub notes: Vec<NoteView>,
    /// The `before` of the page of older notes; `None` when this page holds the oldest.
    pub next: Option<NoteId>,
    /// The notes in the whole project that the reading human has not read; `None` when the read names no human.
    pub unread: Option<u64>,
}

impl MessageService {
    /// Writes note `n-<n>` for the actor and appends `note.added`, whose data names the note, its author and its area and never its text. [`policy::can_add_note`] admits the main agent and the leads; a note of an area lead belongs to its area. A note asks the human nothing and changes no task.
    pub async fn add_note(
        &self,
        actor: &Actor,
        draft: NoteDraft,
    ) -> Result<NoteView, MessageError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                let standing = agents::standing_of(tx.conn(), actor).await?;
                let author = policy::can_add_note(actor, standing).map_err(FoldError::from)?;
                require_sender(tx, actor).await?;
                let Actor::Agent(agent) = actor else {
                    return Err(StoreError::Corrupt("notes.author").into());
                };
                let area = match author {
                    NoteAuthor::AreaLead => Some(
                        agents::find_worker_process(tx.conn(), &agent.agent)
                            .await?
                            .and_then(|row| row.area)
                            .ok_or(StoreError::Corrupt("worker_processes.area_id"))?,
                    ),
                    NoteAuthor::Main | NoteAuthor::ReviewLead => None,
                };
                let number = counters::allocate(tx, IdKind::Note).await?;
                let id = NoteId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
                notes::insert(
                    tx,
                    &NewNote {
                        number: id,
                        author: &agent.worker,
                        area: area.as_ref(),
                        draft: &draft,
                        now,
                    },
                )
                .await?;
                let event = NewEvent::new(types::NOTE_ADDED, actor.event_actor())?.data(json!({
                    "note": id.to_string(),
                    "agent": agent.agent.as_str(),
                    "area": area.as_ref().map(AreaName::as_str),
                }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, MessageError>(NoteView {
                    id,
                    author: agent.worker.clone(),
                    area,
                    title: draft.title().to_owned(),
                    decision: draft.decision().to_owned(),
                    reason: draft.reason().to_owned(),
                    links: draft.links().iter().map(ToString::to_string).collect(),
                    written_at: now,
                })
            })
            .await?;
        self.bus.publish(buffer);
        tracing::info!(
            note = %view.id,
            author = view.author.as_str(),
            area = view.area.as_ref().map(AreaName::as_str),
            "note added"
        );
        Ok(view)
    }

    /// The newest notes the filter takes, with how many notes of the project `reader` has not read when a human reads.
    pub async fn notes(
        &self,
        filter: &NoteFilter,
        reader: Option<&HumanName>,
    ) -> Result<NotePage, MessageError> {
        let limit = page_limit(filter.limit, MAX_NOTE_LIMIT)?.unwrap_or(DEFAULT_NOTE_LIMIT);
        let area = match &filter.area {
            Some(area) => NoteArea::Of(area),
            None => NoteArea::Any,
        };
        let query = NoteQuery {
            author: filter.agent.as_ref(),
            area,
            before: filter.before,
            limit: limit + 1,
        };
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let mut rows = notes::page(&mut tx, &query).await?;
        let unread = match reader {
            Some(human) => Some(notes::unread(&mut *tx, human).await?),
            None => None,
        };
        tx.commit().await.map_err(StoreError::from)?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let notes: Vec<NoteView> = rows.into_iter().map(NoteView::from).collect();
        let next = more.then(|| notes.last().map(|note| note.id)).flatten();
        Ok(NotePage {
            notes,
            next,
            unread,
        })
    }

    /// Marks every note up to `through` as read by the acting human, and returns how many notes the human has not read after it. A `through` past the newest note marks the newest.
    pub async fn mark_notes_read(
        &self,
        actor: &Actor,
        through: NoteId,
    ) -> Result<u64, MessageError> {
        policy::can_mark_notes_read(actor).map_err(FoldError::from)?;
        let Actor::Human { name } = actor else {
            return Err(StoreError::Corrupt("note_reads.human").into());
        };
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                if let Some(newest) = notes::newest(tx.conn()).await? {
                    let through = through.min(newest);
                    notes::mark_read(tx, name, through, now).await?;
                }
                Ok::<_, MessageError>(notes::unread(tx.conn(), name).await?)
            })
            .await
    }
}

/// The newest `limit` notes that `agent` starts with: every note for the main agent, its area's notes for an area lead and its task workers, and the notes without an area for the review lead and its reviewers. An agent that is none of these starts with no notes.
pub(crate) async fn start_notes(
    conn: &mut SqliteConnection,
    agent: &AgentName,
    limit: u32,
) -> Result<Vec<NoteView>, StoreError> {
    let standing = agents::standing_of_agent(&mut *conn, agent).await?;
    let process = agents::find_worker_process(&mut *conn, agent).await?;
    let area = match (&process, standing.is_main_agent()) {
        (_, true) => NoteArea::Any,
        (Some(process), false) => match &process.area {
            Some(area) => NoteArea::Of(area),
            None => NoteArea::Without,
        },
        (None, false) => return Ok(Vec::new()),
    };
    let query = NoteQuery {
        author: None,
        area,
        before: None,
        limit,
    };
    let rows = notes::page(conn, &query).await?;
    Ok(rows.into_iter().map(NoteView::from).collect())
}

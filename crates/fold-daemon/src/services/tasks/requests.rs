use std::collections::BTreeMap;

use fold_core::events::types;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::requests::{
    AttachmentFacts, DraftCard, DraftEdge, RequestKind, RequestState, RequestStep, Requester,
    check_attachments, deputy, drafter_lapsed, excerpt, request_text, request_title,
    validate_draft,
};
use fold_core::{
    Actor, DependencyOrigin, FoldError, IdKind, MessageId, NewEvent, RequestId, TaskDescription,
    TaskId, TaskKind, TaskTitle, Timestamp, WorkerName, codes, policy,
};
use fold_store::tasks::requests::{self, NewAttachment, NewRequest, RequestRow};
pub use fold_store::tasks::requests::{AttachmentRow as RequestAttachment, RequestFilter};
use fold_store::{StoreError, WriteTx, counters};
use fold_store::{agents, messages};
use fold_vcs::objects::ObjectStore;
use serde_json::json;

use super::{
    CreateTask, TaskError, TaskService, add_edge_in_tx, create_in_tx, recompute, require_labels,
    worker_row,
};
use crate::services::events::EventBuffer;
use crate::services::page_limit;
use crate::support::blocking;

/// The most requests one list returns.
pub const MAX_REQUEST_LIST: u32 = 200;

/// The requests an agent's list returns when it names no limit.
pub const DEFAULT_REQUEST_LIST: u32 = 50;

/// A file dropped on a new request. The transports read it from a workspace path or an upload; the service stores its bytes as a blob before the row that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequestAttachment {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

impl NewRequestAttachment {
    /// An attachment whose media type is guessed from its name's extension, `application/octet-stream` when the extension says nothing.
    pub fn named(name: String, bytes: Vec<u8>) -> Self {
        let media_type = mime_guess::from_path(&name)
            .first_or_octet_stream()
            .essence_str()
            .to_owned();
        Self {
            name,
            media_type,
            bytes,
        }
    }
}

/// A request with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestView {
    pub id: RequestId,
    /// `None` until an orchestrator claims and sorts the request.
    pub kind: Option<RequestKind>,
    /// `None` until an orchestrator claims and names the request.
    pub title: Option<String>,
    /// The start of the body, which a list shows while the request has no title.
    pub excerpt: String,
    pub body: String,
    pub state: RequestState,
    pub requester: Requester,
    pub drafter: Option<WorkerName>,
    pub drafter_since: Option<Timestamp>,
    pub close_reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub latest_version: Option<u32>,
}

impl From<RequestRow> for RequestView {
    fn from(row: RequestRow) -> Self {
        let (drafter, drafter_since) = match row.drafter {
            Some(drafter) => (Some(drafter.worker), Some(drafter.since)),
            None => (None, None),
        };
        RequestView {
            id: row.id,
            kind: row.kind,
            title: row.title,
            excerpt: excerpt(&row.body),
            body: row.body,
            state: row.state,
            requester: row.requester,
            drafter,
            drafter_since,
            close_reason: row.close_reason,
            created_at: row.created_at,
            updated_at: row.updated_at,
            latest_version: row.latest_version,
        }
    }
}

/// One draft version. `author` is the actor label of the drafter's worker or of the requester.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestDraft {
    pub version: u32,
    pub author: String,
    pub note: Option<String>,
    pub created_at: Timestamp,
    pub cards: Vec<DraftCard>,
    pub edges: Vec<DraftEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestComment {
    pub version: u32,
    pub author: String,
    pub body: String,
    pub created_at: Timestamp,
}

/// The request's clarification thread at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestThread {
    /// The id of the thread's first message.
    pub id: MessageId,
    pub messages: u32,
    pub last_at: Timestamp,
}

/// A request with everything that belongs to it: attachment metadata, every draft version, the requester's comments, its clarification thread and the tasks its approval created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestDetail {
    pub request: RequestView,
    pub attachments: Vec<RequestAttachment>,
    pub drafts: Vec<RequestDraft>,
    pub comments: Vec<RequestComment>,
    /// `None` before anyone wrote about the request.
    pub thread: Option<RequestThread>,
    pub tasks: Vec<TaskId>,
}

/// A draft as a drafter or the requester sends it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubmitDraft {
    pub cards: Vec<DraftCard>,
    pub edges: Vec<DraftEdge>,
    pub note: Option<String>,
}

/// What an approval made: the request, now approved, and one task per card in card order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedRequest {
    pub request: RequestView,
    pub tasks: Vec<TaskId>,
}

/// Request commands. A request belongs to the tasks domain because its approval creates tasks; messages about a request never move its state.
impl TaskService {
    /// Creates an `open` request with no kind and no title: the requester writes only the message, in Markdown, with optional attachments. Mentions such as `#T-12` or `@web/src/board.ts` stay text in the body; nothing parses them. The attachment bytes are written to the object store under the project's object lock before the transaction that records them.
    pub async fn create_request(
        &self,
        actor: &Actor,
        body: &str,
        attachments: Vec<NewRequestAttachment>,
    ) -> Result<RequestView, TaskError> {
        let requester = policy::can_create_request(actor).map_err(FoldError::from)?;
        let body = request_text(body, "body", false)?.unwrap_or_default();
        let facts: Vec<AttachmentFacts<'_>> = attachments
            .iter()
            .map(|attachment| AttachmentFacts {
                name: &attachment.name,
                media_type: &attachment.media_type,
                size: u64::try_from(attachment.bytes.len()).unwrap_or(u64::MAX),
            })
            .collect();
        check_attachments(&facts)?;
        let mut metadata = Vec::with_capacity(attachments.len());
        let mut blobs = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            metadata.push((
                attachment.name,
                attachment.media_type,
                u64::try_from(attachment.bytes.len()).unwrap_or(u64::MAX),
            ));
            blobs.push(attachment.bytes);
        }
        let (_objects, hashes) = self.store_blobs(blobs).await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let id = self
            .store
            .write(async |tx| {
                let id = next_request_id(tx).await?;
                let new = NewRequest {
                    number: id,
                    body: &body,
                    requester: &requester,
                    now,
                };
                requests::insert(tx, &new).await?;
                for (position, ((name, media_type, size), object)) in
                    (1..).zip(metadata.iter().zip(&hashes))
                {
                    let attachment = NewAttachment {
                        position,
                        name,
                        media_type,
                        size: *size,
                        object,
                    };
                    requests::insert_attachment(tx, id, &attachment, now).await?;
                }
                let event = NewEvent::new(types::REQUEST_CREATED, actor.event_actor())?
                    .data(json!({"request": id.to_string(), "attachments": hashes.len()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(id)
            })
            .await?;
        self.bus.publish(buffer);
        self.request_view(id).await
    }

    /// Writes each blob and returns the object lock guard, held until the caller's transaction has committed, with the hashes in order. A request without attachments needs neither the store nor the lock.
    async fn store_blobs(
        &self,
        blobs: Vec<Vec<u8>>,
    ) -> Result<
        (
            Option<tokio::sync::OwnedRwLockReadGuard<()>>,
            Vec<ObjectHash>,
        ),
        TaskError,
    > {
        if blobs.is_empty() {
            return Ok((None, Vec::new()));
        }
        let access = self.object_access()?;
        let guard = access.lock.read_owned().await;
        let fold_dir = access.fold_dir;
        let hashes = blocking(move || {
            let objects = ObjectStore::open_existing(&fold_dir)?;
            Ok(blobs
                .iter()
                .map(|bytes| objects.write(ObjectKind::Blob, bytes))
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
        Ok((Some(guard), hashes))
    }

    /// Makes the actor's main worker the drafter, sorts the request as `kind` and names it `title`; an `open` request becomes `drafting`. Only the main agent or an area lead claims (`orchestrator_required`); an area lead judges for itself whether the request concerns its area. A request whose drafter's agent has had no open session for `lease_minutes` is taken over (`taken_over: true`); any other held request is `already_claimed`. A repeat claim by the drafter changes nothing.
    pub async fn claim_request(
        &self,
        actor: &Actor,
        request: RequestId,
        kind: RequestKind,
        title: &str,
    ) -> Result<RequestView, TaskError> {
        let title = request_title(title)?;
        let lease_minutes = self.lease_minutes().await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let role = match actor {
                    Actor::Agent(agent) => agents::find_worker_process(tx.conn(), &agent.agent)
                        .await?
                        .map(|process| process.role),
                    Actor::Human { .. } | Actor::System => None,
                };
                let claimant = policy::can_claim_request(actor, role).map_err(FoldError::from)?;
                let worker = worker_row(tx, claimant).await?;
                let row = open_row(tx, request).await?;
                let next = row.state.after(RequestStep::Claim, request)?;
                let mut data = json!({
                    "request": request.to_string(),
                    "worker": worker.name.as_str(),
                    "kind": kind.as_str(),
                    "taken_over": false,
                });
                if let Some(drafter) = &row.drafter {
                    if drafter.worker_id == worker.id {
                        return Ok(());
                    }
                    let (open, last_end) =
                        agents::agent_activity(tx.conn(), drafter.agent_id).await?;
                    if !drafter_lapsed(open, last_end, drafter.since, now, lease_minutes) {
                        return Err(FoldError::new(
                            codes::ALREADY_CLAIMED,
                            format!("{request} is drafted by {}", drafter.worker),
                        )
                        .with_details([
                            ("request", json!(request.to_string())),
                            ("holder", json!(drafter.worker.as_str())),
                        ])
                        .with_hint("call list_requests to pick another request")
                        .into());
                    }
                    data["taken_over"] = json!(true);
                    data["from"] = json!(drafter.worker.as_str());
                }
                requests::claim(tx, request, worker.id, next, kind, &title, now).await?;
                let event =
                    NewEvent::new(types::REQUEST_CLAIMED, actor.event_actor())?.data(data)?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.request_view(request).await
    }

    /// Gives the request up. A `drafting` request returns to `open`; a request with a draft keeps its state for the next drafter or the requester.
    pub async fn release_request(
        &self,
        actor: &Actor,
        request: RequestId,
    ) -> Result<RequestView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = open_row(tx, request).await?;
                let drafter = row.drafter.as_ref().map(|drafter| &drafter.worker);
                policy::is_drafter(actor, drafter).map_err(FoldError::from)?;
                let next = row.state.after(RequestStep::Release, request)?;
                requests::release(tx, request, next, now).await?;
                let event = NewEvent::new(types::REQUEST_RELEASED, actor.event_actor())?.data(
                    json!({"request": request.to_string(), "worker": drafter.map(WorkerName::as_str)}),
                )?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.request_view(request).await
    }

    /// Adds the next draft version written by the drafter's agent, which may re-sort and rename the request. The request becomes `draft_ready`. A card label that does not exist is `label_not_found`.
    pub async fn submit_request_draft(
        &self,
        actor: &Actor,
        request: RequestId,
        draft: SubmitDraft,
        kind: Option<RequestKind>,
        title: Option<&str>,
    ) -> Result<RequestView, TaskError> {
        let title = title.map(request_title).transpose()?;
        self.add_draft(actor, request, draft, (kind, title), Role::Drafter)
            .await
    }

    /// Adds the next draft version written by the requester, who changes the cards themselves instead of asking. The request becomes `draft_ready`.
    pub async fn edit_request_draft(
        &self,
        actor: &Actor,
        request: RequestId,
        draft: SubmitDraft,
    ) -> Result<RequestView, TaskError> {
        self.add_draft(actor, request, draft, (None, None), Role::Requester)
            .await
    }

    async fn add_draft(
        &self,
        actor: &Actor,
        request: RequestId,
        draft: SubmitDraft,
        (kind, title): (Option<RequestKind>, Option<String>),
        role: Role,
    ) -> Result<RequestView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = open_row(tx, request).await?;
                let step = match role {
                    Role::Drafter => {
                        let drafter = row.drafter.as_ref().map(|drafter| &drafter.worker);
                        policy::is_drafter(actor, drafter).map_err(FoldError::from)?;
                        RequestStep::Submit
                    }
                    Role::Requester => {
                        require_requester(tx, actor, &row).await?;
                        RequestStep::Edit
                    }
                };
                let next = row.state.after(step, request)?;
                let checked = validate_draft(draft.cards.clone(), draft.edges.clone())
                    .map_err(FoldError::from)?;
                let labels: Vec<_> = checked
                    .cards()
                    .iter()
                    .filter_map(|card| card.label.clone())
                    .collect();
                require_labels(tx, &labels).await?;
                let note = match &draft.note {
                    Some(note) => request_text(note, "note", true)?,
                    None => None,
                };
                let version = row.latest_version.map_or(1, |latest| latest + 1);
                let author = actor.event_actor();
                requests::insert_draft(
                    tx,
                    request,
                    version,
                    author.as_str(),
                    note.as_deref(),
                    &checked,
                    now,
                )
                .await?;
                requests::set_state(tx, request, next, now).await?;
                if let Some(kind) = kind {
                    requests::set_kind(tx, request, kind, now).await?;
                }
                if let Some(title) = &title {
                    requests::set_title(tx, request, title, now).await?;
                }
                let event = NewEvent::new(types::REQUEST_DRAFT_SUBMITTED, author)?.data(json!({
                    "request": request.to_string(),
                    "version": version,
                    "role": role.as_str(),
                    "cards": checked.cards().len(),
                    "edges": checked.edges().len(),
                }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.request_view(request).await
    }

    /// The requester sends the latest draft version back with a comment; the request becomes `changes_requested` and the drafter owes a new version. An older version is `stale_version`.
    pub async fn request_draft_changes(
        &self,
        actor: &Actor,
        request: RequestId,
        version: u32,
        comment: &str,
    ) -> Result<RequestView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = open_row(tx, request).await?;
                require_requester(tx, actor, &row).await?;
                let next = row.state.after(RequestStep::RequestChanges, request)?;
                require_latest(&row, version)?;
                let comment = request_text(comment, "comment", false)?.unwrap_or_default();
                let author = actor.event_actor();
                requests::insert_comment(tx, request, version, author.as_str(), &comment, now)
                    .await?;
                requests::set_state(tx, request, next, now).await?;
                let event = NewEvent::new(types::REQUEST_CHANGES_REQUESTED, author)?
                    .data(json!({"request": request.to_string(), "version": version}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.request_view(request).await
    }

    /// The requester approves the latest draft version: one task per card, created by the same rules as `create_task` and linked to the request, one direct dependency per edge by the same rules as `add_dependency`, a recompute seeded with the new tasks so each lands in its calculated column, and `request.approved` naming the tasks, all in one transaction. A version other than the latest is `stale_version`.
    pub async fn approve_request_draft(
        &self,
        actor: &Actor,
        request: RequestId,
        expected_version: u32,
    ) -> Result<ApprovedRequest, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let tasks = self
            .store
            .write(async |tx| {
                let row = open_row(tx, request).await?;
                require_requester(tx, actor, &row).await?;
                let next = row.state.after(RequestStep::Approve, request)?;
                require_latest(&row, expected_version)?;
                let draft = requests::drafts(tx.conn(), request)
                    .await?
                    .into_iter()
                    .find(|draft| draft.version == expected_version)
                    .ok_or(StoreError::Corrupt("request_drafts.version"))?;
                let mut by_key: BTreeMap<&str, TaskId> = BTreeMap::new();
                let mut created = Vec::with_capacity(draft.cards.len());
                for card in &draft.cards {
                    let input = CreateTask {
                        title: TaskTitle::parse(&card.title)
                            .map_err(|_| StoreError::Corrupt("request_draft_cards.title"))?,
                        description: TaskDescription::new(card.description.clone()),
                        labels: card.label.iter().cloned().collect(),
                    };
                    let task =
                        create_in_tx(tx, &mut buffer, actor, &input, TaskKind::Proposal, now)
                            .await?;
                    requests::set_task_request(tx, task, request).await?;
                    by_key.insert(&card.key, task);
                    created.push(task);
                }
                for edge in &draft.edges {
                    let (Some(&dependent), Some(&prerequisite)) = (
                        by_key.get(edge.dependent.as_str()),
                        by_key.get(edge.prerequisite.as_str()),
                    ) else {
                        return Err(StoreError::Corrupt("request_draft_edges.key").into());
                    };
                    add_edge_in_tx(
                        tx,
                        &mut buffer,
                        dependent,
                        prerequisite,
                        DependencyOrigin::Direct,
                        actor,
                        now,
                    )
                    .await?;
                }
                buffer.extend(recompute(tx, &created, now).await?.events);
                requests::set_state(tx, request, next, now).await?;
                let names: Vec<String> = created.iter().map(TaskId::to_string).collect();
                let drafter = row.drafter.as_ref().map(|drafter| drafter.worker.as_str());
                let event =
                    NewEvent::new(types::REQUEST_APPROVED, actor.event_actor())?.data(json!({
                        "request": request.to_string(),
                        "version": expected_version,
                        "tasks": names,
                        "drafter": drafter,
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(created)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(ApprovedRequest {
            request: self.request_view(request).await?,
            tasks,
        })
    }

    /// The requester closes the request with a reason; it then only reads.
    pub async fn close_request(
        &self,
        actor: &Actor,
        request: RequestId,
        reason: &str,
    ) -> Result<RequestView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = open_row(tx, request).await?;
                require_requester(tx, actor, &row).await?;
                row.state.after(RequestStep::Close, request)?;
                let reason = request_text(reason, "reason", false)?.unwrap_or_default();
                requests::close(tx, request, &reason, now).await?;
                let event = NewEvent::new(types::REQUEST_CLOSED, actor.event_actor())?
                    .data(json!({"request": request.to_string()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.request_view(request).await
    }

    async fn request_view(&self, request: RequestId) -> Result<RequestView, TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = requests::fetch(&mut conn, request)
            .await?
            .ok_or_else(|| request_not_found(request))?;
        Ok(row.into())
    }

    /// The request with its attachment metadata, drafts, comments and created tasks.
    pub async fn get_request(&self, request: RequestId) -> Result<RequestDetail, TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = requests::fetch(&mut conn, request)
            .await?
            .ok_or_else(|| request_not_found(request))?;
        let attachments = requests::attachments(&mut conn, request).await?;
        let drafts = requests::drafts(&mut conn, request)
            .await?
            .into_iter()
            .map(|draft| RequestDraft {
                version: draft.version,
                author: draft.author,
                note: draft.note,
                created_at: draft.created_at,
                cards: draft.cards,
                edges: draft.edges,
            })
            .collect();
        let comments = requests::comments(&mut conn, request)
            .await?
            .into_iter()
            .map(|comment| RequestComment {
                version: comment.version,
                author: comment.author,
                body: comment.body,
                created_at: comment.created_at,
            })
            .collect();
        let tasks = requests::tasks_of(&mut conn, request).await?;
        let thread = match messages::request_thread(&mut conn, request).await? {
            Some(id) => {
                let rows = messages::thread(&mut conn, id).await?;
                Some(RequestThread {
                    id,
                    messages: u32::try_from(rows.len()).unwrap_or(u32::MAX),
                    last_at: rows.last().map_or(Timestamp::EPOCH, |row| row.created_at),
                })
            }
            None => None,
        };
        Ok(RequestDetail {
            request: row.into(),
            attachments,
            drafts,
            comments,
            thread,
            tasks,
        })
    }

    /// One attachment's metadata and bytes, by its 1-based position. A position the request does not have is `not_found`.
    pub async fn request_attachment(
        &self,
        request: RequestId,
        position: u32,
    ) -> Result<(RequestAttachment, Vec<u8>), TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        if requests::fetch(&mut conn, request).await?.is_none() {
            return Err(request_not_found(request));
        }
        let attachment = requests::attachments(&mut conn, request)
            .await?
            .into_iter()
            .find(|attachment| attachment.position == position)
            .ok_or_else(|| {
                FoldError::new(
                    codes::NOT_FOUND,
                    format!("{request} has no attachment {position}"),
                )
                .with_details([
                    ("request", json!(request.to_string())),
                    ("position", json!(position)),
                ])
                .with_hint("call get_request to see the attachments")
            })?;
        drop(conn);
        let access = self.object_access()?;
        let hash = attachment.object;
        let bytes = blocking(move || {
            Ok(ObjectStore::open_existing(&access.fold_dir)?
                .read_expecting(&hash, ObjectKind::Blob)?
                .payload)
        })
        .await?;
        Ok((attachment, bytes))
    }

    fn object_access(&self) -> Result<super::ObjectAccess, TaskError> {
        Ok(self.objects.clone().ok_or_else(|| {
            FoldError::new(
                codes::INTERNAL_ERROR,
                "this service has no object store for attachments",
            )
        })?)
    }

    /// The requests `filter` selects, newest first. A limit of 0 is `invalid_request`; one over [`MAX_REQUEST_LIST`] is lowered to it.
    pub async fn list_requests(
        &self,
        filter: &RequestFilter,
    ) -> Result<Vec<RequestView>, TaskError> {
        let filter = RequestFilter {
            limit: page_limit(filter.limit, MAX_REQUEST_LIST)?,
            ..filter.clone()
        };
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        Ok(requests::list(&mut conn, &filter)
            .await?
            .into_iter()
            .map(RequestView::from)
            .collect())
    }
}

#[derive(Debug, Clone, Copy)]
enum Role {
    Drafter,
    Requester,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Drafter => "drafter",
            Role::Requester => "requester",
        }
    }
}

async fn next_request_id(tx: &mut WriteTx) -> Result<RequestId, TaskError> {
    let number = counters::allocate(tx, IdKind::Request).await?;
    Ok(RequestId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?)
}

/// Refuses `actor` unless it is the request's requester, or the deputy that acts for an agent requester that fold supervises.
async fn require_requester(
    tx: &mut WriteTx,
    actor: &Actor,
    row: &RequestRow,
) -> Result<(), TaskError> {
    let deputy = requester_deputy(tx, &row.requester).await?;
    policy::is_requester(actor, &row.requester, deputy.as_ref()).map_err(FoldError::from)?;
    Ok(())
}

/// The deputy of an agent requester that fold supervises, from its worker process.
async fn requester_deputy(
    tx: &mut WriteTx,
    requester: &Requester,
) -> Result<Option<Requester>, StoreError> {
    let Requester::Agent(agent) = requester else {
        return Ok(None);
    };
    Ok(agents::find_worker_process(tx.conn(), agent)
        .await?
        .and_then(|process| deputy(process.role, &process.started_by)))
}

/// The request read in the command's transaction, refused when it does not exist or only reads.
async fn open_row(tx: &mut WriteTx, request: RequestId) -> Result<RequestRow, TaskError> {
    let row = requests::fetch(tx.conn(), request)
        .await?
        .ok_or_else(|| request_not_found(request))?;
    row.state.require_open(request)?;
    Ok(row)
}

fn require_latest(row: &RequestRow, version: u32) -> Result<(), TaskError> {
    if row.latest_version == Some(version) {
        return Ok(());
    }
    let latest = row.latest_version.map_or_else(
        || "no draft".to_owned(),
        |latest| format!("version {latest}"),
    );
    Err(FoldError::new(
        codes::STALE_VERSION,
        format!("{} is at {latest}, not version {version}", row.id),
    )
    .with_details([
        ("expected", json!(version)),
        ("latest", json!(row.latest_version)),
    ])
    .with_hint("call get_request to read the latest version")
    .into())
}

pub(crate) fn request_not_found(request: RequestId) -> TaskError {
    TaskError::Fold(request_missing(request))
}

pub(crate) fn request_missing(request: RequestId) -> FoldError {
    FoldError::new(
        codes::REQUEST_NOT_FOUND,
        format!("{request} does not exist"),
    )
    .with_hint("call list_requests to see the requests")
}

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fold_core::requests::{
    CodeAnchor, CodeRef, DraftCard, DraftEdge, MAX_ATTACHMENT_BYTES, RequestKind, RequestState,
    Requester,
};
use fold_core::{Actor, FoldError, LabelName, RequestId, WorkspaceId, codes};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{ContentBlock, JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::error::{from_task, from_workspace};
use super::super::parse;
use super::super::{FoldServer, Reply};
use crate::services::tasks::{
    DEFAULT_REQUEST_LIST, NewRequestAttachment, RequestAttachment, RequestDetail, RequestFilter,
    RequestView, SubmitDraft,
};
use crate::support::blocking;

pub const CREATE_REQUEST: &str = "create_request";
pub const LIST_REQUESTS: &str = "list_requests";
pub const GET_REQUEST: &str = "get_request";
pub const GET_REQUEST_ATTACHMENT: &str = "get_request_attachment";
pub const CLAIM_REQUEST: &str = "claim_request";
pub const RELEASE_REQUEST: &str = "release_request";
pub const SUBMIT_REQUEST_DRAFT: &str = "submit_request_draft";
pub const REQUEST_DRAFT_CHANGES: &str = "request_draft_changes";
pub const EDIT_REQUEST_DRAFT: &str = "edit_request_draft";
pub const APPROVE_REQUEST_DRAFT: &str = "approve_request_draft";
pub const CLOSE_REQUEST: &str = "close_request";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateRequestInput {
    /// What you need and why, in Markdown. In message text `#` names a fold item (#T-12, #P-3) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    pub body: String,
    /// Files to attach, as paths inside your own workspace `ws`, such as logs/run.log: at most 10 files of at most 10 MiB each.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Your workspace, such as ws-9, that holds the attachments. Needed only with attachments.
    #[serde(default)]
    pub ws: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListRequestsInput {
    /// Only requests in this state: open, drafting, draft_ready, changes_requested, approved or closed.
    #[serde(default)]
    pub state: Option<String>,
    /// Only requests of this kind: bug, feature or question.
    #[serde(default)]
    pub kind: Option<String>,
    /// Only the requests your agent created.
    #[serde(default)]
    pub mine: bool,
    /// The most requests to return, newest first: 1 to 200, 50 when left out.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetRequestInput {
    /// The request, such as RQ-4.
    pub id: String,
    /// The draft version to read. Leave it out for the latest.
    #[serde(default)]
    pub version: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetRequestAttachmentInput {
    /// The request, such as RQ-4.
    pub id: String,
    /// The attachment's 1-based position, as get_request lists it.
    pub n: u32,
    /// For a text attachment, the byte offset of the page to read: 0 or the next_offset of the page before. Leave it out for the first page.
    #[serde(default)]
    pub offset: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ClaimRequestInput {
    /// The request, such as RQ-4.
    pub id: String,
    /// What the request asks for, as you read it: bug, feature or question.
    pub kind: String,
    /// A title you write for the request: one line, at most 200 characters.
    pub title: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RequestIdInput {
    /// The request, such as RQ-4.
    pub id: String,
}

/// One card of a draft.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CardInput {
    /// A key local to the draft, such as c1, that the edges use: letters, digits, `-` and `_`.
    pub key: String,
    /// The task title: one line, at most 200 characters.
    pub title: String,
    /// The task description in Markdown, in the task template. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub description: String,
    /// The label of the area that builds this card, such as backend; the main agent sets it. Approval gives the task this label, and fold sends the ready task to the area's lead. Create the label with create_label first; main, the label of the main agent's own area, always exists, and its ready task goes to the main agent, which builds it. Leave it out for a task that the main agent routes itself.
    #[serde(default)]
    pub label: Option<String>,
}

/// One dependency between two cards of a draft.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct EdgeInput {
    /// The key of the card that must be done first.
    pub prerequisite: String,
    /// The key of the card that needs it.
    pub dependent: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitRequestDraftInput {
    /// The request you claimed, such as RQ-4.
    pub id: String,
    /// The cards of the draft, 1 to 20; approval turns each into a task.
    pub cards: Vec<CardInput>,
    /// The dependencies between the cards; approval turns each into a dependency between the tasks.
    #[serde(default)]
    pub edges: Vec<EdgeInput>,
    /// A note to the requester about this version. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    #[serde(default)]
    pub note: Option<String>,
    /// A new kind for the request: bug, feature or question. Leave it out to keep the kind.
    #[serde(default)]
    pub kind: Option<String>,
    /// A new title for the request. Leave it out to keep the title.
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RequestDraftChangesInput {
    /// The request you created, such as RQ-4.
    pub id: String,
    /// The draft version you read, which must be the latest.
    pub version: u32,
    /// What the drafter must change. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub comment: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct EditRequestDraftInput {
    /// The request you created, such as RQ-4.
    pub id: String,
    /// The cards of your version, 1 to 20.
    pub cards: Vec<CardInput>,
    /// The dependencies between the cards.
    #[serde(default)]
    pub edges: Vec<EdgeInput>,
    /// A note about this version. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ApproveRequestDraftInput {
    /// The request you created, such as RQ-4.
    pub id: String,
    /// The draft version you read and approve; an older one is refused with stale_version.
    pub expected_version: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CloseRequestInput {
    /// The request you created, such as RQ-4.
    pub id: String,
    /// Why you close it. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub reason: String,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            CREATE_REQUEST,
            "Ask for work: a bug, a feature or a question, written as a message with optional files from your workspace. The main agent or an area lead sorts and names it, then drafts the cards.",
            schema_for_type::<CreateRequestInput>(),
        ),
        Tool::new(
            LIST_REQUESTS,
            "List the requests, newest first, with their state, kind, title or the start of the message, and drafter.",
            schema_for_type::<ListRequestsInput>(),
        ),
        Tool::new(
            GET_REQUEST,
            "Read one request: the message, the attachments (read one with get_request_attachment), the code it was made on (path, lines, an excerpt and the revision or proposal version), one draft version with its cards and edges, the requester's comments, the clarification thread and the tasks an approval created.",
            schema_for_type::<GetRequestInput>(),
        ),
        Tool::new(
            GET_REQUEST_ATTACHMENT,
            "Read the content of one attachment of a request: a text file in pages of at most 256 KiB, an image of at most 5 MiB as image content, and for any other file only its name, size and media type.",
            schema_for_type::<GetRequestAttachmentInput>(),
        ),
        Tool::new(
            CLAIM_REQUEST,
            "As the main agent, or as an area lead when the request concerns your area, take a request to draft: sort it as bug, feature or question and give it a title.",
            schema_for_type::<ClaimRequestInput>(),
        ),
        Tool::new(
            RELEASE_REQUEST,
            "Give up drafting a request, so another orchestrator can claim it.",
            schema_for_type::<RequestIdInput>(),
        ),
        Tool::new(
            SUBMIT_REQUEST_DRAFT,
            "As the drafter, send the next draft version: the cards in the task template and the dependency edges between them. The requester then approves it, asks for changes or edits it.",
            schema_for_type::<SubmitRequestDraftInput>(),
        ),
        Tool::new(
            REQUEST_DRAFT_CHANGES,
            "As the requester, send the latest draft version back to the drafter with a comment.",
            schema_for_type::<RequestDraftChangesInput>(),
        ),
        Tool::new(
            EDIT_REQUEST_DRAFT,
            "As the requester, write the next draft version yourself instead of asking for changes.",
            schema_for_type::<EditRequestDraftInput>(),
        ),
        Tool::new(
            APPROVE_REQUEST_DRAFT,
            "As the requester, approve the latest draft version: each card becomes a task and each edge a dependency, in one step.",
            schema_for_type::<ApproveRequestDraftInput>(),
        ),
        Tool::new(
            CLOSE_REQUEST,
            "As the requester, close a request with a reason; it then only reads.",
            schema_for_type::<CloseRequestInput>(),
        ),
    ]
}

/// Runs a request tool acting as `actor`. `None` when `tool` is not a request tool.
pub async fn run(
    server: &FoldServer,
    tool: &str,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let value = match tool {
        CREATE_REQUEST => create_request(server, actor, arguments).await?,
        LIST_REQUESTS => list_requests(server, actor, arguments).await?,
        GET_REQUEST => get_request(server, actor, arguments).await?,
        CLAIM_REQUEST => claim_request(server, actor, arguments).await?,
        RELEASE_REQUEST => release_request(server, actor, arguments).await?,
        SUBMIT_REQUEST_DRAFT => submit_request_draft(server, actor, arguments).await?,
        REQUEST_DRAFT_CHANGES => request_draft_changes(server, actor, arguments).await?,
        EDIT_REQUEST_DRAFT => edit_request_draft(server, actor, arguments).await?,
        APPROVE_REQUEST_DRAFT => approve_request_draft(server, actor, arguments).await?,
        CLOSE_REQUEST => close_request(server, actor, arguments).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn request_id(text: &str) -> Result<RequestId, FoldError> {
    Ok(text.parse::<RequestId>()?)
}

fn kind(text: &str) -> Result<RequestKind, FoldError> {
    RequestKind::parse(text).ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "the kind is not a known request kind",
        )
        .with_details([("reason", json!("kind"))])
        .with_hint(
            "give kind as bug, feature or question, or call list_requests to see the kinds in use",
        )
    })
}

fn state(text: &str) -> Result<RequestState, FoldError> {
    RequestState::parse(text).ok_or_else(|| {
        FoldError::new(codes::INVALID_REQUEST, "the state is not a known request state")
        .with_details([("reason", json!("state"))])
        .with_hint(
            "give state as open, drafting, draft_ready, changes_requested, approved or closed; call list_requests to see each request with its state",
        )
    })
}

fn draft(
    cards: Vec<CardInput>,
    edges: Vec<EdgeInput>,
    note: Option<String>,
) -> Result<SubmitDraft, FoldError> {
    Ok(SubmitDraft {
        cards: cards
            .into_iter()
            .map(|card| {
                Ok(DraftCard {
                    key: card.key,
                    title: card.title,
                    description: card.description,
                    label: card.label.as_deref().map(LabelName::parse).transpose()?,
                })
            })
            .collect::<Result<_, FoldError>>()?,
        edges: edges
            .into_iter()
            .map(|edge| DraftEdge {
                prerequisite: edge.prerequisite,
                dependent: edge.dependent,
            })
            .collect(),
        note,
    })
}

/// A card names its label only when it has one.
fn card_json(card: &DraftCard) -> Value {
    let mut value = json!({
        "key": card.key,
        "title": card.title,
        "description": card.description,
    });
    if let Some(label) = &card.label {
        value["label"] = json!(label.as_str());
    }
    value
}

/// The compact line of a request in a list.
fn summary_json(view: &RequestView) -> Value {
    json!({
        "id": view.id.to_string(),
        "state": view.state.as_str(),
        "kind": view.kind.map(RequestKind::as_str),
        "title": view.title,
        "excerpt": view.excerpt,
        "requester": view.requester.label(),
        "drafter": view.drafter.as_ref().map(ToString::to_string),
        "latest_version": view.latest_version,
    })
}

/// The state a command left the request in.
fn state_json(view: &RequestView) -> Value {
    json!({
        "id": view.id.to_string(),
        "state": view.state.as_str(),
        "version": view.latest_version,
    })
}

fn detail_json(detail: &RequestDetail, version: Option<u32>) -> Result<Value, FoldError> {
    let request = &detail.request;
    let shown = match version {
        Some(version) => Some(
            detail
                .drafts
                .iter()
                .find(|draft| draft.version == version)
                .ok_or_else(|| {
                    FoldError::new(
                        codes::NOT_FOUND,
                        format!("{} has no draft version {version}", request.id),
                    )
                    .with_details([("version", json!(version))])
                    .with_hint("call get_request without version to read the latest")
                })?,
        ),
        None => detail.drafts.last(),
    };
    Ok(json!({
        "id": request.id.to_string(),
        "state": request.state.as_str(),
        "kind": request.kind.map(RequestKind::as_str),
        "title": request.title,
        "body": request.body,
        "requester": request.requester.label(),
        "drafter": request.drafter.as_ref().map(ToString::to_string),
        "close_reason": request.close_reason,
        "attachments": detail.attachments.iter().map(|attachment| json!({
            "position": attachment.position,
            "name": attachment.name,
            "media_type": attachment.media_type,
            "size": attachment.size,
        })).collect::<Vec<_>>(),
        "versions": detail.drafts.iter().map(|draft| draft.version).collect::<Vec<_>>(),
        "draft": shown.map(|draft| json!({
            "version": draft.version,
            "author": draft.author,
            "note": draft.note,
            "cards": draft.cards.iter().map(card_json).collect::<Vec<_>>(),
            "edges": draft.edges.iter().map(|edge| json!({
                "prerequisite": edge.prerequisite,
                "dependent": edge.dependent,
            })).collect::<Vec<_>>(),
        })),
        "comments": detail.comments.iter().map(|comment| json!({
            "version": comment.version,
            "author": comment.author,
            "body": comment.body,
        })).collect::<Vec<_>>(),
        "code": request.code.as_ref().map(code_json),
        "thread": detail.thread.map(|thread| thread.id.to_string()),
        "tasks": detail.tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
    }))
}

/// The lines the requester selected, with the anchor that names the tree they were read from.
fn code_json(code: &CodeRef) -> Value {
    let mut value = json!({
        "path": code.path,
        "start_line": code.start_line,
        "end_line": code.end_line,
        "excerpt": code.excerpt,
        "excerpt_cut": code.excerpt_cut,
    });
    match code.anchor {
        CodeAnchor::Revision(revision) => value["revision"] = json!(revision.to_string()),
        CodeAnchor::Version {
            proposal,
            version,
            side,
        } => {
            value["proposal"] = json!(proposal.to_string());
            value["version"] = json!(version);
            value["side"] = json!(side.as_str());
        }
    }
    value
}

async fn create_request(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CreateRequestInput = parse(arguments)?;
    let ws = match input.ws.as_deref() {
        Some(ws) if !input.attachments.is_empty() => Some(ws.parse::<WorkspaceId>()?),
        _ => None,
    };
    let attachments = server
        .workspaces
        .read_own_files(actor, ws, &input.attachments, MAX_ATTACHMENT_BYTES)
        .await
        .map_err(from_workspace)?
        .into_iter()
        .map(|file| NewRequestAttachment::named(file.name, file.bytes))
        .collect();
    let view = server
        .tasks
        .create_request(actor, &input.body, attachments)
        .await
        .map_err(from_task)?;
    Ok(json!({
        "id": view.id.to_string(),
        "state": view.state.as_str(),
        "attachments": input.attachments.len(),
    }))
}

async fn list_requests(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ListRequestsInput = parse(arguments)?;
    let filter = RequestFilter {
        state: input.state.as_deref().map(state).transpose()?,
        kind: input.kind.as_deref().map(kind).transpose()?,
        requester: if input.mine {
            Requester::of(actor)
        } else {
            None
        },
        limit: Some(input.limit.unwrap_or(DEFAULT_REQUEST_LIST)),
    };
    let requests = server
        .tasks
        .list_requests(&filter)
        .await
        .map_err(from_task)?;
    Ok(json!({"requests": requests.iter().map(summary_json).collect::<Vec<_>>()}))
}

async fn get_request(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: GetRequestInput = parse(arguments)?;
    let detail = server
        .tasks
        .read_request(actor, request_id(&input.id)?)
        .await
        .map_err(from_task)?;
    detail_json(&detail, input.version)
}

/// The longest page of a text attachment, so one result stays small in an agent's context.
const TEXT_PAGE_BYTES: usize = 256 * 1024;
/// The largest image sent as image content; a larger one gets the answer of a binary file.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The image types an agent's model reads as image content. SVG is text.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/// Types outside `text/` whose files are text.
const TEXT_TYPES: [&str; 8] = [
    "application/json",
    "application/xml",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/javascript",
    "application/x-sh",
    "image/svg+xml",
];

/// How an attachment reaches an agent.
#[derive(Debug, PartialEq, Eq)]
enum AttachmentContent {
    /// One page of a text file: the bytes from `offset` up to `next_offset`, or to the end when that is `None`.
    Text {
        text: String,
        offset: usize,
        next_offset: Option<usize>,
    },
    /// An image in base64, with its media type without parameters.
    Image { data: String, mime: String },
    /// A file the agent's model cannot read, or an image over [`MAX_IMAGE_BYTES`].
    Binary,
}

/// The image type that the file's signature shows, because the uploader names the media type and a model refuses image content whose bytes are not that image.
fn sniffed_image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The media type without parameters, in lower case.
fn base_type(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// A text-like type, or `application/octet-stream`, which says only that the uploader did not know the type, holds text when its bytes are UTF-8 without a NUL.
fn is_text_type(base: &str) -> bool {
    base.starts_with("text/")
        || base.ends_with("+json")
        || base.ends_with("+xml")
        || TEXT_TYPES.contains(&base)
        || base == "application/octet-stream"
}

/// Classifies and cuts the bytes of an attachment. `offset` picks the page of a text file and must be a character boundary inside it; other files ignore it.
fn attachment_content(
    media_type: &str,
    bytes: &[u8],
    offset: Option<u64>,
) -> Result<AttachmentContent, FoldError> {
    let base = base_type(media_type);
    if IMAGE_TYPES.contains(&base.as_str()) {
        return Ok(match sniffed_image_type(bytes) {
            Some(mime) if bytes.len() <= MAX_IMAGE_BYTES => AttachmentContent::Image {
                data: STANDARD.encode(bytes),
                mime: mime.to_owned(),
            },
            _ => AttachmentContent::Binary,
        });
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) if is_text_type(&base) && !text.contains('\0') => text,
        _ => return Ok(AttachmentContent::Binary),
    };
    let start = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "the offset is not the start of a page of this attachment",
        )
        .with_details([("offset", json!(offset)), ("size", json!(text.len()))])
        .with_hint("call get_request_attachment without offset, or with the next_offset of the page before"));
    }
    let mut end = start.saturating_add(TEXT_PAGE_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(AttachmentContent::Text {
        text: text[start..end].to_owned(),
        offset: start,
        next_offset: (end < text.len()).then_some(end),
    })
}

/// The content of one attachment: the JSON part names the attachment and, for text, holds the page; an image follows as image content.
pub(crate) async fn get_request_attachment(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Reply, FoldError> {
    let input: GetRequestAttachmentInput = parse(arguments)?;
    let request = request_id(&input.id)?;
    let (attachment, bytes) = server
        .tasks
        .read_request_attachment(actor, request, input.n)
        .await
        .map_err(from_task)?;
    let media_type = attachment.media_type.clone();
    let offset = input.offset;
    let content = blocking(move || attachment_content(&media_type, &bytes, offset)).await?;
    Ok(attachment_reply(request, &attachment, content))
}

fn attachment_reply(
    request: RequestId,
    attachment: &RequestAttachment,
    content: AttachmentContent,
) -> Reply {
    let mut value = json!({
        "request": request.to_string(),
        "n": attachment.position,
        "name": attachment.name,
        "media_type": attachment.media_type,
        "size": attachment.size,
    });
    let mut blocks = Vec::new();
    match content {
        AttachmentContent::Text {
            text,
            offset,
            next_offset,
        } => {
            value["content"] = json!("text");
            value["offset"] = json!(offset);
            value["next_offset"] = json!(next_offset);
            value["text"] = json!(text);
        }
        AttachmentContent::Image { data, mime } => {
            value["content"] = json!("image");
            blocks.push(ContentBlock::image(data, mime));
        }
        AttachmentContent::Binary => {
            value["content"] = json!("none");
            value["reason"] = json!(
                "fold sends only text and images of at most 5 MiB (png, jpeg, gif, webp) to an agent"
            );
        }
    }
    Reply::with_content(value, blocks)
}

async fn claim_request(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ClaimRequestInput = parse(arguments)?;
    let view = server
        .tasks
        .claim_request(
            actor,
            request_id(&input.id)?,
            kind(&input.kind)?,
            &input.title,
        )
        .await
        .map_err(from_task)?;
    Ok(json!({
        "id": view.id.to_string(),
        "state": view.state.as_str(),
        "kind": view.kind.map(RequestKind::as_str),
        "title": view.title,
        "drafter": view.drafter.as_ref().map(ToString::to_string),
    }))
}

async fn release_request(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RequestIdInput = parse(arguments)?;
    let view = server
        .tasks
        .release_request(actor, request_id(&input.id)?)
        .await
        .map_err(from_task)?;
    Ok(json!({
        "id": view.id.to_string(),
        "state": view.state.as_str(),
        "drafter": view.drafter.as_ref().map(ToString::to_string),
    }))
}

async fn submit_request_draft(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SubmitRequestDraftInput = parse(arguments)?;
    let kind = input.kind.as_deref().map(kind).transpose()?;
    let view = server
        .tasks
        .submit_request_draft(
            actor,
            request_id(&input.id)?,
            draft(input.cards, input.edges, input.note)?,
            kind,
            input.title.as_deref(),
        )
        .await
        .map_err(from_task)?;
    Ok(state_json(&view))
}

async fn request_draft_changes(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RequestDraftChangesInput = parse(arguments)?;
    let view = server
        .tasks
        .request_draft_changes(actor, request_id(&input.id)?, input.version, &input.comment)
        .await
        .map_err(from_task)?;
    Ok(state_json(&view))
}

async fn edit_request_draft(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: EditRequestDraftInput = parse(arguments)?;
    let view = server
        .tasks
        .edit_request_draft(
            actor,
            request_id(&input.id)?,
            draft(input.cards, input.edges, input.note)?,
        )
        .await
        .map_err(from_task)?;
    Ok(state_json(&view))
}

async fn approve_request_draft(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ApproveRequestDraftInput = parse(arguments)?;
    let approved = server
        .tasks
        .approve_request_draft(actor, request_id(&input.id)?, input.expected_version)
        .await
        .map_err(from_task)?;
    Ok(json!({
        "id": approved.request.id.to_string(),
        "state": approved.request.state.as_str(),
        "tasks": approved.tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
    }))
}

async fn close_request(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CloseRequestInput = parse(arguments)?;
    let view = server
        .tasks
        .close_request(actor, request_id(&input.id)?, &input.reason)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id.to_string(), "state": view.state.as_str()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_page(content: AttachmentContent) -> (String, usize, Option<usize>) {
        match content {
            AttachmentContent::Text {
                text,
                offset,
                next_offset,
            } => (text, offset, next_offset),
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn a_small_text_file_is_one_last_page() {
        let content = attachment_content("text/plain; charset=utf-8", b"hello", None).unwrap();
        assert_eq!(text_page(content), ("hello".to_owned(), 0, None));
    }

    #[test]
    fn a_page_never_cuts_a_multi_byte_character() {
        // Two ASCII bytes, then three-byte characters, so the page limit falls inside a character.
        let text = format!("ab{}", "€".repeat(TEXT_PAGE_BYTES / 3 + 10));
        let (first, offset, next) =
            text_page(attachment_content("text/markdown", text.as_bytes(), None).unwrap());
        assert_eq!(offset, 0);
        let next = next.unwrap();
        assert!(next <= TEXT_PAGE_BYTES);
        assert_eq!(next, TEXT_PAGE_BYTES - 2);
        assert_eq!(first.len(), next);
        assert!(text.is_char_boundary(next));

        let (rest, offset, last) = text_page(
            attachment_content("text/markdown", text.as_bytes(), Some(next as u64)).unwrap(),
        );
        assert_eq!(offset, next);
        assert_eq!(last, None);
        assert_eq!(format!("{first}{rest}"), text);
    }

    #[test]
    fn the_last_page_has_no_next_offset_and_an_offset_at_the_end_is_empty() {
        let text = "x".repeat(TEXT_PAGE_BYTES + 5);
        let (_, _, next) =
            text_page(attachment_content("text/x-log", text.as_bytes(), None).unwrap());
        assert_eq!(next, Some(TEXT_PAGE_BYTES));
        let (last, _, none) = text_page(
            attachment_content("text/x-log", text.as_bytes(), Some(TEXT_PAGE_BYTES as u64))
                .unwrap(),
        );
        assert_eq!(last, "xxxxx");
        assert_eq!(none, None);
        let (empty, _, none) = text_page(
            attachment_content("text/x-log", text.as_bytes(), Some(text.len() as u64)).unwrap(),
        );
        assert_eq!(empty, "");
        assert_eq!(none, None);
    }

    #[test]
    fn an_offset_inside_a_character_or_past_the_end_is_refused() {
        for offset in [1, 99] {
            let error =
                attachment_content("text/plain", "€€".as_bytes(), Some(offset)).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_REQUEST);
            assert!(error.hint().unwrap().contains("get_request_attachment"));
        }
    }

    #[test]
    fn text_like_and_unknown_types_with_utf8_bytes_are_text() {
        for media_type in [
            "application/json",
            "application/vnd.api+json",
            "image/svg+xml",
            "application/octet-stream",
            "TEXT/CSV",
        ] {
            let content = attachment_content(media_type, b"{}", None).unwrap();
            assert!(
                matches!(content, AttachmentContent::Text { .. }),
                "{media_type}"
            );
        }
    }

    #[test]
    fn bytes_that_are_not_text_are_binary_whatever_the_type() {
        let invalid = attachment_content("text/plain", &[0xff, 0xfe], None).unwrap();
        assert_eq!(invalid, AttachmentContent::Binary);
        let nul = attachment_content("text/plain", b"a\0b", None).unwrap();
        assert_eq!(nul, AttachmentContent::Binary);
        let pdf = attachment_content("application/pdf", b"%PDF", None).unwrap();
        assert_eq!(pdf, AttachmentContent::Binary);
    }

    #[test]
    fn an_image_is_base64_up_to_its_limit_and_binary_above() {
        let png = b"\x89PNG\r\n\x1a\n";
        let content = attachment_content("image/PNG", png, Some(7)).unwrap();
        assert_eq!(
            content,
            AttachmentContent::Image {
                data: STANDARD.encode(png),
                mime: "image/png".to_owned(),
            }
        );
        let mut large = vec![0xff, 0xd8, 0xff];
        large.resize(MAX_IMAGE_BYTES + 1, 0);
        assert_eq!(
            attachment_content("image/jpeg", &large, None).unwrap(),
            AttachmentContent::Binary
        );
    }

    #[test]
    fn an_image_takes_the_type_of_its_signature_and_other_bytes_are_binary() {
        let gif = attachment_content("image/png", b"GIF89a\x01\x00", None).unwrap();
        assert!(matches!(gif, AttachmentContent::Image { ref mime, .. } if mime == "image/gif"));
        let webp = attachment_content("image/jpeg", b"RIFF\x04\x00\x00\x00WEBPVP8 ", None).unwrap();
        assert!(matches!(webp, AttachmentContent::Image { ref mime, .. } if mime == "image/webp"));
        let jpeg = attachment_content("image/webp", &[0xff, 0xd8, 0xff, 0xe0], None).unwrap();
        assert!(matches!(jpeg, AttachmentContent::Image { ref mime, .. } if mime == "image/jpeg"));
        for bytes in [&b"not an image"[..], b"RIFF\x04\x00\x00\x00WAVE", b""] {
            assert_eq!(
                attachment_content("image/png", bytes, None).unwrap(),
                AttachmentContent::Binary
            );
        }
    }
}

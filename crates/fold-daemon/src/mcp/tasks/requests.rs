use fold_core::requests::{
    DraftCard, DraftEdge, MAX_ATTACHMENT_BYTES, RequestKind, RequestState, Requester,
};
use fold_core::{Actor, FoldError, RequestId, WorkspaceId, codes};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::FoldServer;
use super::super::error::{from_task, from_workspace};
use super::super::identity::parse;
use crate::services::tasks::{
    NewRequestAttachment, RequestDetail, RequestFilter, RequestView, SubmitDraft,
};

pub const CREATE_REQUEST: &str = "create_request";
pub const LIST_REQUESTS: &str = "list_requests";
pub const GET_REQUEST: &str = "get_request";
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetRequestInput {
    /// The request, such as RQ-4.
    pub id: String,
    /// The draft version to read. Leave it out for the latest.
    #[serde(default)]
    pub version: Option<u32>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RequestIdInput {
    /// The request, such as RQ-4.
    pub id: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ApproveRequestDraftInput {
    /// The request you created, such as RQ-4.
    pub id: String,
    /// The draft version you read and approve; an older one is refused with stale_version.
    pub expected_version: u32,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CloseRequestInput {
    /// The request you created, such as RQ-4.
    pub id: String,
    /// Why you close it. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub reason: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            CREATE_REQUEST,
            "Ask for work: a bug, a feature or a question, written as a message with optional files from your workspace. An orchestrator sorts and names it, then drafts the cards.",
            schema_for_type::<CreateRequestInput>(),
        ),
        Tool::new(
            LIST_REQUESTS,
            "List the requests, newest first, with their state, kind, title or the start of the message, and drafter.",
            schema_for_type::<ListRequestsInput>(),
        ),
        Tool::new(
            GET_REQUEST,
            "Read one request: the message, the attachments, one draft version with its cards and edges, the requester's comments, the clarification thread and the tasks an approval created.",
            schema_for_type::<GetRequestInput>(),
        ),
        Tool::new(
            CLAIM_REQUEST,
            "As an orchestrator (an agent's main worker), take a request to draft: sort it as bug, feature or question and give it a title.",
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

pub fn is_request_tool(tool: &str) -> bool {
    tools().iter().any(|known| known.name == tool)
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
        GET_REQUEST => get_request(server, arguments).await?,
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
            format!("{text} is not a request kind"),
        )
        .with_details([("reason", json!("kind"))])
        .with_hint("give kind as bug, feature or question")
    })
}

fn state(text: &str) -> Result<RequestState, FoldError> {
    RequestState::parse(text).ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("{text} is not a request state"),
        )
        .with_details([("reason", json!("state"))])
        .with_hint(
            "give state as open, drafting, draft_ready, changes_requested, approved or closed",
        )
    })
}

fn draft(cards: Vec<CardInput>, edges: Vec<EdgeInput>, note: Option<String>) -> SubmitDraft {
    SubmitDraft {
        cards: cards
            .into_iter()
            .map(|card| DraftCard {
                key: card.key,
                title: card.title,
                description: card.description,
            })
            .collect(),
        edges: edges
            .into_iter()
            .map(|edge| DraftEdge {
                prerequisite: edge.prerequisite,
                dependent: edge.dependent,
            })
            .collect(),
        note,
    }
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
            "cards": draft.cards.iter().map(|card| json!({
                "key": card.key,
                "title": card.title,
                "description": card.description,
            })).collect::<Vec<_>>(),
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
        "thread": detail.thread.map(|thread| thread.id.to_string()),
        "tasks": detail.tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
    }))
}

async fn create_request(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CreateRequestInput = parse(arguments)?;
    let attachments = if input.attachments.is_empty() {
        Vec::new()
    } else {
        let ws = input.ws.as_deref().ok_or_else(|| {
            FoldError::new(codes::INVALID_REQUEST, "attachments need your workspace ws")
                .with_details([("reason", json!("attachments_need_ws"))])
                .with_hint("call create_request again with ws, the workspace that holds the files")
        })?;
        let ws: WorkspaceId = ws.parse()?;
        server
            .workspaces
            .read_own_files(actor, ws, &input.attachments, MAX_ATTACHMENT_BYTES)
            .await
            .map_err(from_workspace)?
            .into_iter()
            .map(|file| NewRequestAttachment::named(file.name, file.bytes))
            .collect()
    };
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
    };
    let requests = server
        .tasks
        .list_requests(&filter)
        .await
        .map_err(from_task)?;
    Ok(json!({"requests": requests.iter().map(summary_json).collect::<Vec<_>>()}))
}

async fn get_request(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let input: GetRequestInput = parse(arguments)?;
    let detail = server
        .tasks
        .get_request(request_id(&input.id)?)
        .await
        .map_err(from_task)?;
    detail_json(&detail, input.version)
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
            draft(input.cards, input.edges, input.note),
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
            draft(input.cards, input.edges, input.note),
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

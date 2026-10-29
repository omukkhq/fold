use axum::Json;
use axum::extract::multipart::{Field, Multipart, MultipartError, MultipartRejection};
use axum::extract::{DefaultBodyLimit, Path};
use axum::http::header::{
    CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use fold_core::requests::{
    CodeAnchor, CodeRef, CodeSelection, DraftCard, DraftEdge, MAX_ATTACHMENT_BYTES,
    MAX_ATTACHMENTS, Requester, attachment_too_large, check_attachment_count, valid_media_type,
};
use fold_core::{FoldError, LabelName, ProposalId, RequestId, RevisionId, codes};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ApiQuery, ErrorEnvelope};
use super::super::reviews::CommentSide;
use crate::daemon::AppState;
use crate::services::tasks::{self as service, NewRequestAttachment, SubmitDraft};

/// The largest body `POST /requests` reads: every attachment at its limit and room for the message and the form framing.
const MAX_CREATE_BODY: usize = MAX_ATTACHMENTS * MAX_ATTACHMENT_BYTES as usize + 1024 * 1024;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_requests, create_request))
        .layer(DefaultBodyLimit::max(MAX_CREATE_BODY))
        .routes(routes!(create_code_request))
        .routes(routes!(get_request))
        .routes(routes!(get_request_attachment))
        .routes(routes!(request_draft_changes))
        .routes(routes!(edit_request_draft))
        .routes(routes!(approve_request_draft))
        .routes(routes!(close_request))
}

/// What a request asks for, set by the orchestrator that claims it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Bug,
    Feature,
    Question,
}

impl From<fold_core::requests::RequestKind> for RequestKind {
    fn from(kind: fold_core::requests::RequestKind) -> Self {
        match kind {
            fold_core::requests::RequestKind::Bug => Self::Bug,
            fold_core::requests::RequestKind::Feature => Self::Feature,
            fold_core::requests::RequestKind::Question => Self::Question,
        }
    }
}

/// Where a request stands. `approved` and `closed` are final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Open,
    Drafting,
    DraftReady,
    ChangesRequested,
    Approved,
    Closed,
}

impl From<fold_core::requests::RequestState> for RequestState {
    fn from(state: fold_core::requests::RequestState) -> Self {
        use fold_core::requests::RequestState as Core;
        match state {
            Core::Open => Self::Open,
            Core::Drafting => Self::Drafting,
            Core::DraftReady => Self::DraftReady,
            Core::ChangesRequested => Self::ChangesRequested,
            Core::Approved => Self::Approved,
            Core::Closed => Self::Closed,
        }
    }
}

/// One request in a list.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestSummary {
    #[schema(example = "RQ-4")]
    pub id: String,
    /// Null until an orchestrator claims and sorts the request.
    #[schema(required = true)]
    pub kind: Option<RequestKind>,
    /// Null until an orchestrator claims and names the request; show `excerpt` instead.
    #[schema(required = true)]
    pub title: Option<String>,
    /// The first line of the message, cut to about 80 characters.
    pub excerpt: String,
    pub state: RequestState,
    /// An agent name, or `human:<name>`.
    #[schema(example = "human:ana")]
    pub requester: String,
    /// The orchestrator worker drafting the request.
    #[schema(required = true, example = "frontend")]
    pub drafter: Option<String>,
    /// The newest draft version; null before the first draft.
    #[schema(required = true)]
    pub latest_version: Option<u32>,
    /// The code the request was made on; null for a request written without a selection.
    #[schema(required = true)]
    pub code: Option<RequestCodeView>,
    /// UTC milliseconds.
    pub created_at: i64,
    /// UTC milliseconds.
    pub updated_at: i64,
}

/// The lines a request was made on: a run of lines of one file in a revision of main, or in one side of a proposal version.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestCodeView {
    #[schema(example = "web/src/board.ts")]
    pub path: String,
    /// 1-based.
    pub start_line: u32,
    /// 1-based, at least `start_line`.
    pub end_line: u32,
    /// The lines as the tree held them when the request was made, joined by newlines.
    pub excerpt: String,
    /// The excerpt holds only the first of the lines.
    pub excerpt_cut: bool,
    /// The revision of main the lines belong to; null for lines of a proposal version.
    #[schema(required = true, example = "r17")]
    pub revision: Option<String>,
    /// The proposal whose version the lines belong to; null for lines of a revision.
    #[schema(required = true, example = "P-3")]
    pub proposal: Option<String>,
    /// The proposal version; null for lines of a revision.
    #[schema(required = true)]
    pub version: Option<u32>,
    /// The side of the version: `old` is its base, `new` the version itself; null for lines of a revision.
    #[schema(required = true)]
    pub side: Option<CommentSide>,
}

impl From<CodeRef> for RequestCodeView {
    fn from(code: CodeRef) -> Self {
        let (revision, proposal, version, side) = match code.anchor {
            CodeAnchor::Revision(revision) => (Some(revision.to_string()), None, None, None),
            CodeAnchor::Version {
                proposal,
                version,
                side,
            } => (
                None,
                Some(proposal.to_string()),
                Some(version),
                Some(side.into()),
            ),
        };
        Self {
            path: code.path,
            start_line: code.start_line,
            end_line: code.end_line,
            excerpt: code.excerpt,
            excerpt_cut: code.excerpt_cut,
            revision,
            proposal,
            version,
            side,
        }
    }
}

impl From<service::RequestView> for RequestSummary {
    fn from(view: service::RequestView) -> Self {
        Self {
            id: view.id.to_string(),
            kind: view.kind.map(Into::into),
            title: view.title,
            excerpt: view.excerpt,
            state: view.state.into(),
            requester: view.requester.label(),
            drafter: view.drafter.map(|worker| worker.to_string()),
            latest_version: view.latest_version,
            code: view.code.map(Into::into),
            created_at: view.created_at.as_millis(),
            updated_at: view.updated_at.as_millis(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RequestList {
    /// Newest first.
    pub items: Vec<RequestSummary>,
}

/// A file dropped on the request; its bytes come from the attachment route.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestAttachmentView {
    /// 1-based; the `{n}` of the attachment route.
    pub position: u32,
    pub name: String,
    #[schema(example = "image/png")]
    pub media_type: String,
    /// Bytes.
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DraftCardView {
    /// Local to the draft; edges name cards by it.
    #[schema(example = "c1")]
    pub key: String,
    pub title: String,
    /// Markdown in the task template.
    pub description: String,
    /// The label of the area whose lead builds the card; approval gives the task this label. Absent for a card the main agent routes itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "backend")]
    pub label: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DraftEdgeView {
    /// The key of the card that must be done first.
    #[schema(example = "c1")]
    pub prerequisite: String,
    /// The key of the card that needs it.
    #[schema(example = "c2")]
    pub dependent: String,
}

/// One draft version; versions never change.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestDraftView {
    pub version: u32,
    /// The drafter's worker or the requester, such as `frontend` or `human:ana`.
    pub author: String,
    #[schema(required = true)]
    pub note: Option<String>,
    /// UTC milliseconds.
    pub created_at: i64,
    pub cards: Vec<DraftCardView>,
    pub edges: Vec<DraftEdgeView>,
}

/// The requester's comment on one version, written when asking for changes.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestCommentView {
    pub version: u32,
    pub author: String,
    pub body: String,
    /// UTC milliseconds.
    pub created_at: i64,
}

/// The request's clarification thread at a glance; read it through the thread route.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestThreadSummary {
    /// The id of the thread's first message.
    #[schema(example = "m-31")]
    pub id: String,
    pub messages: u32,
    /// UTC milliseconds of the newest message.
    pub last_at: i64,
}

/// A request with its attachments, every draft version, the comments, the clarification thread and the tasks its approval created.
#[derive(Debug, Serialize, ToSchema)]
pub struct RequestDetailView {
    pub request: RequestSummary,
    /// The message in Markdown, as the requester wrote it. `#` names a fold item and `@` names a file.
    pub body: String,
    /// Set once the request is closed.
    #[schema(required = true)]
    pub close_reason: Option<String>,
    pub attachments: Vec<RequestAttachmentView>,
    /// Oldest first.
    pub drafts: Vec<RequestDraftView>,
    pub comments: Vec<RequestCommentView>,
    /// Null before anyone wrote about the request.
    #[schema(required = true)]
    pub thread: Option<RequestThreadSummary>,
    /// The tasks the approval created, in card order.
    pub tasks: Vec<String>,
}

impl From<service::RequestDetail> for RequestDetailView {
    fn from(detail: service::RequestDetail) -> Self {
        let body = detail.request.body.clone();
        let close_reason = detail.request.close_reason.clone();
        Self {
            request: detail.request.into(),
            body,
            close_reason,
            attachments: detail
                .attachments
                .into_iter()
                .map(|attachment| RequestAttachmentView {
                    position: attachment.position,
                    name: attachment.name,
                    media_type: attachment.media_type,
                    size: attachment.size,
                })
                .collect(),
            drafts: detail
                .drafts
                .into_iter()
                .map(|draft| RequestDraftView {
                    version: draft.version,
                    author: draft.author,
                    note: draft.note,
                    created_at: draft.created_at.as_millis(),
                    cards: draft
                        .cards
                        .into_iter()
                        .map(|card| DraftCardView {
                            key: card.key,
                            title: card.title,
                            description: card.description,
                            label: card.label.map(|label| label.as_str().to_owned()),
                        })
                        .collect(),
                    edges: draft
                        .edges
                        .into_iter()
                        .map(|edge| DraftEdgeView {
                            prerequisite: edge.prerequisite,
                            dependent: edge.dependent,
                        })
                        .collect(),
                })
                .collect(),
            comments: detail
                .comments
                .into_iter()
                .map(|comment| RequestCommentView {
                    version: comment.version,
                    author: comment.author,
                    body: comment.body,
                    created_at: comment.created_at.as_millis(),
                })
                .collect(),
            thread: detail.thread.map(|thread| RequestThreadSummary {
                id: thread.id.to_string(),
                messages: thread.messages,
                last_at: thread.last_at.as_millis(),
            }),
            tasks: detail.tasks.iter().map(ToString::to_string).collect(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListRequestsParams {
    /// Only requests in this state.
    pub state: Option<String>,
    /// Only requests of this kind: `bug`, `feature` or `question`.
    pub kind: Option<String>,
    /// Only requests of this requester: an agent name or `human:<name>`.
    pub requester: Option<String>,
}

/// The multipart form of `POST /requests`.
#[derive(Debug, ToSchema)]
#[expect(
    dead_code,
    reason = "declares the form for the OpenAPI document; the handler reads the parts"
)]
pub struct CreateRequestForm {
    /// The message in Markdown. `#` names a fold item and `@` names a file; the daemon parses nothing.
    body: String,
    /// Up to 10 files of at most 10 MiB each, each with a file name; a part without a content type gets one from its name.
    #[schema(value_type = Vec<String>, format = Binary)]
    files: Vec<Vec<u8>>,
}

/// A request about lines a human selected. Name the lines by `revision`, or by `proposal`, `version` and `side`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CodeRequestBody {
    /// The message in Markdown. `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub body: String,
    /// The file the lines belong to, relative to the tree's root.
    #[schema(example = "web/src/board.ts")]
    pub path: String,
    /// 1-based.
    pub start_line: u32,
    /// 1-based, at least `start_line`.
    pub end_line: u32,
    /// A revision of main, such as `r17`.
    #[serde(default)]
    pub revision: Option<String>,
    /// A proposal, such as `P-3`, with `version` and `side`.
    #[serde(default)]
    pub proposal: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub side: Option<CommentSide>,
}

impl CodeRequestBody {
    fn selection(self) -> Result<(String, CodeSelection), ApiError> {
        let anchor = match (self.revision, self.proposal, self.version, self.side) {
            (Some(revision), None, None, None) => {
                CodeAnchor::Revision(revision.parse::<RevisionId>().map_err(FoldError::from)?)
            }
            (None, Some(proposal), Some(version), Some(side)) => CodeAnchor::Version {
                proposal: proposal.parse::<ProposalId>().map_err(FoldError::from)?,
                version,
                side: side.into(),
            },
            _ => {
                return Err(ApiError::invalid_request(
                    "name the lines by revision, or by proposal, version and side",
                ));
            }
        };
        Ok((
            self.body,
            CodeSelection {
                anchor,
                path: self.path,
                start_line: self.start_line,
                end_line: self.end_line,
            },
        ))
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DraftChangesBody {
    /// The draft version the requester read, which must be the latest.
    pub version: u32,
    /// What the drafter must change. `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub comment: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EditDraftBody {
    /// 1 to 20 cards.
    pub cards: Vec<DraftCardView>,
    #[serde(default)]
    pub edges: Vec<DraftEdgeView>,
    /// A note about this version. `#` names a fold item and `@` names a file; the daemon parses nothing.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApproveDraftBody {
    /// The draft version the requester read and approves; an older one is `stale_version`.
    pub expected_version: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CloseRequestBody {
    /// Why the request is closed. `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub reason: String,
}

fn request_id(text: &str) -> Result<RequestId, ApiError> {
    Ok(text.parse::<RequestId>().map_err(FoldError::from)?)
}

/// The message names the filter and never repeats its value.
fn invalid_filter(name: &str) -> ApiError {
    ApiError(
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("the {name} filter is not a known request {name}"),
        )
        .with_details([("reason", json!(name))]),
    )
}

async fn detail(project: &Project, request: RequestId) -> Result<RequestDetailView, ApiError> {
    Ok(project.0.tasks.get_request(request).await?.into())
}

/// The project's requests, newest first. Reading needs no human name.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/requests",
    operation_id = "list_requests",
    params(("slug" = String, Path, description = "The project slug"), ListRequestsParams),
    responses(
        (status = 200, description = "The requests, newest first", body = RequestList),
        (status = 400, description = "`invalid_request`: a filter is not a state, a kind or a requester", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_requests(
    project: Project,
    ApiQuery(params): ApiQuery<ListRequestsParams>,
) -> Result<Json<RequestList>, ApiError> {
    let filter = service::RequestFilter {
        state: params
            .state
            .as_deref()
            .map(|text| {
                fold_core::requests::RequestState::parse(text)
                    .ok_or_else(|| invalid_filter("state"))
            })
            .transpose()?,
        kind: params
            .kind
            .as_deref()
            .map(|text| {
                fold_core::requests::RequestKind::parse(text).ok_or_else(|| invalid_filter("kind"))
            })
            .transpose()?,
        requester: params
            .requester
            .as_deref()
            .map(|text| Requester::parse(text).ok_or_else(|| invalid_filter("requester")))
            .transpose()?,
        limit: None,
    };
    let items = project
        .0
        .tasks
        .list_requests(&filter)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(RequestList { items }))
}

/// A named human creates a request: the message and up to 10 files. The orchestrator that claims it sets the kind and the title.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/requests",
    operation_id = "create_request",
    params(("slug" = String, Path, description = "The project slug")),
    request_body(content = CreateRequestForm, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "The created request", body = RequestDetailView),
        (status = 400, description = "`invalid_request`: no body, an unknown part, or `details.reason` `too_many_attachments`, `attachment_too_large`, `invalid_attachment_name` or `invalid_media_type`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 413, description = "The body is larger than 10 files of 10 MiB and the message: a plain-text answer without an envelope"),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_request(
    human: Human,
    project: Project,
    form: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<RequestDetailView>), Refusal> {
    let mut form = form
        .map_err(|_| ApiError::invalid_request("the request body must be multipart/form-data"))?;
    let mut body = None;
    let mut attachments = Vec::new();
    while let Some(field) = form.next_field().await.map_err(unreadable)? {
        match field.name() {
            Some("body") => body = Some(field.text().await.map_err(unreadable)?),
            Some("files") => {
                check_attachment_count(attachments.len() + 1).map_err(ApiError)?;
                attachments.push(upload(field, attachments.len() + 1).await?);
            }
            _ => {
                return Err(ApiError::invalid_request(
                    "the form holds a part other than body and files",
                )
                .into());
            }
        }
    }
    let body = body.ok_or_else(|| ApiError::invalid_request("the form has no body part"))?;
    let tasks = &project.0.tasks;
    let view = tasks
        .create_request(&human.actor(), &body, attachments)
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::CREATED, Json(detail(&project, view.id).await?)))
}

/// A named human creates a request about lines they selected in a revision of main or in a proposal version. It is the same request as `POST /requests` makes, with the code location and a short excerpt of the lines added.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/requests/from-code",
    operation_id = "create_code_request",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = CodeRequestBody,
    responses(
        (status = 201, description = "The created request", body = RequestDetailView),
        (status = 400, description = "`invalid_request` (no body, or the lines are not named by a revision or by a proposal version and side), `invalid_id` or `invalid_path`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`revision_not_found`, `proposal_not_found` (also for a version the proposal does not have) or `project_not_found`", body = ErrorEnvelope),
        (status = 422, description = "`unknown_comment_path`: the tree has no text file at the path; `invalid_comment_range`: the lines are not in the file", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_code_request(
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<CodeRequestBody>,
) -> Result<(StatusCode, Json<RequestDetailView>), ApiError> {
    let (body, selection) = body.selection()?;
    let view = project
        .0
        .tasks
        .create_code_request(&human.actor(), &body, selection)
        .await?;
    Ok((StatusCode::CREATED, Json(detail(&project, view.id).await?)))
}

/// Why `POST /requests` refused: an error with its envelope, or a body over the route's limit, which answers axum's plain 413.
#[derive(Debug)]
pub enum Refusal {
    Api(ApiError),
    TooLarge(MultipartError),
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::TooLarge(error) => error.into_response(),
        }
    }
}

fn unreadable(error: MultipartError) -> Refusal {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return Refusal::TooLarge(error);
    }
    ApiError::invalid_request("the multipart body could not be read").into()
}

/// A file part at the 1-based `position`: its name, the media type it was sent with when that is a valid one, and its bytes, read no further than the attachment limit.
async fn upload(mut field: Field<'_>, position: usize) -> Result<NewRequestAttachment, Refusal> {
    let name = field.file_name().unwrap_or_default().to_owned();
    let media_type = field
        .content_type()
        .filter(|media_type| valid_media_type(media_type))
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(unreadable)? {
        if (bytes.len() + chunk.len()) as u64 > MAX_ATTACHMENT_BYTES {
            return Err(ApiError(attachment_too_large(position)).into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let guessed = NewRequestAttachment::named(name, bytes);
    Ok(match media_type {
        Some(media_type) => NewRequestAttachment {
            media_type,
            ..guessed
        },
        None => guessed,
    })
}

/// One request with every draft version, the comments, the clarification thread and the tasks its approval created. Reading needs no human name.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/requests/{id}",
    operation_id = "get_request",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The request id, such as RQ-4"),
    ),
    responses(
        (status = 200, description = "The request", body = RequestDetailView),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`request_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_request(
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<Json<RequestDetailView>, ApiError> {
    Ok(Json(detail(&project, request_id(&id)?).await?))
}

/// The bytes of one attachment, always as a download: the bytes come from users, so they are never shown in the page or sniffed as another type.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/requests/{id}/attachments/{n}",
    operation_id = "get_request_attachment",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The request id, such as RQ-4"),
        ("n" = u32, Path, description = "The attachment's 1-based position"),
    ),
    responses(
        (status = 200, description = "The attachment's bytes with its stored media type", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 400, description = "`invalid_id` or `invalid_request`: the position is not a number", body = ErrorEnvelope),
        (status = 404, description = "`request_not_found`, `not_found` (no attachment at that position) or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_request_attachment(
    project: Project,
    Path((_slug, id, position)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let position: u32 = position
        .parse()
        .map_err(|_| ApiError::invalid_request("the attachment position is not a number"))?;
    let (attachment, bytes) = project
        .0
        .tasks
        .request_attachment(request_id(&id)?, position)
        .await?;
    let media_type = HeaderValue::from_str(&attachment.media_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let disposition = HeaderValue::from_str(&content_disposition(&attachment.name))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    Ok((
        [
            (CONTENT_TYPE, media_type),
            (CONTENT_LENGTH, HeaderValue::from(bytes.len())),
            (CONTENT_DISPOSITION, disposition),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; sandbox"),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `attachment` with a plain ASCII `filename` for old clients and the exact name in `filename*` (RFC 6266), so no stored name can break out of the header.
pub fn content_disposition(name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && !matches!(c, '"' | '\\' | ';' | '%') || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

/// The requester sends the latest draft version back to the drafter with a comment.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/requests/{id}/changes",
    operation_id = "request_draft_changes",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The request id, such as RQ-4"),
    ),
    request_body = DraftChangesBody,
    responses(
        (status = 200, description = "The request, now with changes requested", body = RequestDetailView),
        (status = 400, description = "`invalid_request` (an empty comment, or `details.reason` `state` when there is no draft to send back) or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`requester_required`: the human is not the requester", body = ErrorEnvelope),
        (status = 404, description = "`request_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`stale_version` or `request_not_open`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn request_draft_changes(
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<DraftChangesBody>,
) -> Result<Json<RequestDetailView>, ApiError> {
    let request = request_id(&id)?;
    project
        .0
        .tasks
        .request_draft_changes(&human.actor(), request, body.version, &body.comment)
        .await?;
    Ok(Json(detail(&project, request).await?))
}

/// The requester writes the next draft version instead of asking for changes.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/requests/{id}/drafts",
    operation_id = "edit_request_draft",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The request id, such as RQ-4"),
    ),
    request_body = EditDraftBody,
    responses(
        (status = 200, description = "The request with the new version", body = RequestDetailView),
        (status = 400, description = "`invalid_draft` with `details.reason`, `invalid_request` (`details.reason` `state` when there is no draft to edit) or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`requester_required`: the human is not the requester", body = ErrorEnvelope),
        (status = 404, description = "`request_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`dependency_cycle` or `request_not_open`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn edit_request_draft(
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<EditDraftBody>,
) -> Result<Json<RequestDetailView>, ApiError> {
    let request = request_id(&id)?;
    let draft = SubmitDraft {
        cards: body
            .cards
            .into_iter()
            .map(|card| {
                Ok(DraftCard {
                    key: card.key,
                    title: card.title,
                    description: card.description,
                    label: card
                        .label
                        .as_deref()
                        .map(LabelName::parse)
                        .transpose()
                        .map_err(FoldError::from)?,
                })
            })
            .collect::<Result<_, FoldError>>()?,
        edges: body
            .edges
            .into_iter()
            .map(|edge| DraftEdge {
                prerequisite: edge.prerequisite,
                dependent: edge.dependent,
            })
            .collect(),
        note: body.note,
    };
    project
        .0
        .tasks
        .edit_request_draft(&human.actor(), request, draft)
        .await?;
    Ok(Json(detail(&project, request).await?))
}

/// The requester approves the latest draft version: one task per card and one dependency per edge, in one transaction.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/requests/{id}/approve",
    operation_id = "approve_request_draft",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The request id, such as RQ-4"),
    ),
    request_body = ApproveDraftBody,
    responses(
        (status = 200, description = "The approved request with the tasks it created", body = RequestDetailView),
        (status = 400, description = "`invalid_request` (`details.reason` `state` when no draft waits for approval) or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`requester_required`: the human is not the requester", body = ErrorEnvelope),
        (status = 404, description = "`request_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`stale_version` or `request_not_open`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn approve_request_draft(
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<ApproveDraftBody>,
) -> Result<Json<RequestDetailView>, ApiError> {
    let request = request_id(&id)?;
    project
        .0
        .tasks
        .approve_request_draft(&human.actor(), request, body.expected_version)
        .await?;
    Ok(Json(detail(&project, request).await?))
}

/// The requester closes the request with a reason; it then only reads.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/requests/{id}/close",
    operation_id = "close_request",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The request id, such as RQ-4"),
    ),
    request_body = CloseRequestBody,
    responses(
        (status = 200, description = "The closed request", body = RequestDetailView),
        (status = 400, description = "`invalid_request` (an empty reason) or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`requester_required`: the human is not the requester", body = ErrorEnvelope),
        (status = 404, description = "`request_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`request_not_open`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn close_request(
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<CloseRequestBody>,
) -> Result<Json<RequestDetailView>, ApiError> {
    let request = request_id(&id)?;
    project
        .0
        .tasks
        .close_request(&human.actor(), request, &body.reason)
        .await?;
    Ok(Json(detail(&project, request).await?))
}

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use fold_core::{Address, FoldError, MessageBody, MessageId, TaskId};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::actor::Human;
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::messages::{self as service, Feedback, MessageService, ThreadQuery};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_messages))
        .routes(routes!(show_thread))
        .routes(routes!(send_feedback))
}

fn service(state: &AppState, project: &Project) -> MessageService {
    let handle = &project.0;
    MessageService::new(
        handle.store.clone(),
        state.registry.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
    )
}

/// One recipient row. Names starting `human:` are humans.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecipientView {
    #[schema(example = "frontend/ui")]
    pub name: String,
    /// How the sender reached this recipient: `agent`, `worker`, `tag`, `all`, `human` or `humans`.
    pub addressed_as: String,
    /// UTC milliseconds; null until an inbox returned the row.
    #[schema(required = true)]
    pub delivered_at: Option<i64>,
    /// UTC milliseconds; null until the recipient acknowledged it.
    #[schema(required = true)]
    pub acknowledged_at: Option<i64>,
}

/// A stored message with public ids only.
#[derive(Debug, Serialize, ToSchema)]
pub struct MessageView {
    #[schema(example = "m-31")]
    pub id: String,
    /// The id of the thread's first message.
    #[schema(example = "m-31")]
    pub thread: String,
    #[schema(required = true, example = "m-30")]
    pub reply_to: Option<String>,
    /// A worker, `human:<name>` or `fold`.
    #[schema(example = "frontend/ui")]
    pub sender: String,
    #[schema(required = true, example = "T-12")]
    pub task: Option<String>,
    #[schema(required = true, example = "P-3")]
    pub proposal: Option<String>,
    /// Plain text.
    pub body: String,
    /// UTC milliseconds.
    pub sent_at: i64,
    pub recipients: Vec<RecipientView>,
}

impl From<service::MessageView> for MessageView {
    fn from(message: service::MessageView) -> Self {
        Self {
            id: message.id.to_string(),
            thread: message.thread.to_string(),
            reply_to: message.reply_to.map(|id| id.to_string()),
            sender: message.sender.to_string(),
            task: message.task.map(|id| id.to_string()),
            proposal: message.proposal.map(|id| id.to_string()),
            body: message.body,
            sent_at: message.sent_at.as_millis(),
            recipients: message
                .recipients
                .into_iter()
                .map(|recipient| RecipientView {
                    name: recipient.name.to_string(),
                    addressed_as: recipient.addressed_as.as_str().to_owned(),
                    delivered_at: recipient.delivered_at.map(|time| time.as_millis()),
                    acknowledged_at: recipient.acknowledged_at.map(|time| time.as_millis()),
                })
                .collect(),
        }
    }
}

/// A thread with every message in number order.
#[derive(Debug, Serialize, ToSchema)]
pub struct ThreadView {
    /// The id of the thread's first message.
    #[schema(example = "m-31")]
    pub id: String,
    /// The first task a message of the thread is linked to.
    #[schema(required = true, example = "T-12")]
    pub task: Option<String>,
    /// The first proposal a message of the thread is linked to.
    #[schema(required = true, example = "P-3")]
    pub proposal: Option<String>,
    /// Every sender and recipient in the order they first appear.
    pub participants: Vec<String>,
    /// True while the thread holds a question to a human that no human has answered; a human reply clears it.
    pub awaiting_human: bool,
    /// UTC milliseconds of the newest message.
    pub last_at: i64,
    pub messages: Vec<MessageView>,
}

impl From<service::ThreadView> for ThreadView {
    fn from(thread: service::ThreadView) -> Self {
        Self {
            id: thread.id.to_string(),
            task: thread.task.map(|id| id.to_string()),
            proposal: thread.proposal.map(|id| id.to_string()),
            participants: thread.participants,
            awaiting_human: thread.awaiting_human,
            last_at: thread.last_at.as_millis(),
            messages: thread.messages.into_iter().map(Into::into).collect(),
        }
    }
}

/// One page of threads, the newest message first.
#[derive(Debug, Serialize, ToSchema)]
pub struct ThreadList {
    pub items: Vec<ThreadView>,
    /// Pass as `before` for the next page; null on the last page.
    #[schema(required = true, example = "m-40")]
    pub next: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListMessagesParams {
    /// Threads in which this agent (any of its workers) or this exact `agent/worker` sent or received a message.
    pub agent: Option<String>,
    /// Threads with a message linked to this task, such as `T-12`.
    pub task: Option<String>,
    /// Threads with a message linked to this proposal, such as `P-3`.
    pub proposal: Option<String>,
    /// Only threads with a question to a human that no human has answered.
    pub awaiting_human: Option<bool>,
    /// The `next` of the previous page.
    pub before: Option<String>,
    /// The page size: 50 when left out, at most 200.
    pub limit: Option<u32>,
}

/// A human's feedback on a task, or reply to a message.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FeedbackRequest {
    /// Plain text with some visible character.
    pub body: String,
    /// The task the feedback is about, such as `T-12`. Needed when `reply_to` is left out.
    #[serde(default)]
    pub task: Option<String>,
    /// The message to answer, such as `m-31`. The reply joins its thread and takes its task.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// The recipients, as an agent, `agent/worker`, `tag:<tag>`, `all`, `human:<name>` or `humans`. Left out, feedback goes to the task's claim holder and a reply to the parent's sender.
    #[serde(default)]
    pub to: Option<Vec<String>>,
}

/// The project's threads, the newest message first. Any message of a thread can be read here; it changes no task state.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/messages",
    operation_id = "list_messages",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ListMessagesParams,
    ),
    responses(
        (status = 200, description = "One page of threads", body = ThreadList),
        (status = 400, description = "`invalid_request`, `invalid_id` or `invalid_name`: a filter is malformed", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_messages(
    State(state): State<AppState>,
    project: Project,
    params: Result<Query<ListMessagesParams>, QueryRejection>,
) -> Result<Json<ThreadList>, ApiError> {
    let Query(params) =
        params.map_err(|_| ApiError::invalid_request("the query string is not valid"))?;
    let page = service(&state, &project)
        .threads(&ThreadQuery {
            agent: params.agent,
            task: params.task,
            proposal: params.proposal,
            awaiting_human: params.awaiting_human.unwrap_or(false),
            before: params.before,
            limit: params.limit,
        })
        .await?;
    Ok(Json(ThreadList {
        items: page.items.into_iter().map(Into::into).collect(),
        next: page.next.map(|id| id.to_string()),
    }))
}

/// The thread that a message belongs to, every message in number order.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/threads/{id}",
    operation_id = "show_thread",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The id of the thread's first message, or of any message of the thread"),
    ),
    responses(
        (status = 200, description = "The thread", body = ThreadView),
        (status = 400, description = "`invalid_id`: the id is not a message id", body = ErrorEnvelope),
        (status = 404, description = "`message_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn show_thread(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<Json<ThreadView>, ApiError> {
    let id = id.parse::<MessageId>().map_err(FoldError::from)?;
    let thread = service(&state, &project).thread_view(id).await?;
    Ok(Json(thread.into()))
}

/// A named human's feedback on a task or reply to a message. Feedback goes to the task's claim holder unless `to` names recipients; a reply answers a question to humans, which clears the task's `question` attention. The message changes no task state.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/feedback",
    operation_id = "send_feedback",
    params(
        ("slug" = String, Path, description = "The project slug"),
    ),
    request_body = FeedbackRequest,
    responses(
        (status = 201, description = "The stored message", body = MessageView),
        (status = 400, description = "`invalid_request`: no `task` or `reply_to`, an empty body or a malformed request; `invalid_id` or `invalid_name`: an id or recipient is malformed", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `message_not_found`, `proposal_not_found`, `unknown_human`, `recipient_not_found` (also when the task has no claim holder and `to` is left out) or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn send_feedback(
    human: Human,
    State(state): State<AppState>,
    project: Project,
    ApiJson(request): ApiJson<FeedbackRequest>,
) -> Result<(StatusCode, Json<MessageView>), ApiError> {
    let feedback = Feedback {
        body: MessageBody::parse(request.body)?,
        task: request
            .task
            .as_deref()
            .map(str::parse::<TaskId>)
            .transpose()
            .map_err(FoldError::from)?,
        reply_to: request
            .reply_to
            .as_deref()
            .map(str::parse::<MessageId>)
            .transpose()
            .map_err(FoldError::from)?,
        to: request
            .to
            .as_deref()
            .map(Address::parse_all)
            .transpose()
            .map_err(FoldError::from)?,
    };
    let message = service(&state, &project)
        .feedback(&human.actor(), feedback)
        .await?;
    Ok((StatusCode::CREATED, Json(message.into())))
}

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use fold_core::{Address, Choice, FoldError, MessageBody, MessageId, RequestId, TaskId, codes};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::actor::Human;
use super::error::{ApiError, ApiJson, ApiQuery, ErrorEnvelope};
use super::reviews::CommentSide;
use crate::daemon::AppState;
use crate::services::messages::{self as service, DismissScope, Feedback, ThreadQuery};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_messages))
        .routes(routes!(show_thread))
        .routes(routes!(send_feedback))
        .routes(routes!(mark_asks_answered))
}

/// One recipient row. Names starting `human:` are humans.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecipientView {
    #[schema(example = "frontend")]
    pub name: String,
    /// How the sender reached this recipient: `agent`, `worker`, `tag`, `all`, `human` or `humans`.
    pub addressed_as: String,
    /// UTC milliseconds; null until an inbox returned the row.
    #[schema(required = true)]
    pub delivered_at: Option<i64>,
    /// UTC milliseconds; null until the recipient acknowledged it.
    #[schema(required = true)]
    pub acknowledged_at: Option<i64>,
    /// UTC milliseconds: the first time a notice about the message reached a session or the worker process of the recipient's agent, where it waits until the agent reads its inbox. Null for a human and while no notice got through.
    #[schema(required = true)]
    pub pushed_at: Option<i64>,
    /// For an agent recipient, whether fold can post a notice into one of the agent's sessions now; false means the agent learns of the message only when it reads its inbox. Null for a human and for an agent without a connected session.
    #[schema(required = true)]
    pub reachable: Option<bool>,
    /// The lead that fold sent the message on to because this recipient, a stopped worker, could not start to answer it; the sender itself when the sender is that lead. Null while fold has not sent it on.
    #[schema(required = true, example = "web-lead")]
    pub forwarded_to: Option<String>,
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
    #[schema(example = "frontend")]
    pub sender: String,
    #[schema(required = true, example = "T-12")]
    pub task: Option<String>,
    #[schema(required = true, example = "P-3")]
    pub proposal: Option<String>,
    /// The request whose clarification thread holds the message.
    #[schema(required = true, example = "RQ-4")]
    pub request: Option<String>,
    /// Plain text.
    pub body: String,
    /// UTC milliseconds.
    pub sent_at: i64,
    pub recipients: Vec<RecipientView>,
    /// The options of a multiple-choice question, in order; empty for any other message. A human answers with a reply whose body names one option's label, or with free text.
    pub choices: Vec<MessageChoice>,
    /// The short header of a multiple-choice question, such as `Chart library`.
    #[schema(required = true, example = "Chart library")]
    pub header: Option<String>,
    /// The sending agent marked the message as asking a human for an answer. A question with choices and a worker's `AskUserQuestion` message ask without the mark.
    pub asks_human: bool,
    /// The lines of a version of the message's proposal that a human's question to its author is about; null for any other message.
    #[schema(required = true)]
    pub lines: Option<MessageLines>,
}

/// A run of lines of one file of a proposal version, with their text.
#[derive(Debug, Serialize, ToSchema)]
pub struct MessageLines {
    /// The version of the message's proposal.
    pub version: u32,
    #[schema(example = "web/src/board.ts")]
    pub path: String,
    /// `old` reads the version's base, `new` the version.
    pub side: CommentSide,
    /// From 1.
    pub start_line: u32,
    /// Inclusive.
    pub end_line: u32,
    /// The lines joined by newlines, without their line endings.
    pub excerpt: String,
    /// The excerpt holds only the first lines of the run: at most 100 lines and 20000 characters.
    pub excerpt_cut: bool,
}

impl From<fold_store::messages::MessageLines> for MessageLines {
    fn from(lines: fold_store::messages::MessageLines) -> Self {
        Self {
            version: lines.version,
            path: lines.path,
            side: lines.side.into(),
            start_line: lines.start_line,
            end_line: lines.end_line,
            excerpt: lines.excerpt,
            excerpt_cut: lines.excerpt_cut,
        }
    }
}

/// One option of a multiple-choice question.
#[derive(Debug, Serialize, ToSchema)]
pub struct MessageChoice {
    #[schema(example = "Recharts")]
    pub label: String,
    /// One line that says what the option means; null for an option without one.
    #[schema(required = true, example = "Ready-made bar and line components.")]
    pub description: Option<String>,
}

impl From<Choice> for MessageChoice {
    fn from(choice: Choice) -> Self {
        Self {
            label: choice.label,
            description: choice.description,
        }
    }
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
            request: message.request.map(|id| id.to_string()),
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
                    pushed_at: recipient.pushed_at.map(|time| time.as_millis()),
                    reachable: recipient.reachable,
                    forwarded_to: recipient.forwarded_to.map(|worker| worker.to_string()),
                })
                .collect(),
            choices: message
                .choices
                .into_iter()
                .map(MessageChoice::from)
                .collect(),
            header: message.header,
            asks_human: message.asks_human,
            lines: message.lines.map(Into::into),
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
    /// The request whose clarification thread this is.
    #[schema(required = true, example = "RQ-4")]
    pub request: Option<String>,
    /// Every sender and recipient in the order they first appear.
    pub participants: Vec<String>,
    /// True while the thread holds an open ask: a message an agent marked as asking a human, a question with choices or a worker's `AskUserQuestion`, with no human reply after it and not marked answered. In a `mine` listing, only an ask that waits for the signed-in human: addressed to them by name or to `humans`.
    pub awaiting_human: bool,
    /// One of those asks is closable: a human can mark it answered with the asks route, because it is no worker's `AskUserQuestion`, which only a reply in the thread ends. False while `awaiting_human` is false.
    pub closable: bool,
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
            request: thread.request.map(|id| id.to_string()),
            participants: thread.participants,
            awaiting_human: thread.awaiting_human,
            closable: thread.closable,
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
    /// How many threads hold an open ask, whatever the other filters: in the whole project, or with `mine` the threads with an open ask that waits for the signed-in human, which are the threads the listing marks `awaiting_human`. The navigation badge reads it with `mine=true&limit=1`.
    pub total_awaiting_human: u64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListMessagesParams {
    /// Threads in which this agent sent or received a message.
    pub agent: Option<String>,
    /// Threads with a message linked to this task, such as `T-12`.
    pub task: Option<String>,
    /// Threads with a message linked to this proposal, such as `P-3`.
    pub proposal: Option<String>,
    /// Only threads with an open ask; with `mine`, an open ask that waits for the signed-in human.
    pub awaiting_human: Option<bool>,
    /// Only the threads that concern the signed-in human: a message of the thread is from them, addressed to them by name or addressed to `humans`. Agent-to-agent threads stay readable through `task`, `proposal` and the thread route. Needs a signed-in human.
    pub mine: Option<bool>,
    /// The `next` of the previous page.
    pub before: Option<String>,
    /// The page size: 50 when left out, at most 200.
    pub limit: Option<u32>,
}

/// A human's feedback on a task, message in a request's clarification thread, or reply to a message.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FeedbackRequest {
    /// Plain text with some visible character. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub body: String,
    /// The task the feedback is about, such as `T-12`. Needed when neither `request` nor `reply_to` is given.
    #[serde(default)]
    pub task: Option<String>,
    /// A request, such as `RQ-4`: the message joins its clarification thread. A reply takes its parent's request; naming another is `invalid_request` with `details.reason` `request_thread`.
    #[serde(default)]
    pub request: Option<String>,
    /// The message to answer, such as `m-31`. The reply joins its thread and takes its task and request.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// The recipients, as an agent, `tag:<tag>`, `all`, `human:<name>` or `humans`. Feedback on a task takes only the task's readers, and left out it goes to the first of them; left out, a request message to the request's drafter and a reply to the parent's sender.
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
        (status = 401, description = "`human_name_required`: `mine` is set and the request names no human", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_messages(
    human: Option<Human>,
    State(state): State<AppState>,
    project: Project,
    ApiQuery(params): ApiQuery<ListMessagesParams>,
) -> Result<Json<ThreadList>, ApiError> {
    let human = match (params.mine.unwrap_or(false), human) {
        (false, _) => None,
        (true, Some(Human(name))) => Some(name),
        (true, None) => {
            return Err(FoldError::new(
                codes::HUMAN_NAME_REQUIRED,
                "`mine` needs a signed-in human",
            )
            .with_hint("sign in to the web interface, or set FOLD_HUMAN for the CLI")
            .into());
        }
    };
    let page = project
        .0
        .messages(&state.registry)
        .threads(&ThreadQuery {
            agent: params.agent,
            task: params.task,
            proposal: params.proposal,
            human,
            awaiting_human: params.awaiting_human.unwrap_or(false),
            before: params.before,
            limit: params.limit,
        })
        .await?;
    Ok(Json(ThreadList {
        items: page.items.into_iter().map(Into::into).collect(),
        next: page.next.map(|id| id.to_string()),
        total_awaiting_human: page.total_awaiting_human,
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
    let thread = project.0.messages(&state.registry).thread_view(id).await?;
    Ok(Json(thread.into()))
}

/// A named human's feedback on a task, message in a request's clarification thread, or reply to a message. Feedback goes only to the task's readers, as `GET /tasks/{id}/readers` lists them: to `to` when given, otherwise to the first reader. A request message goes to the request's drafter unless `to` names recipients; a reply answers a question to humans, which clears the task's `question` attention. The message changes no task or request state.
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
        (status = 400, description = "`invalid_request`: no `task`, `request` or `reply_to`, a `request` other than the parent's (`details.reason` `request_thread`), an empty body or a malformed request; `invalid_id` or `invalid_name`: an id or recipient is malformed", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `request_not_found`, `message_not_found`, `proposal_not_found`, `unknown_human`, `recipient_not_found` (also when the task has no reader, or the request no drafter, and `to` is left out) or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`recipient_not_reader`: feedback on a task names a recipient that is not one of the task's readers", body = ErrorEnvelope),
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
        request: request
            .request
            .as_deref()
            .map(str::parse::<RequestId>)
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
    let message = project
        .0
        .messages(&state.registry)
        .feedback(&human.actor(), feedback)
        .await?;
    Ok((StatusCode::CREATED, Json(message.into())))
}

/// The asks a human marks answered: those of one thread, or every ask linked to one task.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MarkAnsweredRequest {
    /// Any message of the thread whose asks are answered, such as `m-31`. Give `thread` or `task`, not both.
    #[serde(default)]
    pub thread: Option<String>,
    /// The task whose asks are answered, such as `T-12`.
    #[serde(default)]
    pub task: Option<String>,
}

/// The asks the call marked answered.
#[derive(Debug, Serialize, ToSchema)]
pub struct MarkedAnswered {
    /// In number order; empty when nothing waited. A worker's `AskUserQuestion` stays open, because its worker waits for a reply in the thread.
    #[schema(example = json!(["m-31"]))]
    pub answered: Vec<String>,
}

/// A named human marks the open asks of a thread, or of a task, answered without a reply, such as when they answered the agent elsewhere. Each closed ask appends `message.ask_closed`, and the thread, the board's question mark, Needs you and the Messages count stop waiting for it. The messages stay unchanged.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/asks/answered",
    operation_id = "mark_asks_answered",
    params(
        ("slug" = String, Path, description = "The project slug"),
    ),
    request_body = MarkAnsweredRequest,
    responses(
        (status = 200, description = "The asks marked answered", body = MarkedAnswered),
        (status = 400, description = "`invalid_request`: neither or both of `thread` and `task`; `invalid_id`: an id is malformed", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "`message_not_found`, `task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn mark_asks_answered(
    human: Human,
    State(state): State<AppState>,
    project: Project,
    ApiJson(request): ApiJson<MarkAnsweredRequest>,
) -> Result<Json<MarkedAnswered>, ApiError> {
    let scope = match (request.thread.as_deref(), request.task.as_deref()) {
        (Some(thread), None) => {
            DismissScope::Thread(thread.parse::<MessageId>().map_err(FoldError::from)?)
        }
        (None, Some(task)) => DismissScope::Task(task.parse::<TaskId>().map_err(FoldError::from)?),
        _ => {
            return Err(
                FoldError::new(codes::INVALID_REQUEST, "give thread or task, not both")
                    .with_hint("name the thread such as m-31, or the task such as T-12")
                    .into(),
            );
        }
    };
    let answered = project
        .0
        .messages(&state.registry)
        .dismiss_asks(&human.actor(), scope)
        .await?;
    Ok(Json(MarkedAnswered {
        answered: answered.iter().map(ToString::to_string).collect(),
    }))
}

use axum::Json;
use axum::extract::Path;
use fold_core::roles::PermissionBehavior;
use fold_core::{FoldError, PermissionQuestionId};
use fold_store::agents::QuestionRow;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::super::messages::MessageChoice;
use crate::daemon::AppState;
use crate::services::agents::{NeedsYouItem, NeedsYouKind, Screen};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_worker_questions))
        .routes(routes!(answer_worker_question))
        .routes(routes!(list_needs_you))
}

/// A human's answer to a worker's permission question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    Allow,
    Deny,
}

impl From<Behavior> for PermissionBehavior {
    fn from(behavior: Behavior) -> Self {
        match behavior {
            Behavior::Allow => Self::Allow,
            Behavior::Deny => Self::Deny,
        }
    }
}

impl From<PermissionBehavior> for Behavior {
    fn from(behavior: PermissionBehavior) -> Self {
        match behavior {
            PermissionBehavior::Allow => Self::Allow,
            PermissionBehavior::Deny => Self::Deny,
        }
    }
}

/// The answer a question got.
#[derive(Debug, Serialize, ToSchema)]
pub struct QuestionAnswer {
    pub behavior: Behavior,
    /// `human:<name>`.
    #[schema(example = "human:tanim")]
    pub answered_by: String,
    /// The human's note to the worker.
    #[schema(required = true)]
    pub message: Option<String>,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub answered_at: fold_core::Timestamp,
}

/// A tool call of a worker that a permission rule marks `ask` and Claude Code's auto mode would not decide. The worker waits until a human allows or denies it.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerQuestionEntry {
    #[schema(example = "pq-3")]
    pub id: String,
    /// The worker's agent name.
    #[schema(example = "web-worker")]
    pub worker: String,
    #[schema(example = "Bash")]
    pub tool: String,
    /// The tool's input as Claude Code sent it, such as `{"command": "make deploy"}`. It is shown here and never logged.
    #[schema(value_type = Object)]
    pub input: serde_json::Value,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub asked_at: fold_core::Timestamp,
    /// Null while the question is open.
    #[schema(required = true)]
    pub answer: Option<QuestionAnswer>,
}

impl From<QuestionRow> for WorkerQuestionEntry {
    fn from(row: QuestionRow) -> Self {
        Self {
            id: row.number.map(|id| id.to_string()).unwrap_or_default(),
            worker: row.agent.to_string(),
            tool: row.tool,
            input: row.input,
            asked_at: row.created_at,
            answer: row.answer.map(|answer| QuestionAnswer {
                behavior: answer.behavior.into(),
                answered_by: answer.answered_by,
                message: answer.message,
                answered_at: answer.created_at,
            }),
        }
    }
}

/// The open permission questions, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerQuestionList {
    pub items: Vec<WorkerQuestionEntry>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AnswerWorkerQuestionBody {
    pub behavior: Behavior,
    /// A note for the worker: the tool error it gets with `deny`, or what it reads with the answer after a restart.
    #[serde(default)]
    pub message: Option<String>,
}

/// Why an item waits for a human.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeedsYouItemKind {
    /// A question with choices from an agent to the humans, or a worker's `AskUserQuestion` call, that no human has answered in its thread. A plain message to the humans is not an item.
    Question,
    /// A worker's permission question.
    Permission,
    /// A proposal of a marked task that waits for a human approval.
    HumanReview,
    /// A card draft that waits for its human requester.
    RequestDraft,
    /// A human task in Todo: every prerequisite is Done and a person does the work, then marks it Done.
    HumanTask,
    /// A task with a blocker, which waits until a human clears it; `reason` says why.
    Blocker,
    /// A human change from the project folder that conflicts with main: main did not move, and the main agent explains the conflict in its conversation and prepares the merge for the human's OK. `commit` is the human's Git commit, null for a change `fld record` read from the folder, and `paths` the conflicting files.
    HumanChange,
    /// The project folder's Git branch or HEAD shows a change fold did not make, such as another branch checked out, a reset or a stash, so fold does not update the folder; `reason` is the command that brings the branch back.
    HeadMoved,
}

/// The screen that answers an item: `messages` opens the thread, `worker` the asking worker, `proposal` the proposal, `request` the request, `task` the task and `repository` the history of main.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeedsYouScreen {
    Messages,
    Worker,
    Proposal,
    Request,
    Task,
    Repository,
}

/// One item that waits for a human. Only the ids its kind has are set.
#[derive(Debug, Serialize, ToSchema)]
pub struct NeedsYouEntry {
    pub kind: NeedsYouItemKind,
    pub screen: NeedsYouScreen,
    /// UTC milliseconds since the item waits.
    #[schema(value_type = i64)]
    pub since: fold_core::Timestamp,
    /// The question message of a `question`.
    #[schema(required = true, example = "m-31")]
    pub message: Option<String>,
    /// The thread of a `question`.
    #[schema(required = true, example = "m-31")]
    pub thread: Option<String>,
    /// The permission question of a `permission`.
    #[schema(required = true, example = "pq-3")]
    pub question: Option<String>,
    /// The agent that asks.
    #[schema(required = true, example = "web-worker")]
    pub agent: Option<String>,
    /// The tool of a `permission`.
    #[schema(required = true, example = "Bash")]
    pub tool: Option<String>,
    #[schema(required = true, example = "T-12")]
    pub task: Option<String>,
    #[schema(required = true, example = "P-3")]
    pub proposal: Option<String>,
    #[schema(required = true, example = "RQ-4")]
    pub request: Option<String>,
    /// The human a `human_review` or `request_draft` waits for, or whose change a `human_change` is; null when any human may answer.
    #[schema(required = true, example = "tanim")]
    pub human: Option<String>,
    /// The options of a `question` with choices; empty otherwise.
    pub choices: Vec<MessageChoice>,
    /// The short header of a `question`, such as `Chart library`.
    #[schema(required = true, example = "Chart library")]
    pub header: Option<String>,
    /// The second line of a `human_review` (the proposal title) and of a `request_draft` (the request title, which the claim gave it); the task title of a `human_task` or a `blocker`.
    #[schema(required = true, example = "Database schema for reports")]
    pub title: Option<String>,
    /// The reason of a `blocker`; the command that brings the Git branch back for `head_moved`.
    #[schema(required = true, example = "Wait until the design is signed off")]
    pub reason: Option<String>,
    /// The human's Git commit of a `human_change`.
    #[schema(required = true, example = "9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c3b5d7f9e")]
    pub commit: Option<String>,
    /// The files of a `human_change` that conflict with main, in path order; empty otherwise.
    pub paths: Vec<String>,
}

impl From<NeedsYouItem> for NeedsYouEntry {
    fn from(item: NeedsYouItem) -> Self {
        let kind = match item.kind {
            NeedsYouKind::Question => NeedsYouItemKind::Question,
            NeedsYouKind::Permission => NeedsYouItemKind::Permission,
            NeedsYouKind::HumanReview => NeedsYouItemKind::HumanReview,
            NeedsYouKind::RequestDraft => NeedsYouItemKind::RequestDraft,
            NeedsYouKind::HumanTask => NeedsYouItemKind::HumanTask,
            NeedsYouKind::Blocker => NeedsYouItemKind::Blocker,
            NeedsYouKind::HumanChange => NeedsYouItemKind::HumanChange,
            NeedsYouKind::HeadMoved => NeedsYouItemKind::HeadMoved,
        };
        let screen = match item.kind.screen() {
            Screen::Messages => NeedsYouScreen::Messages,
            Screen::Worker => NeedsYouScreen::Worker,
            Screen::Proposal => NeedsYouScreen::Proposal,
            Screen::Request => NeedsYouScreen::Request,
            Screen::Task => NeedsYouScreen::Task,
            Screen::Repository => NeedsYouScreen::Repository,
        };
        Self {
            kind,
            screen,
            since: item.since,
            message: item.message.map(|(message, _)| message.to_string()),
            thread: item.message.map(|(_, thread)| thread.to_string()),
            question: item.question.map(|id| id.to_string()),
            agent: item.agent.map(|agent| agent.to_string()),
            tool: item.tool,
            task: item.task.map(|id| id.to_string()),
            proposal: item.proposal.map(|id| id.to_string()),
            request: item.request.map(|id| id.to_string()),
            human: item.human.map(|human| human.to_string()),
            choices: item.choices.into_iter().map(MessageChoice::from).collect(),
            header: item.header,
            title: item.title,
            reason: item.reason,
            commit: item.commit,
            paths: item.paths,
        }
    }
}

/// Every item that waits for a human, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct NeedsYouList {
    pub items: Vec<NeedsYouEntry>,
}

/// The open permission questions of the project's workers, oldest first. A worker's `AskUserQuestion` call is not among them: it is a question message.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/worker-questions",
    operation_id = "list_worker_questions",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The open questions", body = WorkerQuestionList),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_worker_questions(project: Project) -> Result<Json<WorkerQuestionList>, ApiError> {
    let questions = project.0.agents().open_permission_questions().await?;
    Ok(Json(WorkerQuestionList {
        items: questions.into_iter().map(Into::into).collect(),
    }))
}

/// A named human allows or denies a worker's tool call. The answer reaches the waiting worker at once, or as the first turn of its session when fold resumes it after a restart.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/worker-questions/{id}/answer",
    operation_id = "answer_worker_question",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The question, such as `pq-3`"),
    ),
    request_body = AnswerWorkerQuestionBody,
    responses(
        (status = 200, description = "The answered question", body = WorkerQuestionEntry),
        (status = 400, description = "`invalid_request` or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`permission_question_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`permission_question_closed`: the question has its answer, or fold stopped its worker", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn answer_worker_question(
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<AnswerWorkerQuestionBody>,
) -> Result<Json<WorkerQuestionEntry>, ApiError> {
    let id = id
        .parse::<PermissionQuestionId>()
        .map_err(FoldError::from)?;
    let answered = project
        .0
        .agents()
        .answer_worker_permission(&human.actor(), id, body.behavior.into(), body.message)
        .await?;
    Ok(Json(answered.into()))
}

/// Every item that waits for a human, oldest first: questions with choices from agents to the humans, workers' permission questions, proposals waiting for a human approval, card drafts waiting for their human requester, and a Git branch of the project folder that fold did not move. Each item names the screen that answers it. The list changes with the stored events `message.sent`, `worker.permission_asked`, `worker.permission_answered`, `worker.stopped`, `worker.failed`, `review.submitted`, `task.human_review_set`, `task.human_review_cleared`, `proposal.*`, `request.*`, `git.head_moved` and `git.attention_cleared`.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/needs-you",
    operation_id = "list_needs_you",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The items", body = NeedsYouList),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_needs_you(project: Project) -> Result<Json<NeedsYouList>, ApiError> {
    let mut items = project.0.agents().needs_you().await?;
    if let Some((since, hint)) = project.0.git_attention.head_moved() {
        items.push(NeedsYouItem::head_moved(since, hint));
        items.sort_by_key(|item| item.since);
    }
    Ok(Json(NeedsYouList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

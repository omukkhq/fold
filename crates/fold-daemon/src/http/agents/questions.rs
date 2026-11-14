use axum::Json;
use axum::extract::{Path, State};
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
use crate::live::LiveActivity;
use crate::services::agents::{
    NeedsYouAction, NeedsYouItem, NeedsYouKind, Screen, with_folder_attention,
};
use crate::services::projects::notify;

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
    /// An open ask from an agent to a human: a message the agent marked as asking, a question with choices, or a worker's `AskUserQuestion` call, with no human reply in its thread and no closure. A report to the humans is not an item.
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
    /// Paths main changed hold changes of the human's that no revision records, so fold does not update the project folder until the human records them with `fld record` or takes them back; `paths` names them and `reason` the command.
    FolderHeld,
    /// `fld unstash` wrote conflict markers into the files `paths` names and kept the stash entry, so fold does not update the project folder until the human fixes the files and records them with `fld record`; `reason` says so.
    UnstashConflicted,
    /// The checks file on main is not valid, so fold holds the integrations of proposals until a revision of main fixes it; `title` states the file, the line and the problem, and `paths` names the file.
    ChecksFileInvalidOnMain,
    /// The worker `agent` failed because it cannot reach fold: its fold MCP server did not connect, or the fld binary it would start is gone. Starting it again fails the same way until the daemon runs the installed fld; `reason` says what the human does. A daemon start resumes the worker once fld is there. The item ends when the worker runs again, through that resume or through `resume_worker`.
    WorkerUnreachable,
    /// The worker `agent` failed several turns in a row before any token reached the model, so it does no work until a human fixes what Claude Code needs, such as its sign-in, its model or its usage limit: `title` holds Claude Code's error and `reason` what to check. The item ends when a turn of the worker reaches the model, with or without an error, or when the worker is stopped. A worker whose process failed cannot be stopped, so it keeps the item until `resume_worker` runs it again and a turn reaches the model; a start that runs a stopped worker again shows the item again until then. The same kind with a null `agent` is the project's one item for turns that could not reach the model at all, such as while the network is down: `agents` names the workers whose turn waits, `title` holds Claude Code's newest error and `reason` says what fold does. fold runs each turn again in its session once the model answers, which ends the item.
    WorkerModelUnreachable,
    /// The worker `agent` failed because the daemon found no `claude` on the `PATH` it gives workers, which it reads from the user's login shell at its start. `reason` says what the human does; a daemon start resumes the worker, and the item ends when the worker runs again.
    ClaudeNotFound,
    /// fold turned the notice target `target` off: its receiver answered 410 Gone, a send failed every try of its retry schedule (about 8.6 hours), or 20 tries in a row failed. `title` is its name and `reason` says why and what the human does. A test send that gets through turns it on again and ends the item.
    NotifyTargetOff,
    /// Files of the project folder changed during the tool calls of supervised workers, so fold does not offer them as the human's change: `agents` names the workers' agents and `agent` the first, `paths` the files and `reason` the choices. The human records them with `record_agent_changes`, puts them aside with `restore_agent_changes` or takes them as their own with `ignore_agent_changes`; the item ends when every path is settled.
    AgentChange,
    /// Shared reference files changed during a tool call of the supervised worker `agent`: `paths` names the files and `reason` what the human does, check the files and run `fld share --dismiss`, which ends the item. Workers read shared files through links into the project folder, where a write has no history to undo it.
    SharedChanged,
    /// The worker `agent` on `task` stalled, fold told its lead, or else the main agent, and one more stall window passed without a fix, or no agent can act on it: `activity` says what fold saw and `reason` what the human does. The web's Restart worker calls `restart_worker`. The item ends when the worker works again. fold keeps it in memory, so after a daemon start it counts from that start.
    WorkerStalled,
    /// Turns of the project's workers reached Claude Code's usage limit `limit`; one item per limit. `agents` names the workers whose turn waits, `title` holds Claude Code's newest message, `reset` its wording of the reset and `resets_at` the reset instant when the message gives one, `reason` what fold does and `actions` what the human can do. fold runs each turn again in its session after the reset, which ends the item.
    UsageLimit,
}

/// A usage limit of Claude Code: `session`, `weekly`, `opus`, `sonnet`, or `usage` for the older message that names no kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageLimitKind {
    Session,
    Weekly,
    Opus,
    Sonnet,
    Usage,
}

impl From<fold_core::turn_waits::LimitKind> for UsageLimitKind {
    fn from(kind: fold_core::turn_waits::LimitKind) -> Self {
        use fold_core::turn_waits::LimitKind;
        match kind {
            LimitKind::Session => Self::Session,
            LimitKind::Weekly => Self::Weekly,
            LimitKind::Opus => Self::Opus,
            LimitKind::Sonnet => Self::Sonnet,
            LimitKind::Usage => Self::Usage,
        }
    }
}

/// What a human can do from an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeedsYouActionKind {
    /// Pause every agent of the project, as `pause_project` does.
    Pause,
    /// Make `account` the project's account, as `PUT /api/v1/projects/{slug}/account` does; each waiting turn then runs again on it.
    SwitchAccount,
}

/// One action of an item.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NeedsYouActionEntry {
    pub action: NeedsYouActionKind,
    /// The account of a `switch_account`, by name; absent for every other action.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "work-b")]
    pub account: Option<String>,
}

impl From<NeedsYouAction> for NeedsYouActionEntry {
    fn from(action: NeedsYouAction) -> Self {
        match action {
            NeedsYouAction::Pause => Self {
                action: NeedsYouActionKind::Pause,
                account: None,
            },
            NeedsYouAction::SwitchAccount(name) => Self {
                action: NeedsYouActionKind::SwitchAccount,
                account: Some(name.as_str().to_owned()),
            },
        }
    }
}

/// The screen that answers an item: `messages` opens the thread, `worker` the asking worker, `proposal` the proposal, `request` the request, `task` the task, `repository` the history of main and `settings` the project's notice targets.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeedsYouScreen {
    Messages,
    Worker,
    Proposal,
    Request,
    Task,
    Repository,
    Settings,
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
    /// The human a `human_review` or a `request_draft` waits for, or whose change a `human_change` is; null when any human may answer.
    #[schema(required = true, example = "tanim")]
    pub human: Option<String>,
    /// The humans a `question` names, in name order, when none of its recipients is `humans`: it waits for each of them, as their Messages count does. Empty when any human may answer, and for every other kind.
    #[schema(example = json!(["ana", "bob"]))]
    pub humans: Vec<String>,
    /// The options of a `question` with choices; empty otherwise.
    pub choices: Vec<MessageChoice>,
    /// The short header of a `question`, such as `Chart library`.
    #[schema(required = true, example = "Chart library")]
    pub header: Option<String>,
    /// How many questions a `question` asks as one set, which a human answers together in one reply; 0 for every other kind.
    pub questions: usize,
    /// The second line of a `human_review` (the proposal title) and of a `request_draft` (the request title, which the claim gave it); the task title of a `human_task` or a `blocker`; the file, line and problem of a `checks_file_invalid_on_main`; Claude Code's error of a `worker_model_unreachable`; what fold saw of a `worker_stalled` at the read, such as `No activity from @web-worker for 12 min`, in the words of the board's chip.
    #[schema(required = true, example = "Database schema for reports")]
    pub title: Option<String>,
    /// The reason of a `blocker`; the command that brings the Git branch back for `head_moved`, records the folder's changes for `folder_held`, ends the conflict for `unstash_conflicted`, or lets the worker of a `worker_unreachable` reach fold again; what to check for a `worker_model_unreachable`.
    #[schema(required = true, example = "Wait until the design is signed off")]
    pub reason: Option<String>,
    /// The human's Git commit of a `human_change`.
    #[schema(required = true, example = "9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c3b5d7f9e")]
    pub commit: Option<String>,
    /// The files of a `human_change` that conflict with main, those of a `folder_held` that keep the project folder where it is, those of an `unstash_conflicted` that hold the conflict, or those of an `agent_change` that a worker changed, in path order; empty otherwise.
    pub paths: Vec<String>,
    /// The notice target of a `notify_target_off`.
    #[schema(required = true, example = "nt-3")]
    pub target: Option<String>,
    /// The agents of an `agent_change`, in name order; empty for every other kind.
    #[schema(example = json!(["reviewer-35"]))]
    pub agents: Vec<String>,
    /// What fold saw of the worker of a `worker_stalled`; null for every other kind.
    #[schema(required = true)]
    pub activity: Option<LiveActivity>,
    /// The limit of a `usage_limit`; null for every other kind.
    #[schema(required = true)]
    pub limit: Option<UsageLimitKind>,
    /// Claude Code's wording of the reset of a `usage_limit`, such as `1:50am (Asia/Tokyo)` or `Mon 12:00am`, a local time without a date; null when its message gives none, and for every other kind.
    #[schema(required = true, example = "1:50am (Asia/Tokyo)")]
    pub reset: Option<String>,
    /// UTC milliseconds of the reset of a `usage_limit`, when Claude Code's message gives the instant; null otherwise.
    #[schema(value_type = Option<i64>, required = true)]
    pub resets_at: Option<fold_core::Timestamp>,
    /// What the human can do from the item, such as `pause` for a `usage_limit`; empty for most kinds.
    pub actions: Vec<NeedsYouActionEntry>,
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
            NeedsYouKind::FolderHeld => NeedsYouItemKind::FolderHeld,
            NeedsYouKind::UnstashConflicted => NeedsYouItemKind::UnstashConflicted,
            NeedsYouKind::ChecksFileInvalidOnMain => NeedsYouItemKind::ChecksFileInvalidOnMain,
            NeedsYouKind::WorkerUnreachable => NeedsYouItemKind::WorkerUnreachable,
            NeedsYouKind::WorkerModelUnreachable => NeedsYouItemKind::WorkerModelUnreachable,
            NeedsYouKind::ClaudeNotFound => NeedsYouItemKind::ClaudeNotFound,
            NeedsYouKind::NotifyTargetOff => NeedsYouItemKind::NotifyTargetOff,
            NeedsYouKind::AgentChange => NeedsYouItemKind::AgentChange,
            NeedsYouKind::SharedChanged => NeedsYouItemKind::SharedChanged,
            NeedsYouKind::WorkerStalled => NeedsYouItemKind::WorkerStalled,
            NeedsYouKind::UsageLimit => NeedsYouItemKind::UsageLimit,
        };
        let screen = match item.kind.screen() {
            Screen::Messages => NeedsYouScreen::Messages,
            Screen::Worker => NeedsYouScreen::Worker,
            Screen::Proposal => NeedsYouScreen::Proposal,
            Screen::Request => NeedsYouScreen::Request,
            Screen::Task => NeedsYouScreen::Task,
            Screen::Repository => NeedsYouScreen::Repository,
            Screen::Settings => NeedsYouScreen::Settings,
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
            humans: item.humans.iter().map(ToString::to_string).collect(),
            choices: item.choices.into_iter().map(MessageChoice::from).collect(),
            header: item.header,
            questions: item.questions,
            title: item.title,
            reason: item.reason,
            commit: item.commit,
            paths: item.paths,
            target: item.target.map(|target| target.to_string()),
            agents: item.agents.iter().map(ToString::to_string).collect(),
            activity: item.activity,
            limit: item.limit.map(UsageLimitKind::from),
            reset: item.reset,
            resets_at: item.resets_at,
            actions: item.actions.into_iter().map(Into::into).collect(),
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

/// Every item that waits for a human, oldest first: open asks from agents to a human (messages marked as asking, questions with choices and `AskUserQuestion` calls), workers' permission questions, proposals waiting for a human approval, card drafts waiting for their human requester, a Git branch of the project folder that fold did not move, a project folder that waits on changes of the human's that no revision records, which a waiting conflicted human change stands for while it waits, an unstash whose conflict markers wait in the folder, which stands for the folder too, a worker that failed because it cannot reach fold, a worker whose turns fail before they reach the model, the turns that could not reach the model at all and those that reached a usage limit, which fold runs again, a notice target fold turned off, which a person's change of the target or a test send that gets through ends, and the files that changed in the project folder during supervised workers' tool calls, whose paths leave the waiting folder's item. Each item names the screen that answers it. The list changes with the stored events `message.sent`, `message.ask_closed`, `worker.permission_asked`, `worker.permission_answered`, `worker.stopped`, `worker.failed`, `review.submitted`, `task.human_review_set`, `task.human_review_cleared`, `proposal.*`, `request.*`, `git.head_moved`, `git.uncommitted_changes`, `git.attention_cleared`, `stash.conflicted`, `stash.dropped`, `stash.discarded`, `worker.started`, `worker.resumed`, `worker.refreshed` and `worker.model_unreachable`, `notify.target_changed`, `folder.agent_changed`, `folder.agent_change_cleared`, `worker.waiting` and `worker.wait_ended`, and with the live `worker.output` steps `result` and `result_error`, which end a turn, and with the live `workspace.changed` whose `stalled` changed, which starts or ends a `worker_stalled` item.
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
pub async fn list_needs_you(
    State(state): State<AppState>,
    project: Project,
) -> Result<Json<NeedsYouList>, ApiError> {
    let mut items = with_folder_attention(
        project.0.agents().needs_you().await?,
        project.0.git_attention.head_moved(),
        project.0.git_attention.folder_held(),
    );
    if let Some(settings) = state.projects.notify() {
        items = notify::with_notify_attention(
            items,
            notify::off_items(settings, &project.0.slug).await?,
        );
    }
    Ok(Json(NeedsYouList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

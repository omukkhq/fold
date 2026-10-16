use std::str::FromStr;

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use fold_core::{
    ChecklistItemId, ChecklistName, FoldError, IdParseError, LabelColor, LabelName, StepText,
    TaskDescription, TaskId, TaskTitle,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::actor::Human;
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::events::DurableEventType;
use super::reviews::ReviewVerdict;
use crate::daemon::AppState;
use crate::services::tasks::{self as service, ItemContent};

pub mod requests;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(requests::routes())
        .routes(routes!(board))
        .routes(routes!(list_tasks, create_task))
        .routes(routes!(get_task, update_task, delete_task))
        .routes(routes!(archive_task))
        .routes(routes!(add_dependency))
        .routes(routes!(remove_dependency))
        .routes(routes!(add_checklist_item))
        .routes(routes!(update_checklist_item, remove_checklist_item))
        .routes(routes!(list_labels, create_label))
}

/// A calculated board status; it is also the board's column key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Backlog,
    Todo,
    InProgress,
    Review,
    Done,
}

impl From<fold_core::TaskStatus> for TaskStatus {
    fn from(status: fold_core::TaskStatus) -> Self {
        match status {
            fold_core::TaskStatus::Backlog => Self::Backlog,
            fold_core::TaskStatus::Todo => Self::Todo,
            fold_core::TaskStatus::InProgress => Self::InProgress,
            fold_core::TaskStatus::Review => Self::Review,
            fold_core::TaskStatus::Done => Self::Done,
        }
    }
}

/// Why a task needs a person. The daemon computes every kind; clients derive none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Question,
    HumanReview,
    Stalled,
    StaleClaim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct AttentionItem {
    pub kind: AttentionKind,
}

impl From<service::AttentionKind> for AttentionKind {
    fn from(kind: service::AttentionKind) -> Self {
        match kind {
            service::AttentionKind::Question => Self::Question,
            service::AttentionKind::HumanReview => Self::HumanReview,
            service::AttentionKind::Stalled => Self::Stalled,
            service::AttentionKind::StaleClaim => Self::StaleClaim,
        }
    }
}

impl From<service::AttentionItem> for AttentionItem {
    fn from(item: service::AttentionItem) -> Self {
        Self {
            kind: item.kind.into(),
        }
    }
}

pub(super) fn attention(items: Vec<service::AttentionItem>) -> Vec<AttentionItem> {
    items.into_iter().map(AttentionItem::from).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Label {
    pub name: String,
    /// One of sage, slate, teal, ink, gold, moss, plum, stone.
    #[schema(example = "sage")]
    pub color: String,
}

impl From<service::LabelView> for Label {
    fn from(label: service::LabelView) -> Self {
        Self {
            name: label.name.to_string(),
            color: label.color.to_string(),
        }
    }
}

fn labels(labels: Vec<service::LabelView>) -> Vec<Label> {
    labels.into_iter().map(Label::from).collect()
}

/// Plain steps and linked items counted apart, over every checklist of the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct ChecklistCounts {
    pub steps_done: u32,
    pub steps_total: u32,
    pub linked_done: u32,
    pub linked_total: u32,
}

impl From<service::ChecklistCounts> for ChecklistCounts {
    fn from(counts: service::ChecklistCounts) -> Self {
        Self {
            steps_done: counts.steps_done,
            steps_total: counts.steps_total,
            linked_done: counts.linked_done,
            linked_total: counts.linked_total,
        }
    }
}

/// The task's request that a human approves its proposal before it integrates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct HumanReviewMark {
    /// The named human whose approval integration waits for; null when any human may approve.
    #[schema(required = true)]
    pub reviewer: Option<String>,
    /// Why a human must look.
    pub reason: String,
    /// The worker or human that set the mark, such as `planner` or `human:tanim`.
    pub set_by: String,
}

impl From<service::HumanReviewMark> for HumanReviewMark {
    fn from(mark: service::HumanReviewMark) -> Self {
        Self {
            reviewer: mark.reviewer.map(|name| name.to_string()),
            reason: mark.reason,
            set_by: mark.set_by.to_string(),
        }
    }
}

fn ids(ids: &[TaskId]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct BoardCard {
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    pub status: TaskStatus,
    pub blocked: bool,
    pub labels: Vec<Label>,
    pub checklist: ChecklistCounts,
    /// Every direct prerequisite, as public task ids.
    pub needs: Vec<String>,
    /// The direct prerequisites that are not Done.
    pub blocked_by: Vec<String>,
    /// How many open tasks wait directly for this one; 0 once it is Done.
    pub blocks: u32,
    /// The worker holding the implementation claim, `agent` or `agent/worker`.
    #[schema(required = true)]
    pub worker: Option<String>,
    /// The live task workspace.
    #[schema(required = true, example = "ws-9")]
    pub workspace: Option<String>,
    #[schema(required = true)]
    pub human_review: Option<HumanReviewMark>,
    /// How many messages name the task.
    pub messages: u32,
    /// UTC milliseconds of the completion record; null until the task is Done.
    #[schema(required = true)]
    pub completed_at: Option<i64>,
    pub attention: Vec<AttentionItem>,
    /// The task's open proposal, or the integrated one once it is Done.
    #[schema(required = true)]
    pub proposal: Option<BoardProposal>,
    /// The last tracked edit in the live workspace.
    #[schema(required = true)]
    pub activity: Option<BoardActivity>,
}

/// Where a proposal stands, as the pill on a card and the block of a task draw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Open,
    Approved,
    ChangesRequested,
    Conflict,
    Integrated,
}

impl From<fold_core::reviews::ProposalStanding> for ProposalState {
    fn from(standing: fold_core::reviews::ProposalStanding) -> Self {
        use fold_core::reviews::ProposalStanding as S;
        match standing {
            S::Open => Self::Open,
            S::Approved => Self::Approved,
            S::ChangesRequested => Self::ChangesRequested,
            S::Conflict => Self::Conflict,
            S::Integrated => Self::Integrated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct BoardProposal {
    #[schema(example = "P-3")]
    pub id: String,
    pub state: ProposalState,
    /// The revision an integrated proposal became; null before integration.
    #[schema(required = true, example = "r8")]
    pub revision: Option<String>,
}

impl From<service::CardProposal> for BoardProposal {
    fn from(proposal: service::CardProposal) -> Self {
        Self {
            id: proposal.id.to_string(),
            state: proposal.standing.into(),
            revision: proposal.revision.map(|revision| revision.to_string()),
        }
    }
}

/// The last tracked edit in the task's workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct BoardActivity {
    /// Relative to the workspace root; null when more than one file changed.
    #[schema(required = true)]
    pub path: Option<String>,
    /// UTC milliseconds.
    pub at: i64,
}

impl From<service::Activity> for BoardActivity {
    fn from(activity: service::Activity) -> Self {
        Self {
            path: activity.path,
            at: activity.at.as_millis(),
        }
    }
}

impl From<service::BoardCard> for BoardCard {
    fn from(card: service::BoardCard) -> Self {
        Self {
            task: card.id.to_string(),
            title: card.title.to_string(),
            status: card.status.into(),
            blocked: card.blocked,
            labels: labels(card.labels),
            checklist: card.checklist.into(),
            needs: ids(&card.needs),
            blocked_by: ids(&card.blocked_by),
            blocks: card.blocks,
            worker: card.holder.map(|worker| worker.to_string()),
            workspace: card.workspace.map(|workspace| workspace.to_string()),
            human_review: card.human_review.map(HumanReviewMark::from),
            messages: card.messages,
            completed_at: card.completed_at.map(|at| at.as_millis()),
            attention: attention(card.attention),
            proposal: card.proposal.map(BoardProposal::from),
            activity: card.activity.map(BoardActivity::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct BoardColumn {
    pub status: TaskStatus,
    /// Backlog to Review hold every task in number order. Done holds the most recently completed tasks, newest first.
    pub cards: Vec<BoardCard>,
    /// Every task with this status. In the Done column it can exceed the number of cards.
    pub total: u32,
}

/// The five columns in board order: backlog, todo, in_progress, review, done. Archived tasks are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct BoardView {
    pub columns: Vec<BoardColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskSummary {
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    pub status: TaskStatus,
    pub blocked: bool,
    pub labels: Vec<Label>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskList {
    /// In number order.
    pub tasks: Vec<TaskSummary>,
}

/// A checklist item inside a task: a plain step, or a linked child task that fold ticks when the child is Done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChecklistItem {
    Step {
        #[schema(example = "ci-4")]
        id: String,
        text: String,
        done: bool,
    },
    Linked {
        #[schema(example = "ci-5")]
        id: String,
        /// The child task.
        #[schema(example = "T-13")]
        task: String,
        title: String,
        status: TaskStatus,
        done: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Checklist {
    pub name: String,
    pub items: Vec<ChecklistItem>,
}

/// `needs`: this task needs the other task. `needed_by`: the other task needs this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDirection {
    Needs,
    NeededBy,
}

/// A `checklist` edge belongs to a linked checklist item and goes only with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    Direct,
    Checklist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskEdge {
    /// The task at the other end of the edge.
    #[schema(example = "T-3")]
    pub task: String,
    pub title: String,
    pub status: TaskStatus,
    pub direction: EdgeDirection,
    pub origin: EdgeOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Implementation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskClaim {
    pub kind: ClaimKind,
    /// The worker, `agent` or `agent/worker`.
    pub holder: String,
    /// UTC milliseconds.
    pub claimed_at: i64,
    /// UTC milliseconds of the last lease renewal.
    pub renewed_at: i64,
    /// True once the lease ran out without a renewal.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskView {
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    /// Markdown.
    pub description: String,
    pub status: TaskStatus,
    pub blocked: bool,
    /// UTC milliseconds; null while the task is on the board.
    #[schema(required = true)]
    pub archived_at: Option<i64>,
    /// The worker or human that created the task.
    pub created_by: String,
    /// UTC milliseconds.
    pub created_at: i64,
    /// UTC milliseconds.
    pub updated_at: i64,
    pub labels: Vec<Label>,
    /// The first checklist is `Subtasks` when the task was split.
    pub checklists: Vec<Checklist>,
    pub progress: ChecklistCounts,
    /// Prerequisites and dependents together; `direction` tells them apart.
    pub dependencies: Vec<TaskEdge>,
    pub claims: Vec<TaskClaim>,
    /// The live task workspace.
    #[schema(required = true, example = "ws-9")]
    pub workspace: Option<String>,
    #[schema(required = true)]
    pub human_review: Option<HumanReviewMark>,
    /// When the human review mark was last set, in UTC milliseconds; null without a mark.
    #[schema(required = true)]
    pub human_review_set_at: Option<i64>,
    pub attention: Vec<AttentionItem>,
    /// The rule of the status calculation that decided the status, with the ids it used.
    pub status_reason: String,
    /// The live task workspace; null without one.
    #[schema(required = true)]
    pub workspace_info: Option<TaskWorkspaceInfo>,
    /// The last tracked edit in the live workspace.
    #[schema(required = true)]
    pub activity: Option<BoardActivity>,
    /// Of the live workspace, or of the latest one when none is live; newest first.
    pub changesets: Vec<TaskChangeset>,
    /// The open proposal, or the integrated one once it is Done.
    #[schema(required = true)]
    pub proposal: Option<TaskProposal>,
    /// The integration that published the proposal; null until the task is integrated.
    #[schema(required = true)]
    pub integration: Option<TaskIntegration>,
    /// The task's newest stored events, newest first, at most 50.
    pub history: Vec<TaskEvent>,
    /// Whether the human who made the request may turn the human review mark on, change it or clear it. False without a named human.
    pub can_change_human_review: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskWorkspaceInfo {
    /// Relative to the project root.
    pub directory: String,
    /// The revision the workspace is based on.
    #[schema(example = "r7")]
    pub base: String,
    /// The main head.
    #[schema(example = "r8")]
    pub main: String,
    /// How many revisions main moved since `base`.
    pub behind: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskChangeset {
    #[schema(example = "C-18")]
    pub id: String,
    pub message: String,
    /// UTC milliseconds.
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskReview {
    /// A worker or `human:<name>`.
    pub reviewer: String,
    pub verdict: ReviewVerdict,
    /// The proposal version the review belongs to.
    pub version: u32,
    /// UTC milliseconds.
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskProposal {
    #[schema(example = "P-3")]
    pub id: String,
    pub state: ProposalState,
    /// The current version.
    pub version: u32,
    /// Newest first.
    pub reviews: Vec<TaskReview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskIntegration {
    /// The revision the integration published.
    #[schema(example = "r8")]
    pub revision: String,
    #[schema(example = "P-3")]
    pub proposal: String,
    /// A worker or `human:<name>`.
    pub by: String,
    /// UTC milliseconds.
    pub at: i64,
    /// The Git commit the revision became in the project folder's repository `.git`; null until it is written, just after the integration.
    #[schema(required = true)]
    pub commit: Option<String>,
}

/// A stored event of the task as its history lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskEvent {
    pub seq: u64,
    #[serde(rename = "type")]
    #[schema(rename = "type", value_type = DurableEventType)]
    pub event_type: String,
    /// A worker, `human:<name>` or `fold`.
    pub actor: String,
    /// UTC milliseconds.
    pub created_at: i64,
}

/// The task read for `human`, the caller of the route; `None` when the request names no human.
pub(super) fn task_view(detail: service::TaskDetail, human: Option<&Human>) -> TaskView {
    let mut view = TaskView::from(detail);
    view.can_change_human_review =
        human.is_some_and(|human| service::can_change_human_review(&human.actor()));
    view
}

fn edge(edge: service::DetailEdge, direction: EdgeDirection) -> TaskEdge {
    TaskEdge {
        task: edge.task.id.to_string(),
        title: edge.task.title.to_string(),
        status: edge.task.status.into(),
        direction,
        origin: match edge.origin {
            fold_core::DependencyOrigin::Direct => EdgeOrigin::Direct,
            fold_core::DependencyOrigin::Checklist => EdgeOrigin::Checklist,
        },
    }
}

fn checklist_item(entry: service::DetailItem) -> Option<ChecklistItem> {
    let item = entry.item;
    let done = item.done_at.is_some();
    let id = item.id.to_string();
    match item.content {
        ItemContent::Step(text) => Some(ChecklistItem::Step {
            id,
            text: text.to_string(),
            done,
        }),
        ItemContent::Linked(_) => entry.linked.map(|linked| ChecklistItem::Linked {
            id,
            task: linked.id.to_string(),
            title: linked.title.to_string(),
            status: linked.status.into(),
            done,
        }),
    }
}

impl From<service::TaskDetail> for TaskView {
    fn from(detail: service::TaskDetail) -> Self {
        let task = detail.task;
        let dependencies = detail
            .needs
            .into_iter()
            .map(|e| edge(e, EdgeDirection::Needs))
            .chain(
                detail
                    .needed_by
                    .into_iter()
                    .map(|e| edge(e, EdgeDirection::NeededBy)),
            )
            .collect();
        Self {
            task: task.id.to_string(),
            title: task.title.to_string(),
            description: task.description.as_str().to_owned(),
            status: task.status.into(),
            blocked: task.blocked,
            archived_at: task.archived_at.map(|at| at.as_millis()),
            created_by: task.created_by,
            created_at: task.created_at.as_millis(),
            updated_at: task.updated_at.as_millis(),
            labels: labels(task.labels),
            checklists: detail
                .checklists
                .into_iter()
                .map(|list| Checklist {
                    name: list.name.to_string(),
                    items: list.items.into_iter().filter_map(checklist_item).collect(),
                })
                .collect(),
            progress: detail.progress.into(),
            dependencies,
            claims: detail
                .claim
                .into_iter()
                .map(|claim| TaskClaim {
                    kind: ClaimKind::Implementation,
                    holder: claim.holder.to_string(),
                    claimed_at: claim.acquired_at.as_millis(),
                    renewed_at: claim.renewed_at.as_millis(),
                    stale: claim.stale,
                })
                .collect(),
            workspace: detail.workspace.map(|workspace| workspace.to_string()),
            human_review: task.human_review.map(HumanReviewMark::from),
            attention: attention(detail.attention),
            human_review_set_at: detail.human_review_set_at.map(|at| at.as_millis()),
            status_reason: detail.status_reason,
            workspace_info: detail.workspace_info.map(|info| TaskWorkspaceInfo {
                directory: info.directory,
                base: info.base.to_string(),
                main: info.main.to_string(),
                behind: info.behind,
            }),
            activity: detail.activity.map(BoardActivity::from),
            changesets: detail
                .changesets
                .into_iter()
                .map(|changeset| TaskChangeset {
                    id: changeset.id.to_string(),
                    message: changeset.message,
                    created_at: changeset.created_at.as_millis(),
                })
                .collect(),
            proposal: detail.proposal.map(|proposal| TaskProposal {
                id: proposal.id.to_string(),
                state: proposal.standing.into(),
                version: proposal.version,
                reviews: proposal
                    .reviews
                    .into_iter()
                    .map(|review| TaskReview {
                        reviewer: review.reviewer.to_string(),
                        verdict: review.verdict.into(),
                        version: review.version,
                        at: review.at.as_millis(),
                    })
                    .collect(),
            }),
            integration: detail.integration.map(|integration| TaskIntegration {
                revision: integration.revision.to_string(),
                proposal: integration.proposal.to_string(),
                by: integration.by,
                at: integration.at.as_millis(),
                commit: integration.commit,
            }),
            history: detail
                .history
                .into_iter()
                .map(|event| TaskEvent {
                    seq: event.seq,
                    event_type: event.event_type.as_str().to_owned(),
                    actor: event.actor.to_string(),
                    created_at: event.created_at.as_millis(),
                })
                .collect(),
            can_change_human_review: false,
        }
    }
}

/// A checklist item as its own write returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ChecklistItemView {
    #[schema(example = "ci-4")]
    pub id: String,
    /// The task that owns the item.
    #[schema(example = "T-12")]
    pub task: String,
    pub checklist: String,
    /// The text of a plain step; null for a linked item.
    #[schema(required = true)]
    pub text: Option<String>,
    /// The child of a linked item; null for a plain step.
    #[schema(required = true, example = "T-13")]
    pub linked_task: Option<String>,
    pub done: bool,
}

impl From<service::ChecklistItemView> for ChecklistItemView {
    fn from(item: service::ChecklistItemView) -> Self {
        let (text, linked_task) = match item.content {
            ItemContent::Step(text) => (Some(text.to_string()), None),
            ItemContent::Linked(child) => (None, Some(child.to_string())),
        };
        Self {
            id: item.id.to_string(),
            task: item.task.to_string(),
            checklist: item.checklist.to_string(),
            text,
            linked_task,
            done: item.done_at.is_some(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTaskBody {
    /// One line, not empty.
    pub title: String,
    /// Markdown; empty when left out.
    #[serde(default)]
    pub description: Option<String>,
    /// Names of existing labels.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Tasks the new task needs; each adds one direct edge.
    #[serde(default)]
    pub needs: Vec<String>,
    /// The task to split: the new task becomes a linked item of its `Subtasks` checklist.
    #[serde(default)]
    pub parent: Option<String>,
}

/// Fields left out stay as they are.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTaskBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The whole new label set, by name.
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DependencyBody {
    /// The task this task needs.
    #[schema(example = "T-3")]
    pub needs: String,
}

/// Exactly one of `text` and `linked_task`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NewChecklistItemBody {
    /// The text of a plain step.
    #[serde(default)]
    pub text: Option<String>,
    /// A child task to link; the task then needs it.
    #[serde(default)]
    pub linked_task: Option<String>,
    /// The checklist name; the task's first checklist when left out. A new name creates the checklist.
    #[serde(default)]
    pub checklist: Option<String>,
}

/// Either an edit (`text`, `done`) or a move (`position`, `checklist`), never both in one request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChecklistItemPatchBody {
    /// The new text of a plain step.
    #[serde(default)]
    pub text: Option<String>,
    /// Ticks or unticks a plain step.
    #[serde(default)]
    pub done: Option<bool>,
    /// Moves the item to this place, counted from 0; past the end puts it last.
    #[serde(default)]
    pub position: Option<u32>,
    /// Moves the item into this checklist of the same task, last unless `position` is given.
    #[serde(default)]
    pub checklist: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateLabelBody {
    pub name: String,
    /// One of sage, slate, teal, ink, gold, moss, plum, stone.
    #[schema(example = "sage")]
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LabelList {
    /// In name order.
    pub labels: Vec<Label>,
}

pub(super) fn id<T>(text: &str) -> Result<T, ApiError>
where
    T: FromStr<Err = IdParseError>,
{
    text.parse()
        .map_err(|error: IdParseError| FoldError::from(error).into())
}

fn label_names(names: &[String]) -> Result<Vec<LabelName>, ApiError> {
    names
        .iter()
        .map(|name| LabelName::parse(name).map_err(|error| FoldError::from(error).into()))
        .collect()
}

fn fold<T, E: Into<FoldError>>(result: Result<T, E>) -> Result<T, ApiError> {
    result.map_err(|error| ApiError(error.into()))
}

/// The board read model.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/board",
    operation_id = "get_board",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The five columns in order", body = BoardView),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn board(project: Project) -> Result<Json<BoardView>, ApiError> {
    let board = project.0.tasks.board().await?;
    Ok(Json(BoardView {
        columns: board
            .columns
            .into_iter()
            .map(|column| BoardColumn {
                status: column.status.into(),
                cards: column.cards.into_iter().map(BoardCard::from).collect(),
                total: column.total,
            })
            .collect(),
    }))
}

/// Every task that is not archived.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/tasks",
    operation_id = "list_tasks",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The tasks in number order", body = TaskList),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_tasks(project: Project) -> Result<Json<TaskList>, ApiError> {
    let tasks = project
        .0
        .tasks
        .list(false)
        .await?
        .into_iter()
        .map(|task| TaskSummary {
            task: task.id.to_string(),
            title: task.title.to_string(),
            status: task.status.into(),
            blocked: task.blocked,
            labels: labels(task.labels),
        })
        .collect();
    Ok(Json(TaskList { tasks }))
}

/// Creates a task with its edges and its parent link in one transaction; a refused edge creates nothing.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/tasks",
    operation_id = "create_task",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = CreateTaskBody,
    responses(
        (status = 201, description = "The created task", body = TaskView),
        (status = 400, description = "`invalid_request`, `invalid_id`, `invalid_name`, `invalid_task_title` or `invalid_label_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `label_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`dependency_cycle` or `task_closed`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_task(
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<CreateTaskBody>,
) -> Result<(StatusCode, Json<TaskView>), ApiError> {
    let create = service::CreateTask {
        title: fold(TaskTitle::parse(&body.title))?,
        description: TaskDescription::new(body.description.unwrap_or_default()),
        labels: label_names(&body.labels)?,
    };
    let needs = body
        .needs
        .iter()
        .map(|need| id(need))
        .collect::<Result<Vec<TaskId>, _>>()?;
    let parent = body.parent.as_deref().map(id).transpose()?;
    let tasks = &project.0.tasks;
    let created = tasks
        .create_linked_task(&human.actor(), create, &needs, parent)
        .await?;
    let view = tasks.get_view(created.id).await?;
    Ok((StatusCode::CREATED, Json(task_view(view, Some(&human)))))
}

/// One task with its checklists, dependencies, claim and workspace. An archived task is found too.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/tasks/{id}",
    operation_id = "get_task",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The task", body = TaskView),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`invalid_config`: the task has a claim and the project configuration that sets its lease is invalid", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_task(
    project: Project,
    human: Result<Human, ApiError>,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    let view = project.0.tasks.get_view(id(&task)?).await?;
    Ok(Json(task_view(view, human.ok().as_ref())))
}

/// Changes the title, the description or the label set.
#[utoipa::path(
    patch,
    path = "/api/v1/projects/{slug}/tasks/{id}",
    operation_id = "update_task",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    request_body = UpdateTaskBody,
    responses(
        (status = 200, description = "The updated task", body = TaskView),
        (status = 400, description = "`invalid_request`, `invalid_id`, `invalid_name`, `invalid_task_title` or `invalid_label_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `label_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn update_task(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<UpdateTaskBody>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    let patch = service::TaskPatch {
        title: fold(body.title.as_deref().map(TaskTitle::parse).transpose())?,
        description: body.description.map(TaskDescription::new),
        labels: body.labels.as_deref().map(label_names).transpose()?,
    };
    let tasks = &project.0.tasks;
    tasks.update_task(&human.actor(), task, patch).await?;
    Ok(Json(task_view(tasks.get_view(task).await?, Some(&human))))
}

/// Deletes a task that never had a workspace, a proposal, a completion or a message.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/tasks/{id}",
    operation_id = "delete_task",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 204, description = "The task is deleted"),
        (status = 400, description = "`invalid_request` with `details.reason` `task_has_history`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn delete_task(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    project
        .0
        .tasks
        .delete_task(&human.actor(), id(&task)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Hides a task from the board and removes its edges, keeping its history.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/tasks/{id}/archive",
    operation_id = "archive_task",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 204, description = "The task is archived"),
        (status = 400, description = "`invalid_request` with `details.reason` `proposal_open`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is already archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn archive_task(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    project
        .0
        .tasks
        .archive_task(&human.actor(), id(&task)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Makes the task need another task.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/tasks/{id}/dependencies",
    operation_id = "add_dependency",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task that waits, such as T-12"),
    ),
    request_body = DependencyBody,
    responses(
        (status = 204, description = "The edge is added"),
        (status = 400, description = "`invalid_request` with `details.reason` `self_dependency`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`dependency_cycle` with the cycle in `details.path`, `dependency_exists` or `task_closed`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn add_dependency(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<DependencyBody>,
) -> Result<StatusCode, ApiError> {
    project
        .0
        .tasks
        .add_dependency(&human.actor(), id(&task)?, id(&body.needs)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a direct edge. An edge of a linked checklist item goes only with its item.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/tasks/{id}/dependencies/{needs}",
    operation_id = "remove_dependency",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task that waits, such as T-12"),
        ("needs" = String, Path, description = "The task it needs, such as T-3"),
    ),
    responses(
        (status = 204, description = "The edge is removed"),
        (status = 400, description = "`invalid_request` with `details.reason` `checklist_origin`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`dependency_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn remove_dependency(
    human: Human,
    project: Project,
    Path((_slug, task, needs)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    project
        .0
        .tasks
        .remove_dependency(&human.actor(), id(&task)?, id(&needs)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Adds a plain step or a linked child task to a checklist of the task.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/tasks/{id}/checklist-items",
    operation_id = "add_checklist_item",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    request_body = NewChecklistItemBody,
    responses(
        (status = 201, description = "The new item", body = ChecklistItemView),
        (status = 400, description = "`invalid_request`, `invalid_id`, `invalid_name`, `invalid_step_text` or `invalid_checklist_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`dependency_cycle`, `dependency_exists` or `task_closed`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn add_checklist_item(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<NewChecklistItemBody>,
) -> Result<(StatusCode, Json<ChecklistItemView>), ApiError> {
    let content = match (body.text, body.linked_task) {
        (Some(text), None) => ItemContent::Step(fold(StepText::parse(&text))?),
        (None, Some(child)) => ItemContent::Linked(id(&child)?),
        _ => {
            return Err(ApiError::invalid_request(
                "give text or linked_task, not both",
            ));
        }
    };
    let item = service::NewChecklistItem {
        content,
        checklist: fold(
            body.checklist
                .as_deref()
                .map(ChecklistName::parse)
                .transpose(),
        )?,
    };
    let view = project
        .0
        .tasks
        .add_checklist_item(&human.actor(), id(&task)?, item)
        .await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

/// Edits or ticks a plain step, or moves an item inside its task. One request is either an edit or a move.
#[utoipa::path(
    patch,
    path = "/api/v1/projects/{slug}/checklist-items/{id}",
    operation_id = "update_checklist_item",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The checklist item id, such as ci-4"),
    ),
    request_body = ChecklistItemPatchBody,
    responses(
        (status = 200, description = "The item after the change", body = ChecklistItemView),
        (status = 400, description = "`invalid_request` (also with `details.reason` `linked_item`), `invalid_id`, `invalid_name`, `invalid_step_text` or `invalid_checklist_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`checklist_item_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn update_checklist_item(
    human: Human,
    project: Project,
    Path((_slug, item)): Path<(String, String)>,
    ApiJson(body): ApiJson<ChecklistItemPatchBody>,
) -> Result<Json<ChecklistItemView>, ApiError> {
    let item: ChecklistItemId = id(&item)?;
    let tasks = &project.0.tasks;
    let actor = human.actor();
    let edit = body.text.is_some() || body.done.is_some();
    let moves = body.position.is_some() || body.checklist.is_some();
    let view = match (edit, moves) {
        (true, true) => {
            return Err(ApiError::invalid_request(
                "a request either edits an item or moves it",
            ));
        }
        (_, true) => {
            let checklist = fold(
                body.checklist
                    .as_deref()
                    .map(ChecklistName::parse)
                    .transpose(),
            )?;
            let position = body.position.unwrap_or(u32::MAX);
            tasks
                .move_checklist_item(&actor, item, position, checklist)
                .await?
        }
        (_, false) => {
            let patch = service::ChecklistItemPatch {
                text: fold(body.text.as_deref().map(StepText::parse).transpose())?,
                done: body.done,
            };
            tasks.update_checklist_item(&actor, item, patch).await?
        }
    };
    Ok(Json(view.into()))
}

/// Removes an item; a linked item takes its edge with it.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/checklist-items/{id}",
    operation_id = "remove_checklist_item",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The checklist item id, such as ci-4"),
    ),
    responses(
        (status = 204, description = "The item is removed"),
        (status = 400, description = "`invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`checklist_item_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn remove_checklist_item(
    human: Human,
    project: Project,
    Path((_slug, item)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    project
        .0
        .tasks
        .remove_checklist_item(&human.actor(), id(&item)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every label of the project.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/labels",
    operation_id = "list_labels",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The labels in name order", body = LabelList),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_labels(project: Project) -> Result<Json<LabelList>, ApiError> {
    let labels = labels(project.0.tasks.labels().await?);
    Ok(Json(LabelList { labels }))
}

/// Creates a label that tasks can carry.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/labels",
    operation_id = "create_label",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = CreateLabelBody,
    responses(
        (status = 201, description = "The created label", body = Label),
        (status = 400, description = "`invalid_request`, `invalid_name`, `invalid_label_name` or `invalid_label_color`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`label_exists`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_label(
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<CreateLabelBody>,
) -> Result<(StatusCode, Json<Label>), ApiError> {
    let name = fold(LabelName::parse(&body.name))?;
    let color = fold(body.color.parse::<LabelColor>())?;
    let label = project
        .0
        .tasks
        .create_label(&human.actor(), name, color)
        .await?;
    Ok((StatusCode::CREATED, Json(label.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_and_attention_kind_has_its_wire_name() {
        for status in fold_core::TaskStatus::ALL {
            let wire = serde_json::to_value(TaskStatus::from(*status)).unwrap();
            assert_eq!(wire, status.as_str());
        }
        for kind in [
            service::AttentionKind::Question,
            service::AttentionKind::HumanReview,
            service::AttentionKind::Stalled,
            service::AttentionKind::StaleClaim,
        ] {
            let wire =
                serde_json::to_value(AttentionItem::from(service::AttentionItem { kind })).unwrap();
            assert_eq!(wire["kind"], kind.as_str());
        }
    }
}

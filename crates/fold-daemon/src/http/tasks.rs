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
use super::error::{ApiError, ApiJson, ErrorEnvelope, fold};
use super::events::{CheckpointSummary, DurableEventType};
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
        .routes(routes!(task_readers))
        .routes(routes!(attach_label, detach_label))
        .routes(routes!(archive_task))
        .routes(routes!(complete_task))
        .routes(routes!(send_back_task))
        .routes(routes!(set_blocker, clear_blocker))
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

/// How a task's work reaches Done: `proposal` through the integration of its proposal, `no_proposal` through a lead check of its task worker's report, `human` when a person marks the work Done; no agent takes a human task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Proposal,
    NoProposal,
    Human,
}

impl From<fold_core::TaskKind> for TaskKind {
    fn from(kind: fold_core::TaskKind) -> Self {
        match kind {
            fold_core::TaskKind::Proposal => Self::Proposal,
            fold_core::TaskKind::NoProposal => Self::NoProposal,
            fold_core::TaskKind::Human => Self::Human,
        }
    }
}

impl From<TaskKind> for fold_core::TaskKind {
    fn from(kind: TaskKind) -> Self {
        match kind {
            TaskKind::Proposal => Self::Proposal,
            TaskKind::NoProposal => Self::NoProposal,
            TaskKind::Human => Self::Human,
        }
    }
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
    /// For a `question`: one of the task's open asks is closable, so a human can mark it answered with the asks route, because it is no worker's `AskUserQuestion`, which only a reply in its thread ends. False for every other kind.
    pub closable: bool,
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
            closable: item.closable,
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

/// The blocker of a task: while it is set, no agent claims the task and no lead starts a task worker for it. It never changes the status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct BlockerMark {
    /// Why the task waits for a person.
    pub reason: String,
    /// The worker or human that set it, such as `web-lead` or `human:tanim`.
    pub set_by: String,
    /// UTC milliseconds.
    pub set_at: i64,
}

impl From<fold_core::HoldMark> for BlockerMark {
    fn from(mark: fold_core::HoldMark) -> Self {
        Self {
            reason: mark.reason.to_string(),
            set_by: mark.set_by.to_string(),
            set_at: mark.set_at.as_millis(),
        }
    }
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
    pub kind: TaskKind,
    /// The note of the task worker's open report on a `no_proposal` task, which waits for a lead check; null otherwise.
    #[schema(required = true)]
    pub report: Option<String>,
    pub labels: Vec<Label>,
    /// True when the task carries the labels of two or more areas. Every write refuses such a label set, but the database can hold one; such a task's title and description change freely, and each change of its labels must leave at most one area label.
    pub areas_conflict: bool,
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
    /// The blocker; null while the task has none.
    #[schema(required = true)]
    pub blocker: Option<BlockerMark>,
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
    /// Whether the files of the live workspace differ from the tree of the open proposal's newest version: false while every file is the same, so the holder sent its work and waits, true from the first file that differs. Null without an open proposal or before the live state compared them.
    #[schema(required = true)]
    pub edits_after_version: Option<bool>,
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
    /// The current version.
    pub version: u32,
    /// The integration checks of the current version failed, so the proposal is back with its author until a new version.
    pub checks_failed: bool,
    /// The revision an integrated proposal became; null before integration.
    #[schema(required = true, example = "r8")]
    pub revision: Option<String>,
}

impl From<service::CardProposal> for BoardProposal {
    fn from(proposal: service::CardProposal) -> Self {
        Self {
            id: proposal.id.to_string(),
            state: proposal.standing.into(),
            version: proposal.version,
            checks_failed: proposal.checks_failed,
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
            kind: card.kind.into(),
            report: card.report,
            labels: labels(card.labels),
            areas_conflict: card.areas_conflict,
            checklist: card.checklist.into(),
            needs: ids(&card.needs),
            blocked_by: ids(&card.blocked_by),
            blocks: card.blocks,
            worker: card.holder.map(|worker| worker.to_string()),
            workspace: card.workspace.map(|workspace| workspace.to_string()),
            human_review: card.human_review.map(HumanReviewMark::from),
            blocker: card.hold.map(BlockerMark::from),
            messages: card.messages,
            completed_at: card.completed_at.map(|at| at.as_millis()),
            attention: attention(card.attention),
            proposal: card.proposal.map(BoardProposal::from),
            activity: card.activity.map(BoardActivity::from),
            edits_after_version: card.edits_after_version,
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
    /// True when the task carries the labels of two or more areas. Every write refuses such a label set, but the database can hold one; such a task's title and description change freely, and each change of its labels must leave at most one area label.
    pub areas_conflict: bool,
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
    pub kind: TaskKind,
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
    /// True when the task carries the labels of two or more areas. Every write refuses such a label set, but the database can hold one; such a task's title and description change freely, and each change of its labels must leave at most one area label.
    pub areas_conflict: bool,
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
    /// The blocker; null while the task has none.
    #[schema(required = true)]
    pub blocker: Option<BlockerMark>,
    pub attention: Vec<AttentionItem>,
    /// The rule of the status calculation that decided the status, with the ids it used.
    pub status_reason: String,
    /// The live task workspace; null without one.
    #[schema(required = true)]
    pub workspace_info: Option<TaskWorkspaceInfo>,
    /// The last tracked edit in the live workspace.
    #[schema(required = true)]
    pub activity: Option<BoardActivity>,
    /// Whether the files of the live workspace differ from the tree of the open proposal's newest version: false while every file is the same, so the holder sent its work and waits, true from the first file that differs. Null without an open proposal or before the live state compared them.
    #[schema(required = true)]
    pub edits_after_version: Option<bool>,
    /// Of the live workspace, or of the latest one when none is live; newest first.
    pub changesets: Vec<TaskChangeset>,
    /// Of the live workspace, or of the latest one when none is live; pruned ones left out, newest first.
    pub checkpoints: Vec<CheckpointSummary>,
    /// The open proposal, or the integrated one once it is Done.
    #[schema(required = true)]
    pub proposal: Option<TaskProposal>,
    /// The integration that published the proposal; null until the task is integrated.
    #[schema(required = true)]
    pub integration: Option<TaskIntegration>,
    /// The task worker's reports on a `no_proposal` task, newest first; the first one without a resolution waits for a lead check.
    pub reports: Vec<TaskReport>,
    /// The task's newest stored events, newest first, at most 50.
    pub history: Vec<TaskEvent>,
    /// Whether the human who made the request may turn the human review mark on, change it or clear it. False without a named human.
    pub can_change_human_review: bool,
    /// Whether the human who made the request may set the blocker or change its reason: false without a named human, and false once an agent has started the task (a claim, a task workspace or an open proposal) or it is past Todo.
    pub can_set_blocker: bool,
    /// Whether the human who made the request may clear the blocker. False without a named human or a blocker.
    pub can_clear_blocker: bool,
    /// Whether the human who made the request may mark this human task Done: false without a named human, for any other kind and for a Done or archived task. The daemon refuses it while a prerequisite is not Done.
    pub can_mark_done: bool,
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
    /// The proposal's title.
    pub title: String,
    pub state: ProposalState,
    /// The worker that holds the open review claim; null while no reviewer holds one.
    #[schema(required = true)]
    pub review_claim: Option<String>,
    /// How many files the current version adds, modifies and deletes.
    pub files: u32,
    /// The current version.
    pub version: u32,
    /// The integration checks of the current version failed, so the proposal is back with its author until a new version.
    pub checks_failed: bool,
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

/// How a lead check answered a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportResolution {
    Completed,
    SentBack,
}

/// A task worker's report that the work of a `no_proposal` task is finished, with the lead check that answered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskReport {
    /// The worker that wrote the report.
    pub by: String,
    pub note: String,
    /// UTC milliseconds.
    pub at: i64,
    /// Null while the report waits for a lead check.
    #[schema(required = true)]
    pub resolution: Option<ReportResolution>,
    /// A worker or `human:<name>`; null while the report waits.
    #[schema(required = true)]
    pub resolved_by: Option<String>,
    /// UTC milliseconds; null while the report waits.
    #[schema(required = true)]
    pub resolved_at: Option<i64>,
    /// What to change, for a report that was sent back; null otherwise.
    #[schema(required = true)]
    pub send_back_note: Option<String>,
}

impl From<service::ReportView> for TaskReport {
    fn from(report: service::ReportView) -> Self {
        use fold_store::tasks::reports::Resolution;
        Self {
            by: report.by.to_string(),
            note: report.note,
            at: report.at.as_millis(),
            resolution: report.resolution.map(|resolution| match resolution {
                Resolution::Completed => ReportResolution::Completed,
                Resolution::SentBack => ReportResolution::SentBack,
            }),
            resolved_by: report.resolved_by,
            resolved_at: report.resolved_at.map(|at| at.as_millis()),
            send_back_note: report.send_back_note,
        }
    }
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
    let rights = service::human_task_rights(&detail, human.is_some());
    let mut view = TaskView::from(detail);
    view.can_change_human_review =
        human.is_some_and(|human| service::can_change_human_review(&human.actor()));
    view.can_set_blocker = rights.set_blocker;
    view.can_clear_blocker = rights.clear_blocker;
    view.can_mark_done = rights.mark_done;
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
            kind: task.kind.into(),
            archived_at: task.archived_at.map(|at| at.as_millis()),
            created_by: task.created_by,
            created_at: task.created_at.as_millis(),
            updated_at: task.updated_at.as_millis(),
            areas_conflict: task.areas_conflict,
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
            blocker: task.hold.map(BlockerMark::from),
            status_reason: detail.status_reason,
            workspace_info: detail.workspace_info.map(|info| TaskWorkspaceInfo {
                directory: info.directory,
                base: info.base.to_string(),
                main: info.main.to_string(),
                behind: info.behind,
            }),
            activity: detail.activity.map(BoardActivity::from),
            edits_after_version: detail.edits_after_version,
            changesets: detail
                .changesets
                .into_iter()
                .map(|changeset| TaskChangeset {
                    id: changeset.id.to_string(),
                    message: changeset.message,
                    created_at: changeset.created_at.as_millis(),
                })
                .collect(),
            checkpoints: detail
                .checkpoints
                .iter()
                .map(CheckpointSummary::from)
                .collect(),
            proposal: detail.proposal.map(|proposal| TaskProposal {
                id: proposal.id.to_string(),
                title: proposal.title,
                review_claim: proposal.review_claim,
                files: proposal.files,
                state: proposal.standing.into(),
                version: proposal.version,
                checks_failed: proposal.checks_failed,
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
            reports: detail.reports.into_iter().map(TaskReport::from).collect(),
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
            can_set_blocker: false,
            can_clear_blocker: false,
            can_mark_done: false,
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
    /// Names of existing labels; at most one of them is the label of an area.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Tasks the new task needs; each adds one direct edge.
    #[serde(default)]
    pub needs: Vec<String>,
    /// The task to split: the new task becomes a linked item of its `Subtasks` checklist.
    #[serde(default)]
    pub parent: Option<String>,
    /// `proposal` when left out. A `no_proposal` task is for work that changes no project file: its task worker reports the work and a lead check completes it. A `human` task is work only a person can do: no agent takes it and a human marks it Done.
    #[serde(default)]
    pub kind: Option<TaskKind>,
}

/// The note a send-back gives the task worker of the task.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SendBackBody {
    /// What to change: plain text, not empty.
    pub note: String,
}

/// Why the task waits for a person.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BlockerBody {
    /// Plain text, not empty, at most 500 characters.
    pub reason: String,
}

/// Fields left out stay as they are.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTaskBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The whole new label set, by name; at most one of them is the label of an area.
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
    /// The checklist name. Left out, a plain step goes to the task's only checklist, or to `Subtasks` when the task has none; with two or more checklists the request is refused with `checklist_ambiguous`. A new name creates the checklist.
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
            areas_conflict: task.areas_conflict,
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
        (status = 400, description = "`invalid_request` (also with `details.reason` `two_areas`), `invalid_id`, `invalid_name`, `invalid_task_title` or `invalid_label_name`", body = ErrorEnvelope),
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
    let kind = body.kind.map_or(fold_core::TaskKind::Proposal, Into::into);
    let created = tasks
        .create_task_of_kind(&human.actor(), create, kind, &needs, parent)
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

/// Why a reader is offered for a human's feedback on a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReaderRole {
    ClaimHolder,
    AreaLead,
    MainAgent,
}

impl From<fold_core::tasks::areas::ReaderRole> for ReaderRole {
    fn from(role: fold_core::tasks::areas::ReaderRole) -> Self {
        match role {
            fold_core::tasks::areas::ReaderRole::ClaimHolder => Self::ClaimHolder,
            fold_core::tasks::areas::ReaderRole::AreaLead => Self::AreaLead,
            fold_core::tasks::areas::ReaderRole::MainAgent => Self::MainAgent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskReader {
    /// The worker, `agent` or `agent/worker`; feedback addressed to it by name reaches it.
    #[schema(example = "web-t12")]
    pub name: String,
    pub role: ReaderRole,
    /// fold supervises the reader's agent and its process is stopped or failed, so the message waits until the process runs again.
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaskReaders {
    /// The default reader first: the claim holder, else the area lead, else the main agent. Each worker once, under its first role.
    pub readers: Vec<TaskReader>,
}

/// Who a human's feedback on the task can go to: the worker that holds the implementation claim, the lead of the task's area (none for a task of Main or in no area) and the main agent. Only they answer for the task.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/tasks/{id}/readers",
    operation_id = "task_readers",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The readers, the default first", body = TaskReaders),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn task_readers(
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskReaders>, ApiError> {
    let readers = project.0.tasks.readers(id(&task)?).await?;
    Ok(Json(TaskReaders {
        readers: readers
            .into_iter()
            .map(|reader| TaskReader {
                name: reader.name.to_string(),
                role: reader.role.into(),
                stopped: reader.stopped,
            })
            .collect(),
    }))
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
        (status = 400, description = "`invalid_request` (also with `details.reason` `two_areas`), `invalid_id`, `invalid_name`, `invalid_task_title` or `invalid_label_name`", body = ErrorEnvelope),
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

/// Puts one existing label on a task and keeps the labels it already has. Putting a label that is already there changes nothing.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/tasks/{id}/labels/{name}",
    operation_id = "attach_label",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
        ("name" = String, Path, description = "The name of an existing label"),
    ),
    responses(
        (status = 200, description = "The task with its labels", body = TaskView),
        (status = 400, description = "`invalid_request` with `details.reason` `two_areas`, `invalid_id`, `invalid_name` or `invalid_label_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `label_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn attach_label(
    human: Human,
    project: Project,
    Path((_slug, task, name)): Path<(String, String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    change_label(human, project, &task, &name, true).await
}

/// Takes one label off a task and keeps the labels it still has. Taking off a label the task does not carry changes nothing.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/tasks/{id}/labels/{name}",
    operation_id = "detach_label",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
        ("name" = String, Path, description = "The name of the label"),
    ),
    responses(
        (status = 200, description = "The task with its labels", body = TaskView),
        (status = 400, description = "`invalid_request` with `details.reason` `two_areas`, `invalid_id`, `invalid_name` or `invalid_label_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn detach_label(
    human: Human,
    project: Project,
    Path((_slug, task, name)): Path<(String, String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    change_label(human, project, &task, &name, false).await
}

async fn change_label(
    human: Human,
    project: Project,
    task: &str,
    name: &str,
    attach: bool,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(task)?;
    let name = fold(LabelName::parse(name))?;
    let tasks = &project.0.tasks;
    tasks
        .change_label(&human.actor(), task, name, attach)
        .await?;
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

/// Completes a reported `no_proposal` task after the human checked its work: the report is answered, the claim released and the completion record written, so the task is Done. On a task with the human review mark, this check by a human is the human review. On a `human` task it marks the person's work Done: the completion record is written and its dependents unblock.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/tasks/{id}/complete",
    operation_id = "complete_task",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The completed task", body = TaskView),
        (status = 400, description = "`invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`, `task_kind_mismatch`, `task_not_reported` or `prerequisites_not_done` (`details.prerequisites`)", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn complete_task(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    let tasks = &project.0.tasks;
    tasks.complete_task(&human.actor(), task).await?;
    Ok(Json(task_view(tasks.get_view(task).await?, Some(&human))))
}

/// Sends a reported `no_proposal` task back to its task worker with a note: the task returns to In Progress and fold resumes the task worker.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/tasks/{id}/send-back",
    operation_id = "send_back_task",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    request_body = SendBackBody,
    responses(
        (status = 200, description = "The task, back in progress", body = TaskView),
        (status = 400, description = "`invalid_request` (an empty or too long note), `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`, `task_kind_mismatch` or `task_not_reported`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn send_back_task(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<SendBackBody>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    let tasks = &project.0.tasks;
    tasks
        .send_back_task(&human.actor(), task, &body.note)
        .await?;
    Ok(Json(task_view(tasks.get_view(task).await?, Some(&human))))
}

/// Puts a blocker on the task with its reason, or changes the reason: no agent claims the task and no lead starts a task worker for it until a human clears it. Only a task in Backlog or Todo that no agent has started gets one. The status does not change.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/tasks/{id}/blocker",
    operation_id = "set_blocker",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    request_body = BlockerBody,
    responses(
        (status = 200, description = "The task with its blocker", body = TaskView),
        (status = 400, description = "`invalid_hold_reason`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`task_hold_denied` with the detail reason `started`: the task has a claim, a task workspace or an open proposal, or is past Todo", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn set_blocker(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<BlockerBody>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    let tasks = &project.0.tasks;
    tasks.set_hold(&human.actor(), task, &body.reason).await?;
    Ok(Json(task_view(tasks.get_view(task).await?, Some(&human))))
}

/// Removes the blocker of the task, so agents can take it again. A task without a blocker is returned as it is.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/tasks/{id}/blocker",
    operation_id = "clear_blocker",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The task without a blocker", body = TaskView),
        (status = 400, description = "`invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn clear_blocker(
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    let tasks = &project.0.tasks;
    tasks.clear_hold(&human.actor(), task).await?;
    Ok(Json(task_view(tasks.get_view(task).await?, Some(&human))))
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
        (status = 409, description = "`dependency_cycle`, `dependency_exists`, `task_closed` or `checklist_ambiguous`", body = ErrorEnvelope),
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
    let item = service::NewChecklistItem {
        content: service::item_content(body.text.as_deref(), body.linked_task.as_deref())?,
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
            tasks
                .move_checklist_item(&actor, item, body.position, checklist)
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
            let wire = serde_json::to_value(TaskStatus::from(status)).unwrap();
            assert_eq!(wire, status.as_str());
        }
        for kind in [
            service::AttentionKind::Question,
            service::AttentionKind::HumanReview,
            service::AttentionKind::Stalled,
            service::AttentionKind::StaleClaim,
        ] {
            let wire = serde_json::to_value(AttentionItem::from(service::AttentionItem::of(kind)))
                .unwrap();
            assert_eq!(wire["kind"], kind.as_str());
        }
    }
}
